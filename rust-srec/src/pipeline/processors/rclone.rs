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
use crate::utils::filename::expand_placeholders_at;

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
    /// `{session_id}`, and chrono-style time tokens (`%Y`, `%m`, `%d`, ...).
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

/// Processor for interacting with Rclone.
pub struct RcloneProcessor {
    /// Path to rclone binary.
    rclone_path: String,
    /// Runs assembled rclone commands; a seam so tests can observe and
    /// stub invocations without spawning a real process.
    command_runner: Arc<dyn RcloneCommandRunner>,
    /// Serializes destination directory creation across concurrent jobs.
    dir_gate: RemoteDirGate,
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
        super::utils::run_rclone_with_progress(
            command,
            &context.progress,
            Some(context.log_sink.clone()),
        )
        .await
    }
}

struct RcloneExecution<'a> {
    remote_destination: &'a str,
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
            std::env::var("RCLONE_PATH").unwrap_or_else(|_| "rclone".to_string()),
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
        }
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
                execution.context.warn(format!(
                    "rclone mkdir attempt {attempt} of {} failed: {error}",
                    execution.attempts
                ));
                if attempt >= execution.attempts {
                    return Err(error);
                }
                tokio::time::sleep(super::utils::retry_delay(1000, attempt)).await;
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
        let mut cmd = Command::new(&self.rclone_path);
        if let Some(cfg) = execution.config_path {
            cmd.arg("--config").arg(cfg);
        }
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
            config_path,
            throughput,
            extra_args,
            attempts,
            context,
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
                tokio::time::sleep(super::utils::retry_delay(1000, attempt)).await;
            }

            let mut cmd = Command::new(&self.rclone_path);

            if let Some(cfg) = *config_path {
                cmd.arg("--config").arg(cfg);
            }

            // `run_rclone_with_progress` parses each stderr line as a
            // `--use-json-log` object; entries carrying `stats` become
            // progress snapshots. `--stats-log-level NOTICE` must not be
            // filtered by the main `--log-level`, or the periodic stats
            // never reach stderr. `--stats-one-line` only shrinks the human
            // `msg` embedded in each stats object.
            cmd.args([
                "--use-json-log",
                "--log-level",
                "NOTICE",
                "--stats-log-level",
                "NOTICE",
                "--stats",
                "1s",
                "--stats-one-line",
                cmd_op,
                input_path,
                remote_destination,
            ]);

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
            config_path,
            throughput,
            extra_args,
            attempts,
            context,
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
                tokio::time::sleep(super::utils::retry_delay(1000, attempt)).await;
            }

            let files_from_path = Self::create_files_from_list(&pending_inputs, &base_dir)
                .await
                .map_err(|e| {
                    crate::Error::io_path("creating rclone files-from list", &base_dir, e)
                })?;
            let files_from_path_str = files_from_path.to_string_lossy().to_string();
            let mut cmd = Command::new(&self.rclone_path);

            if let Some(cfg) = *config_path {
                cmd.arg("--config").arg(cfg);
            }

            // Same stderr contract as `process_single`: JSON log lines with
            // stats at NOTICE so `run_rclone_with_progress` receives them.
            cmd.args([
                "--use-json-log",
                "--log-level",
                "NOTICE",
                "--stats-log-level",
                "NOTICE",
                "--stats",
                "1s",
                "--stats-one-line",
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

        let reference_timestamp_ms = config.time_anchor.reference_time(input).timestamp_millis();

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
        let expanded = expand_placeholders_at(
            &remote_destination_raw,
            &input.streamer_id,
            &input.session_id,
            input.streamer_name.as_deref(),
            input.session_title.as_deref(),
            input.platform.as_deref(),
            Some(reference_timestamp_ms),
        );

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
        ProcessorType::Io
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

        let config: RcloneConfig = match input.config.as_deref() {
            Some(s) => serde_json::from_str(s).map_err(|e| {
                crate::Error::Validation(format!("Invalid rclone config JSON: {e}"))
            })?,
            None => RcloneConfig::default(),
        };

        // Only persisted retries may begin with sources consumed by an earlier
        // execution. Validate the entire initial batch before transferring any file.
        if !matches!(config.operation, RcloneOperation::Move) || !ctx.is_retry {
            for input_path in &input.inputs {
                let path = Path::new(input_path);
                let exists = tokio::fs::try_exists(path)
                    .await
                    .map_err(|error| crate::Error::io_path("try_exists", path, error))?;
                if !exists {
                    return Err(crate::Error::Validation(format!(
                        "Input file does not exist: {}",
                        input_path
                    )));
                }
            }
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

        // Only a retried move can find inputs already consumed and uploaded.
        let skip_absent_inputs = matches!(config.operation, RcloneOperation::Move) && ctx.is_retry;

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
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::super::test_utils::{test_exit_status, utc_datetime};
    use super::*;

    struct MockAttempt {
        moved_inputs: Vec<PathBuf>,
        succeeds: bool,
    }

    #[derive(Clone, Copy)]
    enum MkdirOutcome {
        Succeed,
        Fail,
        /// Never completes, like an rclone stuck retrying a throttled remote.
        Hang,
    }

    /// How long an armed [`FakeRemote`] holds a resolver between finding a
    /// directory missing and creating it, waiting for a second resolver.
    /// Race tests run on paused time, where this elapses only once every
    /// other task is blocked (blocking file IO holds the clock), so a second
    /// resolver that can still arrive always does. Shorter than
    /// [`MKDIR_TIMEOUT`], so a lone `mkdir` waits it out instead of failing.
    const RACE_WINDOW: Duration = Duration::from_secs(10);

    /// Bound on a test's jobs; on paused time a deadlock reaches it at once.
    const TEST_DEADLINE: Duration = Duration::from_secs(3600);

    struct FakeFolder {
        remote: String,
        parent: Option<usize>,
        name: String,
    }

    /// Remote filesystem that, like Google Drive, keeps same-name sibling
    /// folders. Every resolver (mkdir or transfer) looks a path segment up
    /// and creates it when missing, with no re-check in between — the gap
    /// separate rclone processes race in. Once armed, the first two
    /// creations wait in that gap for each other, so two unsynchronized
    /// resolvers of one missing directory deterministically both create it.
    struct FakeRemote {
        /// Folder IDs are indices; a `None` parent is the remote's root.
        folders: Mutex<Vec<FakeFolder>>,
        race_armed: AtomicBool,
        race_slots: AtomicUsize,
        race_barrier: tokio::sync::Barrier,
    }

    impl FakeRemote {
        fn new() -> Self {
            Self {
                folders: Mutex::default(),
                race_armed: AtomicBool::new(false),
                race_slots: AtomicUsize::new(0),
                race_barrier: tokio::sync::Barrier::new(2),
            }
        }

        fn arm_race(&self) {
            self.race_armed.store(true, Ordering::SeqCst);
        }

        async fn create_dir_all(&self, dir: &str) {
            let (remote, path) = dir.split_once(':').expect("fake paths name a remote");
            let mut parent = None;
            for name in path.split('/').filter(|segment| !segment.is_empty()) {
                let existing = self.folders.lock().unwrap().iter().position(|folder| {
                    folder.remote == remote && folder.parent == parent && folder.name == name
                });
                let id = match existing {
                    Some(id) => id,
                    None => {
                        if self.race_armed.load(Ordering::SeqCst)
                            && self.race_slots.fetch_add(1, Ordering::SeqCst) < 2
                        {
                            // A lone resolver times out and proceeds.
                            let _ =
                                tokio::time::timeout(RACE_WINDOW, self.race_barrier.wait()).await;
                        }
                        let mut folders = self.folders.lock().unwrap();
                        folders.push(FakeFolder {
                            remote: remote.to_owned(),
                            parent,
                            name: name.to_owned(),
                        });
                        folders.len() - 1
                    }
                };
                parent = Some(id);
            }
        }

        fn folder_path(folders: &[FakeFolder], id: usize) -> String {
            let folder = &folders[id];
            match folder.parent {
                Some(parent) => format!("{}/{}", Self::folder_path(folders, parent), folder.name),
                None => format!("{}:{}", folder.remote, folder.name),
            }
        }

        /// Every folder's path, sorted; duplicate folders repeat a path.
        fn dir_paths(&self) -> Vec<String> {
            let folders = self.folders.lock().unwrap();
            let mut paths: Vec<String> = (0..folders.len())
                .map(|id| Self::folder_path(&folders, id))
                .collect();
            paths.sort();
            paths
        }
    }

    struct MockRcloneCommandRunner {
        attempts: Mutex<VecDeque<MockAttempt>>,
        manifests: Mutex<Vec<Vec<String>>>,
        commands: Mutex<Vec<Vec<String>>>,
        /// `mkdir` invocations, kept apart from `commands` and answered from
        /// `mkdir_outcomes` (default: succeed) rather than `attempts`.
        mkdirs: Mutex<Vec<Vec<String>>>,
        mkdir_outcomes: Mutex<VecDeque<MkdirOutcome>>,
        mkdir_started: tokio::sync::Notify,
        remote: FakeRemote,
    }

    impl MockRcloneCommandRunner {
        fn new(attempts: Vec<MockAttempt>) -> Self {
            Self {
                attempts: Mutex::new(attempts.into()),
                manifests: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
                mkdirs: Mutex::new(Vec::new()),
                mkdir_outcomes: Mutex::new(VecDeque::new()),
                mkdir_started: tokio::sync::Notify::new(),
                remote: FakeRemote::new(),
            }
        }

        fn with_mkdir_outcomes(self, outcomes: Vec<MkdirOutcome>) -> Self {
            *self.mkdir_outcomes.lock().unwrap() = outcomes.into();
            self
        }

        fn manifests(&self) -> Vec<Vec<String>> {
            self.manifests.lock().unwrap().clone()
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().unwrap().clone()
        }

        fn mkdirs(&self) -> Vec<Vec<String>> {
            self.mkdirs.lock().unwrap().clone()
        }

        /// The directory each recorded `mkdir` targeted.
        fn mkdir_targets(&self) -> Vec<String> {
            self.mkdirs()
                .iter()
                .map(|args| {
                    let index = args.iter().position(|arg| arg == "mkdir").unwrap();
                    args[index + 1].clone()
                })
                .collect()
        }

        async fn run_mkdir(&self, target: &str) -> CommandOutput {
            self.mkdir_started.notify_one();

            let outcome = self
                .mkdir_outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(MkdirOutcome::Succeed);
            let succeeds = match outcome {
                MkdirOutcome::Succeed => {
                    self.remote.create_dir_all(target).await;
                    true
                }
                MkdirOutcome::Fail => false,
                MkdirOutcome::Hang => std::future::pending().await,
            };
            CommandOutput {
                status: test_exit_status(succeeds),
                duration: 0.0,
                logs: if succeeds {
                    Vec::new()
                } else {
                    vec![crate::pipeline::job_queue::JobLogEntry::error(
                        "simulated mkdir failure",
                    )]
                },
            }
        }
    }

    #[async_trait]
    impl RcloneCommandRunner for MockRcloneCommandRunner {
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
            if let Some(index) = args.iter().position(|arg| arg == "mkdir") {
                let target = args[index + 1].clone();
                self.mkdirs.lock().unwrap().push(args);
                return Ok(self.run_mkdir(&target).await);
            }

            // Single-file commands (copyto/moveto) carry no --files-from;
            // only batch commands record a manifest.
            let mut file_destinations = Vec::new();
            if let Some(manifest_index) = args.iter().position(|arg| arg == "--files-from") {
                let manifest_path =
                    PathBuf::from(args.get(manifest_index + 1).ok_or_else(|| {
                        crate::Error::Other("missing files-from path".to_string())
                    })?);
                let manifest: Vec<String> = tokio::fs::read_to_string(&manifest_path)
                    .await
                    .map_err(|e| {
                        crate::Error::io_path("reading test rclone manifest", &manifest_path, e)
                    })?
                    .lines()
                    .map(str::to_string)
                    .collect();
                // `--files-from <manifest> <op> <base_dir> <destination>`
                let root = args[manifest_index + 4].trim_end_matches('/');
                file_destinations.extend(manifest.iter().map(|line| format!("{root}/{line}")));
                self.manifests.lock().unwrap().push(manifest);
            } else if let Some(index) = args
                .iter()
                .position(|arg| arg == "copyto" || arg == "moveto")
            {
                // `<op> <source> <destination>`
                file_destinations.push(args[index + 2].clone());
            }
            self.commands.lock().unwrap().push(args);
            // A transfer resolves each file's directory the same way.
            for destination in &file_destinations {
                if let Some(dir) = RcloneProcessor::remote_parent(destination) {
                    self.remote.create_dir_all(dir).await;
                }
            }

            let attempt = self
                .attempts
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| crate::Error::Other("unexpected rclone attempt".to_string()))?;
            for input in attempt.moved_inputs {
                tokio::fs::remove_file(&input)
                    .await
                    .map_err(|e| crate::Error::io_path("moving test input", &input, e))?;
            }

            Ok(CommandOutput {
                status: test_exit_status(attempt.succeeds),
                duration: 0.0,
                logs: if attempt.succeeds {
                    Vec::new()
                } else {
                    vec![crate::pipeline::job_queue::JobLogEntry::error(
                        "simulated partial move failure",
                    )]
                },
            })
        }
    }

    fn expected_local_destination(dt: chrono::DateTime<chrono::Utc>) -> String {
        format!(
            "remote:/{}/StreamerName",
            pipeline_common::expand_path_template_at("%Y/%m/%d", Some(dt.timestamp_millis()))
        )
    }

    /// A move batch that fails after transferring some inputs reports each
    /// file's own result: the moved ones as completed, the rest as failed, so
    /// the worker pool fails the job without recording transferred files as
    /// failed uploads.
    #[tokio::test(start_paused = true)]
    async fn batch_move_partial_failure_reports_per_file_results() {
        let temp_dir = tempfile::tempdir().unwrap();
        let names = ["a.mp4", "b.mp4", "c.mp4", "d.mp4"];
        let paths: Vec<PathBuf> = names
            .iter()
            .map(|name| temp_dir.path().join(name))
            .collect();
        for path in &paths {
            tokio::fs::write(path, b"video").await.unwrap();
        }
        // Every attempt moves one more input and then fails.
        let runner = Arc::new(MockRcloneCommandRunner::new(
            paths[..3]
                .iter()
                .map(|path| MockAttempt {
                    moved_inputs: vec![path.clone()],
                    succeeds: false,
                })
                .collect(),
        ));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let inputs: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let input = ProcessorInput {
            inputs: inputs.clone(),
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"move"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("partial-move"))
            .await
            .unwrap();

        assert_eq!(runner.commands().len(), 3);
        assert_eq!(output.succeeded_inputs, inputs[..3]);
        assert_eq!(output.failed_inputs.len(), 1);
        assert_eq!(output.failed_inputs[0].0, inputs[3]);
        assert!(output.failed_inputs[0].1.contains("rclone batch failed"));
        assert!(output.outputs.is_empty());
        assert_eq!(
            output
                .uploads
                .iter()
                .map(|item| (
                    item.local_path.as_str(),
                    item.status,
                    item.remote_path.is_some()
                ))
                .collect::<Vec<_>>(),
            vec![
                (inputs[0].as_str(), UploadItemStatus::Completed, true),
                (inputs[1].as_str(), UploadItemStatus::Completed, true),
                (inputs[2].as_str(), UploadItemStatus::Completed, true),
                (inputs[3].as_str(), UploadItemStatus::Failed, false),
            ]
        );
        assert!(paths[3].exists(), "the unmoved source stays for a retry");
    }

    #[tokio::test(start_paused = true)]
    async fn batch_move_retries_only_inputs_that_still_exist() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("first.mp4");
        let second = temp_dir.path().join("second.jpg");
        tokio::fs::write(&first, b"video").await.unwrap();
        tokio::fs::write(&second, b"thumbnail").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![
            MockAttempt {
                moved_inputs: vec![first.clone()],
                succeeds: false,
            },
            MockAttempt {
                moved_inputs: vec![second.clone()],
                succeeds: false,
            },
        ]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let inputs = vec![
            first.to_string_lossy().into_owned(),
            second.to_string_lossy().into_owned(),
        ];
        let input = ProcessorInput {
            inputs: inputs.clone(),
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"move"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("batch-move"))
            .await
            .unwrap();

        assert_eq!(output.succeeded_inputs, inputs);
        assert!(output.failed_inputs.is_empty());
        assert!(!first.exists());
        assert!(!second.exists());
        assert_eq!(
            runner.manifests(),
            vec![
                vec!["first.mp4".to_string(), "second.jpg".to_string()],
                vec!["second.jpg".to_string()],
            ]
        );
        assert!(runner.commands().iter().all(|args| {
            args.iter().any(|arg| arg == "move") && !args.iter().any(|arg| arg == "copy")
        }));
        assert!(std::fs::read_dir(temp_dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".rclone_files_")
        }));
    }

    #[tokio::test]
    async fn batch_move_skips_inputs_moved_by_a_previous_job_attempt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let already_moved = temp_dir.path().join("already-moved.mp4");
        let pending = temp_dir.path().join("pending.jpg");
        tokio::fs::write(&pending, b"thumbnail").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![pending.clone()],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let inputs = vec![
            already_moved.to_string_lossy().into_owned(),
            pending.to_string_lossy().into_owned(),
        ];
        let input = ProcessorInput {
            inputs: inputs.clone(),
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"move"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(
                &input,
                &ProcessorContext::noop("resumed-batch-move").with_retry(true),
            )
            .await
            .unwrap();

        assert_eq!(output.succeeded_inputs, inputs);
        assert_eq!(runner.manifests(), vec![vec!["pending.jpg".to_string()]]);
        assert!(!pending.exists());
    }

    #[tokio::test]
    async fn single_move_skips_input_moved_by_a_previous_job_attempt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let already_moved = temp_dir.path().join("already-moved.mp4");
        let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input_path = already_moved.to_string_lossy().into_owned();
        let input = ProcessorInput {
            inputs: vec![input_path.clone()],
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"move"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(
                &input,
                &ProcessorContext::noop("resumed-single-move").with_retry(true),
            )
            .await
            .unwrap();

        assert_eq!(output.succeeded_inputs, vec![input_path]);
        assert!(output.outputs.is_empty());
        assert!(runner.commands().is_empty());
    }

    #[tokio::test]
    async fn initial_move_rejects_missing_inputs_before_any_transfer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let present = temp_dir.path().join("present.mp4");
        tokio::fs::write(&present, b"video").await.unwrap();
        let present_input = present.to_string_lossy().into_owned();
        let missing = temp_dir
            .path()
            .join("missing.mp4")
            .to_string_lossy()
            .into_owned();
        let another_missing = temp_dir
            .path()
            .join("missing.jpg")
            .to_string_lossy()
            .into_owned();

        for inputs in [
            vec![missing.clone()],
            vec![present_input.clone(), missing.clone()],
            vec![missing.clone(), another_missing],
        ] {
            let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
            let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
            let input = ProcessorInput {
                inputs,
                outputs: vec!["remote:/records".to_owned()],
                config: Some(r#"{"operation":"move"}"#.to_owned()),
                ..Default::default()
            };
            let error = processor
                .process(&input, &ProcessorContext::noop("initial-move"))
                .await
                .unwrap_err();
            assert!(
                matches!(error, crate::Error::Validation(ref message) if message.contains(&missing))
            );
            assert!(
                runner.commands().is_empty(),
                "invalid batch must fail before rclone runs"
            );
            assert!(runner.manifests().is_empty());
            assert_eq!(tokio::fs::read(&present).await.unwrap(), b"video");
        }
    }

    #[tokio::test]
    async fn retried_move_accepts_batch_already_consumed_by_previous_attempt() {
        let temp_dir = tempfile::tempdir().unwrap();
        let inputs: Vec<String> = ["already-moved.mp4", "already-moved.jpg"]
            .iter()
            .map(|name| temp_dir.path().join(name).to_string_lossy().into_owned())
            .collect();
        let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: inputs.clone(),
            outputs: vec!["remote:/records".to_owned()],
            config: Some(r#"{"operation":"move"}"#.to_owned()),
            ..Default::default()
        };
        let output = processor
            .process(
                &input,
                &ProcessorContext::noop("resumed-complete-batch").with_retry(true),
            )
            .await
            .unwrap();
        assert_eq!(output.succeeded_inputs, inputs);
        assert!(output.outputs.is_empty());
        assert_eq!(output.uploads.len(), 2);
        assert!(
            output
                .uploads
                .iter()
                .all(|upload| upload.status == UploadItemStatus::Completed)
        );
        assert!(runner.commands().is_empty());
        assert!(runner.mkdirs().is_empty(), "nothing left to upload");
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(metadata["resumed_inputs"], 2);
        assert_eq!(metadata["attempts"], 0);
    }

    #[tokio::test]
    async fn single_move_succeeds_when_rclone_fails_after_consuming_the_source() {
        let temp_dir = tempfile::tempdir().unwrap();
        let source = temp_dir.path().join("input.mp4");
        tokio::fs::write(&source, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![source.clone()],
            succeeds: false,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input_path = source.to_string_lossy().into_owned();
        let input = ProcessorInput {
            inputs: vec![input_path.clone()],
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"move"}"#.to_string()),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("single-move-consumed"))
            .await
            .unwrap();

        assert_eq!(output.succeeded_inputs, vec![input_path]);
        assert!(output.outputs.is_empty());
        assert!(!source.exists());
        assert_eq!(runner.commands().len(), 1);
        assert!(runner.commands()[0].iter().any(|arg| arg == "moveto"));
    }

    fn succeeding_attempts(count: usize) -> Vec<MockAttempt> {
        (0..count)
            .map(|_| MockAttempt {
                moved_inputs: vec![],
                succeeds: true,
            })
            .collect()
    }

    /// Writes `names` into `dir` and returns their paths as job inputs.
    async fn write_inputs(dir: &Path, names: &[&str]) -> Vec<String> {
        let mut inputs = Vec::new();
        for name in names {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await.unwrap();
            }
            tokio::fs::write(&path, b"data").await.unwrap();
            inputs.push(path.to_string_lossy().into_owned());
        }
        inputs
    }

    fn upload_input(inputs: Vec<String>, destination_root: &str) -> ProcessorInput {
        ProcessorInput {
            inputs,
            config: Some(
                serde_json::json!({ "operation": "copy", "destination_root": destination_root })
                    .to_string(),
            ),
            ..Default::default()
        }
    }

    /// The incident: a single-file upload and a batch upload from one
    /// session start together into a day directory that does not exist yet.
    /// Without coordinated directory creation each rclone process creates
    /// `14` and the files split between two same-name folders.
    #[tokio::test(start_paused = true)]
    async fn concurrent_uploads_into_a_new_directory_share_one_remote_folder() {
        const DESTINATION: &str = "remote:records/Streamer/2026/10/14";
        let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
        runner
            .remote
            .create_dir_all("remote:records/Streamer/2026/10")
            .await;
        runner.remote.arm_race();
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let xml_dir = tempfile::tempdir().unwrap();
        let video_dir = tempfile::tempdir().unwrap();
        let xml_upload = upload_input(
            write_inputs(xml_dir.path(), &["danmu.xml"]).await,
            DESTINATION,
        );
        let video_upload = upload_input(
            write_inputs(video_dir.path(), &["video.mp4", "cover.jpg"]).await,
            DESTINATION,
        );
        let xml_context = ProcessorContext::noop("xml-upload");
        let video_context = ProcessorContext::noop("video-upload");

        let (xml_result, video_result) = tokio::time::timeout(TEST_DEADLINE, async {
            tokio::join!(
                processor.process(&xml_upload, &xml_context),
                processor.process(&video_upload, &video_context),
            )
        })
        .await
        .expect("uploads finish");
        xml_result.unwrap();
        video_result.unwrap();

        assert_eq!(
            runner.remote.dir_paths(),
            [
                "remote:records",
                "remote:records/Streamer",
                "remote:records/Streamer/2026",
                "remote:records/Streamer/2026/10",
                "remote:records/Streamer/2026/10/14",
            ]
        );
    }

    /// Different day directories still share a missing month directory, so
    /// creation is serialized per remote, not per destination path.
    #[tokio::test(start_paused = true)]
    async fn concurrent_uploads_to_sibling_directories_share_a_new_parent() {
        let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
        runner
            .remote
            .create_dir_all("remote:records/Streamer/2026")
            .await;
        runner.remote.arm_race();
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let first = upload_input(
            write_inputs(temp_dir.path(), &["first.mp4"]).await,
            "remote:records/Streamer/2026/10/14",
        );
        let second = upload_input(
            write_inputs(temp_dir.path(), &["second.mp4"]).await,
            "remote:records/Streamer/2026/10/15",
        );
        let first_context = ProcessorContext::noop("first");
        let second_context = ProcessorContext::noop("second");

        let (first_result, second_result) = tokio::time::timeout(TEST_DEADLINE, async {
            tokio::join!(
                processor.process(&first, &first_context),
                processor.process(&second, &second_context),
            )
        })
        .await
        .expect("uploads finish");
        first_result.unwrap();
        second_result.unwrap();

        assert_eq!(
            runner.remote.dir_paths(),
            [
                "remote:records",
                "remote:records/Streamer",
                "remote:records/Streamer/2026",
                "remote:records/Streamer/2026/10",
                "remote:records/Streamer/2026/10/14",
                "remote:records/Streamer/2026/10/15",
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn recent_mkdir_is_reused_until_its_ttl_expires() {
        const DESTINATION: &str = "remote:records/Streamer/2026/10/14";
        let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(3)));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let input = upload_input(
            write_inputs(temp_dir.path(), &["video.mp4"]).await,
            DESTINATION,
        );
        let context = ProcessorContext::noop("upload");

        processor.process(&input, &context).await.unwrap();
        processor.process(&input, &context).await.unwrap();
        assert_eq!(runner.mkdir_targets(), [DESTINATION]);

        tokio::time::advance(ENSURED_DIR_TTL + Duration::from_secs(1)).await;
        processor.process(&input, &context).await.unwrap();
        assert_eq!(runner.mkdir_targets(), [DESTINATION, DESTINATION]);
        assert_eq!(runner.commands().len(), 3);
    }

    /// The same path under another root folder is another directory, so a
    /// recent `mkdir` under one root does not stand in for the other.
    #[tokio::test]
    async fn recent_mkdir_is_not_reused_under_other_backend_args() {
        let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(2)));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let inputs = write_inputs(temp_dir.path(), &["video.mp4"]).await;
        let under_root = |root: &str| ProcessorInput {
            inputs: inputs.clone(),
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination_root": "remote:records/day",
                    "args": ["--drive-root-folder-id", root],
                })
                .to_string(),
            ),
            ..Default::default()
        };
        let context = ProcessorContext::noop("upload");

        processor.process(&under_root("a"), &context).await.unwrap();
        processor.process(&under_root("b"), &context).await.unwrap();

        assert_eq!(runner.mkdirs().len(), 2);
    }

    /// A throttled or hung `mkdir` is retried before the job gives up, like
    /// a transfer: failed jobs are only retried by hand.
    #[tokio::test(start_paused = true)]
    async fn transient_mkdir_failures_are_retried() {
        let runner = Arc::new(
            MockRcloneCommandRunner::new(succeeding_attempts(1))
                .with_mkdir_outcomes(vec![MkdirOutcome::Fail, MkdirOutcome::Hang]),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let input = upload_input(
            write_inputs(temp_dir.path(), &["video.mp4"]).await,
            "remote:records/day",
        );

        processor
            .process(&input, &ProcessorContext::noop("upload"))
            .await
            .unwrap();

        assert_eq!(runner.mkdirs().len(), 3);
        assert_eq!(runner.commands().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_mkdir_fails_the_job_before_any_transfer() {
        let runner = Arc::new(
            MockRcloneCommandRunner::new(succeeding_attempts(1))
                .with_mkdir_outcomes(vec![MkdirOutcome::Fail; 3]),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let input = upload_input(
            write_inputs(temp_dir.path(), &["video.mp4"]).await,
            "remote:records/day",
        );
        let context = ProcessorContext::noop("upload");

        let error = processor.process(&input, &context).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("rclone mkdir remote:records/day failed")
        );
        assert!(runner.commands().is_empty());

        assert_eq!(runner.mkdirs().len(), 3, "every attempt was used");

        // The failure is not remembered: the next job creates the directory.
        processor.process(&input, &context).await.unwrap();
        assert_eq!(runner.mkdirs().len(), 4);
        assert_eq!(runner.commands().len(), 1);
    }

    /// The preset's `max_retries` bounds both `mkdir` and transfer attempts.
    #[tokio::test(start_paused = true)]
    async fn configured_attempts_bound_mkdir_and_transfer_retries() {
        let failing = || MockAttempt {
            moved_inputs: vec![],
            succeeds: false,
        };
        let runner = Arc::new(
            MockRcloneCommandRunner::new(vec![failing(), failing(), failing()])
                .with_mkdir_outcomes(vec![MkdirOutcome::Fail]),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let inputs = write_inputs(temp_dir.path(), &["video.mp4"]).await;
        let input = |max_retries: u32| ProcessorInput {
            inputs: inputs.clone(),
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination_root": "remote:records/day",
                    "max_retries": max_retries,
                })
                .to_string(),
            ),
            ..Default::default()
        };
        let context = ProcessorContext::noop("upload");

        processor.process(&input(1), &context).await.unwrap_err();
        assert_eq!(runner.mkdirs().len(), 1);
        assert!(runner.commands().is_empty());

        processor.process(&input(2), &context).await.unwrap_err();
        assert_eq!(runner.commands().len(), 2);

        // Zero still makes one attempt.
        processor.process(&input(0), &context).await.unwrap_err();
        assert_eq!(runner.commands().len(), 3);
    }

    /// Cancelling a job while its `mkdir` holds the remote's lock must not
    /// keep later jobs out.
    #[tokio::test]
    async fn cancelled_mkdir_releases_the_remote_lock() {
        let runner = Arc::new(
            MockRcloneCommandRunner::new(succeeding_attempts(1))
                .with_mkdir_outcomes(vec![MkdirOutcome::Hang]),
        );
        let processor = Arc::new(RcloneProcessor::with_command_runner(
            "rclone",
            runner.clone(),
        ));
        let temp_dir = tempfile::tempdir().unwrap();
        let input = upload_input(
            write_inputs(temp_dir.path(), &["video.mp4"]).await,
            "remote:records/day",
        );

        let holder = tokio::spawn({
            let processor = processor.clone();
            let input = input.clone();
            async move {
                processor
                    .process(&input, &ProcessorContext::noop("holder"))
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(10), runner.mkdir_started.notified())
            .await
            .expect("holder runs mkdir");
        let lock = processor.dir_gate.remote("remote");
        assert!(lock.try_lock().is_err(), "the hung mkdir holds the lock");

        holder.abort();
        assert!(holder.await.unwrap_err().is_cancelled());
        assert!(lock.try_lock().is_ok(), "cancellation released the lock");

        processor
            .process(&input, &ProcessorContext::noop("next"))
            .await
            .unwrap();
        assert_eq!(runner.mkdirs().len(), 2);
    }

    #[tokio::test]
    async fn mkdir_uses_transfer_flags_and_covers_nested_batch_directories() {
        let runner = Arc::new(MockRcloneCommandRunner::new(succeeding_attempts(1)));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let temp_dir = tempfile::tempdir().unwrap();
        let input = ProcessorInput {
            inputs: write_inputs(temp_dir.path(), &["a.mp4", "nested/b.mp4", "c.jpg"]).await,
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination_root": "remote:records/day",
                    "config_path": "/config/rclone.conf",
                    "transfers": 2,
                    "args": ["--drive-root-folder-id", "folder-id"],
                })
                .to_string(),
            ),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("batch"))
            .await
            .unwrap();

        assert_eq!(
            runner.mkdirs()[0],
            [
                "--config",
                "/config/rclone.conf",
                "--use-json-log",
                "--log-level",
                "NOTICE",
                "mkdir",
                "remote:records/day",
                "--transfers",
                "2",
                "--drive-root-folder-id",
                "folder-id",
            ]
        );
        assert_eq!(
            runner.mkdir_targets(),
            ["remote:records/day", "remote:records/day/nested"]
        );
    }

    #[test]
    fn remote_name_and_parent_parse_destinations() {
        assert_eq!(RcloneProcessor::remote_name("gdrive:a/b"), Some("gdrive"));
        assert_eq!(
            RcloneProcessor::remote_name("gdrive,token=secret:a/b"),
            Some("gdrive")
        );
        assert_eq!(RcloneProcessor::remote_name(":drive,k=v:a"), Some(":drive"));
        assert_eq!(RcloneProcessor::remote_name("/local/a:b"), None);
        assert_eq!(RcloneProcessor::remote_name("relative/path"), None);
        if cfg!(windows) {
            assert_eq!(RcloneProcessor::remote_name(r"C:\records"), None);
        }

        assert_eq!(
            RcloneProcessor::remote_parent("gdrive:records/day/a.mp4"),
            Some("gdrive:records/day")
        );
        assert_eq!(
            RcloneProcessor::remote_parent("gdrive,k=v:records/a.mp4"),
            Some("gdrive,k=v:records")
        );
        assert_eq!(
            RcloneProcessor::remote_parent("gdrive:/records/a.mp4"),
            Some("gdrive:/records")
        );
        assert_eq!(
            RcloneProcessor::remote_parent(":drive,k=v:records/a.mp4"),
            Some(":drive,k=v:records")
        );
        // Parameter values may hold `/`, and quoted ones `:` and `,`.
        assert_eq!(
            RcloneProcessor::remote_parent(":s3,secret_access_key=a/b:bucket/day/a.mp4"),
            Some(":s3,secret_access_key=a/b:bucket/day")
        );
        assert_eq!(
            RcloneProcessor::split_remote(r#"drive,root='a:b,c''d',x="e:f":records/a.mp4"#),
            Some((r#"drive,root='a:b,c''d',x="e:f""#, "records/a.mp4"))
        );
        assert_eq!(
            RcloneProcessor::remote_name("drive,root='a:b':records"),
            Some("drive")
        );
        assert_eq!(RcloneProcessor::remote_parent("gdrive:/records"), None);
        assert_eq!(RcloneProcessor::remote_parent("gdrive:a.mp4"), None);
        assert_eq!(RcloneProcessor::remote_parent("/local/dir/a.mp4"), None);
    }

    /// `plain.txt/child.mp4` makes `try_exists` fail with `NotADirectory`
    /// instead of reporting absence; `partition_move_inputs` must keep such
    /// inputs pending rather than count them as already moved.
    #[cfg(unix)]
    #[tokio::test]
    async fn partition_move_inputs_keeps_unverifiable_paths_pending() {
        let temp_dir = tempfile::tempdir().unwrap();
        let not_a_dir = temp_dir.path().join("plain.txt");
        std::fs::write(&not_a_dir, b"file").unwrap();
        let unverifiable = not_a_dir.join("child.mp4").to_string_lossy().into_owned();

        let (pending, moved) =
            super::super::inputs::partition_absent_inputs(std::slice::from_ref(&unverifiable))
                .await;

        assert_eq!(pending, vec![unverifiable]);
        assert!(moved.is_empty());
    }

    #[tokio::test]
    async fn files_from_list_is_removed_when_guard_is_dropped() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input = temp_dir.path().join("input.mp4");
        tokio::fs::write(&input, b"video").await.unwrap();

        let manifest = RcloneProcessor::create_files_from_list(
            &[input.to_string_lossy().into_owned()],
            temp_dir.path(),
        )
        .await
        .unwrap();
        let manifest_path = manifest.to_path_buf();

        assert_eq!(
            tokio::fs::read_to_string(&manifest_path).await.unwrap(),
            "input.mp4\n"
        );
        assert!(manifest_path.exists());
        drop(manifest);
        assert!(!manifest_path.exists());
    }

    #[tokio::test]
    async fn copy_still_rejects_missing_inputs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let input = ProcessorInput {
            inputs: vec![
                temp_dir
                    .path()
                    .join("missing.mp4")
                    .to_string_lossy()
                    .into_owned(),
            ],
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"copy"}"#.to_string()),
            ..Default::default()
        };

        for is_retry in [false, true] {
            let error = RcloneProcessor::new()
                .process(
                    &input,
                    &ProcessorContext::noop("missing-copy").with_retry(is_retry),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Input file does not exist"));
        }
    }

    /// The ordered flag block `process_single`/`process_batch` place before
    /// the subcommand; `run_rclone_with_progress` parses stderr on the
    /// assumption that every one of these is present.
    const EXPECTED_STATS_FLAGS: [&str; 8] = [
        "--use-json-log",
        "--log-level",
        "NOTICE",
        "--stats-log-level",
        "NOTICE",
        "--stats",
        "1s",
        "--stats-one-line",
    ];

    fn stats_flag_window(args: &[String]) -> Vec<&str> {
        let pos = args
            .iter()
            .position(|arg| arg == "--use-json-log")
            .expect("argv contains --use-json-log");
        args[pos..pos + EXPECTED_STATS_FLAGS.len()]
            .iter()
            .map(String::as_str)
            .collect()
    }

    #[tokio::test]
    async fn single_copy_passes_json_stats_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"copy"}"#.to_string()),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("single-copy-flags"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands.len(), 1);
        let args = &commands[0];
        assert_eq!(stats_flag_window(args), EXPECTED_STATS_FLAGS);
        assert!(!args.iter().any(|arg| arg == "--stats-one-line-date"));
        assert!(args.iter().any(|arg| arg == "copyto"));
    }

    #[tokio::test]
    async fn batch_copy_passes_json_stats_flags() {
        let temp_dir = tempfile::tempdir().unwrap();
        let first = temp_dir.path().join("first.mp4");
        let second = temp_dir.path().join("second.mp4");
        tokio::fs::write(&first, b"a").await.unwrap();
        tokio::fs::write(&second, b"b").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
            outputs: vec!["remote:/records".to_string()],
            config: Some(r#"{"operation":"copy"}"#.to_string()),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("batch-copy-flags"))
            .await
            .unwrap();

        let commands = runner.commands();
        assert_eq!(commands.len(), 1);
        let args = &commands[0];
        assert_eq!(stats_flag_window(args), EXPECTED_STATS_FLAGS);
        assert!(!args.iter().any(|arg| arg == "--stats-one-line-date"));
        assert!(args.iter().any(|arg| arg == "--files-from"));
    }

    #[tokio::test]
    async fn progress_flags_are_stripped_from_extra_args() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec!["remote:/records".to_string()],
            config: Some(
                r#"{"operation":"copy","args":["--progress","-P","--transfers","2"]}"#.to_string(),
            ),
            ..Default::default()
        };

        processor
            .process(&input, &ProcessorContext::noop("strip-progress"))
            .await
            .unwrap();

        let args = &runner.commands()[0];
        assert!(!args.iter().any(|arg| arg == "--progress" || arg == "-P"));
        // Surviving extra args still trail the built-in flag block so they
        // keep last-wins override semantics.
        let transfers_pos = args.iter().position(|arg| arg == "--transfers").unwrap();
        let one_line_pos = args
            .iter()
            .position(|arg| arg == "--stats-one-line")
            .unwrap();
        assert!(transfers_pos > one_line_pos);
        assert_eq!(args[transfers_pos + 1], "2");
    }

    /// End-to-end against a real rclone binary: runs the production flag
    /// set on a bwlimit'd local copy and asserts progress snapshots stream
    /// out of the JSON stats parsing while stats never leak into job logs.
    /// Run manually: `cargo test -p rust-srec --lib live_rclone -- --ignored`.
    #[tokio::test]
    #[ignore = "requires an rclone binary on PATH"]
    async fn live_rclone_copy_streams_progress_snapshots() {
        use crate::pipeline::processors::traits::JobLogSink;
        use std::sync::atomic::AtomicUsize;

        let temp_dir = tempfile::tempdir().unwrap();
        let src = temp_dir.path().join("src.bin");
        let dest_dir = temp_dir.path().join("dest");
        tokio::fs::create_dir(&dest_dir).await.unwrap();
        let payload = vec![0u8; 300 * 1024];
        tokio::fs::write(&src, &payload).await.unwrap();

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(64);
        let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(256);
        let ctx = ProcessorContext::new(
            "live-job",
            crate::pipeline::ProgressReporter::new("live-job", progress_tx),
            JobLogSink::new(log_tx, Arc::new(AtomicUsize::new(0))),
            tokio_util::sync::CancellationToken::new(),
        );

        let processor = RcloneProcessor::new();
        let dest = format!("{}/", dest_dir.to_string_lossy().replace('\\', "/"));
        let input = ProcessorInput {
            inputs: vec![src.to_string_lossy().into_owned()],
            outputs: vec![dest],
            // bwlimit throttles the copy across several 1s stats ticks.
            config: Some(r#"{"operation":"copy","bwlimit":"100k"}"#.to_string()),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(output.succeeded_inputs.len(), 1);
        assert!(dest_dir.join("src.bin").exists(), "file copied");

        drop(ctx);
        let mut snapshots = Vec::new();
        while let Ok(update) = progress_rx.try_recv() {
            assert_eq!(update.job_id, "live-job");
            snapshots.push(update.snapshot);
        }
        assert!(
            !snapshots.is_empty(),
            "no progress snapshots parsed from live rclone stderr"
        );
        let last = snapshots.last().unwrap();
        assert_eq!(last.bytes_total, Some(payload.len() as u64));
        assert!(last.percent.is_some(), "percent computed from stats");

        // Stats lines must be consumed as progress, never surface as logs.
        while let Ok(entry) = log_rx.try_recv() {
            assert!(
                !entry.message.contains("\"stats\""),
                "raw stats JSON leaked into job logs: {}",
                entry.message
            );
        }
    }

    #[test]
    fn sanitize_extra_args_strips_only_progress_tokens() {
        let args = vec![
            "--progress".to_string(),
            "-P".to_string(),
            "--transfers".to_string(),
            "2".to_string(),
        ];
        let (kept, stripped) = sanitize_extra_args(&args);
        assert_eq!(kept, vec!["--transfers".to_string(), "2".to_string()]);
        assert_eq!(stripped, 2);

        let (kept, stripped) = sanitize_extra_args(&[]);
        assert!(kept.is_empty());
        assert_eq!(stripped, 0);
    }

    #[test]
    fn test_batch_remote_path() {
        let base = Path::new("/home/user/videos");
        // Flat file directly under the base.
        assert_eq!(
            RcloneProcessor::batch_remote_path(
                base,
                "remote:bucket/dest",
                "/home/user/videos/a.mp4"
            ),
            Some("remote:bucket/dest/a.mp4".to_string())
        );
        // Nested file keeps its base-relative subpath.
        assert_eq!(
            RcloneProcessor::batch_remote_path(
                base,
                "remote:bucket/dest/",
                "/home/user/videos/2024/b.mp4"
            ),
            Some("remote:bucket/dest/2024/b.mp4".to_string())
        );
        // Input outside the base dir cannot be mapped.
        assert_eq!(
            RcloneProcessor::batch_remote_path(base, "remote:bucket", "/var/other/c.mp4"),
            None
        );
    }

    #[test]
    fn test_find_common_base_dir_single() {
        let paths = vec!["/home/user/videos/file1.mp4".to_string()];
        let result = RcloneProcessor::find_common_base_dir(&paths);
        assert_eq!(result, Some(PathBuf::from("/home/user/videos")));
    }

    #[test]
    fn test_find_common_base_dir_same_dir() {
        let paths = vec![
            "/home/user/videos/file1.mp4".to_string(),
            "/home/user/videos/file2.mp4".to_string(),
        ];
        let result = RcloneProcessor::find_common_base_dir(&paths);
        assert_eq!(result, Some(PathBuf::from("/home/user/videos")));
    }

    #[test]
    fn test_find_common_base_dir_nested() {
        let paths = vec![
            "/home/user/videos/2024/file1.mp4".to_string(),
            "/home/user/videos/2023/file2.mp4".to_string(),
        ];
        let result = RcloneProcessor::find_common_base_dir(&paths);
        assert_eq!(result, Some(PathBuf::from("/home/user/videos")));
    }

    #[test]
    fn test_find_common_base_dir_no_common() {
        let paths = vec![
            "/home/user1/file1.mp4".to_string(),
            "/var/data/file2.mp4".to_string(),
        ];
        let result = RcloneProcessor::find_common_base_dir(&paths);
        // Should return "/" as the common base on Unix
        assert!(result.is_some());
    }

    #[test]
    fn test_find_common_base_dir_empty() {
        let paths: Vec<String> = vec![];
        let result = RcloneProcessor::find_common_base_dir(&paths);
        assert_eq!(result, None);
    }

    #[test]
    fn test_supports_batch_input() {
        let processor = RcloneProcessor::new();
        assert!(processor.supports_batch_input());
    }

    /// Connection-string parameters hold access keys and passwords; logs show
    /// the remote name and path only.
    #[test]
    fn loggable_destination_elides_connection_string_parameters() {
        assert_eq!(
            RcloneProcessor::loggable_destination(
                ":s3,access_key_id=AKIA,secret_access_key=hunter2:bucket/records/a.mp4"
            ),
            ":s3,…:bucket/records/a.mp4"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination("drive,pass=s3cret,token=abc:/videos"),
            "drive,…:/videos"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination("gdrive:/videos/{streamer}"),
            "gdrive:/videos/{streamer}"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination("remote:/a,b/c.mp4"),
            "remote:/a,b/c.mp4"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination(r"C:\records\a.mp4"),
            r"C:\records\a.mp4"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination("/local/records"),
            "/local/records"
        );
        // A local path is never read as a connection string.
        assert_eq!(
            RcloneProcessor::loggable_destination("/local/a,b/c:d.mp4"),
            "/local/a,b/c:d.mp4"
        );
        // Secrets holding `/` or quoted `:` are still elided.
        assert_eq!(
            RcloneProcessor::loggable_destination(":s3,secret_access_key=a/b:bucket/a.mp4"),
            ":s3,…:bucket/a.mp4"
        );
        assert_eq!(
            RcloneProcessor::loggable_destination("drive,pass='x:y':videos"),
            "drive,…:videos"
        );
    }

    #[test]
    fn test_determine_remote_destination_with_metadata() {
        use chrono::TimeZone;

        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            streamer_id: "123".to_string(),
            session_id: "456".to_string(),
            streamer_name: Some("StreamerName".to_string()),
            session_title: Some("Live Title".to_string()),
            platform: None,
            session_start: None,
            config: Some(r#"{"destination_root": "remote:/{streamer}/{title}/"}"#.to_string()),
            created_at: chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            manifest: None,
        };

        let config: RcloneConfig = serde_json::from_str(input.config.as_ref().unwrap()).unwrap();
        let destination = RcloneProcessor::determine_remote_destination(&input, &config);

        assert_eq!(destination, "remote:/StreamerName/Live Title");
    }

    #[test]
    fn test_determine_remote_destination_with_created_at() {
        use chrono::TimeZone;

        // Use a specific created_at timestamp: 2024-01-01 00:00:00 UTC
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            streamer_id: "123".to_string(),
            session_id: "456".to_string(),
            streamer_name: Some("StreamerName".to_string()),
            session_title: Some("Live Title".to_string()),
            platform: None,
            session_start: None,
            config: Some(r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/"}"#.to_string()),
            created_at: chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            manifest: None,
        };

        let config: RcloneConfig = serde_json::from_str(input.config.as_ref().unwrap()).unwrap();
        let destination = RcloneProcessor::determine_remote_destination(&input, &config);

        assert_eq!(destination, expected_local_destination(input.created_at));
    }

    #[test]
    fn test_determine_remote_destination_with_session_start_anchor() {
        let session_start = utc_datetime(2024, 1, 1, 12, 0, 0);
        let created_at = utc_datetime(2024, 1, 2, 12, 0, 0);
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            streamer_id: "123".to_string(),
            session_id: "456".to_string(),
            streamer_name: Some("StreamerName".to_string()),
            session_title: Some("Live Title".to_string()),
            platform: None,
            session_start: Some(session_start),
            config: Some(
                r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/", "time_anchor": "session_start"}"#
                    .to_string(),
            ),
            created_at,
            manifest: None,
        };

        let config: RcloneConfig = serde_json::from_str(input.config.as_ref().unwrap()).unwrap();
        let destination = RcloneProcessor::determine_remote_destination(&input, &config);

        assert_eq!(destination, expected_local_destination(session_start));
    }

    #[test]
    fn test_determine_remote_destination_session_start_falls_back_to_created_at() {
        let created_at = utc_datetime(2024, 1, 2, 12, 0, 0);
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            streamer_id: "123".to_string(),
            session_id: "456".to_string(),
            streamer_name: Some("StreamerName".to_string()),
            session_title: Some("Live Title".to_string()),
            platform: None,
            session_start: None,
            config: Some(
                r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/", "time_anchor": "session_start"}"#
                    .to_string(),
            ),
            created_at,
            manifest: None,
        };

        let config: RcloneConfig = serde_json::from_str(input.config.as_ref().unwrap()).unwrap();
        let destination = RcloneProcessor::determine_remote_destination(&input, &config);

        assert_eq!(destination, expected_local_destination(created_at));
    }

    #[test]
    fn test_session_start_anchor_groups_jobs_that_cross_dates() {
        let session_start = utc_datetime(2024, 1, 1, 12, 0, 0);
        let first_created_at = utc_datetime(2024, 1, 2, 12, 0, 0);
        let second_created_at = utc_datetime(2024, 1, 3, 12, 0, 0);
        let base_input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec![],
            streamer_id: "123".to_string(),
            session_id: "456".to_string(),
            streamer_name: Some("StreamerName".to_string()),
            session_title: Some("Live Title".to_string()),
            platform: None,
            session_start: Some(session_start),
            config: None,
            created_at: first_created_at,
            manifest: None,
        };
        let session_config: RcloneConfig = serde_json::from_str(
            r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/", "time_anchor": "session_start"}"#,
        )
        .unwrap();
        let job_config: RcloneConfig =
            serde_json::from_str(r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/"}"#)
                .unwrap();
        let later_input = ProcessorInput {
            created_at: second_created_at,
            ..base_input.clone()
        };

        let first_session_destination =
            RcloneProcessor::determine_remote_destination(&base_input, &session_config);
        let second_session_destination =
            RcloneProcessor::determine_remote_destination(&later_input, &session_config);
        assert_eq!(
            first_session_destination,
            expected_local_destination(session_start)
        );
        assert_eq!(second_session_destination, first_session_destination);

        let first_job_destination =
            RcloneProcessor::determine_remote_destination(&base_input, &job_config);
        let second_job_destination =
            RcloneProcessor::determine_remote_destination(&later_input, &job_config);
        assert_eq!(
            first_job_destination,
            expected_local_destination(first_created_at)
        );
        assert_eq!(
            second_job_destination,
            expected_local_destination(second_created_at)
        );
    }

    #[test]
    fn throughput_args_empty_when_no_fields() {
        let cfg = RcloneConfig::default();
        assert!(cfg.throughput_args().is_empty());
    }

    #[test]
    fn throughput_args_emits_bwlimit_and_transfers() {
        let cfg = RcloneConfig {
            bwlimit: Some("10M".into()),
            transfers: Some(8),
            ..Default::default()
        };
        let expected: Vec<String> = ["--bwlimit", "10M", "--transfers", "8"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(cfg.throughput_args(), expected);
    }

    #[test]
    fn throughput_args_supports_asymmetric_and_timetable() {
        let cfg = RcloneConfig {
            bwlimit: Some("10M:100k".into()),
            bwlimit_file: Some("08:00,512k 23:00,off".into()),
            ..Default::default()
        };
        let args = cfg.throughput_args();
        assert_eq!(&args[..2], &["--bwlimit", "10M:100k"]);
        assert_eq!(&args[2..], &["--bwlimit-file", "08:00,512k 23:00,off"]);
    }

    #[test]
    fn throughput_args_skips_empty_strings() {
        // Form submissions can produce `Some("")` for cleared text inputs;
        // those should not turn into empty CLI values.
        let cfg = RcloneConfig {
            bwlimit: Some(String::new()),
            multi_thread_cutoff: Some(String::new()),
            ..Default::default()
        };
        assert!(cfg.throughput_args().is_empty());
    }

    #[test]
    fn rclone_config_deserializes_from_partial_json() {
        let json = r#"{ "destination_root": "remote:/x", "bwlimit": "5M" }"#;
        let cfg: RcloneConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.destination_root.as_deref(), Some("remote:/x"));
        assert_eq!(cfg.bwlimit.as_deref(), Some("5M"));
        assert!(cfg.args.is_empty());
        assert_eq!(cfg.operation, RcloneOperation::Copy);
        assert_eq!(cfg.time_anchor, TimeAnchor::JobCreated);
    }

    #[test]
    fn rclone_config_deserializes_time_anchor() {
        let cfg: RcloneConfig =
            serde_json::from_str(r#"{"time_anchor": "session_start"}"#).unwrap();
        assert_eq!(cfg.time_anchor, TimeAnchor::SessionStart);
    }

    #[test]
    fn rclone_config_distinguishes_zero_tpslimit_from_unset() {
        let zero: RcloneConfig = serde_json::from_str(r#"{"tpslimit": 0}"#).unwrap();
        let unset: RcloneConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(zero.tpslimit, Some(0.0));
        assert_eq!(unset.tpslimit, None);
        assert_eq!(zero.throughput_args(), vec!["--tpslimit", "0"]);
        assert!(unset.throughput_args().is_empty());
    }

    #[test]
    fn rclone_config_round_trips_all_throughput_fields() {
        let json = r#"{
            "bwlimit": "10M:100k",
            "bwlimit_file": "1M",
            "transfers": 4,
            "checkers": 8,
            "tpslimit": 2.5,
            "tpslimit_burst": 5,
            "multi_thread_streams": 2,
            "multi_thread_cutoff": "250M"
        }"#;
        let cfg: RcloneConfig = serde_json::from_str(json).unwrap();
        let expected: Vec<String> = [
            "--bwlimit",
            "10M:100k",
            "--bwlimit-file",
            "1M",
            "--transfers",
            "4",
            "--checkers",
            "8",
            "--tpslimit",
            "2.5",
            "--tpslimit-burst",
            "5",
            "--multi-thread-streams",
            "2",
            "--multi-thread-cutoff",
            "250M",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(cfg.throughput_args(), expected);
    }
}
