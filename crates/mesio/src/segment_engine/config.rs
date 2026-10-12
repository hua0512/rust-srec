//! Configuration consumed by the engine core (fetch and assembler). Protocol
//! engines build it from their own configuration once, at engine start.

use std::time::Duration;

#[derive(Debug, Clone)]
pub struct SegmentCoreConfig {
    pub fetch: FetchConfig,
    pub output: OutputConfig,
}

/// Per-segment download settings for the fetch-and-process future.
#[derive(Debug, Clone)]
pub struct FetchConfig {
    /// Query parameters added to every segment request.
    pub params: Vec<(String, String)>,
    pub segment_download_timeout: Duration,
    /// Lifecycle retry budget per segment; it also caps the attempt-level
    /// retries inside one fetch.
    pub max_segment_retries: u32,
    pub segment_retry_delay_base: Duration,
    /// Progress events are emitted once this many bytes or this much time
    /// accumulates (either 0 = every chunk).
    pub progress_emit_min_bytes: u64,
    pub progress_emit_min_interval: Duration,
    /// TTL of processed (decrypted) segments in the cache.
    pub processed_segment_ttl: Duration,
    /// Hard per-segment cap (0 = disabled); a larger body terminalizes as
    /// oversize.
    pub max_segment_size_bytes: u64,
}

/// Reorder, gap and stall policy for the sequence assembler.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Live: buffered duration kept below the emit cursor (zero = unlimited).
    pub live_reorder_buffer_duration: Duration,
    /// Live: buffered segments kept below the emit cursor (0 = unlimited).
    pub live_reorder_buffer_max_segments: usize,
    /// How often a pending gap is re-evaluated without new input.
    pub gap_evaluation_interval: Duration,
    /// Init payloads retained by key (0 = unlimited).
    pub max_pending_init_segments: usize,
    /// Live: terminate when no input arrives for this long.
    pub live_max_overall_stall_duration: Option<Duration>,
    pub live_gap_strategy: GapSkipStrategy,
    pub vod_gap_strategy: GapSkipStrategy,
    /// VOD: skip a missing segment after waiting this long.
    pub vod_segment_timeout: Option<Duration>,
    /// Reorder-buffer cap; reaching it forces gap skips.
    pub buffer_limits: BufferLimits,
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
