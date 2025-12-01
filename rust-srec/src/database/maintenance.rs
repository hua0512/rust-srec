//! Database maintenance operations.
//!
//! This module provides automatic database maintenance including:
//! - Incremental vacuum scheduling
//! - Job history cleanup
//! - Dead letter queue cleanup

use crate::database::DbPool;
use chrono::{DateTime, NaiveTime, Utc};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;

/// Configuration for the maintenance scheduler.
#[derive(Debug, Clone)]
pub struct MaintenanceConfig {
    /// Interval between vacuum checks (default: 24 hours).
    pub vacuum_interval: Duration,
    /// Start of maintenance window (default: 02:00).
    pub window_start: NaiveTime,
    /// End of maintenance window (default: 05:00).
    pub window_end: NaiveTime,
    /// Minimum freeable space in bytes to trigger vacuum (default: 100MB).
    pub vacuum_threshold_bytes: i64,
    /// Maximum active downloads to allow vacuum (default: 0).
    pub max_active_downloads_for_vacuum: i32,
    /// Job retention period in days (default: 30).
    pub job_retention_days: i32,
    /// Dead letter retention period in days (default: 7).
    pub dead_letter_retention_days: i32,
}

impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            vacuum_interval: Duration::from_secs(24 * 60 * 60), // 24 hours
            window_start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            window_end: NaiveTime::from_hms_opt(5, 0, 0).unwrap(),
            vacuum_threshold_bytes: 100 * 1024 * 1024, // 100MB
            max_active_downloads_for_vacuum: 0,
            job_retention_days: 30,
            dead_letter_retention_days: 7,
        }
    }
}

/// Database maintenance scheduler.
pub struct MaintenanceScheduler {
    pool: DbPool,
    config: MaintenanceConfig,
    running: Arc<AtomicBool>,
    last_vacuum: Arc<Mutex<Option<DateTime<Utc>>>>,
}

impl MaintenanceScheduler {
    /// Create a new maintenance scheduler.
    pub fn new(pool: DbPool, config: MaintenanceConfig) -> Self {
        Self {
            pool,
            config,
            running: Arc::new(AtomicBool::new(false)),
            last_vacuum: Arc::new(Mutex::new(None)),
        }
    }

    /// Start the maintenance scheduler.
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let scheduler = self.clone();
        tokio::spawn(async move {
            scheduler.running.store(true, Ordering::SeqCst);
            scheduler.run_loop().await;
        })
    }

    /// Stop the maintenance scheduler.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    async fn run_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60)); // Check hourly

        while self.running.load(Ordering::SeqCst) {
            interval.tick().await;

            if self.is_in_maintenance_window() {
                // Run maintenance tasks
                if let Err(e) = self.run_maintenance().await {
                    tracing::error!("Maintenance error: {}", e);
                }
            }
        }
    }

    /// Check if current time is within the maintenance window.
    fn is_in_maintenance_window(&self) -> bool {
        let now = Utc::now().time();

        if self.config.window_start <= self.config.window_end {
            // Normal range (e.g., 02:00 - 05:00)
            now >= self.config.window_start && now <= self.config.window_end
        } else {
            // Overnight range (e.g., 22:00 - 02:00)
            now >= self.config.window_start || now <= self.config.window_end
        }
    }

    /// Run all maintenance tasks.
    pub async fn run_maintenance(&self) -> Result<(), crate::Error> {
        tracing::info!("Starting database maintenance");

        // Check if vacuum is needed
        if self.should_vacuum().await? {
            self.run_vacuum().await?;
        }

        // Cleanup old jobs
        let jobs_deleted = self.cleanup_old_jobs().await?;
        if jobs_deleted > 0 {
            tracing::info!("Cleaned up {} old jobs", jobs_deleted);
        }

        // Cleanup dead letters
        let dead_letters_deleted = self.cleanup_dead_letters().await?;
        if dead_letters_deleted > 0 {
            tracing::info!("Cleaned up {} dead letter entries", dead_letters_deleted);
        }

        tracing::info!("Database maintenance completed");
        Ok(())
    }

    /// Check if vacuum should be run.
    async fn should_vacuum(&self) -> Result<bool, crate::Error> {
        // Check time since last vacuum
        let last = self.last_vacuum.lock().await;
        if let Some(last_time) = *last {
            let elapsed = Utc::now().signed_duration_since(last_time);
            if elapsed < chrono::Duration::from_std(self.config.vacuum_interval).unwrap_or_default()
            {
                return Ok(false);
            }
        }
        drop(last);

        // Check freeable space
        let freeable = self.get_freeable_space().await?;
        if freeable < self.config.vacuum_threshold_bytes {
            tracing::debug!("Freeable space ({} bytes) below threshold", freeable);
            return Ok(false);
        }

        // Check active downloads
        let active = self.get_active_download_count().await?;
        if active > self.config.max_active_downloads_for_vacuum {
            tracing::debug!("Too many active downloads ({}) for vacuum", active);
            return Ok(false);
        }

        Ok(true)
    }

    /// Get the amount of freeable space in the database.
    async fn get_freeable_space(&self) -> Result<i64, crate::Error> {
        let result: (i64,) = sqlx::query_as(
            "SELECT freelist_count * page_size FROM pragma_freelist_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(result.0)
    }

    /// Get the count of active downloads.
    async fn get_active_download_count(&self) -> Result<i32, crate::Error> {
        let result: (i32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM job WHERE job_type = 'DOWNLOAD' AND status IN ('PENDING', 'PROCESSING')"
        )
            .fetch_one(&self.pool)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(result.0)
    }

    /// Run incremental vacuum.
    async fn run_vacuum(&self) -> Result<(), crate::Error> {
        let start = std::time::Instant::now();
        let before_size = self.get_database_size().await?;

        tracing::info!("Starting incremental vacuum");

        // Use incremental vacuum to avoid blocking
        sqlx::query("PRAGMA incremental_vacuum")
            .execute(&self.pool)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?;

        let after_size = self.get_database_size().await?;
        let duration = start.elapsed();
        let reclaimed = before_size - after_size;

        tracing::info!(
            "Vacuum completed in {:?}, reclaimed {} bytes",
            duration,
            reclaimed
        );

        // Update last vacuum time
        *self.last_vacuum.lock().await = Some(Utc::now());

        Ok(())
    }

    /// Get the current database size.
    async fn get_database_size(&self) -> Result<i64, crate::Error> {
        let result: (i64,) = sqlx::query_as(
            "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;
        Ok(result.0)
    }

    /// Cleanup old completed/failed jobs.
    pub async fn cleanup_old_jobs(&self) -> Result<i32, crate::Error> {
        let cutoff = Utc::now() - chrono::Duration::days(self.config.job_retention_days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

        // First delete execution logs for old jobs
        sqlx::query(
            "DELETE FROM job_execution_log WHERE job_id IN (
                SELECT id FROM job 
                WHERE status IN ('COMPLETED', 'FAILED') 
                AND updated_at < ?
            )",
        )
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;

        // Then delete the jobs
        let result = sqlx::query(
            "DELETE FROM job WHERE status IN ('COMPLETED', 'FAILED') AND updated_at < ?",
        )
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await
        .map_err(|e| crate::Error::Database(e.to_string()))?;

        Ok(result.rows_affected() as i32)
    }

    /// Cleanup old dead letter entries.
    pub async fn cleanup_dead_letters(&self) -> Result<i32, crate::Error> {
        let cutoff =
            Utc::now() - chrono::Duration::days(self.config.dead_letter_retention_days as i64);
        let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

        let result = sqlx::query("DELETE FROM notification_dead_letter WHERE created_at < ?")
            .bind(&cutoff_str)
            .execute(&self.pool)
            .await
            .map_err(|e| crate::Error::Database(e.to_string()))?;

        Ok(result.rows_affected() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maintenance_window_normal() {
        let config = MaintenanceConfig {
            window_start: NaiveTime::from_hms_opt(2, 0, 0).unwrap(),
            window_end: NaiveTime::from_hms_opt(5, 0, 0).unwrap(),
            ..Default::default()
        };

        // 03:00 should be in window
        let time_in = NaiveTime::from_hms_opt(3, 0, 0).unwrap();
        assert!(time_in >= config.window_start && time_in <= config.window_end);

        // 10:00 should be out of window
        let time_out = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        assert!(!(time_out >= config.window_start && time_out <= config.window_end));
    }

    #[test]
    fn test_default_config() {
        let config = MaintenanceConfig::default();
        assert_eq!(config.job_retention_days, 30);
        assert_eq!(config.dead_letter_retention_days, 7);
        assert_eq!(config.vacuum_threshold_bytes, 100 * 1024 * 1024);
    }
}
