//! Copy/Move processor for file operations.
//!
//! This processor handles copying and moving files to different locations
//! with directory creation and integrity verification.
//!
//! Requirements: 1.1, 1.2, 1.3, 1.4, 1.5

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::fs;
use tracing::{debug, error, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::create_log_entry;
use crate::Result;

/// Default value for create_dirs option.
fn default_true() -> bool {
    true
}

/// Operation type for copy/move processor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CopyMoveOperation {
    /// Copy the file to destination (keeps original).
    #[default]
    Copy,
    /// Move the file to destination (removes original).
    Move,
}

/// Configuration for copy/move operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyMoveConfig {
    /// Operation type: "copy" or "move".
    #[serde(default)]
    pub operation: CopyMoveOperation,

    /// Destination path for the file.
    /// If not provided, uses the first output path from ProcessorInput.
    pub destination: Option<String>,

    /// Whether to create destination directories if they don't exist.
    /// Requirements: 1.3
    #[serde(default = "default_true")]
    pub create_dirs: bool,

    /// Whether to verify file integrity after copy using size comparison.
    /// Requirements: 1.5
    #[serde(default = "default_true")]
    pub verify_integrity: bool,

    /// Whether to overwrite existing files at destination.
    #[serde(default)]
    pub overwrite: bool,
}

impl Default for CopyMoveConfig {
    fn default() -> Self {
        Self {
            operation: CopyMoveOperation::Copy,
            destination: None,
            create_dirs: true,
            verify_integrity: true,
            overwrite: false,
        }
    }
}

/// Processor for copying and moving files.
///
/// Handles file copy and move operations with:
/// - Directory creation (Requirements: 1.3)
/// - Integrity verification via size comparison (Requirements: 1.5)
/// - Disk space error reporting (Requirements: 1.4)
pub struct CopyMoveProcessor;

impl CopyMoveProcessor {
    /// Create a new copy/move processor.
    pub fn new() -> Self {
        Self
    }

    /// Get available disk space at the given path.
    /// Returns None if unable to determine.
    ///
    /// Note: This is a best-effort implementation. On some platforms or
    /// configurations, it may not be able to determine available space.
    #[allow(unused_variables)]
    async fn get_available_space(path: &Path) -> Option<u64> {
        // Use fs2 crate for cross-platform disk space checking if available
        // For now, we'll skip the pre-check and rely on the copy operation
        // to fail with an appropriate error if there's insufficient space.
        // This is a reasonable approach since:
        // 1. The copy operation will fail with ENOSPC/ERROR_DISK_FULL
        // 2. We handle that error specifically in the copy operation
        // 3. Checking space before copy has a race condition anyway
        None
    }

    /// Format bytes as human-readable string.
    fn format_bytes(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.2} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.2} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} bytes", bytes)
        }
    }
}

impl Default for CopyMoveProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for CopyMoveProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["copy", "move"]
    }

    fn name(&self) -> &'static str {
        "CopyMoveProcessor"
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Parse config or use defaults
        let config: CopyMoveConfig = if let Some(ref config_str) = input.config {
            serde_json::from_str(config_str).unwrap_or_else(|e| {
                warn!("Failed to parse copy/move config, using defaults: {}", e);
                CopyMoveConfig::default()
            })
        } else {
            CopyMoveConfig::default()
        };

        // Get source path
        let source_path = input.inputs.first().ok_or_else(|| {
            crate::Error::PipelineError("No input file specified for copy/move".to_string())
        })?;

        // Get destination path from config or outputs
        let dest_path = config
            .destination
            .as_ref()
            .or_else(|| input.outputs.first())
            .ok_or_else(|| {
                crate::Error::PipelineError(
                    "No destination path specified for copy/move".to_string(),
                )
            })?;

        let source = Path::new(source_path);
        let dest = Path::new(dest_path);

        // Initialize logs vector
        let mut logs = Vec::new();

        let log_msg = format!(
            "{} {} -> {}",
            if config.operation == CopyMoveOperation::Copy {
                "Copying"
            } else {
                "Moving"
            },
            source_path,
            dest_path
        );
        info!("{}", log_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            log_msg,
        ));

        // Check if source exists
        if !source.exists() {
            let error_msg = format!("Source file does not exist: {}", source_path);
            error!("{}", error_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Error,
                error_msg.clone(),
            ));
            return Err(crate::Error::PipelineError(error_msg));
        }

        // Get source file size
        let source_metadata = fs::metadata(source).await.map_err(|e| {
            crate::Error::PipelineError(format!("Failed to get source file metadata: {}", e))
        })?;
        let source_size = source_metadata.len();

        // Check if destination exists and handle overwrite
        if dest.exists() && !config.overwrite {
            let error_msg = format!(
                "Destination file already exists and overwrite is disabled: {}",
                dest_path
            );
            error!("{}", error_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Error,
                error_msg.clone(),
            ));
            return Err(crate::Error::PipelineError(error_msg));
        }

        // Create destination directory if needed (Requirements: 1.3)
        if config.create_dirs
            && let Some(parent) = dest.parent()
            && !parent.exists()
        {
            let log_msg = format!("Creating destination directory: {:?}", parent);
            debug!("{}", log_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Debug,
                log_msg,
            ));

            fs::create_dir_all(parent).await.map_err(|e| {
                crate::Error::PipelineError(format!(
                    "Failed to create destination directory: {}",
                    e
                ))
            })?;
        }

        // Check available disk space (Requirements: 1.4)
        if let Some(parent) = dest.parent()
            && let Some(available) = Self::get_available_space(parent).await
            && available < source_size
        {
            return Err(crate::Error::PipelineError(format!(
                "Insufficient disk space. Required: {}, Available: {}",
                Self::format_bytes(source_size),
                Self::format_bytes(available)
            )));
        }

        // Perform the copy operation (Requirements: 1.1)
        if let Err(e) = fs::copy(source, dest).await {
            // Check if it's a disk space error
            // ENOSPC on Unix = 28, ERROR_DISK_FULL on Windows = 112
            let is_disk_full = e.raw_os_error() == Some(28)
                || e.raw_os_error() == Some(112)
                || e.to_string().to_lowercase().contains("no space left")
                || e.to_string().to_lowercase().contains("disk full");

            let error_msg = if is_disk_full {
                format!(
                    "Insufficient disk space while copying. Required: {}",
                    Self::format_bytes(source_size)
                )
            } else {
                format!("Failed to copy file: {}", e)
            };

            error!("{}", error_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Error,
                error_msg.clone(),
            ));

            return Err(crate::Error::PipelineError(error_msg));
        }

        // Verify integrity using size comparison (Requirements: 1.5)
        let dest_size = if config.verify_integrity {
            let dest_metadata = fs::metadata(dest).await.map_err(|e| {
                crate::Error::PipelineError(format!(
                    "Failed to get destination file metadata: {}",
                    e
                ))
            })?;
            let dest_size = dest_metadata.len();

            if dest_size != source_size {
                // Clean up the incomplete copy
                let _ = fs::remove_file(dest).await;
                let error_msg = format!(
                    "File integrity check failed. Source size: {}, Destination size: {}",
                    source_size, dest_size
                );
                error!("{}", error_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    error_msg.clone(),
                ));
                return Err(crate::Error::PipelineError(error_msg));
            }

            let log_msg = format!(
                "Integrity verified: {} bytes copied successfully",
                dest_size
            );
            debug!("{}", log_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Debug,
                log_msg,
            ));
            dest_size
        } else {
            // Get dest size without verification
            fs::metadata(dest)
                .await
                .map(|m| m.len())
                .unwrap_or(source_size)
        };

        // For move operation, remove the source file (Requirements: 1.2)
        if config.operation == CopyMoveOperation::Move {
            if let Err(e) = fs::remove_file(source).await {
                let error_msg = format!("Failed to remove source file after move: {}", e);
                error!("{}", error_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    error_msg.clone(),
                ));
                return Err(crate::Error::PipelineError(error_msg));
            }
            let log_msg = format!("Source file removed after move: {}", source_path);
            debug!("{}", log_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Debug,
                log_msg,
            ));
        }

        let duration = start.elapsed().as_secs_f64();

        let success_msg = format!(
            "{} completed in {:.2}s: {} -> {}",
            if config.operation == CopyMoveOperation::Copy {
                "Copy"
            } else {
                "Move"
            },
            duration,
            source_path,
            dest_path
        );
        info!("{}", success_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            success_msg,
        ));

        // Requirements: 11.5 - Track succeeded inputs for partial failure reporting
        Ok(ProcessorOutput {
            outputs: vec![dest_path.clone()],
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "operation": format!("{:?}", config.operation),
                    "source": source_path,
                    "destination": dest_path,
                    "size_bytes": dest_size,
                    "verified": config.verify_integrity,
                })
                .to_string(),
            ),
            items_produced: vec![dest_path.clone()],
            input_size_bytes: Some(source_size),
            output_size_bytes: Some(dest_size),
            // Single input succeeded if we reach this point
            failed_inputs: vec![],
            succeeded_inputs: vec![source_path.clone()],
            skipped_inputs: vec![],
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_copy_move_processor_type() {
        let processor = CopyMoveProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Io);
    }

    #[test]
    fn test_copy_move_processor_job_types() {
        let processor = CopyMoveProcessor::new();
        assert!(processor.can_process("copy"));
        assert!(processor.can_process("move"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_copy_move_processor_name() {
        let processor = CopyMoveProcessor::new();
        assert_eq!(processor.name(), "CopyMoveProcessor");
    }

    #[test]
    fn test_copy_move_config_default() {
        let config = CopyMoveConfig::default();
        assert_eq!(config.operation, CopyMoveOperation::Copy);
        assert!(config.create_dirs);
        assert!(config.verify_integrity);
        assert!(!config.overwrite);
    }

    #[test]
    fn test_copy_move_config_parse() {
        let json = r#"{
            "operation": "move",
            "destination": "/dest/file.mp4",
            "create_dirs": true,
            "verify_integrity": false,
            "overwrite": true
        }"#;

        let config: CopyMoveConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.operation, CopyMoveOperation::Move);
        assert_eq!(config.destination, Some("/dest/file.mp4".to_string()));
        assert!(config.create_dirs);
        assert!(!config.verify_integrity);
        assert!(config.overwrite);
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(CopyMoveProcessor::format_bytes(500), "500 bytes");
        assert_eq!(CopyMoveProcessor::format_bytes(1024), "1.00 KB");
        assert_eq!(CopyMoveProcessor::format_bytes(1536), "1.50 KB");
        assert_eq!(CopyMoveProcessor::format_bytes(1048576), "1.00 MB");
        assert_eq!(CopyMoveProcessor::format_bytes(1073741824), "1.00 GB");
    }

    #[tokio::test]
    async fn test_copy_operation() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "copy"
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify copy succeeded
        assert!(dest_path.exists());
        assert!(source_path.exists()); // Source should still exist
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
        assert!(output.input_size_bytes.is_some());
        assert!(output.output_size_bytes.is_some());
        assert_eq!(output.input_size_bytes, output.output_size_bytes);
    }

    #[tokio::test]
    async fn test_move_operation() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "move"
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify move succeeded
        assert!(dest_path.exists());
        assert!(!source_path.exists()); // Source should be removed
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_create_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("subdir1/subdir2/dest.txt");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "create_dirs": true
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify directories were created and copy succeeded
        assert!(dest_path.exists());
        assert!(dest_path.parent().unwrap().exists());
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_overwrite_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        // Create source and destination files
        fs::write(&source_path, "source content").await.unwrap();
        fs::write(&dest_path, "existing content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "overwrite": false
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because destination exists and overwrite is disabled
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_overwrite_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        // Create source and destination files
        fs::write(&source_path, "new content").await.unwrap();
        fs::write(&dest_path, "old content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "overwrite": true
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify overwrite succeeded
        assert!(dest_path.exists());
        let content = fs::read_to_string(&dest_path).await.unwrap();
        assert_eq!(content, "new content");
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_source_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("nonexistent.txt");
        let dest_path = temp_dir.path().join("dest.txt");

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![dest_path.to_string_lossy().to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because source doesn't exist
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_no_input_file() {
        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec!["/dest/file.txt".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because no input file specified
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No input file"));
    }

    #[tokio::test]
    async fn test_no_destination() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        fs::write(&source_path, "test").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because no destination specified
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No destination"));
    }
}
