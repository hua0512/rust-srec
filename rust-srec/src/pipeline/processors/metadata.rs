//! Metadata Embedding processor for embedding stream metadata into media files.
//!
//! This processor embeds metadata fields (artist, title, date, custom) into media files
//! using ffmpeg. It supports common container formats like MP4, MKV, and FLV.
//!

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;
use tracing::debug;

use super::outputs::{OutputBatch, output_size};
use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use crate::Result;

/// Configuration for metadata embedding operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataConfig {
    /// Artist/streamer name.
    pub artist: Option<String>,

    /// Title/stream title.
    pub title: Option<String>,

    /// Recording date (ISO 8601 format recommended, e.g., "2024-01-15").
    pub date: Option<String>,

    /// Album name (optional).
    pub album: Option<String>,

    /// Comment field (optional).
    pub comment: Option<String>,

    /// Additional custom metadata fields.
    /// Keys are metadata field names, values are the metadata values.
    #[serde(default)]
    pub custom: HashMap<String, String>,

    /// Output file path. If not specified, uses the first output from ProcessorInput
    /// or generates one based on input filename.
    pub output_path: Option<String>,

    /// Whether to overwrite existing output file.
    #[serde(default = "default_true")]
    pub overwrite: bool,

    /// Whether to remove the input file after successful metadata embedding.
    ///
    /// In batch mode, inputs are only removed after *all* embeds succeed.
    #[serde(default)]
    pub remove_input_on_success: bool,
}

fn default_true() -> bool {
    true
}

impl Default for MetadataConfig {
    fn default() -> Self {
        Self {
            artist: None,
            title: None,
            date: None,
            album: None,
            comment: None,
            custom: HashMap::new(),
            output_path: None,
            overwrite: true,
            remove_input_on_success: false,
        }
    }
}

/// Formats that support metadata embedding.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp4", "m4a", "m4v", "mkv", "webm", "mov", "flv", "avi", "mp3", "ogg", "opus", "flac",
];

/// Processor for embedding metadata into media files.
///
/// Uses ffmpeg to embed metadata fields into media containers.
/// - artist → `-metadata artist=`
/// - title → `-metadata title=`
/// - date → `-metadata date=`
pub struct MetadataProcessor {
    /// Path to ffmpeg binary.
    ffmpeg_path: String,
}

impl MetadataProcessor {
    /// Create a new metadata processor.
    pub fn new() -> Self {
        Self {
            ffmpeg_path: std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".to_string()),
        }
    }

    /// Create with a custom ffmpeg path.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "retained for optional runtime paths and diagnostics"
        )
    )]
    pub fn with_ffmpeg_path(path: impl Into<String>) -> Self {
        Self {
            ffmpeg_path: path.into(),
        }
    }

    /// Build FFmpeg command arguments for metadata embedding.
    pub fn build_args(
        &self,
        input_path: &str,
        output_path: &str,
        config: &MetadataConfig,
    ) -> Vec<String> {
        let mut args = Vec::new();

        // Overwrite flag
        if config.overwrite {
            args.push("-y".to_string());
        }

        args.push("-hide_banner".to_string());
        args.push("-nostats".to_string());
        args.extend(["-loglevel".to_string(), "info".to_string()]);
        args.extend(["-progress".to_string(), "pipe:1".to_string()]);

        // Input file
        args.extend(["-i".to_string(), input_path.to_string()]);

        // Copy all streams without re-encoding
        args.extend(["-c".to_string(), "copy".to_string()]);

        // Add standard metadata fields
        if let Some(ref artist) = config.artist {
            args.extend(["-metadata".to_string(), format!("artist={}", artist)]);
        }

        if let Some(ref title) = config.title {
            args.extend(["-metadata".to_string(), format!("title={}", title)]);
        }

        if let Some(ref date) = config.date {
            args.extend(["-metadata".to_string(), format!("date={}", date)]);
        }

        // Album
        if let Some(ref album) = config.album {
            args.extend(["-metadata".to_string(), format!("album={}", album)]);
        }

        // Comment
        if let Some(ref comment) = config.comment {
            args.extend(["-metadata".to_string(), format!("comment={}", comment)]);
        }

        // Custom metadata fields
        for (key, value) in &config.custom {
            args.extend(["-metadata".to_string(), format!("{}={}", key, value)]);
        }

        // Output file
        args.push(output_path.to_string());

        args
    }

    /// Determine the output file path based on config and input.
    fn determine_output_path(
        &self,
        input_path: &str,
        config: &MetadataConfig,
        output_override: Option<&str>,
    ) -> String {
        super::planning::choose_output(config.output_path.as_deref(), output_override, || {
            let path = Path::new(input_path);
            let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
            super::planning::sibling_output(path, "output", "_meta", extension)
        })
    }

    /// Check if the input file format supports metadata embedding.
    fn supports_metadata(&self, input_path: &str) -> bool {
        let path = Path::new(input_path);
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str())
        } else {
            false
        }
    }

    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &MetadataConfig,
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

        // Check if format supports metadata
        // If not supported, pass through the input file instead of failing
        if !self.supports_metadata(input_path) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!(
                "Input file format does not support metadata embedding, passing through: {}",
                input_path
            ));
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "format does not support metadata embedding",
                "unsupported_format",
                duration,
                Vec::new(),
            ));
        }

        let output_path = self.determine_output_path(
            input_path,
            config,
            output_override.filter(|s| !s.is_empty()),
        );

        if Path::new(input_path) == Path::new(&output_path) {
            return Err(crate::Error::PipelineError(format!(
                "metadata: output_path must be different from input_path (input: {}, output: {})",
                input_path, output_path
            )));
        }

        ctx.info(format!(
            "Embedding metadata into {} -> {} (artist: {:?}, title: {:?}, date: {:?})",
            input_path, output_path, config.artist, config.title, config.date
        ));

        let temp_path = batch
            .stage(Path::new(&output_path), config.overwrite)
            .await?;
        // Build ffmpeg arguments
        let args = self.build_args(input_path, &temp_path.to_string_lossy(), config);
        debug!("FFmpeg args: {:?}", args);

        // Get input file size for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        crate::utils::configure_ffmpeg_locale(&mut cmd);
        cmd.args(&args);

        // Execute command and capture logs
        let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
            &mut cmd,
            &ctx.progress,
            Some(ctx.log_sink.clone()),
            None,
        )
        .await?;

        if !command_output.status.success() {
            let stderr_output = command_output.diagnostic_text();

            // Check for common error patterns
            let error_msg = if stderr_output.contains("Invalid data found")
                || stderr_output.contains("could not find codec")
            {
                format!(
                    "Input file format does not support metadata embedding: {}",
                    input_path
                )
            } else {
                format!(
                    "ffmpeg failed with exit code {}: {}",
                    command_output.status.code().unwrap_or(-1),
                    command_output
                        .last_error_message()
                        .map(str::to_owned)
                        .unwrap_or_else(|| "Unknown error".to_string())
                )
            };

            return Err(crate::Error::PipelineError(error_msg));
        }

        // Get output file size for metrics
        let output_size_bytes = Some(output_size(&temp_path).await?);

        ctx.info(format!(
            "Metadata embedding completed in {:.2}s: {}",
            command_output.duration, output_path
        ));

        let logs = command_output.logs;

        // Build metadata summary for output
        let metadata_summary = serde_json::json!({
            "artist": config.artist,
            "title": config.title,
            "date": config.date,
            "album": config.album,
            "comment": config.comment,
            "custom_fields": config.custom.keys().collect::<Vec<_>>(),
            "input": input_path,
            "output": output_path,
            "input_removed": config.remove_input_on_success,
        });

        Ok(ProcessorOutput {
            outputs: vec![output_path.clone()],
            duration_secs: command_output.duration,
            metadata: Some(metadata_summary.to_string()),
            items_produced: vec![output_path],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            uploads: vec![],
            logs,
        })
    }
}

struct MetadataProcessorItem<'a> {
    processor: &'a MetadataProcessor,
    config: &'a MetadataConfig,
    ctx: &'a ProcessorContext,
}

#[async_trait]
impl super::media_driver::MediaItem for MetadataProcessorItem<'_> {
    fn runs_ffmpeg(&self, input: &str) -> bool {
        self.processor.supports_metadata(input)
    }

    async fn process(
        &self,
        input: &str,
        output: Option<&str>,
        batch: &mut OutputBatch,
    ) -> Result<ProcessorOutput> {
        self.processor
            .process_one(input, output, self.config, self.ctx, batch)
            .await
    }
}

impl Default for MetadataProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for MetadataProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["metadata", "embed_metadata"]
    }

    fn name(&self) -> &'static str {
        "MetadataProcessor"
    }

    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let config: MetadataConfig =
            super::utils::parse_config_or_default(input.config.as_deref(), ctx, "metadata", None);
        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for metadata embedding".to_owned(),
            ));
        }
        if input.inputs.len() > 1 && config.output_path.is_some() {
            return Err(crate::Error::PipelineError("metadata: config.output_path is not supported for batch inputs; provide outputs[] per input or omit outputs to use generated defaults".to_owned()));
        }
        let plan = super::planning::OutputPlan::unary_or_mapped(&input.inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("metadata batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})", input.inputs.len(), input.outputs.len())))?;
        let is_batch = plan.is_batch();
        let mut output = super::media_driver::run_media(
            plan,
            OutputBatch::new(&input.inputs, ctx),
            &ctx.progress,
            &MetadataProcessorItem {
                processor: self,
                config: &config,
                ctx,
            },
        )
        .await?;
        if is_batch {
            output.metadata = Some(serde_json::json!({
                "batch": true, "inputs": input.inputs.len(), "input_removed": config.remove_input_on_success,
            }).to_string());
        }

        // Source deletion is only allowed after every output has been published.
        if config.remove_input_on_success {
            for input_path in &output.succeeded_inputs {
                match tokio::fs::remove_file(input_path).await {
                    Ok(()) => output
                        .logs
                        .push(crate::pipeline::job_queue::JobLogEntry::info(format!(
                            "Removed input file: {input_path}"
                        ))),
                    Err(error) => {
                        ctx.warn(format!("Failed to remove input file after metadata embedding {input_path}: {error}"));
                        output
                            .logs
                            .push(crate::pipeline::job_queue::JobLogEntry::warn(format!(
                                "Failed to remove input file {input_path}: {error}"
                            )));
                    }
                }
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_args_with_all_standard_fields() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            artist: Some("StreamerName".to_string()),
            title: Some("Stream Title".to_string()),
            date: Some("2024-01-15".to_string()),
            album: Some("Streams 2024".to_string()),
            comment: Some("Recorded live".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        // Verify all metadata fields are present
        assert!(args.contains(&"artist=StreamerName".to_string()));
        assert!(args.contains(&"title=Stream Title".to_string()));
        assert!(args.contains(&"date=2024-01-15".to_string()));
        assert!(args.contains(&"album=Streams 2024".to_string()));
        assert!(args.contains(&"comment=Recorded live".to_string()));
    }

    #[test]
    fn test_build_args_empty_config() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig::default();

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        // Should still have basic structure
        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-hide_banner".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.mp4".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"copy".to_string()));
        assert!(args.contains(&"/output.mp4".to_string()));

        // No metadata flags when no metadata provided
        let metadata_count = args.iter().filter(|a| *a == "-metadata").count();
        assert_eq!(metadata_count, 0);
    }

    #[test]
    fn test_determine_output_path_preserves_extension() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig::default();
        let input = ProcessorInput {
            inputs: vec!["/path/to/video.mkv".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path(
            "/path/to/video.mkv",
            &config,
            input.outputs.first().map(String::as_str),
        );
        assert!(output.contains("video_meta.mkv"));
    }

    #[test]
    fn test_build_args_with_custom_fields() {
        let processor = MetadataProcessor::new();
        let mut custom = HashMap::new();
        custom.insert("genre".to_string(), "Gaming".to_string());
        custom.insert("language".to_string(), "en".to_string());
        let config = MetadataConfig {
            custom,
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        for field in ["genre=Gaming", "language=en"] {
            assert!(
                args.windows(2)
                    .any(|pair| pair[0] == "-metadata" && pair[1] == field),
                "{field}: {args:?}"
            );
        }
    }
}
