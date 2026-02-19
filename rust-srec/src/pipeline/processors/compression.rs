//! Compression processor for creating archives from input files.
//!
//! This processor creates compressed archives (ZIP or tar.gz) from input files,
//! supporting multiple input files in a single archive.
//!

use async_trait::async_trait;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, parse_config_or_default};
use crate::Result;
use crate::pipeline::progress::{JobProgressSnapshot, ProgressKind, ProgressReporter};

/// Default compression level (6 is a good balance between speed and compression).
fn default_compression_level() -> u8 {
    6
}

/// Archive format options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    /// ZIP archive format.
    #[default]
    Zip,
    /// Gzipped tar archive format.
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

const PROGRESS_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

struct CancelProgressReader<R> {
    inner: R,
    cancel: CancellationToken,
    progress: ProgressReporter,
    bytes_total: u64,
    bytes_done: u64,
    last_report_at: std::time::Instant,
    file_index: usize,
    file_count: usize,
    current_file: String,
}

impl<R> CancelProgressReader<R> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: R,
        cancel: CancellationToken,
        progress: ProgressReporter,
        bytes_total: u64,
        bytes_done: u64,
        file_index: usize,
        file_count: usize,
        current_file: String,
    ) -> Self {
        Self {
            inner,
            cancel,
            progress,
            bytes_total,
            bytes_done,
            last_report_at: std::time::Instant::now(),
            file_index,
            file_count,
            current_file,
        }
    }

    fn maybe_report(&mut self) {
        if self.last_report_at.elapsed() < PROGRESS_REPORT_INTERVAL {
            return;
        }
        self.last_report_at = std::time::Instant::now();

        let percent = if self.bytes_total == 0 {
            None
        } else {
            Some(((self.bytes_done as f64 / self.bytes_total as f64) * 100.0) as f32)
        };

        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Compression);
        snapshot.percent = percent;
        snapshot.bytes_done = Some(self.bytes_done);
        snapshot.bytes_total = Some(self.bytes_total);
        snapshot.raw = serde_json::json!({
            "file_index": self.file_index,
            "file_count": self.file_count,
            "file": self.current_file,
        });
        self.progress.report(snapshot);
    }
}

impl<R: Read> Read for CancelProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "compression cancelled",
            ));
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.bytes_done = self.bytes_done.saturating_add(n as u64);
            self.maybe_report();
        }
        Ok(n)
    }
}

struct CancelOnDrop {
    token: CancellationToken,
    armed: bool,
}

impl CancelOnDrop {
    fn new(token: CancellationToken) -> Self {
        Self { token, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}

fn tmp_output_path(final_path: &Path) -> PathBuf {
    let suffix = uuid::Uuid::new_v4().to_string();
    PathBuf::from(format!("{}.tmp-{}", final_path.display(), suffix))
}

fn archive_entry_name(input_path: &str, preserve_paths: bool) -> Result<String> {
    let path = Path::new(input_path);
    if !preserve_paths {
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
            crate::Error::PipelineError(format!(
                "Invalid input filename (missing file_name): {}",
                input_path
            ))
        })?;
        return Ok(name.to_string());
    }

    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {}
            std::path::Component::Normal(s) => {
                parts.push(s.to_string_lossy().to_string());
            }
        }
    }

    if parts.is_empty() {
        return Err(crate::Error::PipelineError(format!(
            "Invalid input path for archive entry: {}",
            input_path
        )));
    }
    Ok(parts.join("/"))
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
/// - ZIP archives use the `zip` crate
/// - tar.gz archives use `flate2` and `tar` crates
/// - Multiple input files are bundled into a single archive
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

    /// Create a ZIP archive from the input files.
    fn create_zip_archive(
        &self,
        inputs: &[String],
        output_path: &Path,
        config: &CompressionConfig,
        progress: ProgressReporter,
        cancel: CancellationToken,
    ) -> Result<(u64, u64)> {
        let file = File::create(output_path).map_err(|e| {
            crate::Error::PipelineError(format!("Failed to create ZIP archive: {}", e))
        })?;

        let file = BufWriter::new(file);
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
            let metadata = std::fs::metadata(input_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    crate::Error::PipelineError(format!(
                        "Input file does not exist: {}",
                        input_path
                    ))
                } else {
                    crate::Error::PipelineError(format!(
                        "Failed to get input metadata {}: {}",
                        input_path, e
                    ))
                }
            })?;
            total_input_size = total_input_size.saturating_add(metadata.len());
        }

        let mut bytes_done: u64 = 0;

        for (idx, input_path) in inputs.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(crate::Error::PipelineError(
                    "Compression cancelled".to_string(),
                ));
            }

            let archive_name = archive_entry_name(input_path, config.preserve_paths)?;
            debug!("Adding to ZIP: {} as {}", input_path, archive_name);

            let file = File::open(input_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    crate::Error::PipelineError(format!(
                        "Input file does not exist: {}",
                        input_path
                    ))
                } else {
                    crate::Error::PipelineError(format!(
                        "Failed to open input file {}: {}",
                        input_path, e
                    ))
                }
            })?;

            let mut reader = CancelProgressReader::new(
                BufReader::new(file),
                cancel.clone(),
                progress.clone(),
                total_input_size,
                bytes_done,
                idx.saturating_add(1),
                inputs.len(),
                input_path.clone(),
            );

            // Write to archive
            zip.start_file(&archive_name, options).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to start ZIP entry: {}", e))
            })?;

            std::io::copy(&mut reader, &mut zip).map_err(|e| {
                crate::Error::PipelineError(format!("Failed to write ZIP entry: {}", e))
            })?;
            bytes_done = reader.bytes_done;
        }

        zip.finish().map_err(|e| {
            crate::Error::PipelineError(format!("Failed to finalize ZIP archive: {}", e))
        })?;

        // Get output file size
        let output_size = std::fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);

        Ok((total_input_size, output_size))
    }

    /// Create a tar.gz archive from the input files.
    fn create_tar_gz_archive(
        &self,
        inputs: &[String],
        output_path: &Path,
        config: &CompressionConfig,
        progress: ProgressReporter,
        cancel: CancellationToken,
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
            let metadata = std::fs::metadata(input_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    crate::Error::PipelineError(format!(
                        "Input file does not exist: {}",
                        input_path
                    ))
                } else {
                    crate::Error::PipelineError(format!(
                        "Failed to get input metadata {}: {}",
                        input_path, e
                    ))
                }
            })?;
            total_input_size = total_input_size.saturating_add(metadata.len());
        }

        let mut bytes_done: u64 = 0;

        for (idx, input_path) in inputs.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(crate::Error::PipelineError(
                    "Compression cancelled".to_string(),
                ));
            }

            let archive_name = archive_entry_name(input_path, config.preserve_paths)?;
            debug!("Adding to tar.gz: {} as {}", input_path, archive_name);

            let mut file = File::open(input_path).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    crate::Error::PipelineError(format!(
                        "Input file does not exist: {}",
                        input_path
                    ))
                } else {
                    crate::Error::PipelineError(format!(
                        "Failed to open input file {}: {}",
                        input_path, e
                    ))
                }
            })?;

            let metadata = file.metadata().map_err(|e| {
                crate::Error::PipelineError(format!("Failed to get file metadata: {}", e))
            })?;

            let mut header = tar::Header::new_gnu();
            header.set_size(metadata.len());
            header.set_mode(0o644);
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH)
            {
                header.set_mtime(duration.as_secs());
            }

            header.set_cksum();

            let reader = CancelProgressReader::new(
                BufReader::new(&mut file),
                cancel.clone(),
                progress.clone(),
                total_input_size,
                bytes_done,
                idx.saturating_add(1),
                inputs.len(),
                input_path.clone(),
            );

            tar.append_data(&mut header, Path::new(&archive_name), reader)
                .map_err(|e| {
                    crate::Error::PipelineError(format!("Failed to add file to tar archive: {}", e))
                })?;

            bytes_done = bytes_done.saturating_add(metadata.len());
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

    fn clamp_compression_level(level: u8) -> Result<u8> {
        if level <= 9 {
            Ok(level)
        } else {
            Err(crate::Error::PipelineError(format!(
                "Invalid compression_level {} (expected 0..=9)",
                level
            )))
        }
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
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["compress", "archive"]
    }

    fn name(&self) -> &'static str {
        "CompressionProcessor"
    }

    /// Indicates this processor supports multiple inputs (batch processing).
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

        let mut config: CompressionConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "compression", Some(&mut logs));

        config.compression_level = Self::clamp_compression_level(config.compression_level)?;

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
        let output_path_str = self.determine_output_path(&input.inputs, &config, input);
        let output_path = PathBuf::from(&output_path_str);

        if let Some(parent) = output_path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| crate::Error::io_path("create_dir_all", parent, e))?;
        }

        // Check if output exists and handle overwrite
        let output_exists = tokio::fs::try_exists(&output_path)
            .await
            .map_err(|e| crate::Error::io_path("try_exists", &output_path, e))?;
        if output_exists && !config.overwrite {
            let msg = format!(
                "Output archive already exists and overwrite is disabled: {}",
                output_path.display()
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
            output_path_str
        );
        info!("{}", start_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            start_msg,
        ));

        let tmp_path = tmp_output_path(&output_path);

        let inputs = input.inputs.clone();
        let config_for_blocking = config.clone();
        let cancel = ctx.cancellation_token.child_token();
        let mut cancel_on_drop = CancelOnDrop::new(cancel.clone());
        let progress = ctx.progress.clone();

        let result = tokio::task::spawn_blocking(move || {
            struct TmpFileGuard {
                path: Option<PathBuf>,
            }

            impl TmpFileGuard {
                fn new(path: PathBuf) -> Self {
                    Self { path: Some(path) }
                }

                fn commit(mut self) {
                    self.path.take();
                }
            }

            impl Drop for TmpFileGuard {
                fn drop(&mut self) {
                    if let Some(path) = self.path.take() {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }

            let guard = TmpFileGuard::new(tmp_path.clone());

            if cancel.is_cancelled() {
                return Err(crate::Error::PipelineError(
                    "Compression cancelled".to_string(),
                ));
            }

            let processor = CompressionProcessor;
            let sizes = match config_for_blocking.format {
                ArchiveFormat::Zip => processor.create_zip_archive(
                    &inputs,
                    &tmp_path,
                    &config_for_blocking,
                    progress,
                    cancel.clone(),
                ),
                ArchiveFormat::TarGz => processor.create_tar_gz_archive(
                    &inputs,
                    &tmp_path,
                    &config_for_blocking,
                    progress,
                    cancel.clone(),
                ),
            }?;

            if cancel.is_cancelled() {
                return Err(crate::Error::PipelineError(
                    "Compression cancelled".to_string(),
                ));
            }

            match std::fs::rename(&tmp_path, &output_path) {
                Ok(()) => {
                    guard.commit();
                }
                Err(rename_err) => {
                    if config_for_blocking.overwrite && output_path.exists() {
                        std::fs::remove_file(&output_path)
                            .map_err(|e| crate::Error::io_path("remove_file", &output_path, e))?;
                        std::fs::rename(&tmp_path, &output_path)
                            .map_err(|e| crate::Error::io_path("rename", &output_path, e))?;
                        guard.commit();
                    } else {
                        return Err(crate::Error::io_path("rename", &output_path, rename_err));
                    }
                }
            }

            Ok::<_, crate::Error>(sizes)
        })
        .await
        .map_err(|e| crate::Error::Other(format!("Compression worker panicked: {}", e)))?;

        cancel_on_drop.disarm();

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
            output_path_str,
            compression_ratio
        );
        info!("{}", complete_msg);
        logs.push(create_log_entry(
            crate::pipeline::job_queue::LogLevel::Info,
            complete_msg,
        ));

        Ok(ProcessorOutput {
            outputs: vec![output_path_str.clone()],
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
            items_produced: vec![output_path_str],
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
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
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
        let filename = archive_entry_name("/path/to/file.txt", false).unwrap();
        assert_eq!(filename, "file.txt");
    }

    #[test]
    fn test_get_archive_filename_preserve() {
        let filename = archive_entry_name("/path/to/file.txt", true).unwrap();
        assert_eq!(filename, "path/to/file.txt");
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
