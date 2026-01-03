//! Copy/Move processor for file operations.
//!
//! This processor handles copying and moving files to different locations
//! with directory creation et integrity verification.
//!
//! Supports placeholder expansion in destination paths:
//! - `{streamer}` - Streamer name
//! - `{title}` - Session title
//! - `{streamer_id}` - Streamer ID
//! - `{session_id}` - Session ID
//! - `{platform}` - Platform name
//! - Time placeholders: `%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, etc.

use async_trait::async_trait;
use regex::RegexSet;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::path::Path;
use tokio::fs;
use tracing::{debug, error, info};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, parse_config_or_default};
use crate::Result;
use crate::utils::filename::expand_placeholders;

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
    #[serde(default = "default_true")]
    pub create_dirs: bool,

    /// Whether to verify file integrity after copy using size comparison.
    #[serde(default = "default_true")]
    pub verify_integrity: bool,

    /// Whether to overwrite existing files at destination.
    #[serde(default)]
    pub overwrite: bool,

    /// Regex patterns for excluding inputs.
    ///
    /// Patterns are matched against both the full input path and the filename.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

impl Default for CopyMoveConfig {
    fn default() -> Self {
        Self {
            operation: CopyMoveOperation::Copy,
            destination: None,
            create_dirs: true,
            verify_integrity: true,
            overwrite: false,
            exclude_patterns: Vec::new(),
        }
    }
}

/// Processor for copying and moving files.
///
/// Handles file copy and move operations with:
/// - Directory creation
/// - Integrity verification via size comparison
/// - Disk space error reporting
pub struct CopyMoveProcessor;

impl CopyMoveProcessor {
    /// Create a new copy/move processor.
    pub fn new() -> Self {
        Self
    }

    fn is_disk_full_error(error: &std::io::Error) -> bool {
        // Unix: ENOSPC = 28
        // Windows: ERROR_DISK_FULL = 112
        if matches!(error.raw_os_error(), Some(28) | Some(112)) {
            return true;
        }

        let msg = error.to_string().to_lowercase();
        msg.contains("no space left") || msg.contains("disk full")
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
        vec!["copy_move"]
    }

    fn name(&self) -> &'static str {
        "CopyMoveProcessor"
    }

    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Initialize logs vector
        let mut logs = Vec::new();

        let config: CopyMoveConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "copy_move", Some(&mut logs));

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input files specified for copy/move".to_string(),
            ));
        }

        // Get destination template from config
        let dest_template = config.destination.as_deref().ok_or_else(|| {
            crate::Error::PipelineError(
                "No destination directory specified for copy/move operation".to_string(),
            )
        })?;

        // Expand placeholders in destination path
        let dest_dir = expand_placeholders(
            dest_template,
            &input.streamer_id,
            &input.session_id,
            input.streamer_name.as_deref(),
            input.session_title.as_deref(),
            input.platform.as_deref(),
        );

        debug!(
            template = %dest_template,
            expanded = %dest_dir,
            "CopyMove: Placeholder expansion result"
        );

        let dest_dir_path = Path::new(&dest_dir);

        let exclude_set = if config.exclude_patterns.is_empty() {
            None
        } else {
            Some(RegexSet::new(&config.exclude_patterns).map_err(|e| {
                crate::Error::PipelineError(format!("Invalid exclude_patterns regex: {}", e))
            })?)
        };

        let dest_dir_exists = fs::try_exists(dest_dir_path).await.map_err(|e| {
            crate::Error::PipelineError(format!(
                "Failed to check destination directory '{}': {}",
                dest_dir, e
            ))
        })?;

        // Create destination directory if needed
        if !dest_dir_exists {
            if config.create_dirs {
                let log_msg = format!("Creating destination directory: {}", dest_dir);
                debug!("{}", log_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    log_msg,
                ));

                crate::utils::fs::ensure_dir_all_with_op(
                    "creating destination directory",
                    dest_dir_path,
                )
                .await
                .map_err(|e| {
                    crate::Error::PipelineError(format!(
                        "Failed to create destination directory '{}': {}",
                        dest_dir, e
                    ))
                })?;
            } else {
                return Err(crate::Error::PipelineError(format!(
                    "Destination directory does not exist: {}",
                    dest_dir
                )));
            }
        }

        let mut outputs = Vec::with_capacity(input.inputs.len());
        let mut items_produced = Vec::with_capacity(input.inputs.len());
        let mut succeeded_inputs = Vec::with_capacity(input.inputs.len());
        let mut failed_inputs: Vec<(String, String)> = Vec::new();
        let mut skipped_inputs: Vec<(String, String)> = Vec::new();
        let mut total_input_size: u64 = 0;
        let mut total_output_size: u64 = 0;

        for source_path in &input.inputs {
            if let Some(exclude_set) = &exclude_set
                && exclude_set.is_match(source_path)
            {
                let msg = format!("Skipping excluded input: {}", source_path);
                debug!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    msg,
                ));
                skipped_inputs.push((source_path.clone(), "excluded".to_string()));
                continue;
            }

            let source = Path::new(source_path);

            // Get filename from source
            let Some(filename) = source.file_name() else {
                let error_msg = format!("Failed to get filename from source: {}", source_path);
                error!("{}", error_msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    &error_msg,
                ));
                failed_inputs.push((source_path.clone(), error_msg));
                continue;
            };

            if let Some(exclude_set) = &exclude_set
                && exclude_set.is_match(&filename.to_string_lossy())
            {
                let msg = format!("Skipping excluded input: {}", source_path);
                debug!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Debug,
                    msg,
                ));
                skipped_inputs.push((source_path.clone(), "excluded".to_string()));
                continue;
            }

            let dest = dest_dir_path.join(filename);

            let log_msg = format!(
                "{} {} -> {}",
                if config.operation == CopyMoveOperation::Copy {
                    "Copying"
                } else {
                    "Moving"
                },
                source_path,
                dest.display()
            );
            info!("{}", log_msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Info,
                log_msg,
            ));

            // Get source file size (also validates existence)
            let source_size = match fs::metadata(source).await {
                Ok(m) => m.len(),
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    let error_msg = format!("Source file does not exist: {}", source_path);
                    error!("{}", error_msg);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Error,
                        &error_msg,
                    ));
                    failed_inputs.push((source_path.clone(), error_msg));
                    continue;
                }
                Err(e) => {
                    let error_msg = format!("Failed to get source file metadata: {}", e);
                    error!("{}", error_msg);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Error,
                        &error_msg,
                    ));
                    failed_inputs.push((source_path.clone(), error_msg));
                    continue;
                }
            };

            // Check if destination exists and handle overwrite
            if !config.overwrite {
                match fs::try_exists(&dest).await {
                    Ok(true) => {
                        let error_msg = format!(
                            "Destination file already exists and overwrite is disabled: {}",
                            dest.display()
                        );
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let error_msg =
                            format!("Failed to check destination file existence: {}", e);
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }
                }
            }

            let dest_size = match config.operation {
                CopyMoveOperation::Copy => {
                    let bytes_copied = match fs::copy(source, &dest).await {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            let error_msg = if Self::is_disk_full_error(&e) {
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
                                &error_msg,
                            ));
                            failed_inputs.push((source_path.clone(), error_msg));
                            continue;
                        }
                    };

                    // Verify integrity using size comparison (avoid extra metadata call)
                    if config.verify_integrity && bytes_copied != source_size {
                        let _ = fs::remove_file(&dest).await;
                        let error_msg = format!(
                            "File integrity check failed. Source size: {}, Destination size: {}",
                            source_size, bytes_copied
                        );
                        error!("{}", error_msg);
                        logs.push(create_log_entry(
                            crate::pipeline::job_queue::LogLevel::Error,
                            &error_msg,
                        ));
                        failed_inputs.push((source_path.clone(), error_msg));
                        continue;
                    }

                    bytes_copied
                }
                CopyMoveOperation::Move => {
                    // First try a cheap rename (fast-path on the same filesystem).
                    match fs::rename(source, &dest).await {
                        Ok(()) => source_size,
                        Err(e) if e.kind() == ErrorKind::AlreadyExists && config.overwrite => {
                            // Windows doesn't overwrite on rename; remove then retry.
                            if let Err(e) = fs::remove_file(&dest).await {
                                let error_msg = format!(
                                    "Failed to remove destination file for overwrite: {}",
                                    e
                                );
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    &error_msg,
                                ));
                                failed_inputs.push((source_path.clone(), error_msg));
                                continue;
                            }

                            if let Err(e) = fs::rename(source, &dest).await {
                                let error_msg = format!("Failed to move file: {}", e);
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    &error_msg,
                                ));
                                failed_inputs.push((source_path.clone(), error_msg));
                                continue;
                            }

                            source_size
                        }
                        Err(e) if matches!(e.raw_os_error(), Some(17) | Some(18)) => {
                            // Cross-filesystem move: fallback to copy + verify + remove.
                            let bytes_copied = match fs::copy(source, &dest).await {
                                Ok(bytes) => bytes,
                                Err(e) => {
                                    let error_msg = if Self::is_disk_full_error(&e) {
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
                                        &error_msg,
                                    ));
                                    failed_inputs.push((source_path.clone(), error_msg));
                                    continue;
                                }
                            };

                            if config.verify_integrity && bytes_copied != source_size {
                                let _ = fs::remove_file(&dest).await;
                                let error_msg = format!(
                                    "File integrity check failed. Source size: {}, Destination size: {}",
                                    source_size, bytes_copied
                                );
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    &error_msg,
                                ));
                                failed_inputs.push((source_path.clone(), error_msg));
                                continue;
                            }

                            if let Err(e) = fs::remove_file(source).await {
                                let error_msg =
                                    format!("Failed to remove source file after move: {}", e);
                                error!("{}", error_msg);
                                logs.push(create_log_entry(
                                    crate::pipeline::job_queue::LogLevel::Error,
                                    error_msg,
                                ));
                                // Don't mark as failed since copy succeeded
                            }

                            bytes_copied
                        }
                        Err(e) => {
                            let error_msg = format!("Failed to move file: {}", e);
                            error!("{}", error_msg);
                            logs.push(create_log_entry(
                                crate::pipeline::job_queue::LogLevel::Error,
                                &error_msg,
                            ));
                            failed_inputs.push((source_path.clone(), error_msg));
                            continue;
                        }
                    }
                }
            };

            let dest_path = dest.to_string_lossy().into_owned();

            total_input_size += source_size;
            total_output_size += dest_size;
            outputs.push(dest_path.clone());
            items_produced.push(dest_path);
            succeeded_inputs.push(source_path.clone());
        }

        let duration = start.elapsed().as_secs_f64();

        let success_msg = format!(
            "{} completed in {:.2}s: {} inputs ({} succeeded, {} failed, {} skipped)",
            if config.operation == CopyMoveOperation::Copy {
                "Copy"
            } else {
                "Move"
            },
            duration,
            input.inputs.len(),
            succeeded_inputs.len(),
            failed_inputs.len(),
            skipped_inputs.len()
        );
        info!("{}", success_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            success_msg,
        ));

        // If all inputs failed, return error
        if succeeded_inputs.is_empty() && !failed_inputs.is_empty() {
            return Err(crate::Error::PipelineError(format!(
                "All {} input files failed to copy/move",
                failed_inputs.len()
            )));
        }

        Ok(ProcessorOutput {
            outputs,
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "operation": format!("{:?}", config.operation),
                    "destination_dir": dest_dir,
                    "total_files": input.inputs.len(),
                    "succeeded": succeeded_inputs.len(),
                    "failed": failed_inputs.len(),
                    "skipped": skipped_inputs.len(),
                    "total_size_bytes": total_output_size,
                })
                .to_string(),
            ),
            items_produced,
            input_size_bytes: Some(total_input_size),
            output_size_bytes: Some(total_output_size),
            failed_inputs,
            succeeded_inputs,
            skipped_inputs,
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
        assert!(processor.can_process("copy_move"));
        assert!(!processor.can_process("copy"));
        assert!(!processor.can_process("move"));
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
            "overwrite": true,
            "exclude_patterns": ["\\.tmp$"]
        }"#;

        let config: CopyMoveConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.operation, CopyMoveOperation::Move);
        assert_eq!(config.destination, Some("/dest/file.mp4".to_string()));
        assert!(config.create_dirs);
        assert!(!config.verify_integrity);
        assert!(config.overwrite);
        assert_eq!(config.exclude_patterns, vec!["\\.tmp$"]);
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
        let dest_dir = temp_dir.path().join("output");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify copy succeeded
        let dest_path = dest_dir.join("source.txt");
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
    async fn test_exclude_patterns_skip_inputs() {
        let temp_dir = TempDir::new().unwrap();
        let keep_path = temp_dir.path().join("keep.txt");
        let skip_path = temp_dir.path().join("skip.tmp");
        let dest_dir = temp_dir.path().join("output");

        fs::write(&keep_path, "keep").await.unwrap();
        fs::write(&skip_path, "skip").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                keep_path.to_string_lossy().to_string(),
                skip_path.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "exclude_patterns": [r"\.tmp$"],
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        let keep_dest = dest_dir.join("keep.txt");
        let skip_dest = dest_dir.join("skip.tmp");
        assert!(keep_dest.exists());
        assert!(!skip_dest.exists());
        assert_eq!(
            output.outputs,
            vec![keep_dest.to_string_lossy().to_string()]
        );
        assert!(output.failed_inputs.is_empty());
        assert_eq!(
            output.succeeded_inputs,
            vec![keep_path.to_string_lossy().to_string()]
        );
        assert_eq!(
            output.skipped_inputs,
            vec![(
                skip_path.to_string_lossy().to_string(),
                "excluded".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn test_exclude_patterns_invalid_regex_fails() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "exclude_patterns": ["("],
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("Invalid exclude_patterns regex"));
    }

    #[tokio::test]
    async fn test_move_operation() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "move",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify move succeeded
        let dest_path = dest_dir.join("source.txt");
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
        let dest_dir = temp_dir.path().join("subdir1/subdir2");

        // Create source file
        fs::write(&source_path, "test content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
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
        let dest_path = dest_dir.join("source.txt");
        assert!(dest_path.exists());
        assert!(dest_dir.exists());
        assert_eq!(
            output.outputs,
            vec![dest_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_overwrite_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path();

        // Create source and destination files
        fs::write(&source_path, "source content").await.unwrap();
        // Destination file has same name as source
        fs::write(dest_dir.join("source.txt"), "existing content")
            .await
            .unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
                    "overwrite": false
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // In batch mode, single file failure results in partial success
        // Since this is the only file, it should fail completely
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_overwrite_enabled() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("source.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create dest dir and existing file
        fs::create_dir_all(&dest_dir).await.unwrap();
        fs::write(dest_dir.join("source.txt"), "old content")
            .await
            .unwrap();

        // Create source file
        fs::write(&source_path, "new content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy(),
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
        let dest_path = dest_dir.join("source.txt");
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
        let dest_dir = temp_dir.path().join("output");

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;

        // Should fail because source doesn't exist (single file = total failure)
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("failed to copy/move"));
    }

    #[tokio::test]
    async fn test_no_input_file() {
        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "destination": "/dest"
                })
                .to_string(),
            ),
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

    #[tokio::test]
    async fn test_placeholder_expansion() {
        let temp_dir = TempDir::new().unwrap();
        let source_path = temp_dir.path().join("video.mp4");

        // Create source file
        fs::write(&source_path, "test video content").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");

        // Use placeholders in destination using platform-native path separator
        let dest_template = temp_dir
            .path()
            .join("{platform}")
            .join("{streamer}")
            .join("{title}")
            .to_string_lossy()
            .to_string();

        let input = ProcessorInput {
            inputs: vec![source_path.to_string_lossy().to_string()],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_template
                })
                .to_string(),
            ),
            streamer_id: "streamer123".to_string(),
            session_id: "session456".to_string(),
            streamer_name: Some("TestStreamer".to_string()),
            session_title: Some("LiveStream".to_string()),
            platform: Some("Twitch".to_string()),
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify placeholders were expanded - check via output existence
        assert_eq!(output.outputs.len(), 1);
        assert!(output.outputs[0].contains("Twitch"));
        assert!(output.outputs[0].contains("TestStreamer"));
        assert!(output.outputs[0].contains("LiveStream"));
        assert!(output.outputs[0].contains("video.mp4"));

        // Verify file actually exists at output path
        let output_path = Path::new(&output.outputs[0]);
        assert!(output_path.exists());
    }

    #[tokio::test]
    async fn test_batch_copy() {
        let temp_dir = TempDir::new().unwrap();
        let source1 = temp_dir.path().join("file1.txt");
        let source2 = temp_dir.path().join("file2.txt");
        let dest_dir = temp_dir.path().join("output");

        // Create source files
        fs::write(&source1, "content 1").await.unwrap();
        fs::write(&source2, "content 2").await.unwrap();

        let processor = CopyMoveProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                source1.to_string_lossy().to_string(),
                source2.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: Some(
                serde_json::json!({
                    "operation": "copy",
                    "destination": dest_dir.to_string_lossy()
                })
                .to_string(),
            ),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify both files copied
        assert!(dest_dir.join("file1.txt").exists());
        assert!(dest_dir.join("file2.txt").exists());
        assert_eq!(output.outputs.len(), 2);
        assert_eq!(output.succeeded_inputs.len(), 2);
        assert!(output.failed_inputs.is_empty());
    }
}
