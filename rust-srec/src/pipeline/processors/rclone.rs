//! Rclone processor for cloud storage operations.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tracing::{error, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
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
}

impl RcloneProcessor {
    /// Create a new rclone processor.
    pub fn new() -> Self {
        Self {
            rclone_path: std::env::var("RCLONE_PATH").unwrap_or_else(|_| "rclone".to_string()),
            max_retries: 3,
        }
    }

    /// Create with a custom rclone path.
    pub fn with_rclone_path(path: impl Into<String>) -> Self {
        Self {
            rclone_path: path.into(),
            max_retries: 3,
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

    /// Create a temporary file containing relative paths for --files-from.
    /// Creates the file in the base directory with a UUID-based name.
    /// Returns the temp file path and relative paths, or error.
    async fn create_files_from_list(
        inputs: &[String],
        base_dir: &Path,
    ) -> std::io::Result<(PathBuf, Vec<String>)> {
        // Create temp file in base directory
        let temp_filename = format!(".rclone_files_{}.txt", uuid::Uuid::new_v4());
        let temp_path = base_dir.join(&temp_filename);

        let mut file = tokio::fs::File::create(&temp_path).await?;
        let mut relative_paths = Vec::with_capacity(inputs.len());

        for input in inputs {
            let input_path = Path::new(input);
            let relative = input_path
                .strip_prefix(base_dir)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let relative_str = relative.to_string_lossy();
            file.write_all(relative_str.as_bytes()).await?;
            file.write_all(b"\n").await?;
            relative_paths.push(relative_str.to_string());
        }

        file.flush().await?;
        Ok((temp_path, relative_paths))
    }

    /// Clean up the temporary files-from list file.
    async fn cleanup_files_from_list(path: &Path) {
        if let Err(e) = tokio::fs::remove_file(path).await {
            warn!(
                "Failed to clean up temp files-from list {}: {}",
                path.display(),
                e
            );
        }
    }

    /// Execute a single-file rclone operation.
    #[allow(clippy::too_many_arguments)]
    async fn process_single(
        &self,
        input_path: &str,
        remote_destination: &str,
        operation: RcloneOperation,
        config_path: Option<&str>,
        throughput: &[String],
        extra_args: &[String],
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Use 'copyto' and 'moveto' for single-file operations.
        // Unlike 'copy' and 'move', these commands are designed for file-to-file transfer
        // and won't create a directory with the destination filename.
        let cmd_op = match operation {
            RcloneOperation::Copy => "copyto",
            RcloneOperation::Move => "moveto",
            RcloneOperation::Sync => unreachable!(),
        };

        info!(
            "Rclone {}: {} -> {}",
            cmd_op, input_path, remote_destination
        );

        let mut last_error = None;

        for attempt in 0..self.max_retries {
            if attempt > 0 {
                info!("Retry attempt {} for rclone {}", attempt + 1, cmd_op);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            }

            let mut cmd = Command::new(&self.rclone_path);

            if let Some(cfg) = config_path {
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
            cmd.args(throughput);
            for arg in extra_args {
                cmd.arg(arg);
            }

            let command_output = match crate::pipeline::processors::utils::run_rclone_with_progress(
                &mut cmd,
                &ctx.progress,
                Some(ctx.log_sink.clone()),
            )
            .await
            {
                Ok(output) => output,
                Err(e) => {
                    last_error = Some(format!("Failed to execute rclone: {}", e));
                    continue;
                }
            };

            if command_output.status.success() {
                let duration = start.elapsed().as_secs_f64();
                info!("Rclone {} completed in {:.2}s", cmd_op, duration);

                let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

                let outputs = match operation {
                    // Move operation does not produce any outputs
                    RcloneOperation::Move => vec![],
                    _ => vec![input_path.to_string()],
                };

                return Ok(ProcessorOutput {
                    outputs,
                    duration_secs: duration,
                    metadata: None,
                    items_produced: vec![],
                    input_size_bytes,
                    output_size_bytes: None,
                    failed_inputs: vec![],
                    succeeded_inputs: vec![input_path.to_string()],
                    skipped_inputs: vec![],
                    logs: command_output.logs,
                });
            } else {
                let error_msg = command_output
                    .logs
                    .iter()
                    .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                    .map(|l| l.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());

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
    #[allow(clippy::too_many_arguments)]
    async fn process_batch(
        &self,
        inputs: &[String],
        remote_destination: &str,
        operation: RcloneOperation,
        config_path: Option<&str>,
        throughput: &[String],
        extra_args: &[String],
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        let cmd_op = match operation {
            RcloneOperation::Copy => "copy",
            RcloneOperation::Move => "move",
            RcloneOperation::Sync => "sync",
        };

        // Find common base directory
        let base_dir = Self::find_common_base_dir(inputs).ok_or_else(|| {
            crate::Error::Validation(
                "Could not determine common base directory for batch upload".to_string(),
            )
        })?;

        // Create temp file with file list
        let (files_from_path, _relative_paths) = Self::create_files_from_list(inputs, &base_dir)
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to create files-from list: {}", e)))?;

        let files_from_path_str = files_from_path.to_string_lossy().to_string();
        let base_dir_str = base_dir.to_string_lossy().to_string();

        info!(
            "Rclone {} batch: {} files from {} -> {}",
            cmd_op,
            inputs.len(),
            base_dir_str,
            remote_destination
        );

        let mut last_error = None;

        for attempt in 0..self.max_retries {
            if attempt > 0 {
                info!("Retry attempt {} for rclone {} batch", attempt + 1, cmd_op);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            }

            let mut cmd = Command::new(&self.rclone_path);

            if let Some(cfg) = config_path {
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
            cmd.args(throughput);
            for arg in extra_args {
                cmd.arg(arg);
            }

            let command_output = match crate::pipeline::processors::utils::run_rclone_with_progress(
                &mut cmd,
                &ctx.progress,
                Some(ctx.log_sink.clone()),
            )
            .await
            {
                Ok(output) => output,
                Err(e) => {
                    last_error = Some(format!("Failed to execute rclone: {}", e));
                    continue;
                }
            };

            if command_output.status.success() {
                let duration = start.elapsed().as_secs_f64();
                info!(
                    "Rclone {} batch completed in {:.2}s ({} files)",
                    cmd_op,
                    duration,
                    inputs.len()
                );

                // Calculate total input size
                let mut total_input_size: u64 = 0;
                for input in inputs {
                    if let Ok(meta) = tokio::fs::metadata(input).await {
                        total_input_size += meta.len();
                    }
                }

                // Determine outputs based on operation
                let outputs = match operation {
                    RcloneOperation::Move => vec![], // Files consumed
                    _ => inputs.to_vec(),            // Copy/Sync: pass through original inputs
                };

                // Clean up temp file
                Self::cleanup_files_from_list(&files_from_path).await;

                return Ok(ProcessorOutput {
                    outputs,
                    duration_secs: duration,
                    metadata: Some(
                        serde_json::json!({
                            "batch_size": inputs.len(),
                            "base_dir": base_dir_str,
                            "operation": cmd_op,
                        })
                        .to_string(),
                    ),
                    items_produced: vec![],
                    input_size_bytes: Some(total_input_size),
                    output_size_bytes: None,
                    failed_inputs: vec![],
                    succeeded_inputs: inputs.to_vec(),
                    skipped_inputs: vec![],
                    logs: command_output.logs,
                });
            } else {
                let error_msg = command_output
                    .logs
                    .iter()
                    .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                    .map(|l| l.message.clone())
                    .unwrap_or_else(|| "Unknown error".to_string());

                last_error = Some(format!(
                    "rclone batch failed with exit code {}: {}",
                    command_output.status.code().unwrap_or(-1),
                    error_msg
                ));
            }
        }

        // Clean up temp file on failure
        Self::cleanup_files_from_list(&files_from_path).await;

        error!(
            "Rclone {} batch failed after {} attempts",
            cmd_op, self.max_retries
        );
        Err(crate::Error::Other(
            last_error.unwrap_or_else(|| "Rclone batch failed".to_string()),
        ))
    }

    /// Determine remote destination path with placeholder expansion.
    /// Supports: {streamer}, {title}, {streamer_id}, {session_id}, and time placeholders (%Y, %m, %d, etc.)
    ///
    /// Time placeholders use `input.created_at` to ensure consistency across retries.
    fn determine_remote_destination(input: &ProcessorInput, config: &RcloneConfig) -> String {
        // For batch mode, we need a destination root (directory), not a specific file path
        let remote_destination_raw = if let Some(out) = input.outputs.first() {
            out.clone()
        } else if let Some(root) = config.destination_root.as_deref() {
            root.trim_end_matches('/').to_string()
        } else {
            String::new()
        };

        // Use job's created_at timestamp for time placeholder expansion.
        // This ensures retries use the same timestamp as the original job.
        let reference_timestamp_ms = input.created_at.timestamp_millis();

        // Debug: Log all placeholder-related values before expansion
        tracing::debug!(
            template = %remote_destination_raw,
            streamer_id = %input.streamer_id,
            session_id = %input.session_id,
            streamer_name = ?input.streamer_name,
            session_title = ?input.session_title,
            created_at = %input.created_at,
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

        // Validate all input files exist
        for input_path in &input.inputs {
            if !Path::new(input_path).exists() {
                return Err(crate::Error::Validation(format!(
                    "Input file does not exist: {}",
                    input_path
                )));
            }
        }

        // Parse config into the typed struct.
        let config: RcloneConfig = match input.config.as_deref() {
            Some(s) => serde_json::from_str(s).map_err(|e| {
                crate::Error::Validation(format!("Invalid rclone config JSON: {e}"))
            })?,
            None => RcloneConfig::default(),
        };

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
                &full_destination,
                config.operation,
                config.config_path.as_deref(),
                &throughput,
                &config.args,
                ctx,
            )
            .await
        } else {
            // Batch mode - use --files-from
            info!("Using batch mode for {} files", input.inputs.len());

            self.process_batch(
                &input.inputs,
                &remote_destination,
                config.operation,
                config.config_path.as_deref(),
                &throughput,
                &config.args,
                ctx,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            config: Some(r#"{"destination_root": "remote:/%Y/%m/%d/{streamer}/"}"#.to_string()),
            created_at: chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        };

        let config: RcloneConfig = serde_json::from_str(input.config.as_ref().unwrap()).unwrap();
        let destination = RcloneProcessor::determine_remote_destination(&input, &config);

        // Should use created_at (2024-01-01) for time placeholders
        assert_eq!(destination, "remote:/2024/01/01/StreamerName");
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
