//! BaiduPCS-Go processor for Baidu Netdisk uploads.
//!
//! Shells out to the BaiduPCS-Go CLI (`upload <local>... <remote-dir>`).
//! The CLI's exit code does not reflect upload outcomes — `RunUpload`
//! returns nothing and exits 0 even when every file fails — so per-file
//! statuses are resolved from the marker lines it prints
//! ([`parse_upload_outcomes`]). Login state lives in BaiduPCS-Go's own
//! config directory (see `crate::baidupcs`); this processor never handles
//! credentials.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info, warn};

use super::traits::{
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, TimeAnchor,
    UploadItemStatus, UploadResultItem,
};
use super::utils::CommandOutput;
use crate::Result;
use crate::pipeline::job_queue::JobLogEntry;
use crate::utils::filename::expand_placeholders_at;

// Output markers printed by BaiduPCS-Go (hardcoded constants in its
// source, not localized). `parse_upload_outcomes` resolves per-file
// statuses from these because the CLI's exit code carries no signal.
/// `[<id>] 加入上传队列: <local path>` — establishes the task-id→file map.
const MARKER_QUEUED: &str = "加入上传队列: ";
/// `[<id>] 准备上传: <local path>` — same mapping, printed at task start.
const MARKER_PREPARING: &str = "准备上传: ";
/// Suffix carrying the final remote path on both success variants.
const MARKER_REMOTE_PATH: &str = "保存到网盘路径: ";
/// `[<id>] 上传文件成功, 保存到网盘路径: <remote>`.
const MARKER_UPLOAD_SUCCESS: &str = "上传文件成功";
/// `[<id>] 秒传成功, 保存到网盘路径: <remote>` (rapid upload).
const MARKER_RAPID_SUCCESS: &str = "秒传成功";
/// Appears in both terminal failures and retry notices; a line is only
/// terminal when it lacks [`MARKER_RETRY`].
const MARKER_UPLOAD_FAILED: &str = "上传文件失败";
/// `[<id>] 上传文件失败, 重试 <n>/<max>` — the CLI's own retry, not terminal.
const MARKER_RETRY: &str = "重试";
/// Benign skip: the remote file already exists (`目标文件已存在, 跳过...`).
const MARKER_EXISTS: &str = "已存在";
/// Benign skip under `--policy rsync`: `目标大小未发生改变, 跳过`.
const MARKER_UNCHANGED: &str = "大小未发生改变";
/// `[0] <path> 文件路径含有非法字符，已跳过!` — the file is never uploaded,
/// so this "skip" is a permanent failure.
const MARKER_ILLEGAL: &str = "非法字符";
/// `上传结束, 时间: <t>, 总大小: <s>` — the run reached its summary.
const MARKER_FINISHED: &str = "上传结束";
/// `以下文件上传失败:` — header of the terminal failure table whose rows
/// carry task IDs and local paths.
const MARKER_FAILURE_TABLE: &str = "以下文件上传失败";
/// `未检测到上传的文件.` — nothing was queued (e.g. every path was invalid).
const MARKER_NO_TASKS: &str = "未检测到上传的文件";

/// Upper bound on configured retry attempts, guarding against runaway
/// configs re-spawning a large upload dozens of times.
const MAX_ATTEMPTS_CAP: u32 = 10;

/// Same-name handling passed to `upload --policy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BaiduPcsPolicy {
    /// Skip files that already exist at the destination (CLI default).
    #[default]
    Skip,
    /// Overwrite existing files.
    Overwrite,
    /// Overwrite only files whose size changed.
    Rsync,
}

impl BaiduPcsPolicy {
    fn as_arg(self) -> &'static str {
        match self {
            BaiduPcsPolicy::Skip => "skip",
            BaiduPcsPolicy::Overwrite => "overwrite",
            BaiduPcsPolicy::Rsync => "rsync",
        }
    }
}

/// Configuration for the BaiduPCS-Go processor.
///
/// Deserialized from the JSON string in `ProcessorInput::config`. Every
/// field defaults when missing, so saved configs that pre-date newer
/// fields continue to load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BaiduPcsConfig {
    /// BaiduPCS-Go binary path. Falls back to the `BAIDUPCS_PATH`
    /// environment variable, then `BaiduPCS-Go` on `PATH`
    /// (`crate::baidupcs::resolve_binary_path`).
    pub binary_path: Option<String>,

    /// BaiduPCS-Go config directory holding the login session; exported to
    /// the child as `BAIDUPCS_GO_CONFIG_DIR`. If unset, the CLI uses its
    /// default location.
    pub config_dir: Option<String>,

    /// Remote destination directory (e.g. `/records/{streamer}/%Y-%m`).
    /// Supports placeholder expansion for `{streamer}`, `{title}`,
    /// `{streamer_id}`, `{session_id}`, `{platform}`, and chrono-style
    /// time tokens (`%Y`, `%m`, `%d`, ...). Normalized to a
    /// forward-slash, leading-`/` Netdisk path.
    pub destination_root: Option<String>,

    /// Timestamp source for time placeholder expansion.
    pub time_anchor: TimeAnchor,

    /// Same-name handling (`--policy`). Defaults to [`BaiduPcsPolicy::Skip`],
    /// which also makes job retries cheap: files uploaded by an earlier
    /// attempt are skipped instead of re-transferred.
    pub policy: BaiduPcsPolicy,

    /// Disable rapid-upload (秒传) detection (`--norapid`).
    pub norapid: bool,

    /// Free-form extra CLI arguments inserted before the file list.
    pub args: Vec<String>,

    /// Maximum upload attempts per job run (clamped to 1..=10). Each retry
    /// re-runs `upload` with only the files that have no confirmed outcome
    /// yet.
    pub max_retries: u32,

    /// Delete each local file after its upload (or benign skip) is
    /// confirmed. On a job retry, an already-absent input is treated as
    /// uploaded by the earlier attempt, mirroring
    /// `RcloneProcessor::is_confirmed_absent` move-resume semantics.
    pub remove_source_after_upload: bool,
}

impl Default for BaiduPcsConfig {
    fn default() -> Self {
        Self {
            binary_path: None,
            config_dir: None,
            destination_root: None,
            time_anchor: TimeAnchor::default(),
            policy: BaiduPcsPolicy::default(),
            norapid: false,
            args: Vec::new(),
            max_retries: 3,
            remove_source_after_upload: false,
        }
    }
}

/// Processor for uploading files to Baidu Netdisk via BaiduPCS-Go.
pub struct BaiduPcsProcessor {
    /// Binary path resolved at construction; a per-job
    /// `BaiduPcsConfig::binary_path` overrides it.
    binary_path: String,
    /// Runs assembled commands; a seam so tests can observe and stub
    /// invocations without spawning a real process.
    command_runner: Arc<dyn BaiduPcsCommandRunner>,
    /// Test override for [`crate::baidupcs::authenticator`]. `None` in
    /// production constructors — the process-wide authenticator is resolved
    /// at call time because the service container installs it after
    /// `PipelineManager` builds its processor list.
    authenticator_override: Option<Arc<dyn crate::baidupcs::BaiduPcsAuthenticator>>,
}

#[async_trait]
trait BaiduPcsCommandRunner: Send + Sync {
    async fn run(&self, command: &mut Command, context: &ProcessorContext)
    -> Result<CommandOutput>;
}

struct ProcessBaiduPcsCommandRunner;

#[async_trait]
impl BaiduPcsCommandRunner for ProcessBaiduPcsCommandRunner {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
    ) -> Result<CommandOutput> {
        super::utils::run_baidupcs_with_logs(command, Some(context.log_sink.clone())).await
    }
}

/// Per-file outcomes extracted from one `upload` run's captured logs.
#[derive(Debug, Default)]
struct UploadOutcomes {
    /// The run printed its `上传结束` summary.
    finished: bool,
    /// Input → remote path from a success marker (`None` when the marker
    /// carried no readable path).
    completed: HashMap<String, Option<String>>,
    /// Input → benign-skip reason line (already exists / size unchanged).
    skipped: HashMap<String, String>,
    /// Input → terminal error line for this run.
    failed: HashMap<String, String>,
    /// Success/benign-skip markers whose task ID never mapped to an input.
    orphan_successes: usize,
    /// Failure markers whose task ID never mapped to an input.
    orphan_failures: usize,
    /// The `以下文件上传失败` table header appeared.
    saw_failure_table: bool,
}

/// `[<id>] rest` → `(id, rest)`. IDs are numeric task counters; anything
/// else inside the brackets is not a task prefix.
fn parse_task_prefix(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix('[')?;
    let close = rest.find(']')?;
    let id = rest[..close].trim();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id, rest[close + 1..].trim_start()))
}

/// Match a path printed by BaiduPCS-Go back to one of the job's inputs:
/// exact, then separator-normalized, then by file name when that is
/// unambiguous (the CLI may clean or absolutize the path it echoes).
fn match_input<'a>(inputs: &'a [String], printed: &str) -> Option<&'a String> {
    if let Some(exact) = inputs.iter().find(|input| input.as_str() == printed) {
        return Some(exact);
    }
    let normalized = printed.replace('\\', "/");
    if let Some(matched) = inputs
        .iter()
        .find(|input| input.replace('\\', "/") == normalized)
    {
        return Some(matched);
    }
    let printed_name = Path::new(printed).file_name()?;
    let mut candidates = inputs
        .iter()
        .filter(|input| Path::new(input.as_str()).file_name() == Some(printed_name));
    let first = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// Match an input embedded somewhere in `line` (failure-table rows carry a
/// task ID column next to the path, illegal-filename lines embed the path
/// mid-sentence). Falls back to an unambiguous file name.
fn match_input_by_containment<'a>(inputs: &'a [String], line: &str) -> Option<&'a String> {
    if let Some(direct) = inputs.iter().find(|input| line.contains(input.as_str())) {
        return Some(direct);
    }
    let mut candidates = inputs.iter().filter(|input| {
        Path::new(input.as_str())
            .file_name()
            .map(|name| line.contains(&*name.to_string_lossy()))
            .unwrap_or(false)
    });
    let first = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// Resolve per-file outcomes from the captured log lines of one `upload`
/// run over `inputs`.
///
/// Marker precedence guards against the CLI's overloaded wording:
/// `跳过秒传失败, 开始秒传` matches neither the skip test (`, 跳过` /
/// `已跳过`) nor the failure test (which requires [`MARKER_UPLOAD_FAILED`]
/// without [`MARKER_RETRY`]), and `文件路径含有非法字符，已跳过` is a
/// permanent failure despite its skip wording. Unrecognized lines are
/// ignored; inputs without any marker stay unresolved and the caller
/// decides between the orphan-success escape hatch and failing the attempt.
fn parse_upload_outcomes(logs: &[JobLogEntry], inputs: &[String]) -> UploadOutcomes {
    let mut outcomes = UploadOutcomes::default();
    let mut task_to_input: HashMap<String, String> = HashMap::new();
    let mut in_failure_table = false;

    for entry in logs {
        let line = entry.message.trim();
        if line.is_empty() {
            continue;
        }

        if line.contains(MARKER_FAILURE_TABLE) {
            outcomes.saw_failure_table = true;
            in_failure_table = true;
            continue;
        }
        if line.contains(MARKER_FINISHED) {
            outcomes.finished = true;
            continue;
        }
        if line.contains(MARKER_NO_TASKS) {
            continue;
        }

        if let Some((id, rest)) = parse_task_prefix(line) {
            if let Some(printed) = rest
                .strip_prefix(MARKER_QUEUED)
                .or_else(|| rest.strip_prefix(MARKER_PREPARING))
            {
                if let Some(input) = match_input(inputs, printed.trim()) {
                    task_to_input.insert(id.to_string(), input.clone());
                }
                continue;
            }

            if rest.contains(MARKER_UPLOAD_SUCCESS) || rest.contains(MARKER_RAPID_SUCCESS) {
                let remote = rest
                    .split(MARKER_REMOTE_PATH)
                    .nth(1)
                    .map(|path| path.trim().to_string())
                    .filter(|path| !path.is_empty());
                match task_to_input.get(id) {
                    Some(input) => {
                        outcomes.completed.insert(input.clone(), remote);
                    }
                    None => outcomes.orphan_successes += 1,
                }
                continue;
            }

            if rest.contains(", 跳过") || rest.contains("已跳过") {
                if rest.contains(MARKER_ILLEGAL) {
                    // Printed with a literal `[0]` prefix and the path
                    // embedded in the sentence, not a task ID.
                    match match_input_by_containment(inputs, rest) {
                        Some(input) => {
                            outcomes.failed.insert(input.clone(), rest.to_string());
                        }
                        None => outcomes.orphan_failures += 1,
                    }
                } else if rest.contains(MARKER_EXISTS) || rest.contains(MARKER_UNCHANGED) {
                    match task_to_input.get(id) {
                        Some(input) => {
                            outcomes.skipped.insert(input.clone(), rest.to_string());
                        }
                        // A benign skip is a terminal success-like outcome,
                        // so an unmapped one feeds the same escape hatch as
                        // an unmapped success.
                        None => outcomes.orphan_successes += 1,
                    }
                } else {
                    // Remaining skip wordings (over 128 GB, unreadable
                    // file, quota exhausted) mean the file was never
                    // uploaded and never will be by this config.
                    match task_to_input.get(id) {
                        Some(input) => {
                            outcomes.failed.insert(input.clone(), rest.to_string());
                        }
                        None => outcomes.orphan_failures += 1,
                    }
                }
                continue;
            }

            if rest.contains(MARKER_UPLOAD_FAILED) && !rest.contains(MARKER_RETRY) {
                match task_to_input.get(id) {
                    Some(input) => {
                        outcomes.failed.insert(input.clone(), rest.to_string());
                    }
                    None => outcomes.orphan_failures += 1,
                }
                continue;
            }
        }

        if in_failure_table
            && let Some(input) = match_input_by_containment(inputs, line)
        {
            outcomes
                .failed
                .entry(input.clone())
                .or_insert_with(|| format!("listed in the BaiduPCS-Go failure table: {line}"));
        }
    }

    outcomes
}

impl BaiduPcsProcessor {
    /// Create a new BaiduPCS-Go processor. The binary path comes from the
    /// `BAIDUPCS_PATH` environment variable, falling back to `BaiduPCS-Go`.
    pub fn new() -> Self {
        Self {
            binary_path: crate::baidupcs::resolve_binary_path(None),
            command_runner: Arc::new(ProcessBaiduPcsCommandRunner),
            authenticator_override: None,
        }
    }

    /// Create with a custom binary path.
    pub fn with_binary_path(path: impl Into<String>) -> Self {
        Self {
            binary_path: path.into(),
            command_runner: Arc::new(ProcessBaiduPcsCommandRunner),
            authenticator_override: None,
        }
    }

    #[cfg(test)]
    fn with_command_runner(
        path: impl Into<String>,
        command_runner: Arc<dyn BaiduPcsCommandRunner>,
    ) -> Self {
        Self {
            binary_path: path.into(),
            command_runner,
            authenticator_override: None,
        }
    }

    #[cfg(test)]
    fn with_authenticator(
        mut self,
        authenticator: Arc<dyn crate::baidupcs::BaiduPcsAuthenticator>,
    ) -> Self {
        self.authenticator_override = Some(authenticator);
        self
    }

    /// The authenticator to use for stored-credential re-login, if any.
    fn authenticator(&self) -> Option<Arc<dyn crate::baidupcs::BaiduPcsAuthenticator>> {
        self.authenticator_override
            .clone()
            .or_else(crate::baidupcs::authenticator)
    }

    /// True when a `who` probe positively reports no logged-in account
    /// (uid 0). Unparseable or failed probes return `false` so an odd CLI
    /// build cannot force a login loop; the retry-path re-login still
    /// covers a genuinely broken session.
    async fn session_logged_out(
        &self,
        binary_path: &str,
        config: &BaiduPcsConfig,
        ctx: &ProcessorContext,
    ) -> bool {
        let _guard = crate::baidupcs::cli_lock().read().await;
        let mut cmd = crate::baidupcs::base_command(binary_path, config.config_dir.as_deref());
        cmd.arg("who");
        match self.command_runner.run(&mut cmd, ctx).await {
            Ok(output) => {
                let text = output
                    .logs
                    .iter()
                    .map(|entry| entry.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                matches!(crate::baidupcs::parse_who(&text), Some(account) if account.uid == 0)
            }
            Err(_) => false,
        }
    }

    /// Replay stored credentials through `BaiduPCS-Go login`, reporting the
    /// outcome to the job log. Never fails the job — a rejected re-login
    /// just leaves the session as-is and the upload attempt speaks for
    /// itself.
    async fn attempt_relogin(
        &self,
        authenticator: &Arc<dyn crate::baidupcs::BaiduPcsAuthenticator>,
        binary_path: &str,
        config: &BaiduPcsConfig,
        ctx: &ProcessorContext,
    ) {
        match authenticator
            .relogin(binary_path, config.config_dir.as_deref())
            .await
        {
            Ok(true) => {
                info!("BaiduPCS-Go re-login with stored credentials succeeded");
                ctx.info("Re-logged in to Baidu Netdisk with stored credentials");
            }
            Ok(false) => ctx.warn(
                "Automatic Baidu Netdisk re-login did not succeed; continuing with the existing session",
            ),
            Err(e) => ctx.warn(format!("Automatic Baidu Netdisk re-login failed: {e}")),
        }
    }

    /// Remote destination directory with placeholder expansion, normalized
    /// to a forward-slash Netdisk path with a leading `/` (defaults to the
    /// Netdisk root when nothing is configured).
    fn determine_remote_destination(input: &ProcessorInput, config: &BaiduPcsConfig) -> String {
        let raw = if let Some(out) = input.outputs.first() {
            out.clone()
        } else if let Some(root) = config.destination_root.as_deref() {
            root.to_string()
        } else {
            String::new()
        };

        let reference_timestamp_ms = config.time_anchor.reference_time(input).timestamp_millis();
        let expanded = expand_placeholders_at(
            &raw,
            &input.streamer_id,
            &input.session_id,
            input.streamer_name.as_deref(),
            input.session_title.as_deref(),
            input.platform.as_deref(),
            Some(reference_timestamp_ms),
        );
        Self::normalize_remote_dir(&expanded)
    }

    /// Netdisk paths are always absolute and forward-slash separated,
    /// regardless of the local OS separator.
    fn normalize_remote_dir(raw: &str) -> String {
        let cleaned = raw.replace('\\', "/");
        let trimmed = cleaned.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            "/".to_string()
        } else if trimmed.starts_with('/') {
            trimmed.to_string()
        } else {
            format!("/{trimmed}")
        }
    }

    /// Remote path of one input under `remote_dir`, used when the CLI did
    /// not echo one (benign skips, resume, the orphan-success escape
    /// hatch). Matches how `upload` lays files directly under the target
    /// directory. Splits on both separators by hand so Windows paths
    /// recorded in job rows resolve the same way on any host OS.
    fn computed_remote_path(remote_dir: &str, local_path: &str) -> String {
        let file_name = local_path
            .rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(local_path);
        format!("{}/{}", remote_dir.trim_end_matches('/'), file_name)
    }

    /// Split inputs into (present, confirmed absent). Only a positive
    /// `Ok(false)` from `try_exists` counts as absent — an I/O error must
    /// keep the input pending instead of reporting an upload that never
    /// happened (same contract as `RcloneProcessor::is_confirmed_absent`).
    async fn partition_absent_inputs(inputs: &[String]) -> (Vec<String>, Vec<String>) {
        let mut pending = Vec::new();
        let mut resumed = Vec::new();
        for input in inputs {
            if matches!(tokio::fs::try_exists(Path::new(input)).await, Ok(false)) {
                resumed.push(input.clone());
            } else {
                pending.push(input.clone());
            }
        }
        (pending, resumed)
    }

    /// Per-file sizes captured before the upload so `UploadResultItem`
    /// sizes survive `remove_source_after_upload`. Unreadable inputs are
    /// simply absent from the map.
    async fn input_size_map(inputs: &[String]) -> HashMap<String, u64> {
        let mut sizes = HashMap::with_capacity(inputs.len());
        for input in inputs {
            if let Ok(metadata) = tokio::fs::metadata(input).await {
                sizes.insert(input.clone(), metadata.len());
            }
        }
        sizes
    }
}

impl Default for BaiduPcsProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for BaiduPcsProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["baidupcs"]
    }

    fn name(&self) -> &'static str {
        "BaiduPcsProcessor"
    }

    /// `upload` natively takes N local files and one remote directory, so
    /// batches run as a single invocation.
    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        if input.inputs.is_empty() {
            return Err(crate::Error::Validation(
                "No input files provided for BaiduPcsProcessor".to_string(),
            ));
        }

        let config: BaiduPcsConfig = match input.config.as_deref() {
            Some(raw) => serde_json::from_str(raw).map_err(|e| {
                crate::Error::Validation(format!("Invalid baidupcs config JSON: {e}"))
            })?,
            None => BaiduPcsConfig::default(),
        };

        let binary_path = config
            .binary_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.binary_path.clone());

        // With remove_source_after_upload, a retried job may legitimately
        // reference sources deleted after an earlier attempt's upload.
        let (mut pending, resumed) = if config.remove_source_after_upload {
            Self::partition_absent_inputs(&input.inputs).await
        } else {
            for input_path in &input.inputs {
                let path = Path::new(input_path);
                let exists = tokio::fs::try_exists(path)
                    .await
                    .map_err(|error| crate::Error::io_path("try_exists", path, error))?;
                if !exists {
                    return Err(crate::Error::Validation(format!(
                        "Input file does not exist: {input_path}"
                    )));
                }
            }
            (input.inputs.clone(), Vec::new())
        };

        let remote_dir = Self::determine_remote_destination(input, &config);
        let file_sizes = Self::input_size_map(&pending).await;
        let total_input_size = file_sizes.values().copied().sum::<u64>();
        let attempts_max = config.max_retries.clamp(1, MAX_ATTEMPTS_CAP);

        let mut completed: HashMap<String, String> = resumed
            .iter()
            .map(|path| {
                (
                    path.clone(),
                    Self::computed_remote_path(&remote_dir, path),
                )
            })
            .collect();
        let mut skipped: HashMap<String, String> = HashMap::new();
        let mut last_errors: HashMap<String, String> = HashMap::new();
        let mut logs: Vec<JobLogEntry> = Vec::new();
        let mut attempts_used = 0u32;

        if !resumed.is_empty() {
            info!(
                resumed = resumed.len(),
                pending = pending.len(),
                "BaiduPCS-Go upload resuming; absent sources treated as previously uploaded"
            );
            ctx.info(format!(
                "Resuming BaiduPCS-Go upload with {} pending input(s); {} input(s) were already uploaded and removed",
                pending.len(),
                resumed.len()
            ));
        }

        info!(
            files = pending.len(),
            remote_dir = %remote_dir,
            policy = ?config.policy,
            "BaiduPCS-Go upload starting"
        );

        let authenticator = self.authenticator();
        let mut relogin_done = false;

        for attempt in 0..attempts_max {
            if pending.is_empty() {
                break;
            }
            if ctx.cancellation_token.is_cancelled() {
                return Err(crate::Error::Other(
                    "BaiduPCS-Go upload cancelled".to_string(),
                ));
            }
            if attempt > 0 {
                info!("Retry attempt {} for BaiduPCS-Go upload", attempt + 1);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            }
            attempts_used = attempt + 1;

            // `upload_slot` serializes BaiduPCS-Go upload processes (its
            // config-dir databases are single-writer); the read side of
            // `cli_lock` keeps `login`/`logout` from rewriting the session
            // mid-upload.
            let _upload_permit = crate::baidupcs::upload_slot()
                .acquire()
                .await
                .map_err(|_| crate::Error::Other("BaiduPCS-Go upload slot closed".to_string()))?;

            // At most one stored-credential re-login per job run: on the
            // first attempt only when a `who` probe positively reports a
            // logged-out session, on a retry unconditionally because an
            // expired session is a common cause of the failure and `who`
            // cannot see server-side expiry. Must run before the read guard
            // below — `relogin` takes the write side of `cli_lock`.
            if let Some(authenticator) = &authenticator
                && !relogin_done
                && authenticator.has_credentials(config.config_dir.as_deref()).await
                && (attempt > 0
                    || self.session_logged_out(&binary_path, &config, ctx).await)
            {
                relogin_done = true;
                self.attempt_relogin(authenticator, &binary_path, &config, ctx)
                    .await;
            }

            let _config_guard = crate::baidupcs::cli_lock().read().await;

            let mut cmd = crate::baidupcs::base_command(&binary_path, config.config_dir.as_deref());
            cmd.arg("upload");
            cmd.args(["--policy", config.policy.as_arg()]);
            if config.norapid {
                cmd.arg("--norapid");
            }
            for arg in &config.args {
                cmd.arg(arg);
            }
            for path in &pending {
                cmd.arg(path);
            }
            cmd.arg(&remote_dir);

            let command_output = match self.command_runner.run(&mut cmd, ctx).await {
                Ok(output) => output,
                Err(e) => {
                    let message = format!("Failed to execute BaiduPCS-Go: {e}");
                    for path in &pending {
                        last_errors.insert(path.clone(), message.clone());
                    }
                    continue;
                }
            };

            let exit_ok = command_output.status.success();
            let exit_code = command_output.status.code().unwrap_or(-1);
            let outcomes = parse_upload_outcomes(&command_output.logs, &pending);
            logs.extend(command_output.logs);

            let mut still_pending = Vec::new();
            let mut unresolved = Vec::new();
            for path in pending.drain(..) {
                if let Some(error) = outcomes.failed.get(&path) {
                    last_errors.insert(path.clone(), error.clone());
                    still_pending.push(path);
                } else if let Some(reason) = outcomes.skipped.get(&path) {
                    skipped.insert(path, reason.clone());
                } else if let Some(remote) = outcomes.completed.get(&path) {
                    let remote = remote
                        .clone()
                        .unwrap_or_else(|| Self::computed_remote_path(&remote_dir, &path));
                    completed.insert(path, remote);
                } else {
                    unresolved.push(path);
                }
            }

            if !unresolved.is_empty() {
                // Escape hatch for path-echo mismatches: when the run
                // finished cleanly and every unresolved input is covered by
                // a success marker that merely failed to map back to a
                // path, trust the run. Any orphan failure or the failure
                // table poisons this because the unmatched success could
                // then belong to a different file.
                let attributable = outcomes.finished
                    && exit_ok
                    && !outcomes.saw_failure_table
                    && outcomes.orphan_failures == 0
                    && outcomes.orphan_successes >= unresolved.len();
                if attributable {
                    ctx.warn(format!(
                        "BaiduPCS-Go reported {} unmatched success marker(s); treating {} remaining input(s) as uploaded",
                        outcomes.orphan_successes,
                        unresolved.len()
                    ));
                    for path in unresolved {
                        let remote = Self::computed_remote_path(&remote_dir, &path);
                        completed.insert(path, remote);
                    }
                } else {
                    let message =
                        format!("no per-file outcome reported by BaiduPCS-Go (exit code {exit_code})");
                    for path in unresolved {
                        last_errors.insert(path.clone(), message.clone());
                        still_pending.push(path);
                    }
                }
            }

            pending = still_pending;
        }

        if !pending.is_empty() {
            let detail = pending
                .iter()
                .map(|path| {
                    format!(
                        "{}: {}",
                        path,
                        last_errors
                            .get(path)
                            .map(String::as_str)
                            .unwrap_or("unknown error")
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            error!(
                failed = pending.len(),
                attempts = attempts_used,
                "BaiduPCS-Go upload failed"
            );
            return Err(crate::Error::Other(format!(
                "BaiduPCS-Go upload failed for {} file(s) after {} attempt(s): {}",
                pending.len(),
                attempts_used,
                detail
            )));
        }

        let mut removed: HashSet<String> = resumed.iter().cloned().collect();
        if config.remove_source_after_upload {
            for path in completed.keys().chain(skipped.keys()) {
                if removed.contains(path) {
                    continue;
                }
                match tokio::fs::remove_file(path).await {
                    Ok(()) => {
                        removed.insert(path.clone());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        removed.insert(path.clone());
                    }
                    Err(e) => {
                        warn!(path = %path, error = %e, "Failed to remove uploaded source");
                        ctx.warn(format!("Failed to remove uploaded source {path}: {e}"));
                    }
                }
            }
        }

        let uploads: Vec<UploadResultItem> = input
            .inputs
            .iter()
            .map(|path| {
                if let Some(remote) = completed.get(path) {
                    UploadResultItem {
                        local_path: path.clone(),
                        remote_path: Some(remote.clone()),
                        size_bytes: file_sizes.get(path).copied(),
                        status: UploadItemStatus::Completed,
                        error: None,
                    }
                } else if let Some(reason) = skipped.get(path) {
                    UploadResultItem {
                        local_path: path.clone(),
                        remote_path: Some(Self::computed_remote_path(&remote_dir, path)),
                        size_bytes: file_sizes.get(path).copied(),
                        status: UploadItemStatus::Skipped,
                        error: Some(reason.clone()),
                    }
                } else {
                    // Unreachable while the retry loop returns Err on any
                    // unresolved input; recorded as Failed so a future bug
                    // can never persist a phantom success.
                    UploadResultItem {
                        local_path: path.clone(),
                        remote_path: None,
                        size_bytes: file_sizes.get(path).copied(),
                        status: UploadItemStatus::Failed,
                        error: last_errors.get(path).cloned(),
                    }
                }
            })
            .collect();

        let duration = start.elapsed().as_secs_f64();
        info!(
            files = input.inputs.len(),
            skipped = skipped.len(),
            attempts = attempts_used,
            "BaiduPCS-Go upload completed in {:.2}s",
            duration
        );

        Ok(ProcessorOutput {
            outputs: input
                .inputs
                .iter()
                .filter(|path| !removed.contains(*path))
                .cloned()
                .collect(),
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "batch_size": input.inputs.len(),
                    "remote_dir": remote_dir,
                    "policy": config.policy.as_arg(),
                    "attempts": attempts_used,
                    "resumed_inputs": resumed.len(),
                    "removed_sources": removed.len(),
                    "skipped": skipped.len(),
                })
                .to_string(),
            ),
            items_produced: vec![],
            input_size_bytes: Some(total_input_size),
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: input.inputs.clone(),
            skipped_inputs: vec![],
            uploads,
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use super::super::test_utils::utc_datetime;
    use super::*;

    struct MockAttempt {
        exit_ok: bool,
        lines: Vec<String>,
    }

    struct MockBaiduPcsCommandRunner {
        attempts: Mutex<VecDeque<MockAttempt>>,
        commands: Mutex<Vec<Vec<String>>>,
        envs: Mutex<Vec<Vec<(String, String)>>>,
    }

    impl MockBaiduPcsCommandRunner {
        fn new(attempts: Vec<MockAttempt>) -> Self {
            Self {
                attempts: Mutex::new(attempts.into()),
                commands: Mutex::new(Vec::new()),
                envs: Mutex::new(Vec::new()),
            }
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().unwrap().clone()
        }

        fn envs(&self) -> Vec<Vec<(String, String)>> {
            self.envs.lock().unwrap().clone()
        }

        fn processor(runner: Arc<Self>) -> BaiduPcsProcessor {
            BaiduPcsProcessor::with_command_runner("BaiduPCS-Go", runner)
        }
    }

    #[async_trait]
    impl BaiduPcsCommandRunner for MockBaiduPcsCommandRunner {
        async fn run(
            &self,
            command: &mut Command,
            _context: &ProcessorContext,
        ) -> Result<CommandOutput> {
            let args: Vec<String> = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            let envs: Vec<(String, String)> = command
                .as_std()
                .get_envs()
                .filter_map(|(key, value)| {
                    value.map(|value| {
                        (
                            key.to_string_lossy().into_owned(),
                            value.to_string_lossy().into_owned(),
                        )
                    })
                })
                .collect();
            self.commands.lock().unwrap().push(args);
            self.envs.lock().unwrap().push(envs);

            let attempt = self
                .attempts
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| crate::Error::Other("unexpected BaiduPCS-Go attempt".to_string()))?;
            Ok(CommandOutput {
                status: test_exit_status(attempt.exit_ok),
                duration: 0.0,
                logs: attempt.lines.into_iter().map(JobLogEntry::info).collect(),
            })
        }
    }

    #[cfg(unix)]
    fn test_exit_status(succeeds: bool) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(if succeeds { 0 } else { 1 << 8 })
    }

    #[cfg(windows)]
    fn test_exit_status(succeeds: bool) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;

        ExitStatus::from_raw(if succeeds { 0 } else { 1 })
    }

    fn queue_line(id: u32, path: &str) -> String {
        format!("[{id}] {MARKER_QUEUED}{path}")
    }

    fn success_line(id: u32, remote: &str) -> String {
        format!("[{id}] 上传文件成功, {MARKER_REMOTE_PATH}{remote}")
    }

    fn finished_line() -> String {
        "上传结束, 时间: 10s, 总大小: 1.000000GB".to_string()
    }

    fn info_logs(lines: &[String]) -> Vec<JobLogEntry> {
        lines.iter().cloned().map(JobLogEntry::info).collect()
    }

    #[tokio::test]
    async fn upload_command_shape_env_and_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("a.flv");
        let second = temp_dir.path().join("b.flv");
        tokio::fs::write(&first, b"video-a").await.unwrap();
        tokio::fs::write(&second, b"video-b").await.unwrap();
        let first_str = first.to_string_lossy().into_owned();
        let second_str = second.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &first_str),
                queue_line(2, &second_str),
                success_line(1, "/rec/a.flv"),
                success_line(2, "/rec/b.flv"),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![first_str.clone(), second_str.clone()],
            config: Some(
                serde_json::json!({
                    "destination_root": "/rec",
                    "policy": "overwrite",
                    "norapid": true,
                    "args": ["--x"],
                    "config_dir": "/cfg/bpcs",
                })
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("shape"))
            .await
            .unwrap();

        assert_eq!(
            runner.commands(),
            vec![vec![
                "upload".to_string(),
                "--policy".to_string(),
                "overwrite".to_string(),
                "--norapid".to_string(),
                "--x".to_string(),
                first_str.clone(),
                second_str.clone(),
                "/rec".to_string(),
            ]]
        );
        assert!(
            runner.envs()[0]
                .iter()
                .any(|(key, value)| key == "BAIDUPCS_GO_CONFIG_DIR" && value == "/cfg/bpcs")
        );
        assert_eq!(output.uploads.len(), 2);
        assert!(
            output
                .uploads
                .iter()
                .all(|item| item.status == UploadItemStatus::Completed)
        );
        assert_eq!(output.uploads[0].remote_path.as_deref(), Some("/rec/a.flv"));
        assert_eq!(output.uploads[0].size_bytes, Some(7));
        assert_eq!(output.outputs, vec![first_str, second_str]);
    }

    #[test]
    fn parse_outcomes_success_skip_and_fatal_skip() {
        let inputs = vec![
            "/rec/a.flv".to_string(),
            "/rec/b.flv".to_string(),
            "/rec/c.flv".to_string(),
            "/rec/d.flv".to_string(),
        ];
        let logs = info_logs(&[
            queue_line(1, "/rec/a.flv"),
            queue_line(2, "/rec/b.flv"),
            queue_line(3, "/rec/c.flv"),
            queue_line(4, "/rec/d.flv"),
            success_line(1, "/remote/a.flv"),
            format!("[2] 秒传成功, {MARKER_REMOTE_PATH}/remote/b.flv"),
            "[3] 目标文件已存在, 跳过...".to_string(),
            "[4] 文件大小超过128G, 无法上传, 跳过...".to_string(),
            finished_line(),
        ]);

        let outcomes = parse_upload_outcomes(&logs, &inputs);

        assert!(outcomes.finished);
        assert_eq!(
            outcomes.completed.get("/rec/a.flv"),
            Some(&Some("/remote/a.flv".to_string()))
        );
        assert_eq!(
            outcomes.completed.get("/rec/b.flv"),
            Some(&Some("/remote/b.flv".to_string()))
        );
        assert!(outcomes.skipped.contains_key("/rec/c.flv"));
        assert!(outcomes.failed.contains_key("/rec/d.flv"));
        assert_eq!(outcomes.orphan_successes, 0);
        assert_eq!(outcomes.orphan_failures, 0);
        assert!(!outcomes.saw_failure_table);
    }

    #[test]
    fn parse_outcomes_ignores_retry_and_rapid_fallback_notes() {
        let inputs = vec!["/rec/a.flv".to_string()];
        let logs = info_logs(&[
            queue_line(1, "/rec/a.flv"),
            "[1] 上传文件失败, 上传文件错误, 重试 1/3".to_string(),
            "[1] 跳过秒传失败, 开始秒传...".to_string(),
            success_line(1, "/remote/a.flv"),
            finished_line(),
        ]);

        let outcomes = parse_upload_outcomes(&logs, &inputs);

        assert!(outcomes.failed.is_empty());
        assert!(outcomes.completed.contains_key("/rec/a.flv"));
    }

    #[test]
    fn parse_outcomes_failure_table_and_terminal_failure() {
        let inputs = vec!["/rec/a.flv".to_string(), "/rec/b.flv".to_string()];
        let logs = info_logs(&[
            queue_line(1, "/rec/a.flv"),
            queue_line(2, "/rec/b.flv"),
            success_line(1, "/remote/a.flv"),
            "[2] 上传文件失败, 上传文件错误".to_string(),
            finished_line(),
            "以下文件上传失败: ".to_string(),
            "任务ID  本地路径".to_string(),
            "2       /rec/b.flv".to_string(),
        ]);

        let outcomes = parse_upload_outcomes(&logs, &inputs);

        assert!(outcomes.saw_failure_table);
        assert!(outcomes.completed.contains_key("/rec/a.flv"));
        assert!(outcomes.failed.contains_key("/rec/b.flv"));
        assert_eq!(outcomes.orphan_failures, 0);
    }

    #[test]
    fn parse_outcomes_illegal_filename_is_permanent_failure() {
        let inputs = vec!["/rec/bad+name.flv".to_string()];
        let logs = info_logs(&[
            "[0] /rec/bad+name.flv 文件路径含有非法字符，已跳过!".to_string(),
            "未检测到上传的文件.".to_string(),
        ]);

        let outcomes = parse_upload_outcomes(&logs, &inputs);

        assert!(outcomes.failed.contains_key("/rec/bad+name.flv"));
        assert!(!outcomes.finished);
    }

    #[test]
    fn parse_outcomes_unmapped_markers_become_orphans() {
        let inputs = vec!["/rec/a.flv".to_string()];
        let logs = info_logs(&[
            queue_line(1, "/other/unrelated.mp4"),
            success_line(1, "/remote/unrelated.mp4"),
            "[2] 上传文件失败, 上传文件错误".to_string(),
            finished_line(),
        ]);

        let outcomes = parse_upload_outcomes(&logs, &inputs);

        assert!(outcomes.completed.is_empty());
        assert_eq!(outcomes.orphan_successes, 1);
        assert_eq!(outcomes.orphan_failures, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_only_files_without_confirmed_outcome() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("a.flv");
        let second = temp_dir.path().join("b.flv");
        tokio::fs::write(&first, b"video-a").await.unwrap();
        tokio::fs::write(&second, b"video-b").await.unwrap();
        let first_str = first.to_string_lossy().into_owned();
        let second_str = second.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
            // Exit code 0 despite a failed file: only the failure table
            // reports it.
            MockAttempt {
                exit_ok: true,
                lines: vec![
                    queue_line(1, &first_str),
                    queue_line(2, &second_str),
                    success_line(1, "/rec/a.flv"),
                    finished_line(),
                    "以下文件上传失败: ".to_string(),
                    format!("2  {second_str}"),
                ],
            },
            MockAttempt {
                exit_ok: true,
                lines: vec![
                    queue_line(1, &second_str),
                    success_line(1, "/rec/b.flv"),
                    finished_line(),
                ],
            },
        ]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![first_str.clone(), second_str.clone()],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("retry"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands.len(), 2);
        assert!(commands[0].contains(&first_str));
        assert!(commands[0].contains(&second_str));
        assert!(!commands[1].contains(&first_str));
        assert!(commands[1].contains(&second_str));
        assert!(
            output
                .uploads
                .iter()
                .all(|item| item.status == UploadItemStatus::Completed)
        );
    }

    #[tokio::test]
    async fn success_markers_survive_nonzero_exit() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: false,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("nonzero-exit"))
            .await
            .unwrap();

        assert_eq!(runner.commands().len(), 1);
        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
    }

    #[tokio::test(start_paused = true)]
    async fn fails_after_exhausting_attempts() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let attempt = || MockAttempt {
            exit_ok: false,
            lines: vec![queue_line(1, &file_str)],
        };
        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![attempt(), attempt()]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![file_str.clone()],
            config: Some(r#"{"destination_root":"/rec","max_retries":2}"#.to_string()),
            ..Default::default()
        };

        let error = processor
            .process(&input, &ProcessorContext::noop("exhausted"))
            .await
            .unwrap_err();

        assert_eq!(runner.commands().len(), 2);
        let message = error.to_string();
        assert!(message.contains("after 2 attempt(s)"), "got: {message}");
        assert!(message.contains("no per-file outcome"), "got: {message}");
    }

    #[tokio::test]
    async fn missing_input_rejected_without_remove_mode() {
        let temp_dir = tempfile::tempdir().unwrap();
        let missing = temp_dir.path().join("missing.flv");

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(Vec::new()));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![missing.to_string_lossy().into_owned()],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let error = processor
            .process(&input, &ProcessorContext::noop("missing"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Input file does not exist"));
        assert!(runner.commands().is_empty());
    }

    #[tokio::test]
    async fn remove_source_deletes_confirmed_uploads() {
        let temp_dir = tempfile::tempdir().unwrap();
        let uploaded = temp_dir.path().join("a.flv");
        let skipped = temp_dir.path().join("b.flv");
        tokio::fs::write(&uploaded, b"video-a").await.unwrap();
        tokio::fs::write(&skipped, b"video-b").await.unwrap();
        let uploaded_str = uploaded.to_string_lossy().into_owned();
        let skipped_str = skipped.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &uploaded_str),
                queue_line(2, &skipped_str),
                success_line(1, "/rec/a.flv"),
                "[2] 目标文件已存在, 跳过...".to_string(),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![uploaded_str.clone(), skipped_str.clone()],
            config: Some(
                r#"{"destination_root":"/rec","remove_source_after_upload":true}"#.to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("remove-source"))
            .await
            .unwrap();

        assert!(!uploaded.exists());
        assert!(!skipped.exists());
        assert!(output.outputs.is_empty());
        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
        assert_eq!(output.uploads[1].status, UploadItemStatus::Skipped);
        // Sizes were captured before deletion.
        assert_eq!(output.uploads[0].size_bytes, Some(7));
    }

    #[tokio::test]
    async fn remove_source_resume_skips_absent_inputs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let already_uploaded = temp_dir.path().join("gone.flv");
        let pending = temp_dir.path().join("pending.flv");
        tokio::fs::write(&pending, b"video").await.unwrap();
        let gone_str = already_uploaded.to_string_lossy().into_owned();
        let pending_str = pending.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &pending_str),
                success_line(1, "/rec/pending.flv"),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![gone_str.clone(), pending_str.clone()],
            config: Some(
                r#"{"destination_root":"/rec","remove_source_after_upload":true}"#.to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("resume"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands.len(), 1);
        assert!(!commands[0].contains(&gone_str));
        assert!(commands[0].contains(&pending_str));
        assert!(
            output
                .uploads
                .iter()
                .all(|item| item.status == UploadItemStatus::Completed)
        );
        assert_eq!(
            output.uploads[0].remote_path.as_deref(),
            Some("/rec/gone.flv")
        );
    }

    #[tokio::test]
    async fn unmatched_successes_complete_unresolved_inputs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        // The CLI echoes a path shape that matches no input, so the success
        // marker cannot be attributed; the clean-finish escape hatch must
        // still complete the file.
        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, "/entirely/different/echo.mp4"),
                success_line(1, "/rec/echo.mp4"),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![file_str.clone()],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("escape-hatch"))
            .await
            .unwrap();

        assert_eq!(runner.commands().len(), 1);
        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
        assert_eq!(output.uploads[0].remote_path.as_deref(), Some("/rec/a.flv"));
    }

    #[tokio::test]
    async fn destination_placeholders_expand_with_time_anchor() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let created_at = utc_datetime(2026, 8, 5, 12, 0, 0);
        let expected_dir = format!(
            "/rec/StreamerName/{}",
            pipeline_common::expand_path_template_at(
                "%Y%m",
                Some(created_at.timestamp_millis())
            )
        );
        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, &format!("{expected_dir}/a.flv")),
                finished_line(),
            ],
        }]));
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec/{streamer}/%Y%m"}"#.to_string()),
            streamer_name: Some("StreamerName".to_string()),
            created_at,
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("placeholders"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands[0].last(), Some(&expected_dir));
    }

    #[tokio::test]
    async fn invalid_config_json_is_rejected() {
        let processor = MockBaiduPcsCommandRunner::processor(Arc::new(
            MockBaiduPcsCommandRunner::new(Vec::new()),
        ));
        let input = ProcessorInput {
            inputs: vec!["/rec/a.flv".to_string()],
            config: Some("not json".to_string()),
            ..Default::default()
        };

        let error = processor
            .process(&input, &ProcessorContext::noop("bad-config"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Invalid baidupcs config JSON"));
    }

    #[test]
    fn normalize_remote_dir_forces_absolute_forward_slash() {
        assert_eq!(BaiduPcsProcessor::normalize_remote_dir(""), "/");
        assert_eq!(BaiduPcsProcessor::normalize_remote_dir("/"), "/");
        assert_eq!(BaiduPcsProcessor::normalize_remote_dir("rec/a/"), "/rec/a");
        assert_eq!(
            BaiduPcsProcessor::normalize_remote_dir("\\rec\\a"),
            "/rec/a"
        );
        assert_eq!(BaiduPcsProcessor::normalize_remote_dir(" /rec "), "/rec");
    }

    #[test]
    fn computed_remote_path_joins_file_name() {
        assert_eq!(
            BaiduPcsProcessor::computed_remote_path("/rec", "D:\\videos\\a.flv"),
            "/rec/a.flv"
        );
        assert_eq!(
            BaiduPcsProcessor::computed_remote_path("/", "/videos/a.flv"),
            "/a.flv"
        );
    }

    struct MockAuthenticator {
        has_credentials: bool,
        relogin_result: bool,
        relogin_calls: std::sync::atomic::AtomicUsize,
    }

    impl MockAuthenticator {
        fn new(has_credentials: bool, relogin_result: bool) -> Arc<Self> {
            Arc::new(Self {
                has_credentials,
                relogin_result,
                relogin_calls: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn relogin_calls(&self) -> usize {
            self.relogin_calls.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    #[async_trait]
    impl crate::baidupcs::BaiduPcsAuthenticator for MockAuthenticator {
        async fn has_credentials(&self, _config_dir: Option<&str>) -> bool {
            self.has_credentials
        }

        async fn save_credentials(
            &self,
            _config_dir: Option<&str>,
            _material: &crate::baidupcs::LoginMaterial,
        ) -> Result<()> {
            Ok(())
        }

        async fn delete_credentials(&self, _config_dir: Option<&str>) -> Result<()> {
            Ok(())
        }

        async fn relogin(&self, _binary_path: &str, _config_dir: Option<&str>) -> Result<bool> {
            self.relogin_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(self.relogin_result)
        }
    }

    fn who_line(uid: u64) -> String {
        format!("当前帐号 uid: {uid}, 用户名: some_user, 性别: unknown, 年龄: 0.0")
    }

    #[tokio::test]
    async fn logged_out_session_triggers_relogin_before_upload() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
            // The pre-flight `who` probe reports no account.
            MockAttempt {
                exit_ok: true,
                lines: vec![who_line(0)],
            },
            MockAttempt {
                exit_ok: true,
                lines: vec![
                    queue_line(1, &file_str),
                    success_line(1, "/rec/a.flv"),
                    finished_line(),
                ],
            },
        ]));
        let authenticator = MockAuthenticator::new(true, true);
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
            .with_authenticator(authenticator.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("relogin-preflight"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0], vec!["who".to_string()]);
        assert_eq!(commands[1][0], "upload");
        assert_eq!(authenticator.relogin_calls(), 1);
        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
    }

    #[tokio::test]
    async fn logged_in_session_skips_relogin() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
            MockAttempt {
                exit_ok: true,
                lines: vec![who_line(42)],
            },
            MockAttempt {
                exit_ok: true,
                lines: vec![
                    queue_line(1, &file_str),
                    success_line(1, "/rec/a.flv"),
                    finished_line(),
                ],
            },
        ]));
        let authenticator = MockAuthenticator::new(true, true);
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
            .with_authenticator(authenticator.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("relogin-skip"))
            .await
            .unwrap();

        assert_eq!(authenticator.relogin_calls(), 0);
        assert_eq!(runner.commands().len(), 2, "who probe + upload");
    }

    #[tokio::test(start_paused = true)]
    async fn failed_attempt_triggers_relogin_before_retry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![
            // Pre-flight probe: logged in, so no up-front re-login.
            MockAttempt {
                exit_ok: true,
                lines: vec![who_line(42)],
            },
            // First upload attempt fails without per-file outcomes (the
            // shape an expired session produces).
            MockAttempt {
                exit_ok: false,
                lines: vec![queue_line(1, &file_str)],
            },
            MockAttempt {
                exit_ok: true,
                lines: vec![
                    queue_line(1, &file_str),
                    success_line(1, "/rec/a.flv"),
                    finished_line(),
                ],
            },
        ]));
        let authenticator = MockAuthenticator::new(true, true);
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
            .with_authenticator(authenticator.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("relogin-retry"))
            .await
            .unwrap();

        assert_eq!(authenticator.relogin_calls(), 1);
        assert_eq!(runner.commands().len(), 3, "who + failed upload + retry");
        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
    }

    #[tokio::test]
    async fn no_stored_credentials_skips_probe_entirely() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("a.flv");
        tokio::fs::write(&file, b"video").await.unwrap();
        let file_str = file.to_string_lossy().into_owned();

        let runner = Arc::new(MockBaiduPcsCommandRunner::new(vec![MockAttempt {
            exit_ok: true,
            lines: vec![
                queue_line(1, &file_str),
                success_line(1, "/rec/a.flv"),
                finished_line(),
            ],
        }]));
        let authenticator = MockAuthenticator::new(false, true);
        let processor = MockBaiduPcsCommandRunner::processor(runner.clone())
            .with_authenticator(authenticator.clone());
        let input = ProcessorInput {
            inputs: vec![file_str],
            config: Some(r#"{"destination_root":"/rec"}"#.to_string()),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("no-stored"))
            .await
            .unwrap();

        assert_eq!(authenticator.relogin_calls(), 0);
        let commands = runner.commands();
        assert_eq!(commands.len(), 1, "no who probe without stored credentials");
        assert_eq!(commands[0][0], "upload");
    }

    #[test]
    fn config_defaults_apply_for_missing_fields() {
        let config: BaiduPcsConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.policy, BaiduPcsPolicy::Skip);
        assert_eq!(config.max_retries, 3);
        assert!(!config.norapid);
        assert!(!config.remove_source_after_upload);
        assert!(config.args.is_empty());
    }
}
