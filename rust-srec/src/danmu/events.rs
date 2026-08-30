//! Danmu service events and commands.
//!
//! This module defines the events emitted by the danmu service and the
//! internal commands used to control collection sessions.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, error, trace};

use crate::danmu::DanmuControlEvent;

use super::lifecycle::CollectionStopReason;

/// Events emitted by the danmu service.
///
/// These events can be subscribed to via `DanmuService::subscribe()` to
/// monitor the progress and status of danmu collection.
#[derive(Debug, Clone)]
pub enum DanmuEvent {
    /// Collection started for a session
    CollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    /// Collection stopped for a session.
    ///
    /// Carries only the message total, not the full `DanmuStatistics`: the
    /// statistics are persisted by `persist_statistics` before this is emitted,
    /// and the aggregate vectors would otherwise be cloned into the broadcast
    /// channel for every subscriber when the sole consumer wants the count.
    CollectionStopped {
        session_id: String,
        total_count: u64,
    },
    /// Segment file started
    SegmentStarted {
        session_id: String,
        streamer_id: String,
        segment_id: String,
        output_path: PathBuf,
        /// The start time of this segment (for danmu timestamp offset calculation).
        start_time: DateTime<Utc>,
    },
    /// Segment file completed
    SegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_id: String,
        output_path: PathBuf,
        message_count: u64,
    },
    /// Platform control event (best-effort signal derived from danmu stream).
    ///
    /// When a provider emits `DanmuControlEvent::StreamClosed`, the runner will shut down
    /// gracefully, which may be followed by `SegmentCompleted` (if a segment is active) and then
    /// `CollectionStopped`.
    Control {
        session_id: String,
        streamer_id: String,
        platform: String,
        control: DanmuControlEvent,
    },
    /// Connection lost; a reconnect attempt is scheduled.
    Reconnecting { session_id: String, attempt: u32 },
    /// An item arrived again after `Reconnecting`, ending the outage.
    Reconnected {
        session_id: String,
        /// Reconnect attempts the recovery took.
        attempts: u32,
        /// How long the link was down.
        downtime_secs: u64,
    },
    /// The link has been down long enough to be worth attention. Collection keeps
    /// reconnecting — `CollectionRunner` gives up only when the session ends — so
    /// this is an alert, not a terminal state.
    ReconnectFailed { session_id: String, error: String },
    /// Error during collection
    Error { session_id: String, error: String },
}

/// Publishes danmu events through the delivery path appropriate to each
/// consumer.
///
/// Runtime coordination uses an unbounded MPSC because these events are
/// low-volume lifecycle transitions and some producers publish from `Drop`,
/// where awaiting capacity is impossible. Broadcast remains a best-effort
/// observer interface and is never the source of runtime state changes.
#[derive(Clone)]
pub(crate) struct DanmuEventPublisher {
    observer_tx: broadcast::Sender<DanmuEvent>,
    coordination_tx: Option<DanmuCoordinationSender>,
}

impl DanmuEventPublisher {
    pub(crate) fn new(observer_capacity: usize) -> Self {
        let (observer_tx, _) = broadcast::channel(observer_capacity);
        Self {
            observer_tx,
            coordination_tx: None,
        }
    }

    pub(crate) fn with_coordination_sender(
        mut self,
        coordination_tx: DanmuCoordinationSender,
    ) -> Self {
        self.coordination_tx = Some(coordination_tx);
        self
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<DanmuEvent> {
        self.observer_tx.subscribe()
    }

    pub(crate) fn publish(&self, event: DanmuEvent) {
        if let Some(sender) = &self.coordination_tx {
            match sender.publish(event.clone()) {
                CoordinationDelivery::Accepted => {}
                // Publishing behind the shutdown marker is the normal tail of a
                // drain, not a fault: `DanmuCoordinationSender::shutdown` closed
                // admission precisely so no further event is applied.
                CoordinationDelivery::ClosedByShutdown => debug!(
                    event = ?event,
                    "Danmu event published after the coordination shutdown marker"
                ),
                CoordinationDelivery::ConsumerGone => error!(
                    event = ?event,
                    "Required danmu coordination event could not be delivered"
                ),
            }
        }

        // Observers are a live view, not a record of what was applied, so they
        // are notified regardless. Suppressing them on a coordination failure
        // would freeze the UI without making the failure any less lost.
        if self.observer_tx.send(event).is_err() {
            trace!("Danmu event had no broadcast observers");
        }
    }
}

enum DanmuCoordinationDelivery {
    Event(DanmuEvent),
    Shutdown(oneshot::Sender<Vec<String>>),
}

/// Sending half of the required runtime coordination channel.
#[derive(Clone)]
pub(crate) struct DanmuCoordinationSender {
    state: Arc<Mutex<DanmuCoordinationState>>,
}

struct DanmuCoordinationState {
    tx: mpsc::UnboundedSender<DanmuCoordinationDelivery>,
    accepting: bool,
}

/// Why a required-coordination publish did or did not reach its consumer.
enum CoordinationDelivery {
    Accepted,
    /// Admission was closed by `DanmuCoordinationSender::shutdown`.
    ClosedByShutdown,
    /// The receiver was dropped without crossing the shutdown marker.
    ConsumerGone,
}

impl DanmuCoordinationSender {
    fn publish(&self, event: DanmuEvent) -> CoordinationDelivery {
        let mut state = self.state.lock();
        if !state.accepting {
            return CoordinationDelivery::ClosedByShutdown;
        }
        if state
            .tx
            .send(DanmuCoordinationDelivery::Event(event))
            .is_err()
        {
            state.accepting = false;
            return CoordinationDelivery::ConsumerGone;
        }
        CoordinationDelivery::Accepted
    }

    /// Wait until every event published before this call has been handled, then
    /// stop the single required consumer.
    pub(crate) async fn shutdown(&self) -> Result<Vec<String>, &'static str> {
        let (acknowledgement, acknowledged) = oneshot::channel();
        {
            let mut state = self.state.lock();
            if !state.accepting {
                return Err("required danmu coordination channel is already closed");
            }
            state.accepting = false;
            state
                .tx
                .send(DanmuCoordinationDelivery::Shutdown(acknowledgement))
                .map_err(|_| "required danmu coordination channel is closed")?;
        }
        acknowledged
            .await
            .map_err(|_| "danmu coordination handler stopped before acknowledging shutdown")
    }
}

/// Receiving half of the required runtime coordination channel.
pub(crate) struct DanmuCoordinationReceiver {
    rx: mpsc::UnboundedReceiver<DanmuCoordinationDelivery>,
    shutdown_acknowledgement: Option<oneshot::Sender<Vec<String>>>,
}

impl DanmuCoordinationReceiver {
    pub(crate) async fn recv(&mut self) -> Result<Option<DanmuEvent>, &'static str> {
        match self.rx.recv().await {
            Some(DanmuCoordinationDelivery::Event(event)) => Ok(Some(event)),
            Some(DanmuCoordinationDelivery::Shutdown(acknowledgement)) => {
                self.shutdown_acknowledgement = Some(acknowledgement);
                Ok(None)
            }
            None => Err("required danmu coordination channel closed without shutdown"),
        }
    }

    pub(crate) fn acknowledge_shutdown(&mut self, failures: Vec<String>) {
        let Some(acknowledgement) = self.shutdown_acknowledgement.take() else {
            debug!("Danmu coordination receiver had no shutdown marker to acknowledge");
            return;
        };
        if acknowledgement.send(failures).is_err() {
            debug!("Danmu coordination shutdown acknowledgement receiver was dropped");
        }
    }
}

pub(crate) fn danmu_coordination_channel() -> (DanmuCoordinationSender, DanmuCoordinationReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        DanmuCoordinationSender {
            state: Arc::new(Mutex::new(DanmuCoordinationState {
                tx,
                accepting: true,
            })),
        },
        DanmuCoordinationReceiver {
            rx,
            shutdown_acknowledgement: None,
        },
    )
}

/// Commands sent to the collection task.
///
/// These are internal commands used to control segment file writing
/// and stop collection from the `CollectionHandle`.
#[derive(Debug)]
pub(crate) enum CollectionCommand {
    /// Start a new segment file
    StartSegment {
        segment_id: String,
        output_path: PathBuf,
        /// The start time of this segment (for danmu timestamp offset calculation).
        start_time: DateTime<Utc>,
    },
    /// End the current segment file
    EndSegment { segment_id: String },
    /// Stop collection entirely
    Stop(CollectionStopReason),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn coordination_delivery_is_lossless_and_drains_before_shutdown() {
        const EVENT_COUNT: usize = 1_024;

        let (coordination_tx, mut coordination_rx) = danmu_coordination_channel();
        let publisher =
            DanmuEventPublisher::new(1).with_coordination_sender(coordination_tx.clone());
        let _dormant_observer = publisher.subscribe();

        for index in 0..EVENT_COUNT {
            publisher.publish(DanmuEvent::SegmentCompleted {
                session_id: format!("session-{index}"),
                streamer_id: "streamer-1".to_string(),
                segment_id: index.to_string(),
                output_path: PathBuf::from(format!("segment-{index}.xml")),
                message_count: index as u64,
            });
        }

        let consumer = tokio::spawn(async move {
            let mut received = 0;
            while let Some(event) = coordination_rx
                .recv()
                .await
                .expect("coordination channel stays open until shutdown")
            {
                let DanmuEvent::SegmentCompleted {
                    session_id,
                    segment_id,
                    message_count,
                    ..
                } = event
                else {
                    panic!("unexpected coordination event");
                };
                assert_eq!(session_id, format!("session-{received}"));
                assert_eq!(segment_id, received.to_string());
                assert_eq!(message_count, received as u64);
                received += 1;
            }
            coordination_rx.acknowledge_shutdown(Vec::new());
            received
        });

        coordination_tx
            .shutdown()
            .await
            .expect("shutdown is acknowledged after queued events");

        assert_eq!(
            consumer.await.expect("consumer task completes"),
            EVENT_COUNT
        );
        assert!(
            matches!(
                coordination_tx.publish(DanmuEvent::CollectionStopped {
                    session_id: "late-session".to_string(),
                    total_count: 0,
                }),
                CoordinationDelivery::ClosedByShutdown
            ),
            "an event must not be admitted behind the shutdown marker"
        );
    }
}
