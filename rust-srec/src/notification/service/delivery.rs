use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Utc;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::database::models::notification::NotificationDeadLetterDbModel;

use super::{
    DeadLetterEntry, DeliveryStatus, NotificationService, NotificationServiceConfig,
    ProcessingParams, RetryParams,
};

impl NotificationService {
    pub(super) async fn process_notification(&self, id: u64) {
        let channels = self.channels.read().clone();
        Self::process_notification_detached(ProcessingParams {
            id,
            channels,
            pending_queue: self.pending_queue.clone(),
            dead_letters: self.dead_letters.clone(),
            dead_letter_cleanup_ts: self.dead_letter_cleanup_ts.clone(),
            circuit_breakers: self.circuit_breakers.clone(),
            notification_repo: self.notification_repo.clone(),
            config: self.config.clone(),
            next_dead_letter_id: self.next_dead_letter_id.clone(),
            cancellation_token: self.cancellation_token.clone(),
            task_supervisor: self.task_supervisor.clone(),
        })
        .await;
    }

    /// Calculate retry delay with exponential backoff and jitter.
    pub(super) fn _calculate_retry_delay(&self, attempts: u32) -> Duration {
        Self::calculate_retry_delay_detached(&self.config, attempts)
    }

    fn spawn_retry_detached(params: RetryParams) {
        let RetryParams {
            id,
            delay,
            expected_generation,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
            task_supervisor,
        } = params;
        debug!(
            notification_id = id,
            ?delay,
            generation = expected_generation,
            "Scheduling notification retry"
        );

        let retry_supervisor = task_supervisor.clone();
        task_supervisor.spawn("notification retry", async move {
            tokio::select! {
                _ = cancellation_token.cancelled() => return,
                _ = sleep(delay) => {},
            }

            let should_run = pending_queue
                .get(&id)
                .map(|pending| pending.retry_generation == expected_generation)
                .unwrap_or(false);
            if !should_run {
                return;
            }

            Self::process_notification_detached(ProcessingParams {
                id,
                channels,
                pending_queue,
                dead_letters,
                dead_letter_cleanup_ts,
                circuit_breakers,
                notification_repo,
                config,
                next_dead_letter_id,
                cancellation_token,
                task_supervisor: retry_supervisor,
            })
            .await;
        });
    }

    fn calculate_retry_delay_detached(
        config: &NotificationServiceConfig,
        attempts: u32,
    ) -> Duration {
        let delay_ms = config
            .initial_retry_delay_ms
            .saturating_mul(2u64.saturating_pow(attempts))
            .min(config.max_retry_delay_ms);
        let jitter_range = delay_ms / 4;
        let jitter = if jitter_range > 0 {
            (rand::random::<u64>() % (jitter_range * 2)).saturating_sub(jitter_range)
        } else {
            0
        };

        Duration::from_millis(delay_ms.saturating_add(jitter))
    }

    async fn process_notification_detached(params: ProcessingParams) {
        let ProcessingParams {
            id,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
            task_supervisor,
        } = params;
        let pending_snapshot = match pending_queue.get(&id) {
            Some(pending) => pending.clone(),
            None => return,
        };
        let mut circuit_blocked = false;

        for channel in &channels {
            let channel_key = channel.key.clone();
            let is_pending = pending_queue.get(&id).and_then(|pending| {
                pending
                    .channel_state
                    .get(&channel_key)
                    .map(|state| state.status)
            }) == Some(DeliveryStatus::Pending);
            if !is_pending {
                continue;
            }

            let allowed = circuit_breakers
                .get(&channel_key)
                .map(|breaker| breaker.is_allowed())
                .unwrap_or(true);
            if !allowed {
                circuit_blocked = true;
                continue;
            }

            match channel.channel.send(&pending_snapshot.event).await {
                Ok(()) => {
                    if let Some(mut breaker) = circuit_breakers.get_mut(&channel_key) {
                        breaker.record_success();
                    }
                    if let Some(mut pending) = pending_queue.get_mut(&id)
                        && let Some(state) = pending.channel_state.get_mut(&channel_key)
                    {
                        state.status = DeliveryStatus::Delivered;
                        state.last_attempt = Some(Utc::now());
                        state.last_error = None;
                    }
                    debug!(notification_id = id, channel = %channel.channel_type, "Notification delivered");
                }
                Err(error) => {
                    if let Some(mut breaker) = circuit_breakers.get_mut(&channel_key) {
                        breaker.record_failure(config.circuit_breaker_threshold);
                    }

                    let now = Utc::now();
                    let mut attempts = 0;
                    let mut dead_lettered = false;
                    if let Some(mut pending) = pending_queue.get_mut(&id)
                        && let Some(state) = pending.channel_state.get_mut(&channel_key)
                    {
                        state.attempts += 1;
                        state.last_attempt = Some(now);
                        state.last_error = Some(error.to_string());
                        attempts = state.attempts;
                        if state.attempts >= config.max_retries {
                            state.status = DeliveryStatus::DeadLettered;
                            dead_lettered = true;
                        }
                    }

                    if dead_lettered
                        && let (Some(repo), Some(db_channel_id)) =
                            (notification_repo.clone(), channel.db_channel_id.clone())
                    {
                        if let Ok(payload) = serde_json::to_string(&pending_snapshot.event) {
                            let entry = NotificationDeadLetterDbModel::new(
                                db_channel_id.clone(),
                                pending_snapshot.event.event_type(),
                                payload,
                                error.to_string(),
                                attempts as i32,
                                pending_snapshot.created_at.timestamp_millis(),
                            );
                            if let Err(persist_error) = repo.add_to_dead_letter(&entry).await {
                                warn!(
                                    channel_id = %db_channel_id,
                                    error = %persist_error,
                                    "Failed to persist dead letter entry"
                                );
                            }
                        }

                        let dead_letter_id = next_dead_letter_id.fetch_add(1, Ordering::SeqCst);
                        dead_letters.insert(
                            dead_letter_id,
                            DeadLetterEntry {
                                id: dead_letter_id,
                                notification_id: id,
                                event: pending_snapshot.event.clone(),
                                channel_key: Some(channel_key.clone()),
                                channel_id: channel.db_channel_id.clone(),
                                channel_type: channel.channel_type.clone(),
                                attempts,
                                error: error.to_string(),
                                created_at: pending_snapshot.created_at,
                                dead_lettered_at: now,
                            },
                        );
                        Self::maybe_cleanup_dead_letters_detached(
                            &dead_letters,
                            config.dead_letter_retention_days,
                            &dead_letter_cleanup_ts,
                            now,
                        );
                        warn!(
                            notification_id = id,
                            channel = %channel.channel_type,
                            attempts = config.max_retries,
                            "Notification dead-lettered"
                        );
                    }
                }
            }
        }

        let (has_pending, min_delay) = match pending_queue.get(&id) {
            Some(pending) => {
                let mut min_delay: Option<Duration> = None;
                let mut has_pending = false;
                for state in pending.channel_state.values() {
                    if state.status != DeliveryStatus::Pending {
                        continue;
                    }
                    has_pending = true;
                    let delay = Self::calculate_retry_delay_detached(&config, state.attempts);
                    min_delay = Some(min_delay.map_or(delay, |current| current.min(delay)));
                }
                (has_pending, min_delay)
            }
            None => return,
        };

        if !has_pending {
            pending_queue.remove(&id);
            return;
        }

        let mut delay = min_delay.unwrap_or_else(|| Duration::from_secs(1));
        if circuit_blocked {
            delay = delay.max(Duration::from_secs(config.circuit_breaker_cooldown_secs));
        }
        let expected_generation = match pending_queue.get_mut(&id) {
            Some(mut pending) => {
                pending.retry_generation = pending.retry_generation.saturating_add(1);
                pending.next_retry_at = Some(
                    Utc::now()
                        + chrono::Duration::from_std(delay)
                            .unwrap_or_else(|_| chrono::Duration::seconds(delay.as_secs() as i64)),
                );
                pending.retry_generation
            }
            None => return,
        };

        Self::spawn_retry_detached(RetryParams {
            id,
            delay,
            expected_generation,
            channels,
            pending_queue,
            dead_letters,
            dead_letter_cleanup_ts,
            circuit_breakers,
            notification_repo,
            config,
            next_dead_letter_id,
            cancellation_token,
            task_supervisor,
        });
    }
}
