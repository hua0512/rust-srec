//! # Protocol Builders
//!
//! This module provides fluent builder APIs for creating protocol handlers
//! with specific configurations.

use crate::{
    CacheConfig, DownloadError, DownloaderConfig,
    dash::{AudioSelection, DashConfig, DashDownloader, LiveStartPolicy, VideoSelection},
    flv::{FlvDownloader, FlvProtocolConfig},
    hls::{
        HlsDownloader,
        config::{HlsConfig, HlsVariantSelectionPolicy as NewHlsVariantSelectionPolicy},
    },
    proxy::ProxyConfig,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{str::FromStr, time::Duration};

macro_rules! impl_base_downloader_config_methods {
    ($($base:ident).+) => {
        /// Set user agent for HTTP requests
        pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
            self.$($base).+.user_agent = user_agent.into();
            self
        }

        /// Set overall HTTP timeout
        pub fn timeout(mut self, timeout: Duration) -> Self {
            self.$($base).+.timeout = timeout;
            self
        }

        /// Set connection timeout
        pub fn connect_timeout(mut self, timeout: Duration) -> Self {
            self.$($base).+.connect_timeout = timeout;
            self
        }

        /// Set read timeout
        pub fn read_timeout(mut self, timeout: Duration) -> Self {
            self.$($base).+.read_timeout = timeout;
            self
        }

        /// Set whether to follow HTTP redirects
        pub fn follow_redirects(mut self, follow: bool) -> Self {
            self.$($base).+.follow_redirects = follow;
            self
        }

        /// Set HTTP headers
        pub fn headers(mut self, headers: HeaderMap) -> Self {
            self.$($base).+.headers = headers;
            self
        }

        /// Add a single HTTP header
        pub fn add_header(mut self, name: &str, value: &str) -> Self {
            if let (Ok(name), Ok(value)) = (HeaderName::from_str(name), HeaderValue::from_str(value)) {
                self.$($base).+.headers.insert(name, value);
            }
            self
        }
    };
}

/// Generic protocol builder trait
pub trait ProtocolBuilder {
    /// The protocol implementation type being built
    type Protocol;

    /// Build the protocol implementation
    fn build(self) -> Result<Self::Protocol, DownloadError>;
}

/// Builder for FLV protocol handlers
pub struct FlvProtocolBuilder {
    config: FlvProtocolConfig,
}

impl FlvProtocolBuilder {
    /// Create a new FLV protocol builder with default configuration
    pub fn new() -> Self {
        Self {
            config: FlvProtocolConfig::default(),
        }
    }

    /// Set buffer size for download operations
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Set the byte threshold for FLV progress events.
    pub fn progress_emit_min_bytes(mut self, bytes: u64) -> Self {
        self.config.progress_emit_min_bytes = bytes;
        self
    }

    /// Set the time threshold for FLV progress events.
    pub fn progress_emit_min_interval(mut self, interval: Duration) -> Self {
        self.config.progress_emit_min_interval = interval;
        self
    }

    impl_base_downloader_config_methods!(config.base);

    /// Access the raw configuration for more advanced customization
    pub fn with_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut FlvProtocolConfig),
    {
        f(&mut self.config);
        self
    }

    /// Get a copy of the current configuration
    pub fn get_config(&self) -> FlvProtocolConfig {
        self.config.clone()
    }
}

impl ProtocolBuilder for FlvProtocolBuilder {
    type Protocol = FlvDownloader;

    fn build(self) -> Result<Self::Protocol, DownloadError> {
        FlvDownloader::with_config(self.config)
    }
}

impl Default for FlvProtocolBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for HLS protocol handlers
pub struct HlsProtocolBuilder {
    config: HlsConfig,
}

impl HlsProtocolBuilder {
    /// Create a new HLS protocol builder with default configuration
    pub fn new() -> Self {
        Self {
            config: HlsConfig::default(),
        }
    }

    pub fn with_base_config(mut self, base_config: DownloaderConfig) -> Self {
        self.config.base = base_config;
        self
    }

    // --- Base DownloaderConfig methods ---

    impl_base_downloader_config_methods!(config.base);

    /// Set write timeout
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.write_timeout = timeout;
        self
    }

    /// Set proxy configuration
    pub fn proxy(mut self, proxy_config: ProxyConfig) -> Self {
        self.config.base.proxy = Some(proxy_config);
        self
    }

    /// Set whether to use system proxy settings
    pub fn use_system_proxy(mut self, use_system_proxy: bool) -> Self {
        self.config.base.use_system_proxy = use_system_proxy;
        self
    }

    /// Set whether to accept invalid TLS certificates (use with caution)
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.config.base.danger_accept_invalid_certs = accept;
        self
    }

    /// Set base downloader cache configuration
    pub fn downloader_cache_config(mut self, cache_config: CacheConfig) -> Self {
        self.config.base.cache_config = Some(cache_config);
        self
    }

    // --- HLS PlaylistConfig methods ---

    /// Set timeout for fetching the initial playlist.
    pub fn initial_playlist_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.config.playlist_config.initial_playlist_fetch_timeout = timeout;
        self
    }

    /// Set minimum interval for refreshing live playlists.
    pub fn live_refresh_interval(mut self, interval: Duration) -> Self {
        self.config.playlist_config.live_refresh_interval = interval;
        self
    }

    /// Set maximum number of retries for refreshing live playlists.
    pub fn live_max_refresh_retries(mut self, retries: u32) -> Self {
        self.config.playlist_config.live_max_refresh_retries = retries;
        self
    }

    /// Set delay between retries for refreshing live playlists.
    pub fn live_refresh_retry_delay(mut self, delay: Duration) -> Self {
        self.config.playlist_config.live_refresh_retry_delay = delay;
        self
    }

    /// Set the variant selection policy.
    pub fn variant_selection_policy(mut self, policy: NewHlsVariantSelectionPolicy) -> Self {
        self.config.playlist_config.variant_selection_policy = policy;
        self
    }

    /// Select the variant with the highest bitrate.
    pub fn select_highest_bitrate_variant(mut self) -> Self {
        self.config.playlist_config.variant_selection_policy =
            NewHlsVariantSelectionPolicy::HighestBitrate;
        self
    }

    /// Select the variant with the lowest bitrate.
    pub fn select_lowest_bitrate_variant(mut self) -> Self {
        self.config.playlist_config.variant_selection_policy =
            NewHlsVariantSelectionPolicy::LowestBitrate;
        self
    }

    /// Select the variant closest to the specified bitrate.
    pub fn select_variant_by_closest_bitrate(mut self, bitrate: u64) -> Self {
        self.config.playlist_config.variant_selection_policy =
            NewHlsVariantSelectionPolicy::ClosestToBitrate(bitrate);
        self
    }

    // --- HLS SchedulerConfig methods ---

    /// Set maximum concurrent segment downloads.
    pub fn download_concurrency(mut self, concurrency: usize) -> Self {
        self.config.scheduler_config.download_concurrency = concurrency;
        self
    }

    // --- HLS FetcherConfig methods ---

    /// Set timeout for downloading a single segment.
    pub fn segment_download_timeout(mut self, timeout: Duration) -> Self {
        self.config.fetcher_config.segment_download_timeout = timeout;
        self
    }

    /// Set the byte threshold for HLS segment progress events.
    pub fn progress_emit_min_bytes(mut self, bytes: u64) -> Self {
        self.config.fetcher_config.progress_emit_min_bytes = bytes;
        self
    }

    /// Set the time threshold for HLS segment progress events.
    pub fn progress_emit_min_interval(mut self, interval: Duration) -> Self {
        self.config.fetcher_config.progress_emit_min_interval = interval;
        self
    }

    /// Set maximum number of retries for downloading a segment.
    pub fn max_segment_retries(mut self, retries: u32) -> Self {
        self.config.fetcher_config.max_segment_retries = retries;
        self
    }

    /// Set base delay for exponential backoff when retrying segment downloads.
    pub fn segment_retry_delay_base(mut self, delay: Duration) -> Self {
        self.config.fetcher_config.segment_retry_delay_base = delay;
        self
    }

    /// Set timeout for downloading a decryption key.
    pub fn key_download_timeout(mut self, timeout: Duration) -> Self {
        self.config.fetcher_config.key_download_timeout = timeout;
        self
    }

    /// Set maximum number of retries for downloading a decryption key.
    pub fn max_key_retries(mut self, retries: u32) -> Self {
        self.config.fetcher_config.max_key_retries = retries;
        self
    }

    /// Set base delay for exponential backoff when retrying key downloads.
    pub fn key_retry_delay_base(mut self, delay: Duration) -> Self {
        self.config.fetcher_config.key_retry_delay_base = delay;
        self
    }

    // --- HLS ProcessorConfig methods ---

    /// Set TTL for caching processed (decrypted) segments.
    pub fn processed_segment_ttl(mut self, ttl: Duration) -> Self {
        self.config.processor_config.processed_segment_ttl = ttl;
        self
    }

    // --- HLS DecryptionConfig methods ---

    /// Set TTL for decryption keys in the in-memory cache.
    pub fn decryption_key_cache_ttl(mut self, ttl: Duration) -> Self {
        self.config.decryption_config.key_cache_ttl = ttl;
        self
    }

    /// Set whether to use a separate thread pool for decryption.
    pub fn offload_decryption_to_cpu_pool(mut self, offload: bool) -> Self {
        self.config.decryption_config.offload_decryption_to_cpu_pool = offload;
        self
    }

    // --- HLS CacheConfig methods (HLS-specific cache) ---

    /// Set TTL for playlists in the HLS cache.
    pub fn hls_playlist_cache_ttl(mut self, ttl: Duration) -> Self {
        self.config.cache_config.playlist_ttl = ttl;
        self
    }

    /// Set TTL for segments in the HLS cache.
    pub fn hls_segment_cache_ttl(mut self, ttl: Duration) -> Self {
        self.config.cache_config.segment_ttl = ttl;
        self
    }

    /// Set TTL for decryption keys in the HLS cache.
    pub fn hls_decryption_key_cache_ttl(mut self, ttl: Duration) -> Self {
        self.config.cache_config.decryption_key_ttl = ttl;
        self
    }

    // --- HLS OutputConfig methods ---

    /// Set the maximum duration to wait for a segment before considering it stalled.
    /// If None, this timeout is disabled.
    pub fn live_max_overall_stall_duration(mut self, duration: Option<Duration>) -> Self {
        self.config.output_config.live_max_overall_stall_duration = duration;
        self
    }

    // --- General Builder Methods ---

    /// Access the raw HLS configuration for more advanced customization.
    pub fn with_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut HlsConfig),
    {
        f(&mut self.config);
        self
    }

    /// Get a copy of the current HLS configuration.
    pub fn get_config(&self) -> HlsConfig {
        self.config.clone()
    }
}

impl ProtocolBuilder for HlsProtocolBuilder {
    type Protocol = HlsDownloader;

    fn build(self) -> Result<Self::Protocol, DownloadError> {
        HlsDownloader::with_config(self.config)
    }
}

impl Default for HlsProtocolBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for DASH protocol handlers
pub struct DashProtocolBuilder {
    config: DashConfig,
}

impl DashProtocolBuilder {
    /// Create a new DASH protocol builder with default configuration
    pub fn new() -> Self {
        Self {
            config: DashConfig::default(),
        }
    }

    pub fn with_base_config(mut self, base_config: DownloaderConfig) -> Self {
        self.config.base = base_config;
        self
    }

    // --- Base DownloaderConfig methods ---

    impl_base_downloader_config_methods!(config.base);

    /// Set proxy configuration
    pub fn proxy(mut self, proxy_config: ProxyConfig) -> Self {
        self.config.base.proxy = Some(proxy_config);
        self
    }

    /// Set whether to use system proxy settings
    pub fn use_system_proxy(mut self, use_system_proxy: bool) -> Self {
        self.config.base.use_system_proxy = use_system_proxy;
        self
    }

    /// Set whether to accept invalid TLS certificates (use with caution)
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.config.base.danger_accept_invalid_certs = accept;
        self
    }

    // --- Manifest ---

    /// Set the timeout of one MPD request.
    pub fn manifest_fetch_timeout(mut self, timeout: Duration) -> Self {
        self.config.manifest.fetch_timeout = timeout;
        self
    }

    /// Set the lower bound of the MPD refresh interval.
    pub fn min_refresh_interval(mut self, interval: Duration) -> Self {
        self.config.manifest.min_refresh_interval = interval;
        self
    }

    /// Set how many consecutive MPD refresh failures are tolerated.
    pub fn max_refresh_retries(mut self, retries: u32) -> Self {
        self.config.manifest.max_refresh_retries = retries;
        self
    }

    /// Set the delay after a failed MPD refresh.
    pub fn refresh_retry_delay(mut self, delay: Duration) -> Self {
        self.config.manifest.retry_delay = delay;
        self
    }

    /// Set where recording of a live presentation starts.
    pub fn live_start(mut self, policy: LiveStartPolicy) -> Self {
        self.config.live_start = policy;
        self
    }

    // --- Selection ---

    /// Set the video Representation policy.
    pub fn video_selection(mut self, policy: VideoSelection) -> Self {
        self.config.selection.video = policy;
        self
    }

    /// Set the audio Representation policy.
    pub fn audio_selection(mut self, policy: AudioSelection) -> Self {
        self.config.selection.audio = policy;
        self
    }

    /// Prefer audio in this language (RFC 5646 tag).
    pub fn audio_language(mut self, language: impl Into<String>) -> Self {
        self.config.selection.audio_language = Some(language.into());
        self
    }

    /// Record the video track (default: true).
    pub fn include_video(mut self, include: bool) -> Self {
        self.config.selection.include_video = include;
        self
    }

    /// Record the audio track (default: true).
    pub fn include_audio(mut self, include: bool) -> Self {
        self.config.selection.include_audio = include;
        self
    }

    // --- Segments ---

    /// Set maximum concurrent segment downloads per track.
    pub fn download_concurrency(mut self, concurrency: usize) -> Self {
        self.config.scheduler.download_concurrency = concurrency;
        self
    }

    /// Set timeout for downloading a single segment.
    pub fn segment_download_timeout(mut self, timeout: Duration) -> Self {
        self.config.fetcher.segment_download_timeout = timeout;
        self
    }

    /// Set maximum number of retries for downloading a segment.
    pub fn max_segment_retries(mut self, retries: u32) -> Self {
        self.config.fetcher.max_segment_retries = retries;
        self
    }

    // --- Muxing ---

    /// Set how long a video segment waits for its audio.
    pub fn max_audio_lag(mut self, lag: Duration) -> Self {
        self.config.mux.max_audio_lag = lag;
        self
    }

    /// Set the bytes the muxer may buffer across both tracks.
    pub fn max_mux_buffered_bytes(mut self, bytes: usize) -> Self {
        self.config.mux.max_buffered_bytes = bytes;
        self
    }

    // --- General Builder Methods ---

    /// Access the raw DASH configuration for more advanced customization.
    pub fn with_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut DashConfig),
    {
        f(&mut self.config);
        self
    }

    /// Get a copy of the current DASH configuration.
    pub fn get_config(&self) -> DashConfig {
        self.config.clone()
    }
}

impl ProtocolBuilder for DashProtocolBuilder {
    type Protocol = DashDownloader;

    fn build(self) -> Result<Self::Protocol, DownloadError> {
        DashDownloader::with_config(self.config)
    }
}

impl Default for DashProtocolBuilder {
    fn default() -> Self {
        Self::new()
    }
}
