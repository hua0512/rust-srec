//! Monitor adapter for actor integration.
//!
//! This module provides the interface between actors and the monitoring infrastructure.
//! It defines traits that abstract the monitoring operations, allowing actors to
//! perform status checks without direct coupling to the monitor implementation.

use std::sync::Arc;

use async_trait::async_trait;

use crate::monitor::LiveStatus;
use crate::streamer::StreamerMetadata;

use super::messages::{BatchDetectionResult, CheckResult};

/// Factory trait for creating status checkers.
///
/// This allows the Supervisor to create appropriate checker instances
/// without being coupled to specific implementations. The factory pattern
/// enables dependency injection of real or mock checkers.
pub trait StatusCheckerFactory: Send + Sync + 'static {
    /// Create a StatusChecker for individual streamer checks.
    fn create_status_checker(&self) -> Arc<dyn StatusChecker>;

    /// Create a BatchChecker for platform batch checks.
    fn create_batch_checker(&self) -> Arc<dyn BatchChecker>;
}

/// Context for status checks.
#[derive(Debug, Clone, Default)]
pub struct CheckContext {
    /// Consecutive offline count.
    pub offline_count: u32,
    /// Number of offline checks required to confirm offline state.
    pub offline_limit: u32,
    /// Whether the streamer was previously live.
    pub was_live: bool,
}

/// Trait for individual streamer status checking.
///
/// This trait abstracts the status checking operation, allowing
/// StreamerActors to perform checks without direct coupling to
/// the StreamMonitor implementation.
#[async_trait]
pub trait StatusChecker: Send + Sync + 'static {
    /// Check the status of a streamer.
    ///
    /// Returns a `CheckResult` with the detected state.
    async fn check_status(
        &self,
        streamer: &StreamerMetadata,
        context: &CheckContext,
    ) -> Result<CheckResult, CheckError>;

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

/// A wrapper type that implements `StatusChecker` by delegating to `Arc<dyn StatusChecker>`.
///
/// This allows using dynamic dispatch with the generic `StreamerActor<S>` type,
/// enabling the Supervisor to inject different checker implementations at runtime.
#[derive(Clone)]
pub struct DynStatusChecker {
    inner: Arc<dyn StatusChecker>,
}

impl DynStatusChecker {
    /// Create a new DynStatusChecker wrapping the given checker.
    pub fn new(checker: Arc<dyn StatusChecker>) -> Self {
        Self { inner: checker }
    }
}

#[async_trait]
impl StatusChecker for DynStatusChecker {
    async fn check_status(
        &self,
        streamer: &StreamerMetadata,
        context: &CheckContext,
    ) -> Result<CheckResult, CheckError> {
        self.inner.check_status(streamer, context).await
    }

    async fn process_status(
        &self,
        streamer: &StreamerMetadata,
        status: LiveStatus,
    ) -> Result<(), CheckError> {
        self.inner.process_status(streamer, status).await
    }

    async fn handle_error(
        &self,
        streamer: &StreamerMetadata,
        error: &str,
    ) -> Result<(), CheckError> {
        self.inner.handle_error(streamer, error).await
    }
}

/// A wrapper type that implements `BatchChecker` by delegating to `Arc<dyn BatchChecker>`.
///
/// This allows using dynamic dispatch with the generic `PlatformActor<B>` type,
/// enabling the Supervisor to inject different checker implementations at runtime.
#[derive(Clone)]
pub struct DynBatchChecker {
    inner: Arc<dyn BatchChecker>,
}

impl DynBatchChecker {
    /// Create a new DynBatchChecker wrapping the given checker.
    pub fn new(checker: Arc<dyn BatchChecker>) -> Self {
        Self { inner: checker }
    }
}

#[async_trait]
impl BatchChecker for DynBatchChecker {
    async fn batch_check(
        &self,
        platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<Vec<BatchDetectionResult>, CheckError> {
        self.inner.batch_check(platform_id, streamers).await
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
        context: &CheckContext,
    ) -> Result<CheckResult, CheckError> {
        let status = self.monitor.check_streamer(streamer).await?;

        // Convert LiveStatus to CheckResult
        let result = convert_live_status_to_check_result(&status);

        // Apply hysteresis for offline detection
        // If status is offline and we haven't reached the offline limit,
        // we skip processing (which prevents ending the session).
        let should_process = if matches!(status, crate::monitor::LiveStatus::Offline) {
            // Only skip processing if we were previously live (hysteresis applies)
            // and we haven't reached the limit yet.
            // Note: context.offline_count is the count BEFORE this check.
            // We use (count + 1) to include the current check in the count.
            // So if count=0 and limit=3:
            // Check 1: detected offline. count=0. (0+1)<3=true. Skip. (count becomes 1)
            // Check 2: detected offline. count=1. (1+1)<3=true. Skip. (count becomes 2)
            // Check 3: detected offline. count=2. (2+1)<3=false. Process!
            if context.was_live && context.offline_count + 1 < context.offline_limit {
                false
            } else {
                true
            }
        } else {
            // Always process other statuses (Live, Filtered, Errors)
            true
        };

        if should_process {
            // Process the status (update state, emit events, etc.)
            self.monitor.process_status(streamer, status).await?;
        } else {
            tracing::debug!(
                "Skipping status processing for {} (offline hysteresis: {}/{})",
                streamer.id,
                context.offline_count,
                context.offline_limit
            );
        }

        Ok(result)
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

            // Find the streamer metadata to process status
            if let Some(streamer) = streamers.iter().find(|s| s.id == streamer_id) {
                // Process the status for this streamer
                if let Err(e) = self.monitor.process_status(streamer, status).await {
                    tracing::warn!("Failed to process status for {}: {}", streamer_id, e);
                }
            }

            results.push(BatchDetectionResult {
                streamer_id,
                result: check_result,
            });
        }

        // Handle failures
        for failure in batch_result.failures {
            if let Some(streamer) = streamers.iter().find(|s| s.id == failure.streamer_id) {
                if let Err(e) = self.monitor.handle_error(streamer, &failure.error).await {
                    tracing::warn!("Failed to handle error for {}: {}", failure.streamer_id, e);
                }
            }

            results.push(BatchDetectionResult {
                streamer_id: failure.streamer_id,
                result: CheckResult::failure(failure.error),
            });
        }

        Ok(results)
    }
}

/// Factory that creates real checkers connected to StreamMonitor.
///
/// This factory creates `MonitorStatusChecker` and `MonitorBatchChecker` instances
/// that connect to the actual monitoring infrastructure for real status detection.
pub struct MonitorCheckerFactory<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>,
}

impl<SR, FR, SSR, CR> MonitorCheckerFactory<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    /// Create a new MonitorCheckerFactory with the given StreamMonitor.
    pub fn new(monitor: Arc<crate::monitor::StreamMonitor<SR, FR, SSR, CR>>) -> Self {
        Self { monitor }
    }
}

impl<SR, FR, SSR, CR> StatusCheckerFactory for MonitorCheckerFactory<SR, FR, SSR, CR>
where
    SR: crate::database::repositories::StreamerRepository + Send + Sync + 'static,
    FR: crate::database::repositories::FilterRepository + Send + Sync + 'static,
    SSR: crate::database::repositories::SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
{
    fn create_status_checker(&self) -> Arc<dyn StatusChecker> {
        Arc::new(MonitorStatusChecker::new(self.monitor.clone()))
    }

    fn create_batch_checker(&self) -> Arc<dyn BatchChecker> {
        Arc::new(MonitorBatchChecker::new(self.monitor.clone()))
    }
}

/// Convert a LiveStatus to a CheckResult.
fn convert_live_status_to_check_result(status: &LiveStatus) -> CheckResult {
    use crate::domain::StreamerState;

    match status {
        LiveStatus::Live { title, .. } => CheckResult {
            state: StreamerState::Live,
            stream_url: None,
            title: Some(title.clone()),
            checked_at: chrono::Utc::now(),
            error: None,
        },
        LiveStatus::Offline => CheckResult::success(StreamerState::NotLive),
        LiveStatus::Filtered { .. } => CheckResult::success(StreamerState::OutOfSchedule),
        LiveStatus::NotFound => CheckResult {
            state: StreamerState::NotFound,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Streamer not found".to_string()),
        },
        LiveStatus::Banned => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Streamer is banned".to_string()),
        },
        LiveStatus::AgeRestricted => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is age-restricted".to_string()),
        },
        LiveStatus::RegionLocked => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is region-locked".to_string()),
        },
        LiveStatus::Private => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Content is private".to_string()),
        },
        LiveStatus::UnsupportedPlatform => CheckResult {
            state: StreamerState::FatalError,
            stream_url: None,
            title: None,
            checked_at: chrono::Utc::now(),
            error: Some("Unsupported platform".to_string()),
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
        _context: &CheckContext,
    ) -> Result<CheckResult, CheckError> {
        // Simulate a small delay
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        Ok(CheckResult::success(crate::domain::StreamerState::NotLive))
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
            })
            .collect();

        Ok(results)
    }
}

/// Factory that creates NoOp checkers for testing.
///
/// This factory creates `NoOpStatusChecker` and `NoOpBatchChecker` instances,
/// which simulate checks without actually performing them. Useful for unit tests
/// and development scenarios where real monitoring is not needed.
#[derive(Clone, Default)]
pub struct NoOpCheckerFactory;

impl NoOpCheckerFactory {
    /// Create a new NoOpCheckerFactory.
    pub fn new() -> Self {
        Self
    }
}

impl StatusCheckerFactory for NoOpCheckerFactory {
    fn create_status_checker(&self) -> Arc<dyn StatusChecker> {
        Arc::new(NoOpStatusChecker)
    }

    fn create_batch_checker(&self) -> Arc<dyn BatchChecker> {
        Arc::new(NoOpBatchChecker)
    }
}
