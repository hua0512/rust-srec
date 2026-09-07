use std::sync::atomic::Ordering;

use tokio::sync::mpsc::error::TrySendError;
use tracing::warn;

use super::{NotificationService, WebPushQueuedEvent};

impl NotificationService {
    /// FIFO admission drops the newest event at every priority when full. Event-log
    /// persistence and ordinary channel delivery are independent; push is best effort
    /// and rejected events are not retried. Holding the sender lock also fences stop.
    pub(super) fn enqueue_web_push(&self, item: WebPushQueuedEvent) {
        let event_type = item.event.event_type();
        let priority = item.event.priority().as_int();
        let sender = self.web_push_tx.read();
        let reason = if self.cancellation_token.is_cancelled() {
            Some("shutdown")
        } else if let Some(sender) = sender.as_ref() {
            match sender.try_send(item) {
                Ok(()) => None,
                Err(TrySendError::Full(_)) => Some("full"),
                Err(TrySendError::Closed(_)) => Some("closed"),
            }
        } else {
            Some("worker_unavailable")
        };
        if let Some(reason) = reason {
            let dropped = self.web_push_dropped.fetch_add(1, Ordering::Relaxed) + 1;
            // Rate-limit sustained overload diagnostics without losing the count.
            if dropped.is_power_of_two() {
                warn!(
                    reason,
                    event_type, priority, dropped, "Web push event dropped at queue admission"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tokio::sync::mpsc;

    use super::*;
    use crate::notification::NotificationEvent;

    fn event(index: usize) -> WebPushQueuedEvent {
        WebPushQueuedEvent {
            event: NotificationEvent::SystemStartup {
                version: index.to_string(),
                timestamp: Utc::now(),
            },
            event_log_id: Some(index.to_string()),
        }
    }

    #[test]
    fn stalled_consumer_preserves_bounded_fifo_and_counts_overflow() {
        let service = NotificationService::new();
        let (tx, mut rx) = mpsc::channel(super::super::WEB_PUSH_QUEUE_CAPACITY);
        *service.web_push_tx.write() = Some(tx);
        for index in 0..10_000 {
            service.enqueue_web_push(event(index));
        }
        service.enqueue_web_push(WebPushQueuedEvent {
            event: NotificationEvent::PipelineQueueCritical {
                queue_depth: 1_000,
                threshold: 200,
                timestamp: Utc::now(),
            },
            event_log_id: None,
        });
        assert_eq!(rx.len(), super::super::WEB_PUSH_QUEUE_CAPACITY);
        assert_eq!(service.stats().web_push_dropped, 10_001 - rx.len() as u64);
        for index in 0..super::super::WEB_PUSH_QUEUE_CAPACITY {
            assert_eq!(rx.try_recv().unwrap().event_log_id, Some(index.to_string()));
        }
        service.enqueue_web_push(event(10_000));
        assert_eq!(
            rx.try_recv().unwrap().event_log_id.as_deref(),
            Some("10000")
        );
    }

    #[test]
    fn unavailable_closed_and_stopped_queues_never_spawn_fallbacks() {
        // No Tokio runtime: these paths must not spawn even when push cannot queue.
        let service = NotificationService::new();
        service.enqueue_web_push(event(0));
        let (tx, rx) = mpsc::channel(1);
        *service.web_push_tx.write() = Some(tx);
        drop(rx);
        service.enqueue_web_push(event(1));
        let (tx, mut rx) = mpsc::channel(1);
        *service.web_push_tx.write() = Some(tx);
        service.cancellation_token.cancel();
        service.enqueue_web_push(event(2));
        assert!(rx.try_recv().is_err());
        assert_eq!(service.stats().web_push_dropped, 3);
    }

    #[tokio::test]
    async fn shutdown_closes_admission_and_preserves_already_queued_events() {
        let service = NotificationService::new();
        let (tx, mut rx) = mpsc::channel(2);
        *service.web_push_tx.write() = Some(tx);
        service.enqueue_web_push(event(0));
        service.enqueue_web_push(event(1));
        tokio::time::timeout(std::time::Duration::from_secs(1), service.stop())
            .await
            .unwrap();
        service.enqueue_web_push(event(2));
        assert_eq!(service.stats().web_push_dropped, 1);
        assert_eq!(rx.recv().await.unwrap().event_log_id.as_deref(), Some("0"));
        assert_eq!(rx.recv().await.unwrap().event_log_id.as_deref(), Some("1"));
        assert!(rx.recv().await.is_none());
    }
}
