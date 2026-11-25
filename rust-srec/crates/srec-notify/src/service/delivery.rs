use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use futures::StreamExt;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use srec_core::utils::task_supervisor::TaskSupervisor;
use srec_sqlite::models::notification::NotificationDeadLetterDbModel;
use srec_sqlite::repositories::NotificationRepository;

use crate::events::RenderCache;

use super::{
    DeadLetterEntry, DeliveryStatus, DurableDeliveryError, DurableSettlement, NotificationService,
    NotificationServiceConfig, PendingNotification, RuntimeChannel,
};

/// State fixed for the lifetime of one notification's delivery and shared by
/// the initial `process_notification_detached` pass and every retry hop
/// `spawn_retry_detached` schedules: the `pending_queue` id, the channel
/// snapshot taken at enqueue time (retries deliver to these channel objects
/// even if `reload_from_db` swaps the live set), and the
/// `NotificationService` handles the detached tasks run against. Per-hop
/// values (retry delay, expected generation) are passed alongside instead.
struct DeliveryContext {
    id: u64,
    channels: Vec<Arc<RuntimeChannel>>,
    pending_queue: Arc<DashMap<u64, PendingNotification>>,
    dead_letters: Arc<DashMap<u64, DeadLetterEntry>>,
    dead_letter_cleanup_ts: Arc<AtomicU64>,
    notification_repo: Option<Arc<dyn NotificationRepository>>,
    config: Arc<NotificationServiceConfig>,
    next_dead_letter_id: Arc<AtomicU64>,
    cancellation_token: CancellationToken,
    retry_cancel: CancellationToken,
    task_supervisor: Arc<TaskSupervisor>,
    delivery_permits: Arc<Semaphore>,
    durable_settled: Option<DurableSettlement>,
}

impl NotificationService {
    pub(super) async fn process_notification(&self, id: u64) {
        let Some((channels, retry_cancel, durable_settled)) =
            self.pending_queue.get(&id).map(|pending| {
                (
                    pending.targets.clone(),
                    pending.retry_cancel.clone(),
                    pending.durable_settled.clone(),
                )
            })
        else {
            return;
        };
        Self::process_notification_detached(
            Arc::new(DeliveryContext {
                id,
                channels,
                pending_queue: self.pending_queue.clone(),
                dead_letters: self.dead_letters.clone(),
                dead_letter_cleanup_ts: self.dead_letter_cleanup_ts.clone(),
                notification_repo: self.notification_repo.clone(),
                config: self.config.clone(),
                next_dead_letter_id: self.next_dead_letter_id.clone(),
                cancellation_token: self.cancellation_token.clone(),
                retry_cancel,
                task_supervisor: self.task_supervisor.clone(),
                delivery_permits: self.delivery_permits.clone(),
                durable_settled,
            }),
            None,
        )
        .await;
    }

    fn spawn_retry_detached(delay: Duration, expected_generation: u64, ctx: Arc<DeliveryContext>) {
        debug!(
            notification_id = ctx.id,
            ?delay,
            generation = expected_generation,
            "Scheduling notification retry"
        );

        let supervisor = ctx.task_supervisor.clone();
        let task_ctx = ctx.clone();
        let spawned = supervisor.spawn("notification retry", async move {
            tokio::select! {
                biased;
                _ = task_ctx.cancellation_token.cancelled() => {
                    if let Some(settlement) = &task_ctx.durable_settled {
                        if let Some((_, removed)) = task_ctx.pending_queue.remove(&task_ctx.id) {
                            removed.retry_cancel.cancel();
                        }
                        settlement.settle_err(DurableDeliveryError::TransientShuttingDown);
                    }
                    return;
                }
                _ = task_ctx.retry_cancel.cancelled() => {
                    if let Some(settlement) = &task_ctx.durable_settled {
                        settlement.settle_err(DurableDeliveryError::TransientShuttingDown);
                    }
                    return;
                }
                _ = sleep(delay) => {},
            }

            let should_run = task_ctx
                .pending_queue
                .get(&task_ctx.id)
                .map(|pending| pending.retry_generation == expected_generation)
                .unwrap_or(false);
            if !should_run {
                return;
            }

            Self::process_notification_detached(task_ctx, Some(expected_generation)).await;
        });

        if !spawned && let Some(settlement) = &ctx.durable_settled {
            if let Some((_, removed)) = ctx.pending_queue.remove(&ctx.id) {
                removed.retry_cancel.cancel();
            }
            settlement.settle_err(DurableDeliveryError::TransientShuttingDown);
        }
    }

    fn calculate_retry_delay_detached(
        config: &NotificationServiceConfig,
        attempts: u32,
    ) -> Duration {
        Self::jittered_retry_delay(config, attempts, rand::random())
    }

    /// Exponential backoff with ±25% jitter, derived from a uniform `sample`.
    ///
    /// The jittered delay never exceeds `max_retry_delay_ms`; at the cap the
    /// jitter can therefore only shorten the delay.
    fn jittered_retry_delay(
        config: &NotificationServiceConfig,
        attempts: u32,
        sample: u64,
    ) -> Duration {
        let delay_ms = config
            .initial_retry_delay_ms
            .saturating_mul(2u64.saturating_pow(attempts))
            .min(config.max_retry_delay_ms);
        let jitter_range = delay_ms / 4;
        // `offset` is in `0..=2 * jitter_range`; `jitter_range <= delay_ms`, so
        // the subtraction cannot underflow.
        let offset = sample % (jitter_range * 2 + 1);
        let jittered_ms = (delay_ms - jitter_range).saturating_add(offset);

        Duration::from_millis(jittered_ms.min(config.max_retry_delay_ms))
    }

    async fn process_notification_detached(
        ctx: Arc<DeliveryContext>,
        expected_generation: Option<u64>,
    ) {
        // Borrow the context fields under their own names for the delivery
        // pass; the borrows end before `ctx` is moved into
        // `spawn_retry_detached` at the bottom.
        let DeliveryContext {
            id,
            channels,
            pending_queue,
            config,
            ..
        } = &*ctx;
        let id = *id;
        let Some(delivery_lock) = pending_queue
            .get(&id)
            .map(|pending| pending.delivery_lock.clone())
        else {
            return;
        };
        let _delivery_guard = delivery_lock.lock().await;
        if let Some(settlement) = &ctx.durable_settled
            && ctx.cancellation_token.is_cancelled()
        {
            if let Some((_, removed)) = pending_queue.remove(&id) {
                removed.retry_cancel.cancel();
            }
            settlement.settle_err(DurableDeliveryError::TransientShuttingDown);
            return;
        }
        let pending_snapshot = match pending_queue.get(&id) {
            Some(pending)
                if Arc::ptr_eq(&pending.delivery_lock, &delivery_lock)
                    && expected_generation
                        .is_none_or(|generation| pending.retry_generation == generation) =>
            {
                pending.clone()
            }
            _ => return,
        };
        let mut circuit_blocked = false;
        let mut rendered = RenderCache::new();

        {
            // The futures stay owned by this pass: eviction drops in-flight
            // sends, and retries are scheduled only after all admitted targets settle.
            let mut seen = std::collections::HashSet::new();
            let targets: Vec<_> = channels
                .iter()
                .filter(|channel| {
                    seen.insert(channel.key.clone())
                        && pending_snapshot
                            .channel_state
                            .get(channel.key.as_str())
                            .is_some_and(|state| state.status == DeliveryStatus::Pending)
                })
                .cloned()
                .collect();
            let deliveries = futures::stream::iter(targets)
                .map(|channel| {
                    let text = rendered.get(&pending_snapshot.event, channel.channel.locale());
                    let event = &pending_snapshot.event;
                    let delivery_permits = &ctx.delivery_permits;
                    async move {
                        let Ok(_permit) = delivery_permits.acquire().await else {
                            return (channel, None, false);
                        };
                        let is_pending = pending_queue.get(&id).and_then(|pending| {
                            pending
                                .channel_state
                                .get(channel.key.as_str())
                                .map(|state| state.status)
                        }) == Some(DeliveryStatus::Pending);
                        if !is_pending {
                            return (channel, None, false);
                        }
                        let allowed = channel.breaker.lock().is_allowed();
                        if !allowed {
                            return (channel, None, true);
                        }
                        // An event below the channel's `min_priority` counts as delivered.
                        let outcome = if channel.channel.accepts(event) {
                            channel.channel.send_rendered(event, &text).await
                        } else {
                            Ok(())
                        };
                        (channel, Some(outcome), false)
                    }
                })
                .buffer_unordered(4);
            tokio::pin!(deliveries);
            // A settled outcome is not cancelled: its dead letter is already in
            // memory, so its persistence must finish to keep the stores in step.
            while let Some((channel, result, blocked)) = tokio::select! {
                biased;
                _ = ctx.retry_cancel.cancelled() => None,
                _ = ctx.cancellation_token.cancelled(), if ctx.durable_settled.is_some() => None,
                next = deliveries.next() => next,
            } {
                if blocked && ctx.durable_settled.is_some() {
                    // Fail fast to durable dead-letter when a channel's circuit
                    // breaker is already open so one broken endpoint cannot stall
                    // the outbox across 300s cooldown loops. Do not record another
                    // breaker failure so the existing cooldown still expires for
                    // half-open recovery.
                    let now = jiff::Timestamp::now();
                    let channel_key = channel.key.as_str();
                    let mut attempts = 0;
                    if let Some(mut pending) = pending_queue.get_mut(&id)
                        && let Some(state) = pending.channel_state.get_mut(channel_key)
                    {
                        state.last_attempt = Some(now);
                        state.last_error = Some("circuit breaker open".to_owned());
                        state.status = DeliveryStatus::DeadLettered;
                        attempts = state.attempts;
                    }
                    Self::record_dead_letter_detached(
                        &ctx,
                        &pending_snapshot,
                        &channel,
                        attempts,
                        "circuit breaker open",
                        now,
                    )
                    .await;
                    continue;
                }
                circuit_blocked |= blocked;
                let Some(result) = result else {
                    continue;
                };
                let channel_key = channel.key.as_str();
                match result {
                    Ok(()) => {
                        channel.breaker.lock().record_success();
                        if let Some(mut pending) = pending_queue.get_mut(&id)
                            && let Some(state) = pending.channel_state.get_mut(channel_key)
                        {
                            state.status = DeliveryStatus::Delivered;
                            state.last_attempt = Some(jiff::Timestamp::now());
                            state.last_error = None;
                        }
                        debug!(notification_id = id, channel = %channel.channel_type, "Notification delivered");
                    }
                    Err(error) => {
                        let breaker_opened = {
                            let mut breaker = channel.breaker.lock();
                            breaker.record_failure(config.circuit_breaker_threshold);
                            breaker.is_open
                        };
                        circuit_blocked |= breaker_opened;

                        let now = jiff::Timestamp::now();
                        let mut attempts = 0;
                        let mut dead_lettered = false;
                        if let Some(mut pending) = pending_queue.get_mut(&id)
                            && let Some(state) = pending.channel_state.get_mut(channel_key)
                        {
                            state.attempts += 1;
                            state.last_attempt = Some(now);
                            state.last_error = Some(error.to_string());
                            attempts = state.attempts;
                            if state.attempts >= config.max_retries
                                || (breaker_opened && ctx.durable_settled.is_some())
                            {
                                state.status = DeliveryStatus::DeadLettered;
                                dead_lettered = true;
                            }
                        }

                        if dead_lettered {
                            Self::record_dead_letter_detached(
                                &ctx,
                                &pending_snapshot,
                                &channel,
                                attempts,
                                &error.to_string(),
                                now,
                            )
                            .await;
                        }
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
                    let delay = Self::calculate_retry_delay_detached(config, state.attempts);
                    min_delay = Some(min_delay.map_or(delay, |current| current.min(delay)));
                }
                (has_pending, min_delay)
            }
            None => return,
        };

        if !has_pending {
            if let Some((_, completed)) = pending_queue.remove(&id) {
                completed.retry_cancel.cancel();
                if let Some(settlement) = completed.durable_settled {
                    settlement.settle_completed();
                }
            }
            return;
        }

        if let Some(settlement) = &ctx.durable_settled
            && ctx.cancellation_token.is_cancelled()
        {
            if let Some((_, cancelled)) = pending_queue.remove(&id) {
                cancelled.retry_cancel.cancel();
            }
            settlement.settle_err(DurableDeliveryError::TransientShuttingDown);
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
                    jiff::Timestamp::now()
                        + jiff::SignedDuration::try_from(delay).unwrap_or_else(|_| {
                            jiff::SignedDuration::from_secs(delay.as_secs() as i64)
                        }),
                );
                pending.retry_generation
            }
            None => return,
        };

        Self::spawn_retry_detached(delay, expected_generation, ctx);
    }

    async fn record_dead_letter_detached(
        ctx: &DeliveryContext,
        pending_snapshot: &PendingNotification,
        channel: &RuntimeChannel,
        attempts: u32,
        error_message: &str,
        now: jiff::Timestamp,
    ) {
        let dead_letter_id = ctx.next_dead_letter_id.fetch_add(1, Ordering::SeqCst);
        ctx.dead_letters.insert(
            dead_letter_id,
            DeadLetterEntry {
                id: dead_letter_id,
                notification_id: ctx.id,
                event: pending_snapshot.event.clone(),
                channel_key: Some(channel.key.clone()),
                channel_id: channel.db_channel_id.clone(),
                channel_type: channel.channel_type.clone(),
                attempts,
                error: error_message.to_owned(),
                created_at: pending_snapshot.created_at,
                dead_lettered_at: now,
            },
        );
        Self::maybe_cleanup_dead_letters_detached(
            &ctx.dead_letters,
            ctx.config.dead_letter_retention_days,
            &ctx.dead_letter_cleanup_ts,
            now,
        );
        warn!(
            notification_id = ctx.id,
            channel = %channel.channel_type,
            attempts,
            error = error_message,
            "Notification dead-lettered"
        );

        if let (Some(repo), Some(db_channel_id)) = (
            ctx.notification_repo.as_ref(),
            channel.db_channel_id.as_ref(),
        ) {
            match serde_json::to_string(&pending_snapshot.event) {
                Ok(payload) => {
                    let entry = NotificationDeadLetterDbModel::new(
                        db_channel_id.clone(),
                        pending_snapshot.event.event_type(),
                        payload,
                        error_message.to_owned(),
                        attempts as i32,
                        pending_snapshot.created_at.as_millisecond(),
                    );
                    if let Err(persist_error) = repo.add_to_dead_letter(&entry).await {
                        warn!(
                            channel_id = %db_channel_id,
                            error = %persist_error,
                            "Failed to persist dead letter entry"
                        );
                        if let Some(settlement) = &ctx.durable_settled {
                            settlement.record_dead_letter_error(format!(
                                "failed to persist dead letter entry: {persist_error}"
                            ));
                        }
                    }
                }
                Err(serialize_error) => {
                    warn!(
                        channel_id = %db_channel_id,
                        error = %serialize_error,
                        "Failed to serialize dead letter entry"
                    );
                    if let Some(settlement) = &ctx.durable_settled {
                        settlement.record_dead_letter_error(format!(
                            "failed to serialize dead letter entry: {serialize_error}"
                        ));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_jitter_spreads_both_ways_and_respects_the_cap() {
        let config = NotificationServiceConfig {
            initial_retry_delay_ms: 1000,
            max_retry_delay_ms: 60000,
            ..Default::default()
        };
        let delay = |attempts, sample| {
            NotificationService::jittered_retry_delay(&config, attempts, sample).as_millis()
        };

        // Base delays 1000, 2000 and 4000 ms spread over ±25%.
        for (attempts, base) in [(0, 1000), (1, 2000), (2, 4000)] {
            let range = base / 4;
            assert_eq!(delay(attempts, 0), base - range);
            assert_eq!(delay(attempts, range as u64), base);
            assert_eq!(delay(attempts, 2 * range as u64), base + range);
        }

        // At the cap, jitter may shorten the delay but never lengthen it.
        assert_eq!(delay(20, 0), 45000);
        assert_eq!(delay(20, 30000), 60000);
        assert_eq!(delay(u32::MAX, u64::MAX), 60000);

        let zero = NotificationServiceConfig {
            initial_retry_delay_ms: 0,
            ..config.clone()
        };
        assert_eq!(
            NotificationService::jittered_retry_delay(&zero, 3, u64::MAX),
            Duration::ZERO
        );
    }
}
