//! Rclone processor for cloud storage operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
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

/// How the processor derives a public HTTP URL for each uploaded file.
///
/// The derived URL is stored on [`UploadResultItem::public_url`] and
/// persisted with the upload record, so the web UI can keep serving
/// previews from the remote copy after the local file is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PublicUrlMode {
    /// No public URL is derived (default).
    #[default]
    None,
    /// Join [`RcloneConfig::public_url_base`] with the same
    /// destination-relative path used for the remote transfer, mirroring
    /// the remote layout. Suited to path-preserving HTTP storage
    /// (S3-compatible buckets, CDNs, WebDAV) fronting the remote.
    BaseMapping,
    /// Run `rclone link <remote>` after the transfer to obtain a share
    /// link. Suited to ID-addressed drives (Google Drive, OneDrive, ...)
    /// where no path-based URL exists.
    RcloneLink,
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

    /// How to derive a public HTTP URL for each uploaded file. Defaults
    /// to [`PublicUrlMode::None`].
    pub public_url_mode: PublicUrlMode,

    /// HTTP base URL for [`PublicUrlMode::BaseMapping`]. Supports the
    /// same placeholder expansion as `destination_root`; each file's
    /// destination-relative path is appended, so this should point at an
    /// HTTP frontend of the location `destination_root` resolves to
    /// (e.g. `https://cdn.example.com/{streamer}` mirroring
    /// `remote:bucket/{streamer}`).
    pub public_url_base: Option<String>,

    /// `--expire` value passed to `rclone link` in
    /// [`PublicUrlMode::RcloneLink`] (e.g. `"1w"`). Unset uses the
    /// backend default. Not every backend honors expiry.
    pub link_expire: Option<String>,

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

/// Processor for interacting with Rclone.
pub struct RcloneProcessor {
    /// Path to rclone binary.
    rclone_path: String,
    /// Maximum retry attempts.
    max_retries: u32,
    /// Runs assembled rclone commands; a seam so tests can observe and
    /// stub invocations without spawning a real process.
    command_runner: Arc<dyn RcloneCommandRunner>,
}

#[async_trait]
trait RcloneCommandRunner: Send + Sync {
    async fn run(&self, command: &mut Command, context: &ProcessorContext)
    -> Result<CommandOutput>;

    /// Run a short rclone command capturing raw stdout (`rclone link`
    /// prints the share link there, which the JSON-log stderr parsing in
    /// `run` never sees).
    async fn run_capturing_stdout(&self, command: &mut Command) -> Result<std::process::Output>;
}

struct ProcessRcloneCommandRunner;

/// Upper bound for one `rclone link` invocation. Links are metadata-only
/// calls; anything slower than this is treated as a hung child so the
/// upload job is not held open indefinitely.
const LINK_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Attempts per file for `rclone link`, with exponential backoff between
/// them, so transient network errors rarely surface as a missing URL.
const LINK_MAX_ATTEMPTS: u32 = 3;

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

    async fn run_capturing_stdout(&self, command: &mut Command) -> Result<std::process::Output> {
        use process_utils::NoWindowExt;

        command.no_window();
        // Timing out or cancelling the owning future must not leak the child.
        command.kill_on_drop(true);
        command.stdin(std::process::Stdio::null());

        match tokio::time::timeout(LINK_COMMAND_TIMEOUT, command.output()).await {
            Ok(result) => {
                result.map_err(|e| crate::Error::Other(format!("Failed to execute rclone: {e}")))
            }
            Err(_) => Err(crate::Error::Other(format!(
                "rclone link timed out after {}s",
                LINK_COMMAND_TIMEOUT.as_secs()
            ))),
        }
    }
}

struct RcloneExecution<'a> {
    remote_destination: &'a str,
    operation: RcloneOperation,
    config_path: Option<&'a str>,
    throughput: &'a [String],
    extra_args: &'a [String],
    /// Expanded [`RcloneConfig::public_url_base`], present only in
    /// [`PublicUrlMode::BaseMapping`] and already validated as an
    /// absolute URL by `process`.
    public_url_base: Option<&'a str>,
    context: &'a ProcessorContext,
}

impl RcloneProcessor {
    /// Create a new rclone processor.
    pub fn new() -> Self {
        Self {
            rclone_path: std::env::var("RCLONE_PATH").unwrap_or_else(|_| "rclone".to_string()),
            max_retries: 3,
            command_runner: Arc::new(ProcessRcloneCommandRunner),
        }
    }

    /// Create with a custom rclone path.
    pub fn with_rclone_path(path: impl Into<String>) -> Self {
        Self {
            rclone_path: path.into(),
            max_retries: 3,
            command_runner: Arc::new(ProcessRcloneCommandRunner),
        }
    }

    #[cfg(test)]
    fn with_command_runner(
        path: impl Into<String>,
        command_runner: Arc<dyn RcloneCommandRunner>,
    ) -> Self {
        Self {
            rclone_path: path.into(),
            max_retries: 3,
            command_runner,
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
    /// future before the explicit `close()` in `process_batch`.
    async fn create_files_from_list(
        inputs: &[String],
        base_dir: &Path,
    ) -> std::io::Result<TempPath> {
        let named_file = tempfile::Builder::new()
            .prefix(".rclone_files_")
            .suffix(".txt")
            .tempfile_in(base_dir)?;
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

    /// True only when the filesystem positively reports the path as absent.
    ///
    /// Source absence is what marks a move input as consumed, so an I/O
    /// error from `try_exists` (e.g. an unreadable or unmounted parent
    /// directory) must keep the input pending instead of reporting an
    /// upload that never happened.
    async fn is_confirmed_absent(path: &Path) -> bool {
        matches!(tokio::fs::try_exists(path).await, Ok(false))
    }

    /// Split move inputs into (pending, already moved by an earlier attempt).
    async fn partition_move_inputs(inputs: &[String]) -> (Vec<String>, Vec<String>) {
        let mut pending = Vec::new();
        let mut moved = Vec::new();
        for input in inputs {
            if Self::is_confirmed_absent(Path::new(input)).await {
                moved.push(input.clone());
            } else {
                pending.push(input.clone());
            }
        }
        (pending, moved)
    }

    /// Drain inputs whose sources rclone consumed during the last attempt.
    async fn take_moved_inputs(pending_inputs: &mut Vec<String>) -> Vec<String> {
        let mut moved_inputs = Vec::new();
        let mut still_pending = Vec::with_capacity(pending_inputs.len());
        for input in pending_inputs.drain(..) {
            if Self::is_confirmed_absent(Path::new(&input)).await {
                moved_inputs.push(input);
            } else {
                still_pending.push(input);
            }
        }
        *pending_inputs = still_pending;
        moved_inputs
    }

    /// Per-file sizes captured before the transfer so `UploadResultItem`
    /// sizes survive `move` operations that delete the local source.
    /// Unreadable inputs are simply absent from the map.
    async fn input_size_map(inputs: &[String]) -> std::collections::HashMap<String, u64> {
        let mut sizes = std::collections::HashMap::with_capacity(inputs.len());
        for input in inputs {
            if let Ok(metadata) = tokio::fs::metadata(input).await {
                sizes.insert(input.clone(), metadata.len());
            }
        }
        sizes
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

    /// Append `segments` to `base` as percent-encoded URL path segments.
    /// `None` when `base` is not an absolute URL that can carry a path.
    fn join_public_url<'a>(base: &str, segments: impl Iterator<Item = &'a str>) -> Option<String> {
        let mut url = url::Url::parse(base).ok()?;
        {
            let mut path = url.path_segments_mut().ok()?;
            path.pop_if_empty();
            for segment in segments.filter(|s| !s.is_empty()) {
                path.push(segment);
            }
        }
        Some(url.into())
    }

    /// Public URL of one batch input: `url_base` + the same base-relative
    /// path `batch_remote_path` appends to the remote root, so the URL
    /// layout mirrors the remote layout.
    fn batch_public_url(base_dir: &Path, url_base: &str, input: &str) -> Option<String> {
        let relative = Path::new(input).strip_prefix(base_dir).ok()?;
        let relative = relative.to_string_lossy().replace('\\', "/");
        Self::join_public_url(url_base, relative.split('/'))
    }

    /// File name a single-file transfer creates at the destination: the
    /// last segment of the `copyto`/`moveto` target. Falls back across
    /// `:` for root-of-remote destinations like `remote:file.mp4`.
    fn remote_file_name(remote_destination: &str) -> Option<&str> {
        let tail = remote_destination.rsplit('/').next()?;
        let tail = tail.rsplit(':').next()?;
        (!tail.is_empty()).then_some(tail)
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
            public_url_base,
            context,
        } = execution;
        let start = std::time::Instant::now();

        // Use 'copyto' and 'moveto' for single-file operations.
        // Unlike 'copy' and 'move', these commands are designed for file-to-file transfer
        // and won't create a directory with the destination filename.
        let cmd_op = match *operation {
            RcloneOperation::Copy => "copyto",
            RcloneOperation::Move => "moveto",
            RcloneOperation::Sync => unreachable!(),
        };

        info!(
            "Rclone {}: {} -> {}",
            cmd_op, input_path, remote_destination
        );

        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        // The single-file destination already carries the final remote file
        // name, so the mirrored URL is base + that last segment.
        let public_url = public_url_base.and_then(|base| {
            Self::remote_file_name(remote_destination)
                .and_then(|name| Self::join_public_url(base, std::iter::once(name)))
        });
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
                public_url: public_url.clone(),
                // Captured before the transfer, so it is present even after
                // a move deleted the source (None on move-resume retries).
                size_bytes: input_size_bytes,
                status: UploadItemStatus::Completed,
                error: None,
            }],
            logs,
        };

        for attempt in 0..self.max_retries {
            if matches!(*operation, RcloneOperation::Move)
                && Self::is_confirmed_absent(Path::new(input_path)).await
            {
                info!(
                    input = input_path,
                    "Rclone move source is already absent; treating it as successfully moved"
                );
                return Ok(success_output(logs));
            }

            if attempt > 0 {
                info!("Retry attempt {} for rclone {}", attempt + 1, cmd_op);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
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
                        && Self::is_confirmed_absent(Path::new(input_path)).await
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
                    .logs
                    .iter()
                    .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                    .map(|l| l.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());
                logs.extend(command_output.logs);

                if matches!(*operation, RcloneOperation::Move)
                    && Self::is_confirmed_absent(Path::new(input_path)).await
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

        error!(
            "Rclone {} failed after {} attempts",
            cmd_op, self.max_retries
        );
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
            public_url_base,
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
            if matches!(*operation, RcloneOperation::Move) {
                Self::partition_move_inputs(inputs).await
            } else {
                (inputs.to_vec(), Vec::new())
            };
        let resumed_inputs = already_moved_inputs.len();
        let mut completed_inputs = resumed_inputs;
        // Sizes are captured before rclone runs so move'd sources still have
        // one; inputs already consumed by an earlier attempt are absent and
        // fall back to None (the upsert's COALESCE keeps any earlier value).
        let file_sizes = Self::input_size_map(&pending_inputs).await;
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
                    public_url: public_url_base
                        .and_then(|base| Self::batch_public_url(&base_dir, base, input)),
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
            remote_destination
        );

        let mut last_error = None;
        let mut logs = Vec::new();

        for attempt in 0..self.max_retries {
            if attempt > 0 {
                info!("Retry attempt {} for rclone {} batch", attempt + 1, cmd_op);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
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
                        .logs
                        .iter()
                        .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                        .map(|l| l.message.clone())
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
            completed_inputs,
            attempts = self.max_retries,
            "Rclone {} batch failed",
            cmd_op
        );
        Err(crate::Error::Other(
            last_error.unwrap_or_else(|| "Rclone batch failed".to_string()),
        ))
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

        // Debug: Log all placeholder-related values before expansion
        tracing::debug!(
            template = %remote_destination_raw,
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
        let expanded = Self::expand_remote_template(&remote_destination_raw, input, config);

        tracing::debug!(
            template = %remote_destination_raw,
            expanded = %expanded,
            "Rclone: Placeholder expansion result"
        );

        expanded
    }

    /// Expand `{streamer}`/`{title}`/... and chrono time tokens in
    /// `template` using the job metadata and the configured
    /// [`TimeAnchor`]. Shared by `destination_root` and
    /// `public_url_base` so both expand identically and the URL layout
    /// can mirror the remote layout.
    fn expand_remote_template(
        template: &str,
        input: &ProcessorInput,
        config: &RcloneConfig,
    ) -> String {
        let reference_timestamp_ms = config.time_anchor.reference_time(input).timestamp_millis();
        expand_placeholders_at(
            template,
            &input.streamer_id,
            &input.session_id,
            input.streamer_name.as_deref(),
            input.session_title.as_deref(),
            input.platform.as_deref(),
            Some(reference_timestamp_ms),
        )
    }

    /// Resolve the expanded `public_url_base` for
    /// [`PublicUrlMode::BaseMapping`]; `None` when the mode is off.
    ///
    /// An empty or unusable base is a deterministic configuration error, so
    /// it fails validation here — before any transfer runs — rather than
    /// silently uploading (and, for `move`, deleting local sources) without
    /// recording the URLs the preset asked for.
    fn resolve_public_url_base(
        input: &ProcessorInput,
        config: &RcloneConfig,
    ) -> Result<Option<String>> {
        if !matches!(config.public_url_mode, PublicUrlMode::BaseMapping) {
            return Ok(None);
        }
        let Some(base) = config.public_url_base.as_deref().filter(|s| !s.is_empty()) else {
            return Err(crate::Error::Validation(
                "public_url_mode is base_mapping but public_url_base is empty".to_string(),
            ));
        };
        let expanded = Self::expand_remote_template(base, input, config);
        if Self::join_public_url(&expanded, std::iter::empty()).is_none() {
            return Err(crate::Error::Validation(format!(
                "public_url_base expands to '{expanded}', which is not a usable absolute URL"
            )));
        }
        Ok(Some(expanded))
    }

    /// Fill [`UploadResultItem::public_url`] by running `rclone link` for
    /// every completed upload, retrying each file a few times to absorb
    /// transient errors.
    ///
    /// A file whose link still fails does not fail the job: the transfer
    /// already succeeded, and some link failures are permanent (a backend
    /// without public-link support) so a FAILED status would never converge
    /// under retry while misrepresenting an upload that worked. Instead the
    /// failure is stored on [`UploadResultItem::error`], which
    /// `JobQueue::persist_upload_records` persists alongside the COMPLETED
    /// status for the UI to surface.
    async fn attach_public_links(
        &self,
        output: &mut ProcessorOutput,
        config: &RcloneConfig,
        ctx: &ProcessorContext,
    ) {
        let expire = config.link_expire.as_deref().filter(|s| !s.is_empty());
        let mut generated = 0usize;

        for item in output.uploads.iter_mut() {
            if !matches!(item.status, UploadItemStatus::Completed) {
                continue;
            }
            let Some(remote_path) = item.remote_path.as_deref() else {
                continue;
            };

            let mut last_error: Option<crate::Error> = None;
            for attempt in 0..LINK_MAX_ATTEMPTS {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt - 1))).await;
                }
                match self
                    .fetch_public_link(remote_path, config.config_path.as_deref(), expire)
                    .await
                {
                    Ok(link) => {
                        item.public_url = Some(link);
                        generated += 1;
                        last_error = None;
                        break;
                    }
                    Err(e) => {
                        warn!(
                            remote_path,
                            attempt = attempt + 1,
                            error = %e,
                            "rclone link failed"
                        );
                        last_error = Some(e);
                    }
                }
            }

            if let Some(e) = last_error {
                ctx.warn(format!("rclone link failed for {remote_path}: {e}"));
                item.error = Some(format!("rclone link failed: {e}"));
            }
        }

        if generated > 0 {
            ctx.info(format!(
                "Generated {generated} public link(s) via rclone link"
            ));
        }
    }

    /// Run `rclone link <remote_path>` and return the share link, which
    /// rclone prints as the last non-empty stdout line.
    async fn fetch_public_link(
        &self,
        remote_path: &str,
        config_path: Option<&str>,
        expire: Option<&str>,
    ) -> Result<String> {
        let mut cmd = Command::new(&self.rclone_path);
        if let Some(cfg) = config_path {
            cmd.arg("--config").arg(cfg);
        }
        cmd.arg("link");
        if let Some(expire) = expire {
            cmd.arg("--expire").arg(expire);
        }
        cmd.arg(remote_path);

        let output = self.command_runner.run_capturing_stdout(&mut cmd).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no error output")
                .trim()
                .to_string();
            return Err(crate::Error::Other(format!(
                "rclone link exited with code {}: {}",
                output.status.code().unwrap_or(-1),
                detail
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
            .ok_or_else(|| crate::Error::Other("rclone link produced no output".to_string()))
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
        // "upload" kept for backwards compatibility
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

        // A retried move may legitimately reference sources consumed by an earlier attempt.
        if !matches!(config.operation, RcloneOperation::Move) {
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

        let public_url_base = Self::resolve_public_url_base(input, &config)?;

        // Choose single or batch mode based on input count
        let mut output = if input.inputs.len() == 1 {
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

            self.process_single(
                input_path,
                &RcloneExecution {
                    remote_destination: &full_destination,
                    operation: config.operation,
                    config_path: config.config_path.as_deref(),
                    throughput: &throughput,
                    extra_args: &extra_args,
                    public_url_base: public_url_base.as_deref(),
                    context: ctx,
                },
            )
            .await?
        } else {
            // Batch mode - use --files-from
            info!("Using batch mode for {} files", input.inputs.len());

            self.process_batch(
                &input.inputs,
                &RcloneExecution {
                    remote_destination: &remote_destination,
                    operation: config.operation,
                    config_path: config.config_path.as_deref(),
                    throughput: &throughput,
                    extra_args: &extra_args,
                    public_url_base: public_url_base.as_deref(),
                    context: ctx,
                },
            )
            .await?
        };

        // Share links can only be minted once the files exist remotely, and
        // a link failure must not fail a transfer that already succeeded.
        if matches!(config.public_url_mode, PublicUrlMode::RcloneLink) {
            self.attach_public_links(&mut output, &config, ctx).await;
        }

        Ok(output)
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
        moved_inputs: Vec<PathBuf>,
        succeeds: bool,
    }

    struct MockLinkAttempt {
        succeeds: bool,
        stdout: String,
    }

    struct MockRcloneCommandRunner {
        attempts: Mutex<VecDeque<MockAttempt>>,
        link_attempts: Mutex<VecDeque<MockLinkAttempt>>,
        manifests: Mutex<Vec<Vec<String>>>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    impl MockRcloneCommandRunner {
        fn new(attempts: Vec<MockAttempt>) -> Self {
            Self {
                attempts: Mutex::new(attempts.into()),
                link_attempts: Mutex::new(VecDeque::new()),
                manifests: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
            }
        }

        fn with_link_attempts(self, link_attempts: Vec<MockLinkAttempt>) -> Self {
            *self.link_attempts.lock().unwrap() = link_attempts.into();
            self
        }

        fn manifests(&self) -> Vec<Vec<String>> {
            self.manifests.lock().unwrap().clone()
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().unwrap().clone()
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
            // Single-file commands (copyto/moveto) carry no --files-from;
            // only batch commands record a manifest.
            if let Some(manifest_index) = args.iter().position(|arg| arg == "--files-from") {
                let manifest_path =
                    PathBuf::from(args.get(manifest_index + 1).ok_or_else(|| {
                        crate::Error::Other("missing files-from path".to_string())
                    })?);
                let manifest = tokio::fs::read_to_string(&manifest_path)
                    .await
                    .map_err(|e| {
                        crate::Error::io_path("reading test rclone manifest", &manifest_path, e)
                    })?
                    .lines()
                    .map(str::to_string)
                    .collect();
                self.manifests.lock().unwrap().push(manifest);
            }
            self.commands.lock().unwrap().push(args);

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

        async fn run_capturing_stdout(
            &self,
            command: &mut Command,
        ) -> Result<std::process::Output> {
            let args: Vec<String> = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.commands.lock().unwrap().push(args);

            let attempt = self
                .link_attempts
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| crate::Error::Other("unexpected rclone link attempt".to_string()))?;

            Ok(std::process::Output {
                status: test_exit_status(attempt.succeeds),
                stdout: attempt.stdout.into_bytes(),
                stderr: if attempt.succeeds {
                    Vec::new()
                } else {
                    b"simulated link failure\n".to_vec()
                },
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

    fn expected_local_destination(dt: chrono::DateTime<chrono::Utc>) -> String {
        format!(
            "remote:/{}/StreamerName",
            pipeline_common::expand_path_template_at("%Y/%m/%d", Some(dt.timestamp_millis()))
        )
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
            .process(&input, &ProcessorContext::noop("resumed-batch-move"))
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
            .process(&input, &ProcessorContext::noop("resumed-single-move"))
            .await
            .unwrap();

        assert_eq!(output.succeeded_inputs, vec![input_path]);
        assert!(output.outputs.is_empty());
        assert!(runner.commands().is_empty());
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
            RcloneProcessor::partition_move_inputs(std::slice::from_ref(&unverifiable)).await;

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

        let error = RcloneProcessor::new()
            .process(&input, &ProcessorContext::noop("missing-copy"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("Input file does not exist"));
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
        assert_eq!(cfg.public_url_mode, PublicUrlMode::None);
        assert_eq!(cfg.public_url_base, None);
        assert_eq!(cfg.link_expire, None);
    }

    #[test]
    fn rclone_config_deserializes_public_url_fields() {
        let cfg: RcloneConfig = serde_json::from_str(
            r#"{
                "public_url_mode": "base_mapping",
                "public_url_base": "https://cdn.example.com/{streamer}"
            }"#,
        )
        .unwrap();
        assert_eq!(cfg.public_url_mode, PublicUrlMode::BaseMapping);
        assert_eq!(
            cfg.public_url_base.as_deref(),
            Some("https://cdn.example.com/{streamer}")
        );

        let cfg: RcloneConfig =
            serde_json::from_str(r#"{ "public_url_mode": "rclone_link", "link_expire": "1w" }"#)
                .unwrap();
        assert_eq!(cfg.public_url_mode, PublicUrlMode::RcloneLink);
        assert_eq!(cfg.link_expire.as_deref(), Some("1w"));
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

    #[test]
    fn join_public_url_encodes_segments_and_keeps_base_path() {
        assert_eq!(
            RcloneProcessor::join_public_url(
                "https://cdn.example.com/media/",
                ["2024", "my clip.mp4"].into_iter()
            ),
            Some("https://cdn.example.com/media/2024/my%20clip.mp4".to_string())
        );
        // Segments must not be able to smuggle a query or fragment.
        assert_eq!(
            RcloneProcessor::join_public_url(
                "https://cdn.example.com",
                std::iter::once("a#b?c.mp4")
            ),
            Some("https://cdn.example.com/a%23b%3Fc.mp4".to_string())
        );
        assert_eq!(
            RcloneProcessor::join_public_url("not a url", std::iter::once("a.mp4")),
            None
        );
    }

    #[test]
    fn batch_public_url_mirrors_batch_remote_path_layout() {
        let base = Path::new("/home/user/videos");
        assert_eq!(
            RcloneProcessor::batch_public_url(
                base,
                "https://cdn.example.com/media",
                "/home/user/videos/2024/b.mp4"
            ),
            Some("https://cdn.example.com/media/2024/b.mp4".to_string())
        );
        // Input outside the base dir cannot be mapped, same as the remote path.
        assert_eq!(
            RcloneProcessor::batch_public_url(base, "https://cdn.example.com", "/var/other/c.mp4"),
            None
        );
    }

    #[test]
    fn remote_file_name_takes_the_last_destination_segment() {
        assert_eq!(
            RcloneProcessor::remote_file_name("remote:bucket/dir/clip.mp4"),
            Some("clip.mp4")
        );
        assert_eq!(
            RcloneProcessor::remote_file_name("remote:clip.mp4"),
            Some("clip.mp4")
        );
        assert_eq!(
            RcloneProcessor::remote_file_name("remote:bucket/dir/"),
            None
        );
    }

    #[tokio::test]
    async fn single_copy_with_base_mapping_records_public_url() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("my clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            streamer_name: Some("StreamerName".to_string()),
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records/{streamer}",
                    "public_url_mode": "base_mapping",
                    "public_url_base": "https://cdn.example.com/{streamer}"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("single-base-mapping"))
            .await
            .unwrap();

        assert_eq!(output.uploads.len(), 1);
        assert_eq!(
            output.uploads[0].remote_path.as_deref(),
            Some("remote:records/StreamerName/my clip.mp4")
        );
        assert_eq!(
            output.uploads[0].public_url.as_deref(),
            Some("https://cdn.example.com/StreamerName/my%20clip.mp4")
        );
    }

    #[tokio::test]
    async fn batch_copy_with_base_mapping_records_public_urls() {
        let temp_dir = tempfile::tempdir().unwrap();
        let flat = temp_dir.path().join("a.mp4");
        let nested_dir = temp_dir.path().join("sub");
        tokio::fs::create_dir(&nested_dir).await.unwrap();
        let nested = nested_dir.join("c.mp4");
        tokio::fs::write(&flat, b"a").await.unwrap();
        tokio::fs::write(&nested, b"c").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(vec![MockAttempt {
            moved_inputs: vec![],
            succeeds: true,
        }]));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![
                flat.to_string_lossy().into_owned(),
                nested.to_string_lossy().into_owned(),
            ],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records",
                    "public_url_mode": "base_mapping",
                    "public_url_base": "https://cdn.example.com/media"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("batch-base-mapping"))
            .await
            .unwrap();

        let urls: Vec<Option<&str>> = output
            .uploads
            .iter()
            .map(|item| item.public_url.as_deref())
            .collect();
        assert_eq!(
            urls,
            vec![
                Some("https://cdn.example.com/media/a.mp4"),
                Some("https://cdn.example.com/media/sub/c.mp4"),
            ]
        );
    }

    /// An unusable base is a deterministic config error: the job must fail
    /// during validation, before rclone could move (and delete) any source.
    #[tokio::test]
    async fn base_mapping_with_unusable_base_fails_before_transfer() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "move",
                    "destination_root": "remote:records",
                    "public_url_mode": "base_mapping",
                    "public_url_base": "not a url"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let error = processor
            .process(&input, &ProcessorContext::noop("bad-base"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("public_url_base"));
        assert!(runner.commands().is_empty(), "no rclone command may run");
        assert!(file.exists(), "the local source must be untouched");
    }

    #[tokio::test]
    async fn base_mapping_with_empty_base_fails_validation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(MockRcloneCommandRunner::new(Vec::new()));
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records",
                    "public_url_mode": "base_mapping",
                    "public_url_base": ""
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let error = processor
            .process(&input, &ProcessorContext::noop("empty-base"))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("public_url_base is empty"));
        assert!(runner.commands().is_empty());
    }

    #[tokio::test]
    async fn rclone_link_mode_attaches_share_links() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(
            MockRcloneCommandRunner::new(vec![MockAttempt {
                moved_inputs: vec![],
                succeeds: true,
            }])
            .with_link_attempts(vec![MockLinkAttempt {
                succeeds: true,
                stdout: "NOTICE: some banner\nhttps://drive.example.com/share/abc\n".to_string(),
            }]),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records",
                    "public_url_mode": "rclone_link",
                    "link_expire": "1w"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("link-mode"))
            .await
            .unwrap();

        assert_eq!(
            output.uploads[0].public_url.as_deref(),
            Some("https://drive.example.com/share/abc")
        );
        // The transfer command is recorded first, then the link command.
        let commands = runner.commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[1],
            vec![
                "link".to_string(),
                "--expire".to_string(),
                "1w".to_string(),
                "remote:records/clip.mp4".to_string(),
            ]
        );
    }

    /// Exhausted link attempts must not fail a job whose transfer already
    /// succeeded (some link failures are permanent, e.g. a backend without
    /// public-link support); the failure is recorded on the item's `error`
    /// so the persisted COMPLETED record carries it.
    #[tokio::test(start_paused = true)]
    async fn rclone_link_failure_leaves_public_url_unset_but_job_succeeds() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let failed_attempts = (0..LINK_MAX_ATTEMPTS)
            .map(|_| MockLinkAttempt {
                succeeds: false,
                stdout: String::new(),
            })
            .collect();
        let runner = Arc::new(
            MockRcloneCommandRunner::new(vec![MockAttempt {
                moved_inputs: vec![],
                succeeds: true,
            }])
            .with_link_attempts(failed_attempts),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records",
                    "public_url_mode": "rclone_link"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("link-mode-failure"))
            .await
            .unwrap();

        assert_eq!(output.uploads[0].status, UploadItemStatus::Completed);
        assert_eq!(output.uploads[0].public_url, None);
        let error = output.uploads[0].error.as_deref().unwrap();
        assert!(error.contains("rclone link failed"), "got: {error}");
        // One transfer command plus one link command per attempt.
        assert_eq!(runner.commands().len(), 1 + LINK_MAX_ATTEMPTS as usize);
    }

    #[tokio::test(start_paused = true)]
    async fn rclone_link_retries_transient_failures() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file = temp_dir.path().join("clip.mp4");
        tokio::fs::write(&file, b"video").await.unwrap();

        let runner = Arc::new(
            MockRcloneCommandRunner::new(vec![MockAttempt {
                moved_inputs: vec![],
                succeeds: true,
            }])
            .with_link_attempts(vec![
                MockLinkAttempt {
                    succeeds: false,
                    stdout: String::new(),
                },
                MockLinkAttempt {
                    succeeds: true,
                    stdout: "https://drive.example.com/share/retry\n".to_string(),
                },
            ]),
        );
        let processor = RcloneProcessor::with_command_runner("rclone", runner.clone());
        let input = ProcessorInput {
            inputs: vec![file.to_string_lossy().into_owned()],
            outputs: vec![],
            config: Some(
                r#"{
                    "operation": "copy",
                    "destination_root": "remote:records",
                    "public_url_mode": "rclone_link"
                }"#
                .to_string(),
            ),
            ..Default::default()
        };

        let output = processor
            .process(&input, &ProcessorContext::noop("link-retry"))
            .await
            .unwrap();

        assert_eq!(
            output.uploads[0].public_url.as_deref(),
            Some("https://drive.example.com/share/retry")
        );
        assert_eq!(output.uploads[0].error, None);
    }
}
