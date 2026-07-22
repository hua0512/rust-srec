//! Centralized database retention and storage maintenance.
//!
//! Lightweight retention runs on startup and at a fixed cadence. Operations
//! that can block readers, such as vacuuming and WAL truncation, remain gated
//! by the configured maintenance window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveTime, Utc};
use sysinfo::Disks;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::database::models::{DagExecutionStatus, JobStatus, RetentionDays};
use crate::database::retry::retry_on_sqlite_busy;
use crate::database::{DbPool, WritePool};
use crate::{Error, Result};

#[cfg(test)]
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;
const AUTO_VACUUM_NONE: i64 = 0;
const AUTO_VACUUM_FULL: i64 = 1;
const AUTO_VACUUM_INCREMENTAL: i64 = 2;

/// Runtime settings for database maintenance.
#[derive(Debug, Clone)]
pub struct MaintenanceConfig {
    /// Interval between lightweight retention sweeps.
    pub sweep_interval: Duration,
    /// Maximum number of parent rows removed by one statement.
    pub batch_size: u32,
    /// Maximum number of delete statements run per task during one sweep.
    pub max_batches_per_task: u32,
    /// Start of the window for potentially blocking maintenance.
    pub window_start: NaiveTime,
    /// End of the window for potentially blocking maintenance.
    pub window_end: NaiveTime,
    /// Minimum reusable database space required before vacuuming.
    pub vacuum_threshold_bytes: i64,
    /// Maximum active downloads allowed while vacuuming.
    pub max_active_downloads_for_vacuum: i32,
    /// Retention for persisted notification dead letters.
    pub dead_letter_retention: RetentionDays,
    /// Retention for delivered monitor outbox rows.
    pub monitor_outbox_delivered_retention: RetentionDays,
    /// Grace period before ended zero-byte sessions are deleted.
    pub empty_session_retention: Duration,
    /// Minimum interval between vacuum attempts.
    pub vacuum_interval: Duration,
    /// Minimum interval between query-planner optimization runs.
    pub optimize_interval: Duration,
    /// Minimum interval between WAL truncation checkpoints.
    pub wal_checkpoint_interval: Duration,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            sweep_interval: Duration::from_secs(30 * 60),
            batch_size: 500,
            max_batches_per_task: 20,
            window_start: NaiveTime::from_hms_opt(2, 0, 0).unwrap_or(NaiveTime::MIN),
            window_end: NaiveTime::from_hms_opt(5, 0, 0).unwrap_or(NaiveTime::MIN),
            vacuum_threshold_bytes: 100 * 1024 * 1024,
            max_active_downloads_for_vacuum: 0,
            dead_letter_retention: RetentionDays::try_from(7).unwrap_or(RetentionDays::Forever),
            monitor_outbox_delivered_retention: RetentionDays::try_from(1)
                .unwrap_or(RetentionDays::Forever),
            empty_session_retention: Duration::from_secs(5 * 60),
            vacuum_interval: Duration::from_secs(24 * 60 * 60),
            optimize_interval: Duration::from_secs(7 * 24 * 60 * 60),
            wal_checkpoint_interval: Duration::from_secs(60 * 60),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MaintenancePolicy {
    job_history_days: i32,
    notification_event_log_days: i32,
}

/// Failure recorded for one maintenance task.
#[derive(Debug)]
pub struct MaintenanceFailure {
    pub task: &'static str,
    pub error: Error,
}

/// Counts and failures produced by one lightweight maintenance sweep.
#[derive(Debug, Default)]
pub struct MaintenanceReport {
    pub jobs_deleted: u64,
    pub dags_deleted: u64,
    pub refresh_tokens_deleted: u64,
    pub dead_letters_deleted: u64,
    pub delivered_outbox_deleted: u64,
    pub notification_events_deleted: u64,
    pub empty_sessions_deleted: u64,
    pub cancelled: bool,
    pub failures: Vec<MaintenanceFailure>,
}

impl MaintenanceReport {
    fn record_failure(&mut self, task: &'static str, error: Error) {
        self.failures.push(MaintenanceFailure { task, error });
    }

    fn parse_retention(
        &mut self,
        task: &'static str,
        field: &'static str,
        days: i32,
    ) -> Option<RetentionDays> {
        match RetentionDays::try_from(days) {
            Ok(retention) => Some(retention),
            Err(error) => {
                self.record_failure(task, Error::config(format!("{field}: {error}")));
                None
            }
        }
    }

    fn log(&self, elapsed: Duration) {
        info!(
            elapsed_ms = elapsed.as_millis(),
            jobs_deleted = self.jobs_deleted,
            dags_deleted = self.dags_deleted,
            refresh_tokens_deleted = self.refresh_tokens_deleted,
            dead_letters_deleted = self.dead_letters_deleted,
            delivered_outbox_deleted = self.delivered_outbox_deleted,
            notification_events_deleted = self.notification_events_deleted,
            empty_sessions_deleted = self.empty_sessions_deleted,
            cancelled = self.cancelled,
            failures = self.failures.len(),
            "Database retention sweep completed"
        );

        for failure in &self.failures {
            warn!(
                task = failure.task,
                error = %failure.error,
                "Database maintenance task failed; other tasks continued"
            );
        }
    }
}

/// Executes bounded retention operations against SQLite.
struct MaintenanceRepository {
    pool: DbPool,
    write_pool: WritePool,
    batch_size: i64,
    max_batches_per_task: u32,
}

impl MaintenanceRepository {
    fn new(
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

    async fn load_policy(&self) -> Result<MaintenancePolicy> {
        let (job_days, event_days): (i32, i32) = sqlx::query_as(
            "SELECT job_history_retention_days, notification_event_log_retention_days \
             FROM global_config LIMIT 1",
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(MaintenancePolicy {
            job_history_days: job_days,
            notification_event_log_days: event_days,
        })
    }

    async fn prune_jobs_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        let mut total = 0;
        for batch_index in 0..self.max_batches_per_task {
            let result = tokio::select! {
                _ = cancellation.cancelled() => return Ok(total),
                result = retry_on_sqlite_busy("maintenance_prune_jobs", || async {
                    let result = sqlx::query(
                        "DELETE FROM job WHERE id IN (\
                        SELECT j.id FROM job j \
                        WHERE j.status IN (?, ?, ?) AND j.updated_at < ? \
                          AND NOT EXISTS (\
                            SELECT 1 FROM dag_step_execution s \
                            JOIN dag_execution d ON d.id = s.dag_id \
                            WHERE s.job_id = j.id \
                              AND (d.status NOT IN (?, ?, ?) OR d.updated_at >= ?)\
                          ) \
                        ORDER BY j.updated_at ASC LIMIT ?\
                        )",
                    )
                    .bind(JobStatus::Completed.as_str())
                    .bind(JobStatus::Failed.as_str())
                    .bind(JobStatus::Cancelled.as_str())
                    .bind(cutoff_ms)
                    .bind(DagExecutionStatus::Completed.as_str())
                    .bind(DagExecutionStatus::Failed.as_str())
                    .bind(DagExecutionStatus::Cancelled.as_str())
                    .bind(cutoff_ms)
                    .bind(self.batch_size)
                    .execute(&self.write_pool)
                    .await?;
                    Ok(result.rows_affected())
                }) => result?,
            };
            total += result;
            if result < self.batch_size as u64 {
                return Ok(total);
            }
            if batch_index + 1 == self.max_batches_per_task {
                debug!(total, "Job cleanup reached its per-sweep batch limit");
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(total)
    }

    async fn prune_dags_before(
        &self,
        cutoff_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        let mut total = 0;
        for batch_index in 0..self.max_batches_per_task {
            let result = tokio::select! {
                _ = cancellation.cancelled() => return Ok(total),
                result = retry_on_sqlite_busy("maintenance_prune_dags", || async {
                    let result = sqlx::query(
                        "DELETE FROM dag_execution WHERE id IN (\
                        SELECT d.id FROM dag_execution d \
                        WHERE d.status IN (?, ?, ?) AND d.updated_at < ? \
                          AND NOT EXISTS (\
                            SELECT 1 FROM dag_step_execution s \
                            JOIN job j ON j.id = s.job_id \
                            WHERE s.dag_id = d.id \
                              AND (j.status NOT IN (?, ?, ?) OR j.updated_at >= ?)\
                          ) \
                        ORDER BY d.updated_at ASC LIMIT ?\
                        )",
                    )
                    .bind(DagExecutionStatus::Completed.as_str())
                    .bind(DagExecutionStatus::Failed.as_str())
                    .bind(DagExecutionStatus::Cancelled.as_str())
                    .bind(cutoff_ms)
                    .bind(JobStatus::Completed.as_str())
                    .bind(JobStatus::Failed.as_str())
                    .bind(JobStatus::Cancelled.as_str())
                    .bind(cutoff_ms)
                    .bind(self.batch_size)
                    .execute(&self.write_pool)
                    .await?;
                    Ok(result.rows_affected())
                }) => result?,
            };
            total += result;
            if result < self.batch_size as u64 {
                return Ok(total);
            }
            if batch_index + 1 == self.max_batches_per_task {
                debug!(total, "DAG cleanup reached its per-sweep batch limit");
                break;
            }
            tokio::task::yield_now().await;
        }
        Ok(total)
    }

    async fn prune_expired_refresh_tokens(
        &self,
        now_ms: i64,
        cancellation: &CancellationToken,
    ) -> Result<u64> {
        self.prune_simple(
            "maintenance_prune_refresh_tokens",
            "DELETE FROM refresh_tokens WHERE id IN (\
                SELECT id FROM refresh_tokens WHERE expires_at < ? \
                ORDER BY expires_at ASC LIMIT ?\
            )",
            now_ms,
            cancellation,
        )
        .await
    }

    async fn prune_dead_letters_before(
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

    async fn prune_delivered_outbox_before(
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

    async fn prune_notification_events_before(
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

    async fn prune_empty_sessions_before(
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
                    Ok(result.rows_affected())
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

/// Coordinates lightweight retention and windowed SQLite maintenance.
pub struct MaintenanceScheduler {
    pool: DbPool,
    write_pool: WritePool,
    repository: MaintenanceRepository,
    config: MaintenanceConfig,
}

impl MaintenanceScheduler {
    /// Creates a scheduler for the supplied SQLite pools.
    pub fn new(pool: DbPool, write_pool: WritePool, config: MaintenanceConfig) -> Self {
        let repository = MaintenanceRepository::new(
            pool.clone(),
            write_pool.clone(),
            config.batch_size,
            config.max_batches_per_task,
        );
        Self {
            pool,
            write_pool,
            repository,
            config,
        }
    }

    /// Starts maintenance and returns its cancellation-aware background task.
    pub fn start(self: Arc<Self>, cancellation: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move { self.run_loop(cancellation).await })
    }

    /// Runs one lightweight retention sweep using current database configuration.
    pub async fn run_maintenance(&self) -> MaintenanceReport {
        let cancellation = CancellationToken::new();
        self.run_maintenance_with_cancellation(crate::database::time::now_ms(), &cancellation)
            .await
    }

    #[cfg(test)]
    async fn run_maintenance_at(&self, now_ms: i64) -> MaintenanceReport {
        let cancellation = CancellationToken::new();
        self.run_maintenance_with_cancellation(now_ms, &cancellation)
            .await
    }

    async fn run_maintenance_with_cancellation(
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

    async fn run_loop(&self, cancellation: CancellationToken) {
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

                    let now = Utc::now();
                    if self.is_in_maintenance_window(now.time()) {
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

    fn is_in_maintenance_window(&self, now: NaiveTime) -> bool {
        if self.config.window_start <= self.config.window_end {
            now >= self.config.window_start && now <= self.config.window_end
        } else {
            now >= self.config.window_start || now <= self.config.window_end
        }
    }

    async fn run_optimize(&self) -> Result<()> {
        sqlx::query("PRAGMA optimize").execute(&self.pool).await?;
        info!("Database query planner optimized");
        Ok(())
    }

    async fn run_wal_checkpoint(&self) -> Result<()> {
        let (busy, log_frames, checkpointed_frames): (i64, i64, i64) =
            sqlx::query_as("PRAGMA wal_checkpoint(TRUNCATE)")
                .fetch_one(&self.write_pool)
                .await?;
        info!(
            busy,
            log_frames, checkpointed_frames, "WAL checkpoint completed"
        );
        Ok(())
    }

    async fn run_vacuum_if_needed(&self) -> Result<bool> {
        let freeable = self.get_freeable_space().await?;
        if freeable < self.config.vacuum_threshold_bytes {
            debug!(
                freeable,
                "Reusable database space is below the vacuum threshold"
            );
            return Ok(false);
        }

        let active = self.get_active_download_count().await?;
        if active > self.config.max_active_downloads_for_vacuum {
            debug!(active, "Skipping vacuum while downloads are active");
            return Ok(false);
        }

        let mode: (i64,) = sqlx::query_as("PRAGMA auto_vacuum")
            .fetch_one(&self.pool)
            .await?;
        let before_size = self.get_database_size().await?;
        let started = Instant::now();

        match mode.0 {
            AUTO_VACUUM_NONE => self.convert_to_incremental_auto_vacuum(before_size).await?,
            AUTO_VACUUM_INCREMENTAL => {
                sqlx::query("PRAGMA incremental_vacuum")
                    .execute(&self.write_pool)
                    .await?;
            }
            AUTO_VACUUM_FULL => {
                debug!("Database already uses full auto-vacuum");
                return Ok(false);
            }
            unexpected => {
                return Err(Error::Database(format!(
                    "unexpected PRAGMA auto_vacuum value {unexpected}"
                )));
            }
        }

        let after_size = self.get_database_size().await?;
        info!(
            elapsed_ms = started.elapsed().as_millis(),
            reclaimed_bytes = before_size.saturating_sub(after_size),
            "Database vacuum completed"
        );
        Ok(true)
    }

    async fn convert_to_incremental_auto_vacuum(&self, database_size: i64) -> Result<()> {
        let path = self.database_path().await?;
        let required_space = u64::try_from(database_size)
            .unwrap_or_default()
            .saturating_add(64 * 1024 * 1024);
        let available_space = available_space_for_path(&path).ok_or_else(|| {
            Error::Database(format!(
                "could not determine available space for '{}'",
                path.display()
            ))
        })?;

        if available_space < required_space {
            return Err(Error::Database(format!(
                "insufficient space to convert '{}' to incremental auto-vacuum: \
                 need {required_space} bytes, have {available_space} bytes",
                path.display()
            )));
        }

        info!(
            path = %path.display(),
            "Converting existing database to incremental auto-vacuum"
        );
        let mut connection = self.write_pool.acquire().await?;
        sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
            .execute(&mut *connection)
            .await?;
        sqlx::query("VACUUM").execute(&mut *connection).await?;
        Ok(())
    }

    async fn database_path(&self) -> Result<PathBuf> {
        let rows: Vec<(i64, String, String)> = sqlx::query_as("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .find_map(|(_, name, path)| {
                (name == "main" && !path.is_empty()).then(|| PathBuf::from(path))
            })
            .ok_or_else(|| {
                Error::Database("SQLite main database has no filesystem path".to_string())
            })
    }

    async fn get_freeable_space(&self) -> Result<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT freelist_count * page_size FROM pragma_freelist_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }

    async fn get_active_download_count(&self) -> Result<i32> {
        let result: (i32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM job \
             WHERE job_type = 'DOWNLOAD' AND status IN ('PENDING', 'PROCESSING')",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }

    async fn get_database_size(&self) -> Result<i64> {
        let result: (i64,) = sqlx::query_as(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }
}

fn is_due(last: Option<DateTime<Utc>>, now: DateTime<Utc>, interval: Duration) -> bool {
    last.is_none_or(|last| {
        now.signed_duration_since(last)
            .to_std()
            .is_ok_and(|elapsed| elapsed >= interval)
    })
}

fn available_space_for_path(path: &Path) -> Option<u64> {
    let disks = Disks::new_with_refreshed_list();
    let path = path.to_string_lossy();
    disks
        .list()
        .iter()
        .filter_map(|disk| {
            let mount = disk.mount_point().to_string_lossy();
            path.starts_with(mount.as_ref())
                .then_some((mount.len(), disk.available_space()))
        })
        .max_by_key(|(mount_len, _)| *mount_len)
        .map(|(_, available)| available)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::{
        ChannelType, DagExecutionDbModel, DagStepExecutionDbModel, DagStepStatus, JobDbModel,
        JobExecutionLogDbModel, JobExecutionProgressDbModel, LiveSessionDbModel, MediaFileType,
        MediaOutputDbModel, NotificationChannelDbModel, NotificationDeadLetterDbModel,
        NotificationEventLogDbModel, RefreshTokenDbModel, SessionEventDbModel,
        SessionSegmentDbModel, StreamerDbModel,
    };
    use crate::database::repositories::{
        ConfigRepository as _, DagRepository as _, JobRepository as _, NotificationRepository as _,
        RefreshTokenRepository as _, SessionEventRepository as _, SessionRepository as _,
        SqlxConfigRepository, SqlxDagRepository, SqlxJobRepository, SqlxNotificationRepository,
        SqlxRefreshTokenRepository, SqlxSessionEventRepository, SqlxSessionRepository,
        SqlxStreamerRepository, StreamerRepository as _,
    };

    struct TestDatabase {
        pool: DbPool,
        config_repository: SqlxConfigRepository,
        dag_repository: SqlxDagRepository,
        job_repository: SqlxJobRepository,
        notification_repository: SqlxNotificationRepository,
        refresh_token_repository: SqlxRefreshTokenRepository,
        session_event_repository: SqlxSessionEventRepository,
        session_repository: SqlxSessionRepository,
        streamer_repository: SqlxStreamerRepository,
        scheduler: Arc<MaintenanceScheduler>,
        _directory: tempfile::TempDir,
    }

    async fn setup(batch_size: u32) -> TestDatabase {
        setup_with_config(MaintenanceConfig {
            batch_size,
            ..MaintenanceConfig::default()
        })
        .await
    }

    async fn setup_with_config(config: MaintenanceConfig) -> TestDatabase {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("maintenance.db");
        let url = format!(
            "sqlite:{}?mode=rwc",
            path.to_string_lossy().replace('\\', "/")
        );
        let pool = crate::database::init_pool_with_size(&url, 1)
            .await
            .expect("read pool");
        let write_pool = crate::database::init_write_pool(&url)
            .await
            .expect("write pool");
        crate::database::run_migrations(&pool)
            .await
            .expect("migrations");
        let scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            write_pool.clone(),
            config,
        ));
        TestDatabase {
            config_repository: SqlxConfigRepository::new(pool.clone(), write_pool.clone()),
            dag_repository: SqlxDagRepository::new(pool.clone(), write_pool.clone()),
            job_repository: SqlxJobRepository::new(pool.clone(), write_pool.clone()),
            notification_repository: SqlxNotificationRepository::new(
                pool.clone(),
                write_pool.clone(),
            ),
            refresh_token_repository: SqlxRefreshTokenRepository::new(
                pool.clone(),
                write_pool.clone(),
            ),
            session_event_repository: SqlxSessionEventRepository::new(
                pool.clone(),
                write_pool.clone(),
            ),
            session_repository: SqlxSessionRepository::new(pool.clone(), write_pool.clone()),
            streamer_repository: SqlxStreamerRepository::new(pool.clone(), write_pool),
            pool,
            scheduler,
            _directory: directory,
        }
    }

    async fn set_retention(database: &TestDatabase, job_days: i32, event_days: i32) {
        let mut config = database
            .config_repository
            .get_global_config()
            .await
            .expect("global config");
        config.job_history_retention_days = job_days;
        config.notification_event_log_retention_days = event_days;
        database
            .config_repository
            .update_global_config(&config)
            .await
            .expect("retention config");
    }

    async fn seed_streamer(database: &TestDatabase) {
        let mut streamer = StreamerDbModel::new(
            "Maintenance",
            "https://example.com/maintenance",
            "platform-twitch",
        );
        streamer.id = "maintenance-streamer".to_string();
        database
            .streamer_repository
            .create_streamer(&streamer)
            .await
            .expect("streamer");
    }

    async fn create_job(
        database: &TestDatabase,
        id: &str,
        status: JobStatus,
        updated_at: i64,
        dag_step_execution_id: Option<&str>,
    ) {
        let mut job = JobDbModel::new("PROCESS", "{}");
        job.id = id.to_string();
        job.status = status.as_str().to_string();
        job.created_at = updated_at;
        job.updated_at = updated_at;
        job.completed_at = status.is_terminal().then_some(updated_at);
        job.dag_step_execution_id = dag_step_execution_id.map(str::to_string);
        database.job_repository.create_job(&job).await.expect("job");
    }

    async fn insert_job(database: &TestDatabase, id: &str, status: JobStatus, updated_at: i64) {
        create_job(database, id, status, updated_at, None).await;
    }

    async fn insert_dag(
        database: &TestDatabase,
        id: &str,
        status: DagExecutionStatus,
        updated_at: i64,
    ) {
        let dag = DagExecutionDbModel {
            id: id.to_string(),
            dag_definition: "{}".to_string(),
            status: status.as_str().to_string(),
            streamer_id: None,
            session_id: None,
            segment_index: None,
            segment_source: None,
            created_at: updated_at,
            updated_at,
            completed_at: status.is_terminal().then_some(updated_at),
            error: None,
            total_steps: 1,
            completed_steps: i32::from(status == DagExecutionStatus::Completed),
            failed_steps: i32::from(status == DagExecutionStatus::Failed),
        };
        database.dag_repository.create_dag(&dag).await.expect("DAG");
    }

    struct DagJobFixture<'a> {
        dag_id: &'a str,
        dag_status: DagExecutionStatus,
        dag_updated_at: i64,
        step_id: &'a str,
        job_id: &'a str,
        job_status: JobStatus,
        job_updated_at: i64,
    }

    async fn insert_dag_job(database: &TestDatabase, fixture: DagJobFixture<'_>) {
        let DagJobFixture {
            dag_id,
            dag_status,
            dag_updated_at,
            step_id,
            job_id,
            job_status,
            job_updated_at,
        } = fixture;
        let initial_status = if dag_status.is_terminal() {
            DagExecutionStatus::Processing
        } else {
            dag_status
        };
        insert_dag(database, dag_id, initial_status, dag_updated_at).await;
        let step = DagStepExecutionDbModel {
            id: step_id.to_string(),
            dag_id: dag_id.to_string(),
            step_id: step_id.to_string(),
            job_id: None,
            status: DagStepStatus::Pending.as_str().to_string(),
            depends_on_step_ids: "[]".to_string(),
            outputs: None,
            created_at: dag_updated_at,
            updated_at: dag_updated_at,
        };
        database
            .dag_repository
            .create_step(&step)
            .await
            .expect("DAG step");
        create_job(database, job_id, job_status, job_updated_at, Some(step_id)).await;
        database
            .dag_repository
            .update_step_status_with_job(step_id, DagStepStatus::Completed.as_str(), job_id)
            .await
            .expect("DAG step job link");
        if initial_status != dag_status {
            database
                .dag_repository
                .update_dag_status(dag_id, dag_status.as_str(), None)
                .await
                .expect("DAG terminal status");
            sqlx::query("UPDATE dag_execution SET updated_at = ?, completed_at = ? WHERE id = ?")
                .bind(dag_updated_at)
                .bind(dag_updated_at)
                .bind(dag_id)
                .execute(&database.pool)
                .await
                .expect("historic DAG timestamps");
        }
    }

    #[test]
    fn zero_retention_means_forever() {
        assert_eq!(
            RetentionDays::try_from(0).ok(),
            Some(RetentionDays::Forever)
        );
    }

    #[test]
    fn negative_retention_is_rejected() {
        assert!(RetentionDays::try_from(-1).is_err());
    }

    #[tokio::test]
    async fn maintenance_window_supports_normal_and_overnight_ranges() {
        let mut config = MaintenanceConfig::default();
        let scheduler = MaintenanceScheduler::new(
            sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
            sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
            config.clone(),
        );
        assert!(
            scheduler.is_in_maintenance_window(
                NaiveTime::from_hms_opt(3, 0, 0).unwrap_or(NaiveTime::MIN)
            )
        );
        assert!(
            !scheduler.is_in_maintenance_window(
                NaiveTime::from_hms_opt(10, 0, 0).unwrap_or(NaiveTime::MIN)
            )
        );

        config.window_start = NaiveTime::from_hms_opt(22, 0, 0).unwrap_or(NaiveTime::MIN);
        config.window_end = NaiveTime::from_hms_opt(2, 0, 0).unwrap_or(NaiveTime::MIN);
        let scheduler = MaintenanceScheduler::new(
            sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
            sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("valid test URL"),
            config,
        );
        assert!(
            scheduler.is_in_maintenance_window(
                NaiveTime::from_hms_opt(23, 0, 0).unwrap_or(NaiveTime::MIN)
            )
        );
        assert!(
            scheduler.is_in_maintenance_window(
                NaiveTime::from_hms_opt(1, 0, 0).unwrap_or(NaiveTime::MIN)
            )
        );
    }

    #[tokio::test]
    async fn maintenance_prunes_bounded_history_and_preserves_live_data() {
        let database = setup(1).await;
        let now = 2_000_000_000_000_i64;
        let old = now - 31 * MILLIS_PER_DAY;
        let recent = now - 29 * MILLIS_PER_DAY;
        set_retention(&database, 30, 30).await;
        seed_streamer(&database).await;

        for (id, status) in [
            ("old-failed", JobStatus::Failed),
            ("old-cancelled", JobStatus::Cancelled),
        ] {
            insert_job(&database, id, status, old).await;
        }
        insert_job(&database, "old-pending", JobStatus::Pending, old).await;
        insert_dag_job(
            &database,
            DagJobFixture {
                dag_id: "old-dag",
                dag_status: DagExecutionStatus::Completed,
                dag_updated_at: old,
                step_id: "old-step",
                job_id: "old-completed",
                job_status: JobStatus::Completed,
                job_updated_at: old,
            },
        )
        .await;
        insert_dag_job(
            &database,
            DagJobFixture {
                dag_id: "protected-dag",
                dag_status: DagExecutionStatus::Completed,
                dag_updated_at: old,
                step_id: "protected-step",
                job_id: "recent-completed",
                job_status: JobStatus::Completed,
                job_updated_at: recent,
            },
        )
        .await;
        insert_dag_job(
            &database,
            DagJobFixture {
                dag_id: "active-dag",
                dag_status: DagExecutionStatus::Processing,
                dag_updated_at: old,
                step_id: "active-step",
                job_id: "active-dag-old-job",
                job_status: JobStatus::Completed,
                job_updated_at: old,
            },
        )
        .await;
        let mut log = JobExecutionLogDbModel::new("old-completed", "{}");
        log.id = "old-log".to_string();
        log.created_at = old;
        database
            .job_repository
            .add_execution_log(&log)
            .await
            .expect("job log");
        database
            .job_repository
            .upsert_job_execution_progress(&JobExecutionProgressDbModel {
                job_id: "old-completed".to_string(),
                kind: "percent".to_string(),
                progress: "{}".to_string(),
                updated_at: old,
            })
            .await
            .expect("job progress");

        let user_id = "default-admin-00000000-0000-0000-0000-000000000001";
        for (id, expires_at, revoked_at) in [
            ("expired-token", old, None),
            ("active-token", now + MILLIS_PER_DAY, None),
            (
                "revoked-token",
                now + MILLIS_PER_DAY,
                Some(now - MILLIS_PER_DAY),
            ),
        ] {
            database
                .refresh_token_repository
                .create(&RefreshTokenDbModel {
                    id: id.to_string(),
                    user_id: user_id.to_string(),
                    token_hash: format!("hash-{id}"),
                    expires_at,
                    created_at: old,
                    revoked_at,
                    device_info: None,
                })
                .await
                .expect("refresh token");
        }

        for (id, created_at) in [("old-event", old), ("recent-event", recent)] {
            database
                .notification_repository
                .add_event_log(&NotificationEventLogDbModel {
                    id: id.to_string(),
                    event_type: "test".to_string(),
                    priority: 5,
                    payload: "{}".to_string(),
                    streamer_id: None,
                    created_at,
                })
                .await
                .expect("notification event");
        }
        let mut channel = NotificationChannelDbModel::new("Channel", ChannelType::Webhook, "{}");
        channel.id = "channel".to_string();
        database
            .notification_repository
            .create_channel(&channel)
            .await
            .expect("notification channel");
        database
            .notification_repository
            .add_to_dead_letter(&NotificationDeadLetterDbModel {
                id: "old-dead-letter".to_string(),
                channel_id: channel.id,
                event_name: "test".to_string(),
                event_payload: "{}".to_string(),
                error_message: "failed".to_string(),
                retry_count: 0,
                first_attempt_at: old,
                last_attempt_at: old,
                created_at: old,
            })
            .await
            .expect("dead letter");
        sqlx::query(
            "INSERT INTO monitor_event_outbox (\
                streamer_id, event_type, payload, created_at, delivered_at\
             ) VALUES ('maintenance-streamer', 'test', '{}', ?, ?), \
                      ('maintenance-streamer', 'test', '{}', ?, NULL)",
        )
        .bind(old)
        .bind(old)
        .bind(old)
        .execute(&database.pool)
        .await
        .expect("monitor outbox");

        let old_empty_end = now - 10 * 60 * 1000;
        for (id, end_time, total_size) in [
            ("old-empty", old_empty_end, 0_i64),
            ("recent-empty", now, 0_i64),
            ("old-real", old, 1024_i64),
        ] {
            database
                .session_repository
                .create_session(&LiveSessionDbModel {
                    id: id.to_string(),
                    streamer_id: "maintenance-streamer".to_string(),
                    start_time: end_time - 1000,
                    end_time: Some(end_time),
                    titles: Some("[]".to_string()),
                    danmu_statistics_id: None,
                    total_size_bytes: total_size,
                })
                .await
                .expect("session");
        }
        database
            .session_event_repository
            .insert(&SessionEventDbModel {
                id: 0,
                session_id: "old-empty".to_string(),
                streamer_id: "maintenance-streamer".to_string(),
                kind: "session_ended".to_string(),
                occurred_at: old_empty_end,
                payload: Some("{}".to_string()),
            })
            .await
            .expect("session event");
        database
            .session_repository
            .create_session_segment(&SessionSegmentDbModel {
                id: "old-segment".to_string(),
                session_id: "old-empty".to_string(),
                segment_index: 0,
                file_path: "old.flv".to_string(),
                duration_secs: 1.0,
                size_bytes: 0,
                split_reason_code: None,
                split_reason_details_json: None,
                created_at: Some(old_empty_end - 1000),
                completed_at: Some(old_empty_end),
                persisted_at: old_empty_end,
            })
            .await
            .expect("session segment");
        let mut output = MediaOutputDbModel::new("old-empty", "old.flv", MediaFileType::Video, 0);
        output.id = "old-output".to_string();
        output.created_at = old_empty_end;
        database
            .session_repository
            .create_media_output(&output)
            .await
            .expect("media output");

        let report = database.scheduler.run_maintenance_at(now).await;
        assert!(report.failures.is_empty(), "{:#?}", report.failures);
        assert_eq!(report.jobs_deleted, 3);
        assert_eq!(report.dags_deleted, 1);
        assert_eq!(report.refresh_tokens_deleted, 1);
        assert_eq!(report.dead_letters_deleted, 1);
        assert_eq!(report.delivered_outbox_deleted, 1);
        assert_eq!(report.notification_events_deleted, 1);
        assert_eq!(report.empty_sessions_deleted, 1);

        let remaining_jobs: Vec<String> = sqlx::query_scalar("SELECT id FROM job ORDER BY id")
            .fetch_all(&database.pool)
            .await
            .expect("remaining jobs");
        assert_eq!(
            remaining_jobs,
            vec!["active-dag-old-job", "old-pending", "recent-completed"]
        );
        let old_children: i64 = sqlx::query_scalar(
            "SELECT \
                (SELECT COUNT(*) FROM job_execution_logs WHERE job_id = 'old-completed') + \
                (SELECT COUNT(*) FROM job_execution_progress WHERE job_id = 'old-completed')",
        )
        .fetch_one(&database.pool)
        .await
        .expect("old job children");
        assert_eq!(old_children, 0);
        let remaining_dags: Vec<String> =
            sqlx::query_scalar("SELECT id FROM dag_execution ORDER BY id")
                .fetch_all(&database.pool)
                .await
                .expect("remaining DAGs");
        assert_eq!(remaining_dags, vec!["active-dag", "protected-dag"]);
        let remaining_tokens: Vec<String> =
            sqlx::query_scalar("SELECT id FROM refresh_tokens ORDER BY id")
                .fetch_all(&database.pool)
                .await
                .expect("remaining refresh tokens");
        assert_eq!(remaining_tokens, vec!["active-token", "revoked-token"]);
        let undelivered: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM monitor_event_outbox WHERE delivered_at IS NULL",
        )
        .fetch_one(&database.pool)
        .await
        .expect("undelivered outbox");
        assert_eq!(undelivered, 1);
        let remaining_sessions: Vec<String> =
            sqlx::query_scalar("SELECT id FROM live_sessions ORDER BY id")
                .fetch_all(&database.pool)
                .await
                .expect("remaining sessions");
        assert_eq!(remaining_sessions, vec!["old-real", "recent-empty"]);
        let old_session_children: i64 = sqlx::query_scalar(
            "SELECT \
                (SELECT COUNT(*) FROM session_events WHERE session_id = 'old-empty') + \
                (SELECT COUNT(*) FROM session_segments WHERE session_id = 'old-empty') + \
                (SELECT COUNT(*) FROM media_outputs WHERE session_id = 'old-empty')",
        )
        .fetch_one(&database.pool)
        .await
        .expect("old session children");
        assert_eq!(old_session_children, 0);
        let foreign_key_errors: Vec<(String, i64, String, i64)> =
            sqlx::query_as("PRAGMA foreign_key_check")
                .fetch_all(&database.pool)
                .await
                .expect("foreign key check");
        assert!(foreign_key_errors.is_empty());
    }

    #[tokio::test]
    async fn empty_session_cleanup_preserves_materialized_children_and_active_work() {
        let database = setup(10).await;
        let now = 2_000_000_000_000_i64;
        let ended_at = now - 10 * 60 * 1000;
        seed_streamer(&database).await;

        for id in [
            "stale-media-size",
            "materialized-segment",
            "active-job-session",
            "active-dag-session",
        ] {
            database
                .session_repository
                .create_session(&LiveSessionDbModel {
                    id: id.to_string(),
                    streamer_id: "maintenance-streamer".to_string(),
                    start_time: ended_at - 1000,
                    end_time: Some(ended_at),
                    titles: Some("[]".to_string()),
                    danmu_statistics_id: None,
                    total_size_bytes: 0,
                })
                .await
                .expect("session");
        }

        let mut output = MediaOutputDbModel::new(
            "stale-media-size",
            "retained.flv",
            MediaFileType::Video,
            1024,
        );
        output.created_at = ended_at;
        database
            .session_repository
            .create_media_output(&output)
            .await
            .expect("media output");
        sqlx::query("UPDATE live_sessions SET total_size_bytes = 0 WHERE id = 'stale-media-size'")
            .execute(&database.pool)
            .await
            .expect("simulate stale session size");

        database
            .session_repository
            .create_session_segment(&SessionSegmentDbModel {
                id: "retained-segment".to_string(),
                session_id: "materialized-segment".to_string(),
                segment_index: 0,
                file_path: "segment.flv".to_string(),
                duration_secs: 1.0,
                size_bytes: 1024,
                split_reason_code: None,
                split_reason_details_json: None,
                created_at: Some(ended_at - 1000),
                completed_at: Some(ended_at),
                persisted_at: ended_at,
            })
            .await
            .expect("session segment");

        let mut job = JobDbModel::new("PROCESS", "{}");
        job.id = "active-session-job".to_string();
        job.session_id = Some("active-job-session".to_string());
        job.created_at = ended_at;
        job.updated_at = ended_at;
        database
            .job_repository
            .create_job(&job)
            .await
            .expect("active session job");

        database
            .dag_repository
            .create_dag(&DagExecutionDbModel {
                id: "active-session-dag".to_string(),
                dag_definition: "{}".to_string(),
                status: DagExecutionStatus::Processing.as_str().to_string(),
                streamer_id: Some("maintenance-streamer".to_string()),
                session_id: Some("active-dag-session".to_string()),
                segment_index: None,
                segment_source: None,
                created_at: ended_at,
                updated_at: ended_at,
                completed_at: None,
                error: None,
                total_steps: 1,
                completed_steps: 0,
                failed_steps: 0,
            })
            .await
            .expect("active session DAG");

        let report = database.scheduler.run_maintenance_at(now).await;
        assert!(report.failures.is_empty(), "{:#?}", report.failures);
        assert_eq!(report.empty_sessions_deleted, 0);
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM live_sessions WHERE id IN (\
                'stale-media-size', 'materialized-segment', \
                'active-job-session', 'active-dag-session'\
            )",
        )
        .fetch_one(&database.pool)
        .await
        .expect("protected sessions");
        assert_eq!(remaining, 4);
    }

    #[tokio::test]
    async fn cleanup_respects_per_task_batch_budget() {
        let database = setup_with_config(MaintenanceConfig {
            batch_size: 1,
            max_batches_per_task: 1,
            ..MaintenanceConfig::default()
        })
        .await;
        let now = 2_000_000_000_000_i64;
        let old = now - 31 * MILLIS_PER_DAY;
        set_retention(&database, 30, 30).await;
        for id in ["bounded-1", "bounded-2", "bounded-3"] {
            insert_job(&database, id, JobStatus::Failed, old).await;
        }

        for remaining_after_sweep in [2_i64, 1, 0] {
            let report = database.scheduler.run_maintenance_at(now).await;
            assert_eq!(report.jobs_deleted, 1);
            let remaining: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE id LIKE 'bounded-%'")
                    .fetch_one(&database.pool)
                    .await
                    .expect("remaining bounded jobs");
            assert_eq!(remaining, remaining_after_sweep);
        }
    }

    #[tokio::test]
    async fn cancelled_sweep_stops_before_deleting_rows() {
        let database = setup(1).await;
        let now = 2_000_000_000_000_i64;
        let old = now - 31 * MILLIS_PER_DAY;
        set_retention(&database, 30, 30).await;
        insert_job(&database, "cancelled-sweep-job", JobStatus::Failed, old).await;
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let report = database
            .scheduler
            .run_maintenance_with_cancellation(now, &cancellation)
            .await;
        assert!(report.cancelled);
        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE id = 'cancelled-sweep-job'")
                .fetch_one(&database.pool)
                .await
                .expect("cancelled sweep job");
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn zero_retention_preserves_pipeline_and_notification_history() {
        let database = setup(10).await;
        let now = 2_000_000_000_000_i64;
        let old = now - 365 * MILLIS_PER_DAY;
        set_retention(&database, 0, 0).await;
        insert_job(&database, "forever-job", JobStatus::Cancelled, old).await;
        insert_dag(&database, "forever-dag", DagExecutionStatus::Failed, old).await;
        database
            .notification_repository
            .add_event_log(&NotificationEventLogDbModel {
                id: "forever-event".to_string(),
                event_type: "test".to_string(),
                priority: 5,
                payload: "{}".to_string(),
                streamer_id: None,
                created_at: old,
            })
            .await
            .expect("notification event");

        let report = database.scheduler.run_maintenance_at(now).await;
        assert_eq!(report.jobs_deleted, 0);
        assert_eq!(report.dags_deleted, 0);
        assert_eq!(report.notification_events_deleted, 0);
        let count: i64 = sqlx::query_scalar(
            "SELECT \
                (SELECT COUNT(*) FROM job WHERE id = 'forever-job') + \
                (SELECT COUNT(*) FROM dag_execution WHERE id = 'forever-dag') + \
                (SELECT COUNT(*) FROM notification_event_log WHERE id = 'forever-event')",
        )
        .fetch_one(&database.pool)
        .await
        .expect("retained history");
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn invalid_retention_field_does_not_disable_other_policy() {
        let database = setup(10).await;
        let now = 2_000_000_000_000_i64;
        let old = now - 31 * MILLIS_PER_DAY;
        sqlx::query(
            "UPDATE global_config SET job_history_retention_days = -1, \
             notification_event_log_retention_days = 30",
        )
        .execute(&database.pool)
        .await
        .expect("corrupt one retention field");
        insert_job(
            &database,
            "preserved-invalid-policy",
            JobStatus::Failed,
            old,
        )
        .await;
        database
            .notification_repository
            .add_event_log(&NotificationEventLogDbModel {
                id: "pruned-valid-policy".to_string(),
                event_type: "test".to_string(),
                priority: 5,
                payload: "{}".to_string(),
                streamer_id: None,
                created_at: old,
            })
            .await
            .expect("notification event");

        let report = database.scheduler.run_maintenance_at(now).await;
        assert_eq!(report.jobs_deleted, 0);
        assert_eq!(report.notification_events_deleted, 1);
        assert!(
            report
                .failures
                .iter()
                .any(|failure| { failure.task == "job_history_retention_policy" })
        );
        let job_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job WHERE id = 'preserved-invalid-policy'")
                .fetch_one(&database.pool)
                .await
                .expect("preserved job");
        assert_eq!(job_count, 1);
    }

    #[tokio::test]
    async fn policy_failure_does_not_skip_token_cleanup() {
        let database = setup(10).await;
        let now = 2_000_000_000_000_i64;
        database
            .refresh_token_repository
            .create(&RefreshTokenDbModel {
                id: "expired-token".to_string(),
                user_id: "default-admin-00000000-0000-0000-0000-000000000001".to_string(),
                token_hash: "expired-hash".to_string(),
                expires_at: now - 1,
                created_at: now - MILLIS_PER_DAY,
                revoked_at: None,
                device_info: None,
            })
            .await
            .expect("expired token");
        sqlx::query("DROP TABLE global_config")
            .execute(&database.pool)
            .await
            .expect("drop policy source");

        let report = database.scheduler.run_maintenance_at(now).await;
        assert_eq!(report.refresh_tokens_deleted, 1);
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.task == "load_retention_policy")
        );
    }

    #[tokio::test]
    async fn new_database_enables_incremental_auto_vacuum() {
        let database = setup(10).await;
        let (mode,): (i64,) = sqlx::query_as("PRAGMA auto_vacuum")
            .fetch_one(&database.pool)
            .await
            .expect("auto-vacuum mode");
        assert_eq!(mode, AUTO_VACUUM_INCREMENTAL);
    }

    #[tokio::test]
    async fn scheduler_runs_lightweight_cleanup_immediately() {
        let database = setup(10).await;
        let now = crate::database::time::now_ms();
        database
            .refresh_token_repository
            .create(&RefreshTokenDbModel {
                id: "startup-expired".to_string(),
                user_id: "default-admin-00000000-0000-0000-0000-000000000001".to_string(),
                token_hash: "startup-expired-hash".to_string(),
                expires_at: now - 1,
                created_at: now - MILLIS_PER_DAY,
                revoked_at: None,
                device_info: None,
            })
            .await
            .expect("expired token");

        let cancellation = CancellationToken::new();
        let handle = database.scheduler.clone().start(cancellation.child_token());
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let count: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM refresh_tokens WHERE id = 'startup-expired'",
                )
                .fetch_one(&database.pool)
                .await
                .expect("token count");
                if count == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("startup maintenance sweep");
        cancellation.cancel();
        handle.await.expect("maintenance task");
    }
}
