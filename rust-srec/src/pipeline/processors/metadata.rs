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
    #[allow(dead_code)]
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
        args.extend(["-loglevel".to_string(), "warning".to_string()]);
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
        processor_input: &ProcessorInput,
    ) -> String {
        // Priority: config.output_path > processor_input.outputs > generated from input
        if let Some(ref output) = config.output_path {
            return output.clone();
        }

        if let Some(output) = processor_input.outputs.first() {
            return output.clone();
        }

        // Generate output path from input path (add _meta suffix)
        let input = Path::new(input_path);
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let extension = input.extension().and_then(|s| s.to_str()).unwrap_or("mp4");
        let parent = input.parent().unwrap_or(Path::new("."));

        parent
            .join(format!("{}_meta.{}", stem, extension))
            .to_string_lossy()
            .to_string()
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
        remove_input_on_success: bool,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Check if input file exists
        if !Path::new(input_path).exists() {
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
            return Ok(ProcessorOutput {
                outputs: vec![input_path.to_string()],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "unsupported_format",
                        "input": input_path,
                    })
                    .to_string(),
                ),
                skipped_inputs: vec![(
                    input_path.to_string(),
                    "format does not support metadata embedding".to_string(),
                )],
                ..Default::default()
            });
        }

        let mut dummy_input = ProcessorInput::default();
        if let Some(output_override) = output_override.filter(|s| !s.is_empty()) {
            dummy_input.outputs = vec![output_override.to_string()];
        }

        // Determine output path
        let output_path = self.determine_output_path(input_path, config, &dummy_input);

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

        // Build ffmpeg arguments
        let args = self.build_args(input_path, &output_path, config);
        debug!("FFmpeg args: {:?}", args);

        // Get input file size for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args(&args).env("LC_ALL", "C");

        // Execute command and capture logs
        let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
            &mut cmd,
            &ctx.progress,
            Some(ctx.log_sink.clone()),
        )
        .await?;

        if !command_output.status.success() {
            // Reconstruct stderr for error analysis
            let stderr_output = command_output
                .logs
                .iter()
                .filter(|l| l.level != crate::pipeline::job_queue::LogLevel::Info)
                .map(|l| l.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");

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
                        .logs
                        .iter()
                        .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                        .map(|l| l.message.clone())
                        .unwrap_or_else(|| "Unknown error".to_string())
                )
            };

            return Err(crate::Error::PipelineError(error_msg));
        }

        // Get output file size for metrics
        let output_size_bytes = tokio::fs::metadata(&output_path)
            .await
            .ok()
            .map(|m| m.len());

        ctx.info(format!(
            "Metadata embedding completed in {:.2}s: {}",
            command_output.duration, output_path
        ));

        let mut logs = command_output.logs;
        if remove_input_on_success {
            match tokio::fs::remove_file(input_path).await {
                Ok(()) => {
                    ctx.info(format!(
                        "Removed input file after successful metadata embedding: {}",
                        input_path
                    ));
                    logs.push(crate::pipeline::job_queue::JobLogEntry::info(format!(
                        "Removed input file: {}",
                        input_path
                    )));
                }
                Err(e) => {
                    ctx.warn(format!(
                        "Failed to remove input file after metadata embedding {}: {}",
                        input_path, e
                    ));
                    logs.push(crate::pipeline::job_queue::JobLogEntry::warn(format!(
                        "Failed to remove input file {}: {}",
                        input_path, e
                    )));
                }
            }
        }

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
            "input_removed": remove_input_on_success,
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
            logs,
        })
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
                "No input file specified for metadata embedding".to_string(),
            ));
        }

        if input.inputs.len() > 1 {
            // Batch mode: config.output_path is ambiguous when multiple inputs exist.
            if config.output_path.is_some() {
                return Err(crate::Error::PipelineError(
                    "metadata: config.output_path is not supported for batch inputs; provide outputs[] per input or omit outputs to use generated defaults".to_string(),
                ));
            }

            if !input.outputs.is_empty() && input.outputs.len() != input.inputs.len() {
                return Err(crate::Error::PipelineError(format!(
                    "metadata batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})",
                    input.inputs.len(),
                    input.outputs.len()
                )));
            }

            let mut outputs = Vec::with_capacity(input.inputs.len());
            let mut items_produced = Vec::new();
            let mut skipped_inputs = Vec::new();
            let mut succeeded_inputs = Vec::new();
            let mut logs = Vec::new();
            let mut duration_secs = 0.0;

            for (idx, input_path) in input.inputs.iter().enumerate() {
                let output_override = input.outputs.get(idx).map(|s| s.as_str());
                match self
                    .process_one(input_path, output_override, &config, ctx, false)
                    .await
                {
                    Ok(one) => {
                        duration_secs += one.duration_secs;
                        outputs.extend(one.outputs);
                        items_produced.extend(one.items_produced);
                        skipped_inputs.extend(one.skipped_inputs);
                        succeeded_inputs.extend(one.succeeded_inputs);
                        logs.extend(one.logs);
                    }
                    Err(e) => {
                        for produced in &items_produced {
                            let _ = tokio::fs::remove_file(produced).await;
                        }
                        return Err(e);
                    }
                }
            }

            if config.remove_input_on_success {
                for input_path in &succeeded_inputs {
                    match tokio::fs::remove_file(input_path).await {
                        Ok(()) => {
                            logs.push(crate::pipeline::job_queue::JobLogEntry::info(format!(
                                "Removed input file: {}",
                                input_path
                            )));
                        }
                        Err(e) => {
                            ctx.warn(format!(
                                "Failed to remove input file after metadata embedding {}: {}",
                                input_path, e
                            ));
                            logs.push(crate::pipeline::job_queue::JobLogEntry::warn(format!(
                                "Failed to remove input file {}: {}",
                                input_path, e
                            )));
                        }
                    }
                }
            }

            return Ok(ProcessorOutput {
                outputs,
                duration_secs,
                metadata: Some(
                    serde_json::json!({
                        "batch": true,
                        "inputs": input.inputs.len(),
                        "input_removed": config.remove_input_on_success,
                    })
                    .to_string(),
                ),
                items_produced,
                input_size_bytes: None,
                output_size_bytes: None,
                failed_inputs: vec![],
                succeeded_inputs,
                skipped_inputs,
                logs,
            });
        }

        // Get input path
        let input_path = input.inputs.first().ok_or_else(|| {
            crate::Error::PipelineError(
                "No input file specified for metadata embedding".to_string(),
            )
        })?;

        let output_override = input.outputs.first().map(|s| s.as_str());
        self.process_one(
            input_path,
            output_override,
            &config,
            ctx,
            config.remove_input_on_success,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_metadata_processor_type() {
        let processor = MetadataProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_metadata_processor_job_types() {
        let processor = MetadataProcessor::new();
        assert!(processor.can_process("metadata"));
        assert!(processor.can_process("embed_metadata"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_metadata_processor_name() {
        let processor = MetadataProcessor::new();
        assert_eq!(processor.name(), "MetadataProcessor");
    }

    #[test]
    fn test_metadata_config_default() {
        let config = MetadataConfig::default();
        assert!(config.artist.is_none());
        assert!(config.title.is_none());
        assert!(config.date.is_none());
        assert!(config.album.is_none());
        assert!(config.comment.is_none());
        assert!(config.custom.is_empty());
        assert!(config.output_path.is_none());
        assert!(config.overwrite);
        assert!(!config.remove_input_on_success);
    }

    #[test]
    fn test_metadata_config_parse() {
        let json = r#"{
            "artist": "StreamerName",
            "title": "Stream Title",
            "date": "2024-01-15",
            "album": "Streams 2024",
            "comment": "Recorded live",
            "custom": {"genre": "Gaming", "language": "en"},
            "output_path": "/output/video_meta.mp4",
            "overwrite": false,
            "remove_input_on_success": true
        }"#;

        let config: MetadataConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.artist, Some("StreamerName".to_string()));
        assert_eq!(config.title, Some("Stream Title".to_string()));
        assert_eq!(config.date, Some("2024-01-15".to_string()));
        assert_eq!(config.album, Some("Streams 2024".to_string()));
        assert_eq!(config.comment, Some("Recorded live".to_string()));
        assert_eq!(config.custom.get("genre"), Some(&"Gaming".to_string()));
        assert_eq!(config.custom.get("language"), Some(&"en".to_string()));
        assert_eq!(
            config.output_path,
            Some("/output/video_meta.mp4".to_string())
        );
        assert!(!config.overwrite);
        assert!(config.remove_input_on_success);
    }

    #[test]
    fn test_metadata_config_parse_minimal() {
        let json = r#"{"artist": "TestStreamer"}"#;
        let config: MetadataConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.artist, Some("TestStreamer".to_string()));
        assert!(config.title.is_none());
        assert!(config.custom.is_empty());
        assert!(config.overwrite); // default
        assert!(!config.remove_input_on_success); // default
    }

    #[test]
    fn test_build_args_with_artist() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            artist: Some("StreamerName".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.mp4".to_string()));
        assert!(args.contains(&"-c".to_string()));
        assert!(args.contains(&"copy".to_string()));
        assert!(args.contains(&"-metadata".to_string()));
        assert!(args.contains(&"artist=StreamerName".to_string()));
        assert!(args.contains(&"/output.mp4".to_string()));
    }

    #[test]
    fn test_build_args_with_title() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            title: Some("My Stream Title".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        assert!(args.contains(&"-metadata".to_string()));
        assert!(args.contains(&"title=My Stream Title".to_string()));
    }

    #[test]
    fn test_build_args_with_date() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            date: Some("2024-01-15".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        assert!(args.contains(&"-metadata".to_string()));
        assert!(args.contains(&"date=2024-01-15".to_string()));
    }

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

        // Custom fields should be present (order may vary)
        assert!(
            args.contains(&"genre=Gaming".to_string())
                || args.iter().any(|a| a.contains("genre=Gaming"))
        );
        assert!(
            args.contains(&"language=en".to_string())
                || args.iter().any(|a| a.contains("language=en"))
        );
    }

    #[test]
    fn test_build_args_no_overwrite() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            overwrite: false,
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp4", &config);

        assert!(!args.contains(&"-y".to_string()));
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
    fn test_supports_metadata_mp4() {
        let processor = MetadataProcessor::new();
        assert!(processor.supports_metadata("/path/to/video.mp4"));
        assert!(processor.supports_metadata("/path/to/video.MP4"));
    }

    #[test]
    fn test_supports_metadata_mkv() {
        let processor = MetadataProcessor::new();
        assert!(processor.supports_metadata("/path/to/video.mkv"));
        assert!(processor.supports_metadata("/path/to/video.MKV"));
    }

    #[test]
    fn test_supports_metadata_audio_formats() {
        let processor = MetadataProcessor::new();
        assert!(processor.supports_metadata("/path/to/audio.mp3"));
        assert!(processor.supports_metadata("/path/to/audio.flac"));
        assert!(processor.supports_metadata("/path/to/audio.ogg"));
        assert!(processor.supports_metadata("/path/to/audio.opus"));
    }

    #[test]
    fn test_supports_metadata_unsupported() {
        let processor = MetadataProcessor::new();
        assert!(!processor.supports_metadata("/path/to/file.txt"));
        assert!(!processor.supports_metadata("/path/to/file.jpg"));
        assert!(!processor.supports_metadata("/path/to/file.png"));
        assert!(!processor.supports_metadata("/path/to/file"));
    }

    #[test]
    fn test_determine_output_path_from_config() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig {
            output_path: Some("/custom/output.mp4".to_string()),
            ..Default::default()
        };
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec!["/processor/output.mp4".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path("/input.mp4", &config, &input);
        assert_eq!(output, "/custom/output.mp4");
    }

    #[test]
    fn test_determine_output_path_from_processor_input() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig::default();
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec!["/processor/output.mp4".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path("/input.mp4", &config, &input);
        assert_eq!(output, "/processor/output.mp4");
    }

    #[test]
    fn test_determine_output_path_generated() {
        let processor = MetadataProcessor::new();
        let config = MetadataConfig::default();
        let input = ProcessorInput {
            inputs: vec!["/path/to/video.mp4".to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path("/path/to/video.mp4", &config, &input);
        assert!(output.contains("video_meta.mp4"));
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

        let output = processor.determine_output_path("/path/to/video.mkv", &config, &input);
        assert!(output.contains("video_meta.mkv"));
    }

    #[tokio::test]
    async fn test_process_no_input_file() {
        let processor = MetadataProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec!["/output.mp4".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No input file"));
    }

    #[tokio::test]
    async fn test_process_input_not_found() {
        let processor = MetadataProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec!["/nonexistent/file.mp4".to_string()],
            outputs: vec!["/output.mp4".to_string()],
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
    async fn test_process_unsupported_format() {
        let processor = MetadataProcessor::new();
        let ctx = ProcessorContext::noop("test");

        // Create a temporary text file
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join("test_metadata.txt");
        tokio::fs::write(&temp_file, "test content").await.unwrap();

        let input = ProcessorInput {
            inputs: vec![temp_file.to_string_lossy().to_string()],
            outputs: vec![],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let result = processor.process(&input, &ctx).await;
        assert!(result.is_ok());

        let output = result.unwrap();
        // Should pass through the input file
        assert_eq!(output.outputs.len(), 1);
        assert_eq!(output.outputs[0], temp_file.to_string_lossy().to_string());
        // Should be marked as skipped
        assert_eq!(output.skipped_inputs.len(), 1);
        assert!(output.skipped_inputs[0].1.contains("metadata"));

        // Cleanup
        let _ = tokio::fs::remove_file(&temp_file).await;
    }

    #[tokio::test]
    async fn test_metadata_batch_outputs_len_mismatch_errors() {
        let processor = MetadataProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
            outputs: vec!["out.mp4".to_string()],
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
    async fn test_metadata_batch_config_output_path_forbidden() {
        let processor = MetadataProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
            outputs: vec![],
            config: Some(serde_json::json!({ "output_path": "out.mp4" }).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("config.output_path"));
    }

    #[tokio::test]
    async fn test_metadata_batch_skips_unsupported_formats() {
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a.txt");
        let b = temp_dir.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let processor = MetadataProcessor::new();
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
