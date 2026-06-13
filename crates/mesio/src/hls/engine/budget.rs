//! Counting byte-reservation primitives for pipeline backpressure.
//!
//! Both budgets are reservation primitives, not bare counters you read then
//! charge: capacity is acquired up front and held by an RAII guard
//! ([`ByteReservation`]) that releases on drop. Download bytes are reserved at
//! admission inside `SegmentStateStore::next_ready_jobs` (closing the
//! check-then-spawn-then-reserve race); processing bytes are reserved at the
//! encrypted-input upper bound before decrypt and reconciled after.

use std::collections::VecDeque;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;

/// Reservation refused because the budget cannot currently fit the request.
/// `grow` is non-blocking and returns this rather than waiting, so a caller
/// that cannot grow aborts instead of risking a mutual-wait deadlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetExceeded;

/// The request can never fit: it exceeds the semaphore's total capacity.
/// Maps to a terminal (oversize) segment failure, never a retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Oversize;

#[derive(Debug)]
struct SemState {
    available: u64,
    /// FIFO wait queue so one large reservation cannot be starved indefinitely
    /// by a stream of smaller ones. Only the head waiter is ever granted.
    queue: VecDeque<Waiter>,
    next_waiter_id: u64,
}

#[derive(Debug)]
struct Waiter {
    id: u64,
    bytes: u64,
    notify: Arc<Notify>,
}

#[derive(Debug)]
struct SemInner {
    capacity: u64,
    state: Mutex<SemState>,
}

impl SemInner {
    /// Wake the head waiter if its request now fits. Only the head is woken:
    /// granting out of order would break FIFO fairness.
    fn wake_head(state: &SemState) {
        if let Some(head) = state.queue.front()
            && state.available >= head.bytes
        {
            head.notify.notify_one();
        }
    }

    fn release(&self, bytes: u64) {
        let state = &mut *self.state.lock();
        state.available = (state.available + bytes).min(self.capacity);
        Self::wake_head(state);
    }
}

/// Async, FIFO-fair, byte-counting semaphore.
#[derive(Debug, Clone)]
pub struct ByteSemaphore {
    inner: Arc<SemInner>,
}

impl ByteSemaphore {
    /// `capacity == 0` means unlimited (every reservation succeeds and holds
    /// no real capacity); useful for disabling a budget via config.
    pub fn new(capacity: u64) -> Self {
        Self {
            inner: Arc::new(SemInner {
                capacity,
                state: Mutex::new(SemState {
                    available: capacity,
                    queue: VecDeque::new(),
                    next_waiter_id: 0,
                }),
            }),
        }
    }

    pub fn capacity(&self) -> u64 {
        self.inner.capacity
    }

    fn unlimited(&self) -> bool {
        self.inner.capacity == 0
    }

    pub fn available(&self) -> u64 {
        if self.unlimited() {
            return u64::MAX;
        }
        self.inner.state.lock().available
    }

    /// Non-blocking reservation. Refuses (returns `None`) when the bytes do
    /// not currently fit *or* when waiters are queued — barging past the queue
    /// would starve the FIFO waiters in `reserve`.
    pub fn try_reserve(&self, bytes: u64) -> Option<ByteReservation> {
        if self.unlimited() {
            return Some(ByteReservation::unlimited(self.inner.clone()));
        }
        if bytes > self.inner.capacity {
            return None;
        }
        let state = &mut *self.inner.state.lock();
        if state.queue.is_empty() && state.available >= bytes {
            state.available -= bytes;
            Some(ByteReservation::held(self.inner.clone(), bytes))
        } else {
            None
        }
    }

    /// FIFO async reservation. Returns `Err(Oversize)` when the request can
    /// never fit (exceeds total capacity) so callers terminalize instead of
    /// parking forever. Cancel-safe: dropping the future removes its queue
    /// entry and wakes the next waiter.
    pub async fn reserve(&self, bytes: u64) -> Result<ByteReservation, Oversize> {
        if self.unlimited() {
            return Ok(ByteReservation::unlimited(self.inner.clone()));
        }
        if bytes > self.inner.capacity {
            return Err(Oversize);
        }

        let (id, notify) = {
            let state = &mut *self.inner.state.lock();
            if state.queue.is_empty() && state.available >= bytes {
                state.available -= bytes;
                return Ok(ByteReservation::held(self.inner.clone(), bytes));
            }
            let id = state.next_waiter_id;
            state.next_waiter_id += 1;
            let notify = Arc::new(Notify::new());
            state.queue.push_back(Waiter {
                id,
                bytes,
                notify: Arc::clone(&notify),
            });
            (id, notify)
        };

        // Removes the queue entry if this future is dropped before being
        // granted; bytes only ever transfer inside the lock below, so a
        // cancelled waiter can never leak capacity.
        let mut dequeue_guard = DequeueOnDrop {
            inner: &self.inner,
            id,
            armed: true,
        };

        loop {
            notify.notified().await;
            let state = &mut *self.inner.state.lock();
            let is_head = state.queue.front().is_some_and(|w| w.id == id);
            if is_head && state.available >= bytes {
                state.available -= bytes;
                state.queue.pop_front();
                dequeue_guard.armed = false;
                // More capacity may remain for the next waiter.
                SemInner::wake_head(state);
                return Ok(ByteReservation::held(self.inner.clone(), bytes));
            }
            // Spurious or stale wake-up: re-arm by re-checking on next notify.
            SemInner::wake_head(state);
        }
    }
}

struct DequeueOnDrop<'a> {
    inner: &'a Arc<SemInner>,
    id: u64,
    armed: bool,
}

impl Drop for DequeueOnDrop<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let state = &mut *self.inner.state.lock();
        if let Some(pos) = state.queue.iter().position(|w| w.id == self.id) {
            state.queue.remove(pos);
        }
        SemInner::wake_head(state);
    }
}

/// RAII byte reservation. Releases its held bytes on drop.
#[derive(Debug)]
pub struct ByteReservation {
    inner: Arc<SemInner>,
    held: u64,
    /// Reservations from an unlimited (capacity 0) semaphore hold nothing.
    counted: bool,
}

impl ByteReservation {
    fn held(inner: Arc<SemInner>, bytes: u64) -> Self {
        Self {
            inner,
            held: bytes,
            counted: true,
        }
    }

    fn unlimited(inner: Arc<SemInner>) -> Self {
        Self {
            inner,
            held: 0,
            counted: false,
        }
    }

    pub fn held_bytes(&self) -> u64 {
        self.held
    }

    /// Try to acquire `extra` bytes on top of the current holding, so a
    /// reservation can track a body whose real size exceeds the admission
    /// estimate. Non-blocking: returns `Err` rather than waiting (many tasks
    /// all blocking to grow would deadlock against each other).
    pub fn grow(&mut self, extra: u64) -> Result<(), BudgetExceeded> {
        if !self.counted || extra == 0 {
            return Ok(());
        }
        if self
            .held
            .checked_add(extra)
            .is_none_or(|total| total > self.inner.capacity)
        {
            return Err(BudgetExceeded);
        }
        let state = &mut *self.inner.state.lock();
        if state.available >= extra {
            state.available -= extra;
            self.held += extra;
            Ok(())
        } else {
            Err(BudgetExceeded)
        }
    }

    /// Shrink the reservation to the now-known true size, releasing the
    /// difference. Never grows — over-actual sizes go through `grow` so the
    /// budget check applies.
    pub fn reconcile(&mut self, actual: u64) {
        if !self.counted || actual >= self.held {
            return;
        }
        let release = self.held - actual;
        self.held = actual;
        self.inner.release(release);
    }
}

impl Drop for ByteReservation {
    fn drop(&mut self) {
        if self.counted && self.held > 0 {
            self.inner.release(self.held);
        }
    }
}

/// The shared byte budgets, owned by the runtime and shared by the reactor
/// (download admission) and every fetch-and-process task (body growth, crypto
/// gate). Deliberately *not* part of `SegmentStateStore`: the store is the
/// reactor's single-owner state and spawned tasks must never touch it.
#[derive(Debug)]
pub struct ByteBudget {
    /// `max_inflight_download_bytes`: raw response bodies, from admission until
    /// consumed (wrapped on the clear path, fed to decrypt on the encrypted
    /// path).
    pub download: ByteSemaphore,
    /// `max_processing_bytes`: decrypted/transformed output resident in the
    /// decrypt stage, reserved at the encrypted-input upper bound.
    pub processing: ByteSemaphore,
}

impl ByteBudget {
    pub fn new(max_inflight_download_bytes: u64, max_processing_bytes: u64) -> Self {
        Self {
            download: ByteSemaphore::new(max_inflight_download_bytes),
            processing: ByteSemaphore::new(max_processing_bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn try_reserve_charges_and_drop_releases() {
        let sem = ByteSemaphore::new(100);
        let r = sem.try_reserve(60).expect("fits");
        assert_eq!(sem.available(), 40);
        assert!(sem.try_reserve(50).is_none());
        drop(r);
        assert_eq!(sem.available(), 100);
    }

    #[test]
    fn grow_respects_capacity() {
        let sem = ByteSemaphore::new(100);
        let mut r = sem.try_reserve(60).expect("fits");
        assert!(r.grow(40).is_ok());
        assert_eq!(sem.available(), 0);
        assert_eq!(r.grow(1), Err(BudgetExceeded));
        drop(r);
        assert_eq!(sem.available(), 100);
    }

    #[test]
    fn reconcile_releases_difference_and_never_grows() {
        let sem = ByteSemaphore::new(100);
        let mut r = sem.try_reserve(80).expect("fits");
        r.reconcile(30);
        assert_eq!(r.held_bytes(), 30);
        assert_eq!(sem.available(), 70);
        // Reconciling upward is a no-op; growth must go through `grow`.
        r.reconcile(50);
        assert_eq!(r.held_bytes(), 30);
        assert_eq!(sem.available(), 70);
    }

    #[test]
    fn oversize_request_is_refused_not_queued() {
        let sem = ByteSemaphore::new(100);
        assert!(sem.try_reserve(101).is_none());
    }

    #[tokio::test]
    async fn reserve_returns_oversize_for_impossible_request() {
        let sem = ByteSemaphore::new(100);
        assert_eq!(sem.reserve(101).await.unwrap_err(), Oversize);
    }

    #[tokio::test]
    async fn reserve_waits_until_capacity_freed() {
        let sem = ByteSemaphore::new(100);
        let r = sem.try_reserve(100).expect("fits");

        let sem2 = sem.clone();
        let waiter = tokio::spawn(async move { sem2.reserve(50).await });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(!waiter.is_finished(), "must wait while budget is full");

        drop(r);
        let granted = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("granted after release")
            .expect("join")
            .expect("not oversize");
        assert_eq!(granted.held_bytes(), 50);
    }

    #[tokio::test]
    async fn waiters_are_granted_fifo() {
        let sem = ByteSemaphore::new(100);
        let hold = sem.try_reserve(100).expect("fits");

        // First waiter wants a large chunk, second a small one. FIFO means the
        // small one must NOT jump the queue when only 80 bytes free up.
        let sem_a = sem.clone();
        let a = tokio::spawn(async move { sem_a.reserve(90).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sem_b = sem.clone();
        let b = tokio::spawn(async move { sem_b.reserve(10).await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        drop(hold);
        let ra = tokio::time::timeout(Duration::from_secs(1), a)
            .await
            .expect("a granted first")
            .unwrap()
            .unwrap();
        assert_eq!(ra.held_bytes(), 90);
        let rb = tokio::time::timeout(Duration::from_secs(1), b)
            .await
            .expect("b granted next")
            .unwrap()
            .unwrap();
        assert_eq!(rb.held_bytes(), 10);
    }

    #[tokio::test]
    async fn try_reserve_does_not_barge_past_waiters() {
        let sem = ByteSemaphore::new(100);
        let hold = sem.try_reserve(60).expect("fits");

        let sem_w = sem.clone();
        let waiter = tokio::spawn(async move { sem_w.reserve(80).await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        // 40 bytes are free, but a waiter is queued: refuse.
        assert!(sem.try_reserve(40).is_none());

        drop(hold);
        let r = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("waiter granted")
            .unwrap()
            .unwrap();
        assert_eq!(r.held_bytes(), 80);
    }

    #[tokio::test]
    async fn cancelled_waiter_releases_queue_position() {
        let sem = ByteSemaphore::new(100);
        let hold = sem.try_reserve(100).expect("fits");

        let sem_a = sem.clone();
        let a = tokio::spawn(async move { sem_a.reserve(90).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let sem_b = sem.clone();
        let b = tokio::spawn(async move { sem_b.reserve(10).await });
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Cancel the head waiter; the second must then be grantable.
        a.abort();
        let _ = a.await;

        drop(hold);
        let rb = tokio::time::timeout(Duration::from_secs(1), b)
            .await
            .expect("b granted after head cancelled")
            .unwrap()
            .unwrap();
        assert_eq!(rb.held_bytes(), 10);
    }

    #[test]
    fn unlimited_semaphore_never_refuses() {
        let sem = ByteSemaphore::new(0);
        let mut r = sem.try_reserve(u64::MAX).expect("unlimited");
        assert!(r.grow(u64::MAX).is_ok());
        r.reconcile(1);
        drop(r);
    }
}
