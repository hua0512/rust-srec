//! Thumbnail processor for extracting video thumbnails.

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

use super::outputs::{OutputBatch, output_size};
use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{get_extension, is_image, is_video, parse_config_or_default};
use crate::Result;

/// Configuration for thumbnail extraction.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ThumbnailConfig {
    /// Timestamp to extract thumbnail from (in seconds).
    #[serde(default = "default_timestamp")]
    pub timestamp_secs: f64,
    /// Output width (height auto-calculated to maintain aspect ratio).
    /// Ignored if `preserve_resolution` is true.
    #[serde(default = "default_width")]
    pub width: u32,
    /// Output quality (1-31, lower is better).
    #[serde(default = "default_quality")]
    pub quality: u32,
    /// If true, preserve the original video resolution (no scaling).
    #[serde(default)]
    pub preserve_resolution: bool,
}

fn default_timestamp() -> f64 {
    10.0
}

fn default_width() -> u32 {
    320
}

fn default_quality() -> u32 {
    2
}

impl Default for ThumbnailConfig {
    fn default() -> Self {
        Self {
            timestamp_secs: default_timestamp(),
            width: default_width(),
            quality: default_quality(),
            preserve_resolution: false,
        }
    }
}

/// Processor for extracting thumbnails from video files.
pub struct ThumbnailProcessor {
    /// Path to ffmpeg binary.
    ffmpeg_path: String,
}

impl ThumbnailProcessor {
    /// Create a new thumbnail processor.
    pub fn new() -> Self {
        Self {
            ffmpeg_path: std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string()),
        }
    }

    /// Create with a custom ffmpeg path.
    pub fn with_ffmpeg_path(path: impl Into<String>) -> Self {
        Self {
            ffmpeg_path: path.into(),
        }
    }

    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &ThumbnailConfig,
        ctx: &ProcessorContext,
        batch: &mut OutputBatch,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Check if input file exists
        if !super::utils::try_exists(Path::new(input_path)).await? {
            return Err(crate::Error::PipelineError(format!(
                "Input file does not exist: {}",
                input_path
            )));
        }

        // Get extension once for reuse
        let ext = get_extension(input_path).unwrap_or_default();

        // Check if input is already an image - pass through as-is
        if is_image(&ext) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!(
                "Input is already an image, passing through: {}",
                input_path
            ));
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "input is already an image",
                "already_image",
                duration,
                Vec::new(),
            ));
        }

        // Check if input is a supported video format
        // If not supported, pass through the input file instead of failing
        if !is_video(&ext) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!(
                "Input file is not a supported video format for thumbnail extraction, passing through: {}",
                input_path
            ));
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "not a supported video format for thumbnail extraction",
                "unsupported_video_format",
                duration,
                Vec::new(),
            ));
        }

        // Determine output path: use provided output or generate one dynamically
        let output_string = if let Some(out) = output_override.filter(|s| !s.is_empty()) {
            out.to_string()
        } else {
            // Generate dynamic output path: input_filename.jpg
            let input_path_obj = std::path::Path::new(input_path);
            let file_stem = input_path_obj
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let parent = input_path_obj
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));

            // Use same directory as input, but with .jpg extension
            parent
                .join(format!("{}.jpg", file_stem))
                .to_string_lossy()
                .to_string()
        };
        let output_path = output_string.as_str();
        let temp_path = batch.stage(Path::new(output_path), true).await?;
        let temp_name = temp_path.to_string_lossy();

        ctx.info(format!(
            "Extracting thumbnail from {} at {:.2}s{}",
            input_path,
            config.timestamp_secs,
            if config.preserve_resolution {
                " (native resolution)"
            } else {
                ""
            }
        ));

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        crate::utils::configure_ffmpeg_locale(&mut cmd);
        cmd.args([
            "-y",
            "-hide_banner",
            "-nostats",
            "-loglevel",
            "warning",
            "-progress",
            "pipe:1",
            "-ss",
            &format!("{:.2}", config.timestamp_secs),
            "-i",
            input_path,
            "-vframes",
            "1",
        ]);

        // MJPEG requires even dimensions; always ensure width and height are even.
        if !config.preserve_resolution {
            // -2 auto-calculates height rounded to the nearest even number
            cmd.args(["-vf", &format!("scale={}:-2", config.width)]);
        } else {
            // Pad to even dimensions (adds 1px black border only if odd)
            cmd.args(["-vf", "pad=ceil(iw/2)*2:ceil(ih/2)*2"]);
        }

        cmd.args([
            "-q:v",
            &config.quality.to_string(),
            "-update",
            "1",
            &temp_name,
        ]);

        // Execute command and capture logs
        let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
            &mut cmd,
            &ctx.progress,
            Some(ctx.log_sink.clone()),
        )
        .await?;

        if !command_output.status.success() {
            // Reconstruct stderr for error analysis
            let stderr = command_output
                .logs
                .iter()
                .filter(|l| l.level != crate::pipeline::job_queue::LogLevel::Info)
                .map(|l| l.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            ctx.error(format!("ffmpeg failed: {}", stderr));

            // Check for no video stream error - pass through instead of failing
            if stderr.contains("does not contain any stream")
                || stderr.contains("Output file is empty")
                || stderr.contains("Invalid data found")
                || stderr.contains("no video stream")
            {
                batch.discard(&temp_path);
                ctx.info(format!(
                    "Input file has no extractable video frames, passing through: {}",
                    input_path
                ));
                return Ok(ProcessorOutput::skipped_file(
                    input_path,
                    "no extractable video frames",
                    "no_video_frames",
                    command_output.duration,
                    command_output.logs,
                ));
            }

            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code: {}",
                command_output.status.code().unwrap_or(-1)
            )));
        }

        let output_size_bytes = Some(output_size(&temp_path).await?);
        debug!("ffmpeg exited successfully");

        ctx.info(format!(
            "Thumbnail extracted in {:.2}s: {}",
            command_output.duration, output_path
        ));

        // Get file sizes for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

        let width_str = if config.preserve_resolution {
            "native".to_string()
        } else {
            config.width.to_string()
        };

        // Only return the newly produced thumbnail (no additive passthrough)
        Ok(ProcessorOutput {
            outputs: vec![output_path.to_string()],
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "timestamp_secs": config.timestamp_secs,
                    "width": width_str,
                    "preserve_resolution": config.preserve_resolution,
                })
                .to_string(),
            ),
            items_produced: vec![output_path.to_string()],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            uploads: vec![],
            logs: command_output.logs,
        })
    }
}

struct ThumbnailProcessorItem<'a> {
    processor: &'a ThumbnailProcessor,
    config: &'a ThumbnailConfig,
    ctx: &'a ProcessorContext,
}

#[async_trait]
impl super::media_driver::MediaItem for ThumbnailProcessorItem<'_> {
    type Publication = super::media_driver::StagedPublication;
    async fn process(
        &self,
        input: &str,
        output: Option<&str>,
        publication: &mut Self::Publication,
    ) -> Result<ProcessorOutput> {
        self.processor
            .process_one(input, output, self.config, self.ctx, &mut publication.0)
            .await
    }
}

impl Default for ThumbnailProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for ThumbnailProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["thumbnail"]
    }

    fn name(&self) -> &'static str {
        "ThumbnailProcessor"
    }

    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let config: ThumbnailConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "thumbnail", None);

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for thumbnail extraction".to_string(),
            ));
        }

        let plan = super::planning::OutputPlan::unary_or_mapped(&input.inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("Thumbnail batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})", input.inputs.len(), input.outputs.len())))?;
        let is_batch = plan.is_batch();
        let mut output = super::media_driver::run_media(
            plan,
            super::media_driver::StagedPublication(OutputBatch::new(&input.inputs)),
            &ThumbnailProcessorItem {
                processor: self,
                config: &config,
                ctx,
            },
        )
        .await?;
        if is_batch {
            output.metadata = Some(
                serde_json::json!({ "batch": true, "inputs": input.inputs.len() }).to_string(),
            );
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_thumbnail_processor_type() {
        let processor = ThumbnailProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_thumbnail_processor_job_types() {
        let processor = ThumbnailProcessor::new();
        assert!(processor.can_process("thumbnail"));
        assert!(!processor.can_process("upload"));
    }

    #[test]
    fn test_thumbnail_config_parse() {
        let json = r#"{"timestamp_secs": 30.0, "width": 640}"#;
        let config: ThumbnailConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.timestamp_secs, 30.0);
        assert_eq!(config.width, 640);
        assert_eq!(config.quality, 2); // default
        assert!(!config.preserve_resolution); // default
    }

    #[test]
    fn test_thumbnail_config_native_resolution() {
        let json = r#"{"timestamp_secs": 10.0, "preserve_resolution": true}"#;
        let config: ThumbnailConfig = serde_json::from_str(json).unwrap();

        assert_eq!(config.timestamp_secs, 10.0);
        assert!(config.preserve_resolution);
    }

    #[test]
    fn test_thumbnail_processor_name() {
        let processor = ThumbnailProcessor::new();
        assert_eq!(processor.name(), "ThumbnailProcessor");
    }

    #[test]
    fn test_thumbnail_config_invalid_type() {
        // Numeric configuration fields reject strings during deserialization.
        let json = r#"{"width": "1280"}"#; // "1280" string instead of number
        let result: serde_json::Result<ThumbnailConfig> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "Deserialization should fail for string width"
        );

        // Confirm fallback behavior (simulated)
        let config = result.unwrap_or_default();
        assert_eq!(config.width, 320);
    }

    #[tokio::test]
    async fn test_thumbnail_batch_outputs_len_mismatch_errors() {
        let processor = ThumbnailProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
            outputs: vec!["out.jpg".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("outputs to be empty or have the same length")
        );
    }

    #[tokio::test]
    async fn test_thumbnail_batch_skips_unsupported_formats() {
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a.txt");
        let b = temp_dir.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let processor = ThumbnailProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec![
                a.to_string_lossy().to_string(),
                b.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(output.outputs.len(), 2);
        assert_eq!(output.outputs[0], a.to_string_lossy().to_string());
        assert_eq!(output.outputs[1], b.to_string_lossy().to_string());
        assert!(output.items_produced.is_empty());
        assert_eq!(output.skipped_inputs.len(), 2);
    }
}
