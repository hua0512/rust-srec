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

use super::outputs::{OutputBatch, output_size};
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

/// An audio stream ffprobe found in the input.
struct AudioStream {
    /// `codec_name` as reported; `None` when the probe did not report one.
    codec: Option<String>,
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
        args.extend(["-loglevel".to_string(), "info".to_string()]);
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
    ///
    /// `codec` is the source audio codec reported by ffprobe; a stream copy
    /// takes its extension from it so ffmpeg picks a container that can hold
    /// the stream.
    fn determine_output_path(
        &self,
        input_path: &str,
        config: &AudioExtractConfig,
        output_override: Option<&str>,
        codec: Option<&str>,
    ) -> String {
        super::planning::choose_output(config.output_path.as_deref(), output_override, || {
            let extension = config
                .format
                .as_ref()
                .map(|format| format.extension())
                .unwrap_or_else(|| Self::copy_extension(codec));
            super::planning::sibling_output(Path::new(input_path), "output", "_audio", extension)
        })
    }

    /// Container extension for a stream copy of `codec`. Raw elementary streams
    /// are used where ffmpeg has a muxer for them; anything else goes into
    /// Matroska audio, which accepts every codec ffmpeg can demux.
    fn copy_extension(codec: Option<&str>) -> &'static str {
        match codec {
            Some("aac") => "aac",
            Some("mp3") => "mp3",
            Some("opus") => "opus",
            Some("vorbis") => "ogg",
            Some("flac") => "flac",
            Some("ac3") => "ac3",
            Some("eac3") => "eac3",
            Some("alac") => "m4a",
            Some(codec) if codec.starts_with("pcm_") => "wav",
            _ => "mka",
        }
    }

    /// The first audio stream of the file, or `None` when it has none, using ffprobe.
    async fn probe_audio_codec(&self, input_path: &str) -> Result<Option<AudioStream>> {
        let ffprobe_path = std::env::var("FFPROBE_PATH").unwrap_or_else(|_| "ffprobe".to_string());

        let mut cmd = process_utils::tokio_command(&ffprobe_path);
        cmd.args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_type,codec_name",
            "-of",
            "csv=p=0",
            input_path,
        ]);
        let output = super::utils::run_command_with_logs(&mut cmd, None).await?;
        if !output.status.success() {
            return Err(crate::Error::PipelineError(format!(
                "ffprobe failed with exit status {}",
                output.status
            )));
        }
        // One csv line per selected stream, `codec_name,codec_type`; a build
        // that omits the codec name still identifies the stream by its type.
        Ok(output.logs.iter().find_map(|entry| {
            if entry.level != crate::pipeline::job_queue::LogLevel::Info {
                return None;
            }
            let fields: Vec<&str> = entry.message.trim().split(',').collect();
            if !fields.contains(&"audio") {
                return None;
            }
            Some(AudioStream {
                codec: fields
                    .iter()
                    .find(|field| **field != "audio" && !field.is_empty())
                    .map(|codec| codec.to_string()),
            })
        }))
    }

    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &AudioExtractConfig,
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

        // Check if input is an image - pass through as-is
        if is_image(&ext) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!("Input is an image, passing through: {input_path}"));
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "input is an image, no audio to extract",
                "already_image",
                duration,
                Vec::new(),
            ));
        }

        // Check if input is a supported media format
        if !is_media(&ext) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!(
                "Input file is not a supported media format for audio extraction, passing through: {input_path}"
            ));
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "not a supported media format for audio extraction",
                "unsupported_media_format",
                duration,
                Vec::new(),
            ));
        }

        // Check if input has audio stream
        // If no audio stream, pass through the input file instead of failing
        let codec = match self.probe_audio_codec(input_path).await {
            Ok(Some(stream)) => stream.codec,
            Ok(None) => {
                let duration = start.elapsed().as_secs_f64();
                ctx.info(format!(
                    "Input file contains no audio stream, passing through: {input_path}"
                ));
                return Ok(ProcessorOutput::skipped_file(
                    input_path,
                    "input file contains no audio stream",
                    "no_audio_stream",
                    duration,
                    Vec::new(),
                ));
            }
            Err(e) => {
                // If ffprobe fails, we'll try to extract anyway and let ffmpeg report the error
                warn!("Could not verify audio stream presence: {}", e);
                None
            }
        };

        let output_path = self.determine_output_path(
            input_path,
            config,
            output_override.filter(|s| !s.is_empty()),
            codec.as_deref(),
        );
        let temp_path = batch.stage(Path::new(&output_path), true).await?;

        info!(
            "Extracting audio from {} -> {} (format: {:?})",
            input_path, output_path, config.format
        );

        // Build ffmpeg arguments
        let args = self.build_args(input_path, &temp_path.to_string_lossy(), config);
        debug!("FFmpeg args: {:?}", args);

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

            // Check for no audio stream error - pass through instead of failing
            if stderr_output.contains("does not contain any stream")
                || stderr_output.contains("Output file does not contain any stream")
            {
                batch.discard(&temp_path);
                ctx.info(format!(
                    "Input file contains no audio stream (detected by ffmpeg), passing through: {input_path}"
                ));
                return Ok(ProcessorOutput::skipped_file(
                    input_path,
                    "input file contains no audio stream",
                    "no_audio_stream",
                    command_output.duration,
                    command_output.logs,
                ));
            }

            let error_msg = command_output
                .last_error_message()
                .map(str::to_owned)
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
        let output_size_bytes = Some(output_size(&temp_path).await?);

        // Only return the newly produced audio file (no additive passthrough)
        Ok(ProcessorOutput {
            outputs: vec![output_path.clone()],
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "format": config.format.as_ref().map(|f| format!("{:?}", f)),
                    "source_codec": codec,
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
            uploads: vec![],
            logs: command_output.logs,
        })
    }
}

struct AudioExtractProcessorItem<'a> {
    processor: &'a AudioExtractProcessor,
    config: &'a AudioExtractConfig,
    ctx: &'a ProcessorContext,
}

#[async_trait]
impl super::media_driver::MediaItem for AudioExtractProcessorItem<'_> {
    fn runs_ffmpeg(&self, input: &str) -> bool {
        is_media(&get_extension(input).unwrap_or_default())
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
        let config: AudioExtractConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "audio_extract", None);

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for audio extraction".to_string(),
            ));
        }

        if input.inputs.len() > 1 && config.output_path.is_some() {
            return Err(crate::Error::PipelineError("audio_extract: config.output_path is not supported for batch inputs; provide outputs[] per input or omit outputs to use generated defaults".to_string()));
        }
        let plan = super::planning::OutputPlan::unary_or_mapped(&input.inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("audio_extract batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})", input.inputs.len(), input.outputs.len())))?;
        let is_batch = plan.is_batch();
        let mut output = super::media_driver::run_media(
            plan,
            OutputBatch::new(&input.inputs, ctx),
            &ctx.progress,
            &AudioExtractProcessorItem {
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

        let output = processor.determine_output_path(
            "/path/to/video.mp4",
            &config,
            input.outputs.first().map(String::as_str),
            Some("opus"),
        );
        assert!(output.contains("video_audio.mp3"));
    }

    #[tokio::test]
    #[ignore = "bug: overwrite:false is ignored; publishing always replaces an existing output"]
    async fn known_bug_overwrite_false_keeps_an_existing_output() {
        let dir = tempfile::tempdir().unwrap();
        let ffmpeg = super::super::output_tests::fake_success_ffmpeg(dir.path());
        let input = dir.path().join("video.mp4");
        let output = dir.path().join("video.aac");
        std::fs::write(&input, b"source").unwrap();
        std::fs::write(&output, b"existing").unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            AudioExtractProcessor::with_ffmpeg_path(ffmpeg).process(
                &ProcessorInput {
                    inputs: vec![input.to_string_lossy().into_owned()],
                    outputs: vec![output.to_string_lossy().into_owned()],
                    config: Some(r#"{"overwrite":false}"#.to_string()),
                    ..Default::default()
                },
                &ProcessorContext::noop("test"),
            ),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&output).unwrap(), b"existing", "{result:?}");
    }

    /// A stream copy keeps the source codec, so the generated name must use a
    /// container that can hold it instead of always claiming `.aac`.
    #[test]
    fn stream_copy_extension_follows_the_source_codec() {
        let processor = AudioExtractProcessor::new();
        let config = AudioExtractConfig::default();
        for (codec, extension) in [
            (Some("aac"), "aac"),
            (Some("mp3"), "mp3"),
            (Some("opus"), "opus"),
            (Some("vorbis"), "ogg"),
            (Some("flac"), "flac"),
            (Some("pcm_s16le"), "wav"),
            (Some("something_new"), "mka"),
            (None, "mka"),
        ] {
            let output =
                processor.determine_output_path("/path/to/video.mp4", &config, None, codec);
            assert!(
                output.ends_with(&format!("video_audio.{extension}")),
                "{codec:?}: {output}"
            );
        }
    }
}
