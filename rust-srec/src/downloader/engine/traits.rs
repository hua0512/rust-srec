//! Download engine trait and related types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use flv_fix::FlvPipelineConfig;
use hls_fix::HlsPipelineConfig;
use parking_lot::RwLock;
use pipeline_common::config::PipelineConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::Result;

/// Type of download engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    /// FFmpeg-based download.
    #[default]
    Ffmpeg,
    /// Streamlink-based download.
    Streamlink,
    /// Native Mesio engine.
    Mesio,
}

impl EngineType {
    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Streamlink => "streamlink",
            Self::Mesio => "mesio",
        }
    }
}

impl std::str::FromStr for EngineType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "ffmpeg" => Ok(Self::Ffmpeg),
            "streamlink" => Ok(Self::Streamlink),
            "mesio" => Ok(Self::Mesio),
            _ => Err(format!("Unknown engine type: {}", s)),
        }
    }
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Configuration for a download.
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Stream URL to download.
    pub url: String,
    /// Output directory.
    pub output_dir: PathBuf,
    /// Output filename template.
    pub filename_template: String,
    /// Output file format (e.g., "flv", "mp4").
    pub output_format: String,
    /// Maximum segment duration in seconds (0 = no limit).
    pub max_segment_duration_secs: u64,
    /// Maximum segment size in bytes (0 = no limit).
    pub max_segment_size_bytes: u64,
    /// Proxy URL (if any).
    pub proxy_url: Option<String>,
    /// Whether to use system proxy settings (ignored if proxy_url is set).
    pub use_system_proxy: bool,
    /// Cookies for authentication.
    pub cookies: Option<String>,
    /// Additional headers.
    pub headers: Vec<(String, String)>,
    /// Streamer ID for tracking.
    pub streamer_id: String,
    /// Session ID for tracking.
    pub session_id: String,

    // --- Pipeline Configuration Fields ---
    /// Whether to enable stream processing through fix pipelines (HlsPipeline/FlvPipeline).
    /// When false, stream data is written directly without pipeline processing.
    /// Default: false
    pub enable_processing: bool,

    /// Common pipeline configuration (max_file_size, max_duration, channel_size).
    /// Used by both HLS and FLV pipelines.
    pub pipeline_config: Option<PipelineConfig>,

    /// HLS-specific pipeline configuration.
    /// Controls defragment, split_segments, and segment_limiter options.
    pub hls_pipeline_config: Option<HlsPipelineConfig>,

    /// FLV-specific pipeline configuration.
    /// Controls duplicate_tag_filtering, repair_strategy, continuity_mode, etc.
    pub flv_pipeline_config: Option<FlvPipelineConfig>,

    /// Override configuration for engines.
    /// Map of engine_id -> config value.
    pub engines_override: Option<serde_json::Value>,
}

impl DownloadConfig {
    /// Create a new download config with required fields.
    pub fn new(
        url: impl Into<String>,
        output_dir: impl Into<PathBuf>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            url: url.into(),
            output_dir: output_dir.into(),
            filename_template: "{streamer}-%Y%m%d-%H%M%S-{title}".to_string(),
            output_format: "flv".to_string(),
            max_segment_duration_secs: 0,
            max_segment_size_bytes: 0,
            proxy_url: None,
            use_system_proxy: false,
            cookies: None,
            headers: Vec::new(),
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            enable_processing: true,
            pipeline_config: None,
            hls_pipeline_config: None,
            flv_pipeline_config: None,
            engines_override: None,
        }
    }

    /// Set the filename template.
    pub fn with_filename_template(mut self, template: impl Into<String>) -> Self {
        self.filename_template = template.into();
        self
    }

    /// Set the output format.
    pub fn with_output_format(mut self, format: impl Into<String>) -> Self {
        self.output_format = format.into();
        self
    }

    /// Set the maximum segment duration.
    pub fn with_max_segment_duration(mut self, secs: u64) -> Self {
        self.max_segment_duration_secs = secs;
        self
    }

    /// Set the maximum segment size.
    pub fn with_max_segment_size(mut self, bytes: u64) -> Self {
        self.max_segment_size_bytes = bytes;
        self
    }

    /// Set the proxy URL (disables system proxy).
    pub fn with_proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy_url = Some(url.into());
        self.use_system_proxy = false;
        self
    }

    /// Set whether to use system proxy settings.
    /// Note: If a proxy URL is set, system proxy is ignored.
    pub fn with_system_proxy(mut self, use_system: bool) -> Self {
        self.use_system_proxy = use_system;
        self
    }

    /// Set cookies.
    pub fn with_cookies(mut self, cookies: impl Into<String>) -> Self {
        self.cookies = Some(cookies.into());
        self
    }

    /// Add a header.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }

    /// Enable or disable stream processing through fix pipelines.
    pub fn with_processing_enabled(mut self, enabled: bool) -> Self {
        self.enable_processing = enabled;
        self
    }

    /// Set the common pipeline configuration.
    pub fn with_pipeline_config(mut self, config: PipelineConfig) -> Self {
        self.pipeline_config = Some(config);
        self
    }

    /// Set the HLS-specific pipeline configuration.
    pub fn with_hls_pipeline_config(mut self, config: HlsPipelineConfig) -> Self {
        self.hls_pipeline_config = Some(config);
        self
    }

    /// Set the FLV-specific pipeline configuration.
    pub fn with_flv_pipeline_config(mut self, config: FlvPipelineConfig) -> Self {
        self.flv_pipeline_config = Some(config);
        self
    }

    /// Set engine overrides.
    pub fn with_engines_override(mut self, overrides: Option<serde_json::Value>) -> Self {
        self.engines_override = overrides;
        self
    }

    /// Build a PipelineConfig from this DownloadConfig.
    ///
    /// If `pipeline_config` is set, returns a clone of it.
    /// Otherwise, builds a new PipelineConfig from the individual settings.
    pub fn build_pipeline_config(&self) -> PipelineConfig {
        if let Some(ref config) = self.pipeline_config {
            config.clone()
        } else {
            let mut builder = PipelineConfig::builder()
                .max_file_size(self.max_segment_size_bytes)
                .channel_size(64);

            if self.max_segment_duration_secs > 0 {
                builder = builder.max_duration(Duration::from_secs(self.max_segment_duration_secs));
            }

            builder.build()
        }
    }

    /// Build an HlsPipelineConfig from this DownloadConfig.
    ///
    /// If `hls_pipeline_config` is set, returns a clone of it.
    /// Otherwise, returns the default HlsPipelineConfig.
    pub fn build_hls_pipeline_config(&self) -> HlsPipelineConfig {
        self.hls_pipeline_config.clone().unwrap_or_default()
    }

    /// Build a FlvPipelineConfig from this DownloadConfig.
    ///
    /// If `flv_pipeline_config` is set, returns a clone of it.
    /// Otherwise, returns the default FlvPipelineConfig.
    pub fn build_flv_pipeline_config(&self) -> FlvPipelineConfig {
        self.flv_pipeline_config.clone().unwrap_or_default()
    }
}

/// Status of a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    /// Download is starting.
    Starting,
    /// Download is in progress.
    Downloading,
    /// Download is paused.
    Paused,
    /// Download completed successfully.
    Completed,
    /// Download failed.
    Failed,
    /// Download was cancelled.
    Cancelled,
}

impl DownloadStatus {
    /// Stable lowercase string representation for APIs.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Downloading => "downloading",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Progress information for a download.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Total bytes downloaded.
    pub bytes_downloaded: u64,
    /// Download duration in seconds (wall-clock elapsed time).
    pub duration_secs: f64,
    /// Current download speed in bytes/sec.
    pub speed_bytes_per_sec: u64,
    /// Number of segments completed.
    pub segments_completed: u32,
    /// Current segment being downloaded.
    pub current_segment: Option<String>,
    /// Total media duration in seconds (from segment metadata or timestamps).
    pub media_duration_secs: f64,
    /// Playback ratio: media_duration / elapsed_time (>1.0 = faster than real-time).
    pub playback_ratio: f64,
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            bytes_downloaded: 0,
            duration_secs: 0.0,
            speed_bytes_per_sec: 0,
            segments_completed: 0,
            current_segment: None,
            media_duration_secs: 0.0,
            playback_ratio: 0.0,
        }
    }
}

/// Information about a completed segment.
#[derive(Debug, Clone)]
pub struct SegmentInfo {
    /// Path to the segment file.
    pub path: PathBuf,
    /// Segment duration in seconds.
    pub duration_secs: f64,
    /// Segment size in bytes.
    pub size_bytes: u64,
    /// Segment index (0-based).
    pub index: u32,
    /// Timestamp when segment was completed.
    pub completed_at: DateTime<Utc>,
}

/// Events emitted by download engines.
#[derive(Debug, Clone)]
pub enum SegmentEvent {
    /// A new segment has started recording.
    SegmentStarted {
        /// Path to the segment file being written.
        path: PathBuf,
        /// Sequence number of the segment (0-based).
        sequence: u32,
    },
    /// A segment was completed.
    SegmentCompleted(SegmentInfo),
    /// Download progress update.
    Progress(DownloadProgress),
    /// Download completed.
    DownloadCompleted {
        total_bytes: u64,
        total_duration_secs: f64,
        total_segments: u32,
    },
    /// Download failed.
    DownloadFailed { error: String, recoverable: bool },
}

/// Handle to an active download.
pub struct DownloadHandle {
    /// Unique download ID.
    pub id: String,
    /// Engine type used.
    pub engine_type: EngineType,
    /// Download configuration shared with engines.
    pub config: Arc<RwLock<DownloadConfig>>,
    /// Cancellation token.
    pub cancellation_token: CancellationToken,
    /// Event sender for segment events.
    pub event_tx: mpsc::Sender<SegmentEvent>,
    /// Start time.
    pub started_at: DateTime<Utc>,
}

impl DownloadHandle {
    /// Create a new download handle.
    pub fn new(
        id: impl Into<String>,
        engine_type: EngineType,
        config: DownloadConfig,
        event_tx: mpsc::Sender<SegmentEvent>,
    ) -> Self {
        Self {
            id: id.into(),
            engine_type,
            config: Arc::new(RwLock::new(config)),
            cancellation_token: CancellationToken::new(),
            event_tx,
            started_at: Utc::now(),
        }
    }

    /// Cancel the download.
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Check if the download is cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Snapshot the current download configuration.
    pub fn config_snapshot(&self) -> DownloadConfig {
        self.config.read().clone()
    }

    /// Apply in-place configuration updates.
    pub fn update_config<F>(&self, updater: F)
    where
        F: FnOnce(&mut DownloadConfig),
    {
        let mut cfg = self.config.write();
        updater(&mut cfg);
    }
}

/// Information about an active download.
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    /// Download ID.
    pub id: String,
    /// Stream URL being downloaded.
    pub url: String,
    /// Streamer ID.
    pub streamer_id: String,
    /// Session ID.
    pub session_id: String,
    /// Engine type.
    pub engine_type: EngineType,
    /// Current status.
    pub status: DownloadStatus,
    /// Progress information.
    pub progress: DownloadProgress,
    /// Start time.
    pub started_at: DateTime<Utc>,
}

/// Trait for download engines.
#[async_trait]
pub trait DownloadEngine: Send + Sync {
    /// Get the engine type.
    fn engine_type(&self) -> EngineType;

    /// Start a download.
    ///
    /// Returns a handle that can be used to monitor and cancel the download.
    /// The engine should emit events through the handle's event channel.
    async fn start(&self, handle: Arc<DownloadHandle>) -> Result<()>;

    /// Stop a download.
    ///
    /// This should gracefully stop the download and clean up resources.
    async fn stop(&self, handle: &DownloadHandle) -> Result<()>;

    /// Check if the engine is available (e.g., binary exists).
    fn is_available(&self) -> bool;

    /// Get the engine version string.
    fn version(&self) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_type_from_str() {
        assert_eq!(
            "ffmpeg".parse::<EngineType>().ok(),
            Some(EngineType::Ffmpeg)
        );
        assert_eq!(
            "FFMPEG".parse::<EngineType>().ok(),
            Some(EngineType::Ffmpeg)
        );
        assert_eq!(
            "streamlink".parse::<EngineType>().ok(),
            Some(EngineType::Streamlink)
        );
        assert_eq!("mesio".parse::<EngineType>().ok(), Some(EngineType::Mesio));
        assert_eq!("unknown".parse::<EngineType>().ok(), None);
    }

    #[test]
    fn test_download_config_builder() {
        let config = DownloadConfig::new(
            "https://example.com/stream",
            "/tmp/downloads",
            "streamer-123",
            "session-456",
        )
        .with_output_format("mp4")
        .with_max_segment_duration(3600)
        .with_max_segment_size(1024 * 1024)
        .with_proxy("http://proxy:8080");

        assert_eq!(config.url, "https://example.com/stream");
        assert_eq!(config.output_format, "mp4");
        assert_eq!(config.max_segment_duration_secs, 3600);
        assert_eq!(config.max_segment_size_bytes, 1024 * 1024);
        assert_eq!(config.proxy_url, Some("http://proxy:8080".to_string()));
        assert!(!config.use_system_proxy); // Explicit proxy disables system proxy
    }

    #[test]
    fn test_download_progress_default() {
        let progress = DownloadProgress::default();
        assert_eq!(progress.bytes_downloaded, 0);
        assert_eq!(progress.segments_completed, 0);
        assert_eq!(progress.media_duration_secs, 0.0);
        assert_eq!(progress.playback_ratio, 0.0);
    }
}
