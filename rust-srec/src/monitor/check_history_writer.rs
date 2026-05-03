//! Best-effort writer for the `streamer_check_history` ring buffer.
//!
//! The streamer details page renders an "uptime-bar" check-history strip
//! showing one bar per monitor poll. The data behind it flows here:
//! every call to [`MonitorStatusChecker::check_status`] hands a
//! [`CheckRecord`] to a bounded MPSC channel, and a single background task
//! drains the channel into SQLite via [`StreamerCheckHistoryRepository`],
//! then broadcasts the same `CheckRecord` to subscribed WebSocket clients.
//!
//! Three guarantees this module enforces:
//!
//! 1. **Polling latency is unaffected by DB latency.** The hot path calls
//!    [`CheckHistoryWriter::record`] which uses `try_send` and drops the
//!    record on full. Diagnostic rows are nice-to-have; missing one bar in
//!    the strip is far better than blocking the lifecycle FSM behind a
//!    slow SQLite write.
//! 2. **A failed insert never propagates.** The writer task logs and moves
//!    on. Telemetry must not become a new failure surface for the monitor.
//! 3. **Per-streamer retention is bounded** by the repository's insert-time
//!    trim (see [`super::super::database::repositories::KEEP_PER_STREAMER`]),
//!    so the writer doesn't need a separate maintenance pass.
//!
//! [`MonitorStatusChecker::check_status`]: crate::scheduler::actor::MonitorStatusChecker

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

use crate::database::models::StreamerCheckHistoryDbModel;
use crate::database::repositories::StreamerCheckHistoryRepository;
use crate::domain::streamer::CheckRecord;

/// Channel capacity for buffered check records.
///
/// At a default `check_interval_ms = 60000` and ~hundreds of streamers, the
/// steady-state arrival rate is well under 10 records/sec. The writer drains
/// roughly as fast as SQLite can commit (~1k commits/sec on typical disks),
/// so a 1024-record buffer absorbs DB stalls of several seconds before the
/// `try_send` starts dropping. That's the right tradeoff: temporary stalls
/// don't lose records; sustained backpressure (a permanently slow disk)
/// loses the diagnostic strip rather than the monitor.
const CHANNEL_CAPACITY: usize = 1024;

/// Broadcast channel capacity for the live-update fan-out. Matches the
/// pattern used by `download_manager` (256). Per-WebSocket subscribers can
/// lag up to this many events before the receiver returns `Lagged` —
/// clients that fall this far behind will refetch the REST endpoint on the
/// next React Query refetch tick rather than try to reconcile dropped
/// events.
const BROADCAST_CAPACITY: usize = 256;

/// Closure that turns a [`CheckRecord`] into the bytes the WebSocket
/// route ships to subscribers. Owned by the broadcaster so the encoding
/// runs **once per record** in the drain task — not once per subscriber
/// in the WS route's select loop. With N connected clients, this saves
/// N − 1 protobuf encodes per record.
///
/// The `Send + Sync` bounds let the closure be cloned into the broadcaster
/// (one clone, lives for the broadcaster's lifetime) and called from the
/// drain task. `'static` because the broadcaster outlives any individual
/// caller.
pub type WsEncoder = Arc<dyn Fn(&CheckRecord) -> Bytes + Send + Sync + 'static>;

/// One broadcast unit. The `record` is what existing subscribers already
/// keyed off of (streamer-id filter, future record-shape consumers); the
/// `ws_bytes` are the pre-encoded protobuf payload the WS route hands to
/// every subscriber without re-encoding.
///
/// Both fields are cheap to clone — `Arc` is one atomic op, `Bytes` is a
/// refcounted slice. Cloning the envelope across N broadcast subscribers
/// is two atomics per subscriber, no heap allocation.
#[derive(Clone)]
pub struct BroadcastEnvelope {
    pub record: Arc<CheckRecord>,
    pub ws_bytes: Bytes,
}

/// Live-update broadcaster cloned into the WS route loop. The sender is
/// held by the drain task; the WS route subscribes via `subscribe()` to
/// receive every committed [`BroadcastEnvelope`].
///
/// Carries a `BroadcastEnvelope` (record + pre-encoded WS bytes) rather
/// than a bare `Arc<CheckRecord>` so the protobuf encoding runs once
/// per record at production time, not once per subscriber at consumption
/// time.
#[derive(Clone)]
pub struct CheckHistoryBroadcaster {
    tx: broadcast::Sender<BroadcastEnvelope>,
    encoder: WsEncoder,
}

impl CheckHistoryBroadcaster {
    /// Build a broadcaster with the standard fan-out capacity and a
    /// caller-supplied encoder. The encoder lives in the API layer (it
    /// knows about proto types); passing it in keeps the writer module
    /// from depending on `crate::api`.
    pub fn new(encoder: WsEncoder) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx, encoder }
    }

    /// Subscribe to record commits. Each subscriber gets every envelope
    /// published after subscription; envelopes published before this call
    /// are not replayed (the WS route's initial backlog comes from the
    /// REST endpoint, same pattern as download progress).
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEnvelope> {
        self.tx.subscribe()
    }

    /// Build the envelope (encode WS bytes once) and send to subscribers.
    /// Drops silently when there are no subscribers (typical at boot
    /// before any client opens the WS) — and importantly, skips the
    /// encode when there are no subscribers so an idle deployment costs
    /// nothing on the hot path.
    pub(crate) fn send(&self, record: Arc<CheckRecord>) {
        // No subscribers → no encode. Saves the work entirely on
        // headless deployments.
        if self.tx.receiver_count() == 0 {
            return;
        }
        let ws_bytes = (self.encoder)(&record);
        let _ = self.tx.send(BroadcastEnvelope { record, ws_bytes });
    }
}

/// Handle that the [`MonitorStatusChecker`] uses to enqueue check records.
/// Cheap to clone — wraps an [`mpsc::Sender`].
///
/// [`MonitorStatusChecker`]: crate::scheduler::actor::MonitorStatusChecker
#[derive(Clone)]
pub struct CheckHistoryWriter {
    tx: mpsc::Sender<Arc<CheckRecord>>,
}

impl CheckHistoryWriter {
    /// Build a writer + receiver pair. The caller is expected to spawn
    /// [`run`] with the returned receiver and a repository handle.
    pub fn new() -> (Self, mpsc::Receiver<Arc<CheckRecord>>) {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        (Self { tx }, rx)
    }

    /// Enqueue a record, dropping silently on full or closed channel.
    ///
    /// Wraps the record in `Arc` once at the producer boundary so the
    /// drain task can hand the same instance to both the repository
    /// (`&*arc`) and the broadcaster (`Arc::clone`) without cloning the
    /// inner Strings + candidate Vec.
    ///
    /// Called from the monitor adapter on the polling hot path. Must
    /// never `await` on send — that would couple polling cadence to DB
    /// latency.
    pub fn record(&self, record: CheckRecord) {
        self.record_arc(Arc::new(record));
    }

    /// Internal variant for the test module that wants to verify
    /// drop-on-full behavior without re-wrapping in Arc each call.
    fn record_arc(&self, record: Arc<CheckRecord>) {
        match self.tx.try_send(record) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(record)) => {
                warn!(
                    streamer_id = %record.streamer_id,
                    "check-history channel full; dropping record"
                );
            }
            Err(mpsc::error::TrySendError::Closed(record)) => {
                trace!(
                    streamer_id = %record.streamer_id,
                    "check-history channel closed; dropping record"
                );
            }
        }
    }
}

/// Drain records from `rx` into the repository until the channel closes
/// or `cancel` fires.
///
/// Insert failures are logged and discarded — the strip is best-effort
/// and must not become a new error surface for the monitor.
///
/// On every successful insert, the same `Arc<CheckRecord>` is broadcast
/// to live WebSocket subscribers via `broadcaster` (when set). The
/// broadcaster's `send` drops silently when there are no subscribers
/// (typical at boot), so having no clients connected costs nothing on
/// the hot path. The Arc means fan-out to N subscribers is N atomic
/// refcount bumps, not N record clones.
pub async fn run<R>(
    repo: Arc<R>,
    mut rx: mpsc::Receiver<Arc<CheckRecord>>,
    broadcaster: Option<CheckHistoryBroadcaster>,
    cancel: CancellationToken,
) where
    R: StreamerCheckHistoryRepository + ?Sized + 'static,
{
    debug!("streamer-check-history writer started");
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("streamer-check-history writer cancelled");
                break;
            }
            received = rx.recv() => {
                let Some(record) = received else {
                    debug!("streamer-check-history writer channel closed");
                    break;
                };
                let row = StreamerCheckHistoryDbModel::from(record.as_ref());
                match repo.insert(&row).await {
                    Ok(()) => {
                        // Row is durable in SQLite; safe to fan out to
                        // live subscribers.
                        if let Some(b) = &broadcaster {
                            b.send(Arc::clone(&record));
                        }
                    }
                    Err(err) => {
                        warn!(
                            streamer_id = %record.streamer_id,
                            outcome = %record.outcome.as_str(),
                            error = %err,
                            "failed to persist streamer check-history record; dropping"
                        );
                        // Deliberately do NOT broadcast on insert failure —
                        // the live update would lie about durability and
                        // a refetch wouldn't reconcile (the row isn't in DB).
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};

    fn record(streamer_id: &str, msg: &str) -> CheckRecord {
        CheckRecord::from_error(
            streamer_id,
            Utc::now(),
            ChronoDuration::milliseconds(1),
            msg,
        )
    }

    /// Encoder stub for tests — produces a tiny deterministic byte string
    /// derived from `streamer_id` so tests can assert on the bytes that
    /// reach subscribers without depending on the API layer's proto
    /// codegen.
    fn test_encoder() -> WsEncoder {
        Arc::new(|record| Bytes::from(format!("encoded:{}", record.streamer_id)))
    }

    #[tokio::test]
    async fn writer_record_drops_silently_when_buffer_full() {
        // Build a writer with a 1-slot channel by constructing manually so
        // we can exercise the Full branch without queueing thousands of records.
        let (tx, _rx) = mpsc::channel::<Arc<CheckRecord>>(1);
        let writer = CheckHistoryWriter { tx };

        writer.record(record("s1", "first")); // fills slot
        // _rx is never read, so the next record hits Full — must NOT panic
        // and must NOT block.
        writer.record(record("s2", "second"));
    }

    #[tokio::test]
    async fn broadcaster_send_with_no_subscribers_does_not_panic() {
        // Producer-side `send` returns early when receiver_count == 0,
        // so the encoder is not invoked. We use a panicking encoder
        // here to assert that contract.
        let panicking: WsEncoder = Arc::new(|_| panic!("encoder must not run with no subscribers"));
        let b = CheckHistoryBroadcaster::new(panicking);
        b.send(Arc::new(record("s1", "no listener")));
    }

    #[tokio::test]
    async fn broadcaster_fan_out_delivers_to_all_subscribers() {
        let b = CheckHistoryBroadcaster::new(test_encoder());
        let mut sub_a = b.subscribe();
        let mut sub_b = b.subscribe();

        b.send(Arc::new(record("s1", "boom")));

        let got_a = sub_a
            .try_recv()
            .expect("subscriber A receives the envelope");
        let got_b = sub_b
            .try_recv()
            .expect("subscriber B receives the envelope");
        assert_eq!(got_a.record.streamer_id, "s1");
        assert_eq!(got_b.record.streamer_id, "s1");
        // Same `Arc<CheckRecord>` reaches both subscribers — refcount
        // semantics mean the inner record allocates exactly once.
        assert!(Arc::ptr_eq(&got_a.record, &got_b.record));
        // Pre-encoded bytes reach both subscribers identical, no
        // re-encode in the receive path.
        assert_eq!(got_a.ws_bytes, b"encoded:s1".as_ref());
        assert_eq!(got_b.ws_bytes, got_a.ws_bytes);
    }

    #[tokio::test]
    async fn broadcaster_encodes_once_per_record_regardless_of_subscriber_count() {
        // Counts how many times the encoder is invoked across N
        // subscribers receiving one record. Must be exactly 1 — the
        // whole point of pre-encoding at the producer.
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let counted: WsEncoder = {
            let calls = Arc::clone(&calls);
            Arc::new(move |record| {
                calls.fetch_add(1, Ordering::SeqCst);
                Bytes::from(format!("encoded:{}", record.streamer_id))
            })
        };
        let b = CheckHistoryBroadcaster::new(counted);
        let _sub_a = b.subscribe();
        let _sub_b = b.subscribe();
        let _sub_c = b.subscribe();

        b.send(Arc::new(record("s1", "x")));

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "encoder must run once per record, not once per subscriber"
        );
    }

    #[tokio::test]
    async fn run_broadcasts_after_successful_insert() {
        use crate::Result;
        use async_trait::async_trait;
        use std::sync::Mutex;

        // Tiny in-memory repo that records every inserted row so we can
        // assert the writer's commit-then-broadcast ordering: by the time
        // a subscriber sees a record, the repo has already accepted the row.
        struct InMemoryRepo {
            inserts: Mutex<Vec<StreamerCheckHistoryDbModel>>,
        }
        #[async_trait]
        impl StreamerCheckHistoryRepository for InMemoryRepo {
            async fn insert(&self, row: &StreamerCheckHistoryDbModel) -> Result<()> {
                self.inserts.lock().unwrap().push(row.clone());
                Ok(())
            }
            async fn list_recent(
                &self,
                _: &str,
                _: i64,
            ) -> Result<Vec<StreamerCheckHistoryDbModel>> {
                Ok(Vec::new())
            }
        }

        let repo = Arc::new(InMemoryRepo {
            inserts: Mutex::new(Vec::new()),
        });
        let (writer, rx) = CheckHistoryWriter::new();
        let broadcaster = CheckHistoryBroadcaster::new(test_encoder());
        let mut sub = broadcaster.subscribe();
        let cancel = CancellationToken::new();

        let task = tokio::spawn(run(
            repo.clone(),
            rx,
            Some(broadcaster.clone()),
            cancel.clone(),
        ));

        writer.record(record("s1", "x"));

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .expect("subscriber receives within 1s");
        let got = received.expect("not lagged or closed");
        assert_eq!(got.record.streamer_id, "s1");
        assert_eq!(got.ws_bytes, b"encoded:s1".as_ref());
        assert_eq!(
            repo.inserts.lock().unwrap().len(),
            1,
            "record was persisted before being broadcast"
        );

        cancel.cancel();
        let _ = task.await;
    }

    #[tokio::test]
    async fn run_does_not_broadcast_on_insert_failure() {
        // Mirror the production contract: if SQLite rejects a row, the
        // live update would lie about durability. The drain task must
        // skip the broadcast on failure.
        use crate::Result;
        use async_trait::async_trait;

        struct AlwaysFailRepo;
        #[async_trait]
        impl StreamerCheckHistoryRepository for AlwaysFailRepo {
            async fn insert(&self, _: &StreamerCheckHistoryDbModel) -> Result<()> {
                Err(crate::Error::Other("simulated DB failure".to_string()))
            }
            async fn list_recent(
                &self,
                _: &str,
                _: i64,
            ) -> Result<Vec<StreamerCheckHistoryDbModel>> {
                Ok(Vec::new())
            }
        }

        let repo = Arc::new(AlwaysFailRepo);
        let (writer, rx) = CheckHistoryWriter::new();
        let broadcaster = CheckHistoryBroadcaster::new(test_encoder());
        let mut sub = broadcaster.subscribe();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run(repo, rx, Some(broadcaster.clone()), cancel.clone()));

        writer.record(record("s1", "x"));

        let res = tokio::time::timeout(std::time::Duration::from_millis(100), sub.recv()).await;
        assert!(res.is_err(), "no broadcast on insert failure");

        cancel.cancel();
        let _ = task.await;
    }
}
