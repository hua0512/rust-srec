//! # Protocol Builders
//!
//! This module provides fluent builder APIs for creating protocol handlers
//! with specific configurations.

use crate::{
    CacheConfig, DownloadError, DownloaderConfig,
    flv::{FlvConfig, FlvDownloader},
    hls::{
        HlsDownloader,
        config::{HlsConfig, HlsVariantSelectionPolicy as NewHlsVariantSelectionPolicy},
    },
    proxy::ProxyConfig,
};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use std::{str::FromStr, time::Duration};
// use url::Url; // For proxy URL in NewHttpClientConfig - ProxyConfig itself handles Url

/// Generic protocol builder trait
pub trait ProtocolBuilder {
    /// The protocol implementation type being built
    type Protocol;

    /// Build the protocol implementation
    fn build(self) -> Result<Self::Protocol, DownloadError>;
}

/// Builder for FLV protocol handlers
pub struct FlvProtocolBuilder {
    config: FlvConfig,
}

impl FlvProtocolBuilder {
    /// Create a new FLV protocol builder with default configuration
    pub fn new() -> Self {
        Self {
            config: FlvConfig::default(),
        }
    }

    /// Set buffer size for download operations
    pub fn buffer_size(mut self, size: usize) -> Self {
        self.config.buffer_size = size;
        self
    }

    /// Set user agent for HTTP requests
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.config.base.user_agent = user_agent.into();
        self
    }

    /// Set HTTP timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.base.timeout = timeout;
        self
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.connect_timeout = timeout;
        self
    }

    /// Set read timeout
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.read_timeout = timeout;
        self
    }

    /// Set whether to follow HTTP redirects
    pub fn follow_redirects(mut self, follow: bool) -> Self {
        self.config.base.follow_redirects = follow;
        self
    }

    /// Set HTTP headers
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.config.base.headers = headers;
        self
    }

    /// Add a single HTTP header
    pub fn add_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (HeaderName::from_str(name), HeaderValue::from_str(value)) {
            self.config.base.headers.insert(name, value);
        }
        self
    }

    /// Access the raw configuration for more advanced customization
    pub fn with_config<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut FlvConfig),
    {
        f(&mut self.config);
        self
    }

    /// Get a copy of the current configuration
    pub fn get_config(&self) -> FlvConfig {
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

    /// Set user agent for HTTP requests
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.config.base.user_agent = user_agent.into();
        self
    }

    /// Set overall HTTP timeout
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.config.base.timeout = timeout;
        self
    }

    /// Set connection timeout
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.connect_timeout = timeout;
        self
    }

    /// Set read timeout
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.read_timeout = timeout;
        self
    }

    /// Set write timeout
    pub fn write_timeout(mut self, timeout: Duration) -> Self {
        self.config.base.write_timeout = timeout;
        self
    }

    /// Set whether to follow HTTP redirects
    pub fn follow_redirects(mut self, follow: bool) -> Self {
        self.config.base.follow_redirects = follow;
        self
    }

    /// Set HTTP headers
    pub fn headers(mut self, headers: HeaderMap) -> Self {
        self.config.base.headers = headers;
        self
    }

    /// Add a single HTTP header
    pub fn add_header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (HeaderName::from_str(name), HeaderValue::from_str(value)) {
            self.config.base.headers.insert(name, value);
        }
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

    /// Set maximum number of retries for downloading a segment.
    /// Alias for `max_segment_retries`.
    pub fn segment_retry_count(mut self, retries: u32) -> Self {
        self.config.fetcher_config.max_segment_retries = retries;
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

    /// Set TTL for caching raw (undecrypted) segments.
    pub fn segment_raw_cache_ttl(mut self, ttl: Duration) -> Self {
        self.config.fetcher_config.segment_raw_cache_ttl = ttl;
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

    /// Set maximum duration of segments to hold in the reorder buffer for live streams.
    pub fn live_reorder_buffer_duration(mut self, duration: Duration) -> Self {
        self.config.output_config.live_reorder_buffer_duration = duration;
        self
    }

    /// Set maximum number of segments to hold in the reorder buffer for live streams.
    pub fn live_reorder_buffer_max_segments(mut self, max_segments: usize) -> Self {
        self.config.output_config.live_reorder_buffer_max_segments = max_segments;
        self
    }

    /// Enable or disable skipping of missing segments in a live stream.
    pub fn live_gap_skip_enabled(mut self, enabled: bool) -> Self {
        self.config.output_config.live_gap_skip_enabled = enabled;
        self
    }

    /// Set the number of newer media segments that must be received after a gap
    /// before attempting to skip missing segments.
    pub fn live_gap_skip_threshold_segments(mut self, threshold: u64) -> Self {
        self.config.output_config.live_gap_skip_threshold_segments = threshold;
        self
    }

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
