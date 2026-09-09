//! Acknowledged event delivery and observer broadcasts.

use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::debug;

use super::DownloadManagerEvent;

#[derive(Debug, thiserror::Error)]
pub(super) enum PublicationError {
    #[error("scheduler feedback capacity is full; retry admission")]
    FeedbackBusy,
    #[error("{0}")]
    Coordination(String),
}

#[derive(Clone)]
pub(super) struct DownloadEventPublisher {
    observer_tx: broadcast::Sender<DownloadManagerEvent>,
    pub(super) feedback: Arc<OnceLock<Arc<crate::scheduler::feedback::SchedulerFeedback>>>,
    pub(super) coordination_tx: Option<DownloadCoordinationSender>,
}

impl DownloadEventPublisher {
    pub(super) fn new(
        observer_tx: broadcast::Sender<DownloadManagerEvent>,
        coordination_tx: Option<DownloadCoordinationSender>,
    ) -> Self {
        Self {
            feedback: Arc::new(OnceLock::new()),
            observer_tx,
            coordination_tx,
        }
    }

    /// Broadcast an event that no required consumer has to apply.
    ///
    /// Coordination-requiring events go through [`Self::publish_and_wait`] so
    /// their acknowledgement is observed; routing one here would enqueue it and
    /// then discard whether the consumer applied it, which is why this asserts
    /// on the distinction rather than silently branching on it.
    pub(super) fn publish(&self, event: DownloadManagerEvent) -> bool {
        debug_assert!(
            !event.requires_coordination(),
            "coordination-requiring events must use publish_and_wait"
        );
        self.observe(event)
    }

    pub(super) async fn publish_and_wait(
        &self,
        event: DownloadManagerEvent,
    ) -> std::result::Result<(), PublicationError> {
        let permit = if let Some(feedback) = self.feedback.get()
            && !feedback.monitoring_stopped()
            && matches!(event, DownloadManagerEvent::Terminal(_))
        {
            Some(feedback.reserve(event.streamer_id()).map_err(|_| {
                feedback.request_recheck(event.streamer_id());
                PublicationError::FeedbackBusy
            })?)
        } else {
            None
        };
        self.coordinate_and_wait(&event)
            .await
            .map_err(PublicationError::Coordination)?;
        self.observe_reserved(event, permit);
        Ok(())
    }

    pub(super) async fn coordinate_and_wait(
        &self,
        event: &DownloadManagerEvent,
    ) -> std::result::Result<(), String> {
        if !event.requires_coordination() {
            return Ok(());
        }
        match &self.coordination_tx {
            Some(sender) => sender.publish(event.clone()).wait().await,
            None => Ok(()),
        }
    }

    pub(super) fn reserve_feedback(
        &self,
        id: &str,
    ) -> Result<
        (
            Option<crate::scheduler::feedback::FeedbackPermit>,
            Option<crate::scheduler::feedback::FeedbackPermit>,
        ),
        String,
    > {
        match self.feedback.get() {
            Some(feedback) if !feedback.monitoring_stopped() => feedback
                .reserve_attempt(id)
                .map(|(started, ended)| (Some(started), Some(ended))),
            _ => Ok((None, None)),
        }
    }

    pub(super) fn observe_reserved(
        &self,
        event: DownloadManagerEvent,
        permit: Option<crate::scheduler::feedback::FeedbackPermit>,
    ) -> bool {
        if let (Some(feedback), Some(permit)) = (self.feedback.get(), permit) {
            // Application stays owned by the dispatcher even if this optional
            // observation receipt is dropped by the producer.
            drop(feedback.publish_reserved(&event, permit));
        }
        self.observe(event)
    }

    pub(super) fn observe(&self, event: DownloadManagerEvent) -> bool {
        self.observer_tx.send(event).is_ok()
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<DownloadManagerEvent> {
        self.observer_tx.subscribe()
    }

    pub(super) async fn shutdown_coordination(&self) -> std::result::Result<(), &'static str> {
        match &self.coordination_tx {
            Some(sender) => sender.shutdown().await,
            None => Ok(()),
        }
    }
}

enum DownloadCoordinationDelivery {
    Event {
        event: Box<DownloadManagerEvent>,
        acknowledgement: oneshot::Sender<std::result::Result<(), String>>,
    },
    Shutdown(oneshot::Sender<()>),
}

#[derive(Clone)]
pub(crate) struct DownloadCoordinationSender {
    state: Arc<Mutex<DownloadCoordinationState>>,
}

struct DownloadCoordinationState {
    tx: mpsc::UnboundedSender<DownloadCoordinationDelivery>,
    accepting: bool,
}

impl DownloadCoordinationSender {
    pub(super) fn publish(&self, event: DownloadManagerEvent) -> DownloadCoordinationReceipt {
        let (acknowledgement, acknowledged) = oneshot::channel();
        let delivery = DownloadCoordinationDelivery::Event {
            event: Box::new(event),
            acknowledgement,
        };
        let mut state = self.state.lock();
        if !state.accepting || state.tx.send(delivery).is_err() {
            state.accepting = false;
            DownloadCoordinationReceipt::Unavailable
        } else {
            DownloadCoordinationReceipt::Pending(acknowledged)
        }
    }

    pub(super) async fn shutdown(&self) -> std::result::Result<(), &'static str> {
        let (acknowledgement, acknowledged) = oneshot::channel();
        {
            let mut state = self.state.lock();
            if !state.accepting {
                return Err("required download coordination channel is already closed");
            }
            state.accepting = false;
            state
                .tx
                .send(DownloadCoordinationDelivery::Shutdown(acknowledgement))
                .map_err(|_| "required download coordination channel is closed")?;
        }
        acknowledged
            .await
            .map_err(|_| "download coordination handler stopped before acknowledging shutdown")
    }

    #[cfg(test)]
    pub(crate) async fn publish_and_wait_for_test(
        &self,
        event: DownloadManagerEvent,
    ) -> std::result::Result<(), String> {
        self.publish(event).wait().await
    }

    #[cfg(test)]
    pub(crate) async fn shutdown_for_test(&self) -> std::result::Result<(), &'static str> {
        self.shutdown().await
    }
}

pub(super) enum DownloadCoordinationReceipt {
    Pending(oneshot::Receiver<std::result::Result<(), String>>),
    Unavailable,
}

impl DownloadCoordinationReceipt {
    pub(super) async fn wait(self) -> std::result::Result<(), String> {
        match self {
            Self::Pending(acknowledged) => acknowledged.await.map_err(|_| {
                "download coordination handler stopped before acknowledging event".to_string()
            })?,
            Self::Unavailable => {
                Err("required download coordination channel is closed".to_string())
            }
        }
    }
}

pub(crate) struct DownloadCoordinationEvent {
    event: DownloadManagerEvent,
    acknowledgement: oneshot::Sender<std::result::Result<(), String>>,
}

impl DownloadCoordinationEvent {
    pub(crate) fn into_parts(
        self,
    ) -> (
        DownloadManagerEvent,
        oneshot::Sender<std::result::Result<(), String>>,
    ) {
        (self.event, self.acknowledgement)
    }
}

pub(crate) struct DownloadCoordinationReceiver {
    rx: mpsc::UnboundedReceiver<DownloadCoordinationDelivery>,
}

impl DownloadCoordinationReceiver {
    pub(crate) async fn recv(
        &mut self,
    ) -> std::result::Result<Option<DownloadCoordinationEvent>, &'static str> {
        match self.rx.recv().await {
            Some(DownloadCoordinationDelivery::Event {
                event,
                acknowledgement,
            }) => Ok(Some(DownloadCoordinationEvent {
                event: *event,
                acknowledgement,
            })),
            Some(DownloadCoordinationDelivery::Shutdown(acknowledgement)) => {
                if acknowledgement.send(()).is_err() {
                    debug!("Download coordination shutdown acknowledgement receiver was dropped");
                }
                Ok(None)
            }
            None => Err("required download coordination channel closed without shutdown"),
        }
    }
}

pub(crate) fn download_coordination_channel()
-> (DownloadCoordinationSender, DownloadCoordinationReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        DownloadCoordinationSender {
            state: Arc::new(Mutex::new(DownloadCoordinationState {
                tx,
                accepting: true,
            })),
        },
        DownloadCoordinationReceiver { rx },
    )
}
