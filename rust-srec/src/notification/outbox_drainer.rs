//! Durable notification outbox drainer with two-level record decoding.
//!
//! Scans undelivered rows from `monitor_event_outbox` in ascending `id` order,
//! decodes both current [`OutboxEvent`] records and legacy [`MonitorEvent`]
//! records left behind across upgrades, settles alertable events through
//! [`NotificationService::deliver_durable_for_outbox`], and marks `delivered_at`
//! only after settlement succeeds.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::repositories::MonitorOutboxOps;
use crate::database::time::ms_to_datetime;
use crate::monitor::MonitorEvent;
use crate::notification::events::mapping::monitor_notification;
use crate::notification::{DurableDeliveryError, NotificationEvent, NotificationService};
use crate::recorder::OutboxEvent;

const DEFAULT_BATCH_SIZE: i32 = 100;
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Decoded payload from a `monitor_event_outbox` row.
#[derive(Debug, Clone)]
pub enum OutboxRecord {
    /// Current notification-only outbox record written by `RecorderStore`.
    V2(OutboxEvent),
    /// Legacy `MonitorEvent` record written before migration to `OutboxEvent`.
    Legacy(MonitorEvent),
}

impl OutboxRecord {
    /// Decode a raw JSON outbox payload, trying [`OutboxEvent`] before falling
    /// back to legacy [`MonitorEvent`].
    pub fn decode(payload: &str) -> std::result::Result<Self, serde_json::Error> {
        if let Ok(event) = serde_json::from_str::<OutboxEvent>(payload) {
            return Ok(Self::V2(event));
        }
        serde_json::from_str::<MonitorEvent>(payload).map(Self::Legacy)
    }

    /// Translate this outbox record into a user-facing [`NotificationEvent`],
    /// or `None` for non-notifying legacy records (`StreamerLive`,
    /// `StreamerOffline`, `StateChanged`, `TransientError`).
    pub fn into_notification_event(self) -> Option<NotificationEvent> {
        match self {
            Self::V2(event) => Some(outbox_event_to_notification(event)),
            Self::Legacy(event) => monitor_notification(event),
        }
    }
}

/// Convert a typed [`OutboxEvent`] into its corresponding [`NotificationEvent`],
/// preserving the transaction's exact millisecond timestamp.
pub fn outbox_event_to_notification(event: OutboxEvent) -> NotificationEvent {
    match event {
        OutboxEvent::StreamOnline {
            streamer_id,
            streamer_name,
            title,
            category,
            started_at_ms,
            ..
        } => NotificationEvent::StreamOnline {
            streamer_id,
            streamer_name,
            title,
            category,
            timestamp: ms_to_datetime(started_at_ms),
        },
        OutboxEvent::StreamOffline {
            streamer_id,
            streamer_name,
            duration_secs,
            ended_at_ms,
            ..
        } => NotificationEvent::StreamOffline {
            streamer_id,
            streamer_name,
            duration_secs,
            timestamp: ms_to_datetime(ended_at_ms),
        },
        OutboxEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type,
            message,
            occurred_at_ms,
            ..
        } => NotificationEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type: format!("{error_type:?}"),
            message,
            timestamp: ms_to_datetime(occurred_at_ms),
        },
        OutboxEvent::ChecksFailing {
            streamer_id,
            streamer_name,
            platform,
            last_error,
            consecutive_errors,
            occurred_at_ms,
        } => NotificationEvent::StreamerChecksFailing {
            streamer_id,
            streamer_name,
            platform,
            last_error,
            consecutive_errors,
            timestamp: ms_to_datetime(occurred_at_ms),
        },
        OutboxEvent::ChecksRestored {
            streamer_id,
            streamer_name,
            occurred_at_ms,
        } => NotificationEvent::StreamerChecksRestored {
            streamer_id,
            streamer_name,
            timestamp: ms_to_datetime(occurred_at_ms),
        },
    }
}

/// Summary of one [`NotificationOutboxDrainer::drain_once`] pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxDrainReport {
    /// Total undelivered rows fetched in this pass.
    pub fetched: usize,
    /// Alertable rows settled via [`NotificationService::deliver_durable_for_outbox`]
    /// and marked delivered.
    pub delivered: usize,
    /// Non-notifying legacy rows (or unparseable poison rows) marked delivered
    /// without invoking notification delivery.
    pub skipped: usize,
    /// Rows that could not settle (shutdown or transient delivery error) and
    /// were kept undelivered for replay.
    pub failed: usize,
    /// `true` when the drainer paused early due to shutdown or a transient error.
    pub needs_wait: bool,
}

/// Single-consumer durable outbox drainer for `monitor_event_outbox`.
pub struct NotificationOutboxDrainer {
    write_pool: SqlitePool,
    notification_service: Arc<NotificationService>,
    notify: Arc<Notify>,
    cancellation: CancellationToken,
    batch_size: i32,
    poll_interval: Duration,
}

impl NotificationOutboxDrainer {
    /// Create a new outbox drainer backed by `write_pool` and `notification_service`.
    pub fn new(
        write_pool: SqlitePool,
        notification_service: Arc<NotificationService>,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            write_pool,
            notification_service,
            notify: Arc::new(Notify::new()),
            cancellation,
            batch_size: DEFAULT_BATCH_SIZE,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }

    /// Override the maximum rows fetched per drain batch (clamped to `>= 1`).
    pub fn with_batch_size(mut self, batch_size: i32) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Override the fallback polling interval when no wake notification arrives.
    pub fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    /// Shared wake handle notified after commits insert rows into `monitor_event_outbox`.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Process up to `batch_size` undelivered outbox rows in ascending `id` order.
    ///
    /// A row is marked `delivered_at` only after its notification has settled
    /// (or after confirming it is a non-notifying legacy record). Shutdown or
    /// transient settlement errors leave the affected row undelivered and halt
    /// further head-of-line progression until retry.
    pub async fn drain_once(&self) -> Result<OutboxDrainReport> {
        if self.cancellation.is_cancelled() {
            return Ok(OutboxDrainReport {
                needs_wait: true,
                ..Default::default()
            });
        }

        let entries =
            MonitorOutboxOps::fetch_undelivered(&self.write_pool, self.batch_size).await?;
        if entries.is_empty() {
            return Ok(OutboxDrainReport::default());
        }

        let mut report = OutboxDrainReport {
            fetched: entries.len(),
            ..Default::default()
        };
        let mut delivered_ids = Vec::with_capacity(entries.len());
        let mut failed_entries: Vec<(i64, String)> = Vec::new();

        for entry in entries {
            if self.cancellation.is_cancelled() {
                report.needs_wait = true;
                break;
            }

            let record = match OutboxRecord::decode(&entry.payload) {
                Ok(record) => record,
                Err(error) => {
                    // Malformed JSON cannot recover on retry and would block
                    // head-of-line progression (`ORDER BY id`).
                    warn!(
                        outbox_id = entry.id,
                        event_type = %entry.event_type,
                        %error,
                        "Unparseable notification outbox payload; marking delivered to unblock head of line"
                    );
                    delivered_ids.push(entry.id);
                    report.skipped += 1;
                    continue;
                }
            };

            let Some(notification) = record.into_notification_event() else {
                debug!(
                    outbox_id = entry.id,
                    event_type = %entry.event_type,
                    "Marking non-notifying legacy outbox record delivered"
                );
                delivered_ids.push(entry.id);
                report.skipped += 1;
                continue;
            };

            let delivery = tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {
                    Err(DurableDeliveryError::TransientShuttingDown)
                }
                result = self.notification_service.deliver_durable_for_outbox(
                    notification,
                    entry.streamer_id.as_deref(),
                ) => result,
            };

            match delivery {
                Ok(()) => {
                    delivered_ids.push(entry.id);
                    report.delivered += 1;
                }
                Err(DurableDeliveryError::TransientShuttingDown) => {
                    debug!(
                        outbox_id = entry.id,
                        event_type = %entry.event_type,
                        "Retaining undelivered outbox entry during shutdown"
                    );
                    report.failed += 1;
                    report.needs_wait = true;
                    break;
                }
                Err(DurableDeliveryError::Transient(error)) => {
                    warn!(
                        outbox_id = entry.id,
                        event_type = %entry.event_type,
                        %error,
                        "Durable notification settlement failed; retaining outbox entry for retry"
                    );
                    failed_entries.push((entry.id, error));
                    report.failed += 1;
                    report.needs_wait = true;
                    break;
                }
            }
        }

        // Flush rows that already settled before returning so they are not
        // replayed on restart; any failed or interrupted row is never included.
        MonitorOutboxOps::mark_delivered_batch(&self.write_pool, &delivered_ids).await?;
        MonitorOutboxOps::record_failure_batch(&self.write_pool, &failed_entries).await?;

        Ok(report)
    }

    /// Repeatedly call [`Self::drain_once`] until the outbox is empty, the
    /// drainer is cancelled, or a transient failure requires waiting.
    pub async fn drain_until_wait(&self) -> Result<OutboxDrainReport> {
        let mut total = OutboxDrainReport::default();
        loop {
            if self.cancellation.is_cancelled() {
                total.needs_wait = true;
                return Ok(total);
            }
            let step = self.drain_once().await?;
            total.fetched += step.fetched;
            total.delivered += step.delivered;
            total.skipped += step.skipped;
            total.failed += step.failed;
            total.needs_wait = step.needs_wait;
            if step.fetched == 0 || step.needs_wait {
                return Ok(total);
            }
        }
    }

    /// Run the background outbox drain loop until `cancellation` triggers.
    pub async fn run(&self) {
        loop {
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {
                    info!("Notification outbox drainer shutting down");
                    break;
                }
                _ = self.notify.notified() => {}
                _ = tokio::time::sleep(self.poll_interval) => {}
            }

            if self.cancellation.is_cancelled() {
                break;
            }

            if let Err(error) = self.drain_until_wait().await {
                warn!(%error, "Notification outbox drain pass failed");
            }
        }
        debug!("Notification outbox drainer stopped");
    }
}
