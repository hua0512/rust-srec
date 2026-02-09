// HLS Output Manager: Manages the final stream of HLSStreamEvents provided to the client.
// For live streams, it handles buffering and reordering of segments.

use crate::hls::config::HlsConfig;
use crate::hls::events::HlsStreamEvent;
use crate::hls::scheduler::ProcessedSegmentOutput;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

// --- Gap State Tracking ---

/// Tracks the state of a detected gap in the segment sequence.
/// Used for time-based gap detection and skip decisions.
#[derive(Debug)]
pub struct GapState {
    /// The sequence number we're waiting for (the missing segment)
    pub missing_sequence: u64,
    /// When the gap was first detected
    pub detected_at: Instant,
    /// Number of subsequent segments received since gap detection
    pub segments_since_gap: u64,
}

impl GapState {
    /// Creates a new GapState for a missing sequence number.
    pub fn new(missing_sequence: u64) -> Self {
        Self {
            missing_sequence,
            detected_at: Instant::now(),
            segments_since_gap: 0,
        }
    }

    /// Returns the duration since the gap was first detected.
    pub fn elapsed(&self) -> std::time::Duration {
        self.detected_at.elapsed()
    }

    /// Increments the count of segments received since gap detection.
    pub fn increment_segments_since_gap(&mut self) {
        self.segments_since_gap += 1;
    }
}

// --- Buffered Segment with Metadata ---

/// Enhanced segment entry with timing metadata for the reorder buffer.
/// Wraps a ProcessedSegmentOutput with additional tracking information.
#[derive(Debug)]
pub struct BufferedSegment {
    /// The processed segment output
    pub output: ProcessedSegmentOutput,
    /// When this segment was added to the buffer
    pub buffered_at: Instant,
    /// Size in bytes (for memory tracking)
    pub size_bytes: usize,
}

impl BufferedSegment {
    /// Creates a new BufferedSegment from a ProcessedSegmentOutput.
    /// Calculates the size in bytes from the segment data.
    pub fn new(output: ProcessedSegmentOutput) -> Self {
        // Calculate size from the segment data using HlsData::size()
        let size_bytes = output.data.size();

        Self {
            output,
            buffered_at: Instant::now(),
            size_bytes,
        }
    }

    /// Creates a BufferedSegment with a specific size (useful for testing).
    #[allow(dead_code)]
    pub fn with_size(output: ProcessedSegmentOutput, size_bytes: usize) -> Self {
        Self {
            output,
            buffered_at: Instant::now(),
            size_bytes,
        }
    }

    /// Returns the duration since this segment was buffered.
    pub fn time_in_buffer(&self) -> std::time::Duration {
        self.buffered_at.elapsed()
    }

    /// Returns the media sequence number of the wrapped segment.
    pub fn media_sequence_number(&self) -> u64 {
        self.output.media_sequence_number
    }
}

// --- Metrics for Reorder Buffer Observability ---

/// Metrics for observability of the reorder buffer.
/// All counters use atomic operations for thread-safe access.
#[derive(Debug)]
pub struct ReorderBufferMetrics {
    /// Total segments received
    pub segments_received: AtomicU64,
    /// Total segments emitted in order
    pub segments_emitted: AtomicU64,
    /// Total segments rejected as stale
    pub segments_rejected_stale: AtomicU64,
    /// Total gaps detected
    pub gaps_detected: AtomicU64,
    /// Total gap skips performed
    pub gap_skips: AtomicU64,
    /// Cumulative gap size (segments skipped)
    pub total_segments_skipped: AtomicU64,
    /// Current buffer depth (segments)
    pub current_buffer_depth: AtomicU64,
    /// Current buffer size (bytes)
    pub current_buffer_bytes: AtomicU64,
    /// Maximum buffer depth observed
    pub max_buffer_depth: AtomicU64,
    /// Total reorder delay (sum of wait times in milliseconds)
    pub total_reorder_delay_ms: AtomicU64,
}

impl Default for ReorderBufferMetrics {
    fn default() -> Self {
        Self {
            segments_received: AtomicU64::new(0),
            segments_emitted: AtomicU64::new(0),
            segments_rejected_stale: AtomicU64::new(0),
            gaps_detected: AtomicU64::new(0),
            gap_skips: AtomicU64::new(0),
            total_segments_skipped: AtomicU64::new(0),
            current_buffer_depth: AtomicU64::new(0),
            current_buffer_bytes: AtomicU64::new(0),
            max_buffer_depth: AtomicU64::new(0),
            total_reorder_delay_ms: AtomicU64::new(0),
        }
    }
}

impl ReorderBufferMetrics {
    /// Creates a new ReorderBufferMetrics instance with all counters initialized to zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Logs a summary of reordering statistics using tracing.
    pub fn log_summary(&self) {
        info!(
            segments_received = self.segments_received.load(Ordering::Relaxed),
            segments_emitted = self.segments_emitted.load(Ordering::Relaxed),
            segments_rejected = self.segments_rejected_stale.load(Ordering::Relaxed),
            gaps_detected = self.gaps_detected.load(Ordering::Relaxed),
            gap_skips = self.gap_skips.load(Ordering::Relaxed),
            segments_skipped = self.total_segments_skipped.load(Ordering::Relaxed),
            max_buffer_depth = self.max_buffer_depth.load(Ordering::Relaxed),
            "Reorder buffer statistics"
        );
    }

    /// Increments the segments_received counter by 1.
    pub fn record_segment_received(&self) {
        self.segments_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the segments_emitted counter by 1.
    pub fn record_segment_emitted(&self) {
        self.segments_emitted.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the segments_rejected_stale counter by 1.
    pub fn record_segment_rejected_stale(&self) {
        self.segments_rejected_stale.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the gaps_detected counter by 1.
    pub fn record_gap_detected(&self) {
        self.gaps_detected.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a gap skip event with the number of segments skipped.
    pub fn record_gap_skip(&self, segments_skipped: u64) {
        self.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.total_segments_skipped
            .fetch_add(segments_skipped, Ordering::Relaxed);
    }

    /// Updates the current buffer depth and tracks max depth.
    pub fn update_buffer_depth(&self, depth: u64) {
        self.current_buffer_depth.store(depth, Ordering::Relaxed);
        // Update max if current exceeds it
        let mut current_max = self.max_buffer_depth.load(Ordering::Relaxed);
        while depth > current_max {
            match self.max_buffer_depth.compare_exchange_weak(
                current_max,
                depth,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current_max = actual,
            }
        }
    }

    /// Updates the current buffer size in bytes.
    pub fn update_buffer_bytes(&self, bytes: u64) {
        self.current_buffer_bytes.store(bytes, Ordering::Relaxed);
    }

    /// Records reorder delay in milliseconds.
    pub fn record_reorder_delay(&self, delay_ms: u64) {
        self.total_reorder_delay_ms
            .fetch_add(delay_ms, Ordering::Relaxed);
    }

    // Note: we previously exposed test-only getters for internal counters.
    // Those were unused and caused `dead_code` failures under `clippy -D warnings`.
}

use super::HlsDownloaderError;
use crate::hls::config::GapSkipStrategy;
use crate::hls::events::GapSkipReason;
use crate::hls::metrics::PerformanceMetrics;
use hls::SegmentType;

/// Statistics about the current state of the reorder buffer.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BufferStats {
    /// Number of segments currently in the buffer
    pub segment_count: usize,
    /// Total size of buffered segments in bytes
    pub byte_size: usize,
    /// Oldest sequence number in the buffer (if any)
    pub oldest_sequence: Option<u64>,
    /// Newest sequence number in the buffer (if any)
    pub newest_sequence: Option<u64>,
}

pub struct OutputManager {
    config: Arc<HlsConfig>,
    input_rx: mpsc::Receiver<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
    event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
    /// Reorder buffer storing segments with metadata
    reorder_buffer: BTreeMap<u64, BufferedSegment>,
    /// Pending fMP4 init segments keyed by the MSN at which they become applicable.
    ///
    /// These are not inserted into `reorder_buffer` because the buffer is keyed by MSN and
    /// cannot hold both an init segment and a media segment for the same MSN.
    pending_init_segments: BTreeMap<u64, BufferedSegment>,
    /// Whether we've seen at least one init segment on this stream.
    ///
    /// For fMP4 streams, emitting media segments before any init segment results in downstream
    /// buffering/dropping. We therefore gate initial media emission until an init arrives.
    has_seen_init_segment: bool,
    /// Whether we have observed any fMP4 segments on this stream (init or media).
    /// Used to apply fMP4-specific startup heuristics without affecting TS streams.
    is_fmp4_stream: bool,
    /// Whether we've emitted at least one *media* segment (TS or fMP4 media).
    ///
    /// For live fMP4 streams, count-based gap skipping before the first media emission is prone
    /// to false positives due to out-of-order completion under download concurrency.
    has_emitted_media_segment: bool,
    is_live_stream: bool,
    expected_next_media_sequence: u64,
    #[allow(dead_code)]
    playlist_ended: bool,
    token: CancellationToken,

    /// Gap tracking state - replaces gap_detected_waiting_for_sequence and segments_received_since_gap_detected
    ///
    gap_state: Option<GapState>,

    last_input_received_time: Option<Instant>,

    /// Metrics for observability
    metrics: Arc<ReorderBufferMetrics>,

    /// Current total bytes in the reorder buffer
    current_buffer_bytes: usize,

    /// Runtime-configurable gap strategy for live streams
    /// This overrides config.output_config.live_gap_strategy when set
    live_gap_strategy_override: Option<GapSkipStrategy>,

    /// Runtime-configurable gap strategy for VOD streams
    /// This overrides config.output_config.vod_gap_strategy when set
    vod_gap_strategy_override: Option<GapSkipStrategy>,

    /// Performance metrics for the HLS pipeline
    /// Used to log performance summary on stream end
    performance_metrics: Option<Arc<PerformanceMetrics>>,
}

impl OutputManager {
    /// Create new OutputManager with improved configuration.
    ///
    pub fn new(
        config: Arc<HlsConfig>,
        input_rx: mpsc::Receiver<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
        is_live_stream: bool,
        initial_media_sequence: u64,
        token: CancellationToken,
    ) -> Self {
        Self {
            config,
            input_rx,
            event_tx,
            reorder_buffer: BTreeMap::new(),
            pending_init_segments: BTreeMap::new(),
            has_seen_init_segment: false,
            is_fmp4_stream: false,
            has_emitted_media_segment: false,
            is_live_stream,
            expected_next_media_sequence: initial_media_sequence,
            playlist_ended: false,
            token,
            gap_state: None,
            last_input_received_time: if is_live_stream {
                Some(Instant::now())
            } else {
                None
            },
            metrics: Arc::new(ReorderBufferMetrics::new()),
            current_buffer_bytes: 0,
            live_gap_strategy_override: None,
            vod_gap_strategy_override: None,
            performance_metrics: None,
        }
    }

    /// Create new OutputManager with performance metrics tracking.
    ///
    pub fn with_performance_metrics(
        config: Arc<HlsConfig>,
        input_rx: mpsc::Receiver<Result<ProcessedSegmentOutput, HlsDownloaderError>>,
        event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
        is_live_stream: bool,
        initial_media_sequence: u64,
        token: CancellationToken,
        performance_metrics: Arc<PerformanceMetrics>,
    ) -> Self {
        let mut manager = Self::new(
            config,
            input_rx,
            event_tx,
            is_live_stream,
            initial_media_sequence,
            token,
        );
        manager.performance_metrics = Some(performance_metrics);
        manager
    }

    /// Get current metrics snapshot.
    ///
    #[allow(dead_code)]
    pub fn metrics(&self) -> &ReorderBufferMetrics {
        // Note: We return a reference to the inner ReorderBufferMetrics
        // The Arc is used internally for potential future sharing
        &self.metrics
    }

    /// Get current buffer statistics.
    ///
    #[allow(dead_code)]
    pub fn buffer_stats(&self) -> BufferStats {
        BufferStats {
            segment_count: self.reorder_buffer.len(),
            byte_size: self.current_buffer_bytes,
            oldest_sequence: self.reorder_buffer.keys().next().copied(),
            newest_sequence: self.reorder_buffer.keys().next_back().copied(),
        }
    }

    /// Check if buffer is at capacity.
    /// Returns true if either segment count or byte size limit is reached.
    ///
    pub fn is_buffer_full(&self) -> bool {
        let limits = &self.config.output_config.buffer_limits;

        // Check segment count limit (0 = unlimited)
        let segment_limit_reached =
            limits.max_segments > 0 && self.reorder_buffer.len() >= limits.max_segments;

        // Check byte size limit (0 = unlimited)
        let byte_limit_reached =
            limits.max_bytes > 0 && self.current_buffer_bytes >= limits.max_bytes;

        segment_limit_reached || byte_limit_reached
    }

    /// Update gap strategy at runtime.
    /// The new strategy applies to subsequent gaps only (doesn't affect current gap state).
    ///
    ///
    /// This method stores the strategy in a local override field, which takes precedence
    /// over the config's gap strategy. The new strategy will be used for evaluating
    /// subsequent gaps - any gap currently being tracked will continue to be evaluated
    /// with the new strategy on the next check.
    #[allow(dead_code)]
    pub fn set_gap_strategy(&mut self, strategy: GapSkipStrategy) {
        if self.is_live_stream {
            debug!("Updating live gap strategy at runtime: {:?}", strategy);
            self.live_gap_strategy_override = Some(strategy);
        } else {
            debug!("Updating VOD gap strategy at runtime: {:?}", strategy);
            self.vod_gap_strategy_override = Some(strategy);
        }
    }

    /// Get the current effective gap strategy for this stream.
    /// Returns the runtime override if set, otherwise falls back to config.
    ///
    pub fn get_gap_strategy(&self) -> &GapSkipStrategy {
        if self.is_live_stream {
            self.live_gap_strategy_override
                .as_ref()
                .unwrap_or(&self.config.output_config.live_gap_strategy)
        } else {
            self.vod_gap_strategy_override
                .as_ref()
                .unwrap_or(&self.config.output_config.vod_gap_strategy)
        }
    }

    /// Evaluates whether the current gap should be skipped based on the configured strategy.
    /// Returns Some(GapSkipReason) if the gap should be skipped, None otherwise.
    ///
    ///
    /// Note: This method only evaluates gap-specific skip conditions. The overall stall timeout
    /// (live_max_overall_stall_duration) is handled separately in the run() loop and will still
    /// trigger even when this returns None (e.g., for WaitIndefinitely strategy).
    ///
    ///
    /// The strategy used is determined by get_gap_strategy(), which returns the runtime
    /// override if set, otherwise falls back to the config value.
    fn should_skip_gap(&self) -> Option<GapSkipReason> {
        let gap_state = self.gap_state.as_ref()?;

        // Get the appropriate strategy based on stream type, using runtime override if set
        //
        let strategy = self.get_gap_strategy();

        let in_startup =
            self.is_live_stream && self.is_fmp4_stream && !self.has_emitted_media_segment;
        let elapsed = gap_state.elapsed();

        match strategy {
            GapSkipStrategy::WaitIndefinitely => {
                // Never skip based on gap strategy alone.
                // However, the overall stall timeout (live_max_overall_stall_duration) in the
                // run() loop will still trigger if no input is received for the configured
                // duration, regardless of this strategy.
                None
            }
            GapSkipStrategy::SkipAfterCount(threshold) => {
                // More robust startup behavior: count-based skipping is prone to false positives
                // when downloads complete out of order (common with concurrency). Until we have
                // emitted at least one media segment on fMP4 streams, only duration-based skipping is allowed.
                if !in_startup && gap_state.segments_since_gap >= *threshold {
                    Some(GapSkipReason::CountThreshold(gap_state.segments_since_gap))
                } else {
                    None
                }
            }
            GapSkipStrategy::SkipAfterDuration(threshold) => {
                if elapsed >= *threshold {
                    Some(GapSkipReason::DurationThreshold(elapsed))
                } else {
                    None
                }
            }
            GapSkipStrategy::SkipAfterBoth { count, duration } => {
                // Skip when EITHER threshold is exceeded (OR semantics)
                let duration_exceeded = elapsed >= *duration;

                // More robust startup behavior: disable count-based skipping until at least one
                // media segment has been emitted on fMP4 streams.
                let count_exceeded = !in_startup && gap_state.segments_since_gap >= *count;

                if count_exceeded && duration_exceeded {
                    Some(GapSkipReason::BothThresholds {
                        count: gap_state.segments_since_gap,
                        duration: elapsed,
                    })
                } else if count_exceeded {
                    Some(GapSkipReason::CountThreshold(gap_state.segments_since_gap))
                } else if duration_exceeded {
                    Some(GapSkipReason::DurationThreshold(elapsed))
                } else {
                    None
                }
            }
        }
    }

    /// Main loop for the OutputManager.
    pub async fn run(&mut self) {
        debug!("is_live_stream: {}", self.is_live_stream);

        // When a gap is detected, we need a periodic wake-up to re-evaluate gap policies
        // (duration-based skipping / VOD timeouts). Otherwise, if no new segments arrive
        // (or backpressure pauses input), we can stall indefinitely without advancing.
        let gap_evaluation_interval = {
            let interval = self.config.output_config.gap_evaluation_interval;
            if interval == std::time::Duration::ZERO {
                std::time::Duration::from_millis(1)
            } else {
                interval
            }
        };

        loop {
            // Determine timeout for select! based on *remaining* live stall time.
            // This must be derived from `last_input_received_time`, otherwise other periodic
            // wake-ups (e.g. gap evaluation) would continuously reset a fixed-duration timer.
            let overall_stall_timeout = if self.is_live_stream
                && let Some(max_stall_duration) =
                    self.config.output_config.live_max_overall_stall_duration
                && let Some(last_input_time) = self.last_input_received_time
            {
                max_stall_duration.saturating_sub(last_input_time.elapsed())
            } else {
                // Effectively infinite timeout if not live or not configured.
                std::time::Duration::from_secs(u64::MAX / 2)
            };

            tokio::select! {
                biased;

                // Branch 1: Cancellation Token
                _ = self.token.cancelled() => {
                    debug!("Cancellation token received. Preparing to exit.");
                    break;
                }

                // Branch 2: Max Overall Stall Timeout (Live streams only)
                // This timeout is independent of the gap skip strategy.
                // Even when live_gap_strategy is WaitIndefinitely, this overall stall timeout
                // will still trigger if no input is received for the configured duration.
                // This ensures the stream doesn't hang indefinitely waiting for segments.
                _ = sleep(overall_stall_timeout), if self.is_live_stream && self.config.output_config.live_max_overall_stall_duration.is_some() => {
                    if let Some(last_input_time) = self.last_input_received_time
                        && let Some(max_stall_duration) = self.config.output_config.live_max_overall_stall_duration
                        && last_input_time.elapsed() >= max_stall_duration
                    {
                                error!(
                                    "Live stream stalled for more than configured max duration ({:?}). No new segments or events received.",
                                    max_stall_duration
                                );
                                let _ = self.event_tx.send(Err(HlsDownloaderError::TimeoutError(
                                    "Stalled: No input received for max duration.".to_string()
                                ))).await;
                                break; // Exit loop for live stream stall
                    }

                }

                // Branch 3: Input from SegmentScheduler
                // --- Buffer Capacity Check ---
                // Apply backpressure by not receiving from input channel when buffer is full
                processed_result = self.input_rx.recv(), if !self.is_buffer_full() => {

                    // Update last_input_received_time for live streams
                    if self.is_live_stream {
                        self.last_input_received_time = Some(Instant::now());
                    }

                    match processed_result {
                        Some(Ok(processed_output)) => {
                            let current_segment_sequence = processed_output.media_sequence_number;

                            match processed_output.data.segment_type() {
                                SegmentType::M4sInit | SegmentType::M4sMedia => {
                                    self.is_fmp4_stream = true;
                                }
                                SegmentType::Ts | SegmentType::EndMarker => {}
                            }

                            // fMP4 init segments are not part of the media sequence progression
                            // and cannot be stored in the reorder buffer (keyed by MSN). Track them
                            // separately and emit them when we later emit a media segment whose MSN
                            // is >= the init segment's MSN.
                            if processed_output.data.is_init_segment() {
                                self.has_seen_init_segment = true;
                                let buffered_init = BufferedSegment::new(processed_output);
                                self.pending_init_segments
                                    .insert(current_segment_sequence, buffered_init);

                                // Prevent unbounded growth if a playlist produces many init segments.
                                let max_pending_init_segments =
                                    self.config.output_config.max_pending_init_segments;
                                if max_pending_init_segments > 0 {
                                    while self.pending_init_segments.len() > max_pending_init_segments {
                                        self.pending_init_segments.pop_first();
                                    }
                                }

                                // An init segment can unblock emission for already-buffered media.
                                if self.try_emit_segments().await.is_err() {
                                    error!("Error emitting segments from buffer after init segment. Exiting.");
                                    break;
                                }

                                continue;
                            }

                            // Record segment received in metrics
                            if self.config.output_config.metrics_enabled {
                                self.metrics.record_segment_received();
                            }

                            // --- Stale Segment Rejection ---
                            // Check if segment is stale (MSN < expected_next_media_sequence)
                            // Reject immediately without buffering
                            if current_segment_sequence < self.expected_next_media_sequence {
                                debug!(
                                    "Rejecting stale segment {} (expected >= {}). Segment already emitted or skipped.",
                                    current_segment_sequence, self.expected_next_media_sequence
                                );
                                if self.config.output_config.metrics_enabled {
                                    self.metrics.record_segment_rejected_stale();
                                }
                                continue; // Skip buffering this segment
                            }

                            trace!(
                                "Adding segment {} (live: {}) to reorder buffer.",
                                current_segment_sequence, self.is_live_stream
                            );

                            // For both live and VOD, add to reorder buffer.
                            // If it's a live stream and we are waiting for a gap, update counter.
                            if self.is_live_stream
                                && let Some(ref mut gap_state) = self.gap_state
                                && current_segment_sequence > gap_state.missing_sequence
                            {
                                gap_state.increment_segments_since_gap();
                                trace!(
                                    "Live stream: Received segment {} while waiting for {}. Segments since gap: {}.",
                                    current_segment_sequence, gap_state.missing_sequence, gap_state.segments_since_gap
                                );
                            }

                            // Create BufferedSegment with metadata
                            let buffered_segment = BufferedSegment::new(processed_output);
                            let segment_size = buffered_segment.size_bytes;

                            // Update buffer byte tracking
                            self.current_buffer_bytes += segment_size;

                            self.reorder_buffer.insert(current_segment_sequence, buffered_segment);

                            // Update metrics for buffer depth
                            if self.config.output_config.metrics_enabled {
                                self.metrics
                                    .update_buffer_depth(self.reorder_buffer.len() as u64);
                                self.metrics
                                    .update_buffer_bytes(self.current_buffer_bytes as u64);
                            }

                            // Log warning if buffer is now at capacity
                            if self.is_buffer_full() {
                                let limits = &self.config.output_config.buffer_limits;
                                warn!(
                                    "Reorder buffer at capacity. Segments: {}/{}, Bytes: {}/{}. Applying backpressure.",
                                    self.reorder_buffer.len(),
                                    limits.max_segments,
                                    self.current_buffer_bytes,
                                    limits.max_bytes
                                );
                            }

                            // Attempt to emit segments from the buffer.
                            if self.try_emit_segments().await.is_err() {
                                error!("Error emitting segments from buffer. Exiting.");
                                break;
                            }
                        }
                        Some(Err(e)) => {
                            error!("Error received from input channel: {:?}. Forwarding and exiting.", e);
                            if self.event_tx.send(Err(e)).await.is_err() {
                                error!("Failed to send error event after receiving input error. Exiting.");
                            }
                            break; // Critical error, always break
                        }
                        None => { // input_rx channel closed by SegmentScheduler
                            debug!("input_rx channel closed. Natural end of stream or scheduler termination.");
                            // This is the primary condition for VOD to exit the loop gracefully after processing all segments.
                            // For live streams, this also indicates the end of input.
                            break;
                        }
                    }
                }

                // Branch 4: Periodic gap evaluation
                // This ensures duration-based gap skipping and VOD timeouts can trigger even if no
                // further segments arrive (or input is paused by backpressure).
                _ = sleep(gap_evaluation_interval), if self.gap_state.is_some() => {
                    if self.try_emit_segments().await.is_err() {
                        error!("Error emitting segments from buffer during gap evaluation tick. Exiting.");
                        break;
                    }
                }
            }
        }

        // Post-loop operations: Flush any remaining segments for both live and VOD.
        // For VOD, this ensures all segments are emitted if the input channel closed.
        // For Live, this handles segments remaining after a shutdown signal.
        debug!(
            "Flushing reorder buffer (if any segments remain) for stream (live: {}).",
            self.is_live_stream
        );
        if !self.reorder_buffer.is_empty() {
            if self.flush_reorder_buffer().await.is_err() {
                error!(
                    "Failed to flush reorder buffer post-loop (live: {}). Event sender likely closed.",
                    self.is_live_stream
                );
            }
        } else {
            debug!("Reorder buffer already empty post-loop.");
        }

        // --- Stream End Cleanup ---

        // Reset gap state on stream end
        if self.gap_state.is_some() {
            debug!("Resetting gap state on stream end.");
            self.gap_state = None;
        }

        // Log metrics summary before sending StreamEnded event
        if self.config.output_config.metrics_enabled {
            debug!("Logging reorder buffer metrics summary on stream end.");
            self.metrics.log_summary();
        }

        // Log performance metrics summary on stream end
        if let Some(ref performance_metrics) = self.performance_metrics {
            debug!("Logging HLS performance metrics summary on stream end.");
            performance_metrics.log_summary();
        }

        debug!("Sending StreamEnded event.");
        if self
            .event_tx
            .send(Ok(HlsStreamEvent::StreamEnded))
            .await
            .is_err()
        {
            error!("Failed to send StreamEnded event after loop completion.");
        }
        debug!("Finished.");
    }

    /// Attempts to emit segments from the reorder buffer.
    /// Handles ordering, discontinuities, and gap skipping (for live streams).
    /// Returns Ok(()) if successful, Err(()) if event_tx is closed.
    async fn try_emit_segments(&mut self) -> Result<(), ()> {
        while let Some(entry) = self.reorder_buffer.first_entry() {
            let segment_sequence = *entry.key();

            if segment_sequence == self.expected_next_media_sequence {
                // Expected segment found
                if let Some(buffered_segment) = self.reorder_buffer.get(&segment_sequence) {
                    let is_fmp4_media =
                        buffered_segment.output.data.segment_type() == SegmentType::M4sMedia;

                    // For fMP4, do not emit any media segments until we've seen an init segment.
                    // This prevents downstream components from buffering/dropping early segments.
                    if is_fmp4_media && !self.has_seen_init_segment {
                        break;
                    }
                }

                if let Some((_seq, buffered_segment)) =
                    self.reorder_buffer.remove_entry(&segment_sequence)
                {
                    // Update buffer byte tracking
                    self.current_buffer_bytes = self
                        .current_buffer_bytes
                        .saturating_sub(buffered_segment.size_bytes);

                    // Record reorder delay (time spent in buffer)
                    let delay_ms = buffered_segment.time_in_buffer().as_millis() as u64;
                    if self.config.output_config.metrics_enabled {
                        self.metrics.record_reorder_delay(delay_ms);
                    }

                    // Extract the ProcessedSegmentOutput from BufferedSegment
                    let segment_output = buffered_segment.output;

                    if segment_output.discontinuity {
                        debug!("sending discontinuity tag encountered");

                        // Pre-discontinuity flush
                        // Ensure all buffered segments with lower MSN are emitted before the discontinuity event
                        // This handles edge cases where gap skipping might have left segments in the buffer
                        let segments_to_flush: Vec<u64> = self
                            .reorder_buffer
                            .keys()
                            .filter(|&&msn| msn < segment_sequence)
                            .cloned()
                            .collect();

                        if !segments_to_flush.is_empty() {
                            debug!(
                                "Pre-discontinuity flush: emitting {} segments with MSN < {} before discontinuity event",
                                segments_to_flush.len(),
                                segment_sequence
                            );
                            for msn in segments_to_flush {
                                if let Some(buffered_seg) = self.reorder_buffer.remove(&msn) {
                                    // Update buffer byte tracking
                                    self.current_buffer_bytes = self
                                        .current_buffer_bytes
                                        .saturating_sub(buffered_seg.size_bytes);

                                    // Record reorder delay
                                    let delay = buffered_seg.time_in_buffer().as_millis() as u64;
                                    if self.config.output_config.metrics_enabled {
                                        self.metrics.record_reorder_delay(delay);
                                    }

                                    // Emit the segment data
                                    let event =
                                        HlsStreamEvent::Data(Box::new(buffered_seg.output.data));
                                    if self.event_tx.send(Ok(event)).await.is_err() {
                                        return Err(());
                                    }
                                    if self.config.output_config.metrics_enabled {
                                        self.metrics.record_segment_emitted();
                                    }
                                }
                            }
                            // Update metrics for buffer depth after flush
                            if self.config.output_config.metrics_enabled {
                                self.metrics
                                    .update_buffer_depth(self.reorder_buffer.len() as u64);
                                self.metrics
                                    .update_buffer_bytes(self.current_buffer_bytes as u64);
                            }
                        }

                        // Reset gap state on discontinuity
                        // This ensures gap tracking starts fresh for the new sequence
                        if self.gap_state.is_some() {
                            debug!(
                                "Resetting gap state due to discontinuity. Previous gap was waiting for sequence {}",
                                self.gap_state
                                    .as_ref()
                                    .map(|g| g.missing_sequence)
                                    .unwrap_or(0)
                            );
                            self.gap_state = None;
                        }
                        if self
                            .event_tx
                            .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                            .await
                            .is_err()
                        {
                            return Err(());
                        }
                    }

                    // If we sent the discontinuity event for this MSN, avoid duplicating it when
                    // emitting a pending init segment for the same boundary.
                    self.emit_applicable_init_segment(
                        segment_sequence,
                        segment_output.discontinuity,
                    )
                    .await?;

                    let is_media = !segment_output.data.is_init_segment();
                    let event = HlsStreamEvent::Data(Box::new(segment_output.data));
                    if self.event_tx.send(Ok(event)).await.is_err() {
                        return Err(());
                    }

                    // Record segment emitted in metrics
                    if self.config.output_config.metrics_enabled {
                        self.metrics.record_segment_emitted();
                    }
                    if is_media {
                        self.has_emitted_media_segment = true;
                    }

                    self.expected_next_media_sequence += 1;

                    // Reset gap state as we've successfully emitted the expected segment
                    self.gap_state = None;

                    // Update metrics for buffer depth
                    if self.config.output_config.metrics_enabled {
                        self.metrics
                            .update_buffer_depth(self.reorder_buffer.len() as u64);
                        self.metrics
                            .update_buffer_bytes(self.current_buffer_bytes as u64);
                    }
                } else {
                    // Should not happen if first_entry returned Some
                    break;
                }
            } else if segment_sequence < self.expected_next_media_sequence {
                // Stale segment
                debug!(
                    "Discarding stale segment from reorder buffer: sequence {}",
                    segment_sequence
                );
                if let Some(buffered_segment) = self.reorder_buffer.remove(&segment_sequence) {
                    // Update buffer byte tracking
                    self.current_buffer_bytes = self
                        .current_buffer_bytes
                        .saturating_sub(buffered_segment.size_bytes);
                    // Record stale rejection in metrics
                    if self.config.output_config.metrics_enabled {
                        self.metrics.record_segment_rejected_stale();
                    }
                    // Update metrics for buffer depth
                    if self.config.output_config.metrics_enabled {
                        self.metrics
                            .update_buffer_depth(self.reorder_buffer.len() as u64);
                        self.metrics
                            .update_buffer_bytes(self.current_buffer_bytes as u64);
                    }
                }
            } else {
                // Gap detected (segment_sequence > self.expected_next_media_sequence)
                // If a new gap is identified or the gap we are waiting for has changed:
                let is_new_gap = match &self.gap_state {
                    None => true,
                    Some(gap_state) => {
                        gap_state.missing_sequence != self.expected_next_media_sequence
                    }
                };

                if is_new_gap {
                    trace!(
                        "New gap detected. Expected: {}, Found: {}. Creating new gap state.",
                        self.expected_next_media_sequence, segment_sequence
                    );
                    // Record gap detected in metrics
                    if self.config.output_config.metrics_enabled {
                        self.metrics.record_gap_detected();
                    }

                    // Create new gap state with timestamp
                    let mut new_gap_state = GapState::new(self.expected_next_media_sequence);

                    // Count already buffered segments that are newer than the expected (missing) sequence
                    let range = (self.expected_next_media_sequence + 1)..;
                    let buffered_count = self.reorder_buffer.range(range).count() as u64;
                    for _ in 0..buffered_count {
                        new_gap_state.increment_segments_since_gap();
                    }

                    self.gap_state = Some(new_gap_state);

                    trace!(
                        "After re-counting buffered segments, segments_since_gap for expected {}: {}.",
                        self.expected_next_media_sequence,
                        self.gap_state
                            .as_ref()
                            .map(|g| g.segments_since_gap)
                            .unwrap_or(0)
                    );
                }

                // --- VOD Segment Timeout Check ---
                // For VOD streams with vod_segment_timeout configured, check if the gap has exceeded the timeout
                if !self.is_live_stream
                    && let Some(vod_timeout) = self.config.output_config.vod_segment_timeout
                    && let Some(ref gap_state) = self.gap_state
                {
                    let elapsed = gap_state.elapsed();
                    if elapsed >= vod_timeout {
                        // VOD segment timeout exceeded - emit SegmentTimeout event
                        warn!(
                            "VOD segment {} timed out after {:?} (timeout: {:?}). Skipping to next available segment {}.",
                            gap_state.missing_sequence, elapsed, vod_timeout, segment_sequence
                        );

                        // Emit SegmentTimeout event
                        let timeout_event = HlsStreamEvent::SegmentTimeout {
                            sequence_number: gap_state.missing_sequence,
                            waited_duration: elapsed,
                        };
                        if self.event_tx.send(Ok(timeout_event)).await.is_err() {
                            return Err(());
                        }

                        // Record gap skip in metrics
                        let skipped_count = segment_sequence - self.expected_next_media_sequence;
                        if self.config.output_config.metrics_enabled {
                            self.metrics.record_gap_skip(skipped_count);
                        }

                        // Advance to next available segment
                        self.expected_next_media_sequence = segment_sequence;

                        // Reset gap state
                        self.gap_state = None;
                        continue; // Attempt to emit the new expected_next_media_sequence
                    }
                }

                // --- Gap Skipping Logic using GapSkipStrategy ---
                // Evaluate whether to skip based on the configured strategy
                if let Some(skip_reason) = self.should_skip_gap() {
                    let gap_state = self.gap_state.as_ref().unwrap(); // Safe: should_skip_gap returns Some only if gap_state exists
                    let elapsed = gap_state.elapsed();
                    let segments_since_gap = gap_state.segments_since_gap;

                    warn!(
                        "GAP CONFIRMED: Missing segment(s) starting at {}. Reason: {:?}. Elapsed: {:?}, Segments since gap: {}. Skipping to segment {}.",
                        self.expected_next_media_sequence,
                        skip_reason,
                        elapsed,
                        segments_since_gap,
                        segment_sequence
                    );

                    // Record gap skip in metrics
                    let skipped_count = segment_sequence - self.expected_next_media_sequence;
                    if self.config.output_config.metrics_enabled {
                        self.metrics.record_gap_skip(skipped_count);
                    }

                    // Emit GapSkipped event
                    let gap_skipped_event = HlsStreamEvent::GapSkipped {
                        from_sequence: self.expected_next_media_sequence,
                        to_sequence: segment_sequence,
                        reason: skip_reason,
                    };
                    if self.event_tx.send(Ok(gap_skipped_event)).await.is_err() {
                        return Err(());
                    }

                    self.expected_next_media_sequence = segment_sequence; // Jump to the available segment

                    // Reset gap state as we are skipping
                    self.gap_state = None;
                    continue; // Attempt to emit the new expected_next_media_sequence
                } else {
                    // Skip condition not met, log and wait
                    if let Some(ref gap_state) = self.gap_state {
                        let elapsed = gap_state.elapsed();
                        trace!(
                            "Gap detected. Expected: {}, Found: {}. Waiting. Elapsed: {:?}, Segments since gap: {}. Strategy: {:?}",
                            self.expected_next_media_sequence,
                            segment_sequence,
                            elapsed,
                            gap_state.segments_since_gap,
                            if self.is_live_stream {
                                &self.config.output_config.live_gap_strategy
                            } else {
                                &self.config.output_config.vod_gap_strategy
                            }
                        );
                    }
                    break; // Stall and wait for the missing segment or more segments to arrive
                }
            }
        }
        // Pruning is primarily for live streams to manage buffer size.
        if self.is_live_stream {
            self.prune_reorder_buffer();
        }
        Ok(())
    }

    /// Prunes the reorder buffer based on configuration (duration/max_segments).
    /// Uses `BTreeMap::split_off` for O(log n) bulk removal of stale segments.
    ///
    fn prune_reorder_buffer(&mut self) {
        if !self.is_live_stream {
            return;
        }

        // --- Pruning by segment count using split_off ---
        let max_segments = self.config.output_config.live_reorder_buffer_max_segments;
        if self.reorder_buffer.len() > max_segments {
            // Calculate how many segments to remove
            let num_to_remove = self.reorder_buffer.len() - max_segments;

            // Find the threshold sequence number for bulk removal
            // Only prune segments older than expected_next_media_sequence
            let stale_segments: Vec<u64> = self
                .reorder_buffer
                .keys()
                .filter(|&&key| key < self.expected_next_media_sequence)
                .take(num_to_remove)
                .cloned()
                .collect();

            if !stale_segments.is_empty() {
                // Get the threshold (one past the last segment to remove)
                // split_off returns elements >= threshold, so we keep those
                if let Some(&last_to_remove) = stale_segments.last() {
                    let threshold = last_to_remove + 1;

                    // Use split_off for O(log n) bulk removal
                    // split_off(threshold) returns a new map with all entries >= threshold
                    // The original map retains entries < threshold (which we want to remove)
                    let kept = self.reorder_buffer.split_off(&threshold);

                    // Calculate bytes removed from the segments being pruned
                    let bytes_removed: usize =
                        self.reorder_buffer.values().map(|seg| seg.size_bytes).sum();

                    debug!(
                        "Bulk pruning {} segments by count (max_segments: {}), removed {} bytes",
                        self.reorder_buffer.len(),
                        max_segments,
                        bytes_removed
                    );

                    // Update buffer byte tracking
                    self.current_buffer_bytes =
                        self.current_buffer_bytes.saturating_sub(bytes_removed);

                    // Replace the buffer with the kept segments
                    self.reorder_buffer = kept;
                }
            }
        }

        // --- Pruning by duration using split_off ---
        let max_buffer_duration_secs = self
            .config
            .output_config
            .live_reorder_buffer_duration
            .as_secs_f32();

        // Only proceed with duration pruning if a positive max duration is set.
        if max_buffer_duration_secs > 0.0_f32 {
            let mut total_duration = 0.0;
            let mut prune_threshold: Option<u64> = None;

            // Iterate backwards over segments older than the expected one to find the cutoff point.
            for (&sequence_number, buffered_segment) in self
                .reorder_buffer
                .range(..self.expected_next_media_sequence)
                .rev()
            {
                let duration = buffered_segment
                    .output
                    .data
                    .media_segment()
                    .map_or(0.0, |ms| ms.duration);
                total_duration += duration;

                if total_duration > max_buffer_duration_secs {
                    // We want to remove this segment and all older ones
                    // So the threshold for split_off is sequence_number (keep >= sequence_number)
                    // But we want to remove sequence_number too, so threshold = sequence_number + 1
                    prune_threshold = Some(sequence_number);
                    break;
                }
            }

            // If a cutoff point was found, use split_off for bulk removal
            if let Some(threshold_seq) = prune_threshold {
                // split_off(threshold_seq) returns entries >= threshold_seq
                // We want to remove entries < threshold_seq
                let kept = self.reorder_buffer.split_off(&threshold_seq);

                // Calculate bytes removed
                let bytes_removed: usize =
                    self.reorder_buffer.values().map(|seg| seg.size_bytes).sum();

                debug!(
                    "Bulk pruning {} segments by duration (max buffer duration: {:.2}s), removed {} bytes",
                    self.reorder_buffer.len(),
                    max_buffer_duration_secs,
                    bytes_removed
                );

                // Update buffer byte tracking
                self.current_buffer_bytes = self.current_buffer_bytes.saturating_sub(bytes_removed);

                // Replace the buffer with the kept segments
                self.reorder_buffer = kept;
            }
        }

        // --- Update metrics after pruning ---
        let current_depth = self.reorder_buffer.len() as u64;
        if self.config.output_config.metrics_enabled {
            self.metrics.update_buffer_depth(current_depth);
            self.metrics
                .update_buffer_bytes(self.current_buffer_bytes as u64);
        }
    }

    /// Flushes remaining segments from the reorder buffer.
    /// Returns Ok(()) if successful, Err(()) if event_tx is closed.
    async fn flush_reorder_buffer(&mut self) -> Result<(), ()> {
        let mut warned_missing_init = false;
        // Removes and returns the first element (smallest key),
        while let Some((_key, buffered_segment)) = self.reorder_buffer.pop_first() {
            // Update buffer byte tracking
            self.current_buffer_bytes = self
                .current_buffer_bytes
                .saturating_sub(buffered_segment.size_bytes);

            // Record reorder delay (time spent in buffer)
            let delay_ms = buffered_segment.time_in_buffer().as_millis() as u64;
            if self.config.output_config.metrics_enabled {
                self.metrics.record_reorder_delay(delay_ms);
            }

            // Extract the ProcessedSegmentOutput from BufferedSegment
            let segment_output = buffered_segment.output;

            let is_fmp4_media = segment_output.data.segment_type() == SegmentType::M4sMedia;
            if is_fmp4_media && !self.has_seen_init_segment {
                if !warned_missing_init {
                    warn!(
                        "Dropping fMP4 media segments during flush because no init segment was ever received."
                    );
                    warned_missing_init = true;
                }
                continue;
            }

            if segment_output.discontinuity {
                debug!("sending discontinuity tag encountered in flush_reorder_buffer");
                // Reset gap state on discontinuity
                if self.gap_state.is_some() {
                    debug!(
                        "Resetting gap state due to discontinuity in flush. Previous gap was waiting for sequence {}",
                        self.gap_state
                            .as_ref()
                            .map(|g| g.missing_sequence)
                            .unwrap_or(0)
                    );
                    self.gap_state = None;
                }
                if self
                    .event_tx
                    .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                    .await
                    .is_err()
                {
                    // If sending fails, return the error.
                    // The event_tx channel is likely closed.
                    return Err(());
                }
            }

            // Emit the most recent init segment applicable to this MSN (if any).
            self.emit_applicable_init_segment(
                segment_output.media_sequence_number,
                segment_output.discontinuity,
            )
            .await?;

            let is_media = !segment_output.data.is_init_segment();
            let event = HlsStreamEvent::Data(Box::new(segment_output.data));
            if self.event_tx.send(Ok(event)).await.is_err() {
                // If sending fails, return the error.
                return Err(());
            }

            // Record segment emitted in metrics
            if self.config.output_config.metrics_enabled {
                self.metrics.record_segment_emitted();
            }
            if is_media {
                self.has_emitted_media_segment = true;
            }
        }

        // Update metrics for buffer depth (should be 0 after flush)
        if self.config.output_config.metrics_enabled {
            self.metrics.update_buffer_depth(0);
            self.metrics.update_buffer_bytes(0);
        }
        self.current_buffer_bytes = 0;

        Ok(())
    }

    /// Called when the playlist is known to have ended (e.g., ENDLIST tag or VOD completion).
    /// This is also called by the run loop's shutdown path.
    #[allow(dead_code)]
    pub async fn signal_stream_end_and_flush(&mut self) {
        debug!(
            "start to signal end, is_live_stream: {}",
            self.is_live_stream
        );
        if self.is_live_stream || !self.reorder_buffer.is_empty() {
            debug!("Flushing reorder buffer.");
            // Also flush for VOD if somehow buffered
            if self.flush_reorder_buffer().await.is_err() {
                error!("Failed to flush reorder buffer, event_tx likely closed.");
                // event_tx closed, can't send StreamEnded either
                return;
            }
            debug!("Reorder buffer flushed.");
        } else {
            debug!("No flush needed (not live or buffer empty).");
        }
        self.playlist_ended = true; // Mark as ended
        // The main run loop will send StreamEnded upon exiting.
        debug!("Stream end signaled, buffer flushed (if applicable).");
    }

    // Method to update live status and expected sequence, perhaps called by coordinator during init
    #[allow(dead_code)]
    pub fn update_stream_state(&mut self, is_live: bool, initial_sequence: u64) {
        self.is_live_stream = is_live;
        self.expected_next_media_sequence = initial_sequence;
        self.playlist_ended = false; // Reset if re-initializing
        self.reorder_buffer.clear();
        self.pending_init_segments.clear();
        self.has_seen_init_segment = false;
        self.is_fmp4_stream = false;
        self.has_emitted_media_segment = false;
        // Reset gap state
        self.gap_state = None;
        // Reset buffer byte tracking
        self.current_buffer_bytes = 0;
        self.last_input_received_time = if is_live { Some(Instant::now()) } else { None };
        // Reset gap strategy overrides
        self.live_gap_strategy_override = None;
        self.vod_gap_strategy_override = None;

        // Update metrics for buffer depth (should be 0 after clear)
        if self.config.output_config.metrics_enabled {
            self.metrics.update_buffer_depth(0);
            self.metrics.update_buffer_bytes(0);
        }
    }

    async fn emit_applicable_init_segment(
        &mut self,
        msn: u64,
        discontinuity_already_emitted: bool,
    ) -> Result<(), ()> {
        let Some((&last_key, _)) = self.pending_init_segments.range(..=msn).next_back() else {
            return Ok(());
        };

        // Remove all init segments that are now behind or equal to the current MSN.
        // Only emit the most recent one, since it represents the active init state.
        let keys: Vec<u64> = self
            .pending_init_segments
            .range(..=msn)
            .map(|(&k, _)| k)
            .collect();

        let mut last: Option<BufferedSegment> = None;
        for k in keys {
            last = self.pending_init_segments.remove(&k);
        }

        let Some(buffered_init) = last else {
            return Ok(());
        };

        // Sanity: make sure we're emitting the expected most-recent init segment.
        debug_assert_eq!(buffered_init.media_sequence_number(), last_key);

        if buffered_init.output.discontinuity
            && !discontinuity_already_emitted
            && self
                .event_tx
                .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                .await
                .is_err()
        {
            return Err(());
        }

        let event = HlsStreamEvent::Data(Box::new(buffered_init.output.data));
        if self.event_tx.send(Ok(event)).await.is_err() {
            return Err(());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use hls::HlsData;
    use std::time::Duration;

    fn test_init_segment(data: &[u8]) -> HlsData {
        HlsData::mp4_init(
            m3u8_rs::MediaSegment {
                uri: "init.mp4".to_string(),
                duration: 0.0,
                title: None,
                byte_range: None,
                discontinuity: false,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data.to_vec()),
        )
    }

    fn test_media_segment(data: &[u8]) -> HlsData {
        HlsData::mp4_segment(
            m3u8_rs::MediaSegment {
                uri: "segment.m4s".to_string(),
                duration: 1.0,
                title: None,
                byte_range: None,
                discontinuity: false,
                key: None,
                map: None,
                program_date_time: None,
                daterange: None,
                unknown_tags: vec![],
            },
            Bytes::from(data.to_vec()),
        )
    }

    #[tokio::test]
    async fn emits_init_before_media_on_gap_skip() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::SkipAfterCount(1);
        config.output_config.live_max_overall_stall_duration = None;

        let (input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let token = CancellationToken::new();

        let mut mgr = OutputManager::new(
            Arc::new(config),
            input_rx,
            event_tx,
            true,
            99,
            token.clone(),
        );

        let join = tokio::spawn(async move {
            mgr.run().await;
        });

        // Emit one media segment first so that count-based gap skipping is enabled (startup
        // suppresses count-based skipping for robustness).
        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "init0.mp4".to_string(),
                data: test_init_segment(b"init0"),
                media_sequence_number: 99,
                discontinuity: false,
            }))
            .await
            .unwrap();
        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "segment0.m4s".to_string(),
                data: test_media_segment(b"media0"),
                media_sequence_number: 99,
                discontinuity: false,
            }))
            .await
            .unwrap();

        // Drain the initial init+media emissions.
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut drained = 0usize;
            while drained < 2 {
                if let Some(evt) = event_rx.recv().await
                    && matches!(evt, Ok(HlsStreamEvent::Data(_)))
                {
                    drained += 1;
                }
            }
        })
        .await
        .expect("timed out waiting for initial emissions");

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "init.mp4".to_string(),
                data: test_init_segment(b"init"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "segment.m4s".to_string(),
                data: test_media_segment(b"media"),
                media_sequence_number: 101,
                discontinuity: false,
            }))
            .await
            .unwrap();

        let mut data_events: Vec<HlsData> = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = event_rx.recv().await {
                if let Ok(HlsStreamEvent::Data(data)) = evt {
                    data_events.push(*data);
                    if data_events.len() >= 2 {
                        break;
                    }
                }
            }
        })
        .await;
        assert!(read.is_ok(), "timed out waiting for events");

        assert_eq!(data_events.len(), 2);
        assert!(data_events[0].is_init_segment());
        assert!(!data_events[1].is_init_segment());

        token.cancel();
        drop(input_tx);
        let _ = join.await;
    }

    #[tokio::test]
    async fn emits_init_and_media_for_same_msn_without_collision() {
        let mut config = HlsConfig::default();
        config.output_config.live_max_overall_stall_duration = None;

        let (input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let token = CancellationToken::new();

        let mut mgr = OutputManager::new(
            Arc::new(config),
            input_rx,
            event_tx,
            true,
            100,
            token.clone(),
        );

        let join = tokio::spawn(async move {
            mgr.run().await;
        });

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "init.mp4".to_string(),
                data: test_init_segment(b"init"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "segment.m4s".to_string(),
                data: test_media_segment(b"media"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        let mut data_events: Vec<HlsData> = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = event_rx.recv().await {
                if let Ok(HlsStreamEvent::Data(data)) = evt {
                    data_events.push(*data);
                    if data_events.len() >= 2 {
                        break;
                    }
                }
            }
        })
        .await;
        assert!(read.is_ok(), "timed out waiting for events");

        assert_eq!(data_events.len(), 2);
        assert!(data_events[0].is_init_segment());
        assert!(!data_events[1].is_init_segment());

        token.cancel();
        drop(input_tx);
        let _ = join.await;
    }

    #[tokio::test]
    async fn does_not_emit_fmp4_media_before_init() {
        let mut config = HlsConfig::default();
        config.output_config.live_max_overall_stall_duration = None;
        config.output_config.live_gap_strategy = GapSkipStrategy::SkipAfterCount(1);

        let (input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let token = CancellationToken::new();

        let mut mgr = OutputManager::new(
            Arc::new(config),
            input_rx,
            event_tx,
            true,
            100,
            token.clone(),
        );

        let join = tokio::spawn(async move {
            mgr.run().await;
        });

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "segment.m4s".to_string(),
                data: test_media_segment(b"media"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        let no_event = tokio::time::timeout(Duration::from_millis(200), event_rx.recv()).await;
        assert!(no_event.is_err(), "should not emit media before init");

        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "init.mp4".to_string(),
                data: test_init_segment(b"init"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        let mut data_events: Vec<HlsData> = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = event_rx.recv().await {
                if let Ok(HlsStreamEvent::Data(data)) = evt {
                    data_events.push(*data);
                    if data_events.len() >= 2 {
                        break;
                    }
                }
            }
        })
        .await;
        assert!(read.is_ok(), "timed out waiting for events after init");

        assert_eq!(data_events.len(), 2);
        assert!(data_events[0].is_init_segment());
        assert!(!data_events[1].is_init_segment());

        token.cancel();
        drop(input_tx);
        let _ = join.await;
    }

    #[tokio::test]
    async fn duration_gap_skip_triggers_without_new_input() {
        let mut config = HlsConfig::default();
        config.output_config.live_max_overall_stall_duration = None;
        config.output_config.live_gap_strategy =
            GapSkipStrategy::SkipAfterDuration(Duration::from_millis(50));

        let (input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let token = CancellationToken::new();

        let mut mgr = OutputManager::new(
            Arc::new(config),
            input_rx,
            event_tx,
            true,
            100,
            token.clone(),
        );

        let join = tokio::spawn(async move {
            mgr.run().await;
        });

        // Provide init for fMP4, but do not provide media segment #100 (create a gap).
        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "init.mp4".to_string(),
                data: test_init_segment(b"init"),
                media_sequence_number: 100,
                discontinuity: false,
            }))
            .await
            .unwrap();

        // Provide media for #101; OutputManager should skip the missing #100 after duration even
        // if nothing else arrives.
        input_tx
            .send(Ok(ProcessedSegmentOutput {
                original_segment_uri: "segment101.m4s".to_string(),
                data: test_media_segment(b"media101"),
                media_sequence_number: 101,
                discontinuity: false,
            }))
            .await
            .unwrap();

        // We should eventually see a GapSkipped event and then Data emitted for #101.
        let (saw_gap, saw_data) = tokio::time::timeout(Duration::from_secs(2), async {
            let mut saw_gap = false;
            let mut saw_data = false;
            while let Some(evt) = event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped { .. }) => saw_gap = true,
                    Ok(HlsStreamEvent::Data(_)) => {
                        saw_data = true;
                        if saw_gap {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            (saw_gap, saw_data)
        })
        .await
        .expect("timed out waiting for gap skip + data");

        assert!(saw_gap, "expected GapSkipped event");
        assert!(saw_data, "expected Data event after gap skip");

        token.cancel();
        drop(input_tx);
        let _ = join.await;
    }

    #[test]
    fn startup_count_based_gap_skip_is_suppressed_until_first_media_emitted() {
        let mut config = HlsConfig::default();
        config.output_config.live_max_overall_stall_duration = None;
        config.output_config.live_gap_strategy = GapSkipStrategy::SkipAfterCount(3);

        let (_input_tx, input_rx) = mpsc::channel(1);
        let (event_tx, _event_rx) = mpsc::channel(1);
        let token = CancellationToken::new();

        let mut mgr = OutputManager::new(Arc::new(config), input_rx, event_tx, true, 0, token);

        mgr.is_fmp4_stream = true;
        mgr.gap_state = Some(GapState {
            missing_sequence: 0,
            detected_at: Instant::now(),
            segments_since_gap: 3,
        });
        mgr.has_emitted_media_segment = false;

        assert!(mgr.should_skip_gap().is_none());

        mgr.has_emitted_media_segment = true;
        assert!(matches!(
            mgr.should_skip_gap(),
            Some(GapSkipReason::CountThreshold(3))
        ));
    }
}
