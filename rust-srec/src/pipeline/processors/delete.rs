//! Delete processor for file cleanup operations.
//!
//! This processor handles file deletion with retry logic for locked files.
//!
//! Requirements: 5.1, 5.2, 5.3, 5.4, 5.5

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::create_log_entry;
use crate::Result;

/// Default maximum retry attempts for locked files.
fn default_max_retries() -> u32 {
    3
}

/// Default base delay between retries in milliseconds.
fn default_retry_delay_ms() -> u64 {
    100
}

/// Configuration for delete operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteConfig {
    /// Maximum retry attempts for locked files.
    /// Requirements: 5.3
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Base delay between retries in milliseconds.
    /// Uses exponential backoff: delay * 2^attempt
    /// Requirements: 5.3
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
}

impl Default for DeleteConfig {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
        }
    }
}

/// Processor for deleting files with retry logic.
///
/// Handles file deletion with:
/// - Retry with exponential backoff for locked files (Requirements: 5.3)
/// - Warning log for non-existent files (Requirements: 5.2)
/// - Error reporting after all retries fail (Requirements: 5.4)
pub struct DeleteProcessor;

impl DeleteProcessor {
    /// Create a new delete processor.
    pub fn new() -> Self {
        Self
    }

    /// Check if an error indicates the file is locked/in use.
    fn is_file_locked_error(error: &std::io::Error) -> bool {
        // Windows: ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
        // Unix: EBUSY = 16, ETXTBSY = 26
        matches!(
            error.raw_os_error(),
            Some(32) | Some(33) | Some(16) | Some(26)
        ) || error.to_string().to_lowercase().contains("being used")
            || error.to_string().to_lowercase().contains("locked")
            || error.to_string().to_lowercase().contains("busy")
    }

    /// Attempt to delete a file with retry logic for locked files.
    /// Requirements: 5.1, 5.3, 5.4
    async fn delete_with_retry(
        &self,
        path: &Path,
        config: &DeleteConfig,
        logs: &mut Vec<crate::pipeline::job_queue::JobLogEntry>,
    ) -> std::result::Result<(), String> {
        let mut last_error: Option<std::io::Error> = None;

        for attempt in 0..=config.max_retries {
            match fs::remove_file(path).await {
                Ok(()) => {
                    if attempt > 0 {
                        let msg = format!(
                            "File deleted successfully after {} retries: {:?}",
                            attempt, path
                        );
                        debug!("{}", msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Debug,
                            msg,
                        ));
                    }
                    return Ok(());
                }
                Err(e) => {
                    // Check if file is locked and we should retry
                    if Self::is_file_locked_error(&e) && attempt < config.max_retries {
                        let delay = config.retry_delay_ms * 2u64.pow(attempt);
                        let msg = format!(
                            "File is locked, retrying in {}ms (attempt {}/{}): {:?}",
                            delay,
                            attempt + 1,
                            config.max_retries,
                            path
                        );
                        warn!("{}", msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Warn,
                            msg,
                        ));

                        sleep(Duration::from_millis(delay)).await;
                        last_error = Some(e);
                    } else {
                        // Not a locked file error or max retries reached
                        last_error = Some(e);
                        break;
                    }
                }
            }
        }

        // All retries exhausted or non-retryable error
        Err(last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "Unknown error".to_string()))
    }
}

impl Default for DeleteProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for DeleteProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["delete", "cleanup"]
    }

    fn name(&self) -> &'static str {
        "DeleteProcessor"
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();
        let mut logs = Vec::new();

        // Parse config or use defaults
        let config: DeleteConfig = if let Some(ref config_str) = input.config {
            serde_json::from_str(config_str).unwrap_or_else(|e| {
                let msg = format!("Failed to parse delete config, using defaults: {}", e);
                warn!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Warn,
                    msg,
                ));
                DeleteConfig::default()
            })
        } else {
            DeleteConfig::default()
        };

        // Get file path to delete
        let file_path = input.inputs.first().ok_or_else(|| {
            crate::Error::PipelineError("No file path specified for delete".to_string())
        })?;

        let path = Path::new(file_path);

        let start_msg = format!("Deleting file: {}", file_path);
        info!("{}", start_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            start_msg,
        ));

        // Check if file exists (Requirements: 5.2)
        if !path.exists() {
            let msg = format!(
                "File does not exist, marking job as completed: {}",
                file_path
            );
            warn!("{}", msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Warn,
                msg,
            ));

            let duration = start.elapsed().as_secs_f64();
            // Requirements: 11.5 - Track as succeeded (non-existent file is not a failure)
            return Ok(ProcessorOutput {
                outputs: vec![],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "file_not_found",
                        "path": file_path,
                    })
                    .to_string(),
                ),
                items_produced: vec![],
                input_size_bytes: None,
                output_size_bytes: None,
                failed_inputs: vec![],
                succeeded_inputs: vec![file_path.clone()],
                skipped_inputs: vec![],
                logs,
            });
        }

        // Get file size before deletion for metrics
        let file_size = fs::metadata(path).await.map(|m| m.len()).ok();

        // Attempt deletion with retry logic (Requirements: 5.1, 5.3, 5.4)
        match self.delete_with_retry(path, &config, &mut logs).await {
            Ok(()) => {
                let duration = start.elapsed().as_secs_f64();
                let msg = format!(
                    "File deleted successfully in {:.2}s: {}",
                    duration, file_path
                );
                info!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Info,
                    msg,
                ));

                // Requirements: 11.5 - Track succeeded inputs for partial failure reporting
                Ok(ProcessorOutput {
                    outputs: vec![],
                    duration_secs: duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "deleted",
                            "path": file_path,
                            "size_bytes": file_size,
                        })
                        .to_string(),
                    ),
                    items_produced: vec![],
                    input_size_bytes: file_size,
                    output_size_bytes: Some(0),
                    failed_inputs: vec![],
                    succeeded_inputs: vec![file_path.clone()],
                    skipped_inputs: vec![],
                    logs,
                })
            }
            Err(error_msg) => {
                let err_detail = format!(
                    "Failed to delete file after {} retries: {} - {}",
                    config.max_retries, file_path, error_msg
                );
                error!("{}", err_detail); // Log error for backend
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    err_detail.clone(),
                ));

                // Requirements: 5.4 - Report error after all retries fail
                Err(crate::Error::PipelineError(err_detail))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_delete_processor_type() {
        let processor = DeleteProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Io);
    }

    #[test]
    fn test_delete_processor_job_types() {
        let processor = DeleteProcessor::new();
        assert!(processor.can_process("delete"));
        assert!(processor.can_process("cleanup"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_delete_processor_name() {
        let processor = DeleteProcessor::new();
        assert_eq!(processor.name(), "DeleteProcessor");
    }

    #[test]
    fn test_delete_config_default() {
        let config = DeleteConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_delay_ms, 100);
    }

    #[test]
    fn test_delete_config_parse() {
        let json = r#"{
            "max_retries": 5,
            "retry_delay_ms": 200
        }"#;

        let config: DeleteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.retry_delay_ms, 200);
    }

    #[test]
    fn test_delete_config_parse_partial() {
        // Test that defaults are used for missing fields
        let json = r#"{"max_retries": 10}"#;

        let config: DeleteConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_retries, 10);
        assert_eq!(config.retry_delay_ms, 100); // default
    }

    #[tokio::test]
    async fn test_delete_existing_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");

        // Create a test file
        fs::write(&file_path, "test content").await.unwrap();
        assert!(file_path.exists());

        let processor = DeleteProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![file_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify file was deleted
        assert!(!file_path.exists());
        assert!(output.outputs.is_empty());
        assert!(output.input_size_bytes.is_some());
        assert_eq!(output.output_size_bytes, Some(0));

        // Check metadata
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert_eq!(metadata["status"], "deleted");
    }

    #[tokio::test]
    async fn test_delete_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("nonexistent.txt");

        let processor = DeleteProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![file_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };

        // Should complete successfully with warning (Requirements: 5.2)
        let output = processor.process(&input, &ctx).await.unwrap();

        assert!(output.outputs.is_empty());

        // Check metadata indicates skipped
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert_eq!(metadata["status"], "skipped");
        assert_eq!(metadata["reason"], "file_not_found");
    }

    #[tokio::test]
    async fn test_delete_no_input_file() {
        let processor = DeleteProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because no file path specified
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No file path"));
    }

    #[tokio::test]
    async fn test_delete_with_custom_config() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");

        // Create a test file
        fs::write(&file_path, "test content").await.unwrap();

        let processor = DeleteProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![file_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "max_retries": 5,
                    "retry_delay_ms": 50
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify file was deleted
        assert!(!file_path.exists());
        assert!(output.outputs.is_empty());
    }

    #[test]
    fn test_is_file_locked_error() {
        // Test various error messages that indicate locked files
        let locked_msg = std::io::Error::other(
            "The process cannot access the file because it is being used by another process",
        );
        assert!(DeleteProcessor::is_file_locked_error(&locked_msg));
        let busy_msg = std::io::Error::other("Device or resource busy");
        assert!(DeleteProcessor::is_file_locked_error(&busy_msg));

        // Test non-locked error
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        assert!(!DeleteProcessor::is_file_locked_error(&not_found));
    }
}
