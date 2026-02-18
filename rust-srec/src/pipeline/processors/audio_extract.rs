//! Audio Extraction processor for extracting audio from video files.
//!
//! This processor extracts audio streams from video files using ffmpeg,
//! with support for various output formats (MP3, AAC, FLAC, Opus) or
//! stream copy without re-encoding.
//!

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{get_extension, is_image, is_media, parse_config_or_default};
use crate::Result;

/// Audio output format options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// MP3 format using libmp3lame encoder.
    Mp3,
    /// AAC format.
    Aac,
    /// FLAC format (lossless).
    Flac,
    /// Opus format using libopus encoder.
    Opus,
}

impl AudioFormat {
    /// Get the ffmpeg codec argument for this format.
    fn codec_arg(&self) -> &'static str {
        match self {
            Self::Mp3 => "libmp3lame",
            Self::Aac => "aac",
            Self::Flac => "flac",
            Self::Opus => "libopus",
        }
    }

    /// Get the default file extension for this format.
    fn extension(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
            Self::Flac => "flac",
            Self::Opus => "opus",
        }
    }
}

/// Configuration for audio extraction operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioExtractConfig {
    /// Output format (mp3, aac, flac, opus).
    /// If not specified, audio is extracted without re-encoding (stream copy).
    pub format: Option<AudioFormat>,

    /// Audio bitrate (e.g., "128k", "320k").
    /// Only applicable when transcoding (format is specified).
    pub bitrate: Option<String>,

    /// Sample rate in Hz (e.g., 44100, 48000).
    /// Only applicable when transcoding.
    pub sample_rate: Option<u32>,

    /// Number of audio channels (e.g., 1 for mono, 2 for stereo).
    /// Only applicable when transcoding.
    pub channels: Option<u8>,

    /// Output file path. If not specified, uses the first output from ProcessorInput
    /// or generates one based on input filename.
    pub output_path: Option<String>,

    /// Whether to overwrite existing output file.
    #[serde(default = "default_true")]
    pub overwrite: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AudioExtractConfig {
    fn default() -> Self {
        Self {
            format: None,
            bitrate: None,
            sample_rate: None,
            channels: None,
            output_path: None,
            overwrite: true,
        }
    }
}

/// Processor for extracting audio from video files.
///
/// Uses ffmpeg to extract audio streams with optional transcoding.
/// - When format is specified: transcodes to the target format
/// - When format is None: copies audio stream without re-encoding
pub struct AudioExtractProcessor {
    /// Path to ffmpeg binary.
    ffmpeg_path: String,
}

impl AudioExtractProcessor {
    /// Create a new audio extraction processor.
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

    /// Build FFmpeg command arguments for audio extraction.
    pub fn build_args(
        &self,
        input_path: &str,
        output_path: &str,
        config: &AudioExtractConfig,
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

        // No video output
        args.push("-vn".to_string());

        // Audio codec
        if let Some(ref format) = config.format {
            // Transcode to specified format
            args.extend(["-c:a".to_string(), format.codec_arg().to_string()]);
        } else {
            // Copy audio stream without re-encoding
            args.extend(["-c:a".to_string(), "copy".to_string()]);
        }

        // Bitrate (only when transcoding)
        if config.format.is_some()
            && let Some(ref bitrate) = config.bitrate
        {
            args.extend(["-b:a".to_string(), bitrate.clone()]);
        }

        // Sample rate (only when transcoding)
        if config.format.is_some()
            && let Some(sample_rate) = config.sample_rate
        {
            args.extend(["-ar".to_string(), sample_rate.to_string()]);
        }

        // Channels (only when transcoding)
        if config.format.is_some()
            && let Some(channels) = config.channels
        {
            args.extend(["-ac".to_string(), channels.to_string()]);
        }

        // Output file
        args.push(output_path.to_string());

        args
    }

    /// Determine the output file path based on config and input.
    fn determine_output_path(
        &self,
        input_path: &str,
        config: &AudioExtractConfig,
        processor_input: &ProcessorInput,
    ) -> String {
        // Priority: config.output_path > processor_input.outputs > generated from input
        if let Some(ref output) = config.output_path {
            return output.clone();
        }

        if let Some(output) = processor_input.outputs.first() {
            return output.clone();
        }

        // Generate output path from input path
        let input = Path::new(input_path);
        let stem = input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let parent = input.parent().unwrap_or(Path::new("."));

        let extension = config
            .format
            .as_ref()
            .map(|f| f.extension())
            .unwrap_or("aac"); // Default to aac for stream copy

        parent
            .join(format!("{}_audio.{}", stem, extension))
            .to_string_lossy()
            .to_string()
    }

    /// Check if the input file has an audio stream using ffprobe.
    async fn has_audio_stream(&self, input_path: &str) -> Result<bool> {
        let ffprobe_path = std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".to_string());

        let mut cmd = process_utils::tokio_command(&ffprobe_path);
        cmd.args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type",
            "-of",
            "csv=p=0",
            input_path,
        ]);
        let output = cmd
            .output()
            .await
            .map_err(|e| crate::Error::Other(format!("Failed to run ffprobe: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim() == "audio")
    }

    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &AudioExtractConfig,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Check if input file exists
        if !Path::new(input_path).exists() {
            return Err(crate::Error::PipelineError(format!(
                "Input file does not exist: {}",
                input_path
            )));
        }

        // Get extension once for reuse
        let ext = get_extension(input_path).unwrap_or_default();

        // Check if input is an image - pass through as-is
        if is_image(&ext) {
            let duration = start.elapsed().as_secs_f64();
            info!("Input is an image, passing through: {}", input_path);
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
                    "input is an image, no audio to extract".to_string(),
                )],
                ..Default::default()
            });
        }

        // Check if input is a supported media format
        if !is_media(&ext) {
            let duration = start.elapsed().as_secs_f64();
            info!(
                "Input file is not a supported media format for audio extraction, passing through: {}",
                input_path
            );
            return Ok(ProcessorOutput {
                outputs: vec![input_path.to_string()],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "unsupported_media_format",
                        "input": input_path,
                    })
                    .to_string(),
                ),
                skipped_inputs: vec![(
                    input_path.to_string(),
                    "not a supported media format for audio extraction".to_string(),
                )],
                ..Default::default()
            });
        }

        // Check if input has audio stream
        // If no audio stream, pass through the input file instead of failing
        match self.has_audio_stream(input_path).await {
            Ok(true) => {}
            Ok(false) => {
                let duration = start.elapsed().as_secs_f64();
                info!(
                    "Input file contains no audio stream, passing through: {}",
                    input_path
                );
                return Ok(ProcessorOutput {
                    outputs: vec![input_path.to_string()],
                    duration_secs: duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": "no_audio_stream",
                            "input": input_path,
                        })
                        .to_string(),
                    ),
                    skipped_inputs: vec![(
                        input_path.to_string(),
                        "input file contains no audio stream".to_string(),
                    )],
                    ..Default::default()
                });
            }
            Err(e) => {
                // If ffprobe fails, we'll try to extract anyway and let ffmpeg report the error
                warn!("Could not verify audio stream presence: {}", e);
            }
        }

        let mut dummy_input = ProcessorInput::default();
        if let Some(output_override) = output_override.filter(|s| !s.is_empty()) {
            dummy_input.outputs = vec![output_override.to_string()];
        }

        // Determine output path
        let output_path = self.determine_output_path(input_path, config, &dummy_input);

        info!(
            "Extracting audio from {} -> {} (format: {:?})",
            input_path, output_path, config.format
        );

        // Build ffmpeg arguments
        let args = self.build_args(input_path, &output_path, config);
        debug!("FFmpeg args: {:?}", args);

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
                .filter(|l| l.level != crate::pipeline::job_queue::LogLevel::Info) // Assuming warnings/errors are interested
                .map(|l| l.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            // Check for no audio stream error - pass through instead of failing
            if stderr_output.contains("does not contain any stream")
                || stderr_output.contains("Output file does not contain any stream")
            {
                info!(
                    "Input file contains no audio stream (detected by ffmpeg), passing through: {}",
                    input_path
                );
                return Ok(ProcessorOutput {
                    outputs: vec![input_path.to_string()],
                    duration_secs: command_output.duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": "no_audio_stream",
                            "input": input_path,
                        })
                        .to_string(),
                    ),
                    skipped_inputs: vec![(
                        input_path.to_string(),
                        "input file contains no audio stream".to_string(),
                    )],
                    logs: command_output.logs,
                    ..Default::default()
                });
            }

            let error_msg = command_output
                .logs
                .iter()
                .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                .map(|l| l.message.clone())
                .unwrap_or_else(|| "Unknown ffmpeg error".to_string());

            error!("ffmpeg failed: {}", error_msg);

            return Err(crate::Error::PipelineError(format!(
                "ffmpeg failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        info!(
            "Audio extraction completed in {:.2}s: {}",
            command_output.duration, output_path
        );

        // Get file sizes for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let output_size_bytes = tokio::fs::metadata(&output_path)
            .await
            .ok()
            .map(|m| m.len());

        // Only return the newly produced audio file (no additive passthrough)
        Ok(ProcessorOutput {
            outputs: vec![output_path.clone()],
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "format": config.format.as_ref().map(|f| format!("{:?}", f)),
                    "bitrate": config.bitrate,
                    "sample_rate": config.sample_rate,
                    "channels": config.channels,
                    "input": input_path,
                    "output": output_path,
                })
                .to_string(),
            ),
            items_produced: vec![output_path],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            logs: command_output.logs,
        })
    }
}

impl Default for AudioExtractProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for AudioExtractProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["audio_extract", "extract_audio"]
    }

    fn name(&self) -> &'static str {
        "AudioExtractProcessor"
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

        let config: AudioExtractConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "audio_extract", None);

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for audio extraction".to_string(),
            ));
        }

        if input.inputs.len() > 1 {
            // Batch mode: output_path is ambiguous when multiple inputs exist.
            if config.output_path.is_some() {
                return Err(crate::Error::PipelineError(
                    "audio_extract: config.output_path is not supported for batch inputs; provide outputs[] per input or omit outputs to use generated defaults".to_string(),
                ));
            }

            if !input.outputs.is_empty() && input.outputs.len() != input.inputs.len() {
                return Err(crate::Error::PipelineError(format!(
                    "audio_extract batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})",
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
                    .process_one(input_path, output_override, &config, ctx)
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

            return Ok(ProcessorOutput {
                outputs,
                duration_secs,
                metadata: Some(
                    serde_json::json!({
                        "batch": true,
                        "inputs": input.inputs.len(),
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
            crate::Error::PipelineError("No input file specified for audio extraction".to_string())
        })?;

        // Check if input file exists
        if !Path::new(input_path).exists() {
            return Err(crate::Error::PipelineError(format!(
                "Input file does not exist: {}",
                input_path
            )));
        }

        // Get extension once for reuse
        let ext = get_extension(input_path).unwrap_or_default();

        // Check if input is an image - pass through as-is
        if is_image(&ext) {
            let duration = start.elapsed().as_secs_f64();
            info!("Input is an image, passing through: {}", input_path);
            return Ok(ProcessorOutput {
                outputs: vec![input_path.clone()],
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
                    input_path.clone(),
                    "input is an image, no audio to extract".to_string(),
                )],
                ..Default::default()
            });
        }

        // Check if input is a supported media format
        if !is_media(&ext) {
            let duration = start.elapsed().as_secs_f64();
            info!(
                "Input file is not a supported media format for audio extraction, passing through: {}",
                input_path
            );
            return Ok(ProcessorOutput {
                outputs: vec![input_path.clone()],
                duration_secs: duration,
                metadata: Some(
                    serde_json::json!({
                        "status": "skipped",
                        "reason": "unsupported_media_format",
                        "input": input_path,
                    })
                    .to_string(),
                ),
                skipped_inputs: vec![(
                    input_path.clone(),
                    "not a supported media format for audio extraction".to_string(),
                )],
                ..Default::default()
            });
        }

        // Check if input has audio stream
        // If no audio stream, pass through the input file instead of failing
        match self.has_audio_stream(input_path).await {
            Ok(true) => {}
            Ok(false) => {
                let duration = start.elapsed().as_secs_f64();
                info!(
                    "Input file contains no audio stream, passing through: {}",
                    input_path
                );
                return Ok(ProcessorOutput {
                    outputs: vec![input_path.clone()],
                    duration_secs: duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": "no_audio_stream",
                            "input": input_path,
                        })
                        .to_string(),
                    ),
                    skipped_inputs: vec![(
                        input_path.clone(),
                        "input file contains no audio stream".to_string(),
                    )],
                    ..Default::default()
                });
            }
            Err(e) => {
                // If ffprobe fails, we'll try to extract anyway and let ffmpeg report the error
                warn!("Could not verify audio stream presence: {}", e);
            }
        }

        // Determine output path
        let output_path = self.determine_output_path(input_path, &config, input);

        info!(
            "Extracting audio from {} -> {} (format: {:?})",
            input_path, output_path, config.format
        );

        // Build ffmpeg arguments
        let args = self.build_args(input_path, &output_path, &config);
        debug!("FFmpeg args: {:?}", args);

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
                .filter(|l| l.level != crate::pipeline::job_queue::LogLevel::Info) // Assuming warnings/errors are interested
                .map(|l| l.message.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            // Check for no audio stream error - pass through instead of failing
            if stderr_output.contains("does not contain any stream")
                || stderr_output.contains("Output file does not contain any stream")
            {
                info!(
                    "Input file contains no audio stream (detected by ffmpeg), passing through: {}",
                    input_path
                );
                return Ok(ProcessorOutput {
                    outputs: vec![input_path.to_string()],
                    duration_secs: command_output.duration,
                    metadata: Some(
                        serde_json::json!({
                            "status": "skipped",
                            "reason": "no_audio_stream",
                            "input": input_path,
                        })
                        .to_string(),
                    ),
                    skipped_inputs: vec![(
                        input_path.to_string(),
                        "input file contains no audio stream".to_string(),
                    )],
                    logs: command_output.logs,
                    ..Default::default()
                });
            }

            let error_msg = command_output
                .logs
                .iter()
                .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                .map(|l| l.message.clone())
                .unwrap_or_else(|| "Unknown ffmpeg error".to_string());

            error!("ffmpeg failed: {}", error_msg);

            return Err(crate::Error::PipelineError(format!(
                "ffmpeg failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        info!(
            "Audio extraction completed in {:.2}s: {}",
            command_output.duration, output_path
        );

        // Get file sizes for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let output_size_bytes = tokio::fs::metadata(&output_path)
            .await
            .ok()
            .map(|m| m.len());

        // Only return the newly produced audio file (no additive passthrough)
        Ok(ProcessorOutput {
            outputs: vec![output_path.clone()],
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "format": config.format.as_ref().map(|f| format!("{:?}", f)),
                    "bitrate": config.bitrate,
                    "sample_rate": config.sample_rate,
                    "channels": config.channels,
                    "input": input_path,
                    "output": output_path,
                })
                .to_string(),
            ),
            items_produced: vec![output_path],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.clone()],
            skipped_inputs: vec![],
            logs: command_output.logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_audio_extract_processor_type() {
        let processor = AudioExtractProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_audio_extract_processor_job_types() {
        let processor = AudioExtractProcessor::new();
        assert!(processor.can_process("audio_extract"));
        assert!(processor.can_process("extract_audio"));
        assert!(!processor.can_process("remux"));
    }

    #[test]
    fn test_audio_extract_processor_name() {
        let processor = AudioExtractProcessor::new();
        assert_eq!(processor.name(), "AudioExtractProcessor");
    }

    #[test]
    fn test_audio_format_codec_args() {
        assert_eq!(AudioFormat::Mp3.codec_arg(), "libmp3lame");
        assert_eq!(AudioFormat::Aac.codec_arg(), "aac");
        assert_eq!(AudioFormat::Flac.codec_arg(), "flac");
        assert_eq!(AudioFormat::Opus.codec_arg(), "libopus");
    }

    #[test]
    fn test_audio_format_extensions() {
        assert_eq!(AudioFormat::Mp3.extension(), "mp3");
        assert_eq!(AudioFormat::Aac.extension(), "m4a");
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        assert_eq!(AudioFormat::Opus.extension(), "opus");
    }

    #[test]
    fn test_audio_extract_config_default() {
        let config = AudioExtractConfig::default();
        assert!(config.format.is_none());
        assert!(config.bitrate.is_none());
        assert!(config.sample_rate.is_none());
        assert!(config.channels.is_none());
        assert!(config.output_path.is_none());
        assert!(config.overwrite);
    }

    #[test]
    fn test_audio_extract_config_parse() {
        let json = r#"{
            "format": "mp3",
            "bitrate": "320k",
            "sample_rate": 44100,
            "channels": 2,
            "output_path": "/output/audio.mp3",
            "overwrite": false
        }"#;

        let config: AudioExtractConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, Some(AudioFormat::Mp3));
        assert_eq!(config.bitrate, Some("320k".to_string()));
        assert_eq!(config.sample_rate, Some(44100));
        assert_eq!(config.channels, Some(2));
        assert_eq!(config.output_path, Some("/output/audio.mp3".to_string()));
        assert!(!config.overwrite);
    }

    #[test]
    fn test_audio_extract_config_parse_all_formats() {
        // Test MP3
        let json = r#"{"format": "mp3"}"#;
        let config: AudioExtractConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, Some(AudioFormat::Mp3));

        // Test AAC
        let json = r#"{"format": "aac"}"#;
        let config: AudioExtractConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, Some(AudioFormat::Aac));

        // Test FLAC
        let json = r#"{"format": "flac"}"#;
        let config: AudioExtractConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, Some(AudioFormat::Flac));

        // Test Opus
        let json = r#"{"format": "opus"}"#;
        let config: AudioExtractConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format, Some(AudioFormat::Opus));
    }

    #[test]
    fn test_build_args_stream_copy() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig::default();

        let args = processor.build_args("/input.mp4", "/output.aac", &config);

        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.mp4".to_string()));
        assert!(args.contains(&"-vn".to_string())); // No video
        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"copy".to_string())); // Stream copy
        assert!(args.contains(&"/output.aac".to_string()));
    }

    #[test]
    fn test_build_args_mp3_format() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Mp3),
            bitrate: Some("320k".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp3", &config);

        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"libmp3lame".to_string()));
        assert!(args.contains(&"-b:a".to_string()));
        assert!(args.contains(&"320k".to_string()));
    }

    #[test]
    fn test_build_args_aac_format() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Aac),
            bitrate: Some("256k".to_string()),
            sample_rate: Some(48000),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.m4a", &config);

        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"aac".to_string()));
        assert!(args.contains(&"-b:a".to_string()));
        assert!(args.contains(&"256k".to_string()));
        assert!(args.contains(&"-ar".to_string()));
        assert!(args.contains(&"48000".to_string()));
    }

    #[test]
    fn test_build_args_flac_format() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Flac),
            sample_rate: Some(96000),
            channels: Some(2),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.flac", &config);

        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"flac".to_string()));
        assert!(args.contains(&"-ar".to_string()));
        assert!(args.contains(&"96000".to_string()));
        assert!(args.contains(&"-ac".to_string()));
        assert!(args.contains(&"2".to_string()));
    }

    #[test]
    fn test_build_args_opus_format() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Opus),
            bitrate: Some("128k".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.opus", &config);

        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"libopus".to_string()));
        assert!(args.contains(&"-b:a".to_string()));
        assert!(args.contains(&"128k".to_string()));
    }

    #[test]
    fn test_build_args_no_overwrite() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            overwrite: false,
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.aac", &config);

        assert!(!args.contains(&"-y".to_string()));
    }

    #[test]
    fn test_build_args_with_all_options() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Mp3),
            bitrate: Some("192k".to_string()),
            sample_rate: Some(44100),
            channels: Some(1),
            overwrite: true,
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.mp3", &config);

        // Verify all options are present
        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-hide_banner".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.mp4".to_string()));
        assert!(args.contains(&"-vn".to_string()));
        assert!(args.contains(&"-c:a".to_string()));
        assert!(args.contains(&"libmp3lame".to_string()));
        assert!(args.contains(&"-b:a".to_string()));
        assert!(args.contains(&"192k".to_string()));
        assert!(args.contains(&"-ar".to_string()));
        assert!(args.contains(&"44100".to_string()));
        assert!(args.contains(&"-ac".to_string()));
        assert!(args.contains(&"1".to_string()));
        assert!(args.contains(&"/output.mp3".to_string()));
    }

    #[test]
    fn test_build_args_bitrate_ignored_for_copy() {
        // Bitrate should be ignored when doing stream copy
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: None,                      // Stream copy
            bitrate: Some("320k".to_string()), // Should be ignored
            ..Default::default()
        };

        let args = processor.build_args("/input.mp4", "/output.aac", &config);

        assert!(args.contains(&"copy".to_string()));
        assert!(!args.contains(&"-b:a".to_string())); // Bitrate not applied
    }

    #[test]
    fn test_determine_output_path_from_config() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            output_path: Some("/custom/output.mp3".to_string()),
            ..Default::default()
        };
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec!["/processor/output.mp3".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path("/input.mp4", &config, &input);
        assert_eq!(output, "/custom/output.mp3");
    }

    #[test]
    fn test_determine_output_path_from_processor_input() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig::default();
        let input = ProcessorInput {
            inputs: vec!["/input.mp4".to_string()],
            outputs: vec!["/processor/output.mp3".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.determine_output_path("/input.mp4", &config, &input);
        assert_eq!(output, "/processor/output.mp3");
    }

    #[test]
    fn test_determine_output_path_generated() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig {
            format: Some(AudioFormat::Mp3),
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

        let output = processor.determine_output_path("/path/to/video.mp4", &config, &input);
        assert!(output.contains("video_audio.mp3"));
    }

    #[tokio::test]
    async fn test_process_no_input_file() {
        let processor = AudioExtractProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec![],
            outputs: vec!["/output.mp3".to_string()],
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
        let processor = AudioExtractProcessor::new();
        let ctx = ProcessorContext::noop("test");
        let input = ProcessorInput {
            inputs: vec!["/nonexistent/file.mp4".to_string()],
            outputs: vec!["/output.mp3".to_string()],
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
    async fn test_audio_extract_batch_outputs_len_mismatch_errors() {
        let processor = AudioExtractProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
            outputs: vec!["out.mp3".to_string()],
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
    async fn test_audio_extract_batch_config_output_path_forbidden() {
        let processor = AudioExtractProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec!["a.txt".to_string(), "b.txt".to_string()],
            outputs: vec![],
            config: Some(serde_json::json!({ "output_path": "out.mp3" }).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let err = processor.process(&input, &ctx).await.unwrap_err();
        assert!(err.to_string().contains("config.output_path"));
    }

    #[tokio::test]
    async fn test_audio_extract_batch_skips_unsupported_formats() {
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a.txt");
        let b = temp_dir.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let processor = AudioExtractProcessor::new();
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
