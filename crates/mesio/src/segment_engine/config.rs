//! Configuration consumed by the engine core (fetch and assembler). Protocol
//! engines build it from their own configuration once, at engine start.

use std::time::Duration;

use crate::hls::config::{BufferLimits, GapSkipStrategy};

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
