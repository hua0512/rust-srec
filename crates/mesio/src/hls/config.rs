use std::time::Duration;

use crate::DownloaderConfig;

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

// --- Engine (reactor) Configuration ---

/// Identity policy selection for segment/key dedup across playlist refreshes.
#[derive(Debug, Clone, Default)]
pub enum IdentityPolicyConfig {
    /// The full resolved URL is the identity (safe default: never
    /// under-deduplicates). Rotated auth params fork identity under this
    /// policy.
    #[default]
    FullUrl,
    /// Identity strips the listed query keys (rotating tokens, signatures,
    /// expiries) and sorts the rest. Only enable for sources whose token
    /// scheme is known.
    StripQueryKeys(Vec<String>),
}

/// Byte budgets, pending bounds, and lifecycle retry settings for the
/// scheduler reactor.
#[derive(Debug, Clone)]
pub struct HlsEngineConfig {
    /// Raw response-body bytes admitted concurrently (0 = unlimited).
    /// Reserved at admission, reconciled against Content-Length and the
    /// streamed body.
    pub max_inflight_download_bytes: u64,
    /// Decrypted/transformed output resident in the crypto stage
    /// (0 = unlimited). Reserved at the encrypted-input upper bound.
    pub max_processing_bytes: u64,
    /// Completed payload bytes buffered in the reactor between completion and
    /// the downstream permit-send.
    pub max_pending_payload_bytes: u64,
    /// Total `AssemblerInput` items buffered in the reactor, bounding the
    /// near-zero-byte control items that payload bytes do not.
    pub max_pending_items: usize,
    /// Per-segment size estimate used at admission before any segment has
    /// completed; afterwards an EMA of actual sizes takes over.
    pub initial_segment_size_estimate: u64,
    /// Hard per-segment cap (0 = disabled); a body exceeding it terminalizes
    /// as oversize.
    pub max_segment_size_bytes: u64,
    /// Lifecycle reschedule budget per segment (distinct from the tight
    /// per-attempt HTTP retries inside the fetch task).
    pub lifecycle_retry_budget: u32,
    pub lifecycle_retry_delay_base: Duration,
    pub lifecycle_retry_delay_max: Duration,
    /// Control-plane record backstop (applied within the window-prune
    /// invariant).
    pub max_state_entries: usize,
    /// Init-segment records retained across window slides.
    pub max_retained_inits: usize,
    /// Identity policy for segment and key dedup.
    pub identity_policy: IdentityPolicyConfig,
    /// Decryption key cache entries.
    pub key_cache_max_entries: u64,
}

impl Default for HlsEngineConfig {
    fn default() -> Self {
        Self {
            max_inflight_download_bytes: 64 * 1024 * 1024,
            max_processing_bytes: 32 * 1024 * 1024,
            max_pending_payload_bytes: 32 * 1024 * 1024,
            max_pending_items: 1024,
            initial_segment_size_estimate: 2 * 1024 * 1024,
            max_segment_size_bytes: 0,
            lifecycle_retry_budget: 3,
            lifecycle_retry_delay_base: Duration::from_millis(500),
            lifecycle_retry_delay_max: Duration::from_secs(10),
            max_state_entries: 2048,
            max_retained_inits: 8,
            identity_policy: IdentityPolicyConfig::default(),
            key_cache_max_entries: 64,
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
    /// Scheduler-reactor budgets and lifecycle retry settings.
    pub engine_config: HlsEngineConfig,
}

// --- Playlist Configuration ---
#[derive(Debug, Clone)]
pub struct HlsPlaylistConfig {
    pub initial_playlist_fetch_timeout: Duration,
    pub live_refresh_interval: Duration, // Minimum interval for refreshing live playlists
    pub live_max_refresh_retries: u32,
    pub live_refresh_retry_delay: Duration,
    pub variant_selection_policy: HlsVariantSelectionPolicy,
    pub segment_lifecycle_max_entries: usize,
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
            segment_lifecycle_max_entries: 512,
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
    pub max_segment_retry_delay: Duration,  // Hard cap on exponential backoff growth
    pub key_download_timeout: Duration,
    pub max_key_retries: u32,
    pub key_retry_delay_base: Duration,
    pub max_key_retry_delay: Duration, // Hard cap on key retry backoff growth
    /// Minimum bytes accumulated before a `DownloadEvent::Progress` is emitted.
    /// Set to `0` to emit once per network chunk.
    pub progress_emit_min_bytes: u64,
    /// Maximum interval between `DownloadEvent::Progress` emissions while bytes
    /// are arriving. Set to `Duration::ZERO` to emit once per network chunk.
    pub progress_emit_min_interval: Duration,
}

impl Default for HlsFetcherConfig {
    fn default() -> Self {
        Self {
            segment_download_timeout: Duration::from_secs(10),
            max_segment_retries: 3,
            segment_retry_delay_base: Duration::from_millis(500),
            max_segment_retry_delay: Duration::from_secs(10),
            key_download_timeout: Duration::from_secs(5),
            max_key_retries: 3,
            key_retry_delay_base: Duration::from_millis(200),
            max_key_retry_delay: Duration::from_secs(5),
            progress_emit_min_bytes: 256 * 1024,
            progress_emit_min_interval: Duration::from_millis(100),
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
            offload_decryption_to_cpu_pool: true,        // Default: offload crypto to blocking pool
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

    /// How often to wake up and re-evaluate gap policies when stalled.
    ///
    /// This ensures duration-based gap skipping and VOD timeouts can trigger even
    /// if no new segments arrive (or input is paused by backpressure).
    pub gap_evaluation_interval: Duration,
    /// Maximum number of pending fMP4 init segments to keep.
    ///
    /// Init segments are tracked separately from the media reorder buffer and
    /// can otherwise grow without bound on long-running streams.
    ///
    /// `0` disables the limit.
    pub max_pending_init_segments: usize,
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
            gap_evaluation_interval: Duration::from_millis(200),
            max_pending_init_segments: 8,
            live_max_overall_stall_duration: Some(Duration::from_secs(60)),
            live_gap_strategy: GapSkipStrategy::default(),
            vod_gap_strategy: GapSkipStrategy::WaitIndefinitely,
            vod_segment_timeout: None,
            buffer_limits: BufferLimits::default(),
            metrics_enabled: true,
        }
    }
}
