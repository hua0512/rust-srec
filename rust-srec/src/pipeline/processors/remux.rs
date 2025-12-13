//! Remux/transcode processor for converting video formats.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, error, info, warn};

use super::traits::{Processor, ProcessorInput, ProcessorOutput, ProcessorType};
use super::utils::{create_log_entry, get_extension, is_media};
use crate::Result;

/// Helper to ensure path is absolute.
/// If file exists, uses canonicalize.
/// If not (e.g. new output), uses current_dir + path.
fn make_absolute(path: &str) -> String {
    let path_obj = Path::new(path);
    if path_obj.is_absolute() {
        return path.to_string();
    }

    if path_obj.exists() {
        if let Ok(abs) = std::fs::canonicalize(path_obj) {
            return abs.to_string_lossy().to_string();
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        return cwd.join(path_obj).to_string_lossy().to_string();
    }

    path.to_string()
}

/// Video codec options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    /// Copy video stream without re-encoding.
    Copy,
    /// H.264/AVC codec.
    H264,
    /// H.265/HEVC codec.
    H265,
    /// VP9 codec.
    Vp9,
    /// AV1 codec.
    Av1,
    /// Custom codec string.
    Custom(String),
}

impl Default for VideoCodec {
    fn default() -> Self {
        Self::Copy
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    /// Copy audio stream without re-encoding.
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

impl Default for AudioCodec {
    fn default() -> Self {
        Self::Copy
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
    Veryslow,
}

impl Default for Preset {
    fn default() -> Self {
        Self::Medium
    }
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

    /// Build FFmpeg command arguments from config.
    fn build_args(
        &self,
        input: &ProcessorInput,
        config: &RemuxConfig,
        output_path: &str,
    ) -> Vec<String> {
        let mut args = Vec::new();

        // Overwrite flag
        if config.overwrite {
            args.push("-y".to_string());
        }

        args.push("-hide_banner".to_string());

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
        if let Some(input_path) = input.inputs.first() {
            args.extend(["-i".to_string(), input_path.clone()]);
        }

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
            args.extend(["-f".to_string(), format.clone()]);
        }

        // Fast start for MP4
        if config.faststart {
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

    async fn process(&self, input: &ProcessorInput) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        // Parse config or use defaults
        let config: RemuxConfig = if let Some(ref config_str) = input.config {
            serde_json::from_str(config_str).unwrap_or_else(|e| {
                warn!("Failed to parse remux config, using defaults: {}", e);
                RemuxConfig::default()
            })
        } else {
            RemuxConfig::default()
        };

        let raw_input_path = input.inputs.first().map(|s| s.as_str()).unwrap_or("");
        let input_path_string = make_absolute(raw_input_path);
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
            info!(
                "Input file is not a supported media format for remuxing, passing through: {}",
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
                    "not a supported media format for remuxing".to_string(),
                )],
                ..Default::default()
            });
        }

        // Determine output path: use provided output or generate one dynamically
        let output_string = if let Some(out) = input.outputs.first() {
            out.clone()
        } else {
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

            // Use same directory as input, but with new extension
            parent
                .join(format!("{}.{}", file_stem, ext))
                .to_string_lossy()
                .to_string()
        };
        let output_path_string = make_absolute(&output_string);
        let output_path = output_path_string.as_str();

        info!(
            "Processing {} -> {} (video: {:?}, audio: {:?})",
            input_path, output_path, config.video_codec, config.audio_codec
        );

        let args = self.build_args(input, &config, output_path);
        debug!("FFmpeg args: {:?}", args);

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        cmd.args(&args).env("LC_ALL", "C");

        // Execute command and capture logs
        let command_output =
            crate::pipeline::processors::utils::run_command_with_logs(&mut cmd).await?;

        if !command_output.status.success() {
            // Find the last error log
            let error_msg = command_output
                .logs
                .iter()
                .filter(|l| l.level == crate::pipeline::job_queue::LogLevel::Error)
                .last()
                .map(|l| l.message.clone())
                .unwrap_or_else(|| "Unknown ffmpeg error".to_string());

            error!("ffmpeg failed: {}", error_msg);

            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        info!(
            "Processing completed in {:.2}s: {}",
            command_output.duration, output_path
        );

        // Remove input file if requested and successful
        let mut logs = command_output.logs;
        if config.remove_input_on_success {
            match tokio::fs::remove_file(input_path).await {
                Ok(_) => {
                    info!("Removed input file after successful remux: {}", input_path);
                    logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Info,
                        format!("Removed input file: {}", input_path),
                    ));
                }
                Err(e) => {
                    warn!("Failed to remove input file {}: {}", input_path, e);
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

        // Requirements: 11.5 - Track succeeded inputs for partial failure reporting
        Ok(ProcessorOutput {
            outputs: vec![output_path.to_string()],
            duration_secs: command_output.duration,
            metadata: Some(
                serde_json::json!({
                    "video_codec": format!("{:?}", config.video_codec),
                    "audio_codec": format!("{:?}", config.audio_codec),
                })
                .to_string(),
            ),
            items_produced: vec![],
            input_size_bytes,
            output_size_bytes,
            failed_inputs: vec![],
            succeeded_inputs: vec![input_path.to_string()],
            skipped_inputs: vec![],
            logs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let input = ProcessorInput {
            inputs: vec!["/input.flv".to_string()],
            outputs: vec!["/output.mp4".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };
        let config = RemuxConfig::default();

        let args = processor.build_args(&input, &config, "/output.mp4");

        assert!(args.contains(&"-y".to_string()));
        assert!(args.contains(&"-i".to_string()));
        assert!(args.contains(&"/input.flv".to_string()));
        assert!(args.contains(&"/output.mp4".to_string()));
        assert!(args.contains(&"-c:v".to_string()));
        assert!(args.contains(&"copy".to_string()));
    }

    #[test]
    fn test_build_args_with_transcode() {
        let processor = RemuxProcessor::new();
        let input = ProcessorInput {
            inputs: vec!["/input.flv".to_string()],
            outputs: vec!["/output.mp4".to_string()],
            config: None,
            streamer_id: "test".to_string(),
            session_id: "test".to_string(),
        };
        let config = RemuxConfig {
            video_codec: VideoCodec::H264,
            audio_codec: AudioCodec::Aac,
            crf: Some(23),
            preset: Some(Preset::Fast),
            resolution: Some("1280x720".to_string()),
            ..Default::default()
        };

        let args = processor.build_args(&input, &config, "/output.mp4");

        assert!(args.contains(&"libx264".to_string()));
        assert!(args.contains(&"aac".to_string()));
        assert!(args.contains(&"-crf".to_string()));
        assert!(args.contains(&"23".to_string()));
        assert!(args.contains(&"-preset".to_string()));
        assert!(args.contains(&"fast".to_string()));
        assert!(args.contains(&"-vf".to_string()));
    }
    #[test]
    fn test_make_absolute_path() {
        let cwd = std::env::current_dir().unwrap();

        // Test absolute path
        let abs = if cfg!(windows) {
            "C:\\test\\file.txt"
        } else {
            "/test/file.txt"
        };
        assert_eq!(make_absolute(abs), abs);

        // Test relative path (non-existent) - should join with CWD
        let rel = "test_file.txt";
        let expected = cwd.join(rel).to_string_lossy().to_string();
        assert_eq!(make_absolute(rel), expected);
    }
}
