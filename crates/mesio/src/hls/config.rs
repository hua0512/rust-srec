use std::time::Duration;

use crate::DownloaderConfig;

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
}

// --- Playlist Configuration ---
#[derive(Debug, Clone)]
pub struct HlsPlaylistConfig {
    pub initial_playlist_fetch_timeout: Duration,
    pub live_refresh_interval: Duration, // Minimum interval for refreshing live playlists
    pub live_max_refresh_retries: u32,
    pub live_refresh_retry_delay: Duration,
    pub variant_selection_policy: HlsVariantSelectionPolicy,
}

impl Default for HlsPlaylistConfig {
    fn default() -> Self {
        Self {
            initial_playlist_fetch_timeout: Duration::from_secs(15),
            live_refresh_interval: Duration::from_secs(1),
            live_max_refresh_retries: 5,
            live_refresh_retry_delay: Duration::from_secs(1),
            variant_selection_policy: Default::default(),
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
    pub download_concurrency: usize, // Max concurrent segment downloads
}

impl Default for HlsSchedulerConfig {
    fn default() -> Self {
        Self {
            download_concurrency: 3,
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
    pub live_reorder_buffer_duration: Duration, // Max duration of segments to hold in reorder buffer
    pub live_reorder_buffer_max_segments: usize, // Max number of segments in reorder buffer
    /// Enables skipping of missing segments in a live stream after a threshold.
    pub live_gap_skip_enabled: bool,
    /// The number of newer media segments that must be received after a gap
    /// is detected before the OutputManager attempts to skip the missing segment(s).
    pub live_gap_skip_threshold_segments: u64,

    /// Duration to wait for a segment to be received before considering it stalled.
    /// If the overall stall duration exceeds this value, the downloader will throw an error.
    /// If None, this timeout is disabled.
    pub live_max_overall_stall_duration: Option<Duration>,
}

impl Default for HlsOutputConfig {
    fn default() -> Self {
        Self {
            live_reorder_buffer_duration: Duration::from_secs(30),
            live_reorder_buffer_max_segments: 10,
            live_gap_skip_enabled: true,
            live_gap_skip_threshold_segments: 3, // Default to 3 segments
            live_max_overall_stall_duration: Some(Duration::from_secs(60)), // Default to 60 seconds
        }
    }
}

// Implement the marker trait from the main crate
impl crate::media_protocol::ProtocolConfig for HlsConfig {}
