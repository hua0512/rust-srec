//! Job Purge Service for automatic cleanup of old completed/failed jobs.
//!
//! This service runs in the background and periodically purges jobs that have
//! exceeded the configured retention period.
//!

use chrono::{NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::{Duration, interval};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::Result;
use crate::database::repositories::JobRepository;

/// Configuration for job purging.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PurgeConfig {
    /// Number of days to retain completed/failed jobs.
    /// Set to 0 to retain all jobs indefinitely.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,

    /// Time window for purging (e.g., "02:00-05:00").
    /// If None, purging can run at any time.
    #[serde(default)]
    pub time_window: Option<String>,

    /// Batch size for deletion to avoid long-running transactions.
    #[serde(default = "default_batch_size")]
    pub batch_size: u32,

    /// Interval between purge checks in seconds.
    #[serde(default = "default_check_interval_secs")]
    pub check_interval_secs: u64,
}

fn default_retention_days() -> u32 {
    30
}

fn default_batch_size() -> u32 {
    100
}

fn default_check_interval_secs() -> u64 {
    3600 // 1 hour
}

impl Default for PurgeConfig {
    fn default() -> Self {
        Self {
            retention_days: default_retention_days(),
            time_window: Some("02:00-05:00".to_string()),
            batch_size: default_batch_size(),
            check_interval_secs: default_check_interval_secs(),
        }
    }
}

impl PurgeConfig {
    /// Create a new PurgeConfig with default values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the retention days.
    pub fn with_retention_days(mut self, days: u32) -> Self {
        self.retention_days = days;
        self
    }

    /// Set the time window.
    pub fn with_time_window(mut self, window: Option<String>) -> Self {
        self.time_window = window;
        self
    }

    /// Set the batch size.
    pub fn with_batch_size(mut self, size: u32) -> Self {
        self.batch_size = size;
        self
    }

    /// Set the check interval.
    pub fn with_check_interval_secs(mut self, secs: u64) -> Self {
        self.check_interval_secs = secs;
        self
    }
}

/// Parsed time window for purging.
#[derive(Debug, Clone)]
struct TimeWindow {
    start: NaiveTime,
    end: NaiveTime,
}

impl TimeWindow {
    /// Parse a time window string like "02:00-05:00".
    fn parse(window: &str) -> Option<Self> {
        let parts: Vec<&str> = window.split('-').collect();
        if parts.len() != 2 {
            return None;
        }

        let start = NaiveTime::parse_from_str(parts[0].trim(), "%H:%M").ok()?;
        let end = NaiveTime::parse_from_str(parts[1].trim(), "%H:%M").ok()?;

        Some(Self { start, end })
    }

    /// Check if the current time is within the window.
    fn is_within(&self, time: NaiveTime) -> bool {
        if self.start <= self.end {
            // Normal case: e.g., 02:00-05:00
            time >= self.start && time < self.end
        } else {
            // Overnight case: e.g., 23:00-02:00
            time >= self.start || time < self.end
        }
    }
}

/// Job Purge Service for automatic cleanup of old jobs.
pub struct JobPurgeService {
    config: PurgeConfig,
    job_repository: Arc<dyn JobRepository>,
    time_window: Option<TimeWindow>,
}

impl JobPurgeService {
    /// Create a new JobPurgeService.
    pub fn new(config: PurgeConfig, job_repository: Arc<dyn JobRepository>) -> Self {
        let time_window = config
            .time_window
            .as_ref()
            .and_then(|w| TimeWindow::parse(w));

        if config.time_window.is_some() && time_window.is_none() {
            warn!(
                "Invalid time window format: {:?}. Expected format: HH:MM-HH:MM",
                config.time_window
            );
        }

        Self {
            config,
            job_repository,
            time_window,
        }
    }

    /// Check if purging is currently allowed based on time window.
    pub fn is_purge_allowed(&self) -> bool {
        match &self.time_window {
            Some(window) => {
                let now = Utc::now().time();
                window.is_within(now)
            }
            None => true, // No time window restriction
        }
    }

    /// Run a single purge operation.
    /// Returns the number of jobs deleted.
    pub async fn run_purge(&self) -> Result<u64> {
        // Check if retention is disabled (0 = retain forever)
        if self.config.retention_days == 0 {
            debug!("Job purging disabled (retention_days = 0)");
            return Ok(0);
        }

        // Check time window
        if !self.is_purge_allowed() {
            debug!("Purge not allowed outside time window");
            return Ok(0);
        }

        let mut total_deleted: u64 = 0;

        // Delete in batches to avoid long-running transactions
        loop {
            let deleted = self
                .job_repository
                .cleanup_old_jobs(self.config.retention_days as i32)
                .await?;

            if deleted == 0 {
                break;
            }

            total_deleted += deleted as u64;

            // If we deleted less than batch_size, we're done
            if (deleted as u32) < self.config.batch_size {
                break;
            }

            // Small delay between batches to reduce database load
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Log the result
        if total_deleted > 0 {
            info!(
                "Purged {} old jobs (retention: {} days)",
                total_deleted, self.config.retention_days
            );
        } else {
            debug!("No jobs to purge");
        }

        Ok(total_deleted)
    }

    /// Start the background purge task.
    pub fn start_background_task(&self, cancellation_token: CancellationToken) {
        let config = self.config.clone();
        let job_repository = self.job_repository.clone();
        let time_window = self.time_window.clone();

        tokio::spawn(async move {
            let service = JobPurgeService {
                config: config.clone(),
                job_repository,
                time_window,
            };

            let mut check_interval = interval(Duration::from_secs(config.check_interval_secs));

            info!(
                "Job purge service started (retention: {} days, interval: {}s)",
                config.retention_days, config.check_interval_secs
            );

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        info!("Job purge service shutting down");
                        break;
                    }
                    _ = check_interval.tick() => {
                        match service.run_purge().await {
                            Ok(deleted) => {
                                if deleted > 0 {
                                    debug!("Purge cycle completed: {} jobs deleted", deleted);
                                }
                            }
                            Err(e) => {
                                error!("Purge cycle failed: {}", e);
                            }
                        }
                    }
                }
            }
        });
    }

    /// Get the current configuration.
    pub fn config(&self) -> &PurgeConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_purge_config_default() {
        let config = PurgeConfig::default();
        assert_eq!(config.retention_days, 30);
        assert_eq!(config.batch_size, 100);
        assert_eq!(config.check_interval_secs, 3600);
        assert!(config.time_window.is_some());
    }

    #[test]
    fn test_purge_config_builder() {
        let config = PurgeConfig::new()
            .with_retention_days(7)
            .with_batch_size(50)
            .with_time_window(Some("01:00-04:00".to_string()))
            .with_check_interval_secs(1800);

        assert_eq!(config.retention_days, 7);
        assert_eq!(config.batch_size, 50);
        assert_eq!(config.time_window, Some("01:00-04:00".to_string()));
        assert_eq!(config.check_interval_secs, 1800);
    }

    #[test]
    fn test_time_window_parse_valid() {
        let window = TimeWindow::parse("02:00-05:00").unwrap();
        assert_eq!(window.start, NaiveTime::from_hms_opt(2, 0, 0).unwrap());
        assert_eq!(window.end, NaiveTime::from_hms_opt(5, 0, 0).unwrap());
    }

    #[test]
    fn test_time_window_parse_invalid() {
        assert!(TimeWindow::parse("invalid").is_none());
        assert!(TimeWindow::parse("02:00").is_none());
        assert!(TimeWindow::parse("02:00-").is_none());
        assert!(TimeWindow::parse("-05:00").is_none());
        assert!(TimeWindow::parse("25:00-05:00").is_none());
    }

    #[test]
    fn test_time_window_is_within_normal() {
        let window = TimeWindow::parse("02:00-05:00").unwrap();

        // Within window
        assert!(window.is_within(NaiveTime::from_hms_opt(2, 0, 0).unwrap()));
        assert!(window.is_within(NaiveTime::from_hms_opt(3, 30, 0).unwrap()));
        assert!(window.is_within(NaiveTime::from_hms_opt(4, 59, 59).unwrap()));

        // Outside window
        assert!(!window.is_within(NaiveTime::from_hms_opt(1, 59, 59).unwrap()));
        assert!(!window.is_within(NaiveTime::from_hms_opt(5, 0, 0).unwrap()));
        assert!(!window.is_within(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
    }

    #[test]
    fn test_time_window_is_within_overnight() {
        let window = TimeWindow::parse("23:00-02:00").unwrap();

        // Within window (before midnight)
        assert!(window.is_within(NaiveTime::from_hms_opt(23, 0, 0).unwrap()));
        assert!(window.is_within(NaiveTime::from_hms_opt(23, 59, 59).unwrap()));

        // Within window (after midnight)
        assert!(window.is_within(NaiveTime::from_hms_opt(0, 0, 0).unwrap()));
        assert!(window.is_within(NaiveTime::from_hms_opt(1, 30, 0).unwrap()));

        // Outside window
        assert!(!window.is_within(NaiveTime::from_hms_opt(2, 0, 0).unwrap()));
        assert!(!window.is_within(NaiveTime::from_hms_opt(12, 0, 0).unwrap()));
        assert!(!window.is_within(NaiveTime::from_hms_opt(22, 59, 59).unwrap()));
    }

    #[test]
    fn test_retention_days_zero_disables_purge() {
        let config = PurgeConfig::new().with_retention_days(0);
        assert_eq!(config.retention_days, 0);
        // When retention_days is 0, purging should be disabled
    }
}
