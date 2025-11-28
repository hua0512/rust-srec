//! Stream Monitor service implementation.
//!
//! The StreamMonitor coordinates live status detection, filter evaluation,
//! and state updates for streamers.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use crate::database::repositories::{FilterRepository, SessionRepository, StreamerRepository};
use crate::domain::filter::Filter;
use crate::domain::StreamerState;
use crate::streamer::{StreamerManager, StreamerMetadata};
use crate::Result;

use super::batch_detector::{BatchDetector, BatchResult};
use super::detector::{FilterReason, LiveStatus, StreamDetector};
use super::events::{FatalErrorType, MonitorEvent, MonitorEventBroadcaster};
use super::rate_limiter::{RateLimiterConfig, RateLimiterManager};

/// Configuration for the stream monitor.
#[derive(Debug, Clone)]
pub struct StreamMonitorConfig {
    /// Default rate limit (requests per second).
    pub default_rate_limit: f64,
    /// Platform-specific rate limits.
    pub platform_rate_limits: Vec<(String, f64)>,
    /// HTTP client timeout.
    pub request_timeout: Duration,
    /// Maximum concurrent requests.
    pub max_concurrent_requests: usize,
}

impl Default for StreamMonitorConfig {
    fn default() -> Self {
        Self {
            default_rate_limit: 1.0,
            platform_rate_limits: vec![
                ("twitch".to_string(), 2.0),
                ("youtube".to_string(), 1.0),
            ],
            request_timeout: Duration::from_secs(30),
            max_concurrent_requests: 10,
        }
    }
}

/// The Stream Monitor service.
pub struct StreamMonitor<
    SR: StreamerRepository + Send + Sync + 'static,
    FR: FilterRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
> {
    /// Streamer manager for state updates.
    streamer_manager: Arc<StreamerManager<SR>>,
    /// Filter repository for loading filters.
    filter_repo: Arc<FR>,
    /// Session repository for session management.
    #[allow(dead_code)]
    session_repo: Arc<SSR>,
    /// Individual stream detector.
    detector: StreamDetector,
    /// Batch detector.
    batch_detector: BatchDetector,
    /// Rate limiter manager.
    rate_limiter: RateLimiterManager,
    /// In-flight request deduplication.
    in_flight: DashMap<String, Arc<OnceCell<LiveStatus>>>,
    /// Event broadcaster for notifications.
    event_broadcaster: MonitorEventBroadcaster,
    /// Configuration.
    #[allow(dead_code)]
    config: StreamMonitorConfig,
}

impl<
    SR: StreamerRepository + Send + Sync + 'static,
    FR: FilterRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
> StreamMonitor<SR, FR, SSR>
{
    /// Create a new stream monitor.
    pub fn new(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
    ) -> Self {
        Self::with_config(
            streamer_manager,
            filter_repo,
            session_repo,
            StreamMonitorConfig::default(),
        )
    }

    /// Create a new stream monitor with custom configuration.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config: StreamMonitorConfig,
    ) -> Self {
        // Create rate limiter with platform-specific configs
        let mut rate_limiter = RateLimiterManager::with_config(
            RateLimiterConfig::with_rps(config.default_rate_limit),
        );

        for (platform, rps) in &config.platform_rate_limits {
            rate_limiter.set_platform_config(platform, RateLimiterConfig::with_rps(*rps));
        }

        // Create HTTP client with timeout
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .pool_max_idle_per_host(config.max_concurrent_requests)
            .build()
            .unwrap_or_default();

        let detector = StreamDetector::with_client(client.clone());
        let batch_detector = BatchDetector::with_client(client, rate_limiter.clone());

        Self {
            streamer_manager,
            filter_repo,
            session_repo,
            detector,
            batch_detector,
            rate_limiter,
            in_flight: DashMap::new(),
            event_broadcaster: MonitorEventBroadcaster::new(),
            config,
        }
    }

    /// Subscribe to monitor events.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<MonitorEvent> {
        self.event_broadcaster.subscribe()
    }

    /// Get the event broadcaster for external use.
    pub fn event_broadcaster(&self) -> &MonitorEventBroadcaster {
        &self.event_broadcaster
    }

    /// Check the status of a single streamer.
    pub async fn check_streamer(&self, streamer: &StreamerMetadata) -> Result<LiveStatus> {
        debug!("Checking status for streamer: {}", streamer.id);

        // Request deduplication
        let cell = self
            .in_flight
            .entry(streamer.id.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        // If already in flight, wait for result
        if let Some(status) = cell.get() {
            debug!("Using cached in-flight result for {}", streamer.id);
            return Ok(status.clone());
        }

        // Acquire rate limit token
        let wait_time = self.rate_limiter.acquire(&streamer.platform_config_id).await;
        if !wait_time.is_zero() {
            debug!("Rate limited for {:?}", wait_time);
        }

        // Load filters for this streamer
        let filters = self.load_filters(&streamer.id).await?;

        // Check status with filters
        let status = self
            .detector
            .check_status_with_filters(streamer, &filters)
            .await?;

        // Store result for deduplication
        let _ = cell.set(status.clone());

        // Clean up in-flight entry
        self.in_flight.remove(&streamer.id);

        Ok(status)
    }

    /// Check the status of multiple streamers on the same platform.
    pub async fn batch_check(
        &self,
        platform_id: &str,
        streamers: Vec<StreamerMetadata>,
    ) -> Result<BatchResult> {
        debug!(
            "Batch checking {} streamers on platform {}",
            streamers.len(),
            platform_id
        );

        self.batch_detector.batch_check(platform_id, streamers).await
    }

    /// Process a status check result and update state.
    pub async fn process_status(
        &self,
        streamer: &StreamerMetadata,
        status: LiveStatus,
    ) -> Result<()> {
        debug!(
            "Processing status for {}: {:?}",
            streamer.id,
            status_summary(&status)
        );

        match status {
            LiveStatus::Live { title, category, started_at, streams, .. } => {
                self.handle_live(streamer, title, category, started_at, streams).await?;
            }
            LiveStatus::Offline => {
                self.handle_offline(streamer).await?;
            }
            LiveStatus::Filtered { reason, title, category } => {
                self.handle_filtered(streamer, reason, title, category).await?;
            }
            // Fatal errors - stop monitoring until manually cleared
            LiveStatus::NotFound => {
                self.handle_fatal_error(streamer, StreamerState::NotFound, "Streamer not found on platform").await?;
            }
            LiveStatus::Banned => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Streamer is banned on platform").await?;
            }
            LiveStatus::AgeRestricted => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Content is age-restricted").await?;
            }
            LiveStatus::RegionLocked => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Content is region-locked").await?;
            }
            LiveStatus::Private => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Content is private").await?;
            }
            LiveStatus::UnsupportedPlatform => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Platform is not supported").await?;
            }
        }

        Ok(())
    }

    /// Handle a streamer going live.
    async fn handle_live(
        &self,
        streamer: &StreamerMetadata,
        title: String,
        category: Option<String>,
        started_at: Option<chrono::DateTime<chrono::Utc>>,
        streams: Vec<platforms_parser::media::StreamInfo>,
    ) -> Result<()> {
        info!("Streamer {} is LIVE: {} ({} streams available)", streamer.name, title, streams.len());

        // Update state to Live
        self.streamer_manager
            .update_state(&streamer.id, StreamerState::Live)
            .await?;

        // Record success (resets error count, mark as going live)
        self.streamer_manager.record_success(&streamer.id, true).await?;

        // Emit live event for notifications and download triggering
        // Streams are passed directly from platform parser
        let event = MonitorEvent::StreamerLive {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            streamer_url: streamer.url.clone(),
            title: title.clone(),
            category: category.clone(),
            streams,
            timestamp: chrono::Utc::now(),
        };
        let _ = self.event_broadcaster.publish(event);

        // Create or update live session
        // This will be implemented with SessionRepository
        debug!(
            "Creating live session for {} (title: {}, category: {:?}, started: {:?})",
            streamer.id, title, category, started_at
        );

        Ok(())
    }

    /// Handle a streamer going offline.
    async fn handle_offline(&self, streamer: &StreamerMetadata) -> Result<()> {
        self.handle_offline_with_session(streamer, None).await
    }

    /// Handle a streamer going offline with optional session ID.
    pub async fn handle_offline_with_session(
        &self,
        streamer: &StreamerMetadata,
        session_id: Option<String>,
    ) -> Result<()> {
        debug!("Streamer {} is OFFLINE", streamer.name);

        // Only update if currently live
        if streamer.state == StreamerState::Live {
            info!("Streamer {} went offline", streamer.name);

            // Update state to NotLive
            self.streamer_manager
                .update_state(&streamer.id, StreamerState::NotLive)
                .await?;

            // Emit offline event for notifications
            let event = MonitorEvent::StreamerOffline {
                streamer_id: streamer.id.clone(),
                streamer_name: streamer.name.clone(),
                session_id,
                timestamp: chrono::Utc::now(),
            };
            let _ = self.event_broadcaster.publish(event);

            // End live session
            // This will be implemented with SessionRepository
            debug!("Ending live session for {}", streamer.id);
        }

        Ok(())
    }

    /// Handle a filtered status (live but out of schedule, etc.).
    async fn handle_filtered(
        &self,
        streamer: &StreamerMetadata,
        reason: FilterReason,
        _title: String,
        _category: Option<String>,
    ) -> Result<()> {
        let new_state = match reason {
            FilterReason::OutOfSchedule => StreamerState::OutOfSchedule,
            FilterReason::TitleMismatch | FilterReason::CategoryMismatch => {
                // For title/category mismatch, we still consider it "out of schedule"
                StreamerState::OutOfSchedule
            }
        };

        debug!(
            "Streamer {} filtered ({:?}), setting state to {:?}",
            streamer.name, reason, new_state
        );

        self.streamer_manager
            .update_state(&streamer.id, new_state)
            .await?;

        Ok(())
    }

    /// Handle a fatal error - stop monitoring until manually cleared.
    async fn handle_fatal_error(
        &self,
        streamer: &StreamerMetadata,
        new_state: StreamerState,
        reason: &str,
    ) -> Result<()> {
        warn!(
            "Fatal error for streamer {}: {} - setting state to {:?}",
            streamer.name, reason, new_state
        );

        // Update state to the fatal error state
        self.streamer_manager
            .update_state(&streamer.id, new_state)
            .await?;

        // Determine the fatal error type from the state
        let error_type = match new_state {
            StreamerState::NotFound => FatalErrorType::NotFound,
            _ => FatalErrorType::Banned, // Default to Banned for other fatal errors
        };

        // Emit fatal error event for notifications
        let event = MonitorEvent::FatalError {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            error_type,
            message: reason.to_string(),
            new_state,
            timestamp: chrono::Utc::now(),
        };
        let _ = self.event_broadcaster.publish(event);

        Ok(())
    }

    /// Handle an error during status check.
    pub async fn handle_error(&self, streamer: &StreamerMetadata, error: &str) -> Result<()> {
        warn!("Error checking streamer {}: {}", streamer.name, error);

        // Record error (may trigger backoff)
        self.streamer_manager
            .record_error(&streamer.id, error)
            .await?;

        // Get updated error count
        let consecutive_errors = self.streamer_manager
            .get_streamer(&streamer.id)
            .map(|s| s.consecutive_error_count)
            .unwrap_or(1);

        // Emit transient error event for notifications
        let event = MonitorEvent::TransientError {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            error_message: error.to_string(),
            consecutive_errors,
            timestamp: chrono::Utc::now(),
        };
        let _ = self.event_broadcaster.publish(event);

        Ok(())
    }

    /// Load filters for a streamer.
    async fn load_filters(&self, streamer_id: &str) -> Result<Vec<Filter>> {
        // Load filters from repository
        let filter_models = self.filter_repo.get_by_streamer(streamer_id).await?;

        // Convert to domain filters
        let filters: Vec<Filter> = filter_models
            .into_iter()
            .filter_map(|model| Filter::from_db_model(&model).ok())
            .collect();

        Ok(filters)
    }
}

/// Get a summary of a live status for logging.
fn status_summary(status: &LiveStatus) -> &'static str {
    match status {
        LiveStatus::Live { .. } => "Live",
        LiveStatus::Offline => "Offline",
        LiveStatus::Filtered { .. } => "Filtered",
        LiveStatus::NotFound => "NotFound",
        LiveStatus::Banned => "Banned",
        LiveStatus::AgeRestricted => "AgeRestricted",
        LiveStatus::RegionLocked => "RegionLocked",
        LiveStatus::Private => "Private",
        LiveStatus::UnsupportedPlatform => "UnsupportedPlatform",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stream_monitor_config_default() {
        let config = StreamMonitorConfig::default();
        assert_eq!(config.default_rate_limit, 1.0);
        assert_eq!(config.request_timeout, Duration::from_secs(30));
    }

    #[test]
    fn test_status_summary() {
        assert_eq!(status_summary(&LiveStatus::Offline), "Offline");
        
        let live_status = LiveStatus::Live {
            title: "Test".to_string(),
            category: None,
            started_at: None,
            viewer_count: None,
            streams: vec![platforms_parser::media::StreamInfo {
                url: "https://example.com/stream.flv".to_string(),
                stream_format: platforms_parser::media::StreamFormat::Flv,
                media_format: platforms_parser::media::formats::MediaFormat::Flv,
                quality: "best".to_string(),
                bitrate: 5000000,
                priority: 1,
                extras: None,
                codec: "h264".to_string(),
                fps: 30.0,
                is_headers_needed: false,
            }],
        };
        assert_eq!(status_summary(&live_status), "Live");
        
        assert_eq!(
            status_summary(&LiveStatus::Filtered {
                reason: FilterReason::OutOfSchedule,
                title: "Test".to_string(),
                category: None,
            }),
            "Filtered"
        );
        // Test fatal error statuses
        assert_eq!(status_summary(&LiveStatus::NotFound), "NotFound");
        assert_eq!(status_summary(&LiveStatus::Banned), "Banned");
        assert_eq!(status_summary(&LiveStatus::UnsupportedPlatform), "UnsupportedPlatform");
    }
}
