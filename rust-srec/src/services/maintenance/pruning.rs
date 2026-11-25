//! Bounded SQLite table retention and sweep scheduling.

use std::time::{Duration, Instant};

use jiff::Timestamp;
use jiff::civil::Time;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::MaintenanceScheduler;
use super::config::{MaintenancePolicy, MaintenanceReport};
use crate::Result;
use crate::database::retry::retry_on_sqlite_busy;
use crate::database::{DbPool, WritePool};

/// Executes bounded retention operations against SQLite.
pub(super) struct MaintenanceRepository {
    pool: DbPool,
    write_pool: WritePool,
    batch_size: i64,
    max_batches_per_task: u32,
}

impl MaintenanceRepository {
    pub(super) fn new(
        pool: DbPool,
        write_pool: WritePool,
        batch_size: u32,
        max_batches_per_task: u32,
    ) -> Self {
        Self {
            pool,
            write_pool,
            batch_size: i64::from(batch_size.max(1)),
            max_batches_per_task: max_batches_per_task.max(1),
        }
    }

    pub(super) async fn load_policy(&self) -> Result<MaintenancePolicy> {
        let (job_days, event_days, output_days, output_delete_files): (i32, i32, i32, bool) =
            sqlx::query_as(
                "SELECT job_history_retention_days, notification_event_log_retention_days, \
             output_retention_days, output_retention_delete_files \
             FROM global_config LIMIT 1",
            )
            .fetch_one(&self.pool)
            .await?;

        Ok(MaintenancePolicy {
            job_history_days: job_days,
            notification_event_log_days: event_days,
            output_days,
            output_delete_files,
        })
    }

    pub(super) async fn prune_jobs_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_jobs",
            "DELETE FROM job WHERE id IN (\
                SELECT j.id FROM job j \
                WHERE j.status IN ('COMPLETED', 'FAILED', 'CANCELLED') \
                  AND j.updated_at < ?1 AND j.retry_after IS NULL \
                  AND NOT EXISTS (\
                    SELECT 1 FROM dag_step_execution s \
                    JOIN dag_execution d ON d.id = s.dag_id \
                    WHERE s.job_id = j.id \
                      AND (d.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED') \
                           OR d.updated_at >= ?1)\
                  ) \
                ORDER BY j.updated_at ASC LIMIT ?2\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    pub(super) async fn prune_dags_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_dags",
            "DELETE FROM dag_execution WHERE id IN (\
                SELECT d.id FROM dag_execution d \
                WHERE d.status IN ('COMPLETED', 'FAILED', 'CANCELLED') AND d.updated_at < ?1 \
                  AND NOT EXISTS (\
                    SELECT 1 FROM dag_step_execution s \
                    JOIN job j ON j.id = s.job_id \
                    WHERE s.dag_id = d.id \
                      AND (j.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED') \
                           OR j.updated_at >= ?1 OR j.retry_after IS NOT NULL)\
                  ) \
                ORDER BY d.updated_at ASC LIMIT ?2\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    /// Bounds `upload_records`: rows outlive their job (`ON DELETE SET NULL`
    /// on `upload_records.job_id`), so `prune_jobs_before` never touches
    /// them. Shares the `job_history_retention_days` cutoff.
    pub(super) async fn prune_upload_records_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_upload_records",
            "DELETE FROM upload_records WHERE id IN (\
                SELECT id FROM upload_records WHERE created_at < ? \
                ORDER BY created_at ASC LIMIT ?\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    pub(super) async fn prune_expired_refresh_tokens(
        &self,
        now_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        let deleted = self
            .prune_simple(
                "maintenance_prune_refresh_tokens",
                "DELETE FROM refresh_tokens WHERE id IN (\
                SELECT id FROM refresh_tokens WHERE expires_at < ? \
                ORDER BY expires_at ASC LIMIT ?\
            )",
                now_ms,
                cancellation,
            )
            .await?;
        self.prune_simple(
            "maintenance_prune_auth_sessions",
            "DELETE FROM auth_sessions WHERE id IN (SELECT id FROM auth_sessions WHERE expires_at < ? ORDER BY expires_at ASC LIMIT ?)",
            now_ms,
            cancellation,
        ).await?;
        Ok(deleted)
    }

    pub(super) async fn prune_dead_letters_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_dead_letters",
            "DELETE FROM notification_dead_letter WHERE id IN (\
                SELECT id FROM notification_dead_letter WHERE created_at < ? \
                ORDER BY created_at ASC LIMIT ?\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    pub(super) async fn prune_delivered_outbox_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_delivered_outbox",
            "DELETE FROM monitor_event_outbox WHERE id IN (\
                SELECT id FROM monitor_event_outbox \
                WHERE delivered_at IS NOT NULL AND delivered_at < ? \
                ORDER BY delivered_at ASC LIMIT ?\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    pub(super) async fn prune_notification_events_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_notification_events",
            "DELETE FROM notification_event_log WHERE id IN (\
                SELECT id FROM notification_event_log WHERE created_at < ? \
                ORDER BY created_at ASC LIMIT ?\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    pub(super) async fn prune_empty_sessions_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_empty_sessions",
            "DELETE FROM live_sessions WHERE id IN (\
                SELECT s.id FROM live_sessions s \
                WHERE s.total_size_bytes = 0 AND s.end_time IS NOT NULL AND s.end_time < ? \
                  AND NOT EXISTS (\
                    SELECT 1 FROM media_outputs m \
                    WHERE m.session_id = s.id AND m.size_bytes > 0\
                  ) \
                  AND NOT EXISTS (\
                    SELECT 1 FROM session_segments g \
                    WHERE g.session_id = s.id AND g.size_bytes > 0\
                  ) \
                  AND NOT EXISTS (\
                    SELECT 1 FROM job j \
                    WHERE j.session_id = s.id AND j.status IN ('PENDING', 'PROCESSING')\
                  ) \
                  AND NOT EXISTS (\
                    SELECT 1 FROM dag_execution d \
                    WHERE d.session_id = s.id AND d.status IN ('PENDING', 'PROCESSING')\
                  ) \
                ORDER BY s.end_time ASC LIMIT ?\
            )",
            cutoff_ms,
            cancellation,
        )
        .await
    }

    async fn prune_simple(
        &self,
        operation: &'static str,
        sql: &'static str,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        let mut total = 0;
        for batch_index in 0..self.max_batches_per_task {
            let result = tokio::select! {
                _ = cancellation.cancelled() => return Ok(total),
                result = retry_on_sqlite_busy(operation, || async {
                    let result = sqlx::query(sql)
                        .bind(cutoff_ms)
                        .bind(self.batch_size)
                        .execute(&self.write_pool)
                        .await?;
                    Ok::<_, crate::Error>(result.rows_affected())
                }) => result?,
            };
            total += result;
            if result < self.batch_size as u64 {
                return Ok(total);
            }
            if batch_index + 1 == self.max_batches_per_task {
                debug!(
                    operation,
                    total, "Cleanup task reached its per-sweep batch limit"
                );
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(total)
    }
}

impl MaintenanceScheduler {
    #[cfg(test)]
    pub(super) async fn run_maintenance_at(&self, now_ms: i64) -> MaintenanceReport {
        let cancellation = CancellationToken::new();
        self.run_maintenance_with_cancellation(now_ms, &cancellation)
            .await
    }

    pub(super) async fn run_maintenance_with_cancellation(
        &self,
        now_ms: i64,
        cancellation: &CancellationToken,
    ) -> MaintenanceReport {
        let mut report = MaintenanceReport::default();
        if cancellation.is_cancelled() {
            report.cancelled = true;
            return report;
        }

        let policy = match self.repository.load_policy().await {
            Ok(policy) => Some(policy),
            Err(error) => {
                report.record_failure("load_retention_policy", error);
                None
            }
        };
        if cancellation.is_cancelled() {
            report.cancelled = true;
            return report;
        }

        let job_retention = policy.and_then(|value| {
            report.parse_retention(
                "job_history_retention_policy",
                "job_history_retention_days",
                value.job_history_days,
            )
        });
        let notification_retention = policy.and_then(|value| {
            report.parse_retention(
                "notification_event_log_retention_policy",
                "notification_event_log_retention_days",
                value.notification_event_log_days,
            )
        });

        // Inspect output dependencies before pruning diagnostic job/DAG history.
        if let Some(policy) = policy
            && let Some(cutoff) = report
                .parse_retention(
                    "output_retention_policy",
                    "output_retention_days",
                    policy.output_days,
                )
                .and_then(|value| value.cutoff_ms(now_ms))
        {
            self.prune_outputs(
                cutoff,
                policy.output_delete_files,
                cancellation,
                &mut report,
            )
            .await;
        }

        if let Some(cutoff) = job_retention.and_then(|value| value.cutoff_ms(now_ms)) {
            match self
                .repository
                .prune_jobs_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.jobs_deleted = deleted,
                Err(error) => report.record_failure("prune_jobs", error),
            }
            match self
                .repository
                .prune_dags_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.dags_deleted = deleted,
                Err(error) => report.record_failure("prune_dags", error),
            }
            match self
                .repository
                .prune_upload_records_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.upload_records_deleted = deleted,
                Err(error) => report.record_failure("prune_upload_records", error),
            }
        }

        match self
            .repository
            .prune_expired_refresh_tokens(now_ms, cancellation)
            .await
        {
            Ok(deleted) => report.refresh_tokens_deleted = deleted,
            Err(error) => report.record_failure("prune_refresh_tokens", error),
        }

        if let Some(cutoff) = self.config.dead_letter_retention.cutoff_ms(now_ms) {
            match self
                .repository
                .prune_dead_letters_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.dead_letters_deleted = deleted,
                Err(error) => report.record_failure("prune_dead_letters", error),
            }
        }

        if let Some(cutoff) = self
            .config
            .monitor_outbox_delivered_retention
            .cutoff_ms(now_ms)
        {
            match self
                .repository
                .prune_delivered_outbox_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.delivered_outbox_deleted = deleted,
                Err(error) => report.record_failure("prune_delivered_outbox", error),
            }
        }

        if let Some(cutoff) = notification_retention.and_then(|value| value.cutoff_ms(now_ms)) {
            match self
                .repository
                .prune_notification_events_before(cutoff, cancellation)
                .await
            {
                Ok(deleted) => report.notification_events_deleted = deleted,
                Err(error) => report.record_failure("prune_notification_events", error),
            }
        }

        let empty_session_retention_ms =
            i64::try_from(self.config.empty_session_retention.as_millis()).unwrap_or(i64::MAX);
        match self
            .repository
            .prune_empty_sessions_before(
                now_ms.saturating_sub(empty_session_retention_ms),
                cancellation,
            )
            .await
        {
            Ok(deleted) => report.empty_sessions_deleted = deleted,
            Err(error) => report.record_failure("prune_empty_sessions", error),
        }

        report.cancelled = cancellation.is_cancelled();
        report
    }

    pub(super) async fn run_loop(&self, cancellation: CancellationToken) {
        let mut interval = tokio::time::interval(self.config.sweep_interval);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last_vacuum = None;
        let mut last_optimize = None;
        let mut last_wal_checkpoint = None;

        info!(
            interval_secs = self.config.sweep_interval.as_secs(),
            "Database maintenance scheduler started"
        );

        loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    debug!("Database maintenance scheduler shutting down");
                    return;
                }
                _ = interval.tick() => {
                    let started = Instant::now();
                    let report = self
                        .run_maintenance_with_cancellation(
                            crate::database::time::now_ms(),
                            &cancellation,
                        )
                        .await;
                    let cancelled = report.cancelled;
                    report.log(started.elapsed());
                    if cancelled || cancellation.is_cancelled() {
                        debug!("Database maintenance scheduler shutting down");
                        return;
                    }

                    let now = Timestamp::now();
                    if self.is_in_maintenance_window(jiff::tz::TimeZone::UTC.to_datetime(now).time()) {
                        if is_due(last_wal_checkpoint, now, self.config.wal_checkpoint_interval) {
                            match self.run_wal_checkpoint().await {
                                Ok(()) => last_wal_checkpoint = Some(now),
                                Err(error) => warn!(error = %error, "WAL checkpoint failed"),
                            }
                        }
                        if is_due(last_optimize, now, self.config.optimize_interval) {
                            match self.run_optimize().await {
                                Ok(()) => last_optimize = Some(now),
                                Err(error) => warn!(error = %error, "Database optimize failed"),
                            }
                        }
                        if is_due(last_vacuum, now, self.config.vacuum_interval) {
                            match self.run_vacuum_if_needed().await {
                                Ok(true) => last_vacuum = Some(now),
                                Ok(false) => {}
                                Err(error) => warn!(error = %error, "Database vacuum failed"),
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn is_in_maintenance_window(&self, now: Time) -> bool {
        if self.config.window_start <= self.config.window_end {
            now >= self.config.window_start && now <= self.config.window_end
        } else {
            now >= self.config.window_start || now <= self.config.window_end
        }
    }
}

fn is_due(last: Option<Timestamp>, now: Timestamp, interval: Duration) -> bool {
    last.is_none_or(|last| {
        Duration::try_from(now.duration_since(last)).is_ok_and(|elapsed| elapsed >= interval)
    })
}
