use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, warn};

use super::NotificationService;
use crate::notification::events::NotificationEvent;

impl NotificationService {
    /// Spawn a supervised broadcast listener that maps upstream events into
    /// [`NotificationEvent`]s and dispatches them with backpressure.
    pub fn spawn_event_listener<E, F>(
        self: &Arc<Self>,
        task_name: &'static str,
        mut rx: broadcast::Receiver<E>,
        mut map: F,
    ) where
        E: Clone + Send + 'static,
        F: FnMut(E, jiff::Timestamp) -> Option<NotificationEvent> + Send + 'static,
    {
        let service = Arc::clone(self);
        let config = service.config.clone();
        let cancellation_token = service.cancellation_token.clone();
        let task_supervisor = service.task_supervisor.clone();

        task_supervisor.spawn(task_name, async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!(listener = task_name, "Notification event listener shutting down");
                        break;
                    }
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                if !config.enabled {
                                    continue;
                                }

                                if let Some(notification) = map(event, jiff::Timestamp::now()) {
                                    service.dispatch_notification_wait(notification).await;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                warn!(listener = task_name, lagged = n, "Notification event listener lagged");
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                debug!(listener = task_name, "Notification event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }
}
