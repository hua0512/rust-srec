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

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::DownloadError;
use crate::error::SharedError;

use super::config::{GapSkipStrategy, SegmentCoreConfig};
use super::control::EngineNotice;
use super::identity::{SegmentKey, SegmentKind};
use super::input::AssemblerInput;
use super::metrics::PerformanceMetrics;
use super::payload::SegmentPayload;
use super::throttle::{RateLimitHolds, ThrottleClock};

// --- Output ---

/// Reason why a gap was skipped in the reorder buffer
#[derive(Debug, Clone)]
pub enum GapSkipReason {
    /// Gap skipped because count threshold was exceeded
    CountThreshold(u64),
    /// Gap skipped because duration threshold was exceeded
    DurationThreshold(Duration),
    /// Gap skipped because both count and duration thresholds were exceeded
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

/// Bounded insertion-ordered map of failed init keys to their failure. Init
/// rotations are infrequent (discontinuities / rendition changes), so a small
/// cap with FIFO eviction keeps memory bounded on long-running streams.
/// Eviction is safe because an unknown key is treated as unresolved: media
/// still waits in the normal case, and the live buffer-pressure / ENDLIST
/// flush fail-safes surface stuck media as visible gaps.
#[derive(Debug)]
struct FailedInits {
    cap: usize,
    order: std::collections::VecDeque<SegmentKey>,
    errors: HashMap<SegmentKey, SharedError>,
}

impl FailedInits {
    fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            order: std::collections::VecDeque::new(),
            errors: HashMap::new(),
        }
    }

    fn insert(&mut self, key: SegmentKey, error: SharedError) {
        if self.errors.insert(key.clone(), error).is_none() {
            self.order.push_back(key);
            while self.order.len() > self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.errors.remove(&oldest);
                }
            }
        }
    }

    fn get(&self, key: &SegmentKey) -> Option<&SharedError> {
        self.errors.get(key)
    }
}

/// Received init payloads by key, retained so an init can be emitted — again,
/// if the stream switches back to it — whenever media at the cursor needs it.
/// The store never re-downloads a completed init, so this is the only copy.
/// Bounded least-recently-used; an evicted key is unresolved again, with the
/// same fail-safes as [`FailedInits`]. An init that has not been emitted yet
/// and may govern media at or beyond the cursor is never evicted: a finished
/// stream fetches its inits ahead of their media, and evicting one would leave
/// that media waiting for a map that is never fetched again.
#[derive(Debug)]
struct RetainedInits {
    /// `0` disables the limit.
    cap: usize,
    /// Logical clock for recency: touching an entry is O(1) on every media
    /// emission; only an over-cap insert scans for the least recent entry.
    clock: u64,
    entries: HashMap<SegmentKey, RetainedInit>,
}

#[derive(Debug)]
struct RetainedInit {
    payload: SegmentPayload,
    emitted: bool,
    last_used: u64,
}

impl RetainedInits {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// `cursor` is the next media sequence number the stream emits.
    fn insert(&mut self, payload: SegmentPayload, cursor: u64) {
        let key = payload.descriptor().key.clone();
        let now = self.tick();
        match self.entries.get_mut(&key) {
            Some(entry) => {
                entry.payload = payload;
                entry.last_used = now;
            }
            None => {
                self.entries.insert(
                    key,
                    RetainedInit {
                        payload,
                        emitted: false,
                        last_used: now,
                    },
                );
            }
        }
        if self.cap > 0
            && self.entries.len() > self.cap
            && let Some(oldest) = self
                .entries
                .iter()
                .filter(|(_, entry)| entry.emitted || entry.payload.msn() < cursor)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    fn contains(&self, key: &SegmentKey) -> bool {
        self.entries.contains_key(key)
    }

    /// A copy of the payload to emit, and whether this is its first emission.
    fn take_for_emission(&mut self, key: &SegmentKey) -> Option<(SegmentPayload, bool)> {
        let now = self.tick();
        let entry = self.entries.get_mut(key)?;
        let first = !entry.emitted;
        entry.emitted = true;
        entry.last_used = now;
        Some((entry.payload.clone(), first))
    }

    /// Mark `key` as used; a no-op if it is not retained.
    fn touch(&mut self, key: &SegmentKey) {
        let now = self.tick();
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_used = now;
        }
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
    paused_at_detection: Duration,
    segments_since_gap: u64,
}

impl GapState {
    fn new(missing_sequence: u64, paused_at_detection: Duration) -> Self {
        Self {
            missing_sequence,
            detected_at: Instant::now(),
            paused_at_detection,
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
    /// The governing init terminally failed; the media is undecodable.
    Failed,
}

pub struct SequenceAssembler<S> {
    config: Arc<SegmentCoreConfig>,
    input_rx: mpsc::Receiver<AssemblerInput>,
    sink: S,
    reorder_buffer: BTreeMap<u64, BufferedPayload>,
    /// fMP4 init payloads that have arrived, kept out of `reorder_buffer`
    /// because an init is not part of the media sequence. A media segment
    /// whose `descriptor.init_key` is retained here (or is the active init)
    /// may be emitted; one whose init is not here is gated (not just the first
    /// init — every rotation), so a rotated init cannot lose the race against
    /// the first media it covers. If an eviction makes a key unknown again
    /// while dependent media is still buffered, the live buffer-pressure and
    /// ENDLIST flush paths surface that media as a visible gap instead of
    /// stalling or emitting it under the wrong init.
    inits: RetainedInits,
    /// The init most recently emitted: the one consumers decode media with.
    active_init_key: Option<SegmentKey>,
    /// The init most recently received, for fMP4 media without an `init_key`.
    latest_init_key: Option<SegmentKey>,
    is_fmp4_stream: bool,
    /// Init keys that terminally failed, with why. Media depending on a
    /// failed init can never be decoded, so it is skipped (a visible gap)
    /// rather than gating the stream forever. This is bounded to keep memory
    /// flat on long streams; eviction is fail-safe at the cursor and on
    /// flush.
    failed_inits: FailedInits,
    /// The first terminal failure of an init (or of media for want of its
    /// init) and of a media segment. A stream that ends without emitting any
    /// media ends with one of them, the init's first, instead of a clean end.
    init_failure: Option<SharedError>,
    media_failure: Option<SharedError>,
    /// Live streams, before any media is emitted: media that failed because
    /// its init did, by sequence number. A live stream does not end, so one
    /// that would start with such media fails with its init's error rather
    /// than skip media indefinitely.
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

    fn gap_strategy(&self) -> &GapSkipStrategy {
        if self.is_live_stream {
            &self.config.output.live_gap_strategy
        } else {
            &self.config.output.vod_gap_strategy
        }
    }

    /// At-cap detection for the forced-skip rule: when the reorder buffer is
    /// over its byte/segment limit and blocked on a gap, the gap policy's
    /// count/duration thresholds are bypassed.
    fn buffer_at_limit(&self) -> bool {
        let limits = &self.config.output.buffer_limits;
        (limits.max_segments > 0 && self.reorder_buffer.len() >= limits.max_segments)
            || (limits.max_bytes > 0 && self.current_buffer_bytes >= limits.max_bytes)
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
                } else if self.failed_inits.get(key).is_some() {
                    InitState::Failed
                } else {
                    InitState::Gated
                }
            }
            // fMP4 media with no init_key tag: fall back to the coarse
            // "any init seen" gate so a malformed descriptor cannot emit
            // media before the stream's first init.
            None => {
                if self.active_init_key.is_some()
                    || self
                        .latest_init_key
                        .as_ref()
                        .is_some_and(|key| self.inits.contains(key))
                {
                    InitState::Ready
                } else {
                    InitState::Gated
                }
            }
        }
    }

    /// Time a gap has waited, excluding the credited rate-limit pause
    /// since it was detected.
    fn gap_elapsed(&self, gap_state: &GapState, now: Instant) -> Duration {
        let paused = self
            .rate_limit_holds
            .clock()
            .credited_pause(gap_state.paused_at_detection, now);
        now.saturating_duration_since(gap_state.detected_at)
            .saturating_sub(paused)
    }

    fn should_skip_gap(&self) -> Option<GapSkipReason> {
        let gap_state = self.gap_state.as_ref()?;

        // Buffer pressure overrides thresholds: waiting at the cap recreates
        // the deadlock the keep-draining rule exists to prevent.
        //
        // A stream that waits for every segment is exempt: the reactor keeps
        // what is buffered near the limits, and the assembler keeps draining
        // its input either way, so waiting on the missing segment loses
        // nothing. Giving it up would silently drop a segment that is still
        // coming.
        let waits_for_every_segment = !self.is_live_stream
            && matches!(self.gap_strategy(), GapSkipStrategy::WaitIndefinitely);
        if self.buffer_at_limit() && !waits_for_every_segment {
            return Some(GapSkipReason::BufferPressure);
        }

        let in_startup =
            self.is_live_stream && self.is_fmp4_stream && !self.has_emitted_media_segment;
        let elapsed = self.gap_elapsed(gap_state, Instant::now());

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
    /// does.
    fn stall_deadline(&self, clock: &ThrottleClock, now: Instant) -> Option<Instant> {
        let timeout = self.config.output.live_max_overall_stall_duration?;
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
                    if self.is_live_stream {
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
                        Some(AssemblerInput::Skipped { from_msn, to_msn }) => {
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
                                // An init that will never arrive. Any media
                                // depending on it (by init_key) can never be
                                // decoded; record the failure so try_emit
                                // skips those media as a visible gap instead
                                // of gating the stream forever.
                                warn!(msn, "init segment terminally failed; dependent media will be skipped");
                                self.failed_inits.insert(key, error.clone());
                                self.init_failure.get_or_insert(error);
                            } else {
                                self.dead.insert(msn, msn);
                                if init_failed {
                                    if self.is_live_stream && !self.has_emitted_media_segment {
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
                        Some(AssemblerInput::Fatal(err)) => {
                            // Pipeline error: buffered payloads are dropped,
                            // the error is the stream's terminal item.
                            warn!(error = %crate::redact::redact_diagnostic(&err), "fatal pipeline error; dropping reorder buffer");
                            self.fail(err).await;
                            return;
                        }
                        Some(AssemblerInput::End) => {
                            // Authoritative end: ordered drain, then StreamEnded.
                            if self.flush_in_order().await.is_err() {
                                error!("consumer channel closed during end-of-stream drain");
                                return;
                            }
                            // Segments failed and none reached the consumer:
                            // that is the failure, not an empty stream.
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
                            // Channel closed without End or Fatal: cancellation
                            // path. Drop the buffer, emit nothing.
                            debug!("assembler input closed without End; dropping buffer");
                            return;
                        }
                    }
                }

                // Ready recovery payloads and hold updates take precedence
                // over a timer that became due while their tasks were waking.
                _ = sleep(overall_stall_timeout), if stall_deadline.is_some() => {
                    // A producer can publish a pause after the watch arm was
                    // polled but before this ready timer wins the select.
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

                // Periodic gap evaluation so duration-based skipping and VOD
                // timeouts fire even when no new input arrives.
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

        // fMP4 init segments are not part of the media sequence progression;
        // retain them by key and emit one when media referencing it is
        // emitted. Not counted in current_buffer_bytes: they are not media.
        if payload.is_init() {
            self.latest_init_key = Some(payload.descriptor().key.clone());
            self.inits
                .insert(payload, self.expected_next_media_sequence);
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
    /// cursor through dead ranges and applying gap policy.
    ///
    /// Every exit that keeps the stream going leaves the buffer's first
    /// entry at or above the cursor: entries are only buffered at or above
    /// it, and one the cursor passed (a dead range covering it) is removed
    /// as stale before the loop can stop.
    async fn try_emit(&mut self) -> EmitOutcome {
        self.refresh_rate_limit_scope(Instant::now());
        loop {
            // Dead-range fast-forward: the cursor itself can be dead even with
            // an empty buffer (ads, window slide, terminal failure).
            if let Some(run_end) = self.dead.run_end(self.expected_next_media_sequence) {
                let from = self.expected_next_media_sequence;
                // A dead run ending at u64::MAX cannot be advanced past — the
                // cursor would saturate inside the range and re-skip forever.
                // Only a corrupt/hostile playlist reaches MSN u64::MAX; treat
                // it as a fatal stream error rather than spin.
                let Some(to) = run_end.checked_add(1) else {
                    self.abort_u64_max_cursor().await;
                    return EmitOutcome::Finished;
                };
                // A live stream that would start with media its init cannot
                // decode (see `undecodable`).
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
                                return EmitOutcome::Finished;
                            }
                            continue;
                        }
                        break;
                    }
                    InitState::Failed => {
                        // A live stream that would start with media its init
                        // cannot decode (see `undecodable`).
                        if self.is_live_stream
                            && !self.has_emitted_media_segment
                            && let Some(error) = self
                                .reorder_buffer
                                .get(&first_msn)
                                .and_then(|buffered| {
                                    buffered.payload.descriptor().init_key.as_ref()
                                })
                                .and_then(|key| self.failed_inits.get(key))
                                .map(SharedError::to_error)
                        {
                            self.fail_with_segments(error).await;
                            return EmitOutcome::Finished;
                        }
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
                self.advance_to(first_msn + 1);
            } else if first_msn < self.expected_next_media_sequence {
                // Stale entry left behind by a skip.
                if self.remove_buffered_payload(first_msn).is_some() {
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
                    let mut gap = GapState::new(
                        self.expected_next_media_sequence,
                        self.rate_limit_holds.clock().paused_at(Instant::now()),
                    );
                    gap.segments_since_gap = self
                        .reorder_buffer
                        .range(self.expected_next_media_sequence + 1..)
                        .count() as u64;
                    self.gap_state = Some(gap);
                }

                // VOD per-segment timeout.
                if !self.is_live_stream
                    && let Some(vod_timeout) = self.config.output.vod_segment_timeout
                    && let Some(gap_state) = self.gap_state.as_ref()
                {
                    let elapsed = self.gap_elapsed(gap_state, Instant::now());
                    if elapsed >= vod_timeout {
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
                        self.record_gap_skip(first_msn);
                        self.advance_to(first_msn);
                        continue;
                    }
                }

                if let Some(reason) = self.should_skip_gap() {
                    // A skip can span several missing MSNs. Stop before the
                    // first protected one, even when an earlier unprotected
                    // gap has already exceeded its ordinary policy.
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

                // Wait for the missing segment (or a policy trigger).
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

    fn record_gap_skip(&self, to: u64) {
        self.metrics.gap_skips.fetch_add(1, Ordering::Relaxed);
        self.metrics.total_segments_skipped.fetch_add(
            to.saturating_sub(self.expected_next_media_sequence),
            Ordering::Relaxed,
        );
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
        self.sink
            .send(Ok(AssembledItem::GapSkipped {
                from_sequence: from,
                to_sequence: to,
                reason,
            }))
            .await
            .map_err(|_| ())
    }

    async fn abort_u64_max_cursor(&mut self) {
        error!("media sequence reached u64::MAX; aborting stream");
        self.fail(DownloadError::Playlist {
            reason: "media sequence reached u64::MAX".to_string(),
        })
        .await;
    }

    async fn skip_buffered_media_as_gap(
        &mut self,
        msn: u64,
        reason: GapSkipReason,
        advance_cursor: bool,
    ) -> Result<(), ()> {
        let Some(to) = msn.checked_add(1) else {
            self.abort_u64_max_cursor().await;
            return Err(());
        };
        if self.remove_buffered_payload(msn).is_none() {
            debug!(msn, "gap skip requested for missing buffered media");
            return Ok(());
        }
        self.emit_gap_skipped(msn, to, reason).await?;
        if advance_cursor {
            self.advance_to(to);
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
            // Using the active init still counts as an access: otherwise
            // unused prefetched maps can evict it before a switch back.
            self.inits.touch(&key);
            return Ok(());
        }
        // init_state() only reports Ready when this payload is retained.
        let Some((init_payload, first_emission)) = self.inits.take_for_emission(&key) else {
            return Ok(());
        };
        self.active_init_key = Some(key);

        // The init's own discontinuity flag was captured when it was first
        // planned; it only applies to its first appearance.
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

    /// Ends the stream with `error` as its terminal item.
    async fn fail(&mut self, error: DownloadError) {
        let reason = Arc::from(error.to_string());
        match self.sink.send(Err(error)).await {
            Ok(()) => self.failure = Some(reason),
            Err(_) => debug!("consumer closed before receiving the stream's error"),
        }
        self.finish_summaries();
    }

    /// Ends the stream with `error`, a failure of its segments.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::HlsDownloaderError;
    use crate::hls::config::HlsConfig;
    use crate::hls::engine::output::HlsEventSink;
    use crate::hls::events::HlsStreamEvent;
    use crate::segment_engine::descriptor::{SegmentDescriptor, SegmentMeta, SegmentSource};
    use crate::segment_engine::identity::SegmentKey;
    use bytes::Bytes;
    use std::time::Duration;
    use url::Url;

    fn descriptor(uri: &str, msn: u64, kind: SegmentKind) -> Arc<SegmentDescriptor> {
        Arc::new(SegmentDescriptor {
            key: SegmentKey {
                kind,
                uri: Arc::from(uri),
                byte_range: None,
                scope: None,
            },
            msn,
            source: SegmentSource::Playlist,
            parsed_url: Arc::new(Url::parse(uri).unwrap()),
            discontinuity: false,
            encryption: None,
            init_key: None,
            meta: SegmentMeta::Hls(Arc::new(m3u8_rs::MediaSegment {
                uri: uri.to_string(),
                duration: 2.0,
                ..Default::default()
            })),
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
            scope: None,
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
        join: tokio::task::JoinHandle<AssemblerExit>,
        holds: Arc<RateLimitHolds>,
    }

    fn spawn_assembler(mut config: HlsConfig, live: bool, initial_msn: u64) -> Harness {
        config.output_config.live_max_overall_stall_duration = None;
        let (input_tx, input_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let assembler = SequenceAssembler::new(
            Arc::new(SegmentCoreConfig::from(&config)),
            input_rx,
            HlsEventSink::new(event_tx),
            live,
            initial_msn,
            cancel.clone(),
        );
        let holds = assembler.rate_limit_holds();
        let join = tokio::spawn(assembler.run());
        Harness {
            input_tx,
            event_rx,
            cancel,
            join,
            holds,
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
    async fn rate_limited_gaps_do_not_skip_the_cursor_or_an_interior_held_segment() {
        for initial_msn in [99, 100] {
            let (_input_tx, input_rx) = mpsc::channel(16);
            let (event_tx, mut event_rx) = mpsc::channel(16);
            let mut assembler = SequenceAssembler::new(
                Arc::new(SegmentCoreConfig::from(&HlsConfig::default())),
                input_rx,
                HlsEventSink::new(event_tx),
                true,
                initial_msn,
                CancellationToken::new(),
            );
            let held = descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media);
            assembler.rate_limit_holds.hold(
                &held,
                Instant::now() + Duration::from_secs(30),
                Instant::now(),
            );
            let AssemblerInput::Payload(later) = ts_payload(101) else {
                unreachable!()
            };
            assembler.handle_payload(later).await;
            assembler.gap_state.as_mut().unwrap().detected_at =
                Instant::now() - Duration::from_secs(6);
            assembler.try_emit().await;
            assert_eq!(assembler.expected_next_media_sequence, 100);
            if initial_msn == 99 {
                assert!(matches!(
                    event_rx.try_recv(),
                    Ok(Ok(HlsStreamEvent::GapSkipped {
                        from_sequence: 99,
                        to_sequence: 100,
                        ..
                    }))
                ));
            }
            assert!(
                event_rx.try_recv().is_err(),
                "held media must still be pending"
            );

            let AssemblerInput::Payload(recovered) = ts_payload(100) else {
                unreachable!()
            };
            assembler.handle_payload(recovered).await;
            assert_eq!(assembler.expected_next_media_sequence, 102);
            let mut uris = Vec::new();
            while let Ok(Ok(event)) = event_rx.try_recv() {
                match event {
                    HlsStreamEvent::Data(data) => {
                        uris.push(data.media_segment().unwrap().uri.clone());
                    }
                    HlsStreamEvent::GapSkipped { .. } => panic!("recovered media was skipped"),
                    _ => {}
                }
            }
            assert_eq!(uris, ["https://e.com/seg100.ts", "https://e.com/seg101.ts"]);
            assert!(
                assembler
                    .rate_limit_holds
                    .active_until(100, Instant::now())
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn a_vod_segment_timeout_excludes_its_rate_limit_hold() {
        let mut config = HlsConfig::default();
        config.output_config.vod_segment_timeout = Some(Duration::from_secs(5));
        let (_input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let mut assembler = SequenceAssembler::new(
            Arc::new(SegmentCoreConfig::from(&config)),
            input_rx,
            HlsEventSink::new(event_tx),
            false,
            100,
            CancellationToken::new(),
        );
        // A fresh registry lets the hold and its paused time begin in the
        // past, before the assembler's construction-time scope update.
        assembler.rate_limit_holds = Arc::new(RateLimitHolds::new());
        let held_since = Instant::now() - Duration::from_secs(6);
        let held = descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media);
        assembler
            .rate_limit_holds
            .set_scope(100, None, false, held_since);
        assembler
            .rate_limit_holds
            .hold(&held, held_since + Duration::from_secs(60), held_since);
        let AssemblerInput::Payload(later) = ts_payload(101) else {
            unreachable!()
        };
        assembler.handle_payload(later).await;

        // Held since the gap opened six seconds ago: none of it counts.
        let gap = assembler.gap_state.as_mut().unwrap();
        gap.detected_at = held_since;
        gap.paused_at_detection = Duration::ZERO;
        assembler.try_emit().await;
        assert_eq!(assembler.expected_next_media_sequence, 100);
        assert!(
            event_rx.try_recv().is_err(),
            "held media must still be pending"
        );

        // Once the hold ends, the gap's unpaused wait reaches the timeout.
        assembler
            .rate_limit_holds
            .release(&held.key, 100, Instant::now());
        assembler.gap_state.as_mut().unwrap().detected_at = held_since - Duration::from_secs(5);
        assembler.try_emit().await;
        assert!(matches!(
            event_rx.try_recv(),
            Ok(Ok(HlsStreamEvent::SegmentTimeout {
                sequence_number: 100,
                ..
            }))
        ));
        assert_eq!(assembler.expected_next_media_sequence, 102);
    }

    #[tokio::test]
    async fn init_rate_limit_holds_cover_startup_and_only_the_required_buffered_init() {
        let (_input_tx, input_rx) = mpsc::channel(16);
        let (event_tx, _event_rx) = mpsc::channel(16);
        let mut assembler = SequenceAssembler::new(
            Arc::new(SegmentCoreConfig::from(&HlsConfig::default())),
            input_rx,
            HlsEventSink::new(event_tx),
            true,
            100,
            CancellationToken::new(),
        );
        let unused = descriptor("https://e.com/unused.mp4", 99, SegmentKind::Init);
        let needed = descriptor("https://e.com/current.mp4", 100, SegmentKind::Init);
        let now = Instant::now();
        let until = now + Duration::from_secs(30);
        assembler.rate_limit_holds.hold(&unused, until, now);
        assembler.refresh_rate_limit_scope(now);
        assert_eq!(
            assembler.rate_limit_holds.clock().active_until(),
            Some(until)
        );
        assembler.has_emitted_media_segment = true;
        assembler.refresh_rate_limit_scope(now);
        assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);

        let AssemblerInput::Payload(payload) = mp4_media_keyed(100, &needed.key) else {
            unreachable!()
        };
        assembler.handle_payload(payload).await;
        assembler.refresh_rate_limit_scope(now);
        assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);
        assembler.rate_limit_holds.hold(&needed, until, now);
        assembler.refresh_rate_limit_scope(now);
        assert_eq!(
            assembler.rate_limit_holds.clock().active_until(),
            Some(until)
        );
        assembler.refresh_rate_limit_scope(until);
        assert_eq!(assembler.rate_limit_holds.clock().active_until(), None);
    }

    #[tokio::test]
    async fn a_live_rate_limit_hold_defers_the_watchdog_but_not_cancellation_or_expiry() {
        for expired in [false, true] {
            let mut config = HlsConfig::default();
            config.output_config.live_max_overall_stall_duration = Some(Duration::from_millis(1));
            let (_input_tx, input_rx) = mpsc::channel(16);
            let (event_tx, mut event_rx) = mpsc::channel(16);
            let cancel = CancellationToken::new();
            let assembler = SequenceAssembler::new(
                Arc::new(SegmentCoreConfig::from(&config)),
                input_rx,
                HlsEventSink::new(event_tx),
                true,
                100,
                cancel.clone(),
            );
            let now = Instant::now();
            assembler.rate_limit_holds.hold(
                &descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media),
                if expired {
                    now
                } else {
                    now + Duration::from_secs(30)
                },
                now,
            );
            let join = tokio::spawn(assembler.run());
            if expired {
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(2), event_rx.recv())
                        .await
                        .unwrap(),
                    Some(Err(HlsDownloaderError::Timeout { .. }))
                ));
            } else {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), event_rx.recv())
                        .await
                        .is_err()
                );
                cancel.cancel();
                assert!(event_rx.recv().await.is_none());
            }
            tokio::time::timeout(Duration::from_secs(2), join)
                .await
                .unwrap()
                .unwrap();
        }
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
            .send(terminal_failure(
                SegmentKey {
                    kind: SegmentKind::Media,
                    uri: Arc::from("https://e.com/seg100.ts"),
                    byte_range: None,
                    scope: None,
                },
                100,
            ))
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
        h.holds.hold(
            &descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media),
            Instant::now() + Duration::from_secs(30),
            Instant::now(),
        );

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
    async fn a_waiting_vod_stream_keeps_waiting_at_the_buffer_limit() {
        let mut config = HlsConfig::default();
        config.output_config.vod_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        config.output_config.buffer_limits.max_segments = 2;
        config.output_config.buffer_limits.max_bytes = 0;
        let mut h = spawn_assembler(config, false, 100);

        h.input_tx.send(ts_payload(101)).await.unwrap();
        h.input_tx.send(ts_payload(102)).await.unwrap();
        h.input_tx.send(ts_payload(103)).await.unwrap();
        // Segment 100 is late: nothing may be emitted or skipped meanwhile.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), h.event_rx.recv())
                .await
                .is_err(),
            "the stream must wait for segment 100"
        );

        h.input_tx.send(ts_payload(100)).await.unwrap();
        let mut uris = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while uris.len() < 4 {
                match h.event_rx.recv().await {
                    Some(Ok(HlsStreamEvent::Data(d))) => {
                        uris.push(d.media_segment().map(|s| s.uri.clone()).unwrap_or_default());
                    }
                    Some(Ok(HlsStreamEvent::GapSkipped { .. })) => {
                        panic!("segment 100 must not be given up on")
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .expect("timed out waiting for the stream to resume");
        assert_eq!(
            uris,
            [
                "https://e.com/seg100.ts",
                "https://e.com/seg101.ts",
                "https://e.com/seg102.ts",
                "https://e.com/seg103.ts",
            ]
        );
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn buffered_entries_the_cursor_passes_are_dropped_at_once() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);

        // 101 and 102 wait behind 100; an upstream skip through 101 makes
        // the buffered 101 stale.
        h.input_tx.send(ts_payload(102)).await.unwrap();
        h.input_tx.send(ts_payload(101)).await.unwrap();
        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 100,
                to_msn: 101,
            })
            .await
            .unwrap();
        let uris = collect_data_uris(&mut h, 1).await;
        assert_eq!(uris, ["https://e.com/seg102.ts"]);

        // 104 is buffered under one URI while another copy of it fails;
        // once 103 is skipped, the cursor passes the dead 104 as well.
        h.input_tx.send(ts_payload(104)).await.unwrap();
        h.input_tx
            .send(terminal_failure(
                SegmentKey {
                    kind: SegmentKind::Media,
                    uri: Arc::from("https://other.example.com/seg104.ts"),
                    byte_range: None,
                    scope: None,
                },
                104,
            ))
            .await
            .unwrap();
        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 103,
                to_msn: 103,
            })
            .await
            .unwrap();
        h.input_tx.send(ts_payload(105)).await.unwrap();
        let uris = collect_data_uris(&mut h, 1).await;
        assert_eq!(uris, ["https://e.com/seg105.ts"]);
        h.cancel.cancel();
        // The assembler checks after every emission pass that nothing it
        // buffered lies below the cursor.
        assert!(h.join.await.is_ok());
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
            .send(AssemblerInput::Notice(EngineNotice::End))
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
    async fn ts_map_section_precedes_ts_media_as_ts_data() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let key = init_key("https://e.com/pat-pmt.ts");
        h.input_tx
            .send(AssemblerInput::Payload(SegmentPayload::TsInit {
                data: Bytes::from_static(&[0x47, 0x40, 0x00, 0x10]),
                descriptor: descriptor("https://e.com/pat-pmt.ts", 100, SegmentKind::Init),
            }))
            .await
            .unwrap();
        let mut media = (*descriptor("https://e.com/seg100.ts", 100, SegmentKind::Media)).clone();
        media.init_key = Some(key);
        h.input_tx
            .send(AssemblerInput::Payload(SegmentPayload::Ts {
                data: Bytes::from_static(&[0x47, 0x01, 0x00, 0x10]),
                descriptor: Arc::new(media),
            }))
            .await
            .unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();

        let mut emitted = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = h.event_rx.recv().await {
                match event {
                    Ok(HlsStreamEvent::Data(data)) => emitted.push((
                        data.media_segment()
                            .map(|s| s.uri.clone())
                            .unwrap_or_default(),
                        data.segment_type(),
                    )),
                    Ok(HlsStreamEvent::StreamEnded) => break,
                    _ => {}
                }
            }
        })
        .await
        .expect("stream ends");
        assert_eq!(
            emitted,
            [
                ("https://e.com/pat-pmt.ts".to_string(), hls::SegmentType::Ts),
                ("https://e.com/seg100.ts".to_string(), hls::SegmentType::Ts),
            ]
        );
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn switching_back_to_an_earlier_init_re_emits_it() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
        let (init_b, key_b) = mp4_init_keyed("https://e.com/b.mp4", 101);
        h.input_tx.send(init_a).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
        h.input_tx.send(init_b).await.unwrap();
        h.input_tx.send(mp4_media_keyed(101, &key_b)).await.unwrap();
        // The store never re-downloads init A; its media must still be
        // preceded by it again, not decoded under B.
        h.input_tx.send(mp4_media_keyed(102, &key_a)).await.unwrap();
        h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            [
                "https://e.com/a.mp4",
                "https://e.com/seg100.m4s",
                "https://e.com/b.mp4",
                "https://e.com/seg101.m4s",
                "https://e.com/a.mp4",
                "https://e.com/seg102.m4s",
                "https://e.com/seg103.m4s",
            ]
        );
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn unreferenced_init_is_not_emitted_before_later_media() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
        // An ad block's init at MSN 101; its media is skipped upstream.
        let (ad_init, _) = mp4_init_keyed("https://e.com/ad.mp4", 101);
        h.input_tx.send(init_a).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
        h.input_tx.send(ad_init).await.unwrap();
        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 101,
                to_msn: 102,
            })
            .await
            .unwrap();
        h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            [
                "https://e.com/a.mp4",
                "https://e.com/seg100.m4s",
                "https://e.com/seg103.m4s",
            ]
        );
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn active_init_use_keeps_it_ahead_of_unused_inits_in_lru() {
        let mut config = HlsConfig::default();
        config.output_config.max_pending_init_segments = 2;
        let mut h = spawn_assembler(config, true, 100);
        let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
        let (unused_init, _) = mp4_init_keyed("https://e.com/unused.mp4", 101);
        let (init_c, key_c) = mp4_init_keyed("https://e.com/c.mp4", 102);
        h.input_tx.send(init_a).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
        h.input_tx.send(unused_init).await.unwrap();
        h.input_tx.send(mp4_media_keyed(101, &key_a)).await.unwrap();
        h.input_tx.send(init_c).await.unwrap();
        h.input_tx.send(mp4_media_keyed(102, &key_c)).await.unwrap();
        h.input_tx.send(mp4_media_keyed(103, &key_a)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            [
                "https://e.com/a.mp4",
                "https://e.com/seg100.m4s",
                "https://e.com/seg101.m4s",
                "https://e.com/c.mp4",
                "https://e.com/seg102.m4s",
                "https://e.com/a.mp4",
                "https://e.com/seg103.m4s",
            ]
        );
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn inits_fetched_ahead_of_their_media_are_not_evicted_before_use() {
        let mut config = HlsConfig::default();
        config.output_config.max_pending_init_segments = 2;
        let mut h = spawn_assembler(config, false, 100);
        let (init_a, key_a) = mp4_init_keyed("https://e.com/a.mp4", 100);
        let (init_b, key_b) = mp4_init_keyed("https://e.com/b.mp4", 101);
        let (init_c, key_c) = mp4_init_keyed("https://e.com/c.mp4", 102);
        // A finished stream's inits all arrive before the first media.
        h.input_tx.send(init_a).await.unwrap();
        h.input_tx.send(init_b).await.unwrap();
        h.input_tx.send(init_c).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key_a)).await.unwrap();
        h.input_tx.send(mp4_media_keyed(101, &key_b)).await.unwrap();
        h.input_tx.send(mp4_media_keyed(102, &key_c)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            [
                "https://e.com/a.mp4",
                "https://e.com/seg100.m4s",
                "https://e.com/b.mp4",
                "https://e.com/seg101.m4s",
                "https://e.com/c.mp4",
                "https://e.com/seg102.m4s",
            ]
        );
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn init_stamped_with_a_later_msn_still_precedes_its_earliest_media() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 10);
        // A retried init fetch was re-planned from a later window and carries
        // MSN 12, but media 10 and 11 depend on it.
        let (init, key) = mp4_init_keyed("https://e.com/init.mp4", 12);
        h.input_tx.send(mp4_media_keyed(10, &key)).await.unwrap();
        h.input_tx.send(mp4_media_keyed(11, &key)).await.unwrap();
        h.input_tx.send(init).await.unwrap();
        h.input_tx.send(mp4_media_keyed(12, &key)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            [
                "https://e.com/init.mp4",
                "https://e.com/seg10.m4s",
                "https://e.com/seg11.m4s",
                "https://e.com/seg12.m4s",
            ]
        );
        h.join.await.unwrap();
    }

    /// Data URIs emitted before `StreamEnded`, and whether a discontinuity
    /// event was observed.
    async fn collect_until_stream_end(h: &mut Harness) -> (Vec<String>, bool) {
        let mut uris = Vec::new();
        let mut discontinuity = false;
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = h.event_rx.recv().await {
                match event {
                    Ok(HlsStreamEvent::Data(data)) => uris.push(
                        data.media_segment()
                            .map(|s| s.uri.clone())
                            .unwrap_or_default(),
                    ),
                    Ok(HlsStreamEvent::DiscontinuityTagEncountered {}) => discontinuity = true,
                    Ok(HlsStreamEvent::StreamEnded) => break,
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out waiting for StreamEnded");
        (uris, discontinuity)
    }

    fn http_error(status: u16, uri: &str) -> SharedError {
        SharedError::new(DownloadError::http_status(
            reqwest::StatusCode::from_u16(status).unwrap(),
            uri,
            "segment download",
        ))
    }

    /// A terminal failure reported as HTTP 404 of the segment itself.
    fn terminal_failure(key: SegmentKey, msn: u64) -> AssemblerInput {
        AssemblerInput::TerminalFailed {
            error: http_error(404, &key.uri),
            key,
            msn,
            init_failed: false,
        }
    }

    /// Media the store did not fetch because its init failed with `error`.
    fn undecodable_media(msn: u64, error: &SharedError) -> AssemblerInput {
        AssemblerInput::TerminalFailed {
            key: SegmentKey {
                kind: SegmentKind::Media,
                uri: Arc::from(format!("https://e.com/seg{msn}.m4s")),
                byte_range: None,
                scope: None,
            },
            msn,
            error: error.clone(),
            init_failed: true,
        }
    }

    fn media_failure(msn: u64) -> AssemblerInput {
        terminal_failure(
            SegmentKey {
                kind: SegmentKind::Media,
                uri: Arc::from(format!("https://e.com/seg{msn}.m4s")),
                byte_range: None,
                scope: None,
            },
            msn,
        )
    }

    /// The stream's terminal error, when it ends with one before any data.
    async fn first_error_before_data(h: &mut Harness) -> Option<DownloadError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(event) = h.event_rx.recv().await {
                match event {
                    Err(error) => return Some(error),
                    Ok(HlsStreamEvent::Data(_) | HlsStreamEvent::StreamEnded) => return None,
                    Ok(_) => {}
                }
            }
            None
        })
        .await
        .expect("timed out waiting for the stream to end")
    }

    fn failed_url(error: &Option<DownloadError>) -> Option<&str> {
        match error {
            Some(DownloadError::HttpStatus { url, .. }) => Some(url),
            _ => None,
        }
    }

    #[tokio::test]
    async fn a_stream_whose_segments_all_failed_ends_with_the_failure() {
        let mut h = spawn_assembler(HlsConfig::default(), false, 0);
        h.input_tx.send(media_failure(0)).await.unwrap();
        h.input_tx.send(media_failure(1)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let error = first_error_before_data(&mut h).await;
        assert_eq!(failed_url(&error), Some("https://e.com/seg0.m4s"));
        assert!(matches!(h.join.await.unwrap(), AssemblerExit::Failed(_)));
    }

    #[tokio::test]
    async fn an_empty_stream_without_failures_still_ends_cleanly() {
        let mut h = spawn_assembler(HlsConfig::default(), false, 0);
        h.input_tx
            .send(AssemblerInput::Skipped {
                from_msn: 0,
                to_msn: 1,
            })
            .await
            .unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert!(uris.is_empty());
        assert_eq!(h.join.await.unwrap(), AssemblerExit::Done);
    }

    #[tokio::test]
    async fn a_finished_stream_that_delivered_nothing_reports_the_init_failure() {
        let mut h = spawn_assembler(HlsConfig::default(), false, 0);
        let key = init_key("https://e.com/init.mp4");
        let init_error = http_error(403, "https://e.com/init.mp4");
        h.input_tx.send(media_failure(0)).await.unwrap();
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: key.clone(),
                msn: 0,
                error: init_error.clone(),
                init_failed: false,
            })
            .await
            .unwrap();
        h.input_tx
            .send(undecodable_media(1, &init_error))
            .await
            .unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let error = first_error_before_data(&mut h).await;
        assert_eq!(failed_url(&error), Some("https://e.com/init.mp4"));
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn a_finished_stream_records_what_follows_media_of_a_failed_init() {
        // A first Period's init fails, its media is not fetched; the next
        // Period records.
        let mut h = spawn_assembler(HlsConfig::default(), false, 0);
        let failed = init_key("https://e.com/p0.mp4");
        let error = http_error(404, "https://e.com/p0.mp4");
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: failed,
                msn: 0,
                error: error.clone(),
                init_failed: false,
            })
            .await
            .unwrap();
        h.input_tx.send(undecodable_media(0, &error)).await.unwrap();
        let (init, key) = mp4_init_keyed("https://e.com/p1.mp4", 1);
        h.input_tx.send(init).await.unwrap();
        h.input_tx.send(mp4_media_keyed(1, &key)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(uris, ["https://e.com/p1.mp4", "https://e.com/seg1.m4s"]);
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn a_live_stream_starting_with_media_of_a_failed_init_fails_at_once() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let error = http_error(404, "https://e.com/init.mp4");
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: init_key("https://e.com/init.mp4"),
                msn: 100,
                error: error.clone(),
                init_failed: false,
            })
            .await
            .unwrap();
        // No End follows: the live stream would otherwise skip forever.
        h.input_tx
            .send(undecodable_media(100, &error))
            .await
            .unwrap();
        let error = first_error_before_data(&mut h).await;
        assert_eq!(failed_url(&error), Some("https://e.com/init.mp4"));
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn a_live_stream_does_not_fail_on_an_init_of_media_it_already_passed() {
        // Media 100 failed on its own and the cursor moved on before its
        // init failed; media 101 has its own, working init.
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        h.input_tx.send(media_failure(100)).await.unwrap();
        h.input_tx
            .send(AssemblerInput::TerminalFailed {
                key: init_key("https://e.com/p0.mp4"),
                msn: 100,
                error: http_error(404, "https://e.com/p0.mp4"),
                init_failed: false,
            })
            .await
            .unwrap();
        let (init, key) = mp4_init_keyed("https://e.com/p1.mp4", 101);
        h.input_tx.send(init).await.unwrap();
        h.input_tx.send(mp4_media_keyed(101, &key)).await.unwrap();
        let uris = collect_data_uris(&mut h, 2).await;
        assert_eq!(uris, ["https://e.com/p1.mp4", "https://e.com/seg101.m4s"]);
        h.cancel.cancel();
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn a_live_stream_whose_first_media_needs_a_failed_map_fails_with_that_map() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);
        let first = init_key("https://e.com/a.mp4");
        // A later map fails first, with another error.
        for (key, status) in [(init_key("https://e.com/b.mp4"), 403), (first.clone(), 503)] {
            h.input_tx
                .send(AssemblerInput::TerminalFailed {
                    error: http_error(status, &key.uri),
                    key,
                    msn: 100,
                    init_failed: false,
                })
                .await
                .unwrap();
        }
        // An HLS map's container is unknown until its media arrives.
        h.input_tx.send(mp4_media_keyed(100, &first)).await.unwrap();
        let error = first_error_before_data(&mut h).await;
        assert_eq!(failed_url(&error), Some("https://e.com/a.mp4"));
        h.join.await.unwrap();
    }

    #[tokio::test]
    async fn init_whose_media_fails_is_superseded_without_emission() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let (init1, _) = mp4_init_keyed("https://e.com/init1.mp4", 100);
        let (init2, key2) = mp4_init_keyed("https://e.com/init2.mp4", 101);
        h.input_tx.send(init1).await.unwrap();
        h.input_tx.send(media_failure(100)).await.unwrap();
        h.input_tx.send(init2).await.unwrap();
        h.input_tx.send(mp4_media_keyed(101, &key2)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, _) = collect_until_stream_end(&mut h).await;
        assert_eq!(
            uris,
            ["https://e.com/init2.mp4", "https://e.com/seg101.m4s"]
        );
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn init_without_media_is_not_emitted_at_stream_end() {
        let mut h = spawn_assembler(HlsConfig::default(), true, 100);
        let (init, key) = mp4_init_keyed("https://e.com/init.mp4", 100);
        h.input_tx.send(init).await.unwrap();
        h.input_tx.send(mp4_media_keyed(100, &key)).await.unwrap();
        let (mut next_init, _) = mp4_init_keyed("https://e.com/init2.mp4", 101);
        if let AssemblerInput::Payload(SegmentPayload::Mp4Init { descriptor, .. }) = &mut next_init
        {
            let mut d = (**descriptor).clone();
            d.discontinuity = true;
            *descriptor = Arc::new(d);
        }
        h.input_tx.send(next_init).await.unwrap();
        h.input_tx.send(media_failure(101)).await.unwrap();
        h.input_tx.send(AssemblerInput::End).await.unwrap();
        let (uris, discontinuity) = collect_until_stream_end(&mut h).await;
        assert_eq!(uris, ["https://e.com/init.mp4", "https://e.com/seg100.m4s"]);
        assert!(
            !discontinuity,
            "a discontinuity is only emitted ahead of the init and media it applies to"
        );
        let _ = h.join.await;
    }

    #[tokio::test]
    async fn init_terminal_failure_mid_stream_skips_dependent_media_not_stall() {
        let mut config = HlsConfig::default();
        config.output_config.live_gap_strategy = GapSkipStrategy::WaitIndefinitely;
        let mut h = spawn_assembler(config, true, 100);
        let key = init_key("https://e.com/init.mp4");

        h.input_tx.send(ts_payload(100)).await.unwrap();
        // A map rotation terminally fails before its media payload arrives.
        h.input_tx
            .send(terminal_failure(key.clone(), 101))
            .await
            .unwrap();
        // Media depending on the failed init, plus a clear TS that follows.
        h.input_tx.send(mp4_media_keyed(101, &key)).await.unwrap();
        h.input_tx.send(ts_payload(102)).await.unwrap();

        // The dependent media is skipped (visible gap), 102 still emits — no
        // permanent gated stall, no unbounded buffer.
        let mut saw_gap = false;
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while let Some(evt) = h.event_rx.recv().await {
                match evt {
                    Ok(HlsStreamEvent::GapSkipped { .. }) => saw_gap = true,
                    Ok(HlsStreamEvent::Data(d)) => {
                        got.extend(d.media_segment().map(|s| s.uri.clone()));
                        if got.len() == 2 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("timed out: init failure stalled the stream");
        assert!(saw_gap, "failed-init media must surface as a gap skip");
        assert_eq!(got, ["https://e.com/seg100.ts", "https://e.com/seg102.ts"]);
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
