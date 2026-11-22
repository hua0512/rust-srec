//! The sequence assembler: ordered stream events from one typed input stream.
//!
//! Consumes `AssemblerInput` (payloads, skips, terminal failures, notices,
//! fatal errors, end) and hands ordered [`AssembledItem`]s to its
//! [`AssemblerSink`] — the single producer of consumer-facing events. Reorders
//! media by MSN, gates fMP4 media behind init segments, applies gap policies,
//! and keeps draining input even under reorder-buffer pressure (the next item
//! may be the one that unblocks the buffer).
//!
//! Terminal semantics: an explicit `End` item (authoritative-end path)
//! drains the buffer in order and emits `StreamEnded`; `Fatal` drops the
//! buffer and emits the error; a channel close without either is a cancel and
//! emits nothing. A stream that emits no media fails rather than ending
//! empty when segments failed: at `End`, with the first failure (an init's
//! first). A live stream, which never ends, fails as soon as the media it
//! would start with cannot be decoded because its init failed.
//!
//! fMP4 ordering contract, relied on by downstream processing (`hls-fix`):
//! - An init is emitted only immediately before media it governs (matched by
//!   the media's `init_key`, never by MSN), and only when it differs from the
//!   last emitted init. A map that stays the same across a discontinuity is
//!   not re-emitted; a map the stream switches back to is.
//! - An init that no emitted media references is never emitted. Consumers
//!   never see an init followed by a discontinuity, the end of the stream, or
//!   another init.
//! - A discontinuity event precedes the init and media it applies to.
//! - Media whose init is missing or failed is skipped as a gap and never
//!   emitted without that init (unless that fails a live stream, see above).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::DownloadError;
use crate::error::SharedError;

use super::config::SegmentCoreConfig;
use super::control::EngineNotice;
use super::identity::{SegmentKey, SegmentKind};
use super::input::AssemblerInput;
use super::metrics::PerformanceMetrics;
use super::payload::SegmentPayload;
use super::throttle::{RateLimitHolds, ThrottleClock};

mod gaps;
mod inits;

use gaps::{DeadRanges, GapState};
use inits::{FailedInits, RetainedInits};

#[cfg(test)]
mod tests;

// --- Output ---

/// Reason why a gap was skipped in the reorder buffer
#[derive(Debug, Clone)]
pub enum GapSkipReason {
    /// Gap skipped because count threshold was exceeded
    CountThreshold(u64),
    /// Gap skipped because duration threshold was exceeded
    DurationThreshold(Duration),
    /// Gap skipped because both count and duration thresholds was exceeded
    BothThresholds { count: u64, duration: Duration },
    /// Upstream declared these MSNs will never arrive (window slide, ad
    /// filtering, or terminal segment failure) — the assembler advances
    /// without waiting on any threshold.
    Upstream,
    /// The reorder buffer reached its byte/segment cap while blocked on this
    /// gap; the skip decision is forced immediately instead of waiting at the
    /// cap (which would deadlock output behind an out-of-order result).
    BufferPressure,
}

/// One ordered output item. Payloads arrive in sequence order, each preceded
/// by its discontinuity and, for fMP4, the init it is decoded with.
#[derive(Debug)]
pub enum AssembledItem {
    Payload(SegmentPayload),
    /// The next payload starts a discontinuity.
    Discontinuity,
    /// The cursor advanced past sequence numbers `from_sequence` up to (not
    /// including) `to_sequence` without emitting them.
    GapSkipped {
        from_sequence: u64,
        to_sequence: u64,
        reason: GapSkipReason,
    },
    /// VOD: a missing segment was skipped after the per-segment timeout.
    SegmentTimeout {
        sequence_number: u64,
        waited_duration: Duration,
    },
    Notice(EngineNotice),
    /// Authoritative end; nothing follows.
    StreamEnded,
}

/// The sink's consumer is gone; every later send would fail too.
#[derive(Debug)]
pub struct SinkClosed;

/// Receives the assembler's output. Called inline from the assembler task, so
/// a sink converts and forwards without another channel hop; an `Err` item is
/// the stream's terminal error.
pub trait AssemblerSink: Send + 'static {
    fn send(
        &mut self,
        item: Result<AssembledItem, DownloadError>,
    ) -> impl Future<Output = Result<(), SinkClosed>> + Send;
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

/// How a [`SequenceAssembler`] finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblerExit {
    /// The stream ended, was cancelled, or its consumer went away.
    Done,
    /// The stream's terminal item was this error. The other pipeline tasks
    /// see only that the assembler stopped, so the error is the session's
    /// outcome.
    Failed(Arc<str>),
}

enum EmitOutcome {
    Continue,
    /// Nothing more is emitted: the consumer channel closed, or the stream's
    /// terminal error was delivered.
    Finished,
}

/// Whether the media segment at the cursor may be emitted yet, decided by its
/// governing init segment.
enum InitState {
    /// Emittable now (clear/TS, or its init has arrived).
    Ready,
    /// Wait — the governing init has not arrived yet.
    Gated,
    /// The governing init terminally failed or will never arrive; the media is undecodable.
    Failed,
}

pub struct SequenceAssembler<S> {
    config: Arc<SegmentCoreConfig>,
    input_rx: mpsc::Receiver<AssemblerInput>,
    sink: S,
    reorder_buffer: BTreeMap<u64, BufferedPayload>,
    /// fMP4 init payloads that have arrived, kept out of `reorder_buffer`
    /// because an init is not part of the media sequence.
    inits: RetainedInits,
    /// The init most recently emitted: the one consumers decode media with.
    active_init_key: Option<SegmentKey>,
    /// The init most recently received, for fMP4 media without an `init_key`.
    latest_init_key: Option<SegmentKey>,
    /// Highest media sequence number emitted so far, used to keep unemitted
    /// future maps from being evicted before the cursor reaches their media.
    last_emitted_media_msn: Option<u64>,
    /// Set once the reactor confirms the stream is ending and no further init
    /// segment can arrive.
    no_more_inits: bool,
    is_fmp4_stream: bool,
    /// Init keys that terminally failed, with why.
    failed_inits: FailedInits,
    /// The first terminal failure of an init (or of media for want of its
    /// init) and of a media segment.
    init_failure: Option<SharedError>,
    media_failure: Option<SharedError>,
    /// Live streams, before any media is emitted: media that failed because
    /// its init did, by sequence number.
    undecodable: BTreeMap<u64, SharedError>,
    /// The error the stream ended with, once it did.
    failure: Option<Arc<str>>,
    /// Count-based gap skipping is suppressed on fMP4 streams until the first
    /// media emission: out-of-order completion under download concurrency
    /// makes early count triggers false positives.
    has_emitted_media_segment: bool,
    is_live_stream: bool,
    expected_next_media_sequence: u64,
    /// Publishes `expected_next_media_sequence` (see
    /// [`Self::subscribe_cursor`]).
    cursor: Option<watch::Sender<u64>>,
    rate_limit_holds: Arc<RateLimitHolds>,
    gap_state: Option<GapState>,
    dead: DeadRanges,
    last_input_received_time: Option<Instant>,
    last_input_paused: Duration,
    current_buffer_bytes: usize,
    metrics: Arc<ReorderBufferMetrics>,
    /// Session-wide download metrics and the protocol they are logged
    /// under when the stream finishes.
    performance_metrics: Option<(Arc<PerformanceMetrics>, &'static str)>,
    cancel: CancellationToken,
}

impl<S: AssemblerSink> SequenceAssembler<S> {
    /// `initial_cursor` is the first sequence number the stream will emit;
    /// anything below it is stale.
    pub fn new(
        config: Arc<SegmentCoreConfig>,
        input_rx: mpsc::Receiver<AssemblerInput>,
        sink: S,
        is_live_stream: bool,
        initial_cursor: u64,
        cancel: CancellationToken,
    ) -> Self {
        let max_retained_inits = config.output.max_pending_init_segments;
        let assembler = Self {
            config,
            input_rx,
            sink,
            reorder_buffer: BTreeMap::new(),
            inits: RetainedInits::new(max_retained_inits),
            active_init_key: None,
            latest_init_key: None,
            last_emitted_media_msn: None,
            no_more_inits: false,
            is_fmp4_stream: false,
            failed_inits: FailedInits::new(256),
            init_failure: None,
            media_failure: None,
            undecodable: BTreeMap::new(),
            failure: None,
            has_emitted_media_segment: false,
            is_live_stream,
            expected_next_media_sequence: initial_cursor,
            cursor: None,
            rate_limit_holds: Arc::new(RateLimitHolds::new()),
            gap_state: None,
            dead: DeadRanges::default(),
            last_input_received_time: is_live_stream.then(Instant::now),
            last_input_paused: Duration::ZERO,
            current_buffer_bytes: 0,
            metrics: Arc::new(ReorderBufferMetrics::default()),
            performance_metrics: None,
            cancel,
        };
        // Fetches can report throttling before this task is first polled.
        assembler.refresh_rate_limit_scope(Instant::now());
        assembler
    }

    /// Logs `metrics` when the stream finishes. A session with several
    /// assemblers logs its metrics itself instead.
    pub fn with_performance_metrics(
        mut self,
        metrics: Arc<PerformanceMetrics>,
        protocol: &'static str,
    ) -> Self {
        self.performance_metrics = Some((metrics, protocol));
        self
    }

    /// The emit cursor: the next media sequence number the stream emits or
    /// skips. Every media segment below it has been passed downstream or
    /// given up on, so the reorder buffer only holds segments at or above it.
    pub fn subscribe_cursor(&mut self) -> watch::Receiver<u64> {
        self.cursor
            .get_or_insert_with(|| watch::channel(self.expected_next_media_sequence).0)
            .subscribe()
    }

    pub fn rate_limit_holds(&self) -> Arc<RateLimitHolds> {
        Arc::clone(&self.rate_limit_holds)
    }

    fn refresh_rate_limit_scope(&self, now: Instant) {
        let init = if matches!(
            self.init_state(self.expected_next_media_sequence),
            InitState::Gated
        ) {
            self.reorder_buffer
                .get(&self.expected_next_media_sequence)
                .and_then(|buffered| buffered.payload.descriptor().init_key.as_ref())
        } else {
            None
        };
        self.rate_limit_holds.set_scope(
            self.expected_next_media_sequence,
            init,
            self.reorder_buffer.is_empty() && !self.has_emitted_media_segment,
            now,
        );
    }

    fn publish_cursor(&self) {
        if let Some(cursor) = &self.cursor {
            let next = self.expected_next_media_sequence;
            cursor.send_if_modified(|published| {
                let advanced = *published != next;
                *published = next;
                advanced
            });
        }
    }

    /// Init-readiness of the buffered media segment at `msn`.
    fn init_state(&self, msn: u64) -> InitState {
        let Some(buffered) = self.reorder_buffer.get(&msn) else {
            return InitState::Ready;
        };
        if !matches!(buffered.payload, SegmentPayload::Mp4Media { .. }) {
            // TS media repeats its PAT/PMT, so it decodes without its map and
            // is never held for it; the map is still emitted first when it has
            // arrived (see emit_payload).
            return InitState::Ready;
        }
        match buffered.payload.descriptor().init_key.as_ref() {
            Some(key) => {
                if self.active_init_key.as_ref() == Some(key) || self.inits.contains(key) {
                    InitState::Ready
                } else if self.failed_inits.get(key).is_some() || self.no_more_inits {
                    InitState::Failed
                } else {
                    InitState::Gated
                }
            }
            // fMP4 media with no init_key tag: fall back to the coarse
            // "any init seen" gate so a malformed descriptor cannot emit
            // media before the stream's first init, unless the reactor already
            // confirmed no more inits can arrive.
            None => {
                if self.active_init_key.is_some()
                    || self
                        .latest_init_key
                        .as_ref()
                        .is_some_and(|key| self.inits.contains(key))
                {
                    InitState::Ready
                } else if self.no_more_inits {
                    InitState::Failed
                } else {
                    InitState::Gated
                }
            }
        }
    }

    /// Assembles until the stream ends, fails or is cancelled. Returns the
    /// reason when the stream ended with an error (see [`AssemblerExit`]).
    pub async fn run(mut self) -> AssemblerExit {
        self.assemble().await;
        self.rate_limit_holds.clear_scope(Instant::now());
        match self.failure {
            Some(reason) => AssemblerExit::Failed(reason),
            None => AssemblerExit::Done,
        }
    }

    /// When a live stream that receives no input fails; `None` when it never
    /// does. A zero duration disables the deadline, as `None` does.
    fn stall_deadline(&self, clock: &ThrottleClock, now: Instant) -> Option<Instant> {
        let timeout = self
            .config
            .output
            .live_max_overall_stall_duration
            .filter(|timeout| !timeout.is_zero())?;
        let since = self.last_input_received_time?;
        self.is_live_stream
            .then(|| clock.deadline(since, self.last_input_paused, timeout, now))
    }

    async fn assemble(&mut self) {
        debug!(live = self.is_live_stream, "sequence assembler started");

        let gap_evaluation_interval = {
            let interval = self.config.output.gap_evaluation_interval;
            if interval.is_zero() {
                std::time::Duration::from_millis(1)
            } else {
                interval
            }
        };

        let mut hold_changes = self.rate_limit_holds.subscribe();
        loop {
            let now = Instant::now();
            self.refresh_rate_limit_scope(now);
            let clock = self.rate_limit_holds.clock();
            let stall_deadline = self.stall_deadline(&clock, now);
            let overall_stall_timeout = stall_deadline
                .map_or(Duration::from_secs(u64::MAX / 2), |deadline| {
                    deadline.saturating_duration_since(now)
                });

            tokio::select! {
                biased;

                _ = self.cancel.cancelled() => {
                    debug!("assembler cancelled; dropping reorder buffer without emission");
                    return;
                }

                changed = hold_changes.changed() => {
                    if changed.is_ok()
                        && matches!(self.try_emit().await, EmitOutcome::Finished)
                    {
                        return;
                    }
                }

                input = self.input_rx.recv() => {
                    let counts_as_liveness = matches!(
                        input.as_ref(),
                        Some(AssemblerInput::Payload(_))
                            | Some(AssemblerInput::Filtered { .. })
                            | Some(AssemblerInput::Notice(EngineNotice::TrackAbsent { .. }))
                    );
                    if self.is_live_stream && counts_as_liveness {
                        let now = Instant::now();
                        self.last_input_received_time = Some(now);
                        self.last_input_paused = self.rate_limit_holds.clock().paused_at(now);
                    }
                    match input {
                        Some(AssemblerInput::Payload(payload)) => {
                            if matches!(self.handle_payload(payload).await, EmitOutcome::Finished) {
                                return;
                            }
                        }
                        Some(
                            AssemblerInput::Skipped { from_msn, to_msn }
                            | AssemblerInput::Filtered { from_msn, to_msn },
                        ) => {
                            trace!(from_msn, to_msn, "upstream skip range");
                            self.rate_limit_holds.skip(from_msn, to_msn, Instant::now());
                            self.dead.insert(from_msn, to_msn);
                            if matches!(self.try_emit().await, EmitOutcome::Finished) {
                                return;
                            }
                        }
                        Some(AssemblerInput::TerminalFailed {
                            key,
                            msn,
                            error,
                            init_failed,
                        }) => {
                            self.rate_limit_holds.release(&key, msn, Instant::now());
                            if key.kind == SegmentKind::Init {
                                warn!(msn, "init segment terminally failed; dependent media will be skipped");
                                self.failed_inits.insert(key, error.clone());
                                self.init_failure.get_or_insert(error);
                            } else {
                                self.dead.insert(msn, msn);
                                if init_failed {
                                    // Abort live startup immediately only when a declared
                                    // initialization segment failed to fetch; unmapped fMP4
                                    // segments on a mid-window live join advance as a gap so
                                    // a following `#EXT-X-MAP` can still start the recording.
                                    if self.is_live_stream
                                        && !self.has_emitted_media_segment
                                        && !self.failed_inits.is_empty()
                                    {
                                        self.undecodable.insert(msn, error.clone());
                                    }
                                    self.init_failure.get_or_insert(error);
                                } else {
                                    self.media_failure.get_or_insert(error);
                                }
                            }
                            if matches!(self.try_emit().await, EmitOutcome::Finished) {
                                return;
                            }
                        }
                        Some(AssemblerInput::Notice(notice)) => {
                            if self.sink.send(Ok(AssembledItem::Notice(notice))).await.is_err() {
                                return;
                            }
                        }
                        Some(AssemblerInput::NoMoreInits) => {
                            self.no_more_inits = true;
                            if matches!(self.try_emit().await, EmitOutcome::Finished) {
                                return;
                            }
                        }
                        Some(AssemblerInput::Fatal(err)) => {
                            warn!(error = %crate::redact::redact_diagnostic(&err), "fatal pipeline error; dropping reorder buffer");
                            self.fail(err).await;
                            return;
                        }
                        Some(AssemblerInput::End) => {
                            if self.flush_in_order().await.is_err() {
                                error!("consumer channel closed during end-of-stream drain");
                                return;
                            }
                            if !self.has_emitted_media_segment
                                && let Some(error) =
                                    self.init_failure.take().or(self.media_failure.take())
                            {
                                self.fail_with_segments(error.to_error()).await;
                                return;
                            }
                            self.finish_summaries();
                            if self
                                .sink
                                .send(Ok(AssembledItem::StreamEnded))
                                .await
                                .is_err()
                            {
                                debug!("consumer closed before receiving end-of-stream event");
                            }
                            return;
                        }
                        None => {
                            debug!("assembler input closed without End; dropping buffer");
                            return;
                        }
                    }
                }

                _ = sleep(overall_stall_timeout), if stall_deadline.is_some() => {
                    let now = Instant::now();
                    if self
                        .stall_deadline(&self.rate_limit_holds.clock(), now)
                        .is_some_and(|deadline| deadline > now)
                    {
                        continue;
                    }
                    error!("live stream stalled beyond its recovery deadline with no input");
                    self.fail(DownloadError::Timeout {
                        reason: "Stalled: No input received for max duration.".to_string(),
                    }).await;
                    return;
                }

                _ = sleep(gap_evaluation_interval), if self.gap_state.is_some() || clock.active_until().is_some() => {
                    if matches!(self.try_emit().await, EmitOutcome::Finished) {
                        return;
                    }
                }
            }
        }
    }

    async fn handle_payload(&mut self, payload: SegmentPayload) -> EmitOutcome {
        let msn = payload.msn();
        self.rate_limit_holds
            .release(&payload.descriptor().key, msn, Instant::now());

        if payload.is_fmp4() {
            self.is_fmp4_stream = true;
        }

        if payload.is_init() {
            self.latest_init_key = Some(payload.descriptor().key.clone());
            let cursor = self.expected_next_media_sequence;
            let last_emitted = self.last_emitted_media_msn;
            let active_init = &self.active_init_key;
            let reorder_buffer = &self.reorder_buffer;
            self.inits.insert(payload, |key, entry| {
                Some(key) == active_init.as_ref()
                    || reorder_buffer
                        .values()
                        .any(|b| b.payload.descriptor().init_key.as_ref() == Some(key))
                    || entry.payload.msn() >= cursor
                    || (!entry.emitted
                        && last_emitted.is_none_or(|emitted| entry.payload.msn() > emitted))
            });
            // An init can unblock already-buffered media gated on its key.
            return self.try_emit().await;
        }

        self.metrics
            .segments_received
            .fetch_add(1, Ordering::Relaxed);

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
        let is_new_msn = match self.reorder_buffer.insert(msn, buffered) {
            Some(replaced) => {
                self.current_buffer_bytes = self
                    .current_buffer_bytes
                    .saturating_sub(replaced.size_bytes);
                false
            }
            None => true,
        };

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

    async fn try_emit(&mut self) -> EmitOutcome {
        self.refresh_rate_limit_scope(Instant::now());
        loop {
            if let Some(run_end) = self.dead.run_end(self.expected_next_media_sequence) {
                let from = self.expected_next_media_sequence;
                let Some(to) = run_end.checked_add(1) else {
                    self.abort_u64_max_cursor().await;
                    return EmitOutcome::Finished;
                };
                if let Some(error) = self
                    .undecodable
                    .range(from..=run_end)
                    .next()
                    .map(|(_, error)| error.to_error())
                {
                    self.fail_with_segments(error).await;
                    return EmitOutcome::Finished;
                }
                debug!(from, to, "advancing past upstream-skipped MSNs");
                if self
                    .emit_gap_skipped(from, to, GapSkipReason::Upstream)
                    .await
                    .is_err()
                {
                    return EmitOutcome::Finished;
                }
                self.advance_to(to);
                self.undecodable = self.undecodable.split_off(&to);
                continue;
            }

            let Some(first_msn) = self.reorder_buffer.keys().next().copied() else {
                break;
            };

            if first_msn == self.expected_next_media_sequence {
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
                                return EmitOutcome::Finished;
                            }
                            continue;
                        }
                        break;
                    }
                    InitState::Failed => {
                        let error = self.reorder_buffer.get(&first_msn).map(|buffered| {
                            let protocol = buffered.payload.descriptor().meta.protocol();
                            buffered
                                .payload
                                .descriptor()
                                .init_key
                                .as_ref()
                                .and_then(|key| self.failed_inits.get(key))
                                .cloned()
                                .unwrap_or_else(|| {
                                    SharedError::new(DownloadError::InvalidContent {
                                        protocol,
                                        reason: "fMP4 media segment missing initialization segment"
                                            .to_owned(),
                                    })
                                })
                        });
                        if let Some(err) = &error {
                            self.init_failure.get_or_insert_with(|| err.clone());
                        }
                        if self.is_live_stream
                            && !self.has_emitted_media_segment
                            && let Some(error) = error.map(|e| e.to_error())
                        {
                            self.fail_with_segments(error).await;
                            return EmitOutcome::Finished;
                        }
                        warn!(
                            msn = first_msn,
                            "skipping media whose init segment terminally failed"
                        );
                        if self
                            .skip_buffered_media_as_gap(first_msn, GapSkipReason::Upstream, true)
                            .await
                            .is_err()
                        {
                            return EmitOutcome::Finished;
                        }
                        continue;
                    }
                }

                let Some(buffered) = self.remove_buffered_payload(first_msn) else {
                    break;
                };
                trace!(
                    msn = first_msn,
                    buffered_ms = buffered.buffered_at.elapsed().as_millis() as u64,
                    "emitting segment"
                );

                if self.emit_payload(buffered.payload).await.is_err() {
                    return EmitOutcome::Finished;
                }
                self.advance_to(first_msn.saturating_add(1));
            } else if first_msn < self.expected_next_media_sequence {
                if self.remove_buffered_payload(first_msn).is_some() {
                    self.metrics
                        .segments_rejected_stale
                        .fetch_add(1, Ordering::Relaxed);
                }
            } else {
                let is_new_gap = self
                    .gap_state
                    .as_ref()
                    .is_none_or(|g| g.missing_sequence != self.expected_next_media_sequence);
                if is_new_gap {
                    self.metrics.gaps_detected.fetch_add(1, Ordering::Relaxed);
                    let mut gap = GapState::new(
                        self.expected_next_media_sequence,
                        self.rate_limit_holds.clock().paused_at(Instant::now()),
                    );
                    gap.segments_since_gap = self
                        .reorder_buffer
                        .range(self.expected_next_media_sequence.saturating_add(1)..)
                        .count() as u64;
                    self.gap_state = Some(gap);
                }

                if !self.is_live_stream
                    && let Some(vod_timeout) = self.config.output.vod_segment_timeout
                    && let Some(gap_state) = self.gap_state.as_ref()
                {
                    let now = Instant::now();
                    let elapsed = self.gap_elapsed(gap_state, now);
                    if elapsed >= vod_timeout {
                        let to = self
                            .rate_limit_holds
                            .first_media_hold(self.expected_next_media_sequence, first_msn, now)
                            .map_or(first_msn, |(msn, _)| msn);
                        if to == self.expected_next_media_sequence {
                            break;
                        }
                        warn!(
                            msn = gap_state.missing_sequence,
                            ?elapsed,
                            "VOD segment timed out; skipping"
                        );
                        if self
                            .sink
                            .send(Ok(AssembledItem::SegmentTimeout {
                                sequence_number: gap_state.missing_sequence,
                                waited_duration: elapsed,
                            }))
                            .await
                            .is_err()
                        {
                            return EmitOutcome::Finished;
                        }
                        self.record_gap_skip(to);
                        self.advance_to(to);
                        continue;
                    }
                }

                if let Some(reason) = self.should_skip_gap() {
                    let to = if matches!(reason, GapSkipReason::BufferPressure) {
                        first_msn
                    } else {
                        self.rate_limit_holds
                            .first_media_hold(
                                self.expected_next_media_sequence,
                                first_msn,
                                Instant::now(),
                            )
                            .map_or(first_msn, |(msn, _)| msn)
                    };
                    if to == self.expected_next_media_sequence {
                        break;
                    }
                    warn!(
                        from = self.expected_next_media_sequence,
                        to,
                        ?reason,
                        "gap confirmed; skipping"
                    );
                    if self
                        .emit_gap_skipped(self.expected_next_media_sequence, to, reason)
                        .await
                        .is_err()
                    {
                        return EmitOutcome::Finished;
                    }
                    self.advance_to(to);
                    continue;
                }

                break;
            }
        }

        debug_assert!(
            self.reorder_buffer
                .first_key_value()
                .is_none_or(|(&msn, _)| msn >= self.expected_next_media_sequence),
            "a buffered entry lies below the emit cursor"
        );
        self.publish_cursor();
        EmitOutcome::Continue
    }

    /// Moves the emit cursor to `to`: everything below it was emitted or
    /// given up on.
    fn advance_to(&mut self, to: u64) {
        if to > self.expected_next_media_sequence {
            self.rate_limit_holds
                .skip(self.expected_next_media_sequence, to - 1, Instant::now());
        }
        self.expected_next_media_sequence = to;
        self.gap_state = None;
        self.dead.prune_below(to);
        self.refresh_rate_limit_scope(Instant::now());
    }

    fn remove_buffered_payload(&mut self, msn: u64) -> Option<BufferedPayload> {
        let buffered = self.reorder_buffer.remove(&msn)?;
        self.current_buffer_bytes = self
            .current_buffer_bytes
            .saturating_sub(buffered.size_bytes);
        Some(buffered)
    }

    async fn abort_u64_max_cursor(&mut self) {
        error!("media sequence reached u64::MAX; aborting stream");
        self.fail(DownloadError::Playlist {
            reason: "media sequence reached u64::MAX".to_string(),
        })
        .await;
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
                .sink
                .send(Ok(AssembledItem::Discontinuity))
                .await
                .is_err()
            {
                return Err(());
            }
        }

        let init_key = payload.descriptor().init_key.clone();
        if payload.is_fmp4() || init_key.is_some() {
            self.emit_governing_init_segment(init_key.as_ref(), discontinuity)
                .await?;
        }

        let is_media = !payload.is_init();
        if self
            .sink
            .send(Ok(AssembledItem::Payload(payload)))
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
            self.last_emitted_media_msn = Some(
                self.last_emitted_media_msn
                    .map_or(msn, |prev| prev.max(msn)),
            );
            self.undecodable.clear();
        }
        Ok(())
    }

    /// Emit the init governing the media about to be emitted, unless it is
    /// already the active one. Media without an `init_key` uses the latest
    /// received init if none is active yet.
    async fn emit_governing_init_segment(
        &mut self,
        init_key: Option<&SegmentKey>,
        discontinuity_already_emitted: bool,
    ) -> Result<(), ()> {
        let Some(key) = init_key
            .or(self.active_init_key.as_ref())
            .or(self.latest_init_key.as_ref())
            .cloned()
        else {
            return Ok(());
        };
        if self.active_init_key.as_ref() == Some(&key) {
            self.inits.touch(&key);
            return Ok(());
        }
        let Some((init_payload, first_emission)) = self.inits.take_for_emission(&key) else {
            return Ok(());
        };
        self.active_init_key = Some(key);

        if first_emission
            && init_payload.discontinuity()
            && !discontinuity_already_emitted
            && self
                .sink
                .send(Ok(AssembledItem::Discontinuity))
                .await
                .is_err()
        {
            return Err(());
        }

        if self
            .sink
            .send(Ok(AssembledItem::Payload(init_payload)))
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
                InitState::Gated | InitState::Failed => {
                    if let Some(buffered) = self.reorder_buffer.get(&msn) {
                        let protocol = buffered.payload.descriptor().meta.protocol();
                        let fallback = buffered
                            .payload
                            .descriptor()
                            .init_key
                            .as_ref()
                            .and_then(|key| self.failed_inits.get(key))
                            .cloned()
                            .unwrap_or_else(|| {
                                SharedError::new(DownloadError::InvalidContent {
                                    protocol,
                                    reason: "fMP4 media segment missing initialization segment"
                                        .to_owned(),
                                })
                            });
                        self.init_failure.get_or_insert(fallback);
                    }
                    warn!(
                        msn,
                        "skipping fMP4 media during flush: governing init absent or failed"
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

    async fn fail(&mut self, error: DownloadError) {
        let reason = Arc::from(error.to_string());
        match self.sink.send(Err(error)).await {
            Ok(()) => self.failure = Some(reason),
            Err(_) => debug!("consumer closed before receiving the stream's error"),
        }
        self.finish_summaries();
    }

    async fn fail_with_segments(&mut self, error: DownloadError) {
        warn!(error = %crate::redact::redact_diagnostic(&error), "segment stream failed");
        self.fail(error).await;
    }

    fn finish_summaries(&self) {
        self.metrics.log_summary();
        if let Some((perf, protocol)) = &self.performance_metrics {
            perf.log_summary(protocol);
        }
    }
}
