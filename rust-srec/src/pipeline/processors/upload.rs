//! Upload processor for cloud storage and platform uploads.

use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, error, info};

use super::traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;

/// Processor for uploading files to cloud storage via rclone.
pub struct UploadProcessor {
    /// Path to rclone binary.
    rclone_path: String,
    /// Maximum retry attempts.
    max_retries: u32,
}

impl UploadProcessor {
    /// Create a new upload processor.
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

    /// Set the maximum retry attempts.
    pub fn with_max_retries(mut self, retries: u32) -> Self {
        self.max_retries = retries;
        self
    }
}

impl Default for UploadProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for UploadProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["upload"]
    }

    fn name(&self) -> &'static str {
        "UploadProcessor"
    }

    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();
        
        info!(
            "Uploading {} to {}",
            input.input_path, input.output_path
        );

        let mut last_error = None;

        for attempt in 0..self.max_retries {
            if attempt > 0 {
                info!("Retry attempt {} for upload", attempt + 1);
                tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt))).await;
            }

            // Build rclone command
            let mut cmd = Command::new(&self.rclone_path);
            cmd.args([
                "copy",
                "--progress",
                &input.input_path,
                &input.output_path,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    last_error = Some(format!("Failed to spawn rclone: {}", e));
                    continue;
                }
            };

            // Read stderr for progress
            if let Some(stderr) = child.stderr.take() {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!("rclone: {}", line);
                }
            }

            let status = match child.wait().await {
                Ok(s) => s,
                Err(e) => {
                    last_error = Some(format!("Failed to wait for rclone: {}", e));
                    continue;
                }
            };

            if status.success() {
                let duration = start.elapsed().as_secs_f64();
                
                info!(
                    "Upload completed in {:.2}s: {}",
                    duration, input.output_path
                );

                return Ok(ProcessorOutput {
                    output_path: input.output_path.clone(),
                    duration_secs: duration,
                    metadata: None,
                });
            } else {
                last_error = Some(format!(
                    "rclone failed with exit code: {}",
                    status.code().unwrap_or(-1)
                ));
            }
        }

        error!("Upload failed after {} attempts", self.max_retries);
        Err(crate::Error::Other(last_error.unwrap_or_else(|| "Upload failed".to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_processor_type() {
        let processor = UploadProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Io);
    }

    #[test]
    fn test_upload_processor_job_types() {
        let processor = UploadProcessor::new();
        assert!(processor.can_process("upload"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_upload_processor_name() {
        let processor = UploadProcessor::new();
        assert_eq!(processor.name(), "UploadProcessor");
    }
}
