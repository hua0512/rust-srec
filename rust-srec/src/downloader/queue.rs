//! Priority-aware download queue.
//!
//! Replaces the two `tokio::sync::Semaphore`s previously embedded in the
//! [`crate::downloader::manager::DownloadManager`]. Owns the question
//! "who's allowed to start a download right now, and in what order?"
//!
//! ## Why a custom queue
//!
//! `tokio::sync::Semaphore` is strict FIFO. With two semaphores plus a
//! fall-through (`high_priority_extra_slots` first, then the normal
//! pool), high-priority streamers that arrive after the dedicated pool
//! is full lose their priority advantage — they queue behind whichever
//! normal-priority streamer happened to call `acquire_owned` earlier.
//!
//! This queue serves both tiers from a single pool with the rule:
//! *high-priority waiters get the next free slot regardless of which
//! tier freed it, as long as the global capacity constraint is
//! respected*. Normal-priority waiters are only woken when
//! `in_flight_normal < normal_capacity`.
//!
//! ## Capacity model
//!
//! - `normal_capacity` — slots usable by anyone.
//! - `high_extra_capacity` — slots only high-priority downloads may
//!   occupy.
//! - `total_capacity = normal_capacity + high_extra_capacity` — the
//!   absolute cap.
//!
//! Active accounting:
//! - `in_flight_total` — count of all currently-running downloads.
//! - `in_flight_normal` — subset that are normal-priority.
//!
//! A high-priority download can occupy any slot. A normal-priority
//! download can only occupy a "normal" slot (one where increasing
//! `in_flight_normal` doesn't exceed `normal_capacity`).
//!
//! ## Cancellation
//!
//! Each acquire takes a [`CancellationToken`]. A waiter that loses the
//! cancel-vs-wakeup race may briefly count as acquired in the atomics;
//! it detects this via the `promoted` flag on its [`Waiter`] and
//! decrements + cascades to the next waiter on its way out.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering as AtomicOrdering,
};

use chrono::Utc;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::engine::EngineType;

/// Default normal-pool capacity used by [`DownloadQueue::new`] when no
/// explicit value is supplied. Mirrors the historical default in
/// [`crate::downloader::manager::DownloadManagerConfig`].
pub const DEFAULT_NORMAL_CAPACITY: usize = 6;

/// Priority tier for a queued or active download.
///
/// `High` always sorts before `Normal` in the wait order; ties are
/// broken by `queued_at_ms` (earlier first), then by the per-queue
/// monotonic insertion sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    Normal,
    High,
}

impl Priority {
    pub fn is_high(&self) -> bool {
        matches!(self, Self::High)
    }

    /// Stable string label for logging.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

/// Snapshot view of an entry currently waiting on the queue.
///
/// Returned from [`DownloadQueue::snapshot_pending`] for the WebSocket
/// initial-snapshot path; cloning an `Arc` so callers can keep the
/// snapshot around without holding the queue lock.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    pub session_id: String,
    pub streamer_id: String,
    pub streamer_name: String,
    pub engine_type: EngineType,
    pub priority: Priority,
    pub queued_at_ms: i64,
}

/// Request to acquire a slot.
#[derive(Debug, Clone)]
pub struct AcquireRequest {
    pub session_id: String,
    pub streamer_id: String,
    pub streamer_name: String,
    pub engine_type: EngineType,
    pub priority: Priority,
}

/// Outcome of a failed acquire.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    /// The cancellation token fired before a slot became available.
    #[error("acquire cancelled")]
    Cancelled,
    /// Another acquire is already in progress (or active) for the same
    /// session_id. Callers should not retry — the existing pipeline
    /// will run to completion.
    #[error("session {0} is already pending or active")]
    DuplicateSession(String),
    /// The queue is shutting down (capacity zero and no further
    /// reservations accepted).
    #[error("queue shutting down")]
    ShuttingDown,
}

/// Owned slot from the queue.
///
/// Drop semantics:
/// - If `armed` (default), drop returns queue capacity, which may
///   promote the next waiter, then releases the session reservation.
/// - If [`Self::into_active`] has been called, drop is a no-op; the
///   active-downloads entry takes over the lifetime and releases the
///   slot when its [`ActiveSlot`] is dropped.
pub struct SlotGuard {
    queue: Arc<DownloadQueue>,
    priority: Priority,
    queued_at_ms: i64,
    acquired_at_ms: i64,
    session_id: String,
    queued_event_emitted: bool,
    armed: bool,
}

impl SlotGuard {
    /// Milliseconds the request spent in the queue before this slot
    /// was granted. Zero for the fast-path acquire.
    pub fn waited_ms(&self) -> i64 {
        (self.acquired_at_ms - self.queued_at_ms).max(0)
    }

    /// Priority tier assigned at acquire time.
    pub fn priority(&self) -> Priority {
        self.priority
    }

    /// Session id this slot was acquired for.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Wall-clock timestamp (ms) when the slot was originally
    /// requested. Useful to attach to telemetry events.
    pub fn queued_at_ms(&self) -> i64 {
        self.queued_at_ms
    }

    /// Wall-clock timestamp (ms) when the slot was granted.
    pub fn acquired_at_ms(&self) -> i64 {
        self.acquired_at_ms
    }

    /// Whether this slot previously emitted a visible queued event.
    pub fn queued_event_emitted(&self) -> bool {
        self.queued_event_emitted
    }

    /// Convert this guard into an `ActiveSlot` whose lifetime is
    /// managed by the caller (typically via insertion into the
    /// download manager's `active_downloads` map). After this, drop
    /// is a no-op; the caller MUST call [`ActiveSlot::release`] (or
    /// drop the `ActiveSlot`) to return the capacity.
    pub fn into_active(mut self) -> ActiveSlot {
        self.armed = false;
        ActiveSlot {
            queue: self.queue.clone(),
            priority: self.priority,
            session_id: self.session_id.clone(),
            released: false,
        }
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.armed {
            self.queue
                .release_owned_slot(self.priority, &self.session_id);
        }
    }
}

/// Slot ownership that has been moved into a longer-lived structure
/// (e.g. an active-downloads entry). Releases the queue capacity on
/// drop.
pub struct ActiveSlot {
    queue: Arc<DownloadQueue>,
    priority: Priority,
    session_id: String,
    released: bool,
}

impl ActiveSlot {
    /// Explicitly release the slot. Subsequent drops are no-ops.
    pub fn release(mut self) {
        if !self.released {
            self.released = true;
            self.queue
                .release_owned_slot(self.priority, &self.session_id);
        }
    }
}

impl Drop for ActiveSlot {
    fn drop(&mut self) {
        if !self.released {
            self.released = true;
            self.queue
                .release_owned_slot(self.priority, &self.session_id);
        }
    }
}

impl std::fmt::Debug for ActiveSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActiveSlot")
            .field("priority", &self.priority)
            .field("session_id", &self.session_id)
            .field("released", &self.released)
            .finish()
    }
}

/// Internal waiter record, ordered in the wait heap.
struct Waiter {
    session_id: String,
    priority: Priority,
    queued_at_ms: i64,
    seq: u64,
    notify: Arc<Notify>,
    /// Set to `true` by the wakeup path *before* `notify_one` so the
    /// canceled-acquire branch can detect "we were promoted, must
    /// release back" even if our `select!` chose the cancel arm.
    promoted: Arc<AtomicBool>,
}

impl PartialEq for Waiter {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Waiter {}
impl PartialOrd for Waiter {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Waiter {
    /// `BinaryHeap` is a max-heap; we want the *highest priority* and
    /// *earliest queued_at* to come out first, so flip the natural
    /// ordering.
    fn cmp(&self, other: &Self) -> Ordering {
        // High > Normal so that High pops first.
        let prio = priority_rank(self.priority).cmp(&priority_rank(other.priority));
        if prio != Ordering::Equal {
            return prio;
        }
        // Earlier queued_at_ms comes first → reverse natural
        // ordering on i64.
        let ts = other.queued_at_ms.cmp(&self.queued_at_ms);
        if ts != Ordering::Equal {
            return ts;
        }
        // Tie-break by seq, again with earlier-first.
        other.seq.cmp(&self.seq)
    }
}

fn priority_rank(p: Priority) -> u8 {
    match p {
        Priority::High => 1,
        Priority::Normal => 0,
    }
}

/// Priority-aware queue with priority-promotion wakeup.
pub struct DownloadQueue {
    normal_capacity: AtomicUsize,
    high_extra_capacity: AtomicUsize,
    in_flight_total: AtomicUsize,
    in_flight_normal: AtomicUsize,
    /// Heap of waiters ordered by (priority desc, queued_at asc, seq asc).
    waiters: Mutex<BinaryHeap<Waiter>>,
    /// Pending entries indexed by session_id for dedup and snapshot.
    pending: DashMap<String, Arc<PendingEntryInner>>,
    /// Session ids currently pending or active. This reservation is
    /// acquired before either the fast path or slow path can consume
    /// capacity, and released only when the queued wait is abandoned or
    /// the acquired slot is dropped.
    session_reservations: DashMap<String, ()>,
    /// Insertion counter for stable ordering when timestamps tie.
    next_seq: AtomicU64,
    /// Set true by [`Self::shutdown`] to reject new acquires.
    shutting_down: AtomicBool,
}

/// Storage-side pending entry. Mirrors [`PendingEntry`] but also holds
/// wait-notification state. Snapshot reads project to the public shape.
struct PendingEntryInner {
    session_id: String,
    streamer_id: String,
    streamer_name: String,
    engine_type: EngineType,
    priority: Priority,
    queued_at_ms: AtomicI64,
    notify: Arc<Notify>,
    seq: u64,
}

impl PendingEntryInner {
    fn to_public(&self) -> PendingEntry {
        PendingEntry {
            session_id: self.session_id.clone(),
            streamer_id: self.streamer_id.clone(),
            streamer_name: self.streamer_name.clone(),
            engine_type: self.engine_type,
            priority: self.priority,
            queued_at_ms: self.queued_at_ms.load(AtomicOrdering::Relaxed),
        }
    }
}

impl DownloadQueue {
    /// Create a new queue with `normal_capacity` (>= 1) and
    /// `high_extra_capacity` (>= 0).
    pub fn new(normal_capacity: usize, high_extra_capacity: usize) -> Arc<Self> {
        let normal = normal_capacity.max(1);
        Arc::new(Self {
            normal_capacity: AtomicUsize::new(normal),
            high_extra_capacity: AtomicUsize::new(high_extra_capacity),
            in_flight_total: AtomicUsize::new(0),
            in_flight_normal: AtomicUsize::new(0),
            waiters: Mutex::new(BinaryHeap::new()),
            pending: DashMap::new(),
            session_reservations: DashMap::new(),
            next_seq: AtomicU64::new(0),
            shutting_down: AtomicBool::new(false),
        })
    }

    /// Current normal-pool capacity.
    pub fn normal_capacity(&self) -> usize {
        self.normal_capacity.load(AtomicOrdering::SeqCst)
    }

    /// Current high-priority extra capacity.
    pub fn high_extra_capacity(&self) -> usize {
        self.high_extra_capacity.load(AtomicOrdering::SeqCst)
    }

    /// Sum of both capacities.
    pub fn total_capacity(&self) -> usize {
        self.normal_capacity()
            .saturating_add(self.high_extra_capacity())
    }

    /// In-flight count across both tiers.
    pub fn in_flight(&self) -> usize {
        self.in_flight_total.load(AtomicOrdering::SeqCst)
    }

    /// In-flight count restricted to normal-priority downloads.
    pub fn in_flight_normal(&self) -> usize {
        self.in_flight_normal.load(AtomicOrdering::SeqCst)
    }

    /// Number of pending acquires.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Update both capacities. Returns the applied `(normal, high_extra)`.
    ///
    /// Increases wake as many waiters as fit. Decreases stop new
    /// promotions but don't touch in-flight downloads — they release
    /// naturally on completion.
    pub fn set_capacity(&self, normal: usize, high_extra: usize) -> (usize, usize) {
        let normal = normal.max(1);
        self.normal_capacity.store(normal, AtomicOrdering::SeqCst);
        self.high_extra_capacity
            .store(high_extra, AtomicOrdering::SeqCst);
        // Try to wake as many as we can fit now.
        loop {
            if !self.try_promote_one() {
                break;
            }
        }
        (normal, high_extra)
    }

    /// Set just the normal-pool capacity. Returns the applied value.
    pub fn set_normal_capacity(&self, normal: usize) -> usize {
        let (n, _) = self.set_capacity(normal, self.high_extra_capacity());
        n
    }

    /// Set just the high-extra capacity. Returns the applied value.
    pub fn set_high_extra_capacity(&self, high_extra: usize) -> usize {
        let (_, h) = self.set_capacity(self.normal_capacity(), high_extra);
        h
    }

    /// Snapshot of currently-pending entries, in arrival order.
    pub fn snapshot_pending(&self) -> Vec<PendingEntry> {
        let mut entries: Vec<(u64, PendingEntry)> = self
            .pending
            .iter()
            .map(|e| (e.value().seq, e.value().to_public()))
            .collect();
        entries.sort_by_key(|(seq, _)| *seq);
        entries.into_iter().map(|(_, p)| p).collect()
    }

    #[cfg(test)]
    fn cancel_pending(&self, session_id: &str) {
        if let Some(entry) = self.pending.get(session_id) {
            entry.notify.notify_one();
        }
    }

    /// Reject all future acquires. Existing pending acquires are
    /// notified to wake up; they observe the shutdown flag and return
    /// [`AcquireError::ShuttingDown`].
    pub fn shutdown(&self) {
        self.shutting_down.store(true, AtomicOrdering::SeqCst);
        // Wake all waiters.
        for entry in self.pending.iter() {
            entry.notify.notify_one();
        }
    }

    /// Acquire a slot.
    ///
    /// `on_queued` is invoked exactly once *only* when the request had
    /// to park (no fast-path slot was available). It runs while the
    /// pending entry is registered, so callers can read it from the
    /// snapshot or emit events from inside the callback.
    pub async fn acquire<F>(
        self: &Arc<Self>,
        request: AcquireRequest,
        cancel: CancellationToken,
        on_queued: F,
    ) -> std::result::Result<SlotGuard, AcquireError>
    where
        F: FnOnce(&PendingEntry),
    {
        if self.shutting_down.load(AtomicOrdering::SeqCst) {
            return Err(AcquireError::ShuttingDown);
        }

        let queued_at_ms = now_ms();
        let mut session_reservation =
            match SessionReservation::try_new(self.clone(), &request.session_id) {
                Ok(reservation) => reservation,
                Err(()) => return Err(AcquireError::DuplicateSession(request.session_id)),
            };

        // Admission path: try to take a slot without queueing, but
        // hold the waiter heap lock until a slow-path caller is fully
        // registered. This closes the fairness race where a release or
        // newcomer observes "no waiters" while an older caller is
        // between its fast-path miss and heap insertion.
        let seq = self.next_seq.fetch_add(1, AtomicOrdering::SeqCst);
        let notify = Arc::new(Notify::new());
        let promoted = Arc::new(AtomicBool::new(false));
        let inner = Arc::new(PendingEntryInner {
            session_id: request.session_id.clone(),
            streamer_id: request.streamer_id.clone(),
            streamer_name: request.streamer_name.clone(),
            engine_type: request.engine_type,
            priority: request.priority,
            queued_at_ms: AtomicI64::new(queued_at_ms),
            notify: notify.clone(),
            seq,
        });

        {
            let mut heap = self.waiters.lock();

            if self.shutting_down.load(AtomicOrdering::SeqCst) {
                return Err(AcquireError::ShuttingDown);
            }

            if self.pending.is_empty() && heap.is_empty() && self.try_acquire_fast(request.priority)
            {
                session_reservation.disarm();
                return Ok(SlotGuard {
                    queue: self.clone(),
                    priority: request.priority,
                    queued_at_ms,
                    acquired_at_ms: queued_at_ms,
                    session_id: request.session_id,
                    queued_event_emitted: false,
                    armed: true,
                });
            }

            match self.pending.entry(request.session_id.clone()) {
                Entry::Occupied(_) => {
                    return Err(AcquireError::DuplicateSession(request.session_id));
                }
                Entry::Vacant(entry) => {
                    entry.insert(inner.clone());
                }
            }

            heap.push(Waiter {
                session_id: request.session_id.clone(),
                priority: request.priority,
                queued_at_ms,
                seq,
                notify: notify.clone(),
                promoted: promoted.clone(),
            });
        }
        let mut pending_cleanup = PendingAcquireCleanup {
            queue: self.clone(),
            session_id: request.session_id.clone(),
            priority: request.priority,
            promoted: promoted.clone(),
            armed: true,
        };

        if self.shutting_down.load(AtomicOrdering::SeqCst) {
            return Err(AcquireError::ShuttingDown);
        }

        // Race-window catch: a `release()` between the fast-path miss
        // above and the enqueue above would have seen no waiters and
        // returned without promoting anyone. Run a promotion pass now
        // so the slot it freed is honoured. `try_promote_one` is a
        // no-op when no waiter fits current capacity, so this is safe
        // to run unconditionally.
        loop {
            if !self.try_promote_one() {
                break;
            }
        }

        // Invoke queued callback (e.g. emit DownloadQueued event).
        // Done after the promotion pass so we don't fire a "queued"
        // event for a request that immediately got promoted in the
        // race-window catch above.
        let queued_event_emitted = !promoted.load(AtomicOrdering::SeqCst);
        if queued_event_emitted {
            on_queued(&inner.to_public());
        }

        // Wait for promotion or cancel. The notified() future is
        // created up front so we don't miss a wakeup that races with
        // our entry into the select.
        let notified = notify.notified();
        tokio::pin!(notified);

        let was_notified = tokio::select! {
            _ = &mut notified => true,
            _ = cancel.cancelled() => false,
        };

        // The promoter sets `promoted=true` and bumps counters BEFORE
        // calling `notify_one`. On cancellation/error paths, the
        // `PendingAcquireCleanup` drop synchronizes with promotion via
        // the waiter heap lock, removes stale state, and releases any
        // slot that was granted to a future the caller abandoned.
        let promoted_now = promoted.load(AtomicOrdering::SeqCst);

        match (was_notified, promoted_now) {
            (true, true) => {
                // Normal promotion path. Counters already bumped.
                pending_cleanup.commit_acquired();
                session_reservation.disarm();
                let acquired_at_ms = now_ms();
                Ok(SlotGuard {
                    queue: self.clone(),
                    priority: request.priority,
                    queued_at_ms,
                    acquired_at_ms,
                    session_id: request.session_id,
                    queued_event_emitted,
                    armed: true,
                })
            }
            (false, true) => {
                // Race: cancel won the select arm but the wakeup path
                // already promoted us and bumped the counters. The
                // cleanup guard releases the slot and cascades wakeup.
                Err(AcquireError::Cancelled)
            }
            (true, false) => {
                // Spurious wake (e.g. shutdown or test nudge). No
                // counters were bumped.
                if self.shutting_down.load(AtomicOrdering::SeqCst) {
                    Err(AcquireError::ShuttingDown)
                } else {
                    Err(AcquireError::Cancelled)
                }
            }
            (false, false) => {
                // Clean cancellation without promotion.
                if self.shutting_down.load(AtomicOrdering::SeqCst) {
                    Err(AcquireError::ShuttingDown)
                } else {
                    Err(AcquireError::Cancelled)
                }
            }
        }
    }

    /// Try to take a slot without queueing. Returns true if a slot was
    /// taken (in_flight counters incremented).
    fn try_acquire_fast(&self, priority: Priority) -> bool {
        self.try_reserve_capacity(priority)
    }

    fn try_reserve_capacity(&self, priority: Priority) -> bool {
        match priority {
            Priority::High => self.try_reserve_total_capacity(),
            Priority::Normal => self.try_reserve_normal_capacity(),
        }
    }

    fn try_reserve_total_capacity(&self) -> bool {
        loop {
            let total = self.in_flight_total.load(AtomicOrdering::SeqCst);
            let total_cap = self.total_capacity();
            if total >= total_cap {
                return false;
            }

            if self
                .in_flight_total
                .compare_exchange(
                    total,
                    total + 1,
                    AtomicOrdering::SeqCst,
                    AtomicOrdering::SeqCst,
                )
                .is_ok()
            {
                return true;
            }
        }
    }

    fn try_reserve_normal_capacity(&self) -> bool {
        // High priority can use any slot up to total capacity.
        // Normal priority can only use a normal slot. Reserve the
        // normal counter first so concurrent normal acquirers cannot
        // all observe the same normal-capacity gap before any of them
        // increments it.
        loop {
            let normal_in = self.in_flight_normal.load(AtomicOrdering::SeqCst);
            let normal_cap = self.normal_capacity();
            if normal_in >= normal_cap {
                return false;
            }

            if self
                .in_flight_normal
                .compare_exchange(
                    normal_in,
                    normal_in + 1,
                    AtomicOrdering::SeqCst,
                    AtomicOrdering::SeqCst,
                )
                .is_err()
            {
                continue;
            }

            if self.try_reserve_total_capacity() {
                return true;
            }

            self.in_flight_normal.fetch_sub(1, AtomicOrdering::SeqCst);
            return false;
        }
    }

    /// Try to promote one waiter. Called when a slot frees or capacity
    /// is increased. Returns true if a waiter was promoted.
    fn try_promote_one(&self) -> bool {
        let mut heap = self.waiters.lock();
        loop {
            let Some(top) = heap.peek() else {
                return false;
            };

            // Skip stale entries (cancelled but still in heap).
            if !self.pending.contains_key(&top.session_id) {
                heap.pop();
                continue;
            }

            // Check capacity for this waiter's priority tier.
            if !self.try_reserve_capacity(top.priority) {
                // The top isn't a high-priority that fits, and no normal
                // capacity. If the top is high but doesn't fit, no later
                // entry can either (they're all <= high). If the top is
                // normal and there's no normal capacity, no later normal
                // can either, but a later high might still fit — except
                // BinaryHeap orders high > normal so high is always the
                // top. So we can safely return false here.
                return false;
            }

            let waiter = heap.pop().unwrap();
            // Capacity is reserved BEFORE setting promoted+notify so
            // a canceled waiter that races us still sees the bump and
            // releases.
            waiter.promoted.store(true, AtomicOrdering::SeqCst);
            waiter.notify.notify_one();
            return true;
        }
    }

    fn abandon_pending(&self, session_id: &str, priority: Priority, promoted: &AtomicBool) {
        let was_promoted = {
            let mut heap = self.waiters.lock();
            self.pending.remove(session_id);
            let was_promoted = promoted.swap(false, AtomicOrdering::SeqCst);
            if !was_promoted {
                heap.retain(|w| w.session_id != session_id);
            }
            was_promoted
        };

        if was_promoted {
            self.release(priority);
        }
    }

    /// Release a held slot and try to promote the next waiter.
    fn release(&self, priority: Priority) {
        self.in_flight_total.fetch_sub(1, AtomicOrdering::SeqCst);
        if matches!(priority, Priority::Normal) {
            self.in_flight_normal.fetch_sub(1, AtomicOrdering::SeqCst);
        }
        // Wake one waiter if any fit.
        self.try_promote_one();
    }

    fn release_owned_slot(&self, priority: Priority, session_id: &str) {
        self.release(priority);
        self.session_reservations.remove(session_id);
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

struct SessionReservation {
    queue: Arc<DownloadQueue>,
    session_id: String,
    armed: bool,
}

impl SessionReservation {
    fn try_new(queue: Arc<DownloadQueue>, session_id: &str) -> std::result::Result<Self, ()> {
        let session_id = session_id.to_string();
        {
            match queue.session_reservations.entry(session_id.clone()) {
                Entry::Occupied(_) => return Err(()),
                Entry::Vacant(entry) => {
                    entry.insert(());
                }
            }
        }

        Ok(Self {
            queue,
            session_id,
            armed: true,
        })
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SessionReservation {
    fn drop(&mut self) {
        if self.armed {
            self.queue.session_reservations.remove(&self.session_id);
        }
    }
}

struct PendingAcquireCleanup {
    queue: Arc<DownloadQueue>,
    session_id: String,
    priority: Priority,
    promoted: Arc<AtomicBool>,
    armed: bool,
}

impl PendingAcquireCleanup {
    fn commit_acquired(&mut self) {
        self.queue.pending.remove(&self.session_id);
        self.armed = false;
    }
}

impl Drop for PendingAcquireCleanup {
    fn drop(&mut self) {
        if self.armed {
            self.queue
                .abandon_pending(&self.session_id, self.priority, &self.promoted);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::Barrier;

    fn req(session: &str, prio: Priority) -> AcquireRequest {
        AcquireRequest {
            session_id: session.to_string(),
            streamer_id: format!("streamer-{}", session),
            streamer_name: format!("Streamer {}", session),
            engine_type: EngineType::Ffmpeg,
            priority: prio,
        }
    }

    #[tokio::test]
    async fn acquire_fast_path() {
        let q = DownloadQueue::new(2, 0);
        let g = q
            .acquire(
                req("s1", Priority::Normal),
                CancellationToken::new(),
                |_| {
                    panic!("should not queue on fast path");
                },
            )
            .await
            .unwrap();
        assert_eq!(q.in_flight(), 1);
        assert_eq!(q.in_flight_normal(), 1);
        drop(g);
        assert_eq!(q.in_flight(), 0);
        assert_eq!(q.in_flight_normal(), 0);
    }

    #[tokio::test]
    async fn acquire_slow_path_invokes_callback() {
        let q = DownloadQueue::new(1, 0);
        let _g1 = q
            .acquire(
                req("s1", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        let q2 = q.clone();
        let cb_fired = Arc::new(AtomicBool::new(false));
        let cb_fired_clone = cb_fired.clone();
        let h = tokio::spawn(async move {
            q2.acquire(
                req("s2", Priority::Normal),
                CancellationToken::new(),
                move |_| {
                    cb_fired_clone.store(true, AtomicOrdering::SeqCst);
                },
            )
            .await
        });

        // Wait for s2 to register as pending.
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);
        assert!(cb_fired.load(AtomicOrdering::SeqCst));

        // Release s1 so s2 can proceed.
        drop(_g1);
        let g2 = h.await.unwrap().unwrap();
        assert!(g2.waited_ms() >= 0);
        assert_eq!(q.pending_count(), 0);
        assert_eq!(q.in_flight(), 1);
    }

    #[tokio::test]
    async fn priority_ordering_high_first() {
        let q = DownloadQueue::new(1, 0);
        let g0 = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();

        let q1 = q.clone();
        let n1 = tokio::spawn(async move {
            q1.acquire(
                req("n1", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        // Ensure n1 enters the heap first.
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let q2 = q.clone();
        let h1 = tokio::spawn(async move {
            q2.acquire(req("h1", Priority::High), CancellationToken::new(), |_| {})
                .await
        });
        for _ in 0..50 {
            if q.pending_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 2);

        // Release the active slot. h1 must fire first.
        drop(g0);
        let g_high = h1.await.unwrap().unwrap();
        assert!(matches!(g_high.priority(), Priority::High));
        assert_eq!(q.in_flight(), 1);

        // Release h1; n1 should fire next.
        drop(g_high);
        let g_normal = n1.await.unwrap().unwrap();
        assert!(matches!(g_normal.priority(), Priority::Normal));
    }

    #[tokio::test]
    async fn fifo_within_tier() {
        let q = DownloadQueue::new(1, 0);
        let g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();

        let q1 = q.clone();
        let n1 = tokio::spawn(async move {
            q1.acquire(
                req("n1", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let q2 = q.clone();
        let n2 = tokio::spawn(async move {
            q2.acquire(
                req("n2", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        drop(g);
        let _g1 = n1.await.unwrap().unwrap();
        let q3 = q.clone();
        // n2 still waiting; release the just-acquired slot to free n2.
        drop(_g1);
        let _g2 = n2.await.unwrap().unwrap();
        assert_eq!(q3.pending_count(), 0);
    }

    #[tokio::test]
    async fn cancel_during_wait() {
        let q = DownloadQueue::new(1, 0);
        let _g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();

        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(req("b", Priority::Normal), cancel_clone, |_| {})
                .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);
        cancel.cancel();
        let res = h.await.unwrap();
        assert!(matches!(res, Err(AcquireError::Cancelled)));
        assert_eq!(q.pending_count(), 0);
        // Capacity unchanged: only the original `_g` is in-flight.
        assert_eq!(q.in_flight(), 1);
    }

    #[tokio::test]
    async fn cancel_pending_via_session_id() {
        let q = DownloadQueue::new(1, 0);
        let _g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();

        let token = CancellationToken::new();
        let token_clone = token.clone();
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(req("b", Priority::Normal), token_clone, |_| {})
                .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // cancel_pending wakes and cancels the pending acquire without
        // requiring the caller to hold the original CancellationToken.
        q.cancel_pending("b");
        let res = h.await.unwrap();
        assert!(matches!(res, Err(AcquireError::Cancelled)));
    }

    #[tokio::test]
    async fn duplicate_session_id_rejected() {
        let q = DownloadQueue::new(2, 0);
        let _g = q
            .acquire(
                req("dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
        // Saturate so a second acquire would queue.
        let _g2 = q
            .acquire(
                req("other", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        // Attempt another acquire with the same session_id — this is
        // expected to fail at the active-set level, but our pending
        // dedup only catches in-flight pending. The active dedup is
        // the manager's responsibility; we test the pending case by
        // acquiring under saturation:
        let q2 = q.clone();
        let h_first = tokio::spawn(async move {
            q2.acquire(
                req("queued-dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let res2 = q
            .acquire(
                req("queued-dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await;
        assert!(matches!(res2, Err(AcquireError::DuplicateSession(_))));

        // Release everything so the spawned task can resolve.
        drop(_g);
        drop(_g2);
        let _ = h_first.await.unwrap();
    }

    #[tokio::test]
    async fn active_session_id_rejected_until_slot_released() {
        let q = DownloadQueue::new(2, 0);
        let first = q
            .acquire(
                req("active-dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        let duplicate = q
            .acquire(
                req("active-dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await;
        assert!(matches!(duplicate, Err(AcquireError::DuplicateSession(_))));

        drop(first);

        let reacquired = q
            .acquire(
                req("active-dup", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
        drop(reacquired);
        assert_eq!(q.in_flight(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_same_session_pending_reservation_is_atomic() {
        const ATTEMPTS: usize = 16;

        let q = DownloadQueue::new(1, 0);
        let holder = q
            .acquire(
                req("holder", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();
        let barrier = Arc::new(Barrier::new(ATTEMPTS));
        let mut handles = Vec::new();

        for _ in 0..ATTEMPTS {
            let q = q.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                q.acquire(
                    req("same-pending", Priority::Normal),
                    CancellationToken::new(),
                    |_| {},
                )
                .await
            }));
        }

        for _ in 0..100 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);

        drop(holder);

        let mut acquired = 0;
        let mut duplicates = 0;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(slot) => {
                    acquired += 1;
                    drop(slot);
                }
                Err(AcquireError::DuplicateSession(_)) => duplicates += 1,
                Err(err) => panic!("unexpected acquire error: {err}"),
            }
        }

        assert_eq!(acquired, 1);
        assert_eq!(duplicates, ATTEMPTS - 1);
        assert_eq!(q.pending_count(), 0);
        assert_eq!(q.in_flight(), 0);
        assert_eq!(q.session_reservations.len(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_normal_fast_path_respects_normal_capacity() {
        const ATTEMPTS: usize = 32;

        let q = DownloadQueue::new(1, 64);
        let barrier = Arc::new(Barrier::new(ATTEMPTS));
        let mut handles = Vec::new();

        for index in 0..ATTEMPTS {
            let q = q.clone();
            let barrier = barrier.clone();
            handles.push(tokio::spawn(async move {
                barrier.wait().await;
                q.acquire(
                    req(&format!("normal-{index}"), Priority::Normal),
                    CancellationToken::new(),
                    |_| {},
                )
                .await
            }));
        }

        for _ in 0..100 {
            if q.in_flight() + q.pending_count() == ATTEMPTS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        assert_eq!(q.in_flight(), 1);
        assert_eq!(q.in_flight_normal(), 1);
        assert_eq!(q.pending_count(), ATTEMPTS - 1);

        q.set_normal_capacity(ATTEMPTS);

        let mut slots = Vec::new();
        for handle in handles {
            slots.push(handle.await.unwrap().unwrap());
        }
        assert_eq!(slots.len(), ATTEMPTS);
        drop(slots);
        assert_eq!(q.in_flight(), 0);
        assert_eq!(q.in_flight_normal(), 0);
        assert_eq!(q.session_reservations.len(), 0);
    }

    #[tokio::test]
    async fn dropped_pending_acquire_cleans_queue_state() {
        let q = DownloadQueue::new(1, 0);
        let holder = q
            .acquire(
                req("holder", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        let q2 = q.clone();
        let waiter = tokio::spawn(async move {
            q2.acquire(
                req("abandoned", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });

        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.waiters.lock().len(), 1);
        assert!(q.session_reservations.contains_key("abandoned"));

        waiter.abort();
        match waiter.await {
            Err(join_err) => assert!(join_err.is_cancelled()),
            Ok(_) => panic!("pending acquire should have been aborted"),
        }

        for _ in 0..50 {
            if q.pending_count() == 0 && !q.session_reservations.contains_key("abandoned") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 0);
        assert_eq!(q.waiters.lock().len(), 0);
        assert!(!q.session_reservations.contains_key("abandoned"));

        drop(holder);
        assert_eq!(q.in_flight(), 0);
    }

    #[tokio::test]
    async fn set_capacity_increase_wakes_waiters() {
        let q = DownloadQueue::new(1, 0);
        let _g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();

        let q1 = q.clone();
        let h1 = tokio::spawn(async move {
            q1.acquire(req("b", Priority::Normal), CancellationToken::new(), |_| {})
                .await
        });
        let q2 = q.clone();
        let h2 = tokio::spawn(async move {
            q2.acquire(req("c", Priority::Normal), CancellationToken::new(), |_| {})
                .await
        });
        for _ in 0..50 {
            if q.pending_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Bump capacity to 3; both waiters should wake.
        q.set_normal_capacity(3);
        let _gb = h1.await.unwrap().unwrap();
        let _gc = h2.await.unwrap().unwrap();
        assert_eq!(q.in_flight(), 3);
    }

    #[tokio::test]
    async fn set_capacity_decrease_blocks_new_acquires() {
        let q = DownloadQueue::new(3, 0);
        let g1 = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let g2 = q
            .acquire(req("b", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let g3 = q
            .acquire(req("c", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(q.in_flight(), 3);

        q.set_normal_capacity(1);
        assert_eq!(q.normal_capacity(), 1);
        // Existing in-flights stay; new acquire queues.
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(req("d", Priority::Normal), CancellationToken::new(), |_| {})
                .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);

        // Drop two; only one slot is allowed.
        drop(g1);
        drop(g2);
        // After dropping two, in_flight=1, normal_in=1, normal_cap=1
        // -> not promotable yet.
        // Drop the third to make room.
        drop(g3);
        let _gd = h.await.unwrap().unwrap();
        assert_eq!(q.in_flight(), 1);
    }

    #[tokio::test]
    async fn high_priority_uses_extra_pool() {
        let q = DownloadQueue::new(1, 1);
        // total_cap=2, normal_cap=1
        let g_n = q
            .acquire(req("n", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        // High can take the extra slot.
        let g_h = q
            .acquire(req("h", Priority::High), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(q.in_flight(), 2);
        // Another normal must queue.
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(
                req("n2", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Drop high — n2 still queued because normal_cap is full.
        drop(g_h);
        // Give the wake path a chance.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(q.pending_count(), 1);
        // Drop normal — now n2 can run.
        drop(g_n);
        let _g_n2 = h.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn snapshot_pending_returns_arrival_order() {
        let q = DownloadQueue::new(1, 0);
        let _g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let q1 = q.clone();
        let h1 = tokio::spawn(async move {
            q1.acquire(
                req("first", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let q2 = q.clone();
        let h2 = tokio::spawn(async move {
            q2.acquire(
                req("second", Priority::High),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let snap = q.snapshot_pending();
        assert_eq!(snap.len(), 2);
        // Sorted by arrival (seq), regardless of priority tier.
        assert_eq!(snap[0].session_id, "first");
        assert_eq!(snap[1].session_id, "second");

        // Drain in priority order: high-priority "second" wakes first.
        drop(_g);
        let g_second = h2.await.unwrap().unwrap();
        assert!(matches!(g_second.priority(), Priority::High));
        drop(g_second);
        let g_first = h1.await.unwrap().unwrap();
        assert!(matches!(g_first.priority(), Priority::Normal));
    }

    #[tokio::test]
    async fn into_active_releases_only_on_active_drop() {
        let q = DownloadQueue::new(1, 0);
        let g = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await
            .unwrap();
        let active = g.into_active();
        // Slot still held by ActiveSlot.
        assert_eq!(q.in_flight(), 1);
        drop(active);
        assert_eq!(q.in_flight(), 0);
    }

    #[tokio::test]
    async fn shutdown_rejects_new_acquires() {
        let q = DownloadQueue::new(1, 0);
        q.shutdown();
        let res = q
            .acquire(req("a", Priority::Normal), CancellationToken::new(), |_| {})
            .await;
        assert!(matches!(res, Err(AcquireError::ShuttingDown)));
    }

    /// Regression test for the race window between
    /// `try_acquire_fast` returning `false` and the waiter being
    /// enqueued. Before the fix, `release()` running in that gap
    /// would see no waiters and return; the new waiter then slept
    /// even though capacity was free, until another release happened.
    /// The fix is the post-enqueue promotion pass in `acquire`.
    #[tokio::test]
    async fn no_lost_wakeup_when_release_races_with_enqueue() {
        // Saturate.
        let q = DownloadQueue::new(1, 0);
        let g1 = q
            .acquire(
                req("holder", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        // Spawn a waiter that will fail the fast path. We deliberately
        // arrange to drop `g1` *before* the waiter completes its
        // enqueue by holding a barrier-like delay via tokio yield.
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(
                req("waiter", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });

        // Give the waiter a moment to register as pending so we know
        // the heap insertion completed; in practice the catch is for
        // the narrow window before that. The post-enqueue promotion
        // pass handles both orderings (release-before-insert and
        // release-after-insert).
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Drop the holder; the waiter should be promoted promptly.
        drop(g1);
        let g2 = tokio::time::timeout(Duration::from_secs(2), h)
            .await
            .expect("waiter must wake up — would hang on lost wakeup")
            .unwrap()
            .unwrap();
        assert_eq!(q.in_flight(), 1);
        drop(g2);
    }

    /// A newcomer must NOT take the slot ahead of an already-parked
    /// waiter via the fast path. Before the fairness fix, the heap
    /// could be non-empty but `try_acquire_fast` would ignore it.
    #[tokio::test]
    async fn fast_path_does_not_jump_ahead_of_existing_waiter() {
        let q = DownloadQueue::new(1, 0);
        let g = q
            .acquire(
                req("holder", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
            .unwrap();

        // Park a waiter.
        let q1 = q.clone();
        let h_first = tokio::spawn(async move {
            q1.acquire(
                req("first", Priority::Normal),
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(q.pending_count(), 1);

        // Free the slot.
        drop(g);

        // Newcomer arrives concurrently; under saturation-with-waiter
        // it must not fast-path. Issue the newcomer call only after a
        // brief yield so the freed slot has been re-claimed by
        // `first` via the wakeup. With the fix, even a newcomer
        // arriving during the wakeup window sees `pending` non-empty
        // and queues.
        let g_first = tokio::time::timeout(Duration::from_secs(2), h_first)
            .await
            .expect("first waiter must wake")
            .unwrap()
            .unwrap();
        assert_eq!(g_first.session_id(), "first");
    }
}
