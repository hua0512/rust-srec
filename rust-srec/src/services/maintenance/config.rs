//! Configuration and reporting types for database maintenance.

use std::time::Duration;

use jiff::civil::Time;
use tracing::{info, warn};

use crate::Error;
use crate::database::models::RetentionDays;

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
    pub window_start: Time,
    /// End of the window for potentially blocking maintenance.
    pub window_end: Time,
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
            window_start: Time::new(2, 0, 0, 0).unwrap_or(Time::MIN),
            window_end: Time::new(5, 0, 0, 0).unwrap_or(Time::MIN),
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
pub(super) struct MaintenancePolicy {
    pub(super) job_history_days: i32,
    pub(super) notification_event_log_days: i32,
    pub(super) output_days: i32,
    pub(super) output_delete_files: bool,
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
    pub outputs_deleted: u64,
    pub output_files_deleted: u64,
    pub jobs_deleted: u64,
    pub dags_deleted: u64,
    pub upload_records_deleted: u64,
    pub refresh_tokens_deleted: u64,
    pub dead_letters_deleted: u64,
    pub delivered_outbox_deleted: u64,
    pub notification_events_deleted: u64,
    pub empty_sessions_deleted: u64,
    pub cancelled: bool,
    pub failures: Vec<MaintenanceFailure>,
}

impl MaintenanceReport {
    pub(super) fn record_failure(&mut self, task: &'static str, error: Error) {
        self.failures.push(MaintenanceFailure { task, error });
    }

    pub(super) fn parse_retention(
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

    pub(super) fn log(&self, elapsed: Duration) {
        info!(
            elapsed_ms = elapsed.as_millis(),
            outputs_deleted = self.outputs_deleted,
            output_files_deleted = self.output_files_deleted,
            jobs_deleted = self.jobs_deleted,
            dags_deleted = self.dags_deleted,
            upload_records_deleted = self.upload_records_deleted,
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
