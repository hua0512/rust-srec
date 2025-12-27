//! Compression processor for creating archives from input files.
//!
//! This processor creates compressed archives (ZIP or tar.gz) from input files,
//! supporting multiple input files in a single archive.
//!
//! Requirements: 3.1, 3.2, 3.3, 3.4, 3.5

use async_trait::async_trait;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use tar::Builder as TarBuilder;
use tracing::{debug, error, info};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, parse_config_or_default};
use crate::Result;

/// Default compression level (6 is a good balance between speed and compression).
fn default_compression_level() -> u8 {
    6
}

/// Archive format options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    /// ZIP archive format.
    /// Requirements: 3.2
    #[default]
    Zip,
    /// Gzipped tar archive format.
    /// Requirements: 3.3
    TarGz,
}

impl ArchiveFormat {
    /// Get the default file extension for this format.
    fn extension(&self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::TarGz => "tar.gz",
        }
    }
}

/// Configuration for compression operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Archive format (zip or tar.gz).
    /// Requirements: 3.2, 3.3
    #[serde(default)]
    pub format: ArchiveFormat,

    /// Compression level (0-9, where 9 is maximum compression).
    /// 0 = no compression, 1 = fastest, 9 = best compression.
    #[serde(default = "default_compression_level")]
    pub compression_level: u8,

    /// Output archive path. If not specified, uses the first output from ProcessorInput
    /// or generates one based on the first input filename.
    pub output_path: Option<String>,

    /// Whether to overwrite existing archive file.
    #[serde(default = "default_true")]
    pub overwrite: bool,

    /// Whether to preserve directory structure in the archive.
    /// If false, all files are placed at the root of the archive.
    #[serde(default)]
    pub preserve_paths: bool,
}

fn default_true() -> bool {
    true
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            format: ArchiveFormat::Zip,
            compression_level: default_compression_level(),
            output_path: None,
            overwrite: true,
            preserve_paths: false,
        }
    }
}

/// Processor for creating compressed archives.
///
/// Supports creating ZIP and tar.gz archives from one or more input files.
/// - ZIP archives use the `zip` crate (Requirements: 3.2)
/// - tar.gz archives use `flate2` and `tar` crates (Requirements: 3.3)
/// - Multiple input files are bundled into a single archive (Requirements: 3.4)
pub struct CompressionProcessor;

impl CompressionProcessor {
    /// Create a new compression processor.
    pub fn new() -> Self {
        Self
    }

    /// Determine the output archive path based on config and input.
    fn determine_output_path(
        &self,
        inputs: &[String],
        config: &CompressionConfig,
        processor_input: &ProcessorInput,
    ) -> String {
        // Priority: config.output_path > processor_input.outputs > generated from first input
        if let Some(ref output) = config.output_path {
            return output.clone();
        }

        if let Some(output) = processor_input.outputs.first() {
            return output.clone();
        }

        // Generate output path from first input path
        if let Some(first_input) = inputs.first() {
            let input = Path::new(first_input);
            let stem = input
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("archive");
            let parent = input.parent().unwrap_or(Path::new("."));

            return parent
                .join(format!("{}.{}", stem, config.format.extension()))
                .to_string_lossy()
                .to_string();
        }

        // Fallback
        format!("archive.{}", config.format.extension())
    }

    /// Get the filename to use in the archive for a given input path.
    fn get_archive_filename(&self, input_path: &str, preserve_paths: bool) -> String {
        if preserve_paths {
            input_path.to_string()
        } else {
            Path::new(input_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string()
        }
    }

    /// Create a ZIP archive from the input files.
    /// Requirements: 3.2
    fn create_zip_archive(
        &self,
        inputs: &[String],
        output_path: &str,
        config: &CompressionConfig,
    ) -> Result<(u64, u64)> {
        let file = File::create(output_path).map_err(|e| {
            crate::Error::PipelineError(format!("Failed to create ZIP archive: {}", e))
        })?;

        let mut zip = ZipWriter::new(file);

        // Map compression level (0-9) to zip compression method
        let options = if config.compression_level == 0 {
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)
        } else {
            // Deflate compression with level
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(config.compression_level as i64))
        };

        let mut total_input_size: u64 = 0;

        for input_path in inputs {
            let path = Path::new(input_path);
            if !path.exists() {
                return Err(crate::Error::PipelineError(format!(
                    "Input file does not exist: {}",
                    input_path
                )));
            }

            let archive_name = self.get_archive_filename(input_path, config.preserve_paths);
            debug!("Adding to ZIP: {} as {}", input_path, archive_name);

            // Read file content
            let mut file = File::open(path).map_err(|e| {
                crate::Error::PipelineError(format!(
                    "Failed to open input file {}: {}",
                    input_path, e
                ))
            })?;

            let metadata = file.metadata().map_err(|e| {
                crate::Error::PipelineError(format!("Failed to get file metadata: {}", e))
            })?;
            total_input_size += metadata.len();

            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer).map_err(|e| {
                crate::Error::PipelineError(format!(
                    "Failed to read input file {}: {}",
                    input_path, e
                ))
            })?;

            // Write to archive
            zip.start_file(&archive_name, options).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to start ZIP entry: {}", e))
            })?;

            zip.write_all(&buffer).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to write to ZIP archive: {}", e))
            })?;
        }

        zip.finish().map_err(|e| {
            crate::Error::PipelineError(format!("Failed to finalize ZIP archive: {}", e))
        })?;

        // Get output file size
        let output_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);

        Ok((total_input_size, output_size))
    }

    /// Create a tar.gz archive from the input files.
    /// Requirements: 3.3
    fn create_tar_gz_archive(
        &self,
        inputs: &[String],
        output_path: &str,
        config: &CompressionConfig,
    ) -> Result<(u64, u64)> {
        let file = File::create(output_path).map_err(|e| {
            crate::Error::PipelineError(format!("Failed to create tar.gz archive: {}", e))
        })?;

        // Map compression level (0-9) to flate2 Compression
        let compression = match config.compression_level {
            0 => Compression::none(),
            1 => Compression::fast(),
            9 => Compression::best(),
            level => Compression::new(level as u32),
        };

        let encoder = GzEncoder::new(file, compression);
        let mut tar = TarBuilder::new(encoder);

        let mut total_input_size: u64 = 0;

        for input_path in inputs {
            let path = Path::new(input_path);
            if !path.exists() {
                return Err(crate::Error::PipelineError(format!(
                    "Input file does not exist: {}",
                    input_path
                )));
            }

            let archive_name = self.get_archive_filename(input_path, config.preserve_paths);
            debug!("Adding to tar.gz: {} as {}", input_path, archive_name);

            let metadata = std::fs::metadata(path).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to get file metadata: {}", e))
            })?;
            total_input_size += metadata.len();

            // Add file to tar archive
            let mut file = File::open(path).map_err(|e| {
                crate::Error::PipelineError(format!(
                    "Failed to open input file {}: {}",
                    input_path, e
                ))
            })?;

            tar.append_file(&archive_name, &mut file).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to add file to tar archive: {}", e))
            })?;
        }

        // Finish the tar archive and get the gzip encoder back
        let encoder = tar.into_inner().map_err(|e| {
            crate::Error::PipelineError(format!("Failed to finalize tar archive: {}", e))
        })?;

        // Finish gzip compression
        encoder.finish().map_err(|e| {
            crate::Error::PipelineError(format!("Failed to finalize gzip compression: {}", e))
        })?;

        // Get output file size
        let output_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);

        Ok((total_input_size, output_size))
    }

    /// Calculate compression ratio as a percentage.
    fn calculate_compression_ratio(input_size: u64, output_size: u64) -> f64 {
        if input_size == 0 {
            return 0.0;
        }
        (1.0 - (output_size as f64 / input_size as f64)) * 100.0
    }
}

impl Default for CompressionProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for CompressionProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Io
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["compress", "archive"]
    }

    fn name(&self) -> &'static str {
        "CompressionProcessor"
    }

    /// Indicates this processor supports multiple inputs (batch processing).
    /// Requirements: 3.4 - Multiple input files in a single archive
    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Initialize logs
        let mut logs = Vec::new();

        let config: CompressionConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "compression", Some(&mut logs));

        // Validate inputs
        if input.inputs.is_empty() {
            let msg = "No input files specified for compression".to_string();
            error!("{}", msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Error,
                msg.clone(),
            ));
            return Err(crate::Error::PipelineError(msg));
        }

        // Determine output path
        let output_path = self.determine_output_path(&input.inputs, &config, input);

        // Check if output exists and handle overwrite
        if Path::new(&output_path).exists() && !config.overwrite {
            let msg = format!(
                "Output archive already exists and overwrite is disabled: {}",
                output_path
            );
            error!("{}", msg);
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Error,
                msg.clone(),
            ));
            return Err(crate::Error::PipelineError(msg));
        }

        let start_msg = format!(
            "Creating {:?} archive with {} files -> {}",
            config.format,
            input.inputs.len(),
            output_path
        );
        info!("{}", start_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            start_msg,
        ));

        // Create the archive based on format
        // Requirements: 3.1, 3.2, 3.3
        let result = match config.format {
            ArchiveFormat::Zip => self.create_zip_archive(&input.inputs, &output_path, &config),
            ArchiveFormat::TarGz => {
                self.create_tar_gz_archive(&input.inputs, &output_path, &config)
            }
        };

        // Add detailed logs for inputs
        for input in &input.inputs {
            logs.push(create_log_entry(
                crate::pipeline::job_queue::LogLevel::Debug,
                format!("Added file to archive: {}", input),
            ));
        }

        let (total_input_size, output_size) = match result {
            Ok(sizes) => sizes,
            Err(e) => {
                let msg = format!("Compression failed: {}", e);
                error!("{}", msg);
                logs.push(create_log_entry(
                    crate::pipeline::job_queue::LogLevel::Error,
                    msg,
                ));
                return Err(e);
            }
        };

        let compression_ratio = Self::calculate_compression_ratio(total_input_size, output_size);
        let duration = start.elapsed().as_secs_f64();

        let complete_msg = format!(
            "Compression completed in {:.2}s: {} files -> {} (ratio: {:.1}%)",
            duration,
            input.inputs.len(),
            output_path,
            compression_ratio
        );
        info!("{}", complete_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            complete_msg,
        ));

        // Requirements: 3.5 - Record archive file path and compression ratio
        // Requirements: 11.5 - Track succeeded inputs for partial failure reporting
        Ok(ProcessorOutput {
            outputs: vec![output_path.clone()],
            duration_secs: duration,
            metadata: Some(
                serde_json::json!({
                    "format": format!("{:?}", config.format),
                    "compression_level": config.compression_level,
                    "input_files": input.inputs,
                    "input_count": input.inputs.len(),
                    "total_input_size_bytes": total_input_size,
                    "output_size_bytes": output_size,
                    "compression_ratio_percent": compression_ratio,
                })
                .to_string(),
            ),
            items_produced: vec![output_path],
            input_size_bytes: Some(total_input_size),
            output_size_bytes: Some(output_size),
            // All inputs succeeded if we reach this point
            failed_inputs: vec![],
            succeeded_inputs: input.inputs.clone(),
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
    fn test_compression_processor_type() {
        let processor = CompressionProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Io);
    }

    #[test]
    fn test_compression_processor_job_types() {
        let processor = CompressionProcessor::new();
        assert!(processor.can_process("compress"));
        assert!(processor.can_process("archive"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_compression_processor_name() {
        let processor = CompressionProcessor::new();
        assert_eq!(processor.name(), "CompressionProcessor");
    }

    #[test]
    fn test_compression_processor_supports_batch() {
        let processor = CompressionProcessor::new();
        assert!(processor.supports_batch_input());
    }

    #[test]
    fn test_archive_format_extensions() {
        assert_eq!(ArchiveFormat::Zip.extension(), "zip");
        assert_eq!(ArchiveFormat::TarGz.extension(), "tar.gz");
    }

    #[test]
    fn test_compression_config_default() {
        let config = CompressionConfig::default();
        assert_eq!(config.format, ArchiveFormat::Zip);
        assert_eq!(config.compression_level, 6);
        assert!(config.output_path.is_none());
        assert!(config.overwrite);
        assert!(!config.preserve_paths);
    }

    #[test]
    fn test_compression_config_parse() {
        let json = r#"{
            "format": "targz",
            "compression_level": 9,
            "output_path": "/output/archive.tar.gz",
            "overwrite": false,
            "preserve_paths": true
        }"#;

        let config: CompressionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, ArchiveFormat::TarGz);
        assert_eq!(config.compression_level, 9);
        assert_eq!(
            config.output_path,
            Some("/output/archive.tar.gz".to_string())
        );
        assert!(!config.overwrite);
        assert!(config.preserve_paths);
    }

    #[test]
    fn test_compression_config_parse_zip() {
        let json = r#"{"format": "zip"}"#;
        let config: CompressionConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, ArchiveFormat::Zip);
    }

    #[test]
    fn test_calculate_compression_ratio() {
        // 50% compression
        assert!((CompressionProcessor::calculate_compression_ratio(1000, 500) - 50.0).abs() < 0.01);
        // No compression
        assert!((CompressionProcessor::calculate_compression_ratio(1000, 1000) - 0.0).abs() < 0.01);
        // 90% compression
        assert!((CompressionProcessor::calculate_compression_ratio(1000, 100) - 90.0).abs() < 0.01);
        // Zero input size
        assert!((CompressionProcessor::calculate_compression_ratio(0, 0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_get_archive_filename_no_preserve() {
        let processor = CompressionProcessor::new();
        let filename = processor.get_archive_filename("/path/to/file.txt", false);
        assert_eq!(filename, "file.txt");
    }

    #[test]
    fn test_get_archive_filename_preserve() {
        let processor = CompressionProcessor::new();
        let filename = processor.get_archive_filename("/path/to/file.txt", true);
        assert_eq!(filename, "/path/to/file.txt");
    }

    #[tokio::test]
    async fn test_create_zip_archive_single_file() {
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("input.txt");
        let output_path = temp_dir.path().join("output.zip");

        // Create input file
        std::fs::write(&input_path, "test content for compression").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![input_path.to_string_lossy().to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"format": "zip"}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify archive was created
        assert!(output_path.exists());
        assert_eq!(
            output.outputs,
            vec![output_path.to_string_lossy().to_string()]
        );
        assert!(output.input_size_bytes.is_some());
        assert!(output.output_size_bytes.is_some());
    }

    #[tokio::test]
    async fn test_create_tar_gz_archive_single_file() {
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("input.txt");
        let output_path = temp_dir.path().join("output.tar.gz");

        // Create input file
        std::fs::write(&input_path, "test content for compression").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![input_path.to_string_lossy().to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"format": "targz"}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify archive was created
        assert!(output_path.exists());
        assert_eq!(
            output.outputs,
            vec![output_path.to_string_lossy().to_string()]
        );
    }

    #[tokio::test]
    async fn test_create_zip_archive_multiple_files() {
        // Requirements: 3.4 - Multiple input files in single archive
        let temp_dir = TempDir::new().unwrap();
        let input1 = temp_dir.path().join("file1.txt");
        let input2 = temp_dir.path().join("file2.txt");
        let output_path = temp_dir.path().join("output.zip");

        // Create input files
        std::fs::write(&input1, "content of file 1").unwrap();
        std::fs::write(&input2, "content of file 2").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                input1.to_string_lossy().to_string(),
                input2.to_string_lossy().to_string(),
            ],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"format": "zip"}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify archive was created
        assert!(output_path.exists());

        // Verify metadata contains file count
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert_eq!(metadata["input_count"], 2);
    }

    #[tokio::test]
    async fn test_create_tar_gz_archive_multiple_files() {
        // Requirements: 3.4 - Multiple input files in single archive
        let temp_dir = TempDir::new().unwrap();
        let input1 = temp_dir.path().join("file1.txt");
        let input2 = temp_dir.path().join("file2.txt");
        let output_path = temp_dir.path().join("output.tar.gz");

        // Create input files
        std::fs::write(&input1, "content of file 1").unwrap();
        std::fs::write(&input2, "content of file 2").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![
                input1.to_string_lossy().to_string(),
                input2.to_string_lossy().to_string(),
            ],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"format": "targz"}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify archive was created
        assert!(output_path.exists());

        // Verify metadata contains file count
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert_eq!(metadata["input_count"], 2);
    }

    #[tokio::test]
    async fn test_compression_ratio_in_metadata() {
        // Requirements: 3.5 - Record compression ratio
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("input.txt");
        let output_path = temp_dir.path().join("output.zip");

        // Create a larger input file for better compression
        let content = "a".repeat(10000);
        std::fs::write(&input_path, &content).unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![input_path.to_string_lossy().to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"format": "zip", "compression_level": 9}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();

        // Verify compression ratio is recorded
        let metadata: serde_json::Value =
            serde_json::from_str(output.metadata.as_ref().unwrap()).unwrap();
        assert!(metadata["compression_ratio_percent"].as_f64().is_some());
        assert!(metadata["total_input_size_bytes"].as_u64().is_some());
        assert!(metadata["output_size_bytes"].as_u64().is_some());
    }

    #[tokio::test]
    async fn test_no_input_files() {
        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec!["/output.zip".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No input files"));
    }

    #[tokio::test]
    async fn test_input_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let output_path = temp_dir.path().join("output.zip");

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec!["/nonexistent/file.txt".to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_overwrite_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("input.txt");
        let output_path = temp_dir.path().join("output.zip");

        // Create input and existing output
        std::fs::write(&input_path, "test content").unwrap();
        std::fs::write(&output_path, "existing archive").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![input_path.to_string_lossy().to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"overwrite": false}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn test_compression_level_zero() {
        // Test no compression (stored)
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("input.txt");
        let output_path = temp_dir.path().join("output.zip");

        std::fs::write(&input_path, "test content").unwrap();

        let processor = CompressionProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![input_path.to_string_lossy().to_string()],
            outputs: vec![output_path.to_string_lossy().to_string()],
            config: Some(serde_json::json!({"compression_level": 0}).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let _output = processor.process(&input, &ctx).await.unwrap();
        assert!(output_path.exists());
    }

    #[test]
    fn test_determine_output_path_from_config() {
        let processor = CompressionProcessor::new();
        let config = CompressionConfig {
            output_path: Some("/custom/output.zip".to_string()),
            ..Default::default()
        };
        let input = ProcessorInput {
            inputs: vec!["/input.txt".to_string()],
            outputs: vec!["/processor/output.zip".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path(&input.inputs, &config, &input);
        assert_eq!(output, "/custom/output.zip");
    }

    #[test]
    fn test_determine_output_path_from_processor_input() {
        let processor = CompressionProcessor::new();
        let config = CompressionConfig::default();
        let input = ProcessorInput {
            inputs: vec!["/input.txt".to_string()],
            outputs: vec!["/processor/output.zip".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path(&input.inputs, &config, &input);
        assert_eq!(output, "/processor/output.zip");
    }

    #[test]
    fn test_determine_output_path_generated() {
        let processor = CompressionProcessor::new();
        let config = CompressionConfig {
            format: ArchiveFormat::TarGz,
            ..Default::default()
        };
        let input = ProcessorInput {
            inputs: vec!["/path/to/video.mp4".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path(&input.inputs, &config, &input);
        assert!(output.contains("video.tar.gz"));
    }
}
