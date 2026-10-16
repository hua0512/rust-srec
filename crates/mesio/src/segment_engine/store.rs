//! The reactor-owned segment state store.
//!
//! Single authoritative home for identity dedup, lifecycle state, retry
//! budget, and scheduling priority. Owned by the reactor task and never shared
//! — not an `Arc<Mutex<..>>`. The `HashMap` is the single source of truth; the
//! ready index and retry heap are advisory and re-validated on pop (lazy
//! tombstones), because an entry goes stale when its record changes state or
//! is removed while its key still sits in an index.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};
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

/// How far past the emit cursor media is admitted, for a stream that waits
/// for every segment. The assembler skips the segment it waits for once its
/// reorder buffer holds `max_segments` segments or `max_bytes` bytes, so
/// admission keeps what can pile up behind that segment below both.
#[derive(Debug, Clone, Copy)]
pub struct MediaWindow {
    /// The next MSN the assembler emits or skips.
    pub cursor: u64,
    /// Media at or beyond `cursor + segments` waits (0 = no limit).
    pub segments: u64,
    /// Media waits while the media at or beyond the cursor, downloading or
    /// complete, would exceed this many bytes (0 = no limit). The estimate
    /// of a segment still downloading may be off, so this stays below the
    /// assembler's own byte limit.
    pub bytes: u64,
}

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
                let stale_entry = match record.state {
                    SegmentState::Completed | SegmentState::TerminalFailed { .. } => {
                        // Never fetched again; nothing volatile to refresh.
                        // An init's MSN does move forward, though: it records
                        // where the map still applies, which is what keeps a
                        // long-lived init from being trimmed while in use.
                        if key.kind == SegmentKind::Init && descriptor.msn > record.descriptor.msn {
                            record.descriptor = Arc::new(descriptor);
                        }
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
                        record.descriptor = Arc::new(descriptor);
                        if url_changed {
                            record.generation += 1;
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
                self.records.insert(
                    key.clone(),
                    SegmentRecord {
                        descriptor: Arc::new(descriptor),
                        state: SegmentState::Discovered,
                        reschedules: 0,
                        throttled_since: None,
                        awaiting_refresh: false,
                        refresh_wait_used: false,
                        generation: 0,
                        attempt_generation: 0,
                        ready_entry: None,
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
    /// for a later call. Inits are always admitted: media waits for them.
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

        while jobs.len() < slots {
            let Some(entry) = self.ready.first().cloned() else {
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
            // Ready media follows every init in MSN order (prefetch, newer
            // still, last), so no later entry is below the limit either.
            let is_media = record.descriptor.key.kind == SegmentKind::Media;
            if is_media
                && window.is_some_and(|window| {
                    window.segments > 0
                        && record.descriptor.msn >= window.cursor.saturating_add(window.segments)
                })
            {
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
                    // Rate limiting says the server is busy, not that the
                    // segment is bad: it is retried once the cool-down ends,
                    // for as long as the throttling bound allows. A live
                    // segment that leaves the window meanwhile is dropped
                    // with the backlog and skipped (see `prune_below`).
                    // The bound is wall-clock time since the first refusal, so
                    // it also covers short cool-downs and time spent queued.
                    // The retry is clamped to the rest of the allowance, so a
                    // long cool-down near its end cannot carry it past it.
                    FailureClass::RateLimited { delay, .. } => {
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
                    // A signed URL can expire while its segment waits out
                    // rate limiting, and the playlist that would refresh it
                    // may not have been published yet when the cool-down
                    // ends. So once per segment, a denial after rate
                    // limiting waits for re-discovery (see `ingest`), for
                    // at most `REFRESH_WAIT`. Output keeps waiting for it
                    // within the same throttling bound, so a live gap skip
                    // does not drop it while the fresh URL is on its way.
                    FailureClass::Http(401 | 403)
                        if record.generation <= record.attempt_generation
                            && record.throttled_since.is_some()
                            && !record.refresh_wait_used =>
                    {
                        record.refresh_wait_used = true;
                        record.awaiting_refresh = true;
                        keeps_hold = true;
                        Some(REFRESH_WAIT)
                    }
                    // Budget contention says nothing about the segment: it
                    // only means other downloads hold the budget right now,
                    // so it does not spend the retry budget. It still ends:
                    // a size that can never fit is terminal at the fetch,
                    // contention clears as siblings finish, and a live
                    // backlog is bounded.
                    FailureClass::OverBudget => Some(backoff(record.reschedules)),
                    _ => {
                        let retryable = match class {
                            // 425 Too Early: an origin that enforces live
                            // availability (DASH) rejects a request that
                            // lands before the segment is published on its
                            // clock; it is published moments later.
                            FailureClass::Http(404 | 425 | 429)
                            | FailureClass::Http(500..=599)
                            | FailureClass::Network
                            | FailureClass::Timeout
                            | FailureClass::Decode => true,
                            // Auth failures are conditionally retryable: a
                            // signed URL can expire mid-flight while a newer
                            // playlist already refreshed it. Retry only when
                            // re-discovery advanced the generation past the
                            // one this attempt fetched with — otherwise the
                            // denial is real.
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
                        (retryable && record.reschedules < self.config.retry_budget).then(|| {
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
                    // Media and init terminal failures are both surfaced; the
                    // assembler decides whether the stream can continue (see
                    // `SequenceAssembler`).
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

    /// Prune for long-running live streams under one invariant: **never evict
    /// an entry whose key can still appear in the playlist window** — evicting
    /// a `Completed` record still in the window would make the next refresh
    /// re-download it. Only finished records below `window_start_msn` are
    /// eligible, and init records are retained up to `max_retained_inits`
    /// (newest first). `max_state_entries` is a backstop within that rule:
    /// when nothing is safely evictable, state temporarily exceeds the cap.
    ///
    /// Unfinished media below the window (discovered, queued, or awaiting a
    /// retry) is the backlog: a segment that slid out of the playlist is
    /// usually still served for a while, so it is kept and fetched rather
    /// than lost. The backlog is bounded to one window's worth
    /// (`window_len`) and by `max_state_entries`; beyond that, downloads are
    /// persistently slower than the stream and keeping more only grows memory
    /// and lag. The oldest entries go first, and each dropped MSN is returned
    /// as a `Skipped` input — the assembler waits on every planned MSN, so it
    /// must be told. In-flight work and unfinished inits are never dropped.
    pub fn prune_below(&mut self, window_start_msn: u64, window_len: usize) -> Vec<AssemblerInput> {
        self.records.retain(|_, record| {
            record.state.is_unfinished()
                || record.descriptor.key.kind == SegmentKind::Init
                || record.descriptor.msn >= window_start_msn
        });
        let dropped = self.drop_excess_backlog(window_start_msn, window_len);

        // Retain only the newest N init records, but never evict one that can
        // still appear in the window (msn >= window_start_msn) — that would
        // make the next refresh re-discover and re-download it, breaking the
        // same prune invariant the media retain above upholds. Unfinished
        // inits are never evicted either: dependent media is gated on them.
        // Nor is a failed init that unfinished media needs: it keeps that
        // media from being fetched (see `failed_init_of`).
        let needed: HashSet<&SegmentKey> = self
            .records
            .values()
            .filter(|r| r.state.is_unfinished())
            .filter_map(|r| r.descriptor.init_key.as_ref())
            .collect();
        let mut init_msns: Vec<(u64, SegmentKey)> = self
            .records
            .iter()
            .filter(|(key, r)| {
                r.descriptor.key.kind == SegmentKind::Init
                    && !r.state.is_unfinished()
                    && r.descriptor.msn < window_start_msn
                    && !(matches!(r.state, SegmentState::TerminalFailed { .. })
                        && needed.contains(key))
            })
            .map(|(k, r)| (r.descriptor.msn, k.clone()))
            .collect();
        if init_msns.len() > self.config.max_retained_inits {
            init_msns.sort_by_key(|(msn, _)| *msn);
            let excess = init_msns.len() - self.config.max_retained_inits;
            for (_, key) in init_msns.into_iter().take(excess) {
                self.records.remove(&key);
            }
        }

        if self.records.len() > self.config.max_state_entries {
            // Everything still here is in-window, in-flight, or a retained
            // init — not safely evictable. Honoring the invariant beats the
            // cap (see StoreConfig::max_state_entries).
            debug!(
                records = self.records.len(),
                cap = self.config.max_state_entries,
                "state store over capacity; all remaining entries are protected"
            );
        }
        // Ready/heap entries for pruned keys are dropped lazily on pop.
        dropped
    }

    fn drop_excess_backlog(
        &mut self,
        window_start_msn: u64,
        window_len: usize,
    ) -> Vec<AssemblerInput> {
        let mut backlog: Vec<(u64, SegmentKey)> = self
            .records
            .iter()
            .filter(|(_, r)| {
                r.descriptor.key.kind == SegmentKind::Media
                    && r.descriptor.msn < window_start_msn
                    && r.state.is_unfinished()
                    && !matches!(r.state, SegmentState::InFlight)
            })
            .map(|(key, r)| (r.descriptor.msn, key.clone()))
            .collect();
        let over_state_cap = self
            .records
            .len()
            .saturating_sub(self.config.max_state_entries);
        let excess = backlog
            .len()
            .saturating_sub(window_len)
            .max(over_state_cap)
            .min(backlog.len());
        if excess == 0 {
            return Vec::new();
        }

        backlog.sort_unstable_by_key(|(msn, _)| *msn);
        let mut dropped_msns = Vec::with_capacity(excess);
        for (msn, key) in backlog.into_iter().take(excess) {
            self.records.remove(&key);
            self.rate_limit_holds.release(&key, msn, Instant::now());
            dropped_msns.push(msn);
        }
        // Another record can share an MSN (a URI change re-keys a segment);
        // that MSN is still pending, so it must not be declared dead.
        let live_msns: std::collections::HashSet<u64> = self
            .records
            .values()
            .filter(|r| r.descriptor.key.kind == SegmentKind::Media)
            .map(|r| r.descriptor.msn)
            .collect();
        dropped_msns.retain(|msn| !live_msns.contains(msn));
        dropped_msns.dedup();
        warn!(
            dropped = excess,
            window_start_msn, "download backlog exceeds its bound; skipping the oldest segments"
        );

        let mut inputs = Vec::new();
        let mut run: Option<(u64, u64)> = None;
        for msn in dropped_msns {
            run = match run {
                Some((from, to)) if msn == to + 1 => Some((from, msn)),
                Some((from, to)) => {
                    inputs.push(AssemblerInput::Skipped {
                        from_msn: from,
                        to_msn: to,
                    });
                    Some((msn, msn))
                }
                None => Some((msn, msn)),
            };
        }
        if let Some((from_msn, to_msn)) = run {
            inputs.push(AssemblerInput::Skipped { from_msn, to_msn });
        }
        inputs
    }
}

fn encryption_fetch_eq(a: &SegmentDescriptor, b: &SegmentDescriptor) -> bool {
    match (&a.encryption, &b.encryption) {
        (None, None) => true,
        (Some(ea), Some(eb)) => ea.key_fetch_url == eb.key_fetch_url,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segment_engine::descriptor::SegmentMeta;
    use crate::segment_engine::identity::ByteRangeKey;
    use bytes::Bytes;
    use url::Url;

    fn budget_unlimited() -> ByteBudget {
        ByteBudget::new(0, 0)
    }

    fn descriptor(uri: &str, msn: u64, kind: SegmentKind) -> SegmentDescriptor {
        descriptor_with_source(uri, msn, kind, SegmentSource::Playlist)
    }

    fn descriptor_with_source(
        uri: &str,
        msn: u64,
        kind: SegmentKind,
        source: SegmentSource,
    ) -> SegmentDescriptor {
        let url = Url::parse(uri).expect("test url");
        SegmentDescriptor {
            key: SegmentKey {
                kind,
                uri: Arc::from(uri),
                byte_range: None,
                scope: None,
            },
            msn,
            source,
            parsed_url: Arc::new(url),
            discontinuity: false,
            encryption: None,
            init_key: None,
            meta: SegmentMeta::Hls(Arc::new(m3u8_rs::MediaSegment {
                uri: uri.to_string(),
                duration: 2.0,
                ..Default::default()
            })),
        }
    }

    fn payload_for(d: &Arc<SegmentDescriptor>) -> SegmentPayload {
        SegmentPayload::Ts {
            data: Bytes::from_static(b"data"),
            descriptor: Arc::clone(d),
        }
    }

    fn store() -> SegmentStateStore {
        SegmentStateStore::new(
            StoreConfig {
                retry_budget: 2,
                retry_delay_base: Duration::from_millis(10),
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        )
    }

    fn take_one(store: &mut SegmentStateStore, budget: &ByteBudget) -> Option<ReadyJob> {
        let (mut jobs, inputs) = store.next_ready_jobs(1, Instant::now(), budget);
        assert!(inputs.is_empty(), "unexpected admission terminals");
        jobs.pop()
    }

    #[test]
    fn ingest_then_schedule_marks_in_flight_and_dedups() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );

        let job = take_one(&mut s, &b).expect("schedulable");
        assert_eq!(job.descriptor.msn, 1);

        // While in flight (and after), the same key never schedules again.
        assert!(take_one(&mut s, &b).is_none());
        let stats = s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        assert_eq!(stats.discovered, 0);
        assert_eq!(stats.refreshed, 1);
        assert!(take_one(&mut s, &b).is_none());
    }

    #[test]
    fn priority_orders_init_media_prefetch_then_msn() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![
                descriptor_with_source(
                    "https://e.com/pre.ts",
                    1,
                    SegmentKind::Media,
                    SegmentSource::PlaylistPrefetch,
                ),
                descriptor("https://e.com/m5.ts", 5, SegmentKind::Media),
                descriptor("https://e.com/m3.ts", 3, SegmentKind::Media),
                descriptor("https://e.com/init.mp4", 3, SegmentKind::Init),
            ],
            Instant::now(),
        );

        let order: Vec<(SegmentKind, u64)> = std::iter::from_fn(|| take_one(&mut s, &b))
            .map(|j| (j.descriptor.key.kind, j.descriptor.msn))
            .collect();
        assert_eq!(
            order,
            vec![
                (SegmentKind::Init, 3),
                (SegmentKind::Media, 3),
                (SegmentKind::Media, 5),
                (SegmentKind::Media, 1), // prefetch ranks last despite lowest MSN
            ]
        );
    }

    #[test]
    fn media_at_or_beyond_the_limit_waits_while_inits_are_admitted() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![
                descriptor("https://e.com/m4.ts", 4, SegmentKind::Media),
                descriptor("https://e.com/m3.ts", 3, SegmentKind::Media),
                descriptor("https://e.com/init.mp4", 9, SegmentKind::Init),
                descriptor("https://e.com/m5.ts", 5, SegmentKind::Media),
            ],
            Instant::now(),
        );
        let admitted = |s: &mut SegmentStateStore, limit| {
            let (jobs, inputs) = s.next_ready_jobs_within(
                10,
                Instant::now(),
                &b,
                Some(MediaWindow {
                    cursor: 0,
                    segments: limit,
                    bytes: 0,
                }),
            );
            assert!(inputs.is_empty());
            jobs.into_iter()
                .map(|job| (job.descriptor.key.kind, job.descriptor.msn))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            admitted(&mut s, 4),
            [(SegmentKind::Init, 9), (SegmentKind::Media, 3)]
        );
        assert_eq!(admitted(&mut s, 4), []);
        assert_eq!(
            admitted(&mut s, 6),
            [(SegmentKind::Media, 4), (SegmentKind::Media, 5)]
        );
    }

    #[test]
    fn retryable_failure_waits_for_deadline_then_reschedules() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let job = take_one(&mut s, &b).expect("schedulable");
        let key = job.descriptor.key.clone();

        let now = Instant::now();
        let out = s.apply_outcome(
            SegmentOutcome::Failed {
                key: key.clone(),
                msn: 1,
                class: FailureClass::Http(404),
                reason: Arc::from("404"),
                error: None,
            },
            now,
        );
        assert!(
            out.assembler_inputs.is_empty(),
            "retryable failure emits nothing yet"
        );
        let notice = out.retry_notice.expect("retry notice should be surfaced");
        assert_eq!(notice.attempt, 1);
        assert_eq!(notice.reason.as_ref(), "404");
        assert_eq!(notice.delay, Duration::from_millis(10));
        assert!(s.has_unfinished_work());

        // Before the deadline: not schedulable.
        let (jobs, _) = s.next_ready_jobs(1, now, &b);
        assert!(jobs.is_empty());

        // After the deadline: schedulable again.
        let later = now + Duration::from_millis(50);
        let (jobs, _) = s.next_ready_jobs(1, later, &b);
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn too_early_is_retried_like_not_found() {
        let mut s = store();
        let b = budget_unlimited();
        let now = Instant::now();
        s.ingest(
            vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
            now,
        );
        let job = take_one(&mut s, &b).expect("schedulable");
        let out = s.apply_outcome(
            SegmentOutcome::Failed {
                key: job.descriptor.key.clone(),
                msn: 1,
                class: FailureClass::Http(425),
                reason: Arc::from("425"),
                error: None,
            },
            now,
        );
        assert!(out.assembler_inputs.is_empty(), "425 is not terminal");
        assert_eq!(out.retry_notice.expect("retry scheduled").attempt, 1);
    }

    #[test]
    fn retry_budget_exhaustion_terminalizes_and_informs_assembler() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let mut now = Instant::now();
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();

        for attempt in 0..3 {
            let out = s.apply_outcome(
                SegmentOutcome::Failed {
                    key: key.clone(),
                    msn: 1,
                    class: FailureClass::Network,
                    reason: Arc::from("reset"),
                    error: None,
                },
                now,
            );
            if attempt < 2 {
                assert!(out.assembler_inputs.is_empty());
                assert!(out.retry_notice.is_some());
                now += Duration::from_secs(60);
                let (jobs, _) = s.next_ready_jobs(1, now, &b);
                assert_eq!(jobs.len(), 1, "attempt {attempt} reschedules");
            } else {
                assert!(matches!(
                    out.assembler_inputs.as_slice(),
                    [AssemblerInput::TerminalFailed { msn: 1, .. }]
                ));
                assert!(out.retry_notice.is_none());
            }
        }
        assert!(!s.has_unfinished_work());
        // Terminal failures are never rescheduled.
        now += Duration::from_secs(60);
        let (jobs, _) = s.next_ready_jobs(1, now, &b);
        assert!(jobs.is_empty());
    }

    #[test]
    fn auth_failure_with_fresh_url_retries_without_fresh_url_terminalizes() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();

        // Re-discovery with a rotated fetch URL while the attempt is in
        // flight: same identity (FullUrl policy would fork — emulate a
        // token-aware policy by reusing the key with a new parsed_url).
        let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
        refreshed.parsed_url = Arc::new(Url::parse("https://e.com/1.ts?token=fresh").unwrap());
        s.ingest(vec![refreshed], Instant::now());

        let out = s.apply_outcome(
            SegmentOutcome::Failed {
                key: key.clone(),
                msn: 1,
                class: FailureClass::Http(403),
                reason: Arc::from("403"),
                error: None,
            },
            Instant::now(),
        );
        assert!(
            out.assembler_inputs.is_empty(),
            "stale-URL 403 must reschedule"
        );
        assert!(out.retry_notice.is_some());

        // Retry runs with the freshest URL and 403s again -> terminal.
        let later = Instant::now() + Duration::from_secs(60);
        let (jobs, _) = s.next_ready_jobs(1, later, &b);
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].descriptor.parsed_url.as_str(),
            "https://e.com/1.ts?token=fresh"
        );
        let out = s.apply_outcome(
            SegmentOutcome::Failed {
                key,
                msn: 1,
                class: FailureClass::Http(403),
                reason: Arc::from("403"),
                error: None,
            },
            later,
        );
        assert!(matches!(
            out.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
        assert!(out.retry_notice.is_none());
    }

    #[test]
    fn rediscovery_refreshes_url_for_pending_retry() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
        let now = Instant::now();
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: key.clone(),
                msn: 1,
                class: FailureClass::Http(404),
                reason: Arc::from("404"),
                error: None,
            },
            now,
        );

        let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
        refreshed.parsed_url = Arc::new(Url::parse("https://e.com/1.ts?token=v2").unwrap());
        s.ingest(vec![refreshed], now);

        let (jobs, _) = s.next_ready_jobs(1, now + Duration::from_secs(60), &b);
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].descriptor.parsed_url.as_str(),
            "https://e.com/1.ts?token=v2",
            "retry must use the refreshed URL, never the stale one"
        );
    }

    fn http_failure(key: &SegmentKey, msn: u64, status: u16, url: &str) -> SegmentOutcome {
        SegmentOutcome::Failed {
            key: key.clone(),
            msn,
            class: FailureClass::Http(status),
            reason: Arc::from(format!("HTTP {status}")),
            error: Some(DownloadError::http_status(
                StatusCode::from_u16(status).unwrap(),
                url,
                "segment download",
            )),
        }
    }

    fn failed_url(input: &AssemblerInput) -> Option<String> {
        match input {
            AssemblerInput::TerminalFailed { error, .. } => match error.get() {
                DownloadError::HttpStatus { url, .. } => Some(url.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    #[test]
    fn a_terminal_http_failure_names_the_url_that_was_requested() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
        // A refresh replaces the URL while the attempt is in flight.
        let mut refreshed = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
        refreshed.parsed_url = Arc::new(Url::parse("https://cdn2.e.com/1.ts").unwrap());
        s.ingest(vec![refreshed], Instant::now());
        let out = s.apply_outcome(
            http_failure(&key, 1, 410, "https://e.com/1.ts"),
            Instant::now(),
        );
        assert_eq!(
            out.assembler_inputs
                .iter()
                .map(failed_url)
                .collect::<Vec<_>>(),
            [Some("https://e.com/1.ts".to_owned())]
        );
    }

    /// An init, and three media segments that need it.
    fn init_with_media(s: &mut SegmentStateStore) -> SegmentKey {
        let init = descriptor("https://e.com/init.mp4", 1, SegmentKind::Init);
        let init_key = init.key.clone();
        let media = (1..=3).map(|msn| {
            let mut media =
                descriptor(&format!("https://e.com/{msn}.m4s"), msn, SegmentKind::Media);
            media.init_key = Some(init_key.clone());
            media
        });
        s.ingest(std::iter::once(init).chain(media).collect(), Instant::now());
        init_key
    }

    #[test]
    fn fmp4_media_of_a_failed_init_fails_with_it_unfetched() {
        let mut s = store();
        let b = budget_unlimited();
        let init_key = init_with_media(&mut s);
        let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
        let media = Arc::clone(&jobs[1].descriptor);
        // The first media shows the stream is fMP4 (HLS does not declare it).
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: media.key.clone(),
                msn: 1,
                payload: SegmentPayload::Mp4Media {
                    data: Bytes::from_static(b"m"),
                    descriptor: media,
                },
            },
            Instant::now(),
        );
        let out = s.apply_outcome(
            http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
            Instant::now(),
        );
        assert_eq!(out.assembler_inputs.len(), 1);

        let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
        assert!(jobs.is_empty(), "undecodable media is not fetched");
        assert!(
            inputs.iter().all(|input| matches!(
                input,
                AssemblerInput::TerminalFailed {
                    init_failed: true,
                    ..
                }
            )),
            "{inputs:?}"
        );
        assert_eq!(
            inputs.iter().map(failed_url).collect::<Vec<_>>(),
            vec![Some("https://e.com/init.mp4".to_owned()); 2]
        );
        assert!(!s.has_unfinished_work());
    }

    #[test]
    fn fmp4_media_of_another_map_says_nothing_about_a_failed_map() {
        let mut s = store();
        let b = budget_unlimited();
        // Map A's media arrives as fMP4.
        let map_a = descriptor("https://e.com/a.mp4", 1, SegmentKind::Init);
        let mut media_a = descriptor("https://e.com/1.m4s", 1, SegmentKind::Media);
        media_a.init_key = Some(map_a.key.clone());
        s.ingest(vec![map_a, media_a], Instant::now());
        let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
        let media = Arc::clone(&jobs[1].descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: media.key.clone(),
                msn: 1,
                payload: SegmentPayload::Mp4Media {
                    data: Bytes::from_static(b"m"),
                    descriptor: media,
                },
            },
            Instant::now(),
        );
        // Map B fails before any of its media arrived: they are fetched.
        let map_b = descriptor("https://e.com/b.mp4", 2, SegmentKind::Init);
        let map_b_key = map_b.key.clone();
        let mut media_b = descriptor("https://e.com/2.ts", 2, SegmentKind::Media);
        media_b.init_key = Some(map_b_key.clone());
        s.ingest(vec![map_b, media_b], Instant::now());
        take_one(&mut s, &b).unwrap();
        s.apply_outcome(
            http_failure(&map_b_key, 2, 400, "https://e.com/b.mp4"),
            Instant::now(),
        );
        let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
        assert!(inputs.is_empty(), "{inputs:?}");
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn a_failed_init_is_kept_while_media_needing_it_is_unfinished() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_budget: 2,
                retry_delay_base: Duration::from_millis(10),
                max_retained_inits: 0,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        let init_key = init_with_media(&mut s);
        let (jobs, _) = s.next_ready_jobs(3, Instant::now(), &b);
        let first = Arc::clone(&jobs[1].descriptor);
        let second = Arc::clone(&jobs[2].descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: first.key.clone(),
                msn: 1,
                payload: SegmentPayload::Mp4Media {
                    data: Bytes::from_static(b"m"),
                    descriptor: first,
                },
            },
            Instant::now(),
        );
        // The second media waits on a retry when its init fails.
        let now = Instant::now();
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: second.key.clone(),
                msn: 2,
                class: FailureClass::Network,
                reason: Arc::from("reset"),
                error: None,
            },
            now,
        );
        s.apply_outcome(
            http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
            now,
        );
        // The window moves past them all.
        s.prune_below(10, 1);
        let (jobs, inputs) = s.next_ready_jobs(10, now + Duration::from_secs(60), &b);
        assert!(jobs.is_empty(), "undecodable media is not fetched");
        assert!(
            inputs.iter().all(|input| matches!(
                input,
                AssemblerInput::TerminalFailed {
                    init_failed: true,
                    ..
                }
            )),
            "{inputs:?}"
        );
        assert!(!inputs.is_empty());
    }

    #[test]
    fn ts_media_of_a_failed_map_is_still_fetched() {
        let mut s = store();
        let b = budget_unlimited();
        let init_key = init_with_media(&mut s);
        let (jobs, _) = s.next_ready_jobs(2, Instant::now(), &b);
        let media = Arc::clone(&jobs[1].descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: media.key.clone(),
                msn: 1,
                payload: payload_for(&media),
            },
            Instant::now(),
        );
        s.apply_outcome(
            http_failure(&init_key, 1, 400, "https://e.com/init.mp4"),
            Instant::now(),
        );
        let (jobs, inputs) = s.next_ready_jobs(10, Instant::now(), &b);
        assert!(inputs.is_empty(), "{inputs:?}");
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn prefetch_upgrade_keeps_one_key_and_raises_priority() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![
                descriptor_with_source(
                    "https://e.com/p.ts",
                    10,
                    SegmentKind::Media,
                    SegmentSource::PlaylistPrefetch,
                ),
                descriptor("https://e.com/m.ts", 11, SegmentKind::Media),
            ],
            Instant::now(),
        );
        // Prefetch reappears as normal media: same key, upgraded source.
        let stats = s.ingest(
            vec![descriptor("https://e.com/p.ts", 10, SegmentKind::Media)],
            Instant::now(),
        );
        assert_eq!(stats.discovered, 0, "no second key for the same resource");
        assert_eq!(stats.refreshed, 1);

        // After upgrade it schedules by MSN order among playlist media.
        let first = take_one(&mut s, &b).unwrap();
        assert_eq!(first.descriptor.msn, 10);
        assert_eq!(first.descriptor.source, SegmentSource::Playlist);
    }

    #[test]
    fn media_beyond_the_byte_window_waits_but_the_cursor_segment_is_always_admitted() {
        const MIB: u64 = 1024 * 1024;
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            (0..6)
                .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
                .collect(),
            Instant::now(),
        );
        let admit = |s: &mut SegmentStateStore, cursor: u64| -> Vec<u64> {
            let window = MediaWindow {
                cursor,
                segments: 0,
                bytes: 5 * MIB,
            };
            let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
            jobs.into_iter()
                .map(|job| job.descriptor.msn)
                .collect::<Vec<_>>()
        };

        // Two 2 MiB estimates fit; a third would pass 5 MiB.
        assert_eq!(admit(&mut s, 0), [0, 1]);
        assert_eq!(admit(&mut s, 0), Vec::<u64>::new());
        // The cursor moving past 0 releases its bytes.
        assert_eq!(admit(&mut s, 1), [2]);
        assert_eq!(admit(&mut s, 1), Vec::<u64>::new());
        // A cursor at a segment held back by the bytes admits it regardless.
        assert_eq!(admit(&mut s, 3), [3, 4]);
        assert_eq!(admit(&mut s, 5), [5]);
    }

    #[test]
    fn activating_a_byte_window_accounts_for_media_admitted_before_the_bound() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                fallback_size_estimate: 4,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        let now = Instant::now();
        s.ingest(
            (0..4)
                .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
                .collect(),
            now,
        );
        let unbounded = MediaWindow {
            cursor: 0,
            segments: 0,
            bytes: 0,
        };
        let (mut jobs, inputs) = s.next_ready_jobs_within(2, now, &b, Some(unbounded));
        assert!(inputs.is_empty());
        let second = jobs.pop().expect("second unbounded admission");
        let first = jobs.pop().expect("first unbounded admission");
        let payload = payload_for(&second.descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: second.descriptor.key.clone(),
                msn: 1,
                payload,
            },
            now,
        );

        // The missing cursor's in-flight reservation and the later completed
        // payload already fill this byte window before its gate is enabled.
        let bounded = MediaWindow {
            bytes: 8,
            ..unbounded
        };
        assert!(
            s.next_ready_jobs_within(4, now, &b, Some(bounded))
                .0
                .is_empty()
        );

        let payload = payload_for(&first.descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: first.descriptor.key.clone(),
                msn: 0,
                payload,
            },
            now,
        );
        let advanced = MediaWindow {
            cursor: 2,
            ..bounded
        };
        let (jobs, inputs) = s.next_ready_jobs_within(4, now, &b, Some(advanced));
        assert!(inputs.is_empty());
        assert_eq!(
            jobs.iter()
                .map(|job| job.descriptor.msn)
                .collect::<Vec<_>>(),
            [2, 3]
        );
    }

    #[test]
    fn a_failed_attempt_gives_back_its_bytes_in_the_window() {
        const MIB: u64 = 1024 * 1024;
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            (0..3)
                .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
                .collect(),
            Instant::now(),
        );
        let window = MediaWindow {
            cursor: 0,
            segments: 0,
            bytes: 3 * MIB,
        };
        let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
        assert_eq!(jobs.len(), 1, "one 2 MiB estimate fits in 3 MiB");
        let job = jobs.into_iter().next().unwrap();
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: job.descriptor.key.clone(),
                msn: 0,
                class: FailureClass::Network,
                reason: Arc::from("reset"),
                error: None,
            },
            Instant::now(),
        );
        // Segment 0 retries later; meanwhile its failed attempt holds nothing.
        let window = MediaWindow {
            cursor: 1,
            ..window
        };
        let (jobs, _) = s.next_ready_jobs_within(10, Instant::now(), &b, Some(window));
        assert_eq!(
            jobs.iter()
                .map(|job| job.descriptor.msn)
                .collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn completed_payload_flows_and_updates_estimate() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let job = take_one(&mut s, &b).unwrap();
        let before = s.current_size_estimate();
        let out = s.apply_outcome(
            SegmentOutcome::Completed {
                key: job.descriptor.key.clone(),
                msn: 1,
                payload: payload_for(&job.descriptor),
            },
            Instant::now(),
        );
        assert!(matches!(
            out.assembler_inputs.as_slice(),
            [AssemblerInput::Payload(_)]
        ));
        assert!(out.retry_notice.is_none());
        assert!(s.current_size_estimate() < before, "EMA moved toward 4B");
        assert!(!s.has_unfinished_work());
    }

    #[test]
    fn admission_reserves_download_bytes_and_stops_at_budget() {
        let mut s = store();
        // Estimate fallback is 2 MiB; budget fits exactly one.
        let b = ByteBudget::new(3 * 1024 * 1024, 0);
        s.ingest(
            vec![
                descriptor("https://e.com/1.ts", 1, SegmentKind::Media),
                descriptor("https://e.com/2.ts", 2, SegmentKind::Media),
            ],
            Instant::now(),
        );
        let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
        assert!(inputs.is_empty());
        assert_eq!(jobs.len(), 1, "second admission must fail the byte gate");
        drop(jobs);
        // Reservation released: the second segment becomes admissible.
        let (jobs, _) = s.next_ready_jobs(8, Instant::now(), &b);
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn estimate_over_capacity_clamps_instead_of_terminalizing() {
        let mut s = store();
        let b = ByteBudget::new(1024, 0); // capacity below the 2 MiB estimate
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        // The estimate is a guess, not knowledge: the segment admits with a
        // capacity-clamped reservation and the body checks decide from real
        // sizes.
        let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
        assert!(inputs.is_empty());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].reservation.held_bytes(), 1024);
    }

    #[test]
    fn known_byterange_over_capacity_terminalizes_as_oversize() {
        let mut s = store();
        let b = ByteBudget::new(1024, 0);
        let mut d = descriptor("https://e.com/all.ts", 1, SegmentKind::Media);
        d.key.byte_range = Some(ByteRangeKey {
            length: 4096, // provably larger than the whole budget
            offset: 0,
        });
        s.ingest(vec![d], Instant::now());
        let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
        assert!(jobs.is_empty());
        assert!(matches!(
            inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
        assert!(!s.has_unfinished_work());
    }

    #[test]
    fn admission_terminalizations_are_capped_per_pass() {
        let mut s = store();
        let b = ByteBudget::new(1024, 0);
        // 8 segments, every one provably oversize.
        let descriptors: Vec<_> = (0..8u64)
            .map(|i| {
                let mut d = descriptor(&format!("https://e.com/{i}.ts"), i, SegmentKind::Media);
                d.key.byte_range = Some(ByteRangeKey {
                    length: 4096,
                    offset: 0,
                });
                d
            })
            .collect();
        s.ingest(descriptors, Instant::now());

        // One pass with 2 slots may terminalize at most 2; the rest stay
        // schedulable for later passes (producer suspension, not flooding).
        let (jobs, inputs) = s.next_ready_jobs(2, Instant::now(), &b);
        assert!(jobs.is_empty());
        assert_eq!(inputs.len(), 2);
        assert!(s.has_unfinished_work());

        let (_, inputs) = s.next_ready_jobs(2, Instant::now(), &b);
        assert_eq!(inputs.len(), 2);
    }

    #[test]
    fn due_retry_promotion_clears_deadline_even_without_admission() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
        let now = Instant::now();
        s.apply_outcome(
            SegmentOutcome::Failed {
                key,
                msn: 1,
                class: FailureClass::Timeout,
                reason: Arc::from("t"),
                error: None,
            },
            now,
        );
        assert!(s.next_retry_deadline().is_some());

        // After the deadline elapses, promotion alone (no admission — e.g.
        // the reactor's dispatch gate is closed) must drain the heap so the
        // reactor's retry-wake arm goes inert instead of firing forever.
        let later = now + Duration::from_secs(60);
        s.promote_due_retries(later);
        assert!(s.next_retry_deadline().is_none());
        assert!(s.has_unfinished_work(), "promoted entry is Queued work");
    }

    #[test]
    fn byterange_estimate_uses_exact_length() {
        let mut s = store();
        let b = ByteBudget::new(1024, 0);
        let mut d = descriptor("https://e.com/all.ts", 1, SegmentKind::Media);
        d.key.byte_range = Some(ByteRangeKey {
            length: 100,
            offset: 0,
        });
        s.ingest(vec![d], Instant::now());
        let (jobs, inputs) = s.next_ready_jobs(1, Instant::now(), &b);
        assert!(inputs.is_empty());
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].reservation.held_bytes(), 100);
    }

    #[test]
    fn prune_keeps_window_inflight_and_inits() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![
                descriptor("https://e.com/old.ts", 1, SegmentKind::Media),
                descriptor("https://e.com/init.mp4", 1, SegmentKind::Init),
                descriptor("https://e.com/cur.ts", 10, SegmentKind::Media),
                descriptor("https://e.com/run.ts", 2, SegmentKind::Media),
            ],
            Instant::now(),
        );
        // Put msn=1 media into Completed, msn=2 in flight (init is admitted
        // first by priority, complete it too).
        loop {
            let Some(job) = take_one(&mut s, &b) else {
                break;
            };
            let msn = job.descriptor.msn;
            let kind = job.descriptor.key.kind;
            if (msn == 1 || kind == SegmentKind::Init) && msn < 10 {
                s.apply_outcome(
                    SegmentOutcome::Completed {
                        key: job.descriptor.key.clone(),
                        msn,
                        payload: payload_for(&job.descriptor),
                    },
                    Instant::now(),
                );
            }
            // msn 2 and 10 left in flight
        }

        s.prune_below(10, 10);

        // msn=1 Completed media evicted (below window); init kept; in-flight
        // msn=2 kept; msn=10 kept.
        assert!(
            !s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/old.ts")
        );
        assert!(
            s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/init.mp4")
        );
        assert!(
            s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/run.ts")
        );
        assert!(
            s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/cur.ts")
        );
    }

    #[test]
    fn init_trim_never_evicts_in_window_inits() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_retained_inits: 2,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        // Three init keys: two below the window (msn 1, 2) and one in-window
        // (msn 10). With max_retained_inits=2 the trim must drop the two
        // below-window inits, never the in-window one.
        s.ingest(
            vec![
                descriptor("https://e.com/i1.mp4", 1, SegmentKind::Init),
                descriptor("https://e.com/i2.mp4", 2, SegmentKind::Init),
                descriptor("https://e.com/i10.mp4", 10, SegmentKind::Init),
            ],
            Instant::now(),
        );
        // Complete them so none are InFlight (InFlight is never evicted anyway).
        while let Some(job) = take_one(&mut s, &b) {
            s.apply_outcome(
                SegmentOutcome::Completed {
                    key: job.descriptor.key.clone(),
                    msn: job.descriptor.msn,
                    payload: payload_for(&job.descriptor),
                },
                Instant::now(),
            );
        }

        s.prune_below(10, 10);

        assert!(
            s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/i10.mp4"),
            "in-window init must survive the retention trim"
        );
    }

    #[test]
    fn completed_init_still_in_the_window_survives_the_retention_trim() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_retained_inits: 0,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/init.mp4", 1, SegmentKind::Init)],
            Instant::now(),
        );
        let job = take_one(&mut s, &b).expect("admitted");
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: job.descriptor.key.clone(),
                msn: 1,
                payload: payload_for(&job.descriptor),
            },
            Instant::now(),
        );
        // A later window still uses the same map.
        s.ingest(
            vec![descriptor("https://e.com/init.mp4", 20, SegmentKind::Init)],
            Instant::now(),
        );

        s.prune_below(15, 10);

        assert!(
            s.records.contains_key(&job.descriptor.key),
            "trimming it would make the next refresh re-download it"
        );
    }

    #[test]
    fn budget_contention_does_not_spend_the_retry_budget() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_budget: 1,
                retry_delay_base: Duration::ZERO,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/big.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let fail = |s: &mut SegmentStateStore, class| {
            let job = take_one(s, &b).expect("admitted");
            s.apply_outcome(
                SegmentOutcome::Failed {
                    key: job.descriptor.key.clone(),
                    msn: 1,
                    class,
                    reason: Arc::from("failed"),
                    error: None,
                },
                Instant::now(),
            )
        };

        for _ in 0..5 {
            let effects = fail(&mut s, FailureClass::OverBudget);
            assert!(effects.assembler_inputs.is_empty(), "still retrying");
        }
        // The one real retry is still available, then the segment ends.
        assert!(
            fail(&mut s, FailureClass::Timeout)
                .assembler_inputs
                .is_empty()
        );
        assert!(matches!(
            fail(&mut s, FailureClass::Timeout)
                .assembler_inputs
                .as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
    }

    #[test]
    fn rate_limiting_waits_out_the_cool_down_without_spending_the_retry_budget() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_budget: 1,
                retry_delay_base: Duration::from_millis(10),
                max_throttled: Duration::from_secs(60),
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let mut now = Instant::now();
        let fail = |s: &mut SegmentStateStore, class, now: Instant| {
            let (mut jobs, inputs) = s.next_ready_jobs(1, now, &b);
            assert!(inputs.is_empty());
            let job = jobs.pop().expect("admitted once due");
            s.apply_outcome(
                SegmentOutcome::Failed {
                    key: job.descriptor.key.clone(),
                    msn: 1,
                    class,
                    reason: Arc::from("HTTP 429"),
                    error: None,
                },
                now,
            )
        };
        let limited = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(20),
            held_back: false,
        };

        // Three cool-downs of 20 s fit the 60 s bound; the budget of one
        // ordinary retry is untouched.
        for _ in 0..3 {
            let effects = fail(&mut s, limited, now);
            assert!(effects.assembler_inputs.is_empty(), "still waiting");
            let notice = effects.retry_notice.expect("retry scheduled");
            assert_eq!(notice.delay, Duration::from_secs(20));
            assert_eq!(
                s.next_retry_deadline(),
                Some(now + Duration::from_secs(20)),
                "retried when the cool-down ends, not before"
            );
            assert!(s.next_ready_jobs(1, now, &b).0.is_empty());
            now += Duration::from_secs(20);
        }
        assert!(
            fail(&mut s, FailureClass::Timeout, now)
                .assembler_inputs
                .is_empty(),
            "the ordinary retry is still available"
        );
        now += Duration::from_secs(1);
        // A fourth cool-down would exceed the bound: the segment ends.
        let effects = fail(&mut s, limited, now);
        assert!(matches!(
            effects.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, error, .. }]
                if matches!(error.get(), DownloadError::SegmentFetch { retryable: true, .. })
        ));
    }

    fn fail_at(
        s: &mut SegmentStateStore,
        b: &ByteBudget,
        class: FailureClass,
        now: Instant,
    ) -> OutcomeEffects {
        let (mut jobs, inputs) = s.next_ready_jobs(1, now, b);
        assert!(inputs.is_empty());
        let job = jobs.pop().expect("admitted once due");
        s.apply_outcome(
            SegmentOutcome::Failed {
                key: job.descriptor.key.clone(),
                msn: job.descriptor.msn,
                class,
                reason: Arc::from("refused"),
                error: None,
            },
            now,
        )
    }

    #[test]
    fn rate_limit_holds_keep_the_first_deadline_until_output_consumes_the_payload() {
        let holds = Arc::new(RateLimitHolds::new());
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_throttled: Duration::from_secs(60),
                ..StoreConfig::default()
            },
            Arc::clone(&holds),
        );
        let b = budget_unlimited();
        let start = Instant::now();
        let descriptor = descriptor("https://e.com/1.m4s", 1, SegmentKind::Media);
        let key = descriptor.key.clone();
        s.ingest(vec![descriptor], start);
        let limited = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(20),
            held_back: true,
        };

        fail_at(&mut s, &b, limited, start);
        let until = start + Duration::from_secs(60);
        assert_eq!(holds.first_media_hold(1, 2, start), Some((1, until)));
        let second = start + Duration::from_secs(20);
        fail_at(&mut s, &b, limited, second);
        assert_eq!(holds.first_media_hold(1, 2, second), Some((1, until)));

        let now = start + Duration::from_secs(40);
        let job = s
            .next_ready_jobs(1, now, &b)
            .0
            .pop()
            .expect("retry admitted");
        assert_eq!(holds.first_media_hold(1, 2, now), Some((1, until)));
        let payload = payload_for(&job.descriptor);
        s.apply_outcome(
            SegmentOutcome::Completed {
                key: key.clone(),
                msn: 1,
                payload,
            },
            now,
        );
        s.prune_below(2, 1);
        // Completion may still sit behind payload backpressure. Only output
        // consumption, not removal of the finished record, releases its hold.
        assert_eq!(holds.first_media_hold(1, 2, now), Some((1, until)));
        holds.release(&key, 1, now);
        assert_eq!(holds.first_media_hold(1, 2, now), None);
    }

    #[test]
    fn ordinary_failure_and_backlog_pruning_release_rate_limit_holds() {
        let holds = Arc::new(RateLimitHolds::new());
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_budget: 2,
                retry_delay_base: Duration::from_millis(10),
                ..StoreConfig::default()
            },
            Arc::clone(&holds),
        );
        let b = budget_unlimited();
        let start = Instant::now();
        holds.set_scope(1, None, false, start);
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            start,
        );
        let limited = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(1),
            held_back: true,
        };
        fail_at(&mut s, &b, limited, start);
        assert!(holds.first_media_hold(1, 2, start).is_some());
        let now = start + Duration::from_secs(1);
        fail_at(&mut s, &b, FailureClass::Network, now);
        assert_eq!(holds.first_media_hold(1, 2, now), None);
        assert_eq!(holds.clock().paused_at(now), Duration::from_secs(1));
        assert_eq!(holds.clock().active_until(), None);
        assert_eq!(
            holds
                .clock()
                .deadline(start, Duration::ZERO, Duration::from_secs(10), now),
            start + Duration::from_secs(11),
        );

        let now = now + Duration::from_secs(1);
        fail_at(&mut s, &b, limited, now);
        assert!(holds.first_media_hold(1, 2, now).is_some());
        assert!(matches!(
            s.prune_below(2, 0).as_slice(),
            [AssemblerInput::Skipped {
                from_msn: 1,
                to_msn: 1
            }]
        ));
        assert_eq!(holds.first_media_hold(1, 2, now), None);
    }

    #[test]
    fn immediate_rate_limit_retries_still_end_at_the_bound() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_delay_base: Duration::ZERO,
                max_throttled: Duration::from_secs(60),
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
            Instant::now(),
        );
        // A server that keeps asking for an immediate retry.
        let refused = FailureClass::RateLimited {
            status: 429,
            delay: Duration::ZERO,
            held_back: false,
        };
        let start = Instant::now();
        let mut now = start;
        let mut retries = 0;
        loop {
            let effects = fail_at(&mut s, &b, refused, now);
            if !effects.assembler_inputs.is_empty() {
                break;
            }
            let delay = effects.retry_notice.expect("retry scheduled").delay;
            assert!(delay >= MIN_RATE_LIMITED_RETRY, "never a tight loop");
            now += delay;
            retries += 1;
            assert!(retries < 10_000, "rate limiting must end");
        }
        assert!(now.duration_since(start) >= Duration::from_secs(60));
        assert!(now.duration_since(start) < Duration::from_secs(61));
    }

    #[test]
    fn a_rate_limit_retry_never_lands_past_the_bound() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_throttled: Duration::from_secs(300),
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.m4s", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let refused = |seconds| FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(seconds),
            held_back: false,
        };
        let start = Instant::now();
        fail_at(&mut s, &b, refused(1), start);
        // A minute-long cool-down one second before the bound.
        let now = start + Duration::from_secs(299);
        let effects = fail_at(&mut s, &b, refused(60), now);
        assert_eq!(
            effects.retry_notice.expect("one more try").delay,
            Duration::from_secs(1)
        );
        assert_eq!(
            s.next_retry_deadline(),
            Some(start + Duration::from_secs(300))
        );
        let effects = fail_at(&mut s, &b, refused(60), start + Duration::from_secs(300));
        assert!(matches!(
            effects.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
    }

    #[test]
    fn a_url_denied_after_rate_limiting_waits_once_for_a_refresh() {
        let mut s = store();
        let b = budget_unlimited();
        let signed = |token: &str| {
            let mut d = descriptor("https://e.com/1.ts", 1, SegmentKind::Media);
            d.parsed_url = Arc::new(Url::parse(&format!("https://e.com/1.ts?t={token}")).unwrap());
            d
        };
        s.ingest(vec![signed("a")], Instant::now());
        let mut now = Instant::now();
        let refused = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(45),
            held_back: false,
        };
        assert!(
            fail_at(&mut s, &b, refused, now)
                .assembler_inputs
                .is_empty()
        );
        now += Duration::from_secs(45);

        // The token expired while the segment waited, and the playlist with
        // the fresh one has not been published yet.
        let effects = fail_at(&mut s, &b, FailureClass::Http(403), now);
        assert!(effects.assembler_inputs.is_empty(), "not terminal yet");
        assert_eq!(effects.retry_notice.expect("waits").delay, REFRESH_WAIT);
        assert!(s.next_ready_jobs(1, now, &b).0.is_empty());

        // Re-discovery publishes the fresh URL: the segment is retried at
        // once rather than after the wait.
        s.ingest(vec![signed("b")], now);
        let (jobs, _) = s.next_ready_jobs(1, now, &b);
        assert_eq!(jobs.len(), 1, "retried as soon as the URL is fresh");
        assert!(jobs[0].descriptor.parsed_url.as_str().ends_with("t=b"));
        // Denied again with the fresh URL and nothing newer: that denial is
        // real, and the one wait was used.
        let effects = s.apply_outcome(
            SegmentOutcome::Failed {
                key: jobs[0].descriptor.key.clone(),
                msn: 1,
                class: FailureClass::Http(403),
                reason: Arc::from("403"),
                error: None,
            },
            now,
        );
        assert!(matches!(
            effects.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
    }

    #[test]
    fn a_refresh_wait_that_sees_no_fresh_url_ends_in_the_denial() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let now = Instant::now();
        let refused = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(1),
            held_back: false,
        };
        fail_at(&mut s, &b, refused, now);
        let now = now + Duration::from_secs(1);
        assert!(
            fail_at(&mut s, &b, FailureClass::Http(401), now)
                .assembler_inputs
                .is_empty()
        );
        let effects = fail_at(&mut s, &b, FailureClass::Http(401), now + REFRESH_WAIT);
        assert!(matches!(
            effects.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
    }

    #[test]
    fn a_refresh_wait_keeps_the_rate_limit_hold_within_its_bound() {
        let holds = Arc::new(RateLimitHolds::new());
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_throttled: Duration::from_secs(60),
                ..StoreConfig::default()
            },
            Arc::clone(&holds),
        );
        let b = budget_unlimited();
        let start = Instant::now();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            start,
        );
        let refused = FailureClass::RateLimited {
            status: 429,
            delay: Duration::from_secs(1),
            held_back: false,
        };
        fail_at(&mut s, &b, refused, start);
        let until = start + Duration::from_secs(60);
        let now = start + Duration::from_secs(1);
        let effects = fail_at(&mut s, &b, FailureClass::Http(403), now);
        assert_eq!(effects.retry_notice.expect("waits").delay, REFRESH_WAIT);
        assert_eq!(
            holds.first_media_hold(1, 2, now),
            Some((1, until)),
            "output keeps waiting for the fresh URL"
        );
        // A denial after the wait is real and ends the hold with the segment.
        fail_at(&mut s, &b, FailureClass::Http(403), now + REFRESH_WAIT);
        assert_eq!(holds.first_media_hold(1, 2, now + REFRESH_WAIT), None);
    }

    fn skipped_ranges(inputs: &[AssemblerInput]) -> Vec<(u64, u64)> {
        inputs
            .iter()
            .map(|input| match input {
                AssemblerInput::Skipped { from_msn, to_msn } => (*from_msn, *to_msn),
                other => panic!("unexpected input {other:?}"),
            })
            .collect()
    }

    fn media_msns(s: &SegmentStateStore) -> Vec<u64> {
        let mut msns: Vec<u64> = s.records.values().map(|r| r.descriptor.msn).collect();
        msns.sort_unstable();
        msns
    }

    #[test]
    fn backlog_is_capped_by_max_state_entries_and_dropped_msns_are_skipped() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                max_state_entries: 32,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        s.ingest(
            (0..500)
                .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
                .collect(),
            Instant::now(),
        );

        // Downloads fell 500 segments behind a window that starts at 1000.
        let dropped = s.prune_below(1000, 100);

        assert_eq!(s.records.len(), 32);
        assert_eq!(
            media_msns(&s),
            (468..500).collect::<Vec<_>>(),
            "newest kept"
        );
        assert_eq!(skipped_ranges(&dropped), [(0, 467)]);
    }

    #[test]
    fn backlog_is_capped_to_one_window() {
        let mut s = store();
        s.ingest(
            (0..20)
                .map(|msn| descriptor(&format!("https://e.com/{msn}.ts"), msn, SegmentKind::Media))
                .collect(),
            Instant::now(),
        );

        let dropped = s.prune_below(20, 5);

        assert_eq!(media_msns(&s), [15, 16, 17, 18, 19]);
        assert_eq!(skipped_ranges(&dropped), [(0, 14)]);
        // Within the bound nothing more is dropped.
        assert!(s.prune_below(20, 5).is_empty());
    }

    #[test]
    fn backlog_drop_keeps_in_flight_work_and_msns_it_still_covers() {
        let mut s = store();
        let b = budget_unlimited();
        // msn 1 is in flight under one URI and re-keyed to another URI.
        s.ingest(
            vec![descriptor("https://e.com/1a.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let in_flight = take_one(&mut s, &b).expect("admitted");
        s.ingest(
            vec![
                descriptor("https://e.com/1b.ts", 1, SegmentKind::Media),
                descriptor("https://e.com/2.ts", 2, SegmentKind::Media),
            ],
            Instant::now(),
        );

        let dropped = s.prune_below(10, 0);

        assert!(s.records.contains_key(&in_flight.descriptor.key));
        assert_eq!(s.records.len(), 1);
        // msn 1 is still pending in flight, so only msn 2 is declared dead.
        assert_eq!(skipped_ranges(&dropped), [(2, 2)]);
    }

    #[test]
    fn queued_segment_below_the_window_survives_prune_and_still_schedules() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/old.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        // The window slides past it before it was admitted.
        s.prune_below(100, 10);

        let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
        assert_eq!(jobs.len(), 1, "the assembler is still waiting on msn 1");
        assert_eq!(jobs[0].descriptor.msn, 1);
        assert!(inputs.is_empty());
    }

    #[test]
    fn retrying_segment_below_the_window_keeps_its_retry_and_reports_the_outcome() {
        let mut s = SegmentStateStore::new(
            StoreConfig {
                retry_budget: 1,
                retry_delay_base: Duration::ZERO,
                ..StoreConfig::default()
            },
            Arc::new(RateLimitHolds::new()),
        );
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/1.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        let fail = |s: &mut SegmentStateStore, key: &SegmentKey| {
            s.apply_outcome(
                SegmentOutcome::Failed {
                    key: key.clone(),
                    msn: 1,
                    class: FailureClass::Http(404),
                    reason: Arc::from("gone"),
                    error: None,
                },
                Instant::now(),
            )
        };
        let key = take_one(&mut s, &b).unwrap().descriptor.key.clone();
        fail(&mut s, &key);

        s.prune_below(100, 10);
        assert!(
            s.next_retry_deadline().is_some(),
            "retry survives the slide"
        );
        assert!(s.has_unfinished_work());

        // The retry fails too: the terminal failure reaches the assembler
        // instead of the MSN vanishing.
        let retried = take_one(&mut s, &b).expect("retry is admitted");
        let effects = fail(&mut s, &retried.descriptor.key);
        assert!(matches!(
            effects.assembler_inputs.as_slice(),
            [AssemblerInput::TerminalFailed { msn: 1, .. }]
        ));
        s.prune_below(100, 10);
        assert!(!s.has_unfinished_work());
        assert!(
            s.records.is_empty(),
            "finished below-window records are pruned"
        );
    }
}
