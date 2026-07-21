//! The sequence assembler: ordered stream events from one typed input stream.
//!
//! Consumes `AssemblerInput` (payloads, skips, terminal failures, notices,
//! fatal errors, end) and emits `HlsStreamEvent` to the consumer channel — the
//! single producer of consumer-facing events. Reorders media by MSN, gates
//! fMP4 media behind init segments, applies gap policies, and keeps draining
//! input even under reorder-buffer pressure (the next item may be the one
//! that unblocks the buffer).
//!
//! Terminal semantics: an explicit `End` item (authoritative ENDLIST path)
//! drains the buffer in order and emits `StreamEnded`; `Fatal` drops the
//! buffer and emits the error; a channel close without either is a cancel and
//! emits nothing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::hls::HlsDownloaderError;
use crate::hls::config::{GapSkipStrategy, HlsConfig};
use crate::hls::events::{GapSkipReason, HlsStreamEvent};
use crate::hls::metrics::PerformanceMetrics;

use super::identity::{SegmentKey, SegmentKind};
use super::input::{AssemblerInput, PlaylistNotice};
use super::payload::SegmentPayload;

// --- Dead ranges: MSNs that will never arrive ---

/// Coalesced set of MSNs the upstream declared dead (window slides, ads,
/// terminal failures). When the emit cursor reaches a dead MSN, the assembler
/// advances past the whole run instead of waiting on it.
#[derive(Debug, Default)]
struct DeadRanges {
    /// from -> to (inclusive), non-overlapping, non-adjacent.
    ranges: BTreeMap<u64, u64>,
}

impl DeadRanges {
    fn insert(&mut self, from: u64, to: u64) {
        let (mut from, mut to) = (from.min(to), from.max(to));
        // Merge any range overlapping or adjacent to [from, to].
        loop {
            let merge = self
                .ranges
                .range(..=to.saturating_add(1))
                .next_back()
                .filter(|&(_, &t)| t.saturating_add(1) >= from)
                .map(|(&f, &t)| (f, t));
            match merge {
                Some((f, t)) => {
                    self.ranges.remove(&f);
                    from = from.min(f);
                    to = to.max(t);
                }
                None => break,
            }
        }
        self.ranges.insert(from, to);
    }

    /// If `msn` is dead, the inclusive end of its run.
    fn run_end(&self, msn: u64) -> Option<u64> {
        self.ranges
            .range(..=msn)
            .next_back()
            .filter(|&(_, &to)| to >= msn)
            .map(|(_, &to)| to)
    }

    fn prune_below(&mut self, msn: u64) {
        self.ranges.retain(|_, to| *to >= msn);
    }
}

/// Bounded insertion-ordered set of init keys. Init rotations are infrequent
/// (discontinuities / rendition changes), so a small cap with FIFO eviction
/// keeps memory bounded on long-running streams. Eviction is safe because an
/// unknown key is treated as unresolved: media still waits in the normal case,
/// and the live buffer-pressure / ENDLIST flush fail-safes surface stuck media
/// as visible gaps.
#[derive(Debug)]
struct BoundedKeySet {
    cap: usize,
    order: std::collections::VecDeque<SegmentKey>,
    set: std::collections::HashSet<SegmentKey>,
}

impl BoundedKeySet {
    fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: std::collections::VecDeque::new(),
            set: std::collections::HashSet::new(),
        }
    }

    fn insert(&mut self, key: SegmentKey) {
        if self.set.insert(key.clone()) {
            self.order.push_back(key);
            while self.order.len() > self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.set.remove(&oldest);
                }
            }
        }
    }

    fn contains(&self, key: &SegmentKey) -> bool {
        self.set.contains(key)
    }
}

// --- Reorder-buffer metrics (aggregated, bounded cardinality) ---

#[derive(Debug, Default)]
pub struct ReorderBufferMetrics {
    pub segments_received: AtomicU64,
    pub segments_emitted: AtomicU64,
    pub segments_rejected_stale: AtomicU64,
    pub gaps_detected: AtomicU64,
    pub gap_skips: AtomicU64,
    pub total_segments_skipped: AtomicU64,
    pub max_buffer_depth: AtomicU64,
}

impl ReorderBufferMetrics {
    fn record_depth(&self, depth: u64) {
        self.max_buffer_depth.fetch_max(depth, Ordering::Relaxed);
    }

    fn log_summary(&self) {
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
}

// --- Gap tracking ---

#[derive(Debug)]
struct GapState {
    missing_sequence: u64,
    detected_at: Instant,
    segments_since_gap: u64,
}

impl GapState {
    fn new(missing_sequence: u64) -> Self {
        Self {
            missing_sequence,
            detected_at: Instant::now(),
            segments_since_gap: 0,
        }
    }
}

#[derive(Debug)]
struct BufferedPayload {
    payload: SegmentPayload,
    buffered_at: Instant,
    size_bytes: usize,
}

impl BufferedPayload {
    fn new(payload: SegmentPayload) -> Self {
        let size_bytes = payload.len();
        Self {
            payload,
            buffered_at: Instant::now(),
            size_bytes,
        }
    }
}

enum EmitOutcome {
    Continue,
    /// The consumer channel closed: every send fails from here on.
    DownstreamClosed,
}

/// Whether the media segment at the cursor may be emitted yet, decided by its
/// governing init segment.
enum InitState {
    /// Emittable now (clear/TS, or its init has arrived).
    Ready,
    /// Wait — the governing init has not arrived yet.
    Gated,
    /// The governing init terminally failed; the media is undecodable.
    Failed,
}

pub struct SequenceAssembler {
    config: Arc<HlsConfig>,
    input_rx: mpsc::Receiver<AssemblerInput>,
    event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
    reorder_buffer: BTreeMap<u64, BufferedPayload>,
    /// fMP4 init segments keyed by the MSN at which they become applicable;
    /// kept out of `reorder_buffer` because it is keyed by MSN and an init
    /// and a media segment can share one.
    pending_init_segments: BTreeMap<u64, BufferedPayload>,
    /// fMP4 media is gated until its governing init arrives; emitting media
    /// first makes downstream consumers buffer or drop it.
    has_seen_init_segment: bool,
    is_fmp4_stream: bool,
    /// Init keys whose payload has arrived. A media segment whose
    /// `descriptor.init_key` is in this set may be emitted; one whose init is
    /// not yet here is gated (not just the first init — every rotation), so a
    /// rotated init cannot lose the race against the first media it covers.
    /// This set is intentionally bounded; if an eviction makes an old key
    /// unknown again while dependent media is still buffered, the live
    /// buffer-pressure and ENDLIST flush paths surface that media as a visible
    /// gap instead of stalling or dropping it silently.
    seen_init_keys: BoundedKeySet,
    /// Init keys that terminally failed. Media depending on a failed init can
    /// never be decoded, so it is skipped (a visible gap) rather than gating
    /// the stream forever. This is bounded for the same reason as
    /// `seen_init_keys`; eviction is fail-safe at the cursor and on flush.
    failed_init_keys: BoundedKeySet,
    /// Count-based gap skipping is suppressed on fMP4 streams until the first
    /// media emission: out-of-order completion under download concurrency
    /// makes early count triggers false positives.
    has_emitted_media_segment: bool,
    is_live_stream: bool,
    expected_next_media_sequence: u64,
    gap_state: Option<GapState>,
    dead: DeadRanges,
    last_input_received_time: Option<Instant>,
    current_buffer_bytes: usize,
    metrics: Arc<ReorderBufferMetrics>,
    performance_metrics: Option<Arc<PerformanceMetrics>>,
    cancel: CancellationToken,
}

impl SequenceAssembler {
    pub fn new(
        config: Arc<HlsConfig>,
        input_rx: mpsc::Receiver<AssemblerInput>,
        event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
        is_live_stream: bool,
        initial_media_sequence: u64,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            config,
            input_rx,
            event_tx,
            reorder_buffer: BTreeMap::new(),
            pending_init_segments: BTreeMap::new(),
            has_seen_init_segment: false,
            is_fmp4_stream: false,
            seen_init_keys: BoundedKeySet::new(256),
            failed_init_keys: BoundedKeySet::new(256),
            has_emitted_media_segment: false,
            is_live_stream,
            expected_next_media_sequence: initial_media_sequence,
            gap_state: None,
            dead: DeadRanges::default(),
            last_input_received_time: is_live_stream.then(Instant::now),
            current_buffer_bytes: 0,
            metrics: Arc::new(ReorderBufferMetrics::default()),
            performance_metrics: None,
            cancel,
        }
    }

    pub fn with_performance_metrics(mut self, metrics: Arc<PerformanceMetrics>) -> Self {
        self.performance_metrics = Some(metrics);
        self
    }

    fn gap_strategy(&self) -> &GapSkipStrategy {
        if self.is_live_stream {
            &self.config.output_config.live_gap_strategy
        } else {
            &self.config.output_config.vod_gap_strategy
        }
    }

    /// At-cap detection for the forced-skip rule: when the reorder buffer is
    /// over its byte/segment limit and blocked on a gap, the gap policy's
    /// count/duration thresholds are bypassed.
    fn buffer_at_limit(&self) -> bool {
        let limits = &self.config.output_config.buffer_limits;
        (limits.max_segments > 0 && self.reorder_buffer.len() >= limits.max_segments)
            || (limits.max_bytes > 0 && self.current_buffer_bytes >= limits.max_bytes)
    }

    /// Init-readiness of the buffered media segment at `msn`.
    fn init_state(&self, msn: u64) -> InitState {
        let Some(buffered) = self.reorder_buffer.get(&msn) else {
            return InitState::Ready;
        };
        if !matches!(buffered.payload, SegmentPayload::Mp4Media { .. }) {
            // TS (and any non-fMP4 media) needs no init.
            return InitState::Ready;
        }
        match buffered.payload.descriptor().init_key.as_ref() {
            Some(key) => {
                if self.seen_init_keys.contains(key) {
                    InitState::Ready
                } else if self.failed_init_keys.contains(key) {
                    InitState::Failed
                } else {
                    InitState::Gated
                }
            }
            // fMP4 media with no init_key tag: fall back to the coarse
            // "any init seen" gate so a malformed descriptor cannot emit
            // media before the stream's first init.
            None => {
                if self.has_seen_init_segment {
                    InitState::Ready
                } else {
                    InitState::Gated
                }
            }
        }
    }

    fn should_skip_gap(&self) -> Option<GapSkipReason> {
        let gap_state = self.gap_state.as_ref()?;

        // Buffer pressure overrides thresholds: waiting at the cap recreates
        // the deadlock the keep-draining rule exists to prevent.
        if self.buffer_at_limit() {
            return Some(GapSkipReason::BufferPressure);
        }

        let in_startup =
            self.is_live_stream && self.is_fmp4_stream && !self.has_emitted_media_segment;
        let elapsed = gap_state.detected_at.elapsed();

        match self.gap_strategy() {
            GapSkipStrategy::WaitIndefinitely => None,
            GapSkipStrategy::SkipAfterCount(threshold) => {
                if !in_startup && gap_state.segments_since_gap >= *threshold {
                    Some(GapSkipReason::CountThreshold(gap_state.segments_since_gap))
                } else {
                    None
                }
            }
            GapSkipStrategy::SkipAfterDuration(threshold) => {
                (elapsed >= *threshold).then_some(GapSkipReason::DurationThreshold(elapsed))
            }
            GapSkipStrategy::SkipAfterBoth { count, duration } => {
                let duration_exceeded = elapsed >= *duration;
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

    pub async fn run(mut self) {
        debug!(live = self.is_live_stream, "sequence assembler started");

        let gap_evaluation_interval = {
            let interval = self.config.output_config.gap_evaluation_interval;
            if interval.is_zero() {
                std::time::Duration::from_millis(1)
            } else {
                interval
            }
        };

        loop {
            let overall_stall_timeout = if self.is_live_stream
                && let Some(max_stall) = self.config.output_config.live_max_overall_stall_duration
                && let Some(last_input) = self.last_input_received_time
            {
                max_stall.saturating_sub(last_input.elapsed())
            } else {
                std::time::Duration::from_secs(u64::MAX / 2)
            };

            tokio::select! {
                biased;

                _ = self.cancel.cancelled() => {
                    debug!("assembler cancelled; dropping reorder buffer without emission");
                    return;
                }

                // Live stall watchdog: triggers regardless of gap strategy.
                _ = sleep(overall_stall_timeout),
                    if self.is_live_stream
                        && self.config.output_config.live_max_overall_stall_duration.is_some() =>
                {
                    if let Some(last_input) = self.last_input_received_time
                        && let Some(max_stall) =
                            self.config.output_config.live_max_overall_stall_duration
                        && last_input.elapsed() >= max_stall
                    {
                        error!(
                            "live stream stalled beyond {:?} with no input; terminating",
                            max_stall
                        );
                        if self
                            .event_tx
                            .send(Err(HlsDownloaderError::Timeout {
                                reason: "Stalled: No input received for max duration.".to_string(),
                            }))
                            .await
                            .is_err()
                        {
                            debug!("consumer closed before receiving stall timeout");
                        }
                        self.finish_summaries();
                        return;
                    }
                }

                input = self.input_rx.recv() => {
                    if self.is_live_stream {
                        self.last_input_received_time = Some(Instant::now());
                    }
                    match input {
                        Some(AssemblerInput::Payload(payload)) => {
                            if matches!(self.handle_payload(payload).await, EmitOutcome::DownstreamClosed) {
                                return;
                            }
                        }
                        Some(AssemblerInput::Skipped { from_msn, to_msn }) => {
                            trace!(from_msn, to_msn, "upstream skip range");
                            self.dead.insert(from_msn, to_msn);
                            if matches!(self.try_emit().await, EmitOutcome::DownstreamClosed) {
                                return;
                            }
                        }
                        Some(AssemblerInput::TerminalFailed { key, msn }) => {
                            if key.kind == SegmentKind::Init {
                                // An init that will never arrive. Any media
                                // depending on it (by init_key) can never be
                                // decoded; record the failure so try_emit skips
                                // those media as a visible gap instead of
                                // gating the stream forever. This handles both
                                // a first init failing before any media and a
                                // mid-stream rotation failing.
                                warn!(msn, "init segment terminally failed; dependent media will be skipped");
                                self.failed_init_keys.insert(key);
                            } else {
                                self.dead.insert(msn, msn);
                            }
                            if matches!(self.try_emit().await, EmitOutcome::DownstreamClosed) {
                                return;
                            }
                        }
                        Some(AssemblerInput::Notice(notice)) => {
                            let event = match notice {
                                PlaylistNotice::PlaylistRefreshed {
                                    media_sequence_base,
                                    target_duration,
                                } => HlsStreamEvent::PlaylistRefreshed {
                                    media_sequence_base,
                                    target_duration,
                                },
                                PlaylistNotice::EndlistEncountered => {
                                    HlsStreamEvent::EndlistEncountered
                                }
                            };
                            if self.event_tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                        Some(AssemblerInput::Fatal(err)) => {
                            // Pipeline error: buffered payloads are dropped,
                            // the error is the stream's terminal item.
                            warn!(error = %err, "fatal pipeline error; dropping reorder buffer");
                            if self.event_tx.send(Err(err)).await.is_err() {
                                debug!("consumer closed before receiving fatal pipeline error");
                            }
                            self.finish_summaries();
                            return;
                        }
                        Some(AssemblerInput::End) => {
                            // Authoritative end: ordered drain, then StreamEnded.
                            if self.flush_in_order().await.is_err() {
                                error!("consumer channel closed during end-of-stream drain");
                                return;
                            }
                            self.finish_summaries();
                            if self
                                .event_tx
                                .send(Ok(HlsStreamEvent::StreamEnded))
                                .await
                                .is_err()
                            {
                                debug!("consumer closed before receiving end-of-stream event");
                            }
                            return;
                        }
                        None => {
                            // Channel closed without End or Fatal: cancellation
                            // path. Drop the buffer, emit nothing.
                            debug!("assembler input closed without End; dropping buffer");
                            return;
                        }
                    }
                }

                // Periodic gap evaluation so duration-based skipping and VOD
                // timeouts fire even when no new input arrives.
                _ = sleep(gap_evaluation_interval), if self.gap_state.is_some() => {
                    if matches!(self.try_emit().await, EmitOutcome::DownstreamClosed) {
                        return;
                    }
                }
            }
        }
    }

    async fn handle_payload(&mut self, payload: SegmentPayload) -> EmitOutcome {
        let msn = payload.msn();

        if payload.is_fmp4() {
            self.is_fmp4_stream = true;
        }

        // fMP4 init segments are not part of the media sequence progression;
        // track them separately and emit them when the first applicable media
        // segment (MSN >= init's MSN) is emitted.
        if payload.is_init() {
            self.has_seen_init_segment = true;
            self.seen_init_keys.insert(payload.descriptor().key.clone());
            // pending_init_segments are not counted in current_buffer_bytes
            // (they are emitted as init events, not media), so a plain insert
            // is correct here.
            self.pending_init_segments
                .insert(msn, BufferedPayload::new(payload));
            let cap = self.config.output_config.max_pending_init_segments;
            if cap > 0 {
                while self.pending_init_segments.len() > cap {
                    self.pending_init_segments.pop_first();
                }
            }
            // An init can unblock already-buffered media gated on its key.
            return self.try_emit().await;
        }

        self.metrics
            .segments_received
            .fetch_add(1, Ordering::Relaxed);

        // Stale = MSN below the emit cursor (already emitted or skipped) or
        // declared dead. Reject without buffering.
        if msn < self.expected_next_media_sequence || self.dead.run_end(msn).is_some() {
            debug!(
                msn,
                expected = self.expected_next_media_sequence,
                "rejecting stale segment"
            );
            self.metrics
                .segments_rejected_stale
                .fetch_add(1, Ordering::Relaxed);
            return EmitOutcome::Continue;
        }

        let buffered = BufferedPayload::new(payload);
        self.current_buffer_bytes += buffered.size_bytes;
        // Replacing an existing entry at this MSN must release the old entry's
        // bytes, or current_buffer_bytes drifts upward forever and pins
        // buffer_at_limit() true. A duplicate MSN is reachable when a segment's
        // URI changes at an already-buffered MSN (CDN failover, ad re-stitch),
        // forming a new SegmentKey the store downloads as fresh work.
        let is_new_msn = match self.reorder_buffer.insert(msn, buffered) {
            Some(replaced) => {
                self.current_buffer_bytes = self
                    .current_buffer_bytes
                    .saturating_sub(replaced.size_bytes);
                false
            }
            None => true,
        };

        // Count only a genuinely-new later MSN toward the gap's skip-after-count
        // threshold. A duplicate-MSN replacement is not a new subsequent
        // segment, so counting it would trip SkipAfterCount/SkipAfterBoth a
        // segment early.
        if is_new_msn
            && self.is_live_stream
            && let Some(gap_state) = self.gap_state.as_mut()
            && msn > gap_state.missing_sequence
        {
            gap_state.segments_since_gap += 1;
        }

        self.metrics.record_depth(self.reorder_buffer.len() as u64);

        if self.buffer_at_limit() {
            warn!(
                segments = self.reorder_buffer.len(),
                bytes = self.current_buffer_bytes,
                "reorder buffer at capacity; gap skips are now forced"
            );
        }

        self.try_emit().await
    }

    /// Emit everything emittable from the reorder buffer, advancing the
    /// cursor through dead ranges and applying gap policy. Mirrors the
    /// while-loop shape of the original OutputManager.
    async fn try_emit(&mut self) -> EmitOutcome {
        loop {
            // Dead-range fast-forward: the cursor itself can be dead even with
            // an empty buffer (ads, window slide, terminal failure).
            if let Some(run_end) = self.dead.run_end(self.expected_next_media_sequence) {
                let from = self.expected_next_media_sequence;
                // A dead run ending at u64::MAX cannot be advanced past — the
                // cursor would saturate inside the range and re-skip forever.
                // Only a corrupt/hostile playlist reaches MSN u64::MAX; treat
                // it as a fatal stream error rather than spin.
                if run_end == u64::MAX {
                    error!("dead range extends to u64::MAX; aborting stream");
                    if self
                        .event_tx
                        .send(Err(HlsDownloaderError::Playlist {
                            reason: "media sequence reached u64::MAX".to_string(),
                        }))
                        .await
                        .is_err()
                    {
                        debug!("consumer closed before receiving invalid sequence error");
                    }
                    self.finish_summaries();
                    return EmitOutcome::DownstreamClosed;
                }
                let to = run_end + 1;
                self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
                self.metrics
                    .total_segments_skipped
                    .fetch_add(to - from, Ordering::Relaxed);
                debug!(from, to, "advancing past upstream-skipped MSNs");
                if self
                    .event_tx
                    .send(Ok(HlsStreamEvent::GapSkipped {
                        from_sequence: from,
                        to_sequence: to,
                        reason: GapSkipReason::Upstream,
                    }))
                    .await
                    .is_err()
                {
                    return EmitOutcome::DownstreamClosed;
                }
                self.expected_next_media_sequence = to;
                self.gap_state = None;
                self.dead.prune_below(to);
                continue;
            }

            let Some(first_msn) = self.reorder_buffer.keys().next().copied() else {
                break;
            };

            if first_msn == self.expected_next_media_sequence {
                // Init gating: fMP4 media is emittable only once the specific
                // init it depends on (descriptor.init_key) has arrived. This
                // gates every rotation, not just the first init, so a slow or
                // re-fetched init cannot lose the race against the first media
                // it covers.
                match self.init_state(first_msn) {
                    InitState::Ready => {}
                    InitState::Gated => {
                        if self.is_live_stream && self.buffer_at_limit() {
                            warn!(
                                msn = first_msn,
                                "skipping gated fMP4 media under buffer pressure"
                            );
                            if self
                                .skip_buffered_media_as_gap(
                                    first_msn,
                                    GapSkipReason::BufferPressure,
                                    true,
                                )
                                .await
                                .is_err()
                            {
                                return EmitOutcome::DownstreamClosed;
                            }
                            continue;
                        }
                        break;
                    }
                    InitState::Failed => {
                        // The governing init will never arrive: this media is
                        // undecodable. Skip it as a visible one-MSN gap rather
                        // than gating the stream forever.
                        warn!(
                            msn = first_msn,
                            "skipping media whose init segment terminally failed"
                        );
                        self.dead.insert(first_msn, first_msn);
                        continue;
                    }
                }

                let buffered = self
                    .reorder_buffer
                    .remove(&first_msn)
                    .expect("first key exists");
                self.current_buffer_bytes = self
                    .current_buffer_bytes
                    .saturating_sub(buffered.size_bytes);
                trace!(
                    msn = first_msn,
                    buffered_ms = buffered.buffered_at.elapsed().as_millis() as u64,
                    "emitting segment"
                );

                if self.emit_payload(buffered.payload).await.is_err() {
                    return EmitOutcome::DownstreamClosed;
                }

                self.expected_next_media_sequence += 1;
                self.gap_state = None;
                self.dead.prune_below(self.expected_next_media_sequence);
            } else if first_msn < self.expected_next_media_sequence {
                // Stale entry left behind by a skip.
                if let Some(buffered) = self.reorder_buffer.remove(&first_msn) {
                    self.current_buffer_bytes = self
                        .current_buffer_bytes
                        .saturating_sub(buffered.size_bytes);
                    self.metrics
                        .segments_rejected_stale
                        .fetch_add(1, Ordering::Relaxed);
                }
            } else {
                // Gap: first_msn > expected.
                let is_new_gap = self
                    .gap_state
                    .as_ref()
                    .is_none_or(|g| g.missing_sequence != self.expected_next_media_sequence);
                if is_new_gap {
                    self.metrics.gaps_detected.fetch_add(1, Ordering::Relaxed);
                    let mut gap = GapState::new(self.expected_next_media_sequence);
                    gap.segments_since_gap = self
                        .reorder_buffer
                        .range(self.expected_next_media_sequence + 1..)
                        .count() as u64;
                    self.gap_state = Some(gap);
                }

                // VOD per-segment timeout.
                if !self.is_live_stream
                    && let Some(vod_timeout) = self.config.output_config.vod_segment_timeout
                    && let Some(gap_state) = self.gap_state.as_ref()
                {
                    let elapsed = gap_state.detected_at.elapsed();
                    if elapsed >= vod_timeout {
                        warn!(
                            msn = gap_state.missing_sequence,
                            ?elapsed,
                            "VOD segment timed out; skipping"
                        );
                        if self
                            .event_tx
                            .send(Ok(HlsStreamEvent::SegmentTimeout {
                                sequence_number: gap_state.missing_sequence,
                                waited_duration: elapsed,
                            }))
                            .await
                            .is_err()
                        {
                            return EmitOutcome::DownstreamClosed;
                        }
                        self.record_gap_skip(first_msn);
                        self.expected_next_media_sequence = first_msn;
                        self.gap_state = None;
                        continue;
                    }
                }

                if let Some(reason) = self.should_skip_gap() {
                    warn!(
                        from = self.expected_next_media_sequence,
                        to = first_msn,
                        ?reason,
                        "gap confirmed; skipping"
                    );
                    self.record_gap_skip(first_msn);
                    if self
                        .event_tx
                        .send(Ok(HlsStreamEvent::GapSkipped {
                            from_sequence: self.expected_next_media_sequence,
                            to_sequence: first_msn,
                            reason,
                        }))
                        .await
                        .is_err()
                    {
                        return EmitOutcome::DownstreamClosed;
                    }
                    self.expected_next_media_sequence = first_msn;
                    self.gap_state = None;
                    continue;
                }

                // Wait for the missing segment (or a policy trigger).
                break;
            }
        }

        if self.is_live_stream {
            self.prune_reorder_buffer();
        }
        EmitOutcome::Continue
    }

    fn record_gap_skip(&self, to: u64) {
        self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.metrics.total_segments_skipped.fetch_add(
            to.saturating_sub(self.expected_next_media_sequence),
            Ordering::Relaxed,
        );
    }

    fn remove_buffered_payload(&mut self, msn: u64) -> Option<BufferedPayload> {
        let buffered = self.reorder_buffer.remove(&msn)?;
        self.current_buffer_bytes = self
            .current_buffer_bytes
            .saturating_sub(buffered.size_bytes);
        Some(buffered)
    }

    async fn emit_gap_skipped(
        &mut self,
        from: u64,
        to: u64,
        reason: GapSkipReason,
    ) -> Result<(), ()> {
        self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.metrics
            .total_segments_skipped
            .fetch_add(to.saturating_sub(from), Ordering::Relaxed);
        self.event_tx
            .send(Ok(HlsStreamEvent::GapSkipped {
                from_sequence: from,
                to_sequence: to,
                reason,
            }))
            .await
            .map_err(|_| ())
    }

    async fn abort_u64_max_cursor(&mut self) -> Result<(), ()> {
        error!("media sequence reached u64::MAX; aborting stream");
        self.event_tx
            .send(Err(HlsDownloaderError::Playlist {
                reason: "media sequence reached u64::MAX".to_string(),
            }))
            .await
            .map_err(|_| ())?;
        self.finish_summaries();
        Err(())
    }

    async fn skip_buffered_media_as_gap(
        &mut self,
        msn: u64,
        reason: GapSkipReason,
        advance_cursor: bool,
    ) -> Result<(), ()> {
        let Some(to) = msn.checked_add(1) else {
            return self.abort_u64_max_cursor().await;
        };
        if self.remove_buffered_payload(msn).is_none() {
            debug!(msn, "gap skip requested for missing buffered media");
            return Ok(());
        }
        self.emit_gap_skipped(msn, to, reason).await?;
        if advance_cursor {
            self.expected_next_media_sequence = to;
            self.gap_state = None;
            self.dead.prune_below(to);
        }
        Ok(())
    }

    /// Emit one media payload, preceded by its discontinuity event and any
    /// applicable pending init segment.
    async fn emit_payload(&mut self, payload: SegmentPayload) -> Result<(), ()> {
        let msn = payload.msn();
        let discontinuity = payload.discontinuity();

        if discontinuity {
            debug!(msn, "emitting discontinuity event");
            self.gap_state = None;
            if self
                .event_tx
                .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                .await
                .is_err()
            {
                return Err(());
            }
        }

        self.emit_applicable_init_segment(msn, discontinuity)
            .await?;

        let is_media = !payload.is_init();
        if self
            .event_tx
            .send(Ok(HlsStreamEvent::Data(Box::new(payload.into_hls_data()))))
            .await
            .is_err()
        {
            return Err(());
        }
        self.metrics
            .segments_emitted
            .fetch_add(1, Ordering::Relaxed);
        if is_media {
            self.has_emitted_media_segment = true;
        }
        Ok(())
    }

    /// Emit the most recent init segment applicable to `msn` (the active init
    /// state), dropping superseded ones.
    async fn emit_applicable_init_segment(
        &mut self,
        msn: u64,
        discontinuity_already_emitted: bool,
    ) -> Result<(), ()> {
        let keys: Vec<u64> = self
            .pending_init_segments
            .range(..=msn)
            .map(|(&k, _)| k)
            .collect();
        let mut last: Option<BufferedPayload> = None;
        for k in keys {
            last = self.pending_init_segments.remove(&k);
        }
        let Some(buffered_init) = last else {
            return Ok(());
        };

        if buffered_init.payload.discontinuity()
            && !discontinuity_already_emitted
            && self
                .event_tx
                .send(Ok(HlsStreamEvent::DiscontinuityTagEncountered {}))
                .await
                .is_err()
        {
            return Err(());
        }

        if self
            .event_tx
            .send(Ok(HlsStreamEvent::Data(Box::new(
                buffered_init.payload.into_hls_data(),
            ))))
            .await
            .is_err()
        {
            return Err(());
        }
        self.metrics
            .segments_emitted
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Authoritative-end drain: emit everything left in MSN order. fMP4 media
    /// whose governing init never arrived is surfaced as a visible gap.
    async fn flush_in_order(&mut self) -> Result<(), ()> {
        while let Some(msn) = self.reorder_buffer.keys().next().copied() {
            match self.init_state(msn) {
                InitState::Ready => {}
                InitState::Gated => {
                    warn!(
                        msn,
                        "skipping fMP4 media during flush: governing init never arrived"
                    );
                    self.skip_buffered_media_as_gap(msn, GapSkipReason::Upstream, false)
                        .await?;
                    continue;
                }
                InitState::Failed => {
                    warn!(
                        msn,
                        "skipping fMP4 media during flush: governing init terminally failed"
                    );
                    self.skip_buffered_media_as_gap(msn, GapSkipReason::Upstream, false)
                        .await?;
                    continue;
                }
            }

            if let Some(buffered) = self.remove_buffered_payload(msn) {
                self.emit_payload(buffered.payload).await?;
            }
        }
        self.current_buffer_bytes = 0;
        Ok(())
    }

    /// Live pruning by count and buffered duration (only entries below the
    /// emit cursor are eligible — entries at/above it are pending output).
    fn prune_reorder_buffer(&mut self) {
        let max_segments = self.config.output_config.live_reorder_buffer_max_segments;
        if max_segments > 0 && self.reorder_buffer.len() > max_segments {
            let excess = self.reorder_buffer.len() - max_segments;
            let stale: Vec<u64> = self
                .reorder_buffer
                .keys()
                .filter(|&&k| k < self.expected_next_media_sequence)
                .take(excess)
                .copied()
                .collect();
            for msn in stale {
                if let Some(b) = self.reorder_buffer.remove(&msn) {
                    self.current_buffer_bytes =
                        self.current_buffer_bytes.saturating_sub(b.size_bytes);
                }
            }
        }

        let max_duration = self
            .config
            .output_config
            .live_reorder_buffer_duration
            .as_secs_f32();
        if max_duration > 0.0 {
            let mut total = 0.0f32;
            let mut cutoff: Option<u64> = None;
            for (&msn, buffered) in self
                .reorder_buffer
                .range(..self.expected_next_media_sequence)
                .rev()
            {
                total += buffered.payload.descriptor().media_segment.duration;
                if total > max_duration {
                    cutoff = Some(msn);
                    break;
                }
            }
            if let Some(cutoff) = cutoff {
                // split_off(cutoff+1)? No: keep >= cutoff+1; everything below
                // (including cutoff) is removed.
                let kept = self.reorder_buffer.split_off(&(cutoff + 1));
                let removed_bytes: usize = self.reorder_buffer.values().map(|b| b.size_bytes).sum();
                self.current_buffer_bytes = self.current_buffer_bytes.saturating_sub(removed_bytes);
                self.reorder_buffer = kept;
            }
        }
    }

    fn finish_summaries(&self) {
        self.metrics.log_summary();
        if let Some(perf) = &self.performance_metrics {
            perf.log_summary();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::engine::descriptor::{SegmentDescriptor, SegmentSource};
    use crate::hls::engine::identity::SegmentKey;
    use bytes::Bytes;
    use std::time::Duration;
    use url::Url;

    fn descriptor(uri: &str, msn: u64, kind: SegmentKind) -> Arc<SegmentDescriptor> {
        Arc::new(SegmentDescriptor {
            key: SegmentKey {
                kind,
                uri: Arc::from(uri),
                byte_range: None,
            },
            msn,
            source: SegmentSource::Playlist,
            parsed_url: Arc::new(Url::parse(uri).unwrap()),
            discontinuity: false,
            encryption: None,
            init_key: None,
            media_segment: Arc::new(m3u8_rs::MediaSegment {
                uri: uri.to_string(),
                duration: 2.0,
                ..Default::default()
            }),
        })
    }

    fn ts_payload(msn: u64) -> AssemblerInput {
        AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from(format!("seg{msn}")),
            descriptor: descriptor(
                &format!("https://e.com/seg{msn}.ts"),
                msn,
                SegmentKind::Media,
            ),
        })
    }

    fn mp4_media(msn: u64) -> AssemblerInput {
        AssemblerInput::Payload(SegmentPayload::Mp4Media {
            data: Bytes::from(format!("m{msn}")),
            descriptor: descriptor(
                &format!("https://e.com/seg{msn}.m4s"),
                msn,
                SegmentKind::Media,
            ),
        })
    }

    fn mp4_init(msn: u64) -> AssemblerInput {
        AssemblerInput::Payload(SegmentPayload::Mp4Init {
            data: Bytes::from(format!("init{msn}")),
            descriptor: descriptor(
                &format!("https://e.com/init{msn}.mp4"),
                msn,
                SegmentKind::Init,
            ),
        })
    }

    fn init_key(uri: &str) -> SegmentKey {
        SegmentKey {
            kind: SegmentKind::Init,
            uri: Arc::from(uri),
            byte_range: None,
        }
    }

    /// An init payload plus the key media will reference via `init_key`.
    fn mp4_init_keyed(uri: &str, msn: u64) -> (AssemblerInput, SegmentKey) {
        let key = init_key(uri);
        let payload = AssemblerInput::Payload(SegmentPayload::Mp4Init {
            data: Bytes::from(format!("init-{uri}")),
            descriptor: descriptor(uri, msn, SegmentKind::Init),
        });
        (payload, key)
    }

    /// An fMP4 media payload that depends on the given init key.
    fn mp4_media_keyed(msn: u64, init: &SegmentKey) -> AssemblerInput {
        let uri = format!("https://e.com/seg{msn}.m4s");
        let mut d = (*descriptor(&uri, msn, SegmentKind::Media)).clone();
        d.init_key = Some(init.clone());
        AssemblerInput::Payload(SegmentPayload::Mp4Media {
            data: Bytes::from(format!("m{msn}")),
            descriptor: Arc::new(d),
        })
    }

    struct Harness {
        input_tx: mpsc::Sender<AssemblerInput>,
        event_rx: mpsc::Receiver<Result<HlsStreamEvent, HlsDownloaderError>>,
        cancel: CancellationToken,
        join: tokio::task::JoinHandle<()>,
    }

    fn spawn_assembler(mut config: HlsConfig, live: bool, initial_msn: u64) -> Harness {
        config.output_config.live_max_overall_stall_duration = None;
        let (input_tx, input_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let assembler = SequenceAssembler::new(
            Arc::new(config),
            input_rx,
            event_tx,
            live,
            initial_msn,
            cancel.clone(),
        );
        let join = tokio::spawn(assembler.run());
        Harness {
            input_tx,
            event_rx,
            cancel,
            join,
        }
    }

    async fn collect_data_uris(h: &mut Harness, n: usize) -> Vec<String> {
        let mut uris = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while uris.len() < n {
                match h.event_rx.recv().await {
                    Some(Ok(HlsStreamEvent::Data(data))) => {
                        uris.push(
                            data.media_segment()
                                .map(|s| s.uri.clone())
                                .unwrap_or_default(),
                        );
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .expect("timed out collecting data events");
        uris
    }

    #[tokio::test]
    async fn reorders_out_of_order_payloads() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx.send(ts_payload(101)).await.unwrap();
        h.input_tx.send(ts_payload(100)).await.unwrap();
        let uris = collect_data_uris(&mut h, 2).await;
        assert_eq!(uris, ["https://e.com/seg100.ts", "https://e.com/seg101.ts"]);
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn terminal_failure_unblocks_instead_of_stalling() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);

        h.input_tx.send(ts_payload(101)).await.unwrap();
        // Without this, WaitIndefinitely would stall forever on 100.
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: SegmentKey {
                    kind: SegmentKind::Media,
                    uri: Arc::from("https://e.com/seg100.ts"),
                    byte_range: None,
                },
                msn: 100,
            })
            .await
            .unwrap();

        let mut saw_gap = false;
        let mut got = None;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped {
                        reason: GapSkipReason::Upstream,
                        ..
                    }) => {
                        saw_gap = true;
                    }
                    Ok(HlsStreamEvent::Data(d)) => {
                        got = d.media_segment().map(|s| s.uri.clone());
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out");
        assert!(saw_gap);
        assert_eq!(got.as_deref(), Some("https://e.com/seg101.ts"));
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn skipped_range_advances_even_with_empty_buffer() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);

        // Window slid: 100..=104 are gone; 105 arrives later.
        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 100,
                to_msn: 104,
            })
            .await
            .unwrap();
        h.input_tx.send(ts_payload(105)).await.unwrap();

        let uris = collect_data_uris(&mut h, 1).await;
        assert_eq!(uris, ["https://e.com/seg105.ts"]);
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn end_drains_in_order_then_stream_ended() {
        let mut h = spawn_assembler(HlsConfig::default(), false, 0);
        h.input_tx.send(ts_payload(1)).await.unwrap();
        h.input_tx.send(ts_payload(0)).await.unwrap();
        h.input_tx.send(ts_payload(2)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();

        let mut uris = Vec::new();
        let mut ended = false;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::Data(d)) => {
                        uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                    }
                    Ok(HlsStreamEvent::StreamEnded) => {
                        ended = true;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out");
        assert_eq!(
            uris,
            [
                "https://e.com/seg0.ts",
                "https://e.com/seg1.ts",
                "https://e.com/seg2.ts"
            ]
        );
        assert!(ended);
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn close_without_end_emits_nothing() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx.send(ts_payload(101)).await.unwrap();
        drop(h.input_tx);
        let _ = h.join.await;
        // Buffered 101 dropped; no StreamEnded, no Data.
        let mut events = Vec::new();
        while let Ok(evt) = h.event_rx.try_recv() {
            events.push(evt);
        }
        assert!(
            events.is_empty(),
            "cancel path must not emit, got {events:?}"
        );
    }

    #[tokio::test]
    async fn fatal_drops_buffer_and_emits_error() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx.send(ts_payload(101)).await.unwrap();
        h.input_tx
            .send(AssemblerInput::Fatal(HlsDownloaderError::Playlist {
                reason: "watcher died".to_string(),
            }))
            .await
            .unwrap();

        let evt = tokio::time::timeout(Duration::from_secs(2), h.event_rx.recv())
            .await
            .expect("event")
            .expect("open");
        assert!(matches!(evt, Err(HlsDownloaderError::Playlist { .. })));
        let next = h.event_rx.recv().await;
        assert!(next.is_none(), "no events after the terminal Err");
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn fmp4_media_gated_until_init_and_init_emitted_first() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx.send(mp4_media(100)).await.unwrap();
        // Nothing should be emitted yet.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(h.event_rx.try_recv().is_err(), "media must wait for init");

        h.input_tx.send(mp4_init(100)).await.unwrap();
        let mut data = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while data.len() < 2 {
                if let Some(Ok(HlsStreamEvent::Data(d))) = h.event_rx.recv().await {
                    data.push(d.is_init_segment());
                }
            }
        })
        .await
        .expect("timed out");
        assert_eq!(data, [true, false], "init must precede media");
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn buffer_pressure_forces_gap_skip() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        config.output_config.buffer_limits.max_segments = 2;
        config.output_config.buffer_limits.max_bytes = 0;
        let mut h = spawn_assembler(config, true, 100);

        // 100 never arrives; the buffer fills to its cap with later segments.
        h.input_tx.send(ts_payload(101)).await.unwrap();
        h.input_tx.send(ts_payload(102)).await.unwrap();

        let mut saw_pressure_skip = false;
        let mut uris = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while uris.len() < 2 {
                match h.event_rx.recv().await {
                    Some(Ok(HlsStreamEvent::GapSkipped {
                        reason: GapSkipReason::BufferPressure,
                        ..
                    })) => saw_pressure_skip = true,
                    Some(Ok(HlsStreamEvent::Data(d))) => {
                        uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .expect("timed out: buffer-pressure skip did not fire");
        assert!(saw_pressure_skip, "WaitIndefinitely must yield at the cap");
        assert_eq!(uris, ["https://e.com/seg101.ts", "https://e.com/seg102.ts"]);
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn late_payload_for_skipped_msn_is_stale_rejected() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);

        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 100,
                to_msn: 100,
            })
            .await
            .unwrap();
        h.input_tx.send(ts_payload(101)).await.unwrap();
        let uris = collect_data_uris(&mut h, 1).await;
        assert_eq!(uris, ["https://e.com/seg101.ts"]);

        // Late completion for the skipped MSN must not be emitted.
        h.input_tx.send(ts_payload(100)).await.unwrap();
        h.input_tx.send(ts_payload(102)).await.unwrap();
        let uris = collect_data_uris(&mut h, 1).await;
        assert_eq!(uris, ["https://e.com/seg102.ts"]);
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn notices_forward_immediately() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx
            .send(AssemblerInput::Notice(PlaylistNotice::EndlistEncountered))
            .await
            .unwrap();
        let evt = tokio::time::timeout(Duration::from_secs(1), h.event_rx.recv())
            .await
            .expect("event")
            .expect("open");
        assert!(matches!(evt, Ok(HlsStreamEvent::EndlistEncountered)));
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn rotated_init_gates_media_until_its_own_init_arrives() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let (init1, key1) = mp4_init_keyed("https://e.com/init1.mp4", 100);
        let (init2, key2) = mp4_init_keyed("https://e.com/init2.mp4", 101);

        // First init + first media emit normally.
        h.input_tx.send(init1).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key1)).await.unwrap();
        let first = collect_data_uris(&mut h, 2).await; // init1 then media100
        assert_eq!(first[1], "https://e.com/seg100.m4s");

        // Media 101 depends on the rotated init2, which has NOT arrived. Even
        // though an init was already seen, media 101 must be gated.
        h.input_tx.send(mp4_media_keyed(101, &key2)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            h.event_rx.try_recv().is_err(),
            "media must wait for its own rotated init, not emit under the old init"
        );

        // init2 arrives → media 101 unblocks, init2 emitted before it.
        h.input_tx.send(init2).await.unwrap();
        let next = collect_data_uris(&mut h, 2).await;
        assert_eq!(
            next,
            ["https://e.com/init2.mp4", "https://e.com/seg101.m4s"]
        );
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn init_terminal_failure_before_media_skips_dependent_media_not_stall() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);
        let key = init_key("https://e.com/init.mp4");

        // The init terminally fails before any media payload arrives.
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: key.clone(),
                msn: 100,
            })
            .await
            .unwrap();
        // Media depending on the failed init, plus a clear TS that follows.
        h.input_tx.send(mp4_media_keyed(100, &key)).await.unwrap();
        h.input_tx.send(ts_payload(101)).await.unwrap();

        // The dependent media is skipped (visible gap), 101 still emits — no
        // permanent gated stall, no unbounded buffer.
        let mut saw_gap = false;
        let mut got = None;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped { .. }) => saw_gap = true,
                    Ok(HlsStreamEvent::Data(d)) => {
                        got = d.media_segment().map(|s| s.uri.clone());
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out: init failure stalled the stream");
        assert!(saw_gap, "failed-init media must surface as a gap skip");
        assert_eq!(got.as_deref(), Some("https://e.com/seg101.ts"));
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn gated_media_skips_under_live_buffer_pressure() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        config.output_config.buffer_limits.max_segments = 2;
        config.output_config.buffer_limits.max_bytes = 0;
        let mut h = spawn_assembler(config, true, 100);
        let unknown_init = init_key("https://e.com/forgotten-init.mp4");

        h.input_tx
            .send(mp4_media_keyed(100, &unknown_init))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            h.event_rx.try_recv().is_err(),
            "gated media below the buffer limit should still wait for init"
        );

        h.input_tx.send(ts_payload(101)).await.unwrap();

        let mut saw_pressure_skip = false;
        let mut got = None;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped {
                        from_sequence: 100,
                        to_sequence: 101,
                        reason: GapSkipReason::BufferPressure,
                    }) => saw_pressure_skip = true,
                    Ok(HlsStreamEvent::Data(d)) => {
                        got = d.media_segment().map(|s| s.uri.clone());
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out: gated media stalled under buffer pressure");
        assert!(
            saw_pressure_skip,
            "gated media at the buffer limit must surface as a visible gap"
        );
        assert_eq!(got.as_deref(), Some("https://e.com/seg101.ts"));
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn flush_skips_fmp4_media_with_missing_init_as_visible_gap() {
        let mut h = spawn_assembler(HlsConfig::default(), false, 100);
        let unknown_init = init_key("https://e.com/missing-init.mp4");

        h.input_tx
            .send(mp4_media_keyed(100, &unknown_init))
            .await
            .unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();

        let mut saw_gap = false;
        let mut saw_data = false;
        let mut ended = false;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped {
                        from_sequence: 100,
                        to_sequence: 101,
                        ..
                    }) => saw_gap = true,
                    Ok(HlsStreamEvent::Data(_)) => saw_data = true,
                    Ok(HlsStreamEvent::StreamEnded) => {
                        ended = true;
                        break;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out waiting for flush gap and end");
        assert!(saw_gap, "flush must surface unresolved fMP4 media as a gap");
        assert!(!saw_data, "unresolved fMP4 media must not be emitted");
        assert!(ended, "flush must still end the stream");
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn duplicate_msn_does_not_double_count_gap_skip_threshold() {
        // SkipAfterCount(3): the gap at 100 must only skip once THREE distinct
        // later MSNs have arrived. A duplicate-MSN replacement must not count.
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::SkipAfterCount(3);
        let mut h = spawn_assembler(config, true, 100);

        // Emit one media first so count-based skipping is enabled (TS streams
        // have no startup suppression, but keep the cursor moving cleanly).
        h.input_tx.send(ts_payload(101)).await.unwrap();
        // Two distinct payloads at MSN 102 (duplicate MSN): only ONE distinct
        // later segment past the gap.
        let dup_a = AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from_static(b"a"),
            descriptor: descriptor("https://e.com/a.ts", 102, SegmentKind::Media),
        });
        let dup_b = AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from_static(b"b"),
            descriptor: descriptor("https://e.com/b.ts", 102, SegmentKind::Media),
        });
        h.input_tx.send(dup_a).await.unwrap();
        h.input_tx.send(dup_b).await.unwrap();

        // Two distinct later MSNs (101, 102) have arrived — below the threshold
        // of 3 — so NO gap skip must fire yet. (If the duplicate were counted,
        // segments_since_gap would reach 3 and skip prematurely.)
        let premature = tokio::time::timeout(Duration::from_millis(200), h.event_rx.recv()).await;
        assert!(
            premature.is_err(),
            "duplicate MSN must not trip the count threshold early"
        );

        // A genuine third distinct MSN trips it.
        h.input_tx.send(ts_payload(103)).await.unwrap();
        let saw_skip = tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                if matches!(evt, Ok(HlsStreamEvent::GapSkipped { .. })) {
                    return true;
                }
            }
            false
        })
        .await
        .expect("timed out");
        assert!(saw_skip, "third distinct segment should trip the threshold");
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn duplicate_msn_insert_does_not_leak_buffer_bytes() {
        // Two distinct payloads at the same MSN (URI changed → new key →
        // store treats as fresh work) must not double-count buffer bytes.
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        config.output_config.buffer_limits.max_segments = 0;
        config.output_config.buffer_limits.max_bytes = 32;
        let mut h = spawn_assembler(config, true, 100);

        // Both buffer behind the missing 100..; second replaces first at 101.
        let dup_a = AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from(vec![0u8; 20]),
            descriptor: descriptor("https://e.com/a.ts", 101, SegmentKind::Media),
        });
        let dup_b = AssemblerInput::Payload(SegmentPayload::Ts {
            data: Bytes::from(vec![0u8; 20]),
            descriptor: descriptor("https://e.com/b.ts", 101, SegmentKind::Media),
        });
        h.input_tx.send(dup_a).await.unwrap();
        h.input_tx.send(dup_b).await.unwrap();
        // Now deliver 100; if bytes leaked (40 > 32 cap) a BufferPressure skip
        // would have fired and dropped 100. With correct accounting (20 <= 32),
        // 100 then 101 emit in order.
        h.input_tx.send(ts_payload(100)).await.unwrap();

        let uris = collect_data_uris(&mut h, 2).await;
        assert_eq!(uris[0], "https://e.com/seg100.ts");
        assert_eq!(uris[1], "https://e.com/b.ts");
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[test]
    fn dead_ranges_merge_and_query() {
        let mut dead = DeadRanges::default();
        dead.insert(10, 12);
        dead.insert(13, 14); // adjacent: merges
        dead.insert(20, 22);
        assert_eq!(dead.run_end(10), Some(14));
        assert_eq!(dead.run_end(14), Some(14));
        assert_eq!(dead.run_end(15), None);
        assert_eq!(dead.run_end(21), Some(22));
        dead.prune_below(15);
        assert_eq!(dead.run_end(10), None);
        assert_eq!(dead.run_end(21), Some(22));
    }
}
