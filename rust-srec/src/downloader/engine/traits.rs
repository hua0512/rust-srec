//! Download engine trait and related types.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::Result;

/// Type of download engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineType {
    /// FFmpeg-based download.
    Ffmpeg,
    /// Streamlink-based download.
    Streamlink,
    /// Native Mesio engine.
    Mesio,
}

impl EngineType {
    /// Get the engine type from a string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ffmpeg" => Some(Self::Ffmpeg),
            "streamlink" => Some(Self::Streamlink),
            "mesio" => Some(Self::Mesio),
            _ => None,
        }
    }

    /// Get the string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ffmpeg => "ffmpeg",
            Self::Streamlink => "streamlink",
            Self::Mesio => "mesio",
        }
    }
}

impl std::fmt::Display for EngineType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Default for EngineType {
    fn default() -> Self {
        Self::Ffmpeg
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
    /// Cookies for authentication.
    pub cookies: Option<String>,
    /// Additional headers.
    pub headers: Vec<(String, String)>,
    /// Streamer ID for tracking.
    pub streamer_id: String,
    /// Session ID for tracking.
    pub session_id: String,
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
            filename_template: "{streamer}-{date}-{time}".to_string(),
            output_format: "flv".to_string(),
            max_segment_duration_secs: 0,
            max_segment_size_bytes: 0,
            proxy_url: None,
            cookies: None,
            headers: Vec::new(),
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
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

    /// Set the proxy URL.
    pub fn with_proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy_url = Some(url.into());
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

/// Progress information for a download.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Total bytes downloaded.
    pub bytes_downloaded: u64,
    /// Download duration in seconds.
    pub duration_secs: f64,
    /// Current download speed in bytes/sec.
    pub speed_bytes_per_sec: u64,
    /// Number of segments completed.
    pub segments_completed: u32,
    /// Current segment being downloaded.
    pub current_segment: Option<String>,
}

impl Default for DownloadProgress {
    fn default() -> Self {
        Self {
            bytes_downloaded: 0,
            duration_secs: 0.0,
            speed_bytes_per_sec: 0,
            segments_completed: 0,
            current_segment: None,
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
    DownloadFailed {
        error: String,
        recoverable: bool,
    },
}

/// Handle to an active download.
pub struct DownloadHandle {
    /// Unique download ID.
    pub id: String,
    /// Engine type used.
    pub engine_type: EngineType,
    /// Download configuration.
    pub config: DownloadConfig,
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
            config,
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
}

/// Information about an active download.
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    /// Download ID.
    pub id: String,
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
        assert_eq!(EngineType::from_str("ffmpeg"), Some(EngineType::Ffmpeg));
        assert_eq!(EngineType::from_str("FFMPEG"), Some(EngineType::Ffmpeg));
        assert_eq!(EngineType::from_str("streamlink"), Some(EngineType::Streamlink));
        assert_eq!(EngineType::from_str("mesio"), Some(EngineType::Mesio));
        assert_eq!(EngineType::from_str("unknown"), None);
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
        .with_proxy("http://proxy:8080");

        assert_eq!(config.url, "https://example.com/stream");
        assert_eq!(config.output_format, "mp4");
        assert_eq!(config.max_segment_duration_secs, 3600);
        assert_eq!(config.proxy_url, Some("http://proxy:8080".to_string()));
    }

    #[test]
    fn test_download_progress_default() {
        let progress = DownloadProgress::default();
        assert_eq!(progress.bytes_downloaded, 0);
        assert_eq!(progress.segments_completed, 0);
    }
}
