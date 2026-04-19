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
    /// Streamer display name for notifications.
    pub streamer_name: String,
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
        streamer_name: impl Into<String>,
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
            streamer_name: streamer_name.into(),
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
    /// Timestamp when segment recording started (from the start callback).
    pub started_at: Option<DateTime<Utc>>,
    /// Timestamp when segment was completed.
    pub completed_at: DateTime<Utc>,
    pub split_reason_code: Option<String>,
    pub split_reason_details_json: Option<String>,
}

/// Serializable subset of [`std::io::ErrorKind`] used by the output-root write gate.
///
/// `std::io::ErrorKind` does not implement `Serialize`/`Deserialize` and has many
/// variants we don't care about. This enum covers the kinds the gate classifies
/// distinctly for notifications and recovery messaging; everything else falls
/// into [`IoErrorKindSer::Other`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IoErrorKindSer {
    /// `ENOENT` — path or one of its ancestors does not exist. The fingerprint
    /// of a stale Docker bind mount where the host source directory was
    /// renamed/deleted leaving an orphaned inode.
    NotFound,
    /// `ENOSPC` — disk full.
    StorageFull,
    /// `EACCES`/`EPERM` — permission denied.
    PermissionDenied,
    /// `EROFS` — filesystem mounted read-only.
    ReadOnlyFilesystem,
    /// Probe or operation timed out (e.g., hung NFS mount during the startup probe).
    TimedOut,
    /// Any other `io::ErrorKind` we don't classify distinctly.
    Other,
}

impl IoErrorKindSer {
    /// Classify a [`std::io::ErrorKind`] into the gate's narrower enum.
    pub fn from_io_kind(kind: std::io::ErrorKind) -> Self {
        match kind {
            // ENOTDIR (Unix) / ERROR_DIRECTORY (Windows) → NotADirectory;
            // ERROR_ALREADY_EXISTS (Windows, what create_dir_all surfaces
            // when an ancestor component exists as a non-directory) →
            // AlreadyExists. All three mean "the gate-tracked path has no
            // usable directory", so bucket with NotFound — same "stale
            // mount / restart container" recovery text applies.
            std::io::ErrorKind::NotFound
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::AlreadyExists => Self::NotFound,
            std::io::ErrorKind::StorageFull => Self::StorageFull,
            std::io::ErrorKind::PermissionDenied => Self::PermissionDenied,
            std::io::ErrorKind::ReadOnlyFilesystem => Self::ReadOnlyFilesystem,
            std::io::ErrorKind::TimedOut => Self::TimedOut,
            _ => Self::Other,
        }
    }

    /// Stable lowercase string for log fields and `notification.*.description.<key>` lookups.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::StorageFull => "storage_full",
            Self::PermissionDenied => "permission_denied",
            Self::ReadOnlyFilesystem => "read_only",
            Self::TimedOut => "timed_out",
            Self::Other => "other",
        }
    }
}

/// Classified error kind for download failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFailureKind {
    /// HTTP 4xx client error (not rate-limiting). Resource permanently unavailable at this URL.
    HttpClientError { status: u16 },
    /// HTTP 429 Too Many Requests.
    RateLimited,
    /// HTTP 5xx server error (transient).
    HttpServerError { status: u16 },
    /// Network-level failure: connection refused/reset, DNS, TLS, timeout.
    Network,
    /// Local filesystem I/O error (write failure, disk full) that does NOT
    /// affect the entire output root. Reserved for cases where the failure is
    /// scoped to a single file or operation; for root-wide failures use
    /// [`Self::OutputRootUnavailable`] instead so the write gate can short-circuit.
    Io,
    /// Output root (e.g. a configured recording directory) is unwritable.
    /// Caught at the filesystem boundary by the output-root write gate; does
    /// not count toward the engine circuit breaker because it is infrastructure,
    /// not engine fault.
    OutputRootUnavailable { io_kind: IoErrorKindSer },
    /// Stream source unavailable (all sources failed, playlist empty, stream ended).
    SourceUnavailable,
    /// Configuration/protocol error (invalid URL, unsupported protocol).
    Configuration,
    /// External process exited abnormally (FFmpeg, Streamlink).
    ProcessExit { code: Option<i32> },
    /// Writer/pipeline processing error (FLV decode, segment processing).
    Processing,
    /// Download was cancelled.
    Cancelled,
    /// Catch-all.
    Other,
}

impl DownloadFailureKind {
    /// Whether this failure should count toward the circuit breaker.
    ///
    /// Permanent HTTP client errors (4xx except 429) and configuration errors
    /// are NOT counted because they indicate the specific resource is gone or
    /// misconfigured, not that the engine is malfunctioning. Output-root
    /// failures are also excluded because they are infrastructure-level and
    /// already gated by [`crate::downloader::output_root_gate`]; routing them
    /// into the engine circuit breaker would double-block recovery.
    pub fn affects_circuit_breaker(&self) -> bool {
        !matches!(
            self,
            Self::HttpClientError { .. }
                | Self::Configuration
                | Self::Cancelled
                | Self::OutputRootUnavailable { .. }
        )
    }

    /// Whether the download could succeed if retried.
    pub fn is_recoverable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::HttpServerError { .. }
                | Self::Network
                | Self::Io
                | Self::OutputRootUnavailable { .. }
                | Self::SourceUnavailable
                | Self::ProcessExit { .. }
                | Self::Other
        )
    }
}

/// Error returned by [`DownloadEngine::start`] carrying a classified
/// [`DownloadFailureKind`] so the manager can make informed retry and
/// circuit-breaker decisions without hardcoding `Other`.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct EngineStartError {
    /// Classified failure kind.
    pub kind: DownloadFailureKind,
    /// Human-readable error message.
    pub message: String,
}

impl EngineStartError {
    /// Create a new `EngineStartError`.
    pub fn new(kind: DownloadFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl From<crate::Error> for EngineStartError {
    fn from(err: crate::Error) -> Self {
        // Walk the error source chain to find the first std::io::Error and
        // classify by its ErrorKind. Filesystem failures from the recording
        // path (ENOENT/ENOSPC/EACCES/EROFS) become `OutputRootUnavailable`
        // so the manager and the write gate can treat them as infra-level;
        // other I/O errors stay as `Io`. Without this, every `crate::Error`
        // collapsed to `DownloadFailureKind::Other` and lost retry/CB context.
        let kind = io_error_kind_in_chain(&err)
            .map(|k| match IoErrorKindSer::from_io_kind(k) {
                IoErrorKindSer::NotFound
                | IoErrorKindSer::StorageFull
                | IoErrorKindSer::PermissionDenied
                | IoErrorKindSer::ReadOnlyFilesystem
                | IoErrorKindSer::TimedOut => DownloadFailureKind::OutputRootUnavailable {
                    io_kind: IoErrorKindSer::from_io_kind(k),
                },
                IoErrorKindSer::Other => DownloadFailureKind::Io,
            })
            .unwrap_or(DownloadFailureKind::Other);
        Self {
            kind,
            message: err.to_string(),
        }
    }
}

/// Walk `err`'s `std::error::Error::source()` chain and return the
/// `std::io::ErrorKind` of the first `std::io::Error` encountered, if any.
fn io_error_kind_in_chain(err: &(dyn std::error::Error + 'static)) -> Option<std::io::ErrorKind> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            return Some(io_err.kind());
        }
        current = e.source();
    }
    None
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
        started_at: DateTime<Utc>,
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
    DownloadFailed {
        /// Classified error kind for programmatic decisions.
        kind: DownloadFailureKind,
        /// Human-readable error message for logging and display.
        message: String,
    },
    /// The recording output filesystem appears unwritable mid-stream
    /// (e.g., ffmpeg emitted `"No space left on device"` or exited with
    /// code 228). Emitted by engines *before* propagating the underlying
    /// failure, so the download manager can route it into the output-root
    /// write gate via `gate.record_failure` without having to parse the
    /// generic `DownloadFailed` message field.
    ///
    /// This is how the gate detects mid-stream ENOSPC — the common 508
    /// scenario where the disk fills while today's date directory already
    /// exists, meaning `prepare_output_dir` is a no-op and cannot catch
    /// the failure on its own.
    DiskFull {
        /// Resolved output directory the engine was writing to. The
        /// manager passes this to `gate.record_failure` so `resolve_root`
        /// can determine the gate key.
        output_dir: PathBuf,
        /// Human-readable detail from the engine (e.g., "ffmpeg: No space
        /// left on device, exit 228"). Shown in logs and the gate's
        /// RootMeta.last_error_msg.
        detail: String,
    },
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
    async fn start(&self, handle: Arc<DownloadHandle>)
    -> std::result::Result<(), EngineStartError>;

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

    #[test]
    fn io_error_kind_classification() {
        use std::io::ErrorKind;
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::NotFound),
            IoErrorKindSer::NotFound
        );
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::StorageFull),
            IoErrorKindSer::StorageFull
        );
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::PermissionDenied),
            IoErrorKindSer::PermissionDenied
        );
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::ReadOnlyFilesystem),
            IoErrorKindSer::ReadOnlyFilesystem
        );
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::TimedOut),
            IoErrorKindSer::TimedOut
        );
        assert_eq!(
            IoErrorKindSer::from_io_kind(ErrorKind::ConnectionRefused),
            IoErrorKindSer::Other
        );
    }

    #[test]
    fn output_root_unavailable_does_not_affect_circuit_breaker() {
        let kind = DownloadFailureKind::OutputRootUnavailable {
            io_kind: IoErrorKindSer::NotFound,
        };
        assert!(!kind.affects_circuit_breaker());
        assert!(kind.is_recoverable());
    }

    #[test]
    fn engine_start_error_from_io_path_classifies_as_output_root_unavailable() {
        // crate::Error::IoPath wraps an io::Error in its source chain — the
        // From impl should walk the chain, find the io::Error, and produce
        // OutputRootUnavailable for the kinds we care about.
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such directory");
        let crate_err = crate::Error::io_path(
            "creating output directory",
            std::path::Path::new("/rec/huya/X/20260415"),
            io_err,
        );
        let engine_err: EngineStartError = crate_err.into();
        assert_eq!(
            engine_err.kind,
            DownloadFailureKind::OutputRootUnavailable {
                io_kind: IoErrorKindSer::NotFound
            }
        );
        // Path::display() on Windows uses backslashes; normalize before matching.
        assert!(
            engine_err.message.replace('\\', "/").contains("/rec/huya/X/20260415"),
            "msg={}",
            engine_err.message
        );
    }

    #[test]
    fn engine_start_error_from_storage_full_classifies_correctly() {
        let io_err = std::io::Error::new(std::io::ErrorKind::StorageFull, "no space");
        let crate_err: crate::Error = io_err.into();
        let engine_err: EngineStartError = crate_err.into();
        assert_eq!(
            engine_err.kind,
            DownloadFailureKind::OutputRootUnavailable {
                io_kind: IoErrorKindSer::StorageFull
            }
        );
    }

    #[test]
    fn engine_start_error_from_other_io_classifies_as_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "refused");
        let crate_err: crate::Error = io_err.into();
        let engine_err: EngineStartError = crate_err.into();
        assert_eq!(engine_err.kind, DownloadFailureKind::Io);
    }

    #[test]
    fn engine_start_error_from_non_io_error_classifies_as_other() {
        let crate_err = crate::Error::Validation("bad input".to_string());
        let engine_err: EngineStartError = crate_err.into();
        assert_eq!(engine_err.kind, DownloadFailureKind::Other);
    }

    #[test]
    fn io_error_kind_ser_string_matches_yaml_keys() {
        // These string values are used as the leaf segment of i18n keys
        // like `notification.output_path_inaccessible.description.<as_str>`.
        // Keep them in sync with `rust-srec/locales/{en,zh-CN}.yml`.
        assert_eq!(IoErrorKindSer::NotFound.as_str(), "not_found");
        assert_eq!(IoErrorKindSer::StorageFull.as_str(), "storage_full");
        assert_eq!(
            IoErrorKindSer::PermissionDenied.as_str(),
            "permission_denied"
        );
        assert_eq!(IoErrorKindSer::ReadOnlyFilesystem.as_str(), "read_only");
        assert_eq!(IoErrorKindSer::TimedOut.as_str(), "timed_out");
        assert_eq!(IoErrorKindSer::Other.as_str(), "other");
    }
}
