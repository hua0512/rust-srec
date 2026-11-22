//! Transactional monitor event outbox publisher and batch delivery flushing.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::StreamMonitor;
use crate::Result;
use crate::database::repositories::{
    ConfigRepository, MonitorOutboxOps, SessionRepository, StreamerRepository,
};
use crate::monitor::events::{MonitorEvent, MonitorEventBroadcaster, MonitorEventDelivery};
use crate::utils::task_supervisor::TaskSupervisor;

impl<
    SR: StreamerRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
    CR: ConfigRepository + Send + Sync + 'static,
> StreamMonitor<SR, SSR, CR>
{
    pub(super) fn spawn_outbox_publisher(
        &self,
        outbox_notify: Arc<Notify>,
        cancellation_token: CancellationToken,
        task_supervisor: Arc<TaskSupervisor>,
    ) {
        let pool = self.write_pool.clone();
        let broadcaster = self.event_broadcaster.clone();
        let required_event_sender = self.required_event_sender.clone();

        task_supervisor.spawn("monitor outbox publisher", async move {
            loop {
                tokio::select! {
                    biased;

                    // Check for cancellation first
                    _ = cancellation_token.cancelled() => {
                        info!("Outbox publisher shutting down");
                        break;
                    }
                    _ = outbox_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }

                // Check cancellation again before processing
                if cancellation_token.is_cancelled() {
                    break;
                }

                if let Err(e) = flush_outbox_until_wait(
                    &pool,
                    &broadcaster,
                    required_event_sender.as_ref(),
                    &cancellation_token,
                )
                .await
                {
                    warn!("Monitor outbox flush failed: {}", e);
                }
            }
            debug!("Outbox publisher stopped");
        });
    }
}

const OUTBOX_NO_RECEIVER_MAX_ATTEMPTS: i64 = 5;
const OUTBOX_NO_RECEIVER_MAX_AGE_SECS: i64 = 30;

pub(super) struct FlushOutboxResult {
    pub(super) fetched: usize,
    pub(super) needs_wait: bool,
}

pub(super) async fn flush_outbox_until_wait(
    pool: &SqlitePool,
    broadcaster: &MonitorEventBroadcaster,
    required_event_sender: Option<&mpsc::Sender<MonitorEventDelivery>>,
    cancellation_token: &CancellationToken,
) -> Result<()> {
    loop {
        if cancellation_token.is_cancelled() {
            return Ok(());
        }

        let result =
            flush_outbox_once(pool, broadcaster, required_event_sender, cancellation_token).await?;
        if result.fetched == 0 || result.needs_wait {
            return Ok(());
        }
    }
}
pub(super) async fn flush_outbox_once(
    pool: &SqlitePool,
    broadcaster: &MonitorEventBroadcaster,
    required_event_sender: Option<&mpsc::Sender<MonitorEventDelivery>>,
    cancellation_token: &CancellationToken,
) -> Result<FlushOutboxResult> {
    let entries = MonitorOutboxOps::fetch_undelivered(pool, 100).await?;

    if entries.is_empty() {
        return Ok(FlushOutboxResult {
            fetched: 0,
            needs_wait: false,
        });
    }

    let fetched = entries.len();
    let mut needs_wait = false;
    let mut delivered_ids = Vec::with_capacity(fetched);
    let mut failed_entries: Vec<(i64, String)> = Vec::new();

    for entry in entries {
        match serde_json::from_str::<MonitorEvent>(&entry.payload) {
            Ok(event) => {
                if let Some(sender) = required_event_sender {
                    let (delivery, acknowledgement) = MonitorEventDelivery::new(event.clone());
                    let send_result = tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        result = sender.send(delivery) => result,
                    };

                    if send_result.is_err() {
                        warn!(
                            outbox_id = entry.id,
                            "Required monitor event consumer is unavailable; retaining outbox event"
                        );
                        failed_entries.push((
                            entry.id,
                            "required monitor event consumer unavailable".to_string(),
                        ));
                        needs_wait = true;
                        break;
                    }

                    let acknowledgement_result = tokio::select! {
                        _ = cancellation_token.cancelled() => break,
                        result = acknowledgement => result,
                    };
                    if acknowledgement_result.is_err() {
                        warn!(
                            outbox_id = entry.id,
                            "Required monitor event consumer dropped acknowledgement; retaining outbox event"
                        );
                        failed_entries.push((
                            entry.id,
                            "required monitor event acknowledgement dropped".to_string(),
                        ));
                        needs_wait = true;
                        break;
                    }

                    match broadcaster.publish(event) {
                        Ok(receiver_count) => debug!(
                            outbox_id = entry.id,
                            receiver_count,
                            "Delivered required monitor event and notified observers"
                        ),
                        Err(_) => debug!(
                            outbox_id = entry.id,
                            "Delivered required monitor event with no observational subscribers"
                        ),
                    }
                    delivered_ids.push(entry.id);
                    continue;
                }

                // Attempt to broadcast the event
                match broadcaster.publish(event) {
                    Ok(receiver_count) => {
                        // Event was successfully sent to `receiver_count` receivers
                        debug!("Published event to {} receivers", receiver_count);
                        delivered_ids.push(entry.id);
                    }
                    Err(e) => {
                        // No receivers available - keep events briefly to handle listener startup.
                        let now = jiff::Timestamp::now();
                        let age_secs = now
                            .duration_since(crate::database::time::ms_to_datetime(entry.created_at))
                            .as_secs();

                        let should_drop = entry.attempts >= OUTBOX_NO_RECEIVER_MAX_ATTEMPTS
                            || age_secs >= OUTBOX_NO_RECEIVER_MAX_AGE_SECS;

                        if should_drop {
                            warn!(
                                "Monitor outbox event id={} has no receivers after {} attempts or {}s, discarding: {:?}",
                                entry.id, entry.attempts, age_secs, e.0
                            );
                            delivered_ids.push(entry.id);
                        } else {
                            debug!(
                                "Monitor outbox event id={} has no receivers, retrying later",
                                entry.id
                            );
                            failed_entries.push((entry.id, "no receivers".to_string()));
                            needs_wait = true;
                        }
                    }
                }
            }
            Err(e) => {
                // A JSON parse failure is not recoverable, and will otherwise permanently
                // poison the outbox head-of-line (ORDER BY id). Discard it.
                warn!(
                    "Invalid monitor outbox payload id={}, discarding: {}",
                    entry.id, e
                );
                delivered_ids.push(entry.id);
            }
        }
    }

    MonitorOutboxOps::mark_delivered_batch(pool, &delivered_ids).await?;
    MonitorOutboxOps::record_failure_batch(pool, &failed_entries).await?;

    Ok(FlushOutboxResult {
        fetched,
        needs_wait,
    })
}
