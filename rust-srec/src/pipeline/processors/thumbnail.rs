//! Thumbnail processor for extracting video thumbnails.

use async_trait::async_trait;
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, error, info};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{get_extension, is_image, is_video};
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

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Parse config or use defaults
        let config: ThumbnailConfig = if let Some(ref config_str) = input.config {
            match serde_json::from_str(config_str) {
                Ok(c) => c,
                Err(e) => {
                    let _ = ctx.error(&format!(
                        "Failed to parse thumbnail config: {} (config: '{}'). Using defaults.",
                        e, config_str
                    ));
                    ThumbnailConfig::default()
                }
            }
        } else {
            ThumbnailConfig::default()
        };

        let input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");

        // Check if input file exists
        if !Path::new(input_path).exists() {
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
            let _ = ctx.info(&format!(
                "Input is already an image, passing through: {}",
                input_path
            ));
            return Ok(ProcessorOutput {
                outputs: vec![input_path.to_string()],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "already_image",
                        "input": input_path,
                    })
                    .to_string(),
                ),
                skipped_inputs: vec![(
                    input_path.to_string(),
                    "input is already an image".to_string(),
                )],
                ..Default::default()
            });
        }

        // Check if input is a supported video format
        // If not supported, pass through the input file instead of failing
        if !is_video(&ext) {
            let duration = start.elapsed().as_secs_f64();
            let _ = ctx.info(&format!(
                "Input file is not a supported video format for thumbnail extraction, passing through: {}",
                input_path
            ));
            return Ok(ProcessorOutput {
                outputs: vec![input_path.to_string()],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "unsupported_video_format",
                        "input": input_path,
                    })
                    .to_string(),
                ),
                skipped_inputs: vec![(
                    input_path.to_string(),
                    "not a supported video format for thumbnail extraction".to_string(),
                )],
                ..Default::default()
            });
        }

        // Determine output path: use provided output or generate one dynamically
        let output_string = if let Some(out) = input.outputs.first().filter(|s| !s.is_empty()) {
            out.clone()
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

        let _ = ctx.info(&format!(
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
            &input_path,
            "-vframes",
            "1",
        ]);

        // Only apply scale filter if not preserving original resolution
        if !config.preserve_resolution {
            cmd.args(["-vf", &format!("scale={}:-1", config.width)]);
        }

        cmd.args([
            "-q:v",
            &config.quality.to_string(),
            "-update",
            "1",
            &output_path,
        ])
        .env("LC_ALL", "C");

        // Execute command and capture logs
        let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
            &mut cmd,
            &ctx.progress,
            Some(ctx.log_tx.clone()),
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

            let _ = ctx.error(&format!("ffmpeg failed: {}", stderr));

            // Check for no video stream error - pass through instead of failing
            if stderr.contains("does not contain any stream")
                || stderr.contains("Output file is empty")
                || stderr.contains("Invalid data found")
                || stderr.contains("no video stream")
            {
                let _ = ctx.info(&format!(
                    "Input file has no extractable video frames, passing through: {}",
                    input_path
                ));
                return Ok(ProcessorOutput {
                    outputs: vec![input_path.to_string()],
                    duration_secs: command_output.duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": "no_video_frames",
                            "input": input_path,
                        })
                        .to_string(),
                    ),
                    skipped_inputs: vec![(
                        input_path.to_string(),
                        "no extractable video frames".to_string(),
                    )],
                    logs: command_output.logs,
                    ..Default::default()
                });
            }

            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code: {}",
                command_output.status.code().unwrap_or(-1)
            )));
        }

        debug!("ffmpeg exited successfully");

        let _ = ctx.info(&format!(
            "Thumbnail extracted in {:.2}s: {}",
            command_output.duration, output_path
        ));

        // Get file sizes for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let output_size_bytes = tokio::fs::metadata(output_path).await.ok().map(|m| m.len());

        // Requirements: 11.5 - Track succeeded inputs for partial failure reporting
        let width_str = if config.preserve_resolution {
            "native".to_string()
        } else {
            config.width.to_string()
        };

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
            items_produced: vec![],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            logs: command_output.logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_thumbnail_config_default() {
        let config = ThumbnailConfig::default();
        assert_eq!(config.timestamp_secs, 10.0);
        assert_eq!(config.width, 320);
        assert_eq!(config.quality, 2);
        assert!(!config.preserve_resolution);
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
        // This test confirms that providing a string for a numeric field causes deserialization to fail.
        // This validates our hypothesis that a type mismatch (which causes the fallback to default)
        // is caught by the error handling logic we added (which catches serde errors).
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
}
