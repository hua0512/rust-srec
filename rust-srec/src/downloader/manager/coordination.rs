//! Acknowledged event delivery and observer broadcasts.

use std::sync::{Arc, OnceLock};

use tokio::sync::{OwnedSemaphorePermit, broadcast, oneshot};

use super::DownloadManagerEvent;
use crate::utils::required_channel::{
    Publish, RequiredChannelError, RequiredReceiver, RequiredSender, required_channel,
};
use crate::utils::shared_event::{SharedEvent, SharedEventSender, publish_shared};

/// Bounded lifecycle delivery permit reserved before an attempt starts in the
/// recorder actor runtime.
#[derive(Debug)]
pub(crate) struct RecorderFeedbackPermit {
    streamer_id: String,
    _global: OwnedSemaphorePermit,
    _streamer: OwnedSemaphorePermit,
}

impl RecorderFeedbackPermit {
    pub(crate) fn new(
        streamer_id: String,
        global: OwnedSemaphorePermit,
        streamer: OwnedSemaphorePermit,
    ) -> Self {
        Self {
            streamer_id,
            _global: global,
            _streamer: streamer,
        }
    }

    pub(crate) fn streamer_id(&self) -> &str {
        &self.streamer_id
    }
}

/// Reserved-capacity lifecycle sink used by the recorder actor runtime to
/// receive `DownloadStarted` and post-`release_active()` terminal events
/// without blocking attempt startup or dropping terminal signals.
pub(crate) trait RecorderDownloadFeedback: Send + Sync {
    fn ensure_attempt_capacity(&self, attempts: usize);
    fn monitoring_stopped(&self) -> bool;
    fn reserve(&self, streamer_id: &str) -> std::result::Result<RecorderFeedbackPermit, String>;
    fn reserve_attempt(
        &self,
        streamer_id: &str,
    ) -> std::result::Result<(RecorderFeedbackPermit, RecorderFeedbackPermit), String>;
    fn publish_reserved(&self, event: &DownloadManagerEvent, permit: RecorderFeedbackPermit);
    fn request_recheck(&self, streamer_id: &str);
    fn stop_monitoring(&self);
}

#[derive(Clone)]
pub(super) enum LifecycleFeedback {
    Legacy(Arc<crate::scheduler::feedback::SchedulerFeedback>),
    Recorder(Arc<dyn RecorderDownloadFeedback>),
}

pub(super) enum ReservedPermit {
    Legacy(crate::scheduler::feedback::FeedbackPermit),
    Recorder(RecorderFeedbackPermit),
}

impl LifecycleFeedback {
    pub(super) fn ensure_attempt_capacity(&self, attempts: usize) {
        match self {
            Self::Legacy(feedback) => feedback.ensure_attempt_capacity(attempts),
            Self::Recorder(feedback) => feedback.ensure_attempt_capacity(attempts),
        }
    }

    pub(super) fn monitoring_stopped(&self) -> bool {
        match self {
            Self::Legacy(feedback) => feedback.monitoring_stopped(),
            Self::Recorder(feedback) => feedback.monitoring_stopped(),
        }
    }

    pub(super) fn reserve(&self, streamer_id: &str) -> std::result::Result<ReservedPermit, String> {
        match self {
            Self::Legacy(feedback) => feedback.reserve(streamer_id).map(ReservedPermit::Legacy),
            Self::Recorder(feedback) => feedback.reserve(streamer_id).map(ReservedPermit::Recorder),
        }
    }

    pub(super) fn reserve_attempt(
        &self,
        streamer_id: &str,
    ) -> std::result::Result<(ReservedPermit, ReservedPermit), String> {
        match self {
            Self::Legacy(feedback) => feedback
                .reserve_attempt(streamer_id)
                .map(|(a, b)| (ReservedPermit::Legacy(a), ReservedPermit::Legacy(b))),
            Self::Recorder(feedback) => feedback
                .reserve_attempt(streamer_id)
                .map(|(a, b)| (ReservedPermit::Recorder(a), ReservedPermit::Recorder(b))),
        }
    }

    pub(super) fn publish_reserved(&self, event: &DownloadManagerEvent, permit: ReservedPermit) {
        match (self, permit) {
            (Self::Legacy(feedback), ReservedPermit::Legacy(permit)) => {
                drop(feedback.publish_reserved(event, permit));
            }
            (Self::Recorder(feedback), ReservedPermit::Recorder(permit)) => {
                feedback.publish_reserved(event, permit);
            }
            _ => {
                tracing::error!("Mismatched lifecycle feedback permit kind");
            }
        }
    }

    pub(super) fn request_recheck(&self, streamer_id: &str) {
        match self {
            Self::Legacy(feedback) => feedback.request_recheck(streamer_id),
            Self::Recorder(feedback) => feedback.request_recheck(streamer_id),
        }
    }

    pub(super) async fn shutdown(&self) {
        match self {
            Self::Legacy(feedback) => feedback.shutdown().await,
            Self::Recorder(feedback) => feedback.stop_monitoring(),
        }
    }

    pub(super) async fn abort(&self, deadline: tokio::time::Instant) {
        match self {
            Self::Legacy(feedback) => feedback.abort(deadline).await,
            Self::Recorder(feedback) => feedback.stop_monitoring(),
        }
    }
}

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
    shared_tx: Arc<SharedEventSender<DownloadManagerEvent>>,
    pub(super) feedback: Arc<OnceLock<LifecycleFeedback>>,
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
            shared_tx: Arc::new(SharedEventSender::new(256)),
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
    ) -> Result<(Option<ReservedPermit>, Option<ReservedPermit>), String> {
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
        permit: Option<ReservedPermit>,
    ) -> bool {
        if let (Some(feedback), Some(permit)) = (self.feedback.get(), permit) {
            // Application stays owned by the dispatcher even if this optional
            // observation receipt is dropped by the producer.
            feedback.publish_reserved(&event, permit);
        }
        self.observe(event)
    }

    pub(super) fn observe(&self, event: DownloadManagerEvent) -> bool {
        publish_shared(&self.observer_tx, &self.shared_tx, event)
    }

    pub(super) fn subscribe(&self) -> broadcast::Receiver<DownloadManagerEvent> {
        self.observer_tx.subscribe()
    }

    pub(super) fn subscribe_shared(
        &self,
    ) -> broadcast::Receiver<Arc<SharedEvent<DownloadManagerEvent>>> {
        self.shared_tx.subscribe()
    }

    pub(super) async fn shutdown_coordination(
        &self,
    ) -> std::result::Result<(), RequiredChannelError> {
        match &self.coordination_tx {
            Some(sender) => sender.shutdown().await,
            None => Ok(()),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DownloadCoordinationSender(RequiredSender<DownloadCoordinationEvent, ()>);

impl DownloadCoordinationSender {
    pub(super) fn publish(&self, event: DownloadManagerEvent) -> DownloadCoordinationReceipt {
        let (acknowledgement, acknowledged) = oneshot::channel();
        let delivery = DownloadCoordinationEvent {
            event: Box::new(event),
            acknowledgement,
        };
        match self.0.publish(delivery) {
            Publish::Accepted => DownloadCoordinationReceipt::Pending(acknowledged),
            Publish::ClosedByShutdown | Publish::ConsumerGone => {
                DownloadCoordinationReceipt::Unavailable
            }
        }
    }

    pub(super) async fn shutdown(&self) -> std::result::Result<(), RequiredChannelError> {
        self.0.shutdown().await
    }

    #[cfg(test)]
    pub(crate) async fn publish_and_wait_for_test(
        &self,
        event: DownloadManagerEvent,
    ) -> std::result::Result<(), String> {
        self.publish(event).wait().await
    }

    #[cfg(test)]
    pub(crate) async fn shutdown_for_test(&self) -> std::result::Result<(), RequiredChannelError> {
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
                "download coordination consumer stopped before acknowledging event".to_string()
            })?,
            Self::Unavailable => {
                Err("required download coordination channel is closed".to_string())
            }
        }
    }
}

pub(crate) struct DownloadCoordinationEvent {
    event: Box<DownloadManagerEvent>,
    acknowledgement: oneshot::Sender<std::result::Result<(), String>>,
}

impl DownloadCoordinationEvent {
    pub(crate) fn into_parts(
        self,
    ) -> (
        DownloadManagerEvent,
        oneshot::Sender<std::result::Result<(), String>>,
    ) {
        (*self.event, self.acknowledgement)
    }
}

pub(crate) type DownloadCoordinationReceiver = RequiredReceiver<DownloadCoordinationEvent, ()>;

pub(crate) fn download_coordination_channel()
-> (DownloadCoordinationSender, DownloadCoordinationReceiver) {
    let (sender, receiver) = required_channel("download coordination");
    (DownloadCoordinationSender(sender), receiver)
}
