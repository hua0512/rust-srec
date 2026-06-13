//! The reactor-owned segment state store.
//!
//! Single authoritative home for identity dedup, lifecycle state, retry
//! budget, and scheduling priority. Owned by the reactor task and never shared
//! — not an `Arc<Mutex<..>>`. The `HashMap` is the single source of truth; the
//! ready index and retry heap are advisory and re-validated on pop (lazy
//! tombstones), because pruning can evict a record whose key still sits in an
//! index.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, trace, warn};

use super::budget::{ByteBudget, ByteReservation};
use super::descriptor::{SegmentDescriptor, SegmentSource};
use super::identity::{SegmentKey, SegmentKind};
use super::input::AssemblerInput;
use super::payload::SegmentPayload;

/// Machine-readable failure classification reported by the fetch-and-process
/// future. The store — not the future — maps class to retryable-vs-terminal,
/// so retry policy lives in exactly one place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Http(u16),
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

#[derive(Debug, Clone)]
pub enum SegmentState {
    /// Ingested and waiting in the ready index.
    Discovered,
    /// A due retry placed back in the ready index.
    Queued,
    InFlight {
        attempt: u32,
        started_at: Instant,
    },
    Completed {
        completed_at: Instant,
    },
    RetryAt {
        attempt: u32,
        retry_at: Instant,
        class: FailureClass,
        reason: Arc<str>,
    },
    TerminalFailed {
        class: FailureClass,
        reason: Arc<str>,
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
            Self::Discovered | Self::Queued | Self::InFlight { .. } | Self::RetryAt { .. }
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
    },
}

impl SegmentOutcome {
    pub fn key(&self) -> &SegmentKey {
        match self {
            Self::Completed { key, .. } | Self::Failed { key, .. } => key,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RetryNotice {
    pub key: SegmentKey,
    pub msn: u64,
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
    /// 1-based attempt number (lifecycle attempts, not per-attempt HTTP retries).
    pub attempt: u32,
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
            fallback_size_estimate: 2 * 1024 * 1024,
            max_segment_size: 0,
            max_retained_inits: 8,
        }
    }
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
    /// Bumped whenever re-discovery refreshes the fetch metadata. Compared
    /// against `attempt_generation` for the 401/403 stale-signed-URL rule.
    generation: u64,
    /// Generation the current/last in-flight attempt fetched with.
    attempt_generation: u64,
    /// Exact entry placed in the ready index, so a source upgrade
    /// (prefetch -> playlist) can relocate it under its new priority class.
    ready_entry: Option<ReadyEntry>,
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
    /// Aggregated counters (bounded cardinality — per-segment detail goes to
    /// spans, not labels).
    pub lifecycle_retries: u64,
    pub terminal_failures: u64,
    pub dedup_hits: u64,
}

impl SegmentStateStore {
    pub fn new(config: StoreConfig) -> Self {
        let size_estimate = config.fallback_size_estimate.max(1);
        Self {
            config,
            records: HashMap::new(),
            ready: BTreeSet::new(),
            ready_lookup: HashMap::new(),
            retry_heap: BinaryHeap::new(),
            next_order: 0,
            size_estimate,
            lifecycle_retries: 0,
            terminal_failures: 0,
            dedup_hits: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

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
            if let Some(record) = self.records.get_mut(&key) {
                let stale_entry = match record.state {
                    SegmentState::Completed { .. } | SegmentState::TerminalFailed { .. } => {
                        // Never fetched again; nothing volatile to refresh.
                        stats.deduplicated += 1;
                        self.dedup_hits += 1;
                        continue;
                    }
                    SegmentState::Discovered
                    | SegmentState::Queued
                    | SegmentState::RetryAt { .. }
                    | SegmentState::InFlight { .. } => {
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
                        self.dedup_hits += 1;
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
                }
            } else {
                self.records.insert(
                    key.clone(),
                    SegmentRecord {
                        descriptor: Arc::new(descriptor),
                        state: SegmentState::Discovered,
                        reschedules: 0,
                        generation: 0,
                        attempt_generation: 0,
                        ready_entry: None,
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

    /// Batched admission: returns up to `slots` jobs in priority order, each
    /// owning its download-byte reservation, plus any assembler items implied
    /// by admission-time terminalization (oversize segments). Stops early when
    /// the budget cannot fit the next estimate — admission is the byte gate.
    pub fn next_ready_jobs(
        &mut self,
        slots: usize,
        now: Instant,
        budget: &ByteBudget,
    ) -> (Vec<ReadyJob>, Vec<AssemblerInput>) {
        self.promote_due_retries(now);

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
                warn!(msn, %reason, "segment oversize at admission");
                record.state = SegmentState::TerminalFailed {
                    class: FailureClass::Oversize,
                    reason,
                };
                self.terminal_failures += 1;
                inputs.push(AssemblerInput::TerminalFailed {
                    key: key.clone(),
                    msn,
                });
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
            let Some(reservation) = budget.download.try_reserve(reserve_size) else {
                break;
            };

            self.remove_ready_entry(&entry);
            let record = self.records.get_mut(&key).expect("validated above");
            record.ready_entry = None;
            let attempt = record.reschedules + 1;
            record.state = SegmentState::InFlight {
                attempt,
                started_at: now,
            };
            record.attempt_generation = record.generation;
            trace!(
                msn = record.descriptor.msn,
                kind = ?record.descriptor.key.kind,
                attempt,
                reserved = reservation.held_bytes(),
                "segment admitted"
            );
            jobs.push(ReadyJob {
                descriptor: Arc::clone(&record.descriptor),
                attempt,
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
                let payload_len = payload.len() as u64;
                if let Some(record) = self.records.get_mut(&key) {
                    record.state = SegmentState::Completed { completed_at: now };
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
            } => {
                let Some(record) = self.records.get_mut(&key) else {
                    return OutcomeEffects::default();
                };

                let retryable = match class {
                    FailureClass::Http(404 | 429) | FailureClass::Http(500..=599) => true,
                    FailureClass::Network
                    | FailureClass::Timeout
                    | FailureClass::Decode
                    | FailureClass::OverBudget => true,
                    // Auth failures are conditionally retryable: a signed URL
                    // can expire mid-flight while a newer playlist already
                    // refreshed it. Retry only when re-discovery advanced the
                    // generation past the one this attempt fetched with —
                    // otherwise the denial is real.
                    FailureClass::Http(401 | 403) => record.generation > record.attempt_generation,
                    FailureClass::Http(_)
                    | FailureClass::UnsupportedCrypto
                    | FailureClass::InvalidFormat
                    | FailureClass::Oversize => false,
                };

                if retryable && record.reschedules < self.config.retry_budget {
                    record.reschedules += 1;
                    let reschedules = record.reschedules;
                    let exp = reschedules.saturating_sub(1).min(16);
                    let delay = self
                        .config
                        .retry_delay_base
                        .checked_mul(1u32 << exp)
                        .unwrap_or(self.config.retry_delay_max)
                        .min(self.config.retry_delay_max);
                    let retry_at = now + delay;
                    record.state = SegmentState::RetryAt {
                        attempt: reschedules,
                        retry_at,
                        class,
                        reason: Arc::clone(&reason),
                    };
                    self.lifecycle_retries += 1;
                    debug!(
                        msn,
                        ?class,
                        %reason,
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
                            key,
                            msn,
                            attempt: reschedules,
                            delay,
                            reason,
                        }),
                    }
                } else {
                    record.state = SegmentState::TerminalFailed {
                        class,
                        reason: Arc::clone(&reason),
                    };
                    self.terminal_failures += 1;
                    warn!(msn, ?class, %reason, "segment terminally failed");
                    // Media and init terminal failures are both surfaced; the
                    // assembler decides whether the stream can continue (a
                    // failed init on an fMP4 stream is fatal, a failed media
                    // MSN is a gap to advance past).
                    OutcomeEffects {
                        assembler_inputs: vec![AssemblerInput::TerminalFailed { key, msn }],
                        retry_notice: None,
                    }
                }
            }
        }
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
    /// re-download it. Only records below `window_start_msn` are eligible,
    /// in-flight work is always kept, and init records are retained up to
    /// `max_retained_inits` (newest first). `max_state_entries` is a backstop
    /// within that rule: when nothing is safely evictable, state temporarily
    /// exceeds the cap.
    pub fn prune_below(&mut self, window_start_msn: u64) {
        self.records.retain(|_, record| {
            matches!(record.state, SegmentState::InFlight { .. })
                || record.descriptor.key.kind == SegmentKind::Init
                || record.descriptor.msn >= window_start_msn
        });

        // Retain only the newest N init records, but never evict one that can
        // still appear in the window (msn >= window_start_msn) — that would
        // make the next refresh re-discover and re-download it, breaking the
        // same prune invariant the media retain above upholds. In-flight inits
        // are never evicted either.
        let mut init_msns: Vec<(u64, SegmentKey)> = self
            .records
            .iter()
            .filter(|(_, r)| {
                r.descriptor.key.kind == SegmentKind::Init
                    && !matches!(r.state, SegmentState::InFlight { .. })
                    && r.descriptor.msn < window_start_msn
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
    use crate::hls::engine::identity::ByteRangeKey;
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
            },
            msn,
            source,
            parsed_url: Arc::new(url),
            discontinuity: false,
            encryption: None,
            init_key: None,
            media_segment: Arc::new(m3u8_rs::MediaSegment {
                uri: uri.to_string(),
                duration: 2.0,
                ..Default::default()
            }),
        }
    }

    fn payload_for(d: &Arc<SegmentDescriptor>) -> SegmentPayload {
        SegmentPayload::Ts {
            data: Bytes::from_static(b"data"),
            descriptor: Arc::clone(d),
        }
    }

    fn store() -> SegmentStateStore {
        SegmentStateStore::new(StoreConfig {
            retry_budget: 2,
            retry_delay_base: Duration::from_millis(10),
            ..StoreConfig::default()
        })
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
            },
            now,
        );
        assert!(
            out.assembler_inputs.is_empty(),
            "retryable failure emits nothing yet"
        );
        let notice = out.retry_notice.expect("retry notice should be surfaced");
        assert_eq!(notice.key, key);
        assert_eq!(notice.msn, 1);
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
        assert_eq!(jobs[0].attempt, 2);
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

        s.prune_below(10);

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
        let mut s = SegmentStateStore::new(StoreConfig {
            max_retained_inits: 2,
            ..StoreConfig::default()
        });
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

        s.prune_below(10);

        assert!(
            s.records
                .keys()
                .any(|k| k.uri.as_ref() == "https://e.com/i10.mp4"),
            "in-window init must survive the retention trim"
        );
    }

    #[test]
    fn pruned_keys_in_indices_never_schedule() {
        let mut s = store();
        let b = budget_unlimited();
        s.ingest(
            vec![descriptor("https://e.com/old.ts", 1, SegmentKind::Media)],
            Instant::now(),
        );
        // Key sits in the ready index; prune evicts the record.
        s.prune_below(100);
        let (jobs, inputs) = s.next_ready_jobs(8, Instant::now(), &b);
        assert!(jobs.is_empty(), "tombstoned entry must not schedule");
        assert!(inputs.is_empty());
    }

    #[test]
    fn next_retry_deadline_skips_stale_entries() {
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
                class: FailureClass::Timeout,
                reason: Arc::from("t"),
            },
            now,
        );
        assert!(s.next_retry_deadline().is_some());

        // Prune the record: the heap entry is now stale and must be skipped.
        s.prune_below(100);
        assert!(s.next_retry_deadline().is_none());
    }
}
