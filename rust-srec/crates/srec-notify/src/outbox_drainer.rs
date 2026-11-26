//! Durable notification outbox drainer with two-level record decoding.
//!
//! Scans undelivered rows from `monitor_event_outbox` in ascending `id` order,
//! decodes both current [`OutboxEvent`] records and legacy [`LegacyOutboxEvent`]
//! records left behind across upgrades, settles alertable events through the
//! [`NotificationService`], and marks `delivered_at` only after a row's
//! settlement succeeds.
//!
//! Up to 32 rows settle at the same time, so an alert that waits on a slow or
//! failing channel does not hold back the alerts of other streamers. A
//! streamer's alerts still settle one at a time in `id` order: a row is
//! admitted only after every earlier undelivered row of the same streamer has
//! settled. A row whose settlement fails is retried with a backoff that grows
//! with its `attempts`, holding back only the later rows of its own streamer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Notify;
use tokio::task::{self, JoinError, JoinSet};
use tokio::time::{self, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use srec_core::domain::notification::OutboxEvent;
use srec_core::domain::streamer::FatalErrorType;
use srec_sqlite::repositories::{MonitorOutboxOps, OutboxEntry};
use srec_sqlite::time::ms_to_datetime;

use crate::NotifyResult as Result;
use crate::service::DurableDeliveryError;
use crate::{NotificationEvent, NotificationService};

/// Rows read per query while scanning for rows to admit.
const PAGE_SIZE: i32 = 100;
/// Fallback wake-up for rows whose commit notification was missed.
const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Rows whose notifications may settle at the same time.
const MAX_IN_FLIGHT: usize = 32;
/// Wait after a row's first failed settlement before it is admitted again.
const RETRY_BASE_DELAY: Duration = Duration::from_secs(1);
/// Longest wait between two attempts at a row that keeps failing.
const RETRY_MAX_DELAY: Duration = Duration::from_secs(300);

/// Wait before a row whose settlement has failed `attempts` times is admitted
/// again: `base` (`RETRY_BASE_DELAY` outside tests), doubled for each earlier
/// failure, at most `RETRY_MAX_DELAY`. A row's first attempt after a restart
/// is not delayed.
fn retry_delay(attempts: i64, base: Duration) -> Duration {
    // Clamped before the cast; 2^16 times the base is far past the cap.
    let doublings = attempts.saturating_sub(1).clamp(0, 16) as u32;
    base.saturating_mul(1 << doublings).min(RETRY_MAX_DELAY)
}

/// Minimal JSON wire-compatible projection of legacy `MonitorEvent` outbox rows.
///
/// Retains exact field compatibility with pre-v2 `monitor_event_outbox` rows
/// without importing outer monitoring types. Non-notifying variants ignore
/// their JSON object payload while validating that the variant name and object
/// framing match the legacy wire format.
#[derive(Debug, Clone, Deserialize)]
pub enum LegacyOutboxEvent {
    StreamerLive {},
    StreamerOffline {},
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        timestamp: Timestamp,
    },
    TransientError {},
    ChecksFailing {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        last_error: String,
        consecutive_errors: i32,
        timestamp: Timestamp,
    },
    ChecksRestored {
        streamer_id: String,
        streamer_name: String,
        timestamp: Timestamp,
    },
    StateChanged {},
}

/// Decoded payload from a `monitor_event_outbox` row.
#[derive(Debug, Clone)]
pub enum OutboxRecord {
    /// Current notification-only outbox record written alongside state commits.
    V2(OutboxEvent),
    /// Legacy `MonitorEvent` record written before migration to `OutboxEvent`.
    Legacy(LegacyOutboxEvent),
}

impl OutboxRecord {
    /// Decode a raw JSON outbox payload, trying [`OutboxEvent`] before falling
    /// back to [`LegacyOutboxEvent`].
    pub fn decode(payload: &str) -> std::result::Result<Self, serde_json::Error> {
        if let Ok(event) = serde_json::from_str::<OutboxEvent>(payload) {
            return Ok(Self::V2(event));
        }
        serde_json::from_str::<LegacyOutboxEvent>(payload).map(Self::Legacy)
    }

    /// Translate this outbox record into a user-facing [`NotificationEvent`],
    /// or `None` for non-notifying legacy records (`StreamerLive`,
    /// `StreamerOffline`, `StateChanged`, `TransientError`).
    pub fn into_notification_event(self) -> Option<NotificationEvent> {
        match self {
            Self::V2(event) => Some(outbox_event_to_notification(event)),
            Self::Legacy(event) => legacy_outbox_to_notification(event),
        }
    }
}

fn legacy_outbox_to_notification(event: LegacyOutboxEvent) -> Option<NotificationEvent> {
    match event {
        LegacyOutboxEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type,
            message,
            timestamp,
        } => Some(NotificationEvent::FatalError {
            streamer_id,
            streamer_name,
            error_type: format!("{error_type:?}"),
            message,
            timestamp,
        }),
        LegacyOutboxEvent::ChecksFailing {
            streamer_id,
            streamer_name,
            platform,
            last_error,
            consecutive_errors,
            timestamp,
        } => Some(NotificationEvent::StreamerChecksFailing {
            streamer_id,
            streamer_name,
            platform,
            last_error,
            consecutive_errors,
            timestamp,
        }),
        LegacyOutboxEvent::ChecksRestored {
            streamer_id,
            streamer_name,
            timestamp,
        } => Some(NotificationEvent::StreamerChecksRestored {
            streamer_id,
            streamer_name,
            timestamp,
        }),
        LegacyOutboxEvent::StreamerLive {}
        | LegacyOutboxEvent::StreamerOffline {}
        | LegacyOutboxEvent::TransientError {}
        | LegacyOutboxEvent::StateChanged {} => None,
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

/// Summary of one [`NotificationOutboxDrainer::drain_until_wait`] call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxDrainReport {
    /// Alertable rows whose notification settled and that were marked delivered.
    pub delivered: usize,
    /// Non-notifying legacy rows (or unparseable poison rows) marked delivered
    /// without invoking notification delivery.
    pub skipped: usize,
    /// Deliveries that ended without settling (a transient failure, shutdown,
    /// or cancellation of the drainer); their rows stay undelivered for replay.
    pub failed: usize,
    /// `true` when undelivered rows were left for later: rows whose delivery
    /// failed during the call, the later rows of their streamers, or rows
    /// left by shutdown or cancellation.
    pub needs_wait: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DrainMode {
    /// Return once nothing is in flight and no remaining row can be admitted.
    UntilIdle,
    /// Keep draining until cancelled, retrying failed rows when they are due.
    Background,
}

type DeliveryResult = std::result::Result<(), DurableDeliveryError>;

struct InFlightRow {
    id: i64,
    event_type: String,
    streamer: Option<String>,
    /// The row's `attempts` when it was admitted.
    attempts: i64,
}

/// Bookkeeping of one drain loop.
#[derive(Default)]
struct DrainState {
    /// Deliveries of admitted rows. Dropping the set aborts them, which leaves
    /// their rows undelivered for replay.
    in_flight: JoinSet<DeliveryResult>,
    rows: HashMap<task::Id, InFlightRow>,
    in_flight_ids: HashSet<i64>,
    /// Streamers with a row in flight; their later rows wait for it.
    busy_streamers: HashSet<String>,
    /// Rows whose settlement failed, with the time they may be admitted again.
    retry_at: HashMap<i64, Instant>,
    /// Rows to mark delivered: settled rows and rows that need no delivery.
    /// They are not admitted again while the write is outstanding.
    to_mark: HashSet<i64>,
    /// Failures to record in the rows' `attempts` and `last_error`.
    failures: Vec<(i64, String)>,
    report: OutboxDrainReport,
    /// Set once the notification service reports shutdown; nothing is
    /// admitted after it.
    service_stopped: bool,
}

impl DrainState {
    /// Release the row of a finished delivery and record its outcome.
    fn settle(
        &mut self,
        joined: std::result::Result<(task::Id, DeliveryResult), JoinError>,
        retry_base_delay: Duration,
    ) {
        let (task_id, outcome) = match joined {
            Ok((task_id, outcome)) => (task_id, outcome),
            // A delivery that panicked is retried like a transient failure.
            Err(error) => (
                error.id(),
                Err(DurableDeliveryError::Transient(format!(
                    "notification delivery task failed: {error}"
                ))),
            ),
        };
        let Some(row) = self.rows.remove(&task_id) else {
            return;
        };
        self.in_flight_ids.remove(&row.id);
        if let Some(streamer) = &row.streamer {
            self.busy_streamers.remove(streamer);
        }
        match outcome {
            Ok(()) => {
                self.to_mark.insert(row.id);
                self.report.delivered += 1;
            }
            Err(DurableDeliveryError::TransientShuttingDown) => {
                debug!(
                    outbox_id = row.id,
                    event_type = %row.event_type,
                    "Retaining undelivered outbox entry during shutdown"
                );
                self.service_stopped = true;
                self.report.failed += 1;
            }
            Err(DurableDeliveryError::Transient(error)) => {
                let attempts = row.attempts.saturating_add(1);
                let delay = retry_delay(attempts, retry_base_delay);
                warn!(
                    outbox_id = row.id,
                    event_type = %row.event_type,
                    attempts,
                    retry_in = ?delay,
                    %error,
                    "Durable notification settlement failed; retaining outbox entry for retry"
                );
                self.retry_at.insert(row.id, Instant::now() + delay);
                self.failures.push((row.id, error));
                self.report.failed += 1;
            }
        }
    }
}

/// The notification an outbox row asks for, or `None` for a row that needs no
/// delivery: a non-notifying legacy record or a payload that cannot be decoded.
fn notification_for(entry: &OutboxEntry) -> Option<NotificationEvent> {
    match OutboxRecord::decode(&entry.payload) {
        Ok(record) => {
            let notification = record.into_notification_event();
            if notification.is_none() {
                debug!(
                    outbox_id = entry.id,
                    event_type = %entry.event_type,
                    "Marking non-notifying legacy outbox record delivered"
                );
            }
            notification
        }
        Err(error) => {
            // Malformed JSON cannot recover on retry; leaving it undelivered
            // would only fetch it again on every scan.
            warn!(
                outbox_id = entry.id,
                event_type = %entry.event_type,
                %error,
                "Unparseable notification outbox payload; marking delivered"
            );
            None
        }
    }
}

/// Event-log id of an outbox row's notification, the same on every attempt to
/// deliver that row.
///
/// `monitor_event_outbox.id` is a rowid without `AUTOINCREMENT`, so SQLite
/// reuses an id once the newest rows are pruned; `created_at` tells such rows
/// apart. Event-log ids are UUIDs elsewhere and API clients validate them as
/// such, so the id is a name-based (version 8) UUID built from SHA-256.
fn outbox_event_log_id(outbox_id: i64, created_at_ms: i64) -> String {
    let digest = Sha256::new()
        .chain_update(b"monitor_event_outbox")
        .chain_update(outbox_id.to_be_bytes())
        .chain_update(created_at_ms.to_be_bytes())
        .finalize();
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// Single-consumer durable outbox drainer for `monitor_event_outbox`.
///
/// Only one drain, [`Self::run`] or [`Self::drain_until_wait`], may run on a
/// database at a time; two would deliver the same rows twice.
pub struct NotificationOutboxDrainer {
    write_pool: SqlitePool,
    notification_service: Arc<NotificationService>,
    notify: Arc<Notify>,
    cancellation: CancellationToken,
    retry_base_delay: Duration,
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
            retry_base_delay: RETRY_BASE_DELAY,
        }
    }

    /// Wait `delay` instead of `RETRY_BASE_DELAY` after a row's first failed
    /// settlement, so tests of the retry loop take milliseconds.
    #[cfg(test)]
    fn with_retry_base_delay(mut self, delay: Duration) -> Self {
        self.retry_base_delay = delay;
        self
    }

    /// Shared wake handle notified after commits insert rows into `monitor_event_outbox`.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Drain until nothing is in flight and no remaining row can be admitted.
    ///
    /// A row whose settlement fails is not attempted again within the call; it
    /// and the later rows of its streamer are left undelivered and the report
    /// sets `needs_wait`. Shutdown of the notification service or cancellation
    /// of the drainer stops admission and abandons rows still in flight, which
    /// stay undelivered for replay.
    pub async fn drain_until_wait(&self) -> Result<OutboxDrainReport> {
        self.drain(DrainMode::UntilIdle).await
    }

    /// Run the background outbox drain loop until `cancellation` triggers or
    /// the notification service shuts down, retrying failed rows as their
    /// backoff expires.
    pub async fn run(&self) {
        if let Err(error) = self.drain(DrainMode::Background).await {
            warn!(%error, "Notification outbox drainer failed to record settled rows");
        }
        info!("Notification outbox drainer stopped");
    }

    async fn drain(&self, mode: DrainMode) -> Result<OutboxDrainReport> {
        let background = mode == DrainMode::Background;
        let mut state = DrainState::default();
        let outcome = loop {
            if self.cancellation.is_cancelled() {
                break Ok(());
            }
            if let Err(error) = self.admit(&mut state, mode).await {
                if !background {
                    break Err(error);
                }
                warn!(%error, "Notification outbox drain pass failed");
            }
            if state.in_flight.is_empty() && (!background || state.service_stopped) {
                break Ok(());
            }
            // Wake for the earliest backoff that is still running. A row already
            // due but not admitted waits for a free slot; a row settling (or the
            // poll) wakes the loop, and waking for the past deadline would spin.
            let now = Instant::now();
            let retry_due = state
                .retry_at
                .values()
                .copied()
                .filter(|due| background && *due > now)
                .min();
            tokio::select! {
                biased;
                _ = self.cancellation.cancelled() => {}
                Some(joined) = state.in_flight.join_next_with_id() => state.settle(joined, self.retry_base_delay),
                _ = self.notify.notified(), if background => {}
                _ = time::sleep_until(retry_due.unwrap_or_else(Instant::now)), if retry_due.is_some() => {}
                _ = time::sleep(POLL_INTERVAL), if background => {}
            }
        };
        // Abandoned deliveries leave their rows undelivered for replay. Those
        // that finished before the abort still settle, so their rows are not
        // delivered again.
        state.in_flight.abort_all();
        while let Some(joined) = state.in_flight.join_next_with_id().await {
            match joined {
                Err(error) if error.is_cancelled() => state.report.failed += 1,
                joined => state.settle(joined, self.retry_base_delay),
            }
        }
        let recorded = self.flush(&mut state).await;
        state.report.needs_wait =
            self.cancellation.is_cancelled() || state.service_stopped || !state.retry_at.is_empty();
        outcome.and(recorded).map(|()| state.report)
    }

    /// Record settled rows, then admit rows in `id` order while fewer than
    /// `MAX_IN_FLIGHT` are in flight.
    async fn admit(&self, state: &mut DrainState, mode: DrainMode) -> Result<()> {
        self.flush(state).await?;
        if state.service_stopped {
            return Ok(());
        }
        let now = Instant::now();
        // Streamers whose earliest undelivered row stays out of this pass;
        // their later rows must not overtake it.
        let mut held = HashSet::new();
        let mut after_id = i64::MIN;
        'scan: while state.in_flight.len() < MAX_IN_FLIGHT {
            let page =
                MonitorOutboxOps::fetch_undelivered_after(&self.write_pool, after_id, PAGE_SIZE)
                    .await?;
            let last_page = page.len() < PAGE_SIZE as usize;
            for entry in page {
                after_id = entry.id;
                if state.in_flight.len() >= MAX_IN_FLIGHT {
                    break 'scan;
                }
                if state.in_flight_ids.contains(&entry.id) || state.to_mark.contains(&entry.id) {
                    continue;
                }
                let Some(notification) = notification_for(&entry) else {
                    state.to_mark.insert(entry.id);
                    state.report.skipped += 1;
                    continue;
                };
                // Order follows the streamer named in the payload: the
                // `streamer_id` column turns NULL when the streamer is deleted
                // while its rows wait, and every outbox alert names one.
                let streamer = notification.streamer_id().map(str::to_owned);
                if streamer
                    .as_ref()
                    .is_some_and(|id| state.busy_streamers.contains(id) || held.contains(id))
                {
                    continue;
                }
                let waiting = state
                    .retry_at
                    .get(&entry.id)
                    .is_some_and(|due| mode == DrainMode::UntilIdle || *due > now);
                if waiting {
                    held.extend(streamer);
                    continue;
                }
                state.retry_at.remove(&entry.id);
                self.spawn_delivery(state, entry, notification, streamer);
            }
            if last_page {
                break;
            }
        }
        self.flush(state).await
    }

    fn spawn_delivery(
        &self,
        state: &mut DrainState,
        entry: OutboxEntry,
        notification: NotificationEvent,
        streamer: Option<String>,
    ) {
        let service = self.notification_service.clone();
        let outbox_streamer_id = entry.streamer_id;
        let event_log_id = outbox_event_log_id(entry.id, entry.created_at);
        let task = state.in_flight.spawn(async move {
            service
                .deliver_durable_for_outbox(
                    notification,
                    outbox_streamer_id.as_deref(),
                    event_log_id,
                )
                .await
        });
        state.in_flight_ids.insert(entry.id);
        if let Some(streamer) = &streamer {
            state.busy_streamers.insert(streamer.clone());
        }
        state.rows.insert(
            task.id(),
            InFlightRow {
                id: entry.id,
                event_type: entry.event_type,
                streamer,
                attempts: entry.attempts,
            },
        );
    }

    /// Write `delivered_at` for settled rows and record failures.
    ///
    /// Rows stay in `to_mark`, and failures in `failures`, until their write
    /// succeeds: a failed write is retried rather than delivering the rows
    /// again or losing the `attempts` that their backoff grows with.
    async fn flush(&self, state: &mut DrainState) -> Result<()> {
        if !state.to_mark.is_empty() {
            let ids: Vec<i64> = state.to_mark.iter().copied().collect();
            MonitorOutboxOps::mark_delivered_batch(&self.write_pool, &ids).await?;
            state.to_mark.clear();
        }
        let failures = std::mem::take(&mut state.failures);
        if let Err(error) =
            MonitorOutboxOps::record_failure_batch(&self.write_pool, &failures).await
        {
            state.failures = failures;
            return Err(error.into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
