//! The reactor-owned segment state store.
//!
//! Single authoritative home for identity dedup, lifecycle state, retry
//! budget, and scheduling priority. Owned by the reactor task and never shared
//! — not an `Arc<Mutex<..>>`. The `HashMap` is the single source of truth; the
//! ready index and retry heap are advisory and re-validated on pop (lazy
//! tombstones), because an entry goes stale when its record changes state or
//! is removed while its key still sits in an index.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use tracing::{debug, trace, warn};

use crate::DownloadError;
use crate::error::SharedError;

use super::budget::{ByteBudget, ByteReservation};
use super::container::ContainerKind;
use super::descriptor::{SegmentDescriptor, SegmentSource};
use super::identity::{SegmentKey, SegmentKind};
use super::input::AssemblerInput;
use super::payload::SegmentPayload;
use super::throttle::RateLimitHolds;

mod window;
pub use window::MediaWindow;

#[cfg(test)]
mod tests;

/// Machine-readable failure classification reported by the fetch-and-process
/// future. The store — not the future — maps class to retryable-vs-terminal,
/// so retry policy lives in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Http(u16),
    /// The server is rate limiting (a 429, or a 503 with `Retry-After`);
    /// `delay` is the rest of the cool-down. Retried after it without
    /// spending the retry budget, until [`StoreConfig::max_throttled`] has
    /// passed since the segment was first held back.
    RateLimited {
        status: u16,
        delay: Duration,
        /// Held back before any request was sent: not a failed download.
        held_back: bool,
    },
    Network,
    Timeout,
    Decode,
    UnsupportedCrypto,
    InvalidFormat,
    /// Download aborted because the byte budget could not grow to fit the
    /// body. Retryable: the lifecycle retry re-attempts when the budget is
    /// freer, guaranteeing forward progress without a mutual-wait deadlock.
    OverBudget,
    /// The segment can never fit (exceeds the budget capacity or the
    /// configured per-segment maximum). Terminal.
    Oversize,
}

impl FailureClass {
    /// The error a terminal failure of `descriptor` surfaces as, should the
    /// stream end up delivering nothing because of it, when the failure site
    /// gave no precise error. An HTTP failure has one: the URL it names is
    /// the one requested, which a refresh may since have replaced.
    fn error(self, reason: &str, descriptor: &SegmentDescriptor) -> DownloadError {
        let protocol = descriptor.meta.protocol();
        let reason = reason.to_owned();
        match self {
            Self::Http(status) => DownloadError::SegmentFetch {
                reason,
                retryable: !StatusCode::from_u16(status)
                    .is_ok_and(crate::error::is_permanent_client_error),
            },
            Self::Network | Self::OverBudget | Self::RateLimited { .. } => {
                DownloadError::SegmentFetch {
                    reason,
                    retryable: true,
                }
            }
            Self::Timeout => DownloadError::Timeout { reason },
            Self::Decode => DownloadError::Decryption { reason },
            Self::UnsupportedCrypto => DownloadError::Unsupported {
                protocol,
                feature: reason,
            },
            Self::InvalidFormat => DownloadError::InvalidContent { protocol, reason },
            Self::Oversize => DownloadError::SegmentFetch {
                reason,
                retryable: false,
            },
        }
    }
}

/// Tells the assembler that `descriptor` will never arrive, and why.
fn terminal_input(
    descriptor: &SegmentDescriptor,
    error: SharedError,
    init_failed: bool,
) -> AssemblerInput {
    AssemblerInput::TerminalFailed {
        key: descriptor.key.clone(),
        msn: descriptor.msn,
        error,
        init_failed,
    }
}

#[derive(Debug, Clone)]
pub enum SegmentState {
    /// Ingested and waiting in the ready index.
    Discovered,
    /// A due retry placed back in the ready index.
    Queued,
    InFlight,
    Completed,
    RetryAt {
        retry_at: Instant,
    },
    TerminalFailed {
        /// What the stream reports should it deliver nothing because of
        /// this; for an init, also what its media fails with.
        error: SharedError,
    },
}

impl SegmentState {
    fn is_schedulable(&self) -> bool {
        matches!(self, Self::Discovered | Self::Queued)
    }

    fn is_unfinished(&self) -> bool {
        // Every `RetryAt` counts, including future deadlines: a final-window
        // segment waiting on a retry must hold the authoritative-end drain
        // open until it completes or terminalizes.
        matches!(
            self,
            Self::Discovered | Self::Queued | Self::InFlight | Self::RetryAt { .. }
        )
    }
}

/// Result of one fetch-and-process future.
#[derive(Debug)]
pub enum SegmentOutcome {
    Completed {
        key: SegmentKey,
        msn: u64,
        payload: SegmentPayload,
    },
    Failed {
        key: SegmentKey,
        msn: u64,
        class: FailureClass,
        reason: Arc<str>,
        /// The precise error, when the failure site has one: a key fetch's
        /// names the key, not the segment. Otherwise a terminal failure's
        /// error is derived from `class`.
        error: Option<DownloadError>,
    },
}

#[derive(Debug, Clone)]
pub struct RetryNotice {
    /// The descriptor the retry will fetch, for the event's resource identity.
    pub descriptor: Arc<SegmentDescriptor>,
    pub attempt: u32,
    pub delay: Duration,
    pub reason: Arc<str>,
}

#[derive(Debug, Default)]
pub struct OutcomeEffects {
    pub assembler_inputs: Vec<AssemblerInput>,
    pub retry_notice: Option<RetryNotice>,
}

/// A schedulable job handed to a fetch-and-process task. Owns the RAII
/// download-byte reservation, so admission and byte-charging are one step.
#[derive(Debug)]
pub struct ReadyJob {
    pub descriptor: Arc<SegmentDescriptor>,
    pub reservation: ByteReservation,
}

#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Control-plane record backstop. Applied only within the prune invariant
    /// (never evict a key still inside the window); when nothing can be
    /// evicted safely, state temporarily exceeds this cap.
    pub max_state_entries: usize,
    /// Lifecycle reschedule budget per segment (distinct from per-attempt HTTP
    /// retries inside the fetch future).
    pub retry_budget: u32,
    pub retry_delay_base: Duration,
    pub retry_delay_max: Duration,
    /// Time budget for retrying one segment, measured from its first attempt
    /// and checked when an attempt fails: a failure past it is terminal
    /// instead of rescheduled, and a running attempt is never cut short.
    /// Waiting out rate limiting restarts it. `None` removes the cap (the
    /// retry count still applies).
    pub retry_deadline: Option<Duration>,
    /// How long after it was first held back by rate limiting a segment
    /// fails; rate-limited attempts do not spend `retry_budget`.
    pub max_throttled: Duration,
    /// Size estimate used before any segment has completed; afterwards an EMA
    /// of actual completed sizes takes over.
    pub fallback_size_estimate: u64,
    /// Per-segment hard cap (0 = disabled). A body that would exceed it
    /// aborts as `Oversize` (terminal).
    pub max_segment_size: u64,
    /// How many init-segment records to retain across window slides.
    pub max_retained_inits: usize,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            max_state_entries: 2048,
            retry_budget: 3,
            retry_delay_base: Duration::from_millis(500),
            retry_delay_max: Duration::from_secs(10),
            retry_deadline: None,
            max_throttled: crate::rate_limit::RateLimitPolicy::default().max_throttled,
            fallback_size_estimate: 2 * 1024 * 1024,
            max_segment_size: 0,
            max_retained_inits: 8,
        }
    }
}

/// Shortest wait before retrying a rate-limited segment, should the
/// configured retry base be zero.
const MIN_RATE_LIMITED_RETRY: Duration = Duration::from_millis(100);

/// Longest a segment refused with 401/403 after rate limiting waits for
/// re-discovery to publish a fresh URL.
const REFRESH_WAIT: Duration = Duration::from_secs(10);

/// Counters for ingest observability (aggregated, bounded cardinality).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    pub discovered: usize,
    pub refreshed: usize,
    pub deduplicated: usize,
}

#[derive(Debug)]
struct SegmentRecord {
    descriptor: Arc<SegmentDescriptor>,
    state: SegmentState,
    reschedules: u32,
    /// When the current run of ordinary attempts started. The retry deadline
    /// is measured from here, so the backoff between those attempts counts
    /// against it. Cleared by an outcome that does not spend the retry
    /// budget (rate limiting, the URL refresh wait, budget contention): the
    /// time the segment waited there says nothing about the segment.
    first_attempt_at: Option<Instant>,
    /// When rate limiting first held this segment back.
    throttled_since: Option<Instant>,
    /// Waiting for re-discovery to refresh a URL that was refused with
    /// 401/403 after rate limiting; re-discovery then retries at once.
    awaiting_refresh: bool,
    /// The one such wait was taken.
    refresh_wait_used: bool,
    /// Bumped whenever re-discovery refreshes the fetch metadata. Compared
    /// against `attempt_generation` for the 401/403 stale-signed-URL rule.
    generation: u64,
    /// Generation the current/last in-flight attempt fetched with.
    attempt_generation: u64,
    /// Exact entry placed in the ready index, so a source upgrade
    /// (prefetch -> playlist) can relocate it under its new priority class.
    ready_entry: Option<ReadyEntry>,
    /// Highest MSN at which this segment (or init map) has been observed in the
    /// playlist window; protects active maps from sliding-window eviction.
    latest_msn: u64,
    /// Cached payload for a completed init segment, replayed into the assembler
    /// when a multi-map stream rotates back to an earlier `#EXT-X-MAP`.
    completed_init: Option<SegmentPayload>,
    /// For an init: media it governs arrived as fMP4, so that media cannot
    /// be decoded without it. Evidence for manifests that do not declare the
    /// container (HLS, whose maps may also govern MPEG-TS).
    governs_fmp4: bool,
}

/// Ready-index entry. Order: init first, then playlist media, then prefetch;
/// within a class lower MSN first, then stable insertion order. `order` is
/// globally unique, which keeps the `BTreeSet` collision-free.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ReadyEntry {
    class: u8,
    msn: u64,
    order: u64,
}

fn priority_class(descriptor: &SegmentDescriptor) -> u8 {
    match (descriptor.key.kind, descriptor.source) {
        (SegmentKind::Init, _) => 0,
        (SegmentKind::Media, SegmentSource::Playlist) => 1,
        (SegmentKind::Media, SegmentSource::PlaylistPrefetch) => 2,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetryEntry {
    retry_at: Instant,
    order: u64,
    key: SegmentKey,
}

impl PartialOrd for RetryEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RetryEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.retry_at, self.order).cmp(&(other.retry_at, other.order))
    }
}

#[derive(Debug)]
pub struct SegmentStateStore {
    config: StoreConfig,
    records: HashMap<SegmentKey, SegmentRecord>,
    /// Advisory ready index; entries map back to records via `ready_lookup`.
    ready: BTreeSet<ReadyEntry>,
    ready_lookup: HashMap<u64, SegmentKey>,
    retry_heap: BinaryHeap<Reverse<RetryEntry>>,
    next_order: u64,
    /// EMA of actual completed segment sizes; the admission estimate for
    /// segments without a BYTERANGE length.
    size_estimate: u64,
    /// Bytes of admitted or completed media at or beyond the emit cursor,
    /// by MSN: the estimate while a segment downloads, its size once it has
    /// completed. Maintained for the byte bound of a [`MediaWindow`].
    window_bytes: BTreeMap<u64, u64>,
    last_admitted_init_key: Option<SegmentKey>,
    rate_limit_holds: Arc<RateLimitHolds>,
}

impl SegmentStateStore {
    pub fn new(config: StoreConfig, rate_limit_holds: Arc<RateLimitHolds>) -> Self {
        let size_estimate = config.fallback_size_estimate.max(1);
        rate_limit_holds.set_max_pause(config.max_throttled);
        Self {
            config,
            records: HashMap::new(),
            ready: BTreeSet::new(),
            ready_lookup: HashMap::new(),
            retry_heap: BinaryHeap::new(),
            next_order: 0,
            size_estimate,
            window_bytes: BTreeMap::new(),
            last_admitted_init_key: None,
            rate_limit_holds,
        }
    }

    #[cfg(test)]
    pub fn current_size_estimate(&self) -> u64 {
        self.size_estimate
    }

    fn alloc_order(&mut self) -> u64 {
        let order = self.next_order;
        self.next_order += 1;
        order
    }

    fn insert_ready(&mut self, key: &SegmentKey) {
        let order = self.alloc_order();
        let record = self.records.get_mut(key).expect("record exists");
        let entry = ReadyEntry {
            class: priority_class(&record.descriptor),
            msn: record.descriptor.msn,
            order,
        };
        record.ready_entry = Some(entry.clone());
        self.ready.insert(entry);
        self.ready_lookup.insert(order, key.clone());
    }

    fn remove_ready_entry(&mut self, entry: &ReadyEntry) {
        self.ready.remove(entry);
        self.ready_lookup.remove(&entry.order);
    }

    /// Register planner output. Known keys are *not* a no-op: identity is
    /// stable but fetch URLs are not, so re-discovery refreshes the volatile
    /// fetch metadata (`parsed_url`, key fetch URL, `source` upgrade) while
    /// preserving lifecycle state — otherwise dedup would pin an expired
    /// signed URL onto a future retry.
    pub fn ingest(&mut self, descriptors: Vec<SegmentDescriptor>, _now: Instant) -> IngestStats {
        let mut stats = IngestStats::default();
        for descriptor in descriptors {
            let key = descriptor.key.clone();
            let mut promote = false;
            if let Some(record) = self.records.get_mut(&key) {
                record.latest_msn = record.latest_msn.max(descriptor.msn);
                let stale_entry = match record.state {
                    SegmentState::Completed | SegmentState::TerminalFailed { .. } => {
                        // Never fetched again; `latest_msn` above records where
                        // a recurring `#EXT-X-MAP` still applies so `prune_below`
                        // keeps it while in-window.
                        stats.deduplicated += 1;
                        continue;
                    }
                    SegmentState::Discovered
                    | SegmentState::Queued
                    | SegmentState::RetryAt { .. }
                    | SegmentState::InFlight => {
                        // For InFlight the running attempt keeps its URL; the
                        // refreshed descriptor is what a post-failure retry
                        // will fetch.
                        let url_changed = record.descriptor.parsed_url != descriptor.parsed_url
                            || !encryption_fetch_eq(&record.descriptor, &descriptor);
                        let class_changed =
                            priority_class(&record.descriptor) != priority_class(&descriptor);
                        if key.kind == SegmentKind::Init {
                            if url_changed {
                                let mut updated = descriptor;
                                updated.msn = record.descriptor.msn;
                                updated.discontinuity = record.descriptor.discontinuity;
                                record.descriptor = Arc::new(updated);
                                record.generation += 1;
                            }
                        } else {
                            let mut updated = descriptor;
                            updated.discontinuity |= record.descriptor.discontinuity;
                            record.descriptor = Arc::new(updated);
                            if url_changed {
                                record.generation += 1;
                            }
                        }
                        stats.refreshed += 1;
                        if url_changed
                            && record.awaiting_refresh
                            && matches!(record.state, SegmentState::RetryAt { .. })
                        {
                            // The fresh URL it was waiting for: retry now.
                            // The retry heap entry goes stale with the state.
                            record.awaiting_refresh = false;
                            record.state = SegmentState::Queued;
                            promote = true;
                        }
                        if class_changed {
                            record.ready_entry.take()
                        } else {
                            None
                        }
                    }
                };
                if let Some(entry) = stale_entry {
                    // Source upgrade (prefetch -> playlist): relocate the
                    // ready entry under its new priority class.
                    self.remove_ready_entry(&entry);
                    self.insert_ready(&key);
                } else if promote {
                    self.insert_ready(&key);
                }
            } else {
                let latest_msn = descriptor.msn;
                self.records.insert(
                    key.clone(),
                    SegmentRecord {
                        descriptor: Arc::new(descriptor),
                        state: SegmentState::Discovered,
                        reschedules: 0,
                        first_attempt_at: None,
                        throttled_since: None,
                        awaiting_refresh: false,
                        refresh_wait_used: false,
                        generation: 0,
                        attempt_generation: 0,
                        ready_entry: None,
                        latest_msn,
                        completed_init: None,
                        governs_fmp4: false,
                    },
                );
                self.insert_ready(&key);
                stats.discovered += 1;
            }
        }
        stats
    }

    /// Move due retries from the heap into the ready index. The reactor calls
    /// this every loop pass — not just at admission — so a deadline that
    /// elapses while the dispatch gate is closed drains out of the heap
    /// (`next_retry_deadline` then returns the next future deadline) instead
    /// of leaving the retry-wake arm ready forever and busy-spinning the loop.
    pub fn promote_due_retries(&mut self, now: Instant) {
        while let Some(Reverse(top)) = self.retry_heap.peek() {
            if top.retry_at > now {
                break;
            }
            let Reverse(entry) = self.retry_heap.pop().expect("peeked");
            let Some(record) = self.records.get_mut(&entry.key) else {
                continue; // pruned: tombstone
            };
            match &record.state {
                SegmentState::RetryAt { retry_at, .. } if *retry_at == entry.retry_at => {
                    record.state = SegmentState::Queued;
                    let key = entry.key.clone();
                    self.insert_ready(&key);
                }
                // Stale heap entry (rescheduled since): drop it.
                _ => {}
            }
        }
    }

    /// Earliest pending retry deadline, lazily discarding stale heap entries.
    pub fn next_retry_deadline(&mut self) -> Option<Instant> {
        while let Some(Reverse(top)) = self.retry_heap.peek() {
            let valid = self
                .records
                .get(&top.key)
                .is_some_and(|r| matches!(&r.state, SegmentState::RetryAt { retry_at, .. } if *retry_at == top.retry_at));
            if valid {
                return Some(top.retry_at);
            }
            self.retry_heap.pop();
        }
        None
    }

    /// True while any segment still needs work — the authoritative-end drain
    /// predicate. This is intentionally *not* "nothing schedulable right now":
    /// a future-deadline retry is unfinished work.
    pub fn has_unfinished_work(&self) -> bool {
        self.records.values().any(|r| r.state.is_unfinished())
    }

    /// True while any initialization segment (`SegmentKind::Init`) is still
    /// discovered, queued, in flight, or awaiting retry.
    pub fn has_unfinished_inits(&self) -> bool {
        self.records
            .values()
            .any(|r| r.descriptor.key.kind == SegmentKind::Init && r.state.is_unfinished())
    }

    /// [`Self::next_ready_jobs_within`] without a media window.
    #[cfg(test)]
    pub fn next_ready_jobs(
        &mut self,
        slots: usize,
        now: Instant,
        budget: &ByteBudget,
    ) -> (Vec<ReadyJob>, Vec<AssemblerInput>) {
        self.next_ready_jobs_within(slots, now, budget, None)
    }

    /// Batched admission: returns up to `slots` jobs in priority order, each
    /// owning its download-byte reservation, plus any assembler items implied
    /// by admission-time terminalization (oversize segments). Stops early when
    /// the budget cannot fit the next estimate — admission is the byte gate.
    /// Media is admitted only inside `window`; segments outside it stay ready
    /// for a later call. Inits are admitted within a bounded lookahead ahead of
    /// `window.cursor` so a multi-map VOD does not download every map up front
    /// and overflow retained-init capacity before early media is emitted.
    pub fn next_ready_jobs_within(
        &mut self,
        slots: usize,
        now: Instant,
        budget: &ByteBudget,
        window: Option<MediaWindow>,
    ) -> (Vec<ReadyJob>, Vec<AssemblerInput>) {
        self.promote_due_retries(now);

        let mut held_bytes = 0;
        if let Some(window) = window {
            // Media below the cursor has left the reorder buffer.
            self.window_bytes = self.window_bytes.split_off(&window.cursor);
            held_bytes = self.window_bytes.values().sum::<u64>();
        }

        let mut jobs = Vec::new();
        let mut inputs = Vec::new();
        let mut lower_bound = ReadyEntry {
            class: 0,
            msn: 0,
            order: 0,
        };

        while jobs.len() < slots {
            let Some(entry) = self.ready.range(lower_bound.clone()..).next().cloned() else {
                break;
            };
            let Some(key) = self.ready_lookup.get(&entry.order).cloned() else {
                self.ready.remove(&entry);
                continue;
            };
            let Some(record) = self.records.get(&key) else {
                // Pruned while indexed: tombstone, drop lazily.
                self.remove_ready_entry(&entry);
                continue;
            };
            if record.ready_entry.as_ref() != Some(&entry) || !record.state.is_schedulable() {
                self.remove_ready_entry(&entry);
                continue;
            }

            let is_media = record.descriptor.key.kind == SegmentKind::Media;
            if !is_media
                && window.is_some_and(|window| {
                    window.segments > 0
                        && entry.msn
                            >= window
                                .cursor
                                .saturating_add(window.segments.saturating_mul(2).max(16))
                })
            {
                // Later inits in class 0 have msn >= entry.msn >= init_limit;
                // advance past class 0 so playlist media (class 1) still admits.
                lower_bound = ReadyEntry {
                    class: 1,
                    msn: 0,
                    order: 0,
                };
                continue;
            }

            // Ready media within a priority class is ordered by MSN, so no
            // later entry in this class is below the segment window either.
            if is_media
                && window.is_some_and(|window| {
                    window.segments > 0
                        && record.descriptor.msn >= window.cursor.saturating_add(window.segments)
                })
            {
                if entry.class == 1 {
                    lower_bound = ReadyEntry {
                        class: 2,
                        msn: 0,
                        order: 0,
                    };
                    continue;
                }
                break;
            }

            // fMP4 media whose own init terminally failed cannot be decoded,
            // so it fails with the init's error instead of being downloaded.
            if let Some(init_error) = self.failed_init_of(&record.descriptor) {
                if inputs.len() >= slots {
                    break;
                }
                self.remove_ready_entry(&entry);
                let record = self.records.get_mut(&key).expect("validated above");
                record.ready_entry = None;
                trace!(
                    msn = record.descriptor.msn,
                    "skipping media of a failed init"
                );
                inputs.push(terminal_input(&record.descriptor, init_error.clone(), true));
                self.rate_limit_holds
                    .release(&key, record.descriptor.msn, now);
                record.state = SegmentState::TerminalFailed { error: init_error };
                continue;
            }

            let estimate = record.descriptor.size_estimate(self.size_estimate);

            // Oversize at admission requires a *known* size: only a BYTERANGE
            // length proves the segment can never fit. A fallback estimate is
            // a guess — it clamps to capacity below and the body-streaming
            // checks in fetch decide from real sizes.
            let known_size = record.descriptor.key.byte_range.map(|range| range.length);
            let capacity = budget.download.capacity();
            let over_capacity = known_size.is_some_and(|size| capacity > 0 && size > capacity);
            let over_segment_max = known_size.is_some_and(|size| {
                self.config.max_segment_size > 0 && size > self.config.max_segment_size
            });
            if over_capacity || over_segment_max {
                // Terminalizations are admission work too: cap them per pass
                // so one call cannot convert an entire ready index into
                // `pending` items and blow the reactor's item bound. The
                // remaining entries stay in the ready index for later passes.
                if inputs.len() >= slots {
                    break;
                }
                self.remove_ready_entry(&entry);
                let reason: Arc<str> = Arc::from(format!(
                    "segment estimate {estimate} exceeds {}",
                    if over_capacity {
                        "download byte budget capacity"
                    } else {
                        "configured per-segment maximum"
                    }
                ));
                let record = self.records.get_mut(&key).expect("validated above");
                record.ready_entry = None;
                let msn = record.descriptor.msn;
                warn!(msn, reason = %crate::redact::redact_diagnostic(&reason), "segment oversize at admission");
                let error =
                    SharedError::new(FailureClass::Oversize.error(&reason, &record.descriptor));
                inputs.push(terminal_input(&record.descriptor, error.clone(), false));
                self.rate_limit_holds
                    .release(&key, record.descriptor.msn, now);
                record.state = SegmentState::TerminalFailed { error };
                continue;
            }

            // Reserving here — before the spawn — closes the race where many
            // tasks pass an advisory budget read before any has charged. An
            // estimate above capacity clamps rather than terminalizing (the
            // real size is unknown); the reservation then grows at chunk
            // granularity if the body is genuinely larger.
            let reserve_size = if capacity > 0 {
                estimate.min(capacity)
            } else {
                estimate
            };
            // The segment at the cursor is exempt: it is the one that frees
            // the buffer, so holding it back could only deadlock the stream.
            // So is any segment when nothing is held, which keeps a bound
            // below one segment's estimate from stalling the stream.
            if is_media
                && held_bytes > 0
                && let Some(window) = window
                && window.bytes > 0
                && record.descriptor.msn > window.cursor
                && held_bytes.saturating_add(reserve_size) > window.bytes
            {
                break;
            }
            let Some(reservation) = budget.download.try_reserve(reserve_size) else {
                break;
            };

            self.remove_ready_entry(&entry);
            let record = self.records.get_mut(&key).expect("validated above");
            record.ready_entry = None;
            let attempt = record.reschedules + 1;
            record.awaiting_refresh = false;
            record.state = SegmentState::InFlight;
            record.attempt_generation = record.generation;
            record.first_attempt_at.get_or_insert(now);
            let media_init_and_msn = (is_media
                && record.descriptor.source == SegmentSource::Playlist)
                .then(|| (record.descriptor.init_key.clone(), record.descriptor.msn));
            if is_media && window.is_some() {
                self.window_bytes
                    .insert(record.descriptor.msn, reserve_size);
                held_bytes = held_bytes.saturating_add(reserve_size);
            }
            trace!(
                msn = record.descriptor.msn,
                kind = ?record.descriptor.key.kind,
                attempt,
                reserved = reservation.held_bytes(),
                "segment admitted"
            );
            jobs.push(ReadyJob {
                descriptor: Arc::clone(&record.descriptor),
                reservation,
            });
            if let Some((init_key, media_msn)) = media_init_and_msn {
                if let Some(ref init_key) = init_key
                    && self.last_admitted_init_key.as_ref() != Some(init_key)
                    && let Some(cached_init) = self
                        .records
                        .get(init_key)
                        .and_then(|r| r.completed_init.as_ref())
                {
                    inputs.push(AssemblerInput::Payload(cached_init.with_msn(media_msn)));
                }
                self.last_admitted_init_key = init_key;
            }
        }

        (jobs, inputs)
    }

    /// Apply a finished future's outcome and return the effects it implies:
    /// a `Payload` on success, a `TerminalFailed` when the failure terminalizes,
    /// and a best-effort retry notice when lifecycle retry is scheduled. Retry
    /// policy is decided here, from the `FailureClass` and the remaining
    /// lifecycle budget — never inside the fetch future.
    pub fn apply_outcome(&mut self, outcome: SegmentOutcome, now: Instant) -> OutcomeEffects {
        match outcome {
            SegmentOutcome::Completed { key, msn, payload } => {
                // RFC 8216 §3 / §4.3.2.5: fragmented MP4 media segments require
                // an initialization map (#EXT-X-MAP). Terminalizing here frees
                // the VOD admission window instead of parking undecodable media
                // behind an init gate that will never open.
                if matches!(payload, SegmentPayload::Mp4Media { .. })
                    && payload.descriptor().init_key.is_none()
                {
                    let protocol = payload.descriptor().meta.protocol();
                    let error = SharedError::new(DownloadError::InvalidContent {
                        protocol,
                        reason: "fMP4 media segment missing initialization map (#EXT-X-MAP)"
                            .to_owned(),
                    });
                    self.window_bytes.remove(&msn);
                    self.rate_limit_holds.release(&key, msn, now);
                    if let Some(record) = self.records.get_mut(&key) {
                        record.state = SegmentState::TerminalFailed {
                            error: error.clone(),
                        };
                    }
                    warn!(
                        msn,
                        "fMP4 media segment completed without #EXT-X-MAP; marking terminal failure"
                    );
                    return OutcomeEffects {
                        assembler_inputs: vec![AssemblerInput::TerminalFailed {
                            key,
                            msn,
                            error,
                            init_failed: true,
                        }],
                        retry_notice: None,
                    };
                }

                if !payload.is_init()
                    && payload.is_fmp4()
                    && let Some(init) = payload
                        .descriptor()
                        .init_key
                        .as_ref()
                        .and_then(|init_key| self.records.get_mut(init_key))
                {
                    init.governs_fmp4 = true;
                }
                let payload_len = payload.len() as u64;
                if !payload.is_init()
                    && let Some(bytes) = self.window_bytes.get_mut(&msn)
                {
                    *bytes = payload_len;
                }
                if let Some(record) = self.records.get_mut(&key) {
                    record.state = SegmentState::Completed;
                    if payload.is_init() {
                        record.completed_init = Some(payload.clone());
                    }
                    self.update_size_estimate(payload_len);
                    trace!(msn, "segment completed");
                }
                // A record pruned mid-flight (window long gone) still forwards
                // its payload: the assembler stale-rejects if it is too old.
                OutcomeEffects {
                    assembler_inputs: vec![AssemblerInput::Payload(payload)],
                    retry_notice: None,
                }
            }
            SegmentOutcome::Failed {
                key,
                msn,
                class,
                reason,
                error,
            } => {
                // Rate limiting and the URL refresh wait below keep the
                // hold; any other failure ends it.
                let mut keeps_hold = matches!(class, FailureClass::RateLimited { .. });
                if key.kind == SegmentKind::Media {
                    self.window_bytes.remove(&msn);
                }
                let Some(record) = self.records.get_mut(&key) else {
                    if !keeps_hold {
                        self.rate_limit_holds.release(&key, msn, now);
                    }
                    return OutcomeEffects::default();
                };

                let backoff = |reschedules: u32| {
                    let exp = reschedules.saturating_sub(1).min(16);
                    self.config
                        .retry_delay_base
                        .checked_mul(1u32 << exp)
                        .unwrap_or(self.config.retry_delay_max)
                        .min(self.config.retry_delay_max)
                };
                let retry_delay = match class {
                    FailureClass::RateLimited { delay, .. } => {
                        record.first_attempt_at = None;
                        let since = *record.throttled_since.get_or_insert(now);
                        self.config
                            .max_throttled
                            .checked_sub(now.saturating_duration_since(since))
                            .filter(|left| !left.is_zero())
                            .map(|left| {
                                delay
                                    .max(self.config.retry_delay_base)
                                    .max(MIN_RATE_LIMITED_RETRY)
                                    .min(left)
                            })
                    }
                    FailureClass::Http(401 | 403)
                        if record.generation <= record.attempt_generation
                            && record.throttled_since.is_some()
                            && !record.refresh_wait_used =>
                    {
                        record.first_attempt_at = None;
                        record.refresh_wait_used = true;
                        record.awaiting_refresh = true;
                        keeps_hold = true;
                        Some(REFRESH_WAIT)
                    }
                    FailureClass::OverBudget => {
                        record.first_attempt_at = None;
                        Some(backoff(record.reschedules))
                    }
                    _ => {
                        let retryable = match class {
                            FailureClass::Http(404 | 425 | 429)
                            | FailureClass::Http(500..=599)
                            | FailureClass::Network
                            | FailureClass::Timeout
                            | FailureClass::Decode => true,
                            FailureClass::Http(401 | 403) => {
                                record.generation > record.attempt_generation
                            }
                            FailureClass::Http(_)
                            | FailureClass::UnsupportedCrypto
                            | FailureClass::InvalidFormat
                            | FailureClass::Oversize
                            | FailureClass::RateLimited { .. }
                            | FailureClass::OverBudget => false,
                        };
                        let within_deadline = self.config.retry_deadline.is_none_or(|deadline| {
                            record
                                .first_attempt_at
                                .is_none_or(|first| now.saturating_duration_since(first) < deadline)
                        });
                        (retryable
                            && within_deadline
                            && record.reschedules < self.config.retry_budget)
                            .then(|| {
                                record.reschedules += 1;
                                backoff(record.reschedules)
                            })
                    }
                };

                if let Some(delay) = retry_delay {
                    if !keeps_hold {
                        self.rate_limit_holds.release(&key, msn, now);
                    } else if let Some(since) = record.throttled_since
                        && let Some(until) = since.checked_add(self.config.max_throttled)
                    {
                        self.rate_limit_holds.hold(&record.descriptor, until, now);
                    }
                    let reschedules = record.reschedules;
                    let retry_at = now + delay;
                    record.state = SegmentState::RetryAt { retry_at };
                    let descriptor = Arc::clone(&record.descriptor);
                    debug!(
                        msn,
                        ?class,
                        reason = %crate::redact::redact_diagnostic(&reason),
                        reschedules,
                        delay_ms = delay.as_millis() as u64,
                        "segment scheduled for lifecycle retry"
                    );
                    let order = self.alloc_order();
                    self.retry_heap.push(Reverse(RetryEntry {
                        retry_at,
                        order,
                        key: key.clone(),
                    }));
                    OutcomeEffects {
                        assembler_inputs: Vec::new(),
                        retry_notice: Some(RetryNotice {
                            descriptor,
                            attempt: reschedules,
                            delay,
                            reason,
                        }),
                    }
                } else {
                    self.rate_limit_holds.release(&key, msn, now);
                    warn!(msn, ?class, reason = %crate::redact::redact_diagnostic(&reason), "segment terminally failed");
                    let error = SharedError::new(
                        error.unwrap_or_else(|| class.error(&reason, &record.descriptor)),
                    );
                    let input = terminal_input(&record.descriptor, error.clone(), false);
                    record.state = SegmentState::TerminalFailed { error };
                    OutcomeEffects {
                        assembler_inputs: vec![input],
                        retry_notice: None,
                    }
                }
            }
        }
    }

    /// The error of `descriptor`'s init when that init terminally failed and
    /// the media cannot be decoded without it: fMP4 media, declared so by
    /// the manifest or shown so by other media of the same init. MPEG-TS
    /// media decodes without its map.
    fn failed_init_of(&self, descriptor: &SegmentDescriptor) -> Option<SharedError> {
        if descriptor.key.kind != SegmentKind::Media {
            return None;
        }
        let init = self.records.get(descriptor.init_key.as_ref()?)?;
        let SegmentState::TerminalFailed { error, .. } = &init.state else {
            return None;
        };
        let fmp4 = match descriptor.meta.container() {
            Some(container) => container == ContainerKind::Fmp4,
            None => init.governs_fmp4,
        };
        fmp4.then(|| error.clone())
    }

    fn update_size_estimate(&mut self, actual: u64) {
        if actual == 0 {
            return;
        }
        // EMA (alpha = 1/4): responsive enough to track bitrate shifts without
        // letting one outlier whipsaw admission.
        self.size_estimate = (self.size_estimate * 3 + actual) / 4;
        self.size_estimate = self.size_estimate.max(1);
    }
}

fn encryption_fetch_eq(a: &SegmentDescriptor, b: &SegmentDescriptor) -> bool {
    match (&a.encryption, &b.encryption) {
        (None, None) => true,
        (Some(ea), Some(eb)) => ea.key_fetch_url == eb.key_fetch_url,
        _ => false,
    }
}
