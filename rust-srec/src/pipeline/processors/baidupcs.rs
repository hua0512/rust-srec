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
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tracing::{error, info, warn};

use super::traits::{
    ConfirmedUpload, LocalFileFingerprint, Processor, ProcessorContext, ProcessorInput,
    ProcessorOutput, ProcessorType, TimeAnchor, UploadItemStatus, UploadResultItem,
};
#[cfg(test)]
use super::upload_runner::{MARKER_QUEUED, MARKER_REMOTE_PATH};
use super::upload_runner::{
    UploadCounts, UploadTracker, parse_upload_outcomes, run_baidupcs_with_logs,
};
use super::utils::CommandOutput;
use crate::Result;
use crate::domain::processor::{BaiduPcsAuthenticator, JobLogEntry, ProgressKind};
use crate::domain::proxy::{
    OutputScrubber, ProxyRoute, ProxyRouteResolver, ProxyTarget, upload_target,
};

/// Upper bound on configured retry attempts, guarding against runaway
/// configs re-spawning a large upload dozens of times.
const MAX_ATTEMPTS_CAP: u32 = 10;

/// Bytes of local paths handed to one `upload` invocation. Windows limits a
/// command line to 32,767 characters and the fixed arguments, the remote
/// directory and the quoting of each path take the rest; a batch that does
/// not fit runs as several invocations within the same attempt.
const UPLOAD_ARGS_BUDGET: usize = 24 * 1024;

/// Split `paths` into runs whose summed length (plus quoting and a separator
/// per path) stays within `budget`. A single path longer than the budget
/// forms its own run so nothing is dropped.
fn chunk_by_argument_budget(paths: &[String], budget: usize) -> Vec<Vec<String>> {
    let mut chunks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0usize;
    for path in paths {
        let cost = path.len().saturating_add(3);
        if !current.is_empty() && used.saturating_add(cost) > budget {
            chunks.push(std::mem::take(&mut current));
            used = 0;
        }
        current.push(path.clone());
        used = used.saturating_add(cost);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

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
    /// `{streamer_id}`, `{session_id}`, `{platform}`, and strftime-style
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
    /// `inputs::is_confirmed_absent` move-resume semantics.
    pub remove_source_after_upload: bool,

    /// How BaiduPCS-Go connects, re-logins included; `inherit`, the
    /// default, follows the global route.
    pub proxy_route: ProxyRoute,
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
            proxy_route: ProxyRoute::Inherit,
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
    /// Override for [`crate::baidupcs::authenticator`]. `None` in
    /// production constructors — the process-wide authenticator is resolved
    /// at call time because the service container installs it after
    /// `PipelineManager` builds its processor list.
    authenticator_override: Option<Arc<dyn BaiduPcsAuthenticator>>,
    /// See [`UPLOAD_ARGS_BUDGET`]; lowered by tests to exercise chunking.
    upload_args_budget: usize,
    /// Resolves each step's proxy route; see [`upload_target`].
    routes: Option<Arc<dyn ProxyRouteResolver>>,
}

#[async_trait]
trait BaiduPcsCommandRunner: Send + Sync {
    /// An `upload` invocation's output lines feed `tracker`; other commands
    /// have none. `config_lease` is the caller's read side of
    /// [`crate::baidupcs::cli_lock`]; the runner keeps it until the process
    /// has exited.
    async fn run(
        &self,
        command: Command,
        context: &ProcessorContext,
        tracker: Option<Arc<UploadTracker>>,
        config_lease: &ConfigLease,
    ) -> Result<CommandOutput>;

    /// Probe the active Baidu account UID under the caller's held
    /// `config_lease`. Returns `None` when logged out (`uid == 0`) or when the
    /// probe fails/cannot be parsed, so retry carry-forward fails closed.
    async fn active_account_uid(
        &self,
        binary_path: &str,
        config_dir: Option<&str>,
        context: &ProcessorContext,
        config_lease: &ConfigLease,
    ) -> Option<u64>;
}

/// The read side of [`crate::baidupcs::cli_lock`], shared with the tasks that
/// own running BaiduPCS-Go processes.
type ConfigLease = Arc<tokio::sync::OwnedRwLockReadGuard<()>>;

async fn config_lease() -> ConfigLease {
    Arc::new(crate::baidupcs::cli_lock().clone().read_owned().await)
}

/// Held from spawning a BaiduPCS-Go process until it has exited. Its upload
/// database is single-writer, and a cancelled or timed-out job ends before
/// its killed process exits, so the next process must wait here rather than
/// for the job.
fn process_lock() -> &'static Arc<tokio::sync::Mutex<()>> {
    static LOCK: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
    LOCK.get_or_init(Arc::default)
}

struct ProcessBaiduPcsCommandRunner;

#[async_trait]
impl BaiduPcsCommandRunner for ProcessBaiduPcsCommandRunner {
    async fn run(
        &self,
        mut command: Command,
        context: &ProcessorContext,
        tracker: Option<Arc<UploadTracker>>,
        config_lease: &ConfigLease,
    ) -> Result<CommandOutput> {
        let process_lease = match process_lock().clone().try_lock_owned() {
            Ok(lease) => lease,
            Err(_) => {
                context.info("Waiting for the previous BaiduPCS-Go process to exit");
                process_lock().clone().lock_owned().await
            }
        };
        let config_lease = Arc::clone(config_lease);
        let token = tokio_util::sync::CancellationToken::new();
        // Dropping this future (job cancellation or timeout) stops the
        // process; the task below keeps both leases until it has exited.
        let _stop_on_drop = token.clone().drop_guard();
        let log_sink = context.log_sink.clone();
        tokio::spawn(async move {
            let _leases = (process_lease, config_lease);
            run_baidupcs_with_logs(&mut command, tracker, Some(log_sink), &token).await
        })
        .await
        .map_err(|error| crate::Error::Other(format!("BaiduPCS-Go task failed: {error}")))?
    }

    async fn active_account_uid(
        &self,
        binary_path: &str,
        config_dir: Option<&str>,
        context: &ProcessorContext,
        config_lease: &ConfigLease,
    ) -> Option<u64> {
        let mut cmd = crate::baidupcs::base_command(binary_path, config_dir);
        cmd.arg("who");
        let output = self
            .run(cmd, &context.without_job_log(), None, config_lease)
            .await
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = output
            .logs
            .iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        crate::baidupcs::parse_who(&text)
            .map(|account| account.uid)
            .filter(|uid| *uid != 0)
    }
}

impl BaiduPcsProcessor {
    /// Create a new BaiduPCS-Go processor. The binary path comes from the
    /// `BAIDUPCS_PATH` environment variable, falling back to `BaiduPCS-Go`.
    pub fn new() -> Self {
        Self {
            binary_path: crate::baidupcs::resolve_binary_path(None),
            command_runner: Arc::new(ProcessBaiduPcsCommandRunner),
            authenticator_override: None,
            upload_args_budget: UPLOAD_ARGS_BUDGET,
            routes: None,
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
            upload_args_budget: UPLOAD_ARGS_BUDGET,
            routes: None,
        }
    }

    /// Resolves each step's proxy route through `routes`.
    pub fn with_routes(mut self, routes: Arc<dyn ProxyRouteResolver>) -> Self {
        self.routes = Some(routes);
        self
    }

    /// Override the authenticator used for automatic re-login.
    pub fn with_authenticator(mut self, authenticator: Arc<dyn BaiduPcsAuthenticator>) -> Self {
        self.authenticator_override = Some(authenticator);
        self
    }

    #[cfg(test)]
    fn with_upload_args_budget(mut self, budget: usize) -> Self {
        self.upload_args_budget = budget;
        self
    }

    /// The authenticator to use for stored-credential re-login, if any.
    fn authenticator(&self) -> Option<Arc<dyn BaiduPcsAuthenticator>> {
        self.authenticator_override
            .clone()
            .or_else(crate::baidupcs::authenticator)
    }

    /// True when a `who` probe positively reports no logged-in account
    /// (uid 0). Unparseable or failed probes return `false` so an odd CLI
    /// build cannot force a login loop; the retry-path re-login still
    /// covers a genuinely broken session. The probe prints the account's
    /// uid and name, so it runs without the job log.
    async fn session_logged_out(
        &self,
        binary_path: &str,
        config: &BaiduPcsConfig,
        ctx: &ProcessorContext,
    ) -> bool {
        let lease = config_lease().await;
        let mut cmd = crate::baidupcs::base_command(binary_path, config.config_dir.as_deref());
        cmd.arg("who");
        match self
            .command_runner
            .run(cmd, &ctx.without_job_log(), None, &lease)
            .await
        {
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
        authenticator: &Arc<dyn BaiduPcsAuthenticator>,
        binary_path: &str,
        config: &BaiduPcsConfig,
        proxy: &ProxyTarget,
        ctx: &ProcessorContext,
    ) {
        match authenticator
            .relogin(binary_path, config.config_dir.as_deref(), proxy)
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

        let reference_timestamp_ms = config.time_anchor.reference_time(input).as_millisecond();
        let expanded = input.expand_placeholders(&raw, Some(reference_timestamp_ms));
        Self::normalize_remote_dir(&expanded)
    }

    /// Normalize a Netdisk path with Go `path.Clean` semantics: forward-slash
    /// separated, leading `/`, repeated slashes and `.` collapsed, `..`
    /// resolved, and no trailing `/` (except root `/`). Does not trim
    /// whitespace inside path segments so applying it repeatedly is strictly
    /// idempotent (for example `/rec/daily /` -> `/rec/daily `).
    fn normalize_remote_path(raw: &str) -> String {
        let mut segments: Vec<&str> = Vec::new();
        for segment in raw.split(['/', '\\']) {
            match segment {
                "" | "." => {}
                ".." => {
                    segments.pop();
                }
                other => segments.push(other),
            }
        }
        if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", segments.join("/"))
        }
    }

    /// Netdisk paths are always absolute and forward-slash separated,
    /// regardless of the local OS separator. Trims outer whitespace from the
    /// configured destination once before cleaning.
    fn normalize_remote_dir(raw: &str) -> String {
        Self::normalize_remote_path(raw.trim())
    }

    /// Snapshot `(size, mtime, file_id, change_time, content_sha256)` from a
    /// single open file handle so carry-forward, in-run retries, and source
    /// cleanup detect both same-size/preserved-mtime replacements and in-place
    /// content edits.
    async fn local_file_fingerprint(path: &Path) -> Option<LocalFileFingerprint> {
        let path = path.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let mut file = std::fs::File::open(&path).ok()?;
            fingerprint_from_file(&mut file)
        })
        .await
        .ok()
        .flatten()
    }

    /// Revalidate `path` against `expected` immediately before deleting it so
    /// a source replaced after earlier checks is never deleted.
    async fn verify_and_remove_source(
        path: &str,
        expected: LocalFileFingerprint,
        ctx: &ProcessorContext,
    ) -> bool {
        let path_buf = PathBuf::from(path);
        let outcome =
            tokio::task::spawn_blocking(move || verify_and_remove_sync(&path_buf, expected)).await;
        match outcome {
            Ok(VerifiedRemoval::Removed | VerifiedRemoval::AlreadyAbsent) => true,
            Ok(VerifiedRemoval::Changed) => {
                ctx.warn(format!(
                    "Skipping source removal for {path}: local file changed since it was uploaded"
                ));
                false
            }
            Ok(VerifiedRemoval::Failed(error)) => {
                warn!(path = %path, %error, "Failed to remove uploaded source");
                ctx.warn(format!("Failed to remove uploaded source {path}: {error}"));
                false
            }
            Err(error) => {
                warn!(path = %path, %error, "Source removal task failed");
                ctx.warn(format!("Failed to remove uploaded source {path}: {error}"));
                false
            }
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
        let dir = Self::normalize_remote_path(remote_dir);
        format!("{}/{}", dir.trim_end_matches('/'), file_name)
    }
}

impl BaiduPcsProcessor {
    /// Per-file results in input order: completed and skipped files as the CLI
    /// reported them, every other input failed with its last error.
    fn upload_results(
        input: &ProcessorInput,
        completed: &HashMap<String, String>,
        skipped: &HashMap<String, String>,
        last_errors: &HashMap<String, String>,
        file_sizes: &HashMap<String, u64>,
        remote_dir: &str,
    ) -> Vec<UploadResultItem> {
        input
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
                        remote_path: Some(Self::computed_remote_path(remote_dir, path)),
                        size_bytes: file_sizes.get(path).copied(),
                        status: UploadItemStatus::Skipped,
                        error: Some(reason.clone()),
                    }
                } else {
                    UploadResultItem {
                        local_path: path.clone(),
                        remote_path: None,
                        size_bytes: file_sizes.get(path).copied(),
                        status: UploadItemStatus::Failed,
                        error: last_errors.get(path).cloned(),
                    }
                }
            })
            .collect()
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
        ProcessorType::Upload
    }

    fn runs_one_at_a_time(&self) -> bool {
        // BaiduPCS-Go's upload database in its config directory is
        // single-writer.
        true
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

    fn carries_forward_completed_uploads(&self) -> bool {
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

        let config: BaiduPcsConfig =
            super::utils::parse_config_strict(input.config.as_deref(), "baidupcs")?;
        let proxy = upload_target(self.routes.as_deref(), &config.proxy_route).await?;
        let scrubber = OutputScrubber::for_target(&proxy);
        let ctx = &ctx.scrubbing(&scrubber);
        let result = async {

        let binary_path = config
            .binary_path
            .as_deref()
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.binary_path.clone());

        // With remove_source_after_upload, a retried job may legitimately
        // reference sources deleted after an earlier attempt's upload.
        let (mut pending, resumed) = if config.remove_source_after_upload && ctx.is_retry {
            super::inputs::partition_absent_inputs(&input.inputs).await
        } else {
            super::utils::ensure_inputs_exist(&input.inputs).await?;
            (input.inputs.clone(), Vec::new())
        };

        let remote_dir = Self::determine_remote_destination(input, &config);

        // `upload` lays every file directly under `remote_dir`, so two inputs
        // with the same file name (fan-in from two branches) would overwrite or
        // skip each other remotely. Refuse the job before any transfer.
        let mut remote_names: HashMap<String, &String> = HashMap::new();
        for path in &pending {
            let name = path
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(path)
                .to_string();
            if let Some(earlier) = remote_names.insert(name.clone(), path) {
                return Err(crate::Error::Validation(format!(
                    "Inputs {earlier} and {path} would both upload as {}/{name}; rename one or upload them in separate steps",
                    remote_dir.trim_end_matches('/')
                )));
            }
        }

        let file_sizes = super::inputs::input_size_map(&pending).await;
        let total_input_size = file_sizes.values().copied().sum::<u64>();
        let job_inputs = pending.clone();
        let configured_attempts = config.max_retries.clamp(1, MAX_ATTEMPTS_CAP);
        let attempts_max = if ctx.has_step_retry {
            if configured_attempts > 1 {
                info!(
                    job_id = %ctx.job_id,
                    configured_attempts,
                    "Workflow step retry policy is active; deferring BaiduPCS-Go retries to the DAG scheduler"
                );
            }
            1
        } else {
            configured_attempts
        };

        let mut completed: HashMap<String, String> = resumed
            .iter()
            .map(|path| (path.clone(), Self::computed_remote_path(&remote_dir, path)))
            .collect();
        // Track the Baidu account UID and local file version behind every
        // retained remote outcome so carry-forward, in-run retries, and source
        // cleanup never trust a different account or a replaced local file.
        let mut completed_accounts: HashMap<String, u64> = HashMap::new();
        let mut completed_fingerprints: HashMap<String, LocalFileFingerprint> = HashMap::new();
        let mut prior_carry_applied = false;
        let mut skipped: HashMap<String, String> = HashMap::new();
        let mut skipped_accounts: HashMap<String, u64> = HashMap::new();
        let mut skipped_fingerprints: HashMap<String, LocalFileFingerprint> = HashMap::new();
        let mut permanent: HashMap<String, String> = HashMap::new();
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
                super::utils::wait_to_retry(
                    ctx,
                    ProgressKind::BaiduPcs,
                    attempt,
                    attempts_max,
                    &format!(
                        "BaiduPCS-Go upload attempt {attempt} of {attempts_max} left {} file(s) not uploaded",
                        pending.len()
                    ),
                )
                .await;
            }
            attempts_used = attempt + 1;

            // Shows the job's counts until the invocation reports.
            ctx.progress.report(
                UploadCounts::of_job(&job_inputs, &file_sizes, &completed, &skipped, &permanent)
                    .snapshot(),
            );

            // At most one stored-credential re-login per job run: on the
            // first attempt of a fresh job only when a `who` probe positively
            // reports a logged-out session; on an in-run or workflow-scheduled
            // retry unconditionally because an expired session is a common
            // cause of the failure and `who` cannot see server-side expiry.
            // Must run before the read guard below — `relogin` takes the write
            // side of `cli_lock`.
            if let Some(authenticator) = &authenticator
                && !relogin_done
                && authenticator
                    .has_credentials(config.config_dir.as_deref())
                    .await
                && (attempt > 0
                    || ctx.is_retry
                    || self.session_logged_out(&binary_path, &config, ctx).await)
            {
                relogin_done = true;
                self.attempt_relogin(authenticator, &binary_path, &config, &proxy, ctx)
                    .await;
            }

            // The read side of `cli_lock` keeps `login`/`logout` from
            // rewriting the session mid-upload.
            let config_lease = config_lease().await;
            let active_uid = self
                .command_runner
                .active_account_uid(&binary_path, config.config_dir.as_deref(), ctx, &config_lease)
                .await;

            // Carry forward files confirmed by an earlier job attempt only
            // after re-login and under `config_lease`, and only when the
            // active account UID, cleaned remote path, and local file version
            // all match. If the active account or file version cannot be
            // verified or changed, fail closed and upload the current file.
            if ctx.is_retry && !prior_carry_applied {
                prior_carry_applied = true;
                if let Some(uid) = active_uid
                    && !ctx.prior_completed_uploads.is_empty()
                {
                    let mut carried = Vec::new();
                    for path in &pending {
                        let expected_remote = Self::computed_remote_path(&remote_dir, path);
                        let Some(prior) = ctx.prior_completed_uploads.get(path) else {
                            continue;
                        };
                        if prior.account_uid != uid
                            || Self::normalize_remote_path(&prior.remote_path) != expected_remote
                        {
                            continue;
                        }
                        if Self::local_file_fingerprint(std::path::Path::new(path)).await
                            == Some(prior.fingerprint)
                        {
                            carried.push((path.clone(), expected_remote, prior.fingerprint));
                        }
                    }
                    if !carried.is_empty() {
                        let carried_set: HashSet<&str> =
                            carried.iter().map(|(path, _, _)| path.as_str()).collect();
                        pending.retain(|path| !carried_set.contains(path.as_str()));
                        let previously_completed = carried.len();
                        for (path, expected_remote, fingerprint) in carried {
                            completed.insert(path.clone(), expected_remote);
                            completed_accounts.insert(path.clone(), uid);
                            completed_fingerprints.insert(path, fingerprint);
                        }
                        info!(
                            previously_completed,
                            pending = pending.len(),
                            account_uid = uid,
                            "BaiduPCS-Go retry carrying forward previously confirmed uploads"
                        );
                        ctx.info(format!(
                            "Carrying forward {previously_completed} file(s) already uploaded in an earlier attempt; {} remain pending",
                            pending.len()
                        ));
                        ctx.progress.report(
                            UploadCounts::of_job(
                                &job_inputs,
                                &file_sizes,
                                &completed,
                                &skipped,
                                &permanent,
                            )
                            .snapshot(),
                        );
                    }
                    if pending.is_empty() {
                        break;
                    }
                }
            }

            // If an in-run backoff allowed a concurrent login to switch the
            // active account (or either `who` probe failed), OR if a retained
            // local file changed on disk since the earlier attempt, do not
            // retain completed or skipped outcomes — re-upload the current
            // file instead.
            if attempt > 0 {
                let mut invalidated: HashSet<String> = HashSet::new();
                let completed_paths: Vec<String> = completed
                    .keys()
                    .filter(|path| !resumed.contains(*path))
                    .cloned()
                    .collect();
                for path in completed_paths {
                    let same_account = matches!(
                        (completed_accounts.get(&path).copied(), active_uid),
                        (Some(prev_uid), Some(curr_uid)) if prev_uid == curr_uid
                    );
                    let same_file = match completed_fingerprints.get(&path).copied() {
                        Some(expected_fp) => {
                            Self::local_file_fingerprint(Path::new(&path)).await
                                == Some(expected_fp)
                        }
                        None => false,
                    };
                    if !same_account || !same_file {
                        completed.remove(&path);
                        invalidated.insert(path);
                    }
                }
                let skipped_paths: Vec<String> = skipped.keys().cloned().collect();
                for path in skipped_paths {
                    let same_account = matches!(
                        (skipped_accounts.get(&path).copied(), active_uid),
                        (Some(prev_uid), Some(curr_uid)) if prev_uid == curr_uid
                    );
                    let same_file = match skipped_fingerprints.get(&path).copied() {
                        Some(expected_fp) => {
                            Self::local_file_fingerprint(Path::new(&path)).await
                                == Some(expected_fp)
                        }
                        None => false,
                    };
                    if !same_account || !same_file {
                        skipped.remove(&path);
                        invalidated.insert(path);
                    }
                }
                if !invalidated.is_empty() {
                    for path in &invalidated {
                        completed_accounts.remove(path);
                        completed_fingerprints.remove(path);
                        skipped_accounts.remove(path);
                        skipped_fingerprints.remove(path);
                    }
                    let mut requeued: Vec<String> = input
                        .inputs
                        .iter()
                        .filter(|path| invalidated.contains(*path))
                        .cloned()
                        .collect();
                    requeued.extend(pending);
                    pending = requeued;
                }
            }

            // One invocation per chunk of paths that fits the command line;
            // the lease above spans every chunk of this attempt.
            let chunks = chunk_by_argument_budget(&pending, self.upload_args_budget);
            if chunks.len() > 1 {
                ctx.info(format!(
                    "Uploading {} file(s) in {} BaiduPCS-Go invocations to stay within the command-line length limit",
                    pending.len(),
                    chunks.len()
                ));
            }
            let mut still_pending = Vec::new();
            for chunk in chunks {
                if ctx.cancellation_token.is_cancelled() {
                    return Err(crate::Error::Other(
                        "BaiduPCS-Go upload cancelled".to_string(),
                    ));
                }
                let mut chunk_fingerprints: HashMap<String, LocalFileFingerprint> =
                    HashMap::with_capacity(chunk.len());
                for path in &chunk {
                    if let Some(fp) = Self::local_file_fingerprint(std::path::Path::new(path)).await
                    {
                        chunk_fingerprints.insert(path.clone(), fp);
                    }
                }
                let mut cmd = crate::baidupcs::connected_command(
                    &binary_path,
                    config.config_dir.as_deref(),
                    &proxy,
                )
                .await?;
                cmd.arg("upload");
                cmd.args(["--policy", config.policy.as_arg()]);
                if config.norapid {
                    cmd.arg("--norapid");
                }
                for arg in &config.args {
                    cmd.arg(arg);
                }
                for path in &chunk {
                    cmd.arg(path);
                }
                cmd.arg(&remote_dir);

                let tracker = Arc::new(UploadTracker::new(
                    ctx.progress.clone(),
                    UploadCounts::of_job(
                        &job_inputs,
                        &file_sizes,
                        &completed,
                        &skipped,
                        &permanent,
                    ),
                    &chunk,
                    &file_sizes,
                ));
                let run = self
                    .command_runner
                    .run(cmd, ctx, Some(tracker.clone()), &config_lease);
                let command_output = match tracker.report_while(run).await {
                    Ok(output) => output,
                    Err(e) => {
                        let message = format!("Failed to execute BaiduPCS-Go: {e}");
                        for path in chunk {
                            last_errors.insert(path.clone(), message.clone());
                            still_pending.push(path);
                        }
                        continue;
                    }
                };

                let exit_ok = command_output.status.success();
                let exit_code = command_output.status.code().unwrap_or(-1);
                let outcomes = parse_upload_outcomes(&command_output.logs, &chunk);
                logs.extend(command_output.logs);

                let mut unresolved = Vec::new();
                for path in chunk {
                    if let Some(error) = outcomes.permanent.get(&path) {
                        // Never retried: another attempt (and the re-login
                        // before it) cannot change the file or the account.
                        permanent.insert(path, error.clone());
                    } else if let Some(error) = outcomes.failed.get(&path) {
                        last_errors.insert(path.clone(), error.clone());
                        still_pending.push(path);
                    } else if let Some(reason) = outcomes.skipped.get(&path) {
                        if let Some(fp) = chunk_fingerprints.get(&path).copied()
                            && Self::local_file_fingerprint(Path::new(&path)).await == Some(fp)
                        {
                            skipped.insert(path.clone(), reason.clone());
                            if let Some(uid) = active_uid {
                                skipped_accounts.insert(path.clone(), uid);
                            }
                            skipped_fingerprints.insert(path, fp);
                        } else {
                            let message = "local file changed during upload".to_string();
                            ctx.warn(format!(
                                "Local file {path} changed during upload; re-queuing instead of treating it as skipped"
                            ));
                            last_errors.insert(path.clone(), message);
                            still_pending.push(path);
                        }
                    } else if let Some(remote) = outcomes.completed.get(&path) {
                        if let Some(fp) = chunk_fingerprints.get(&path).copied()
                            && Self::local_file_fingerprint(Path::new(&path)).await == Some(fp)
                        {
                            let remote = remote
                                .as_deref()
                                .map(Self::normalize_remote_path)
                                .unwrap_or_else(|| Self::computed_remote_path(&remote_dir, &path));
                            completed.insert(path.clone(), remote);
                            if let Some(uid) = active_uid {
                                completed_accounts.insert(path.clone(), uid);
                            }
                            completed_fingerprints.insert(path, fp);
                        } else {
                            // Never treat a file modified mid-upload as completed:
                            // BaiduPCS-Go may have uploaded stale or torn bytes, and
                            // later retries/cleanup must not skip or delete the new file.
                            let message = "local file changed during upload".to_string();
                            ctx.warn(format!(
                                "Local file {path} changed during upload; re-queuing instead of treating it as completed"
                            ));
                            last_errors.insert(path.clone(), message);
                            still_pending.push(path);
                        }
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
                            if let Some(fp) = chunk_fingerprints.get(&path).copied()
                                && Self::local_file_fingerprint(Path::new(&path)).await == Some(fp)
                            {
                                let remote = Self::computed_remote_path(&remote_dir, &path);
                                completed.insert(path.clone(), remote);
                                if let Some(uid) = active_uid {
                                    completed_accounts.insert(path.clone(), uid);
                                }
                                completed_fingerprints.insert(path, fp);
                            } else {
                                let message = "local file changed during upload".to_string();
                                ctx.warn(format!(
                                    "Local file {path} changed during upload; re-queuing instead of treating it as completed"
                                ));
                                last_errors.insert(path.clone(), message);
                                still_pending.push(path);
                            }
                        }
                    } else {
                        let message = format!(
                            "no per-file outcome reported by BaiduPCS-Go (exit code {exit_code})"
                        );
                        for path in unresolved {
                            last_errors.insert(path.clone(), message.clone());
                            still_pending.push(path);
                        }
                    }
                }
            }

            pending = still_pending;
        }

        // Permanent failures were held out of the retries; report them with
        // the files that ran out of attempts, in input order.
        if !permanent.is_empty() {
            last_errors.extend(
                permanent
                    .iter()
                    .map(|(path, error)| (path.clone(), error.clone())),
            );
            pending = input
                .inputs
                .iter()
                .filter(|path| pending.contains(*path) || permanent.contains_key(*path))
                .cloned()
                .collect();
        }

        if !pending.is_empty() {
            let all_permanent =
                !permanent.is_empty() && pending.iter().all(|path| permanent.contains_key(path));
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
                completed = completed.len(),
                attempts = attempts_used,
                "BaiduPCS-Go upload failed"
            );
            if completed.is_empty() && skipped.is_empty() {
                let message = format!(
                    "BaiduPCS-Go upload failed for {} file(s) after {} attempt(s): {}",
                    pending.len(),
                    attempts_used,
                    detail
                );
                return Err(if all_permanent {
                    crate::Error::NonRetryablePipeline(message)
                } else {
                    crate::Error::Other(message)
                });
            }
            // Some files did upload: hand the per-file results to the worker
            // pool, which fails the job from `failed_inputs` and records every
            // file's outcome. Sources are only removed after a full success.
            let uploads = Self::upload_results(
                input,
                &completed,
                &skipped,
                &last_errors,
                &file_sizes,
                &remote_dir,
            );
            let confirmed_uploads: HashMap<String, ConfirmedUpload> = completed
                .iter()
                .filter_map(|(path, remote_path)| {
                    let account_uid = *completed_accounts.get(path)?;
                    let fingerprint = *completed_fingerprints.get(path)?;
                    Some((
                        path.clone(),
                        ConfirmedUpload {
                            remote_path: remote_path.clone(),
                            account_uid,
                            fingerprint,
                        },
                    ))
                })
                .collect();
            return Ok(ProcessorOutput {
                outputs: input
                    .inputs
                    .iter()
                    .filter(|path| !resumed.contains(*path))
                    .cloned()
                    .collect(),
                duration_secs: start.elapsed().as_secs_f64(),
                metadata: Some(
                    serde_json::json!({
                        "batch_size": input.inputs.len(),
                        "remote_dir": remote_dir,
                        "policy": config.policy.as_arg(),
                        "attempts": attempts_used,
                        "resumed_inputs": resumed.len(),
                        "removed_sources": 0,
                        "skipped": skipped.len(),
                        "failed": pending.len(),
                    })
                    .to_string(),
                ),
                items_produced: vec![],
                input_size_bytes: Some(total_input_size),
                output_size_bytes: None,
                failed_inputs: pending
                    .iter()
                    .map(|path| {
                        (
                            path.clone(),
                            last_errors
                                .get(path)
                                .cloned()
                                .unwrap_or_else(|| "unknown error".to_string()),
                        )
                    })
                    .collect(),
                succeeded_inputs: input
                    .inputs
                    .iter()
                    .filter(|path| completed.contains_key(*path))
                    .cloned()
                    .collect(),
                skipped_inputs: input
                    .inputs
                    .iter()
                    .filter_map(|path| {
                        skipped
                            .get(path)
                            .map(|reason| (path.clone(), reason.clone()))
                    })
                    .collect(),
                uploads,
                confirmed_uploads,
                logs,
                non_retryable: all_permanent,
            });
        }

        let mut removed: HashSet<String> = resumed.iter().cloned().collect();
        if config.remove_source_after_upload {
            for path in &input.inputs {
                if !completed.contains_key(path) || removed.contains(path) {
                    continue;
                }
                let Some(expected_fp) = completed_fingerprints.get(path).copied() else {
                    ctx.warn(format!(
                        "Skipping source removal for {path}: local file version could not be verified"
                    ));
                    continue;
                };
                if Self::verify_and_remove_source(path, expected_fp, ctx).await {
                    removed.insert(path.clone());
                }
            }
        }

        let uploads = Self::upload_results(
            input,
            &completed,
            &skipped,
            &last_errors,
            &file_sizes,
            &remote_dir,
        );

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
            confirmed_uploads: HashMap::new(),
            logs,
            non_retryable: false,
        })
        }
        .await;
        ProcessorOutput::scrubbed(result, &scrubber)
    }
}

/// Files up to this size are fully SHA-256 hashed for version verification;
/// larger recordings hash head, middle, and tail 64 KiB slices alongside
/// OS file identity and change-time metadata to avoid re-reading gigabytes.
const FULL_FINGERPRINT_HASH_MAX_BYTES: u64 = 4 * 1024 * 1024;
const SAMPLE_FINGERPRINT_SLICE_BYTES: usize = 64 * 1024;

enum VerifiedRemoval {
    Removed,
    AlreadyAbsent,
    Changed,
    Failed(std::io::Error),
}

fn system_time_to_unix_parts(time: std::time::SystemTime) -> Option<(i64, u32)> {
    match time.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => Some((
            i64::try_from(duration.as_secs()).ok()?,
            duration.subsec_nanos(),
        )),
        Err(before_epoch) => {
            let duration = before_epoch.duration();
            let secs = i64::try_from(duration.as_secs()).ok()?;
            let nanos = duration.subsec_nanos();
            if nanos == 0 {
                Some((-secs, 0))
            } else {
                Some((-secs - 1, 1_000_000_000 - nanos))
            }
        }
    }
}

struct PlatformFileIdentity {
    file_id: Option<(u64, u64)>,
    change_time: Option<(i64, u32)>,
}

#[cfg(unix)]
fn platform_file_identity(file: &std::fs::File, meta: &std::fs::Metadata) -> PlatformFileIdentity {
    use std::os::unix::fs::MetadataExt;
    let _ = file;
    let file_id = Some((meta.dev(), meta.ino()));
    let change_time = u32::try_from(meta.ctime_nsec())
        .ok()
        .map(|ns| (meta.ctime(), ns));
    PlatformFileIdentity {
        file_id,
        change_time,
    }
}

#[cfg(windows)]
fn platform_file_identity(file: &std::fs::File, _meta: &std::fs::Metadata) -> PlatformFileIdentity {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    let handle = file.as_raw_handle();
    // SAFETY: `handle` is an open file handle owned by `file` for the duration
    // of this call, and `info` is a properly aligned output buffer.
    let file_id = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        if GetFileInformationByHandle(handle, &mut info) != 0 {
            let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
            Some((u64::from(info.dwVolumeSerialNumber), index))
        } else {
            None
        }
    };
    // SAFETY: `handle` is an open file handle owned by `file` for the duration
    // of this call, and `basic` is a properly aligned `FILE_BASIC_INFO` buffer
    // matching `FileBasicInfo` and the passed byte size.
    let change_time = unsafe {
        let mut basic: FILE_BASIC_INFO = zeroed();
        if GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        ) != 0
            && basic.ChangeTime > 0
        {
            let secs = basic.ChangeTime.div_euclid(10_000_000);
            let subsec_nanos = (basic.ChangeTime.rem_euclid(10_000_000) as u32) * 100;
            Some((secs, subsec_nanos))
        } else {
            None
        }
    };
    PlatformFileIdentity {
        file_id,
        change_time,
    }
}

#[cfg(not(any(unix, windows)))]
fn platform_file_identity(
    _file: &std::fs::File,
    _meta: &std::fs::Metadata,
) -> PlatformFileIdentity {
    PlatformFileIdentity {
        file_id: None,
        change_time: None,
    }
}

fn hash_file_slice(
    file: &mut std::fs::File,
    hasher: &mut Sha256,
    offset: u64,
    mut remaining: usize,
    buf: &mut [u8],
) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    while remaining > 0 {
        let to_read = remaining.min(buf.len());
        let n = file.read(&mut buf[..to_read])?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "file shrank while computing fingerprint",
            ));
        }
        hasher.update(&buf[..n]);
        remaining -= n;
    }
    Ok(())
}

fn fingerprint_from_file(file: &mut std::fs::File) -> Option<LocalFileFingerprint> {
    let meta = file.metadata().ok()?;
    let size_bytes = meta.len();
    let (modified_secs, modified_subsec_nanos) = system_time_to_unix_parts(meta.modified().ok()?)?;
    let PlatformFileIdentity {
        file_id,
        change_time,
    } = platform_file_identity(file, &meta);

    let mut hasher = Sha256::new();
    hasher.update(size_bytes.to_le_bytes());
    let mut buf = [0u8; 64 * 1024];
    // Hash the entire file when small or when the OS/filesystem does not
    // provide a kernel-maintained change time (`ctime` / NTFS `ChangeTime`).
    if size_bytes <= FULL_FINGERPRINT_HASH_MAX_BYTES || change_time.is_none() {
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut total = 0u64;
        loop {
            let n = file.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            total += n as u64;
            hasher.update(&buf[..n]);
        }
        if total != size_bytes {
            return None;
        }
    } else {
        let slice_len = SAMPLE_FINGERPRINT_SLICE_BYTES;
        let mid_offset = (size_bytes / 2).saturating_sub((slice_len / 2) as u64);
        let tail_offset = size_bytes.saturating_sub(slice_len as u64);
        hash_file_slice(file, &mut hasher, 0, slice_len, &mut buf).ok()?;
        hash_file_slice(file, &mut hasher, mid_offset, slice_len, &mut buf).ok()?;
        hash_file_slice(file, &mut hasher, tail_offset, slice_len, &mut buf).ok()?;
    }

    // Re-check metadata on the same handle after reading to fail closed if
    // the file was written while we were hashing it.
    let after = file.metadata().ok()?;
    let (after_mod_secs, after_mod_nanos) = system_time_to_unix_parts(after.modified().ok()?)?;
    let after_identity = platform_file_identity(file, &after);
    if after.len() != size_bytes
        || after_mod_secs != modified_secs
        || after_mod_nanos != modified_subsec_nanos
        || after_identity.file_id != file_id
        || after_identity.change_time != change_time
    {
        return None;
    }

    let content_sha256: [u8; 32] = hasher.finalize().into();
    Some(LocalFileFingerprint {
        size_bytes,
        modified_secs,
        modified_subsec_nanos,
        file_id,
        change_time,
        content_sha256,
    })
}

#[cfg(windows)]
fn verify_and_remove_sync(path: &Path, expected: LocalFileFingerprint) -> VerifiedRemoval {
    use std::mem::{size_of, zeroed};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FileDispositionInfo, GetFileInformationByHandle, SetFileInformationByHandle,
    };

    // Open the final component without following reparse points (so a symlink's
    // target can never be opened or deleted) and without FILE_SHARE_WRITE (so
    // no concurrent writer can modify the file between verification and
    // handle-bound deletion).
    let mut file = match std::fs::OpenOptions::new()
        .read(true)
        .access_mode(FILE_GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return VerifiedRemoval::AlreadyAbsent;
        }
        Err(error) => return VerifiedRemoval::Failed(error),
    };
    // SAFETY: `file` owns the open handle for the duration of this call and
    // `info` is a properly aligned output buffer.
    let is_reparse_point = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        GetFileInformationByHandle(file.as_raw_handle(), &mut info) == 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    };
    if is_reparse_point || fingerprint_from_file(&mut file) != Some(expected) {
        return VerifiedRemoval::Changed;
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `file` is a valid open handle with DELETE access and `disposition`
    // is a properly aligned `FILE_DISPOSITION_INFO` valid for the call.
    let ok = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok != 0 {
        // Dropping `file` closes the last handle and completes deletion.
        drop(file);
        VerifiedRemoval::Removed
    } else {
        VerifiedRemoval::Failed(std::io::Error::last_os_error())
    }
}

#[cfg(not(windows))]
fn verify_and_remove_sync(path: &Path, expected: LocalFileFingerprint) -> VerifiedRemoval {
    // Do not follow or delete symlink inputs during source cleanup.
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => return VerifiedRemoval::Changed,
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return VerifiedRemoval::AlreadyAbsent;
        }
        Err(error) => return VerifiedRemoval::Failed(error),
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return VerifiedRemoval::AlreadyAbsent;
        }
        Err(error) => return VerifiedRemoval::Failed(error),
    };
    if fingerprint_from_file(&mut file) != Some(expected) {
        return VerifiedRemoval::Changed;
    }
    #[cfg(unix)]
    if let Some((expected_dev, expected_ino)) = expected.file_id {
        use std::os::unix::fs::MetadataExt;
        match std::fs::symlink_metadata(path) {
            Ok(meta)
                if !meta.file_type().is_symlink()
                    && meta.dev() == expected_dev
                    && meta.ino() == expected_ino => {}
            Ok(_) => return VerifiedRemoval::Changed,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return VerifiedRemoval::AlreadyAbsent;
            }
            Err(error) => return VerifiedRemoval::Failed(error),
        }
    }
    drop(file);
    match std::fs::remove_file(path) {
        Ok(()) => VerifiedRemoval::Removed,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            VerifiedRemoval::AlreadyAbsent
        }
        Err(error) => VerifiedRemoval::Failed(error),
    }
}

#[cfg(test)]
mod tests;
