use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use super::{
    NotificationService, WEB_PUSH_BATCH_SIZE, WEB_PUSH_FLUSH_INTERVAL_MS, WEB_PUSH_QUEUE_CAPACITY,
    WebPushQueuedEvent,
};

impl NotificationService {
    /// Start a background worker to process web push delivery from a bounded queue.
    ///
    /// This avoids spawning a new task per notification event and reduces DB load by
    /// letting `WebPushService` reuse its internal subscription cache.
    pub fn start_web_push_worker(self: &Arc<Self>) {
        let Some(web_push) = self.web_push_service.clone() else {
            return;
        };
        if self
            .web_push_worker_started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let (tx, rx) = mpsc::channel::<WebPushQueuedEvent>(WEB_PUSH_QUEUE_CAPACITY);
        *self.web_push_tx.write() = Some(tx);

        let cancellation_token = self.cancellation_token.clone();

        self.task_supervisor.spawn(
            "web push worker",
            run_web_push_worker(rx, cancellation_token, move |item| {
                let web_push = web_push.clone();
                async move {
                    web_push
                        .send_event(&item.event, item.event_log_id.as_deref())
                        .await;
                }
            }),
        );
    }

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

async fn flush_buffer<F, Fut>(send: &mut F, buffer: &mut Vec<WebPushQueuedEvent>)
where
    F: FnMut(WebPushQueuedEvent) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut batch = std::mem::take(buffer);
    for item in batch.drain(..) {
        send(item).await;
    }
    *buffer = batch;
}

async fn run_web_push_worker<F, Fut>(
    mut rx: mpsc::Receiver<WebPushQueuedEvent>,
    cancellation_token: CancellationToken,
    mut send: F,
) where
    F: FnMut(WebPushQueuedEvent) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut buffer: Vec<WebPushQueuedEvent> = Vec::with_capacity(WEB_PUSH_BATCH_SIZE);
    let mut ticker = tokio::time::interval(Duration::from_millis(WEB_PUSH_FLUSH_INTERVAL_MS));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = cancellation_token.cancelled() => {
                debug!("Web push worker shutting down");
                while let Ok(item) = rx.try_recv() {
                    buffer.push(item);
                }
                if !buffer.is_empty() {
                    flush_buffer(&mut send, &mut buffer).await;
                }
                break;
            }
            _ = ticker.tick() => {
                if buffer.is_empty() {
                    continue;
                }
                flush_buffer(&mut send, &mut buffer).await;
            }
            maybe = rx.recv() => {
                let Some(item) = maybe else {
                    while let Ok(item) = rx.try_recv() {
                        buffer.push(item);
                    }
                    if !buffer.is_empty() {
                        flush_buffer(&mut send, &mut buffer).await;
                    }
                    break;
                };
                buffer.push(item);
                if buffer.len() < WEB_PUSH_BATCH_SIZE {
                    continue;
                }
                flush_buffer(&mut send, &mut buffer).await;
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

    #[tokio::test]
    async fn worker_drains_all_admitted_items_in_order_on_close_and_cancellation() {
        for cancel in [false, true] {
            let (tx, rx) = mpsc::channel(128);
            for index in 0..70 {
                tx.send(event(index)).await.unwrap();
            }
            let token = CancellationToken::new();
            if cancel {
                token.cancel();
            } else {
                drop(tx);
            }
            let delivered = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let output = delivered.clone();
            tokio::time::timeout(
                Duration::from_secs(1),
                run_web_push_worker(rx, token, move |item| {
                    output.lock().push(item.event_log_id.unwrap());
                    async {}
                }),
            )
            .await
            .unwrap();
            assert_eq!(
                *delivered.lock(),
                (0..70).map(|index| index.to_string()).collect::<Vec<_>>()
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn worker_flushes_partial_batches_on_its_interval() {
        let (tx, rx) = mpsc::channel(128);
        let token = CancellationToken::new();
        let (sent, mut received) = mpsc::unbounded_channel();
        let worker = tokio::spawn(run_web_push_worker(rx, token.clone(), move |item| {
            sent.send(item.event_log_id).unwrap();
            async {}
        }));
        // Let the worker consume the interval's immediate first tick while empty.
        tokio::task::yield_now().await;
        tx.send(event(1)).await.unwrap();
        tokio::task::yield_now().await;
        assert!(received.try_recv().is_err());
        tokio::time::advance(Duration::from_millis(WEB_PUSH_FLUSH_INTERVAL_MS)).await;
        assert_eq!(received.recv().await.unwrap(), Some("1".into()));
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }
}
