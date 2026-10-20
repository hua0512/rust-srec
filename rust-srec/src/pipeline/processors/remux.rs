//! Remux/transcode processor for converting video formats.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::process::Command;
use tracing::{debug, info, warn};

use super::outputs::{OutputBatch, output_size};
use super::traits::{
    Processor, ProcessorContext, ProcessorInput, ProcessorOutput, ProcessorType, TimeAnchor,
};
use super::utils::{create_log_entry, get_extension, is_media, parse_config_or_default};
use crate::Result;

use super::paths::{remux_command_path as make_absolute, remux_comparison_key as comparison_key};

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
    /// H.264 NVENC (NVIDIA hardware encoder).
    #[serde(alias = "h264_nvenc")]
    H264Nvenc,
    /// H.265 NVENC (NVIDIA hardware encoder).
    #[serde(alias = "hevc_nvenc")]
    HevcNvenc,
    /// AV1 NVENC (NVIDIA hardware encoder, requires RTX 4000+).
    #[serde(alias = "av1_nvenc")]
    Av1Nvenc,
    /// Custom codec string.
    Custom(String),
}

impl VideoCodec {
    fn ffmpeg_name(&self) -> &str {
        match self {
            Self::Copy => "copy",
            Self::H264 => "libx264",
            Self::H265 => "libx265",
            Self::Vp9 => "libvpx-vp9",
            Self::Av1 => "libaom-av1",
            Self::H264Nvenc => "h264_nvenc",
            Self::HevcNvenc => "hevc_nvenc",
            Self::Av1Nvenc => "av1_nvenc",
            Self::Custom(codec) => codec,
        }
    }

    fn as_ffmpeg_args(&self) -> Vec<String> {
        vec!["-c:v".to_string(), self.ffmpeg_name().to_string()]
    }

    fn is_nvenc(&self) -> bool {
        matches!(self, Self::H264Nvenc | Self::HevcNvenc | Self::Av1Nvenc)
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
    fn ffmpeg_name(&self) -> Option<&str> {
        match self {
            Self::Copy => Some("copy"),
            Self::Aac => Some("aac"),
            Self::Mp3 => Some("libmp3lame"),
            Self::Opus => Some("libopus"),
            Self::Flac => Some("flac"),
            Self::None => None,
            Self::Custom(codec) => Some(codec),
        }
    }

    fn as_ffmpeg_args(&self) -> Vec<String> {
        match self.ffmpeg_name() {
            Some(name) => vec!["-c:a".to_string(), name.to_string()],
            None => vec!["-an".to_string()],
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
    // NVENC-specific presets
    P1,
    P2,
    P3,
    P4,
    P5,
    P6,
    P7,
    /// NVENC low-latency.
    #[serde(alias = "ll")]
    Ll,
    /// NVENC high quality.
    #[serde(alias = "hq")]
    Hq,
    /// NVENC high performance.
    #[serde(alias = "hp")]
    Hp,
    /// NVENC Blu-ray disc.
    #[serde(alias = "bd")]
    Bd,
    /// NVENC low-latency high quality.
    #[serde(alias = "llhq")]
    Llhq,
    /// NVENC low-latency high performance.
    #[serde(alias = "llhp")]
    Llhp,
    /// NVENC lossless.
    Lossless,
    /// NVENC lossless high performance.
    #[serde(alias = "losslesshp")]
    Losslesshp,
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
            Self::P1 => "p1",
            Self::P2 => "p2",
            Self::P3 => "p3",
            Self::P4 => "p4",
            Self::P5 => "p5",
            Self::P6 => "p6",
            Self::P7 => "p7",
            Self::Ll => "ll",
            Self::Hq => "hq",
            Self::Hp => "hp",
            Self::Bd => "bd",
            Self::Llhq => "llhq",
            Self::Llhp => "llhp",
            Self::Lossless => "lossless",
            Self::Losslesshp => "losslesshp",
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

    /// Fill artist, title and date from the recording; explicit metadata wins.
    /// Date uses the session start (or original job timestamp) in UTC.
    #[serde(default)]
    pub include_recording_metadata: bool,

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
            include_recording_metadata: false,
            remove_input_on_success: false,
        }
    }
}

impl RemuxConfig {
    fn resolve_recording_metadata(&mut self, input: &ProcessorInput) {
        if !self.include_recording_metadata {
            return;
        }
        let artist = input
            .streamer_name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or_else(|| (!input.streamer_id.is_empty()).then_some(input.streamer_id.as_str()));
        let date = TimeAnchor::SessionStart
            .reference_time(input)
            .format("%Y-%m-%d")
            .to_string();
        // These are tag values, not filename templates. Keep braces, percent
        // signs and Unicode unchanged, and never expand literal custom tags.
        for (key, value) in [
            ("artist", artist),
            (
                "title",
                input
                    .session_title
                    .as_deref()
                    .filter(|title| !title.is_empty()),
            ),
            ("date", Some(date.as_str())),
        ] {
            if let Some(value) = value
                && !self
                    .metadata
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(key))
            {
                self.metadata.push((key.to_owned(), value.to_owned()));
            }
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
        let mut args = Vec::with_capacity(32);

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

        // Quality-based encoding: NVENC uses -cq, others use -crf
        if let Some(crf) = config.crf {
            let flag = if config.video_codec.is_nvenc() {
                "-cq"
            } else {
                "-crf"
            };
            args.extend([flag.to_string(), crf.to_string()]);
        }

        // Preset
        if let Some(ref preset) = config.preset {
            args.extend(["-preset".to_string(), preset.as_str().to_string()]);
        }

        // Video filters
        let mut vf_parts = Vec::new();
        let is_cuda = config
            .hwaccel
            .as_deref()
            .is_some_and(|h| h.eq_ignore_ascii_case("cuda"));

        if let Some(ref resolution) = config.resolution {
            let scale_filter = if is_cuda { "scale_cuda" } else { "scale" };
            vf_parts.push(format!("{}={}", scale_filter, resolution.replace('x', ":")));
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

    async fn determine_output_path_for_input(
        input_path: &str,
        config: &RemuxConfig,
        output_override: Option<&str>,
    ) -> Result<String> {
        let input_abs = comparison_key(input_path).await;

        if let Some(out) = output_override.filter(|s| !s.is_empty()) {
            let out_abs = comparison_key(out).await;
            if super::paths::spelling_equal(
                &input_abs,
                &out_abs,
                super::paths::CasePolicy::WindowsAndMacOs,
            ) {
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
        let candidate_abs = comparison_key(&candidate).await;

        // Avoid in-place remux (ffmpeg cannot safely write to the same path it's reading from).
        if super::paths::spelling_equal(
            &input_abs,
            &candidate_abs,
            super::paths::CasePolicy::WindowsAndMacOs,
        ) {
            return Ok(parent
                .join(format!("{}_remux.{}", file_stem, ext))
                .to_string_lossy()
                .to_string());
        }

        Ok(candidate)
    }

    /// Remux one input into a staged file owned by `batch`; the caller publishes
    /// the whole batch and deletes sources only after that.
    async fn process_one(
        &self,
        input_path: &str,
        output_override: Option<&str>,
        config: &RemuxConfig,
        ctx: &ProcessorContext,
        batch: &mut OutputBatch,
    ) -> Result<ProcessorOutput> {
        let start = std::time::Instant::now();

        let input_path_string = make_absolute(input_path).await;
        let input_path = input_path_string.as_str();

        let input_path_obj = Path::new(input_path);
        let input_exists = tokio::fs::try_exists(input_path_obj)
            .await
            .map_err(|error| crate::Error::io_path("try_exists", input_path_obj, error))?;
        if !input_exists {
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
            return Ok(ProcessorOutput::skipped_file(
                input_path,
                "not a supported media format for remuxing",
                "unsupported_media_format",
                duration,
                Vec::new(),
            ));
        }

        // Determine output path: use provided output or generate one dynamically.
        // Ensures we never choose an output path equal to the input path.
        let output_string =
            Self::determine_output_path_for_input(input_path, config, output_override).await?;
        let output_path_string = make_absolute(&output_string).await;
        let output_path = output_path_string.as_str();

        let output_path_obj = Path::new(output_path);
        let output_exists = tokio::fs::try_exists(output_path_obj)
            .await
            .map_err(|error| crate::Error::io_path("try_exists", output_path_obj, error))?;
        if !config.overwrite && output_exists {
            return Err(crate::Error::PipelineError(format!(
                "Output file already exists and overwrite is disabled: {}",
                output_path
            )));
        }

        ctx.info(format!(
            "Processing {} -> {} (video: {:?}, audio: {:?})",
            input_path, output_path, config.video_codec, config.audio_codec
        ));

        // Staging validates the destination against every input of the batch, so
        // one item's output can never overwrite a sibling input that has not been
        // remuxed yet.
        let temp_path = batch.stage(output_path_obj, config.overwrite).await?;
        let mut args = self.build_args(input_path, config, output_path);
        let output_arg = args.last_mut().ok_or_else(|| {
            crate::Error::PipelineError("FFmpeg command has no output path".to_string())
        })?;
        *output_arg = temp_path.to_string_lossy().into_owned();
        debug!("FFmpeg args: {:?}", args);

        // Build ffmpeg command
        let mut cmd = Command::new(&self.ffmpeg_path);
        crate::utils::configure_ffmpeg_locale(&mut cmd);
        cmd.args(&args);

        // `-ss` is an input option, so output timestamps start at zero and the
        // output `-to` limits the length just like `-t`.
        let start = config.start_time.unwrap_or(0.0);
        let end = config
            .duration
            .or(config.end_time)
            .map_or(f64::INFINITY, |length| start + length);

        // Execute command and capture logs
        let command_output = crate::pipeline::processors::utils::run_ffmpeg_with_progress(
            &mut cmd,
            &ctx.progress,
            Some(ctx.log_sink.clone()),
            Some(start..end),
        )
        .await?;

        if !command_output.status.success() {
            // Find the last error log
            let error_msg = command_output
                .last_error_message()
                .map(str::to_owned)
                .unwrap_or_else(|| "Unknown ffmpeg error".to_string());

            ctx.error(format!("ffmpeg failed: {}", error_msg));

            return Err(crate::Error::Other(format!(
                "ffmpeg failed with exit code {}: {}",
                command_output.status.code().unwrap_or(-1),
                error_msg
            )));
        }

        let output_size_bytes = Some(output_size(&temp_path).await?);

        ctx.info(format!(
            "Processing completed in {:.2}s: {}",
            command_output.duration, output_path
        ));

        let input_size_bytes = tokio::fs::metadata(input_path).await.ok().map(|m| m.len());

        // Only return the newly produced output file (no additive passthrough)
        Ok(ProcessorOutput {
            outputs: vec![output_path.to_string()],
            duration_secs: command_output.duration,
            items_produced: vec![output_path.to_string()],
            input_size_bytes,
            output_size_bytes,
            succeeded_inputs: vec![input_path.to_string()],
            logs: command_output.logs,
            ..Default::default()
        })
    }
}

struct RemuxProcessorItem<'a> {
    processor: &'a RemuxProcessor,
    config: &'a RemuxConfig,
    ctx: &'a ProcessorContext,
}

#[async_trait]
impl super::media_driver::MediaItem for RemuxProcessorItem<'_> {
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
        let mut config: RemuxConfig =
            parse_config_or_default(input.config.as_deref(), ctx, "remux", None);
        config.resolve_recording_metadata(input);

        if input.inputs.is_empty() {
            return Err(crate::Error::PipelineError(
                "No input file specified for remuxing".to_string(),
            ));
        }

        let plan = super::planning::OutputPlan::unary_or_mapped(&input.inputs, &input.outputs)
            .map_err(|_| crate::Error::PipelineError(format!("Remux batch job requires outputs to be empty or have the same length as inputs (inputs={}, outputs={})", input.inputs.len(), input.outputs.len())))?;
        let is_batch = plan.is_batch();
        let mut output = super::media_driver::run_media(
            plan,
            OutputBatch::new(&input.inputs, ctx),
            &ctx.progress,
            &RemuxProcessorItem {
                processor: self,
                config: &config,
                ctx,
            },
        )
        .await?;
        if !is_batch && output.succeeded_inputs.is_empty() {
            // A single unsupported input keeps the shared skip result unchanged.
            return Ok(output);
        }
        // Sources are deleted only after every output of the job has been published.
        let mut removed_inputs = 0usize;
        if config.remove_input_on_success {
            // An input the remux wrote back over (see the identity check in process_one) is now the
            // output itself, so removing it would delete the result.
            let mut output_keys = Vec::with_capacity(output.outputs.len());
            for output_path in &output.outputs {
                output_keys.push(comparison_key(output_path).await);
            }

            // Only remove inputs that were actually remuxed. Skipped inputs should never be removed.
            for input_path in &output.succeeded_inputs {
                let input_key = comparison_key(input_path).await;
                if output_keys.iter().any(|output_key| {
                    super::paths::spelling_equal(
                        &input_key,
                        output_key,
                        super::paths::CasePolicy::WindowsAndMacOs,
                    )
                }) {
                    let msg = format!(
                        "Not removing input {input_path}: it is the same file as a remux output"
                    );
                    warn!("{msg}");
                    output.logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Warn,
                        msg,
                    ));
                    continue;
                }

                let input_path_string = make_absolute(input_path).await;
                let input_path = input_path_string.as_str();
                if let Err(e) = tokio::fs::remove_file(input_path).await {
                    warn!(path = input_path, error = %e, "Failed to remove remux input");
                    ctx.warn(format!("Failed to remove input file {}: {}", input_path, e));
                    output.logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Warn,
                        format!("Failed to remove input file {}: {}", input_path, e),
                    ));
                } else {
                    removed_inputs += 1;
                    info!("Removed input file after successful remux: {}", input_path);
                    output.logs.push(create_log_entry(
                        crate::pipeline::job_queue::LogLevel::Info,
                        format!("Removed input file: {}", input_path),
                    ));
                }
            }
        }

        let metadata = if is_batch {
            serde_json::json!({
                "batch": true,
                "inputs": input.inputs.len(),
                "input_removed": removed_inputs > 0,
                "video_codec": format!("{:?}", config.video_codec),
                "audio_codec": format!("{:?}", config.audio_codec),
            })
        } else {
            serde_json::json!({
                "video_codec": format!("{:?}", config.video_codec),
                "audio_codec": format!("{:?}", config.audio_codec),
                "input_removed": removed_inputs > 0,
            })
        };

        output.metadata = Some(metadata.to_string());
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_tags_are_opt_in_literal_overridable_and_use_stable_dates() {
        use super::super::test_utils::utc_datetime;

        let input = ProcessorInput::default()
            .with_streamer_name("主播 / {title}")
            .with_session_title("A title with {artist}, %Y and 🦀")
            .with_session_start(utc_datetime(2024, 1, 1, 23, 30, 0))
            .with_created_at(utc_datetime(2024, 1, 3, 0, 0, 0));
        let args = |mut config: RemuxConfig, input: &ProcessorInput| {
            config.resolve_recording_metadata(input);
            RemuxProcessor::new()
                .build_args("in.mp4", &config, "out.mp4")
                .windows(2)
                .filter(|pair| pair[0] == "-metadata")
                .map(|pair| pair[1].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            args(
                RemuxConfig {
                    metadata: vec![("title".into(), "{title}".into())],
                    ..Default::default()
                },
                &input
            ),
            ["title={title}"]
        );
        assert_eq!(
            args(
                RemuxConfig {
                    include_recording_metadata: true,
                    ..Default::default()
                },
                &input
            ),
            [
                "artist=主播 / {title}",
                "title=A title with {artist}, %Y and 🦀",
                "date=2024-01-01"
            ]
        );
        assert_eq!(
            args(
                RemuxConfig {
                    include_recording_metadata: true,
                    metadata: vec![
                        ("TITLE".into(), "literal {title}".into()),
                        ("date".into(), String::new())
                    ],
                    ..Default::default()
                },
                &input
            ),
            ["TITLE=literal {title}", "date=", "artist=主播 / {title}"]
        );
        let fallback = ProcessorInput {
            streamer_id: "streamer-id".into(),
            created_at: input.created_at,
            ..Default::default()
        };
        assert_eq!(
            args(
                RemuxConfig {
                    include_recording_metadata: true,
                    ..Default::default()
                },
                &fallback
            ),
            ["artist=streamer-id", "date=2024-01-03"]
        );
        assert_eq!(
            args(
                RemuxConfig {
                    include_recording_metadata: true,
                    ..Default::default()
                },
                &ProcessorInput {
                    streamer_name: Some(String::new()),
                    session_title: Some(String::new()),
                    created_at: input.created_at,
                    ..Default::default()
                }
            ),
            ["date=2024-01-03"]
        );
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
    fn test_build_args_nvenc_uses_cq_and_scale_cuda() {
        let processor = RemuxProcessor::new();
        let config = RemuxConfig {
            video_codec: VideoCodec::H264Nvenc,
            audio_codec: AudioCodec::Aac,
            crf: Some(23),
            preset: Some(Preset::P4),
            resolution: Some("1280x720".to_string()),
            hwaccel: Some("cuda".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.flv", &config, "/output.mp4");

        assert!(args.contains(&"h264_nvenc".to_string()));
        assert!(args.contains(&"-cq".to_string()));
        assert!(!args.contains(&"-crf".to_string()));
        assert!(args.contains(&"-preset".to_string()));
        assert!(args.contains(&"p4".to_string()));

        let vf_idx = args.iter().position(|a| a == "-vf").unwrap();
        assert!(args[vf_idx + 1].starts_with("scale_cuda="));
    }

    #[test]
    fn test_build_args_software_uses_crf_and_scale() {
        let processor = RemuxProcessor::new();
        let config = RemuxConfig {
            video_codec: VideoCodec::H264,
            crf: Some(23),
            resolution: Some("1280x720".to_string()),
            ..Default::default()
        };

        let args = processor.build_args("/input.flv", &config, "/output.mp4");

        assert!(args.contains(&"-crf".to_string()));
        assert!(!args.contains(&"-cq".to_string()));

        let vf_idx = args.iter().position(|a| a == "-vf").unwrap();
        assert!(args[vf_idx + 1].starts_with("scale="));
    }

    #[test]
    fn test_nvenc_config_deserialization() {
        let json = r#"{
            "video_codec": "h264nvenc",
            "audio_codec": "aac",
            "preset": "p7",
            "crf": 20,
            "hwaccel": "cuda"
        }"#;

        let config: RemuxConfig = serde_json::from_str(json).unwrap();
        assert!(matches!(config.video_codec, VideoCodec::H264Nvenc));
        assert!(config.video_codec.is_nvenc());
        assert!(matches!(config.preset, Some(Preset::P7)));
        assert_eq!(config.crf, Some(20));
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
}
