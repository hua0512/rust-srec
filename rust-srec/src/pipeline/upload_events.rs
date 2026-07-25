//! Live upload-status fan-out to WebSocket subscribers.
//!
//! `JobQueue` publishes one event per upload-job lifecycle transition
//! (dequeue → `Started`, progress-aggregator flush → `Progress`,
//! complete/fail/cancel → `Terminal`). The `/api/downloads/ws` route
//! subscribes and forwards the pre-encoded bytes to connected clients,
//! applying its per-connection streamer filter on the Rust event.
//!
//! Mirrors `monitor::check_history_writer::CheckHistoryBroadcaster`:
//! protobuf encoding runs **once per event** in the producer (not once per
//! subscriber), and is skipped entirely when no client is connected. The
//! encoder closure lives in the API layer (it knows the proto types) and is
//! injected by `services::container` so this module never depends on
//! `crate::api`.

use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::broadcast;

use super::progress::JobProgressSnapshot;

/// Matches `download_manager`'s broadcast capacity. A subscriber that lags
/// more than this many events re-syncs from the next `DownloadSnapshot`
/// (sent on WS subscribe/unsubscribe) rather than reconciling drops.
const BROADCAST_CAPACITY: usize = 256;

/// Terminal outcome of an upload job as broadcast to WS clients. Distinct
/// from the per-file `UploadItemStatus`: this is job-level, and includes
/// `Cancelled` (which never produces `upload_records` rows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadTerminalStatus {
    Completed,
    Failed,
    Cancelled,
}

/// One upload lifecycle transition, published by `JobQueue`.
#[derive(Debug, Clone)]
pub enum UploadStatusEvent {
    Started {
        job_id: String,
        streamer_id: Option<String>,
        session_id: Option<String>,
        /// Upload kind from `job_queue::upload_kind_for_job_type` ("rclone").
        uploader: &'static str,
        files_total: u32,
        started_at_ms: i64,
    },
    Progress {
        job_id: String,
        streamer_id: Option<String>,
        snapshot: JobProgressSnapshot,
    },
    Terminal {
        job_id: String,
        streamer_id: Option<String>,
        status: UploadTerminalStatus,
        files_succeeded: u32,
        files_failed: u32,
        files_skipped: u32,
        error: Option<String>,
    },
}

impl UploadStatusEvent {
    /// Streamer the event belongs to; the WS route's per-connection filter
    /// compares against this (same contract as
    /// `DownloadManagerEvent::streamer_id`).
    pub fn streamer_id(&self) -> Option<&str> {
        match self {
            UploadStatusEvent::Started { streamer_id, .. }
            | UploadStatusEvent::Progress { streamer_id, .. }
            | UploadStatusEvent::Terminal { streamer_id, .. } => streamer_id.as_deref(),
        }
    }

    pub fn job_id(&self) -> &str {
        match self {
            UploadStatusEvent::Started { job_id, .. }
            | UploadStatusEvent::Progress { job_id, .. }
            | UploadStatusEvent::Terminal { job_id, .. } => job_id,
        }
    }
}

/// Turns an [`UploadStatusEvent`] into the bytes the WS route ships to
/// subscribers. Injected from the API layer; see the module docs.
pub type UploadWsEncoder = Arc<dyn Fn(&UploadStatusEvent) -> Bytes + Send + Sync + 'static>;

/// One broadcast unit: the event (for Rust-side filtering) plus the
/// pre-encoded protobuf bytes. Both fields are refcounted — cloning across
/// N subscribers is atomics, not heap copies.
#[derive(Clone)]
pub struct UploadBroadcastEnvelope {
    pub event: Arc<UploadStatusEvent>,
    pub ws_bytes: Bytes,
}

/// Cheap-to-clone fan-out handle. `JobQueue` holds one for publishing; the
/// WS route clones it into each connection for `subscribe()`.
#[derive(Clone)]
pub struct UploadStatusBroadcaster {
    tx: broadcast::Sender<UploadBroadcastEnvelope>,
    encoder: UploadWsEncoder,
}

impl UploadStatusBroadcaster {
    pub fn new(encoder: UploadWsEncoder) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx, encoder }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UploadBroadcastEnvelope> {
        self.tx.subscribe()
    }

    /// True when at least one WS connection is subscribed. `send` already
    /// early-returns without one, but producers on hot paths (the progress
    /// aggregator flush) check this first to skip building the event —
    /// cloning a `JobProgressSnapshot` and its `raw` JSON tree — at all.
    pub fn has_subscribers(&self) -> bool {
        self.tx.receiver_count() > 0
    }

    /// Encode once and fan out. No subscribers → no encode, no send; an
    /// idle or headless deployment pays nothing on the job hot path.
    pub fn send(&self, event: UploadStatusEvent) {
        if self.tx.receiver_count() == 0 {
            return;
        }
        let event = Arc::new(event);
        let ws_bytes = (self.encoder)(&event);
        let _ = self.tx.send(UploadBroadcastEnvelope { event, ws_bytes });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(job_id: &str, streamer_id: Option<&str>) -> UploadStatusEvent {
        UploadStatusEvent::Started {
            job_id: job_id.to_string(),
            streamer_id: streamer_id.map(str::to_string),
            session_id: None,
            uploader: "rclone",
            files_total: 3,
            started_at_ms: 1_000,
        }
    }

    fn test_encoder() -> UploadWsEncoder {
        Arc::new(|event| Bytes::from(format!("encoded:{}", event.job_id())))
    }

    #[tokio::test]
    async fn send_with_no_subscribers_skips_encoding() {
        let panicking: UploadWsEncoder =
            Arc::new(|_| panic!("encoder must not run with no subscribers"));
        let b = UploadStatusBroadcaster::new(panicking);
        b.send(started("job-1", None));
    }

    #[tokio::test]
    async fn fan_out_shares_one_encode_across_subscribers() {
        let b = UploadStatusBroadcaster::new(test_encoder());
        let mut sub_a = b.subscribe();
        let mut sub_b = b.subscribe();

        b.send(started("job-1", Some("streamer-1")));

        let got_a = sub_a.try_recv().expect("subscriber A receives");
        let got_b = sub_b.try_recv().expect("subscriber B receives");
        assert!(Arc::ptr_eq(&got_a.event, &got_b.event));
        assert_eq!(got_a.ws_bytes, b"encoded:job-1".as_ref());
        assert_eq!(got_a.event.streamer_id(), Some("streamer-1"));
    }
}
