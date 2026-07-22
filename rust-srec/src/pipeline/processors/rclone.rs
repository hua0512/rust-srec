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

/// Processor for interacting with Rclone.
pub struct RcloneProcessor {
    /// Path to rclone binary.
    rclone_path: String,
    /// Maximum retry attempts.
    max_retries: u32,
    command_runner: Arc<dyn RcloneCommandRunner>,
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

    // TempPath removes the manifest when a timeout or cancellation drops this future.
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

    fn partition_move_inputs(inputs: &[String]) -> (Vec<String>, Vec<String>) {
        inputs
            .iter()
            .cloned()
            .partition(|input| Path::new(input).exists())
    }

    fn take_moved_inputs(pending_inputs: &mut Vec<String>) -> Vec<String> {
        let mut moved_inputs = Vec::new();
        pending_inputs.retain(|input| {
            if Path::new(input).exists() {
                true
            } else {
                moved_inputs.push(input.clone());
                false
            }
        });
        moved_inputs
    }

    async fn input_size(inputs: &[String]) -> u64 {
        let mut total = 0u64;
        for input in inputs {
            if let Ok(metadata) = tokio::fs::metadata(input).await {
                total = total.saturating_add(metadata.len());
            }
        }
        total
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
            logs,
        };

        for attempt in 0..self.max_retries {
            if matches!(*operation, RcloneOperation::Move) && !Path::new(input_path).exists() {
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

            cmd.args([
                "--log-level",
                "ERROR",
                "--stats",
                "1s",
                "--stats-one-line",
                "--stats-one-line-date",
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
                        && !Path::new(input_path).exists()
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

                if matches!(*operation, RcloneOperation::Move) && !Path::new(input_path).exists() {
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
                Self::partition_move_inputs(inputs)
            } else {
                (inputs.to_vec(), Vec::new())
            };
        let resumed_inputs = already_moved_inputs.len();
        let mut completed_inputs = resumed_inputs;
        let total_input_size = Self::input_size(&pending_inputs).await;

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

            cmd.args([
                "--log-level",
                "ERROR",
                "--stats",
                "1s",
                "--stats-one-line",
                "--stats-one-line-date",
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
                let moved_inputs = Self::take_moved_inputs(&mut pending_inputs);
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

        let reference_timestamp_ms = config.time_anchor.reference_time(input).timestamp_millis();

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
            template = %remote_destination_raw,
            expanded = %expanded,
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
                if !Path::new(input_path).exists() {
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

            self.process_single(
                input_path,
                &RcloneExecution {
                    remote_destination: &full_destination,
                    operation: config.operation,
                    config_path: config.config_path.as_deref(),
                    throughput: &throughput,
                    extra_args: &config.args,
                    context: ctx,
                },
            )
            .await
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
                    extra_args: &config.args,
                    context: ctx,
                },
            )
            .await
        }
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

    struct MockRcloneCommandRunner {
        attempts: Mutex<VecDeque<MockAttempt>>,
        manifests: Mutex<Vec<Vec<String>>>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    impl MockRcloneCommandRunner {
        fn new(attempts: Vec<MockAttempt>) -> Self {
            Self {
                attempts: Mutex::new(attempts.into()),
                manifests: Mutex::new(Vec::new()),
                commands: Mutex::new(Vec::new()),
            }
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
            let manifest_index = args
                .iter()
                .position(|arg| arg == "--files-from")
                .ok_or_else(|| crate::Error::Other("missing --files-from argument".to_string()))?;
            let manifest_path = PathBuf::from(
                args.get(manifest_index + 1)
                    .ok_or_else(|| crate::Error::Other("missing files-from path".to_string()))?,
            );
            let manifest = tokio::fs::read_to_string(&manifest_path)
                .await
                .map_err(|e| {
                    crate::Error::io_path("reading test rclone manifest", &manifest_path, e)
                })?
                .lines()
                .map(str::to_string)
                .collect();

            self.manifests.lock().unwrap().push(manifest);
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
