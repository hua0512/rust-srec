//! Lossless, ordered event channel with exactly one required consumer and a
//! shutdown barrier that drains everything published before it.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tracing::debug;

#[derive(Debug, thiserror::Error)]
pub(crate) enum RequiredChannelError {
    #[error("required {0} channel is already closed")]
    AlreadyClosed(&'static str),
    #[error("required {0} channel is closed")]
    Closed(&'static str),
    #[error("{0} consumer stopped before acknowledging shutdown")]
    ShutdownUnacknowledged(&'static str),
    #[error("required {0} channel closed without shutdown")]
    ClosedWithoutShutdown(&'static str),
}

/// What [`RequiredReceiver::recv`] yields. The shutdown marker carries the
/// acknowledgement so the consumer replies only once it has finished its own
/// work; every event published before the marker has already been yielded.
pub(crate) enum Delivery<E, A> {
    Event(E),
    Shutdown(oneshot::Sender<A>),
}

/// Why a publish did or did not reach the consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Publish {
    Accepted,
    /// Admission was closed by [`RequiredSender::shutdown`].
    ClosedByShutdown,
    /// The receiver was dropped without crossing the shutdown marker.
    ConsumerGone,
}

pub(crate) struct RequiredSender<E, A> {
    name: &'static str,
    state: Arc<Mutex<SenderState<E, A>>>,
}

impl<E, A> Clone for RequiredSender<E, A> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            state: Arc::clone(&self.state),
        }
    }
}

struct SenderState<E, A> {
    tx: mpsc::UnboundedSender<Delivery<E, A>>,
    accepting: bool,
}

impl<E, A> RequiredSender<E, A> {
    pub(crate) fn publish(&self, event: E) -> Publish {
        let mut state = self.state.lock();
        if !state.accepting {
            return Publish::ClosedByShutdown;
        }
        if state.tx.send(Delivery::Event(event)).is_err() {
            state.accepting = false;
            return Publish::ConsumerGone;
        }
        Publish::Accepted
    }

    /// Wait until every event published before this call has been handled,
    /// then stop the consumer, returning its acknowledgement payload.
    pub(crate) async fn shutdown(&self) -> Result<A, RequiredChannelError> {
        let (acknowledgement, acknowledged) = oneshot::channel();
        {
            // Closing admission and enqueueing the marker under one lock is
            // what guarantees no event is admitted behind the marker.
            let mut state = self.state.lock();
            if !state.accepting {
                return Err(RequiredChannelError::AlreadyClosed(self.name));
            }
            state.accepting = false;
            state
                .tx
                .send(Delivery::Shutdown(acknowledgement))
                .map_err(|_| RequiredChannelError::Closed(self.name))?;
        }
        acknowledged
            .await
            .map_err(|_| RequiredChannelError::ShutdownUnacknowledged(self.name))
    }
}

pub(crate) struct RequiredReceiver<E, A> {
    name: &'static str,
    rx: mpsc::UnboundedReceiver<Delivery<E, A>>,
}

impl<E, A> RequiredReceiver<E, A> {
    pub(crate) async fn recv(&mut self) -> Result<Delivery<E, A>, RequiredChannelError> {
        self.rx
            .recv()
            .await
            .ok_or(RequiredChannelError::ClosedWithoutShutdown(self.name))
    }
}

impl<E> RequiredReceiver<E, ()> {
    /// Receive the next event, acknowledging the shutdown marker at once and
    /// returning `None` for it.
    pub(crate) async fn recv_event(&mut self) -> Result<Option<E>, RequiredChannelError> {
        match self.recv().await? {
            Delivery::Event(event) => Ok(Some(event)),
            Delivery::Shutdown(acknowledgement) => {
                if acknowledgement.send(()).is_err() {
                    debug!(
                        channel = self.name,
                        "Required channel shutdown acknowledgement receiver was dropped"
                    );
                }
                Ok(None)
            }
        }
    }
}

/// `name` identifies the channel in errors and logs.
pub(crate) fn required_channel<E, A>(
    name: &'static str,
) -> (RequiredSender<E, A>, RequiredReceiver<E, A>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (
        RequiredSender {
            name,
            state: Arc::new(Mutex::new(SenderState {
                tx,
                accepting: true,
            })),
        },
        RequiredReceiver { name, rx },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_drains_prior_events_and_rejects_later_ones() {
        let (sender, mut receiver) = required_channel::<u32, ()>("test");
        let late_sender = sender.clone();
        assert_eq!(sender.publish(1), Publish::Accepted);
        assert_eq!(sender.publish(2), Publish::Accepted);

        let shutdown = tokio::spawn(async move { sender.shutdown().await });
        assert!(matches!(receiver.recv_event().await, Ok(Some(1))));
        assert!(matches!(receiver.recv_event().await, Ok(Some(2))));
        assert!(matches!(receiver.recv_event().await, Ok(None)));
        shutdown
            .await
            .expect("shutdown task should not panic")
            .expect("shutdown marker should be acknowledged");
        assert_eq!(
            late_sender.publish(3),
            Publish::ClosedByShutdown,
            "an event must not be admitted behind the shutdown marker"
        );
    }
}
