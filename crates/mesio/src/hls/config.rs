use std::time::Duration;

use crate::DownloaderConfig;

// --- Performance Configuration Types ---

/// Configuration for segment prefetching
#[derive(Debug, Clone)]
pub struct PrefetchConfig {
    /// Enable prefetching
    pub enabled: bool,
    /// Number of segments to prefetch ahead
    pub prefetch_count: usize,
    /// Maximum buffer size before skipping prefetch
    pub max_buffer_before_skip: usize,
}

impl Default for PrefetchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            prefetch_count: 2,
            max_buffer_before_skip: 40,
        }
    }
}

/// Configuration for buffer pooling
#[derive(Debug, Clone)]
pub struct BufferPoolConfig {
    /// Enable buffer pooling
    pub enabled: bool,
    /// Maximum buffers to keep in pool
    pub pool_size: usize,
    /// Default buffer capacity
    pub default_capacity: usize,
}

impl Default for BufferPoolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            pool_size: 10,
            default_capacity: 2 * 1024 * 1024, // 2MB
        }
    }
}

/// Configuration for batch scheduling
#[derive(Debug, Clone)]
pub struct BatchSchedulerConfig {
    /// Enable batch scheduling
    pub enabled: bool,
    /// Time window to collect batch (ms)
    pub batch_window_ms: u64,
    /// Maximum segments per batch
    pub max_batch_size: usize,
}

impl Default for BatchSchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            batch_window_ms: 50,
            max_batch_size: 5,
        }
    }
}

/// Aggregated performance configuration for HLS pipeline
#[derive(Debug, Clone)]
pub struct HlsPerformanceConfig {
    /// Decryption offloading to blocking thread pool
    pub decryption_offload_enabled: bool,
    /// Prefetch configuration
    pub prefetch: PrefetchConfig,
    /// Buffer pool configuration
    pub buffer_pool: BufferPoolConfig,
    /// Batch scheduler configuration
    pub batch_scheduler: BatchSchedulerConfig,
    /// Zero-copy forwarding enabled
    pub zero_copy_enabled: bool,
    /// Performance metrics enabled
    pub metrics_enabled: bool,
}

impl Default for HlsPerformanceConfig {
    fn default() -> Self {
        Self {
            decryption_offload_enabled: true,
            prefetch: PrefetchConfig::default(),
            buffer_pool: BufferPoolConfig::default(),
            batch_scheduler: BatchSchedulerConfig::default(),
            zero_copy_enabled: true,
            metrics_enabled: true,
        }
    }
}

// --- Gap Skip Strategy ---
/// Strategy for handling gaps in segment sequences
#[derive(Debug, Clone)]
pub enum GapSkipStrategy {
    /// Wait indefinitely for missing segments (VOD default)
    WaitIndefinitely,
    /// Skip after receiving N subsequent segments
    SkipAfterCount(u64),
    /// Skip after waiting for a duration
    SkipAfterDuration(Duration),
    /// Skip when EITHER count OR duration threshold is exceeded
    SkipAfterBoth { count: u64, duration: Duration },
}

impl Default for GapSkipStrategy {
    fn default() -> Self {
        // Default: skip after 10 segments OR 5 second for live.
        // Rationale: with concurrent downloads, segments can arrive out-of-order.
        // A low count threshold (e.g. 3) can cause false skips where the missing
        // segment arrives shortly after the skip decision.
        GapSkipStrategy::SkipAfterBoth {
            count: 10,
            duration: Duration::from_secs(5),
        }
    }
}

// --- Buffer Limits ---
/// Configuration for buffer size limits
#[derive(Debug, Clone)]
pub struct BufferLimits {
    /// Maximum number of segments in buffer (0 = unlimited)
    pub max_segments: usize,
    /// Maximum total bytes in buffer (0 = unlimited)
    pub max_bytes: usize,
}

impl Default for BufferLimits {
    fn default() -> Self {
        Self {
            max_segments: 50,             // Reasonable default for most streams
            max_bytes: 100 * 1024 * 1024, // 100MB
        }
    }
}

// --- Top-Level Configuration ---
#[derive(Debug, Clone, Default)]
pub struct HlsConfig {
    /// Base downloader configuration
    pub base: DownloaderConfig,
    pub playlist_config: HlsPlaylistConfig,
    pub scheduler_config: HlsSchedulerConfig,
    pub fetcher_config: HlsFetcherConfig,
    pub processor_config: HlsProcessorConfig,
    pub decryption_config: HlsDecryptionConfig,
    pub cache_config: HlsCacheConfig,
    pub output_config: HlsOutputConfig,
    /// Performance optimization configuration
    pub performance_config: HlsPerformanceConfig,
}

// --- Playlist Configuration ---
#[derive(Debug, Clone)]
pub struct HlsPlaylistConfig {
    pub initial_playlist_fetch_timeout: Duration,
    pub live_refresh_interval: Duration, // Minimum interval for refreshing live playlists
    pub live_max_refresh_retries: u32,
    pub live_refresh_retry_delay: Duration,
    pub variant_selection_policy: HlsVariantSelectionPolicy,
    /// Enable adaptive refresh interval based on actual segment arrival rate
    pub adaptive_refresh_enabled: bool,
    /// Minimum adaptive refresh interval (won't go below this)
    pub adaptive_refresh_min_interval: Duration,
    /// Maximum adaptive refresh interval (won't go above this)
    pub adaptive_refresh_max_interval: Duration,
}

impl Default for HlsPlaylistConfig {
    fn default() -> Self {
        Self {
            initial_playlist_fetch_timeout: Duration::from_secs(15),
            live_refresh_interval: Duration::from_secs(1),
            live_max_refresh_retries: 5,
            live_refresh_retry_delay: Duration::from_secs(1),
            variant_selection_policy: Default::default(),
            adaptive_refresh_enabled: true,
            adaptive_refresh_min_interval: Duration::from_millis(500),
            adaptive_refresh_max_interval: Duration::from_secs(3),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum HlsVariantSelectionPolicy {
    #[default]
    HighestBitrate, // Select the variant with the highest bandwidth
    LowestBitrate,
    ClosestToBitrate(u64), // Select variant closest to the specified bitrate
    AudioOnly,             // If an audio-only variant exists
    VideoOnly,             // If a video-only variant exists (less common for HLS main content)
    MatchingResolution {
        width: u32,
        height: u32,
    },
    Custom(String), // For future extensibility, e.g., a name or specific tag
}

// --- Scheduler Configuration ---
#[derive(Debug, Clone)]
pub struct HlsSchedulerConfig {
    /// Max concurrent segment downloads (default: 5)
    pub download_concurrency: usize,
    /// Channel buffer multiplier for processed segments (default: 4)
    /// Actual buffer size = download_concurrency * buffer_multiplier
    pub processed_segment_buffer_multiplier: usize,
}

impl Default for HlsSchedulerConfig {
    fn default() -> Self {
        Self {
            download_concurrency: 5,
            processed_segment_buffer_multiplier: 4,
        }
    }
}

// --- Fetcher Configuration ---
#[derive(Debug, Clone)]
pub struct HlsFetcherConfig {
    pub segment_download_timeout: Duration,
    pub max_segment_retries: u32,
    pub segment_retry_delay_base: Duration, // Base for exponential backoff
    pub key_download_timeout: Duration,
    pub max_key_retries: u32,
    pub key_retry_delay_base: Duration,
    pub segment_raw_cache_ttl: Duration, // TTL for caching raw (undecrypted) segments
    /// Threshold in bytes above which segments are streamed instead of buffered entirely
    /// This reduces memory spikes for large segments (default: 2MB)
    pub streaming_threshold_bytes: usize,
}

impl Default for HlsFetcherConfig {
    fn default() -> Self {
        Self {
            segment_download_timeout: Duration::from_secs(10),
            max_segment_retries: 3,
            segment_retry_delay_base: Duration::from_millis(500),
            key_download_timeout: Duration::from_secs(5),
            max_key_retries: 3,
            key_retry_delay_base: Duration::from_millis(200),
            segment_raw_cache_ttl: Duration::from_secs(60), // Default 1 minutes for raw segments
            streaming_threshold_bytes: 2 * 1024 * 1024,     // 2MB threshold for streaming
        }
    }
}

// --- Processor Configuration ---
#[derive(Debug, Clone)]
pub struct HlsProcessorConfig {
    // Configuration specific to segment processing, if any beyond decryption
    // e.g., if transmuxing options were added.
    pub processed_segment_ttl: Duration, // TTL for caching processed (decrypted) segments
}

impl Default for HlsProcessorConfig {
    fn default() -> Self {
        Self {
            processed_segment_ttl: Duration::from_secs(60), // Default 1 minutes for processed segments
        }
    }
}

// --- Decryption Configuration ---
#[derive(Debug, Clone)]
pub struct HlsDecryptionConfig {
    pub key_cache_ttl: Duration, // TTL for keys in the in-memory cache
    pub offload_decryption_to_cpu_pool: bool, // Whether to use a separate thread pool for decryption
}

impl Default for HlsDecryptionConfig {
    fn default() -> Self {
        Self {
            key_cache_ttl: Duration::from_secs(60 * 60), // Default to 1 hour TTL for keys
            offload_decryption_to_cpu_pool: false,       // Default to inline async decryption
        }
    }
}

// --- Cache Configuration ---
#[derive(Debug, Clone)]
pub struct HlsCacheConfig {
    pub playlist_ttl: Duration,
    pub segment_ttl: Duration, // TTL for processed (decrypted) segments
    pub decryption_key_ttl: Duration,
}

impl Default for HlsCacheConfig {
    fn default() -> Self {
        Self {
            playlist_ttl: Duration::from_secs(60), // Cache playlists for a minute
            segment_ttl: Duration::from_secs(2 * 60), // Cache segments for 1 minutes
            decryption_key_ttl: Duration::from_secs(60 * 60), // Cache keys for an hour
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsOutputConfig {
    /// Max duration of segments to hold in reorder buffer
    pub live_reorder_buffer_duration: Duration,
    /// Max number of segments in reorder buffer
    pub live_reorder_buffer_max_segments: usize,

    /// Duration to wait for a segment to be received before considering it stalled.
    /// If the overall stall duration exceeds this value, the downloader will throw an error.
    /// If None, this timeout is disabled.
    pub live_max_overall_stall_duration: Option<Duration>,

    /// Gap skip strategy for live streams
    pub live_gap_strategy: GapSkipStrategy,
    /// Gap skip strategy for VOD streams (default: WaitIndefinitely)
    pub vod_gap_strategy: GapSkipStrategy,
    /// Per-segment timeout for VOD (None = wait indefinitely)
    pub vod_segment_timeout: Option<Duration>,
    /// Buffer limits for memory management
    pub buffer_limits: BufferLimits,
    /// Enable metrics collection
    pub metrics_enabled: bool,
}

impl Default for HlsOutputConfig {
    fn default() -> Self {
        Self {
            live_reorder_buffer_duration: Duration::from_secs(30),
            live_reorder_buffer_max_segments: 10,
            live_max_overall_stall_duration: Some(Duration::from_secs(60)),
            live_gap_strategy: GapSkipStrategy::default(),
            vod_gap_strategy: GapSkipStrategy::WaitIndefinitely,
            vod_segment_timeout: None,
            buffer_limits: BufferLimits::default(),
            metrics_enabled: true,
        }
    }
}

// Implement the marker trait from the main crate
impl crate::media_protocol::ProtocolConfig for HlsConfig {}
