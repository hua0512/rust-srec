//! Rclone processor for cloud storage operations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tempfile::TempPath;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{error, info, warn};

use super::traits::{
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, TimeAnchor,
    UploadItemStatus, UploadResultItem,
};
use super::utils::CommandOutput;
use crate::Result;
use crate::pipeline::progress::ProgressKind;
use crate::proxies::{ProxyRoute, ProxyTarget};

/// Rclone operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RcloneOperation {
    /// Copy files (default).
    #[default]
    Copy,
    /// Move files (deletes source).
    Move,
    /// Sync files (make destination identical to source).
    Sync,
}

/// Configuration for the rclone processor.
///
/// Deserialized from the JSON string in `ProcessorInput::config`. Every
/// field is optional and defaults are applied when keys are missing, so
/// older saved configs that pre-date newer fields continue to load.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RcloneConfig {
    /// Base remote path (e.g. `gdrive:/videos`). Supports placeholder
    /// expansion for `{streamer}`, `{title}`, `{streamer_id}`,
    /// `{session_id}`, and strftime-style time tokens (`%Y`, `%m`, `%d`, ...).
    pub destination_root: Option<String>,

    /// Path to a custom `rclone.conf`. If unset, rclone uses its default.
    pub config_path: Option<String>,

    /// Transfer operation. Defaults to [`RcloneOperation::Copy`].
    pub operation: RcloneOperation,

    /// Timestamp source for time placeholder expansion.
    pub time_anchor: TimeAnchor,

    /// Attempts per transfer and per destination `mkdir`, at least one.
    /// Defaults to [`DEFAULT_ATTEMPTS`].
    pub max_retries: Option<u32>,

    /// Free-form extra CLI arguments appended verbatim after the
    /// throughput flags. Provided as a power-user escape hatch; prefer
    /// the dedicated fields below when possible.
    pub args: Vec<String>,

    // -------- Throughput / bandwidth controls --------
    /// `--bwlimit` value, e.g. `"10M"`, `"10M:100k"`, or a timetable like
    /// `"08:00,512k 23:00,off"`. Units are bytes (default base KiB/s).
    /// See <https://rclone.org/docs/#bwlimit-bandwidth-spec>.
    pub bwlimit: Option<String>,

    /// `--bwlimit-file` per-file bandwidth cap. Same syntax as `bwlimit`.
    pub bwlimit_file: Option<String>,

    /// `--transfers`: number of concurrent file transfers.
    pub transfers: Option<u32>,

    /// `--checkers`: number of concurrent checkers.
    pub checkers: Option<u32>,

    /// `--tpslimit`: max transactions per second against the remote.
    /// `0` means unlimited; `None` falls back to rclone's default.
    pub tpslimit: Option<f64>,

    /// `--tpslimit-burst`: burst capacity for `tpslimit`.
    pub tpslimit_burst: Option<u32>,

    /// `--multi-thread-streams`: streams per file for multi-thread copy.
    pub multi_thread_streams: Option<u32>,

    /// `--multi-thread-cutoff`: size threshold (e.g. `"250M"`) above
    /// which multi-thread copy kicks in.
    pub multi_thread_cutoff: Option<String>,

    /// How rclone connects; `inherit`, the default, follows the global
    /// route.
    pub proxy_route: ProxyRoute,
}

impl RcloneConfig {
    /// Build the list of CLI arguments contributed by the throughput
    /// fields, as flag-then-value pairs. Empty when no throughput field
    /// is set.
    ///
    /// Returned as `Vec<String>` so unit tests can assert on the exact
    /// argv without constructing a [`Command`].
    fn throughput_args(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();

        if let Some(v) = self.bwlimit.as_deref().filter(|s| !s.is_empty()) {
            out.push("--bwlimit".into());
            out.push(v.into());
        }
        if let Some(v) = self.bwlimit_file.as_deref().filter(|s| !s.is_empty()) {
            out.push("--bwlimit-file".into());
            out.push(v.into());
        }
        if let Some(n) = self.transfers {
            out.push("--transfers".into());
            out.push(n.to_string());
        }
        if let Some(n) = self.checkers {
            out.push("--checkers".into());
            out.push(n.to_string());
        }
        if let Some(n) = self.tpslimit {
            out.push("--tpslimit".into());
            out.push(n.to_string());
        }
        if let Some(n) = self.tpslimit_burst {
            out.push("--tpslimit-burst".into());
            out.push(n.to_string());
        }
        if let Some(n) = self.multi_thread_streams {
            out.push("--multi-thread-streams".into());
            out.push(n.to_string());
        }
        if let Some(v) = self
            .multi_thread_cutoff
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            out.push("--multi-thread-cutoff".into());
            out.push(v.into());
        }

        out
    }
}

/// Drops `-P`/`--progress` from user-supplied extra args, returning the kept
/// args and how many tokens were removed. `--progress` replaces rclone's
/// line-delimited stderr logging with a control-code terminal display that
/// `run_rclone_with_progress` cannot parse (and whose newline-free redraws
/// its line reader would have to skip as overlong). Only these exact tokens
/// are stripped; every other arg keeps the last-wins override contract.
fn sanitize_extra_args(args: &[String]) -> (Vec<String>, usize) {
    let kept: Vec<String> = args
        .iter()
        .filter(|arg| *arg != "-P" && *arg != "--progress")
        .cloned()
        .collect();
    let stripped = args.len() - kept.len();
    (kept, stripped)
}

/// How long a successful `mkdir` is trusted, so jobs uploading into the same
/// directory moments apart skip a redundant one. Kept short because a
/// directory deleted on the remote within this window would be recreated by
/// racing transfers instead of under the lock.
const ENSURED_DIR_TTL: Duration = Duration::from_secs(60);

/// Upper bound on waiting for other jobs' `mkdir`s on the same remote. Leaves
/// room for a few jobs ahead to exhaust every [`MKDIR_TIMEOUT`] attempt
/// (about 3 minutes each with backoff) without failing the jobs queued behind.
const DIR_LOCK_WAIT: Duration = Duration::from_secs(600);

/// Upper bound on one `rclone mkdir` attempt, which retries throttled
/// requests itself.
const MKDIR_TIMEOUT: Duration = Duration::from_secs(60);

/// A directory a `mkdir` created. The config file and backend arguments
/// decide which directory a path names, so they are part of its identity.
#[derive(PartialEq, Eq, Hash)]
struct EnsuredDir {
    config_path: Option<String>,
    extra_args: Vec<String>,
    path: String,
}

/// Remote directories created within [`ENSURED_DIR_TTL`].
type EnsuredDirs = HashMap<EnsuredDir, tokio::time::Instant>;

/// A directory on an rclone remote that a transfer writes into.
struct RemoteDir {
    /// Remote name without connection-string parameters; selects the lock.
    remote: String,
    /// Full rclone path, parameters included; what `mkdir` creates.
    path: String,
}

/// One lock per rclone remote that serializes its directory creation; the
/// lock guards the directories it has recently ensured.
#[derive(Default)]
struct RemoteDirGate {
    remotes: DashMap<String, Arc<tokio::sync::Mutex<EnsuredDirs>>>,
}

impl RemoteDirGate {
    fn remote(&self, name: &str) -> Arc<tokio::sync::Mutex<EnsuredDirs>> {
        self.remotes.entry(name.to_owned()).or_default().clone()
    }
}

const DEFAULT_ATTEMPTS: u32 = 3;

/// Transfer flags for `run_rclone_with_progress`, which parses each stderr
/// line as a `--use-json-log` object and turns entries carrying `stats` into
/// progress snapshots. `--stats-log-level NOTICE` must not be filtered by the
/// main `--log-level`, or the periodic stats never reach stderr.
/// `--stats-one-line` only shrinks the human `msg` embedded in each stats object.
const JSON_STATS_ARGS: [&str; 8] = [
    "--use-json-log",
    "--log-level",
    "NOTICE",
    "--stats-log-level",
    "NOTICE",
    "--stats",
    "1s",
    "--stats-one-line",
];

/// Limits each transfer process to one pass. The processor's own loop, bounded
/// by `max_retries`, retries failed transfers with backoff and reports each
/// attempt; rclone's default of 3 internal passes would multiply that count
/// unseen. Request-level retries (`--low-level-retries`) stay at rclone's
/// default. Placed before `extra_args`, so a preset can still set `--retries`.
const SINGLE_PASS_ARGS: [&str; 2] = ["--retries", "1"];

/// Processor for interacting with Rclone.
pub struct RcloneProcessor {
    /// Path to rclone binary.
    rclone_path: String,
    /// Runs assembled rclone commands; a seam so tests can observe and
    /// stub invocations without spawning a real process.
    command_runner: Arc<dyn RcloneCommandRunner>,
    /// Serializes destination directory creation across concurrent jobs.
    dir_gate: RemoteDirGate,
    /// Resolves each step's proxy route; see [`crate::proxies::upload_target`].
    routes: Option<crate::proxies::ServiceRoutes>,
}

#[async_trait]
trait RcloneCommandRunner: Send + Sync {
    async fn run(&self, command: &mut Command, context: &ProcessorContext)
    -> Result<CommandOutput>;
}

struct ProcessRcloneCommandRunner;

#[async_trait]
impl RcloneCommandRunner for ProcessRcloneCommandRunner {
    async fn run(
        &self,
        command: &mut Command,
        context: &ProcessorContext,
    ) -> Result<CommandOutput> {
        super::upload_runner::run_rclone_with_progress(
            command,
            &context.progress,
            Some(context.log_sink.clone()),
        )
        .await
    }
}

struct RcloneExecution<'a> {
    remote_destination: &'a str,
    proxy: &'a ProxyTarget,
    operation: RcloneOperation,
    config_path: Option<&'a str>,
    throughput: &'a [String],
    extra_args: &'a [String],
    attempts: u32,
    context: &'a ProcessorContext,
}

impl RcloneProcessor {
    /// Create a new rclone processor.
    pub fn new() -> Self {
        Self::with_command_runner(
            crate::utils::tools::resolve(None, "RCLONE_PATH", "rclone"),
            Arc::new(ProcessRcloneCommandRunner),
        )
    }

    fn with_command_runner(
        path: impl Into<String>,
        command_runner: Arc<dyn RcloneCommandRunner>,
    ) -> Self {
        Self {
            rclone_path: path.into(),
            command_runner,
            dir_gate: RemoteDirGate::default(),
            routes: None,
        }
    }

    /// Resolves each step's proxy route through `routes`.
    pub fn with_routes(mut self, routes: crate::proxies::ServiceRoutes) -> Self {
        self.routes = Some(routes);
        self
    }

    /// An rclone command on the execution's connection and config file.
    ///
    /// rclone prefers `--http-proxy` and `RCLONE_HTTP_PROXY` over the
    /// environment proxy, so a route other than System proxy removes the
    /// variable and refuses the flag in the step's arguments. Proxy settings
    /// of a remote in the rclone config still apply to that remote.
    fn command(&self, execution: &RcloneExecution<'_>) -> Result<Command> {
        let mut cmd = Command::new(&self.rclone_path);
        if *execution.proxy != ProxyTarget::System {
            if execution
                .extra_args
                .iter()
                .any(|arg| arg == "--http-proxy" || arg.starts_with("--http-proxy="))
            {
                return Err(crate::Error::Validation(
                    "rclone's --http-proxy argument overrides the step's proxy route; remove it, or choose System proxy for this step".to_owned(),
                ));
            }
            cmd.env_remove("RCLONE_HTTP_PROXY");
        }
        crate::proxies::set_tool_environment(&mut cmd, execution.proxy, "rclone")?;
        if let Some(cfg) = execution.config_path {
            cmd.arg("--config").arg(cfg);
        }
        Ok(cmd)
    }

    /// Find the common base directory for a list of paths.
    /// Returns the deepest common ancestor directory.
    fn find_common_base_dir(paths: &[String]) -> Option<PathBuf> {
        if paths.is_empty() {
            return None;
        }

        let first_path = Path::new(&paths[0]);
        let first_parent = first_path.parent()?;

        // Start with the first path's parent as candidate
        let mut common = first_parent.to_path_buf();

        for path in paths.iter().skip(1) {
            let p = Path::new(path);
            let parent = p.parent()?;

            // Find common prefix between current common and this path's parent
            let common_components: Vec<_> = common.components().collect();
            let path_components: Vec<_> = parent.components().collect();

            let mut new_common = PathBuf::new();
            for (a, b) in common_components.iter().zip(path_components.iter()) {
                if a == b {
                    new_common.push(a.as_os_str());
                } else {
                    break;
                }
            }

            if new_common.as_os_str().is_empty() {
                return None; // No common base
            }
            common = new_common;
        }

        Some(common)
    }

    /// Create the `--files-from` manifest listing `inputs` relative to `base_dir`.
    ///
    /// Returned as a [`TempPath`] so the manifest is removed when the guard
    /// drops, including when a job timeout or cancellation drops the owning
    /// future before the explicit `close()` in `process_batch`. It lives in
    /// the system temporary directory: `base_dir` may be a read-only mount or
    /// the filesystem root when the inputs share no deeper ancestor.
    async fn create_files_from_list(
        inputs: &[String],
        base_dir: &Path,
    ) -> std::io::Result<TempPath> {
        let named_file = tempfile::Builder::new()
            .prefix(".rclone_files_")
            .suffix(".txt")
            .tempfile()?;
        let (file, temp_path) = named_file.into_parts();
        let mut file = tokio::fs::File::from_std(file);

        for input in inputs {
            let input_path = Path::new(input);
            let relative = input_path
                .strip_prefix(base_dir)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let relative_str = relative.to_string_lossy();
            file.write_all(relative_str.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        file.flush().await?;
        Ok(temp_path)
    }

    /// Drain inputs whose sources rclone consumed during the last attempt.
    async fn take_moved_inputs(pending_inputs: &mut Vec<String>) -> Vec<String> {
        let mut moved_inputs = Vec::new();
        let mut still_pending = Vec::with_capacity(pending_inputs.len());
        for input in pending_inputs.drain(..) {
            if super::inputs::is_confirmed_absent(Path::new(&input)).await {
                moved_inputs.push(input);
            } else {
                still_pending.push(input);
            }
        }
        *pending_inputs = still_pending;
        moved_inputs
    }

    /// A destination as it may appear in logs. An rclone connection string
    /// (`remote,param=value,...:path` or `:backend,param=value:path`) carries
    /// credentials in its parameters, which are elided; a plain `remote:path`
    /// or local path is unchanged.
    fn loggable_destination(destination: &str) -> String {
        match Self::split_remote(destination)
            .and_then(|(spec, path)| Some((spec.split_once(',')?.0, path)))
        {
            Some((name, path)) => format!("{name},…:{path}"),
            None => destination.to_owned(),
        }
    }

    /// Splits an rclone destination into its remote spec — the remote name
    /// plus any connection-string parameters (`drive,token=…`, `:backend,…`)
    /// — and the path on that remote, or `None` for a local path.
    fn split_remote(destination: &str) -> Option<(&str, &str)> {
        let backend_prefix = usize::from(destination.starts_with(':'));
        let bytes = destination.as_bytes();
        let name_end = backend_prefix + destination[backend_prefix..].find([',', ':'])?;
        // Remote names never contain a path separator, and rclone on Windows
        // reads a single letter before the colon as a drive. Parameter values
        // may contain either, so only the name is checked.
        let name = &destination[backend_prefix..name_end];
        if name.is_empty()
            || name.contains(['/', '\\'])
            || (cfg!(windows) && backend_prefix == 0 && name.len() == 1)
        {
            return None;
        }
        // A value holding `,` or `:` is quoted with `"` or `'` from its first
        // character; a doubled quote inside stands for the quote itself.
        let mut quote = None;
        let mut index = name_end;
        while index < bytes.len() {
            let byte = bytes[index];
            match quote {
                Some(open) if byte == open => {
                    if bytes.get(index + 1) == Some(&open) {
                        index += 1;
                    } else {
                        quote = None;
                    }
                }
                Some(_) => {}
                None if byte == b':' => {
                    return Some((&destination[..index], &destination[index + 1..]));
                }
                None if matches!(byte, b'"' | b'\'') && bytes[index - 1] == b'=' => {
                    quote = Some(byte);
                }
                None => {}
            }
            index += 1;
        }
        None
    }

    /// Remote path of one batch input: `remote_root` + the input's path
    /// relative to `base_dir`, matching how rclone `--files-from` lays out
    /// files under the destination. Always joined with `/` — rclone remote
    /// paths are forward-slash regardless of the local OS separator.
    fn batch_remote_path(base_dir: &Path, remote_root: &str, input: &str) -> Option<String> {
        let relative = Path::new(input).strip_prefix(base_dir).ok()?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        Some(format!(
            "{}/{}",
            remote_root.trim_end_matches('/'),
            relative
        ))
    }

    /// The rclone remote a destination is on, or `None` for a local path.
    /// Connection-string parameters are dropped, so `drive,token=…:a` and
    /// `drive:b` share a remote.
    fn remote_name(destination: &str) -> Option<&str> {
        let (spec, _path) = Self::split_remote(destination)?;
        Some(
            spec.split_once(',')
                .map_or(spec, |(name, _parameters)| name),
        )
    }

    /// The directory holding `destination` on its remote, or `None` when
    /// that is the remote's root, which always exists.
    fn remote_parent(destination: &str) -> Option<&str> {
        let (spec, path) = Self::split_remote(destination)?;
        let slash = path.trim_end_matches('/').rfind('/')?;
        (!path[..slash].trim_matches('/').is_empty())
            .then(|| &destination[..spec.len() + 1 + slash])
    }

    /// Remote directories a transfer writes into, deduplicated in input order.
    ///
    /// With `batch_base_dir`, `destination` is the root that inputs keep their
    /// paths relative to; without it, `destination` is the single input's
    /// file path. Inputs a resumed move already consumed write nothing, and
    /// local destinations need no coordination.
    async fn mkdir_targets(
        inputs: &[String],
        destination: &str,
        batch_base_dir: Option<&Path>,
        skip_absent_inputs: bool,
    ) -> Vec<RemoteDir> {
        let Some(remote) = Self::remote_name(destination) else {
            return Vec::new();
        };
        let mut targets: Vec<RemoteDir> = Vec::new();
        for input in inputs {
            if skip_absent_inputs && super::inputs::is_confirmed_absent(Path::new(input)).await {
                continue;
            }
            let file_destination = match batch_base_dir {
                Some(base_dir) => Self::batch_remote_path(base_dir, destination, input),
                None => Some(destination.to_owned()),
            };
            if let Some(parent) = file_destination.as_deref().and_then(Self::remote_parent)
                && !targets.iter().any(|target| target.path == parent)
            {
                targets.push(RemoteDir {
                    remote: remote.to_owned(),
                    path: parent.to_owned(),
                });
            }
        }
        targets
    }

    /// Create each target directory before a transfer writes into it.
    ///
    /// Each rclone process resolves a destination with its own directory
    /// cache, so concurrent transfers into a missing directory each create
    /// it, and remotes that allow same-name siblings (Google Drive) keep
    /// every copy, splitting the files between them. Running `mkdir` here,
    /// one at a time per remote, lets the transfers find the one existing
    /// directory. The lock covers a whole remote rather than a path because
    /// sibling destinations share missing ancestors; differently named
    /// remotes are not coordinated, even when they reach the same account.
    ///
    /// A `mkdir` is retried like a transfer; one that keeps failing fails the
    /// job, because transferring anyway could race another job's `mkdir` of
    /// the same directory. Retries keep the lock for the same reason.
    async fn ensure_remote_dirs(
        &self,
        targets: &[RemoteDir],
        execution: &RcloneExecution<'_>,
    ) -> Result<()> {
        for RemoteDir { remote, path } in targets {
            let loggable = Self::loggable_destination(path);
            let remote_dirs = self.dir_gate.remote(remote);
            let mut ensured = tokio::time::timeout(DIR_LOCK_WAIT, remote_dirs.lock())
                .await
                .map_err(|_| {
                    crate::Error::Other(format!(
                        "Timed out waiting to create remote directory {loggable}"
                    ))
                })?;
            ensured.retain(|_, ensured_at| ensured_at.elapsed() < ENSURED_DIR_TTL);
            let ensured_dir = EnsuredDir {
                config_path: execution.config_path.map(str::to_owned),
                extra_args: execution.extra_args.to_vec(),
                path: path.clone(),
            };
            if ensured.contains_key(&ensured_dir) {
                execution.context.info(format!(
                    "Remote directory {loggable} was created moments ago; skipping mkdir"
                ));
                continue;
            }

            let mut attempt = 1;
            while let Err(error) = self.run_mkdir(path, &loggable, execution).await {
                let failure = format!(
                    "rclone mkdir attempt {attempt} of {} failed: {error}",
                    execution.attempts
                );
                if attempt >= execution.attempts {
                    execution.context.warn(failure);
                    return Err(error);
                }
                super::utils::wait_out_retry(
                    execution.context,
                    ProgressKind::Rclone,
                    attempt,
                    &failure,
                )
                .await;
                attempt += 1;
            }
            execution
                .context
                .info(format!("Ensured remote directory {loggable}"));
            ensured.insert(ensured_dir, tokio::time::Instant::now());
        }
        Ok(())
    }

    /// One `rclone mkdir` of `target`, bounded by [`MKDIR_TIMEOUT`].
    async fn run_mkdir(
        &self,
        target: &str,
        loggable: &str,
        execution: &RcloneExecution<'_>,
    ) -> Result<()> {
        let mut cmd = self.command(execution)?;
        cmd.args(["--use-json-log", "--log-level", "NOTICE", "mkdir", target]);
        // Extra args can carry backend flags (e.g. a root folder ID) that
        // change which directory the path names, so they apply here too.
        cmd.args(execution.throughput);
        cmd.args(execution.extra_args);

        let command_output = tokio::time::timeout(
            MKDIR_TIMEOUT,
            self.command_runner.run(&mut cmd, execution.context),
        )
        .await
        .map_err(|_| {
            crate::Error::Other(format!(
                "rclone mkdir {loggable} timed out after {}s",
                MKDIR_TIMEOUT.as_secs()
            ))
        })?
        .map_err(|e| {
            crate::Error::Other(format!("Failed to execute rclone mkdir {loggable}: {e}"))
        })?;
        if !command_output.status.success() {
            return Err(crate::Error::Other(format!(
                "rclone mkdir {loggable} failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                command_output
                    .last_error_message()
                    .unwrap_or("Unknown error")
            )));
        }
        Ok(())
    }

    /// Execute a single-file rclone operation.
    async fn process_single(
        &self,
        input_path: &str,
        execution: &RcloneExecution<'_>,
    ) -> Result<ProcessorOutput> {
        let RcloneExecution {
            remote_destination,
            operation,
            throughput,
            extra_args,
            attempts,
            context,
            ..
        } = execution;
        let start = std::time::Instant::now();

        // Use 'copyto' and 'moveto' for single-file operations.
        // Unlike 'copy' and 'move', these commands are designed for file-to-file transfer
        // and won't create a directory with the destination filename.
        let cmd_op = match *operation {
            RcloneOperation::Copy => "copyto",
            RcloneOperation::Move => "moveto",
            RcloneOperation::Sync => {
                return Err(crate::Error::Validation(
                    "Sync requires batch inputs".to_owned(),
                ));
            }
        };

        info!(
            "Rclone {}: {} -> {}",
            cmd_op,
            input_path,
            Self::loggable_destination(remote_destination)
        );

        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let mut last_error = None;
        let mut logs = Vec::new();

        let success_output = |logs| ProcessorOutput {
            outputs: match *operation {
                RcloneOperation::Move => vec![],
                _ => vec![input_path.to_string()],
            },
            duration_secs: start.elapsed().as_secs_f64(),
            metadata: None,
            items_produced: vec![],
            input_size_bytes,
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            uploads: vec![UploadResultItem {
                local_path: input_path.to_string(),
                remote_path: Some(remote_destination.to_string()),
                // Captured before the transfer, so it is present even after
                // a move deleted the source (None on move-resume retries).
                size_bytes: input_size_bytes,
                status: UploadItemStatus::Completed,
                error: None,
            }],
            logs,
        };

        for attempt in 0..*attempts {
            if matches!(*operation, RcloneOperation::Move)
                && (context.is_retry || attempt > 0)
                && super::inputs::is_confirmed_absent(Path::new(input_path)).await
            {
                info!(
                    input = input_path,
                    "Rclone move source is already absent; treating it as successfully moved"
                );
                return Ok(success_output(logs));
            }

            if attempt > 0 {
                info!("Retry attempt {} for rclone {}", attempt + 1, cmd_op);
                let error = last_error.as_deref().unwrap_or("unknown error");
                super::utils::wait_to_retry(
                    context,
                    ProgressKind::Rclone,
                    attempt,
                    *attempts,
                    &format!("rclone {cmd_op} attempt {attempt} of {attempts} failed: {error}"),
                )
                .await;
            }

            let mut cmd = self.command(execution)?;
            cmd.args(JSON_STATS_ARGS);
            cmd.args(SINGLE_PASS_ARGS);
            cmd.args([cmd_op, input_path, remote_destination]);

            // Throughput flags first, so any duplicates in `extra_args` win.
            cmd.args(*throughput);
            for arg in *extra_args {
                cmd.arg(arg);
            }

            let command_output = match self.command_runner.run(&mut cmd, context).await {
                Ok(output) => output,
                Err(e) => {
                    if matches!(*operation, RcloneOperation::Move)
                        && super::inputs::is_confirmed_absent(Path::new(input_path)).await
                    {
                        warn!(
                            input = input_path,
                            attempt = attempt + 1,
                            error = %e,
                            "Rclone reported an execution error after moving the source"
                        );
                        return Ok(success_output(logs));
                    }
                    last_error = Some(format!("Failed to execute rclone: {}", e));
                    continue;
                }
            };

            if command_output.status.success() {
                let duration = start.elapsed().as_secs_f64();
                info!("Rclone {} completed in {:.2}s", cmd_op, duration);
                logs.extend(command_output.logs);
                return Ok(success_output(logs));
            } else {
                let error_msg = command_output
                    .last_error_message()
                    .map(str::to_owned)
                    .unwrap_or_else(|| "Unknown error".to_string());
                logs.extend(command_output.logs);

                if matches!(*operation, RcloneOperation::Move)
                    && super::inputs::is_confirmed_absent(Path::new(input_path)).await
                {
                    warn!(
                        input = input_path,
                        attempt = attempt + 1,
                        "Rclone exited unsuccessfully after moving the source; treating the input as successful"
                    );
                    return Ok(success_output(logs));
                }

                last_error = Some(format!(
                    "rclone failed with exit code {}: {}",
                    command_output.status.code().unwrap_or(-1),
                    error_msg
                ));
            }
        }

        error!("Rclone {} failed after {} attempts", cmd_op, attempts);
        Err(crate::Error::Other(
            last_error.unwrap_or_else(|| "Rclone failed".to_string()),
        ))
    }

    /// Execute a batch rclone operation using --files-from.
    async fn process_batch(
        &self,
        inputs: &[String],
        execution: &RcloneExecution<'_>,
    ) -> Result<ProcessorOutput> {
        let RcloneExecution {
            remote_destination,
            operation,
            throughput,
            extra_args,
            attempts,
            context,
            ..
        } = execution;
        let start = std::time::Instant::now();

        let cmd_op = match *operation {
            RcloneOperation::Copy => "copy",
            RcloneOperation::Move => "move",
            RcloneOperation::Sync => "sync",
        };

        let base_dir = Self::find_common_base_dir(inputs).ok_or_else(|| {
            crate::Error::Validation(
                "Could not determine common base directory for batch upload".to_string(),
            )
        })?;
        let base_dir_str = base_dir.to_string_lossy().to_string();
        let (mut pending_inputs, already_moved_inputs) =
            if matches!(*operation, RcloneOperation::Move) && context.is_retry {
                super::inputs::partition_absent_inputs(inputs).await
            } else {
                (inputs.to_vec(), Vec::new())
            };
        let resumed_inputs = already_moved_inputs.len();
        let mut completed_inputs = resumed_inputs;
        // Sizes are captured before rclone runs so move'd sources still have
        // one; inputs already consumed by an earlier attempt are absent and
        // fall back to None (the upsert's COALESCE keeps any earlier value).
        let file_sizes = super::inputs::input_size_map(&pending_inputs).await;
        let total_input_size = file_sizes.values().copied().sum::<u64>();

        let success_output = |logs, attempts| ProcessorOutput {
            outputs: match *operation {
                RcloneOperation::Move => vec![],
                _ => inputs.to_vec(),
            },
            duration_secs: start.elapsed().as_secs_f64(),
            metadata: Some(
                serde_json::json!({
                    "batch_size": inputs.len(),
                    "base_dir": base_dir_str,
                    "operation": cmd_op,
                    "attempts": attempts,
                    "resumed_inputs": resumed_inputs,
                })
                .to_string(),
            ),
            items_produced: vec![],
            input_size_bytes: Some(total_input_size),
            output_size_bytes: None,
            failed_inputs: vec![],
            succeeded_inputs: inputs.to_vec(),
            skipped_inputs: vec![],
            // Every input (including ones resumed from an earlier attempt)
            // is Completed here — batch success is all-or-nothing.
            uploads: inputs
                .iter()
                .map(|input| UploadResultItem {
                    local_path: input.clone(),
                    remote_path: Self::batch_remote_path(&base_dir, remote_destination, input),
                    size_bytes: file_sizes.get(input).copied(),
                    status: UploadItemStatus::Completed,
                    error: None,
                })
                .collect(),
            logs,
        };

        if resumed_inputs > 0 {
            warn!(
                resumed_inputs,
                pending_inputs = pending_inputs.len(),
                "Rclone move batch contains missing sources; treating them as previously moved"
            );
            context.info(format!(
                "Resuming rclone move with {} pending input(s); {} input(s) were already moved",
                pending_inputs.len(),
                resumed_inputs
            ));
        }

        if pending_inputs.is_empty() {
            info!(
                inputs = inputs.len(),
                "Rclone move batch is already complete because all sources are absent"
            );
            return Ok(success_output(Vec::new(), 0));
        }

        info!(
            "Rclone {} batch: {} pending files ({} total) from {} -> {}",
            cmd_op,
            pending_inputs.len(),
            inputs.len(),
            base_dir_str,
            Self::loggable_destination(remote_destination)
        );

        let mut last_error = None;
        let mut logs = Vec::new();

        for attempt in 0..*attempts {
            if attempt > 0 {
                info!("Retry attempt {} for rclone {} batch", attempt + 1, cmd_op);
                let error = last_error.as_deref().unwrap_or("unknown error");
                super::utils::wait_to_retry(
                    context,
                    ProgressKind::Rclone,
                    attempt,
                    *attempts,
                    &format!("rclone {cmd_op} attempt {attempt} of {attempts} failed: {error}"),
                )
                .await;
            }

            let files_from_path = Self::create_files_from_list(&pending_inputs, &base_dir)
                .await
                .map_err(|e| {
                    crate::Error::io_path("creating rclone files-from list", &base_dir, e)
                })?;
            let files_from_path_str = files_from_path.to_string_lossy().to_string();
            let mut cmd = self.command(execution)?;
            cmd.args(JSON_STATS_ARGS);
            cmd.args(SINGLE_PASS_ARGS);
            cmd.args([
                "--files-from",
                &files_from_path_str,
                cmd_op,
                &base_dir_str,
                remote_destination,
            ]);

            // Throughput flags first, so any duplicates in `extra_args` win.
            cmd.args(*throughput);
            for arg in *extra_args {
                cmd.arg(arg);
            }

            let command_result = self.command_runner.run(&mut cmd, context).await;
            if let Err(e) = files_from_path.close() {
                warn!(
                    path = %files_from_path_str,
                    error = %e,
                    "Failed to remove rclone files-from manifest"
                );
            }

            if matches!(*operation, RcloneOperation::Move) {
                let moved_inputs = Self::take_moved_inputs(&mut pending_inputs).await;
                completed_inputs = completed_inputs.saturating_add(moved_inputs.len());
                if !moved_inputs.is_empty() {
                    info!(
                        attempt = attempt + 1,
                        moved_inputs = moved_inputs.len(),
                        pending_inputs = pending_inputs.len(),
                        "Reconciled successfully moved rclone inputs"
                    );
                    context.info(format!(
                        "Rclone move attempt {} completed {} input(s); {} remain pending",
                        attempt + 1,
                        moved_inputs.len(),
                        pending_inputs.len()
                    ));
                }
            }

            match command_result {
                Ok(command_output) if command_output.status.success() => {
                    logs.extend(command_output.logs);
                    let duration = start.elapsed().as_secs_f64();
                    info!(
                        "Rclone {} batch completed in {:.2}s ({} files)",
                        cmd_op,
                        duration,
                        inputs.len()
                    );
                    return Ok(success_output(logs, attempt + 1));
                }
                Ok(command_output) => {
                    let error_msg = command_output
                        .last_error_message()
                        .map(str::to_owned)
                        .unwrap_or_else(|| "Unknown error".to_string());
                    let exit_code = command_output.status.code().unwrap_or(-1);
                    logs.extend(command_output.logs);

                    if matches!(*operation, RcloneOperation::Move) && pending_inputs.is_empty() {
                        warn!(
                            attempt = attempt + 1,
                            exit_code,
                            "Rclone exited unsuccessfully after moving every pending input; treating the batch as successful"
                        );
                        return Ok(success_output(logs, attempt + 1));
                    }

                    last_error = Some(format!(
                        "rclone batch failed with exit code {}: {}",
                        exit_code, error_msg
                    ));
                }
                Err(e) => {
                    if matches!(*operation, RcloneOperation::Move) && pending_inputs.is_empty() {
                        warn!(
                            attempt = attempt + 1,
                            error = %e,
                            "Rclone reported an execution error after moving every pending input"
                        );
                        return Ok(success_output(logs, attempt + 1));
                    }

                    last_error = Some(format!("Failed to execute rclone: {}", e));
                }
            }
        }

        error!(
            pending_inputs = pending_inputs.len(),
            completed_inputs, attempts, "Rclone {} batch failed", cmd_op
        );
        let error = last_error.unwrap_or_else(|| "Rclone batch failed".to_string());

        // A move confirms each transferred file by the absence of its source, so
        // a batch that failed part-way still has per-file results: the moved
        // inputs are reported as completed and the rest as failed, and the
        // worker pool fails the job from `failed_inputs`. Copy and sync give no
        // per-file signal, so every input of a failed batch is failed.
        let moved: Vec<String> = inputs
            .iter()
            .filter(|input| !pending_inputs.contains(input))
            .cloned()
            .collect();
        if matches!(*operation, RcloneOperation::Move) && !moved.is_empty() {
            return Ok(ProcessorOutput {
                outputs: vec![],
                duration_secs: start.elapsed().as_secs_f64(),
                metadata: Some(
                    serde_json::json!({
                        "batch_size": inputs.len(),
                        "base_dir": base_dir_str,
                        "operation": cmd_op,
                        "attempts": attempts,
                        "resumed_inputs": resumed_inputs,
                        "moved_inputs": moved.len(),
                        "failed_inputs": pending_inputs.len(),
                    })
                    .to_string(),
                ),
                items_produced: vec![],
                input_size_bytes: Some(total_input_size),
                output_size_bytes: None,
                failed_inputs: pending_inputs
                    .iter()
                    .map(|input| (input.clone(), error.clone()))
                    .collect(),
                succeeded_inputs: moved.clone(),
                skipped_inputs: vec![],
                uploads: inputs
                    .iter()
                    .map(|input| {
                        let completed = moved.contains(input);
                        UploadResultItem {
                            local_path: input.clone(),
                            remote_path: completed
                                .then(|| {
                                    Self::batch_remote_path(&base_dir, remote_destination, input)
                                })
                                .flatten(),
                            size_bytes: file_sizes.get(input).copied(),
                            status: if completed {
                                UploadItemStatus::Completed
                            } else {
                                UploadItemStatus::Failed
                            },
                            error: (!completed).then(|| error.clone()),
                        }
                    })
                    .collect(),
                logs,
            });
        }
        Err(crate::Error::Other(error))
    }

    /// Determine remote destination path with placeholder expansion.
    /// Supports: {streamer}, {title}, {streamer_id}, {session_id}, and time placeholders (%Y, %m, %d, etc.)
    ///
    /// Time placeholders use the configured [`TimeAnchor`]. `job_created` preserves
    /// retry consistency, while `session_start` groups a live session under its start date.
    fn determine_remote_destination(input: &ProcessorInput, config: &RcloneConfig) -> String {
        // For batch mode, we need a destination root (directory), not a specific file path
        let remote_destination_raw = if let Some(out) = input.outputs.first() {
            out.clone()
        } else if let Some(root) = config.destination_root.as_deref() {
            root.trim_end_matches('/').to_string()
        } else {
            String::new()
        };

        let reference_timestamp_ms = config.time_anchor.reference_time(input).as_millisecond();

        // Debug: Log all placeholder-related values before expansion
        tracing::debug!(
            template = %Self::loggable_destination(&remote_destination_raw),
            streamer_id = %input.streamer_id,
            session_id = %input.session_id,
            streamer_name = ?input.streamer_name,
            session_title = ?input.session_title,
            created_at = %input.created_at,
            session_start = ?input.session_start,
            time_anchor = ?config.time_anchor,
            "Rclone: Expanding placeholders"
        );

        // Expand placeholders in destination path using reference timestamp
        let expanded =
            input.expand_placeholders(&remote_destination_raw, Some(reference_timestamp_ms));

        tracing::debug!(
            template = %Self::loggable_destination(&remote_destination_raw),
            expanded = %Self::loggable_destination(&expanded),
            "Rclone: Placeholder expansion result"
        );

        expanded
    }
}

impl Default for RcloneProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for RcloneProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Upload
    }

    fn job_types(&self) -> Vec<&'static str> {
        // Both job type names select the rclone processor.
        vec!["rclone", "upload"]
    }

    fn name(&self) -> &'static str {
        "RcloneProcessor"
    }

    /// Indicates this processor supports batch input for efficiency.
    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        // Validate we have at least one input
        if input.inputs.is_empty() {
            return Err(crate::Error::Validation(
                "No input files provided for RcloneProcessor".to_string(),
            ));
        }

        let config: RcloneConfig =
            super::utils::parse_config_strict(input.config.as_deref(), "rclone")?;

        // Only persisted retries may begin with sources consumed by an earlier
        // execution. Validate the entire initial batch before transferring any file.
        if !matches!(config.operation, RcloneOperation::Move) || !ctx.is_retry {
            super::utils::ensure_inputs_exist(&input.inputs).await?;
        }

        let remote_destination = Self::determine_remote_destination(input, &config);

        // Sync operation is only allowed in batch mode (directory sync semantics)
        if matches!(config.operation, RcloneOperation::Sync) && input.inputs.len() == 1 {
            return Err(crate::Error::Validation(
                "Sync operation is not supported for single file uploads. \
                 Use 'copy' or 'move' instead. Sync is designed for directory synchronization \
                 and may delete files at the destination."
                    .to_string(),
            ));
        }

        // Throughput flags are computed once and shared across both code paths.
        // They go on the command line *before* `config.args`, so user-supplied
        // extra args win on duplicate flags (rclone applies last-wins).
        let throughput = config.throughput_args();

        let (extra_args, stripped_progress_args) = sanitize_extra_args(&config.args);
        if stripped_progress_args > 0 {
            ctx.warn(format!(
                "Removed {} unsupported --progress/-P argument(s) from rclone extra args; \
                 transfer progress is reported from rclone's periodic stats instead",
                stripped_progress_args
            ));
        }

        let attempts = config.max_retries.unwrap_or(DEFAULT_ATTEMPTS).max(1);
        let proxy =
            crate::proxies::upload_target(self.routes.as_ref(), &config.proxy_route).await?;
        let scrubber = crate::proxies::OutputScrubber::for_target(&proxy);
        let ctx = &ctx.scrubbing(&scrubber);
        let result = async {
            // Only a retried move can find inputs already consumed and uploaded.
            let skip_absent_inputs =
                matches!(config.operation, RcloneOperation::Move) && ctx.is_retry;

            // Choose single or batch mode based on input count
            if input.inputs.len() == 1 {
                // Single file mode - append filename to destination
                let input_path = &input.inputs[0];
                let input_file = Path::new(input_path);
                let file_name = input_file.file_name().unwrap_or_default().to_string_lossy();

                // If destination doesn't look like it includes the filename, append it
                let full_destination =
                    if remote_destination.ends_with('/') || config.destination_root.is_some() {
                        format!("{}/{}", remote_destination.trim_end_matches('/'), file_name)
                    } else {
                        remote_destination.clone()
                    };

                let execution = RcloneExecution {
                    remote_destination: &full_destination,
                    proxy: &proxy,
                    operation: config.operation,
                    config_path: config.config_path.as_deref(),
                    throughput: &throughput,
                    extra_args: &extra_args,
                    attempts,
                    context: ctx,
                };
                let targets =
                    Self::mkdir_targets(&input.inputs, &full_destination, None, skip_absent_inputs)
                        .await;
                self.ensure_remote_dirs(&targets, &execution).await?;
                self.process_single(input_path, &execution).await
            } else {
                // Batch mode - use --files-from
                info!("Using batch mode for {} files", input.inputs.len());

                let execution = RcloneExecution {
                    remote_destination: &remote_destination,
                    proxy: &proxy,
                    operation: config.operation,
                    config_path: config.config_path.as_deref(),
                    throughput: &throughput,
                    extra_args: &extra_args,
                    attempts,
                    context: ctx,
                };
                // Same base directory `process_batch` lays the inputs out from;
                // without one it rejects the batch before any transfer.
                let targets = match Self::find_common_base_dir(&input.inputs) {
                    Some(base_dir) => {
                        Self::mkdir_targets(
                            &input.inputs,
                            &remote_destination,
                            Some(&base_dir),
                            skip_absent_inputs,
                        )
                        .await
                    }
                    None => Vec::new(),
                };
                self.ensure_remote_dirs(&targets, &execution).await?;
                self.process_batch(&input.inputs, &execution).await
            }
        }
        .await;
        ProcessorOutput::scrubbed(result, &scrubber)
    }
}

#[cfg(test)]
mod tests;
