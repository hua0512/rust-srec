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
    async fn process_single(
        &self,
        input_path: &str,
        remote_destination: &str,
        operation: RcloneOperation,
        config_path: Option<&str>,
        extra_args: Option<&[String]>,
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

            if let Some(args) = extra_args {
                for arg in args {
                    cmd.arg(arg);
                }
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
    async fn process_batch(
        &self,
        inputs: &[String],
        remote_destination: &str,
        operation: RcloneOperation,
        config_path: Option<&str>,
        extra_args: Option<&[String]>,
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

            if let Some(args) = extra_args {
                for arg in args {
                    cmd.arg(arg);
                }
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
    fn determine_remote_destination(
        input: &ProcessorInput,
        config_json: Option<&serde_json::Value>,
    ) -> String {
        // Determine destination root
        let destination_root = config_json
            .and_then(|v| v.get("destination_root"))
            .and_then(|s| s.as_str());

        // For batch mode, we need a destination root (directory), not a specific file path
        let remote_destination_raw = if let Some(out) = input.outputs.first() {
            out.clone()
        } else if let Some(root) = destination_root {
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

        // Parse config
        let config_json = input
            .config
            .as_ref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok());

        // Determine destination root (needed for logic later)
        let destination_root = config_json
            .as_ref()
            .and_then(|v| v.get("destination_root"))
            .and_then(|s| s.as_str());

        let remote_destination = Self::determine_remote_destination(input, config_json.as_ref());

        // Determine operation
        let operation: RcloneOperation = config_json
            .as_ref()
            .and_then(|v| v.get("operation"))
            .and_then(|s| serde_json::from_value(s.clone()).ok())
            .unwrap_or_default();

        // Sync operation is only allowed in batch mode (directory sync semantics)
        if matches!(operation, RcloneOperation::Sync) && input.inputs.len() == 1 {
            return Err(crate::Error::Validation(
                "Sync operation is not supported for single file uploads. \
                 Use 'copy' or 'move' instead. Sync is designed for directory synchronization \
                 and may delete files at the destination."
                    .to_string(),
            ));
        }

        // Determine config path
        let config_path = config_json
            .as_ref()
            .and_then(|v| v.get("config_path"))
            .and_then(|s| s.as_str());

        // Extract extra args
        let extra_args: Option<Vec<String>> = config_json
            .as_ref()
            .and_then(|v| v.get("args"))
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        let extra_args_ref = extra_args.as_deref();

        // Choose single or batch mode based on input count
        if input.inputs.len() == 1 {
            // Single file mode - append filename to destination
            let input_path = &input.inputs[0];
            let input_file = Path::new(input_path);
            let file_name = input_file.file_name().unwrap_or_default().to_string_lossy();

            // If destination doesn't look like it includes the filename, append it
            let full_destination =
                if remote_destination.ends_with('/') || destination_root.is_some() {
                    format!("{}/{}", remote_destination.trim_end_matches('/'), file_name)
                } else {
                    remote_destination.clone()
                };

            self.process_single(
                input_path,
                &full_destination,
                operation,
                config_path,
                extra_args_ref,
                ctx,
            )
            .await
        } else {
            // Batch mode - use --files-from
            info!("Using batch mode for {} files", input.inputs.len());

            self.process_batch(
                &input.inputs,
                &remote_destination,
                operation,
                config_path,
                extra_args_ref,
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

        let config_json =
            serde_json::from_str::<serde_json::Value>(input.config.as_ref().unwrap()).ok();
        let destination =
            RcloneProcessor::determine_remote_destination(&input, config_json.as_ref());

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

        let config_json =
            serde_json::from_str::<serde_json::Value>(input.config.as_ref().unwrap()).ok();
        let destination =
            RcloneProcessor::determine_remote_destination(&input, config_json.as_ref());

        // Should use created_at (2024-01-01) for time placeholders
        assert_eq!(destination, "remote:/2024/01/01/StreamerName");
    }
}
