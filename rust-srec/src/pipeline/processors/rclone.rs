//! Rclone processor for cloud storage operations.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

use super::traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;

/// Rclone operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RcloneOperation {
    /// Copy files (default).
    Copy,
    /// Move files (deletes source).
    Move,
    /// Sync files (make destination identical to source).
    Sync,
}

impl Default for RcloneOperation {
    fn default() -> Self {
        Self::Copy
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

    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        let input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");

        // Parse config
        let config_json = input
            .config
            .as_ref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(c).ok());

        // Determine destination root
        let destination_root = config_json
            .as_ref()
            .and_then(|v| v.get("destination_root"))
            .and_then(|s| s.as_str());

        // Determine output path (remote)
        let remote_destination = if let Some(out) = input.outputs.first() {
            out.clone()
        } else if let Some(root) = destination_root {
            let input_path_obj = std::path::Path::new(input_path);
            let file_name = input_path_obj
                .file_name()
                .unwrap_or_default()
                .to_string_lossy();
            format!("{}/{}", root.trim_end_matches('/'), file_name)
        } else {
            return Err(crate::Error::Validation(
                "No output path provided and no 'destination_root' in config for RcloneProcessor"
                    .to_string(),
            ));
        };

        // Determine operation
        let operation: RcloneOperation = config_json
            .as_ref()
            .and_then(|v| v.get("operation"))
            .and_then(|s| serde_json::from_value(s.clone()).ok())
            .unwrap_or_default();

        // Determine config path
        let config_path = config_json
            .as_ref()
            .and_then(|v| v.get("config_path"))
            .and_then(|s| s.as_str());

        let cmd_op = match operation {
            RcloneOperation::Copy => "copy",
            RcloneOperation::Move => "move",
            RcloneOperation::Sync => "sync",
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

            // Build rclone command
            let mut cmd = Command::new(&self.rclone_path);

            if let Some(cfg) = config_path {
                cmd.arg("--config").arg(cfg);
            }

            cmd.args([cmd_op, "--progress", input_path, &remote_destination]);

            // Add extra args if present
            if let Some(args) = config_json
                .as_ref()
                .and_then(|v| v.get("args"))
                .and_then(|a| a.as_array())
            {
                for arg in args {
                    if let Some(s) = arg.as_str() {
                        cmd.arg(s);
                    }
                }
            }

            // Execute command and capture logs
            let command_output =
                match crate::pipeline::processors::utils::run_command_with_logs(&mut cmd).await {
                    Ok(output) => output,
                    Err(e) => {
                        last_error = Some(format!("Failed to execute rclone: {}", e));
                        continue;
                    }
                };

            if command_output.status.success() {
                info!(
                    "Rclone {} completed in {:.2}s",
                    cmd_op, command_output.duration
                );

                let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

                // Determine pipeline outputs
                let outputs = match operation {
                    RcloneOperation::Move => vec![],   // File consumed
                    _ => vec![input_path.to_string()], // Copy/Sync: pass through original input
                };

                return Ok(ProcessorOutput {
                    outputs,
                    duration_secs: command_output.duration,
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
                // Find last error log
                let error_msg = command_output
                    .logs
                    .iter()
                    .filter(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                    .last()
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
}
