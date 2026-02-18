//! Remux/transcode processor for converting video formats.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info};

use super::traits::{Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, get_extension, is_media, parse_config_or_default};
use crate::Result;

/// Helper to ensure path is absolute.
/// If file exists, uses canonicalize.
/// If not (e.g. new output), uses current_dir + path.
async fn make_absolute(path: &str) -> String {
    let path_obj = Path::new(path);
    if path_obj.is_absolute() {
        return path.to_string();
    }

    if let Ok(true) = tokio::fs::try_exists(path_obj).await
        && let Ok(abs) = tokio::fs::canonicalize(path_obj).await
    {
        return abs.to_string_lossy().into_owned();
    }

    if let Ok(Ok(cwd)) = tokio::task::spawn_blocking(std::env::current_dir).await {
        return cwd.join(path_obj).to_string_lossy().into_owned();
    }

    path.to_string()
}

/// Video codec options.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// Copy video stream without re-encoding.
    #[default]
    Copy,
    /// H.264/AVC codec.
    H264,
    /// H.265/HEVC codec.
    #[serde(alias = "hevc")]
    H265,
    /// VP9 codec.
    Vp9,
    /// AV1 codec.
    Av1,
    /// Custom codec string.
    Custom(String),
}

impl VideoCodec {
    fn as_ffmpeg_args(&self) -> Vec<String> {
        match self {
            Self::Copy => vec!["-c:v".to_string(), "copy".to_string()],
            Self::H264 => vec!["-c:v".to_string(), "libx264".to_string()],
            Self::H265 => vec!["-c:v".to_string(), "libx265".to_string()],
            Self::Vp9 => vec!["-c:v".to_string(), "libvpx-vp9".to_string()],
            Self::Av1 => vec!["-c:v".to_string(), "libaom-av1".to_string()],
            Self::Custom(codec) => vec!["-c:v".to_string(), codec.clone()],
        }
    }
}

/// Audio codec options.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    /// Copy audio stream without re-encoding.
    #[default]
    Copy,
    /// AAC codec.
    Aac,
    /// MP3 codec.
    Mp3,
    /// Opus codec.
    Opus,
    /// FLAC codec (lossless).
    Flac,
    /// No audio.
    None,
    /// Custom codec string.
    Custom(String),
}

impl AudioCodec {
    fn as_ffmpeg_args(&self) -> Vec<String> {
        match self {
            Self::Copy => vec!["-c:a".to_string(), "copy".to_string()],
            Self::Aac => vec!["-c:a".to_string(), "aac".to_string()],
            Self::Mp3 => vec!["-c:a".to_string(), "libmp3lame".to_string()],
            Self::Opus => vec!["-c:a".to_string(), "libopus".to_string()],
            Self::Flac => vec!["-c:a".to_string(), "flac".to_string()],
            Self::None => vec!["-an".to_string()],
            Self::Custom(codec) => vec!["-c:a".to_string(), codec.clone()],
        }
    }
}

/// Video quality preset (for encoding speed vs quality tradeoff).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    #[default]
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl Preset {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Ultrafast => "ultrafast",
            Self::Superfast => "superfast",
            Self::Veryfast => "veryfast",
            Self::Faster => "faster",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
            Self::Slower => "slower",
            Self::Veryslow => "veryslow",
        }
    }
}

/// Configuration for remux/transcode operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemuxConfig {
    /// Video codec to use.
    #[serde(default)]
    pub video_codec: VideoCodec,

    /// Audio codec to use.
    #[serde(default)]
    pub audio_codec: AudioCodec,

    /// Output container format (e.g., "mp4", "mkv", "webm").
    /// If not specified, inferred from output file extension.
    #[serde(default)]
    pub format: Option<String>,

    /// Video bitrate (e.g., "2M", "5000k").
    #[serde(default)]
    pub video_bitrate: Option<String>,

    /// Audio bitrate (e.g., "128k", "320k").
    #[serde(default)]
    pub audio_bitrate: Option<String>,

    /// Constant Rate Factor for quality-based encoding (0-51, lower is better).
    #[serde(default)]
    pub crf: Option<u8>,

    /// Encoding preset (speed vs quality tradeoff).
    #[serde(default)]
    pub preset: Option<Preset>,

    /// Video resolution (e.g., "1920x1080", "1280x720").
    #[serde(default)]
    pub resolution: Option<String>,

    /// Frame rate (e.g., 30, 60).
    #[serde(default)]
    pub fps: Option<f32>,

    /// Start time in seconds (for trimming).
    #[serde(default)]
    pub start_time: Option<f64>,

    /// Duration in seconds (for trimming).
    #[serde(default)]
    pub duration: Option<f64>,

    /// End time in seconds (alternative to duration).
    #[serde(default)]
    pub end_time: Option<f64>,

    /// Video filters (e.g., "scale=1280:720,fps=30").
    #[serde(default)]
    pub video_filter: Option<String>,

    /// Audio filters (e.g., "volume=2.0").
    #[serde(default)]
    pub audio_filter: Option<String>,

    /// Hardware acceleration (e.g., "cuda", "vaapi", "qsv").
    #[serde(default)]
    pub hwaccel: Option<String>,

    /// Additional FFmpeg input options.
    #[serde(default)]
    pub input_options: Vec<String>,

    /// Additional FFmpeg output options.
    #[serde(default)]
    pub output_options: Vec<String>,

    /// Whether to use fast start for MP4 (moves moov atom to beginning).
    #[serde(default = "default_faststart")]
    pub faststart: bool,

    /// Whether to overwrite output file if it exists.
    #[serde(default = "default_overwrite")]
    pub overwrite: bool,

    /// Map specific streams (e.g., ["0:v:0", "0:a:0"]).
    #[serde(default)]
    pub map_streams: Vec<String>,

    /// Metadata to add (key-value pairs).
    #[serde(default)]
    pub metadata: Vec<(String, String)>,

    /// Whether to remove input file after successful remux.
    #[serde(default)]
    pub remove_input_on_success: bool,
}

fn default_faststart() -> bool {
    true
}

fn default_overwrite() -> bool {
    true
}

impl Default for RemuxConfig {
    fn default() -> Self {
        Self {
            video_codec: VideoCodec::Copy,
            audio_codec: AudioCodec::Copy,
            format: None,
            video_bitrate: None,
            audio_bitrate: None,
            crf: None,
            preset: None,
            resolution: None,
            fps: None,
            start_time: None,
            duration: None,
            end_time: None,
            video_filter: None,
            audio_filter: None,
            hwaccel: None,
            input_options: Vec::new(),
            output_options: Vec::new(),
            faststart: true,
            overwrite: true,
            map_streams: Vec::new(),
            metadata: Vec::new(),
            remove_input_on_success: false,
        }
    }
}

/// Processor for remuxing/transcoding video files.
pub struct RemuxProcessor {
    /// Path to ffmpeg binary.
    ffmpeg_path: String,
}

impl RemuxProcessor {
    /// Create a new remux processor.
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

    /// Map user-friendly format names to FFmpeg's internal format names.
    fn normalize_format(format: &str) -> &str {
        match format.to_ascii_lowercase().as_str() {
            "mkv" => "matroska",
            "ts" | "m2ts" | "mts" => "mpegts",
            "mp4" | "m4v" => "mp4",
            "ogg" | "ogv" => "ogg",
            "wmv" | "asf" => "asf",
            _ => format,
        }
    }

    /// Build FFmpeg command arguments from config.
    fn build_args(&self, input_path: &str, config: &RemuxConfig, output_path: &str) -> Vec<String> {
        let mut args = Vec::new();

        // Overwrite flag
        if config.overwrite {
            args.push("-y".to_string());
        }

        args.push("-hide_banner".to_string());
        args.push("-nostats".to_string());
        args.extend(["-loglevel".to_string(), "info".to_string()]);
        args.extend(["-progress".to_string(), "pipe:1".to_string()]);

        // Hardware acceleration
        if let Some(ref hwaccel) = config.hwaccel {
            args.extend(["-hwaccel".to_string(), hwaccel.clone()]);
        }

        // Input options
        for opt in &config.input_options {
            args.push(opt.clone());
        }

        // Start time (before input for faster seeking)
        if let Some(start) = config.start_time {
            args.extend(["-ss".to_string(), format!("{:.3}", start)]);
        }

        // Input file
        args.extend(["-i".to_string(), input_path.to_string()]);

        // Duration or end time
        if let Some(duration) = config.duration {
            args.extend(["-t".to_string(), format!("{:.3}", duration)]);
        } else if let Some(end) = config.end_time {
            args.extend(["-to".to_string(), format!("{:.3}", end)]);
        }

        // Stream mapping
        for map in &config.map_streams {
            args.extend(["-map".to_string(), map.clone()]);
        }

        // Video codec
        args.extend(config.video_codec.as_ffmpeg_args());

        // Audio codec
        args.extend(config.audio_codec.as_ffmpeg_args());

        // Video bitrate
        if let Some(ref bitrate) = config.video_bitrate {
            args.extend(["-b:v".to_string(), bitrate.clone()]);
        }

        // Audio bitrate
        if let Some(ref bitrate) = config.audio_bitrate {
            args.extend(["-b:a".to_string(), bitrate.clone()]);
        }

        // CRF (quality-based encoding)
        if let Some(crf) = config.crf {
            args.extend(["-crf".to_string(), crf.to_string()]);
        }

        // Preset
        if let Some(ref preset) = config.preset {
            args.extend(["-preset".to_string(), preset.as_str().to_string()]);
        }

        // Video filters
        let mut vf_parts = Vec::new();

        if let Some(ref resolution) = config.resolution {
            vf_parts.push(format!("scale={}", resolution.replace('x', ":")));
        }

        if let Some(fps) = config.fps {
            vf_parts.push(format!("fps={}", fps));
        }

        if let Some(ref filter) = config.video_filter {
            vf_parts.push(filter.clone());
        }

        if !vf_parts.is_empty() {
            args.extend(["-vf".to_string(), vf_parts.join(",")]);
        }

        // Audio filters
        if let Some(ref filter) = config.audio_filter {
            args.extend(["-af".to_string(), filter.clone()]);
        }

        // Metadata
        for (key, value) in &config.metadata {
            args.extend(["-metadata".to_string(), format!("{}={}", key, value)]);
        }

        // Output format
        if let Some(ref format) = config.format {
            let normalized = Self::normalize_format(format);
            args.extend(["-f".to_string(), normalized.to_string()]);
        }

        // Fast start for MP4-family containers only (moves moov atom to beginning).
        // Applying `-movflags +faststart` to non-MP4 outputs (e.g. MKV) can cause ffmpeg to fail
        // with "Error opening output files: Invalid argument".
        let output_ext = get_extension(output_path);
        let output_format = config.format.as_deref().or(output_ext.as_deref());
        let faststart_supported = matches!(
            output_format.map(|s| s.to_ascii_lowercase()).as_deref(),
            Some("mp4" | "mov" | "m4v")
        );
        if config.faststart && faststart_supported {
            args.extend(["-movflags".to_string(), "+faststart".to_string()]);
        }

        // Additional output options
        for opt in &config.output_options {
            args.push(opt.clone());
        }

        // Output file
        args.push(output_path.to_string());

        args
    }

    fn paths_equal(a: &str, b: &str) -> bool {
        if cfg!(windows) {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    }

    async fn determine_output_path_for_input(
        input_path: &str,
        config: &RemuxConfig,
        output_override: Option<&str>,
    ) -> Result<String> {
        let input_abs = make_absolute(input_path).await;

        if let Some(out) = output_override.filter(|s| !s.is_empty()) {
            let out_abs = make_absolute(out).await;
            if Self::paths_equal(&input_abs, &out_abs) {
                return Err(crate::Error::PipelineError(
                    "Remux output path must not be the same as the input path (use a different output or omit outputs for an auto-generated path)".to_string(),
                ));
            }
            return Ok(out.to_string());
        }

        // Generate dynamic output path: input_filename.{extension}
        let input_path_obj = std::path::Path::new(input_path);
        let file_stem = input_path_obj
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        let parent = input_path_obj
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));

        // Determine extension based on config format or default to "mp4"
        let ext = config.format.as_deref().unwrap_or("mp4");

        let candidate = parent
            .join(format!("{}.{}", file_stem, ext))
            .to_string_lossy()
            .to_string();
        let candidate_abs = make_absolute(&candidate).await;

        // Avoid in-place remux (ffmpeg cannot safely write to the same path it's reading from).
        if Self::paths_equal(&input_abs, &candidate_abs) {
            return Ok(parent
                .join(format!("{}_remux.{}", file_stem, ext))
                .to_string_lossy()
                .to_string());
        }

        Ok(candidate)
    }

    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &RemuxConfig,
        ctx: &ProcessorContext,
        remove_input_on_success: bool,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        let input_path_string = make_absolute(input_path).await;
        let input_path = input_path_string.as_str();

        // Check if input file exists
        if !Path::new(input_path).exists() {
            return Err(crate::Error::PipelineError(format!(
                "Input file does not exist: {}",
                input_path
            )));
        }

        // Get extension once for reuse
        let ext = get_extension(input_path).unwrap_or_default();

        // Check if input is a supported media format
        // If not supported, pass through the input file instead of failing
        if !is_media(&ext) {
            let duration = start.elapsed().as_secs_f64();
            ctx.info(format!(
                "Input file is not a supported media format for remuxing, passing through: {}",
                input_path
            ));
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
                    "not a supported media format for remuxing".to_string(),
                )],
                ..Default::default()
            });
        }

        // Determine output path: use provided output or generate one dynamically.
        // Ensures we never choose an output path equal to the input path.
        let output_string =
            Self::determine_output_path_for_input(input_path, config, output_override).await?;
        let output_path_string = make_absolute(&output_string).await;
        let output_path = output_path_string.as_str();

        ctx.info(format!(
            "Processing {} -> {} (video: {:?}, audio: {:?})",
            input_path, output_path, config.video_codec, config.audio_codec
        ));

        let args = self.build_args(input_path, config, output_path);
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
            // Find the last error log
            let error_msg = command_output
                .logs
                .iter()
                .rfind(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                .map(|l| l.message.clone())
                .unwrap_or_else(|| "Unknown ffmpeg error".to_string());

            ctx.error(format!("ffmpeg failed: {}", error_msg));

            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        ctx.info(format!(
            "Processing completed in {:.2}s: {}",
            command_output.duration, output_path
        ));

        // Remove input file if requested and successful
        let mut logs = command_output.logs;
        if remove_input_on_success {
            match tokio::fs::remove_file(input_path).await {
                Ok(_) => {
                    info!("Removed input file after successful remux: {}", input_path);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Info,
                        format!("Removed input file: {}", input_path),
                    ));
                }
                Err(e) => {
                    ctx.warn(format!("Failed to remove input file {}: {}", input_path, e));
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Warn,
                        format!("Failed to remove input file {}: {}", input_path, e),
                    ));
                }
            }
        }

        // Get file sizes for metrics
        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());
        let output_size_bytes = tokio::fs::metadata(output_path).await.ok().map(|m| m.len());

        // Only return the newly produced output file (no additive passthrough)
        let outputs = vec![output_path.to_string()];

        Ok(ProcessorOutput {
            outputs,
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "video_codec": format!("{:?}", config.video_codec),
                    "audio_codec": format!("{:?}", config.audio_codec),
                    "input_removed": remove_input_on_success,
                })
                .to_string(),
            ),
            items_produced: vec![output_path.to_string()],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            logs,
        })
    }
}

impl Default for RemuxProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Processor for RemuxProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["remux", "transcode", "convert"]
    }

    fn name(&self) -> &'static str {
        "RemuxProcessor"
    }

    fn supports_batch_input(&self) -> bool {
        true
    }

    async fn process(
        &self,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> Result<ProcessorOutput> {
        let config: RemuxConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "remux", None);

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for remuxing".to_string(),
            ));
        }

        if input.inputs.len() == 1 {
            let input_path = input.inputs[0].as_str();
            let output_override = input.outputs.first().map(|s| s.as_str());
            return self
                .process_one(
                    input_path,
                    output_override,
                    &config,
                    ctx,
                    config.remove_input_on_success,
                )
                .await;
        }

        // Batch mode: map remux over each input.
        // Output mapping contract:
        // - if `outputs` is empty: generate output next to each input
        // - if `outputs.len() == inputs.len()`: map by index
        // - otherwise: error (ambiguous)
        if !input.outputs.is_empty() && input.outputs.len() != input.inputs.len() {
            return Err(crate::Error::PipelineError(format!(
                "Remux batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})",
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

        // Keep input files until *all* remuxes succeed, then optionally remove them at the end.
        for (idx, input_path) in input.inputs.iter().enumerate() {
            let output_override = input
                .outputs
                .get(idx)
                .map(|s| s.as_str())
                .filter(|s| !s.is_empty());

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
                    // Best-effort cleanup of any produced outputs in this batch to avoid leaving partial artifacts.
                    for produced in &items_produced {
                        let _ = tokio::fs::remove_file(produced).await;
                    }
                    return Err(e);
                }
            }
        }

        if config.remove_input_on_success {
            // Only remove inputs that were actually remuxed. Skipped inputs should never be removed.
            for input_path in &succeeded_inputs {
                let input_path_string = make_absolute(input_path).await;
                let input_path = input_path_string.as_str();
                if let Err(e) = tokio::fs::remove_file(input_path).await {
                    let _ = ctx.warn(format!("Failed to remove input file {}: {}", input_path, e));
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Warn,
                        format!("Failed to remove input file {}: {}", input_path, e),
                    ));
                } else {
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Info,
                        format!("Removed input file: {}", input_path),
                    ));
                }
            }
        }

        Ok(ProcessorOutput {
            outputs,
            duration_secs,
            metadata: Some(
                serde_json::json!({
                    "batch": true,
                    "inputs": input.inputs.len(),
                    "input_removed": config.remove_input_on_success,
                    "video_codec": format!("{:?}", config.video_codec),
                    "audio_codec": format!("{:?}", config.audio_codec),
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_remux_processor_type() {
        let processor = RemuxProcessor::new();
        assert_eq!(processor.processor_type(), ProcessorType::Cpu);
    }

    #[test]
    fn test_remux_processor_job_types() {
        let processor = RemuxProcessor::new();
        assert!(processor.can_process("remux"));
        assert!(processor.can_process("transcode"));
        assert!(processor.can_process("convert"));
        assert!(!processor.can_process("upload"));
    }

    #[test]
    fn test_remux_processor_name() {
        let processor = RemuxProcessor::new();
        assert_eq!(processor.name(), "RemuxProcessor");
    }

    #[test]
    fn test_remux_config_default() {
        let config = RemuxConfig::default();
        assert!(matches!(config.video_codec, VideoCodec::Copy));
        assert!(matches!(config.audio_codec, AudioCodec::Copy));
        assert!(config.faststart);
        assert!(config.overwrite);
    }

    #[test]
    fn test_remux_config_parse() {
        let json = r#"{
            "video_codec": "h264",
            "audio_codec": "aac",
            "video_bitrate": "5M",
            "audio_bitrate": "192k",
            "crf": 23,
            "preset": "fast",
            "resolution": "1920x1080",
            "fps": 30.0
        }"#;

        let config: RemuxConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.video_codec, VideoCodec::H264));
        assert!(matches!(config.audio_codec, AudioCodec::Aac));
        assert_eq!(config.video_bitrate, Some("5M".to_string()));
        assert_eq!(config.crf, Some(23));
        assert_eq!(config.fps, Some(30.0));
    }

    #[test]
    fn test_video_codec_args() {
        assert_eq!(VideoCodec::Copy.as_ffmpeg_args(), vec!["-c:v", "copy"]);
        assert_eq!(VideoCodec::H264.as_ffmpeg_args(), vec!["-c:v", "libx264"]);
        assert_eq!(VideoCodec::H265.as_ffmpeg_args(), vec!["-c:v", "libx265"]);
    }

    #[test]
    fn test_audio_codec_args() {
        assert_eq!(AudioCodec::Copy.as_ffmpeg_args(), vec!["-c:a", "copy"]);
        assert_eq!(AudioCodec::Aac.as_ffmpeg_args(), vec!["-c:a", "aac"]);
        assert_eq!(AudioCodec::None.as_ffmpeg_args(), vec!["-an"]);
    }

    #[test]
    fn test_build_args_simple() {
        let processor = RemuxProcessor::new();
        let config = RemuxConfig::default();

        let args = processor.build_args("/input.flv", &config, "/output.mp4");

        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.flv".to_string()));
        assert!(args.contains(&"/output.mp4".to_string()));
        assert!(args.contains(&"-c:v".to_string()));
        assert!(args.contains(&"copy".to_string()));
    }

    #[test]
    fn test_build_args_faststart_only_applies_to_mp4_family_outputs() {
        let processor = RemuxProcessor::new();

        // MP4 should include faststart by default.
        let mp4_config = RemuxConfig {
            format: Some("mp4".to_string()),
            ..Default::default()
        };
        let mp4_args = processor.build_args("/input.flv", &mp4_config, "/output.mp4");
        assert!(mp4_args.contains(&"-movflags".to_string()));
        assert!(mp4_args.contains(&"+faststart".to_string()));

        // MKV must not include faststart (movflags is not applicable).
        let mkv_config = RemuxConfig {
            format: Some("mkv".to_string()),
            ..Default::default()
        };
        let mkv_args = processor.build_args("/input.flv", &mkv_config, "/output.mkv");
        assert!(!mkv_args.contains(&"-movflags".to_string()));
        assert!(!mkv_args.contains(&"+faststart".to_string()));
    }

    #[test]
    fn test_build_args_with_transcode() {
        let processor = RemuxProcessor::new();
        let config = RemuxConfig {
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Aac,
            crf: Some(23),
            preset: Some(Preset::Fast),
            resolution: Some("1280x720".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.flv", &config, "/output.mp4");

        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"aac".to_string()));
        assert!(args.contains(&"-crf".to_string()));
        assert!(args.contains(&"23".to_string()));
        assert!(args.contains(&"-preset".to_string()));
        assert!(args.contains(&"fast".to_string()));
        assert!(args.contains(&"-vf".to_string()));
    }
    #[tokio::test]
    async fn test_make_absolute_path() {
        let cwd = std::env::current_dir().unwrap();

        // Test absolute path
        let abs = if cfg!(windows) {
            "C:\\test\\file.txt"
        } else {
            "/test/file.txt"
        };
        assert_eq!(make_absolute(abs).await, abs);

        // Test relative path (non-existent) - should join with CWD
        let rel = "test_file.txt";
        let expected = cwd.join(rel).to_string_lossy().to_string();
        assert_eq!(make_absolute(rel).await, expected);
    }

    #[tokio::test]
    async fn test_remux_batch_outputs_len_mismatch_errors() {
        let processor = RemuxProcessor::new();
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
    async fn test_remux_batch_skips_unsupported_formats() {
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a.txt");
        let b = temp_dir.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let processor = RemuxProcessor::new();
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

    #[tokio::test]
    async fn test_remux_batch_does_not_remove_skipped_inputs() {
        let temp_dir = TempDir::new().unwrap();
        let a = temp_dir.path().join("a.txt");
        let b = temp_dir.path().join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let processor = RemuxProcessor::new();
        let ctx = ProcessorContext::noop("test");

        let input = ProcessorInput {
            inputs: vec![
                a.to_string_lossy().to_string(),
                b.to_string_lossy().to_string(),
            ],
            outputs: vec![],
            config: Some(serde_json::json!({ "remove_input_on_success": true }).to_string()),
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
            ..Default::default()
        };

        let output = processor.process(&input, &ctx).await.unwrap();
        assert_eq!(output.outputs.len(), 2);
        assert!(a.exists());
        assert!(b.exists());
    }

    #[tokio::test]
    async fn test_determine_output_path_avoids_in_place_collision() {
        let temp_dir = TempDir::new().unwrap();
        let input_path = temp_dir.path().join("a.mp4");
        fs::write(&input_path, "dummy").unwrap();

        let config = RemuxConfig::default(); // format=None => default "mp4"
        let output = RemuxProcessor::determine_output_path_for_input(
            &input_path.to_string_lossy(),
            &config,
            None,
        )
        .await
        .unwrap();

        assert!(output.ends_with("_remux.mp4"));
        assert_ne!(
            make_absolute(&output).await,
            make_absolute(&input_path.to_string_lossy()).await
        );
    }
}
