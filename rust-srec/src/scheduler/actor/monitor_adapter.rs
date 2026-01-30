//! Monitor adapter for actor integration.
//!
//! This module provides the interface between actors and the monitoring infrastructure.
//! It defines traits that abstract the monitoring operations, allowing actors to
//! perform status checks without direct coupling to the monitor implementation.

use std::sync::Arc;

use async_trait::async_trait;

use crate::monitor::{FilterReason, LiveStatus};
use crate::streamer::StreamerMetadata;

use super::messages::{BatchDetectionResult, CheckResult};

/// Trait for individual streamer status checking.
///
/// This trait abstracts the status checking operation, allowing
/// StreamerActors to perform checks without direct coupling to
/// the StreamMonitor implementation.
#[async_trait]
pub trait StatusChecker: Send + Sync + 'static {
    /// Check the status of a streamer.
    ///
    /// Returns a tuple of `(CheckResult, LiveStatus)` with the detected state.
    /// The `LiveStatus` can be used for `process_status()` if the caller decides to emit events.
    async fn check_status(
        &self,
        streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError>;

    /// Process a status result and update the streamer state.
    ///
    /// This handles state transitions, event emission, and persistence.
    async fn process_status(
        &self,
        streamer: &StreamerMetadata,
        status: LiveStatus,
    ) -> Result<(), CheckError>;

    /// Handle an error during status checking.
    async fn handle_error(
        &self,
        streamer: &StreamerMetadata,
        error: &str,
    ) -> Result<(), CheckError>;

    /// Set the streamer to temporarily disabled due to circuit breaker block.
    ///
    /// This sets the state to `TemporalDisabled` and stores the retry time
    /// without incrementing the error count (since it's an infrastructure issue,
    /// not a streamer issue).
    ///
    /// # Arguments
    /// * `streamer` - The streamer metadata
    /// * `retry_after_secs` - Seconds until the circuit breaker allows retries
    async fn set_circuit_breaker_blocked(
        &self,
        streamer: &StreamerMetadata,
        retry_after_secs: u64,
    ) -> Result<(), CheckError>;
}

/// Trait for batch status checking.
///
/// This trait abstracts batch detection operations, allowing
/// PlatformActors to perform batch checks without direct coupling
/// to the BatchDetector implementation.
#[async_trait]
pub trait BatchChecker: Send + Sync + 'static {
    /// Perform a batch status check for multiple streamers.
    ///
    /// Returns results for each streamer in the batch.
    async fn batch_check(
        &self,
        platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<Vec<BatchDetectionResult>, CheckError>;
}

/// Error type for check operations.
#[derive(Debug, Clone)]
pub struct CheckError {
    /// Error message.
    pub message: String,
    /// Whether this error is transient (can be retried).
    pub transient: bool,
}

impl CheckError {
    /// Create a transient error (can be retried).
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: true,
        }
    }

    /// Create a permanent error (should not be retried).
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: false,
        }
    }
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CheckError {}

impl From<crate::Error> for CheckError {
    fn from(err: crate::Error) -> Self {
        CheckError::transient(err.to_string())
    }
}

/// Real implementation of StatusChecker using StreamMonitor.
///
/// This adapter connects StreamerActors to the actual monitoring infrastructure.
pub struct MonitorStatusChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>,
}

impl<SR, FR, SSR, CR> MonitorStatusChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    /// Create a new MonitorStatusChecker.
    pub fn new(monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl<SR, FR, SSR, CR> StatusChecker for MonitorStatusChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    async fn check_status(
        &self,
        streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        let status = self.monitor.check_streamer(streamer).await?;

        // Convert LiveStatus to CheckResult
        let result = convert_live_status_to_check_result(&status);

        // Return both the result and the status.
        Ok((result, status))
    }

    async fn process_status(
        &self,
        streamer: &StreamerMetadata,
        status: LiveStatus,
    ) -> Result<(), CheckError> {
        self.monitor.process_status(streamer, status).await?;
        Ok(())
    }

    async fn handle_error(
        &self,
        streamer: &StreamerMetadata,
        error: &str,
    ) -> Result<(), CheckError> {
        self.monitor.handle_error(streamer, error).await?;
        Ok(())
    }

    async fn set_circuit_breaker_blocked(
        &self,
        streamer: &StreamerMetadata,
        retry_after_secs: u64,
    ) -> Result<(), CheckError> {
        self.monitor
            .set_circuit_breaker_blocked(streamer, retry_after_secs)
            .await?;
        Ok(())
    }
}

/// Real implementation of BatchChecker using StreamMonitor.
///
/// This adapter connects PlatformActors to the actual batch detection infrastructure.
pub struct MonitorBatchChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>,
}

impl<SR, FR, SSR, CR> MonitorBatchChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    /// Create a new MonitorBatchChecker.
    pub fn new(monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>) -> Self {
        Self { monitor }
    }
}

#[async_trait]
impl<SR, FR, SSR, CR> BatchChecker for MonitorBatchChecker<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    async fn batch_check(
        &self,
        platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<Vec<BatchDetectionResult>, CheckError> {
        let batch_result = self
            .monitor
            .batch_check(platform_id, streamers.clone())
            .await?;

        // Convert BatchResult to Vec<BatchDetectionResult>
        let mut results = Vec::new();

        for (streamer_id, status) in batch_result.results {
            let check_result = convert_live_status_to_check_result(&status);

            results.push(BatchDetectionResult {
                streamer_id,
                result: check_result,
                status,
            });
        }

        // Handle failures
        for failure in batch_result.failures {
            results.push(BatchDetectionResult {
                streamer_id: failure.streamer_id,
                result: CheckResult::failure(failure.error),
                // NOTE: This status is ignored by StreamerActor for error results; it will call
                // StatusChecker::handle_error() instead of process_status().
                status: crate::monitor::LiveStatus::Offline,
            });
        }

        Ok(results)
    }
}

/// Convert a LiveStatus to a CheckResult.
fn convert_live_status_to_check_result(status: &LiveStatus) -> CheckResult {
    use crate::domain::StreamerState;

    match status {
        LiveStatus::Live {
            title,
            next_check_hint,
            ..
        } => CheckResult {
            state: StreamerState::Live,
            stream_url: None,
            title: Some(title.clone()),
            checked_at: chrono::Utc::now(),
            error: None,
            next_check_hint: *next_check_hint,
        },
        LiveStatus::Offline => CheckResult::success(StreamerState::NotLive),
        LiveStatus::Filtered { reason, title, .. } => {
            let next_check_hint = match reason {
                FilterReason::OutOfSchedule { next_available } => *next_available,
                _ => None,
            };
            CheckResult {
                state: StreamerState::OutOfSchedule,
                stream_url: None,
                title: Some(title.clone()),
                checked_at: chrono::Utc::now(),
                error: None,
                next_check_hint,
            }
        }
        LiveStatus::NotFound => CheckResult {
            state: StreamerState::NotFound,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Streamer not found".to_string()),
            next_check_hint: None,
        },
        LiveStatus::Banned => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Streamer is banned".to_string()),
            next_check_hint: None,
        },
        LiveStatus::AgeRestricted => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is age-restricted".to_string()),
            next_check_hint: None,
        },
        LiveStatus::RegionLocked => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is region-locked".to_string()),
            next_check_hint: None,
        },
        LiveStatus::Private => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is private".to_string()),
            next_check_hint: None,
        },
        LiveStatus::UnsupportedPlatform => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Unsupported platform".to_string()),
            next_check_hint: None,
        },
    }
}

/// No-op implementation of StatusChecker for testing.
///
/// This implementation simulates checks without actually performing them.
#[derive(Clone)]
pub struct NoOpStatusChecker;

#[async_trait]
impl StatusChecker for NoOpStatusChecker {
    async fn check_status(
        &self,
        _streamer: &StreamerMetadata,
    ) -> Result<(CheckResult, LiveStatus), CheckError> {
        // Simulate a small delay
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok((
            CheckResult::success(crate::domain::StreamerState::NotLive),
            LiveStatus::Offline,
        ))
    }

    async fn process_status(
        &self,
        _streamer: &StreamerMetadata,
        _status: LiveStatus,
    ) -> Result<(), CheckError> {
        Ok(())
    }

    async fn handle_error(
        &self,
        _streamer: &StreamerMetadata,
        _error: &str,
    ) -> Result<(), CheckError> {
        Ok(())
    }

    async fn set_circuit_breaker_blocked(
        &self,
        _streamer: &StreamerMetadata,
        _retry_after_secs: u64,
    ) -> Result<(), CheckError> {
        Ok(())
    }
}

/// No-op implementation of BatchChecker for testing.
///
/// This implementation simulates batch checks without actually performing them.
#[derive(Clone)]
pub struct NoOpBatchChecker;

#[async_trait]
impl BatchChecker for NoOpBatchChecker {
    async fn batch_check(
        &self,
        _platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<Vec<BatchDetectionResult>, CheckError> {
        // Simulate a small delay
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Return offline status for all streamers
        let results = streamers
            .into_iter()
            .map(|s| BatchDetectionResult {
                streamer_id: s.id,
                result: CheckResult::success(crate::domain::StreamerState::NotLive),
                status: LiveStatus::Offline,
            })
            .collect();

        Ok(results)
    }
}
