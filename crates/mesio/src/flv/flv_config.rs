//! # FLV Protocol Configuration
//!
//! This module defines the configuration options specific to FLV downloads.

use std::time::Duration;

use crate::DownloaderConfig;

/// Configuration for FLV downloads
#[derive(Debug, Clone)]
pub struct FlvProtocolConfig {
    /// Base downloader configuration
    pub base: DownloaderConfig,
    /// Buffer size for download chunks (in bytes)
    pub buffer_size: usize,
    /// Maximum time without a media tag before the stream fails.
    ///
    /// The HTTP read timeout only sees bytes; a source that trickles bytes
    /// without producing audio/video tags keeps it from ever firing. This
    /// watchdog watches decoded tags instead. `None` disables it.
    pub media_stall_timeout: Option<Duration>,
    /// Minimum bytes accumulated before a `DownloadEvent::Progress` is emitted.
    /// Set to `0` to emit once per network chunk.
    pub progress_emit_min_bytes: u64,
    /// Maximum interval between `DownloadEvent::Progress` emissions while bytes
    /// are arriving. Set to `Duration::ZERO` to emit once per network chunk.
    pub progress_emit_min_interval: Duration,
}

const DEFAULT_BUFFER_SIZE: usize = 64 * 1024; // 64KB default buffer size
const DEFAULT_MEDIA_STALL_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_PROGRESS_EMIT_MIN_BYTES: u64 = 256 * 1024;
const DEFAULT_PROGRESS_EMIT_MIN_INTERVAL: Duration = Duration::from_millis(100);

impl Default for FlvProtocolConfig {
    fn default() -> Self {
        Self {
            base: DownloaderConfig::default(),
            buffer_size: DEFAULT_BUFFER_SIZE,
            media_stall_timeout: Some(DEFAULT_MEDIA_STALL_TIMEOUT),
            progress_emit_min_bytes: DEFAULT_PROGRESS_EMIT_MIN_BYTES,
            progress_emit_min_interval: DEFAULT_PROGRESS_EMIT_MIN_INTERVAL,
        }
    }
}

impl From<DownloaderConfig> for FlvProtocolConfig {
    fn from(base: DownloaderConfig) -> Self {
        Self {
            base,
            buffer_size: DEFAULT_BUFFER_SIZE,
            media_stall_timeout: Some(DEFAULT_MEDIA_STALL_TIMEOUT),
            progress_emit_min_bytes: DEFAULT_PROGRESS_EMIT_MIN_BYTES,
            progress_emit_min_interval: DEFAULT_PROGRESS_EMIT_MIN_INTERVAL,
        }
    }
}

impl FlvProtocolConfig {
    /// Create a new builder for FlvConfig
    pub fn builder() -> FlvProtocolConfigBuilder {
        FlvProtocolConfigBuilder::new()
    }
}

/// Builder for FlvProtocolConfig
#[derive(Debug, Clone)]
pub struct FlvProtocolConfigBuilder {
    base: DownloaderConfig,
    buffer_size: usize,
    media_stall_timeout: Option<Duration>,
    progress_emit_min_bytes: u64,
    progress_emit_min_interval: Duration,
}

impl FlvProtocolConfigBuilder {
    /// Create a new FlvProtocolConfigBuilder with default values
    pub fn new() -> Self {
        Self {
            base: DownloaderConfig::default(),
            buffer_size: DEFAULT_BUFFER_SIZE,
            media_stall_timeout: Some(DEFAULT_MEDIA_STALL_TIMEOUT),
            progress_emit_min_bytes: DEFAULT_PROGRESS_EMIT_MIN_BYTES,
            progress_emit_min_interval: DEFAULT_PROGRESS_EMIT_MIN_INTERVAL,
        }
    }

    /// Set the base downloader configuration
    pub fn with_base_config(mut self, base: DownloaderConfig) -> Self {
        self.base = base;
        self
    }

    /// Set the buffer size for download chunks
    pub fn buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Set how long the stream may go without a media tag before it fails.
    /// `None` or a zero duration disables the watchdog.
    pub fn media_stall_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.media_stall_timeout = timeout.filter(|timeout| !timeout.is_zero());
        self
    }

    pub fn progress_emit_min_bytes(mut self, bytes: u64) -> Self {
        self.progress_emit_min_bytes = bytes;
        self
    }

    pub fn progress_emit_min_interval(mut self, interval: Duration) -> Self {
        self.progress_emit_min_interval = interval;
        self
    }

    /// Build the FlvProtocolConfig
    pub fn build(self) -> FlvProtocolConfig {
        FlvProtocolConfig {
            base: self.base,
            buffer_size: self.buffer_size,
            media_stall_timeout: self.media_stall_timeout,
            progress_emit_min_bytes: self.progress_emit_min_bytes,
            progress_emit_min_interval: self.progress_emit_min_interval,
        }
    }
}

impl Default for FlvProtocolConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}
