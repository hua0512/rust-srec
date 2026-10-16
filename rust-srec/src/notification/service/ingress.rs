use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use futures::{FutureExt, StreamExt, stream::FuturesUnordered};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;

use super::{MAX_NOTIFICATION_PASSES, NotificationEvent, NotificationService};

impl NotificationService {
    /// Synchronous failure reporters cannot backpressure their callers. Reject the
    /// newest event when full, before persistence, without a fallback task.
    pub(crate) fn dispatch_notification(self: &Arc<Self>, event: NotificationEvent) {
        if !self.config.enabled {
            return;
        }
        match self.dispatch_tx.try_reserve() {
            Ok(permit) => self.admit_dispatch(permit, event),
            Err(TrySendError::Full(_)) => self.reject_dispatch(event.event_type(), "full"),
            Err(TrySendError::Closed(_)) => self.reject_dispatch(event.event_type(), "closed"),
        }
    }

    /// Listeners backpressure at admission. Their bounded source broadcasts report
    /// lag when producers outrun this queue; no extra dispatch tasks are spawned.
    pub(super) async fn dispatch_notification_wait(self: &Arc<Self>, event: NotificationEvent) {
        if !self.config.enabled {
            return;
        }
        tokio::select! {
            biased;
            _ = self.cancellation_token.cancelled() => self.reject_dispatch(event.event_type(), "shutdown"),
            permit = self.dispatch_tx.reserve() => match permit {
                Ok(permit) => self.admit_dispatch(permit, event),
                Err(_) => self.reject_dispatch(event.event_type(), "closed"),
            },
        }
    }

    fn admit_dispatch(
        self: &Arc<Self>,
        permit: mpsc::Permit<'_, NotificationEvent>,
        event: NotificationEvent,
    ) {
        // Enqueue and worker handoff share a lock with the worker's idle exit and
        // stop. A racing enqueue either joins that worker or starts its successor.
        let mut receiver = self.dispatch_rx.lock();
        if self.cancellation_token.is_cancelled() {
            self.reject_dispatch(event.event_type(), "shutdown");
            return;
        }
        let event_type = event.event_type();
        permit.send(event);
        if let Some(rx) = receiver.take() {
            let service = self.clone();
            // A rejected worker drops the receiver and the event just queued;
            // later senders then see a closed queue and are counted there.
            if !self
                .task_supervisor
                .spawn("notification dispatch", async move {
                    service.run_dispatch(rx).await;
                })
            {
                self.reject_dispatch(event_type, "shutdown");
            }
        }
    }

    fn reject_dispatch(&self, event_type: &'static str, reason: &str) {
        let dropped = self.dispatch_dropped.fetch_add(1, Ordering::Relaxed) + 1;
        if dropped.is_power_of_two() {
            warn!(
                reason,
                event_type, dropped, "Notification rejected before event-log persistence"
            );
        }
    }

    async fn run_dispatch(self: Arc<Self>, mut rx: mpsc::Receiver<NotificationEvent>) {
        let mut active = FuturesUnordered::new();
        loop {
            while active.len() < MAX_NOTIFICATION_PASSES {
                let Ok(event) = rx.try_recv() else { break };
                active.push(self.notify_dispatched(event));
            }
            if active.is_empty() {
                let mut receiver = self.dispatch_rx.lock();
                if rx.is_empty() {
                    *receiver = Some(rx);
                    return;
                }
                continue;
            }
            tokio::select! {
                Some(event) = rx.recv(), if active.len() < MAX_NOTIFICATION_PASSES => {
                    active.push(self.notify_dispatched(event));
                },
                _ = active.next() => {},
            }
        }
    }

    async fn notify_dispatched(&self, event: NotificationEvent) {
        // Accepted events retain logging while stop drains ingress. Web push
        // keeps its own shutdown admission policy. Direct callers share this limit.
        let Ok(_permit) = self.dispatch_permits.acquire().await else {
            return;
        };
        match AssertUnwindSafe(self.notify_admitted(event))
            .catch_unwind()
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "Failed to dispatch notification"),
            Err(_) => warn!("Notification dispatch panicked; continuing queued events"),
        }
    }
}
