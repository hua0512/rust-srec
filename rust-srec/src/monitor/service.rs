//! Stream Monitor service implementation.
//!
//! The StreamMonitor coordinates live status detection, filter evaluation,
//! and state updates for streamers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::OnceCell;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::models::{LiveSessionDbModel, TitleEntry};
use crate::database::repositories::{FilterRepository, SessionRepository, StreamerRepository};
use crate::domain::StreamerState;
use crate::domain::filter::Filter;
use crate::streamer::{StreamerManager, StreamerMetadata};

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
            platform_rate_limits: vec![("twitch".to_string(), 2.0), ("youtube".to_string(), 1.0)],
            request_timeout: Duration::from_secs(0),
            max_concurrent_requests: 10,
        }
    }
}

/// The Stream Monitor service.
pub struct StreamMonitor<
    SR: StreamerRepository + Send + Sync + 'static,
    FR: FilterRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
> {
    /// Streamer manager for state updates.
    streamer_manager: Arc<StreamerManager<SR>>,
    /// Filter repository for loading filters.
    filter_repo: Arc<FR>,
    /// Session repository for session management.
    #[allow(dead_code)]
    session_repo: Arc<SSR>,
    /// Config service for resolving streamer configuration.
    config_service: Arc<crate::config::ConfigService<CR, SR>>,
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
    CR: crate::database::repositories::ConfigRepository + Send + Sync + 'static,
> StreamMonitor<SR, FR, SSR, CR>
{
    /// Create a new stream monitor.
    pub fn new(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config_service: Arc<crate::config::ConfigService<CR, SR>>,
    ) -> Self {
        Self::with_config(
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            StreamMonitorConfig::default(),
        )
    }

    /// Create a new stream monitor with custom configuration.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config_service: Arc<crate::config::ConfigService<CR, SR>>,
        config: StreamMonitorConfig,
    ) -> Self {
        // Create rate limiter with platform-specific configs
        let mut rate_limiter =
            RateLimiterManager::with_config(RateLimiterConfig::with_rps(config.default_rate_limit));

        for (platform, rps) in &config.platform_rate_limits {
            rate_limiter.set_platform_config(platform, RateLimiterConfig::with_rps(*rps));
        }

        // Create HTTP client
        let mut client_builder = platforms_parser::extractor::create_client_builder(None);

        if config.request_timeout > Duration::from_secs(0) {
            client_builder = client_builder.timeout(config.request_timeout);
        }

        if config.max_concurrent_requests > 0 {
            client_builder = client_builder.pool_max_idle_per_host(config.max_concurrent_requests);
        }

        let client = client_builder
            .build()
            .expect("Failed to create HTTP client");

        let detector = StreamDetector::with_client(client.clone());
        let batch_detector = BatchDetector::with_client(client, rate_limiter.clone());

        Self {
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
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
        let wait_time = self
            .rate_limiter
            .acquire(&streamer.platform_config_id)
            .await;
        if !wait_time.is_zero() {
            debug!("Rate limited for {:?}", wait_time);
        }

        // Load filters for this streamer
        let filters = self.load_filters(&streamer.id).await?;

        // Get merged configuration to access stream selection preference
        let config = self
            .config_service
            .get_config_for_streamer(&streamer.id)
            .await?;

        // Check status with filters and selection config
        let status = self
            .detector
            .check_status_with_filters(streamer, &filters, Some(&config.stream_selection))
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

        self.batch_detector
            .batch_check(platform_id, streamers)
            .await
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
            LiveStatus::Live {
                title,
                category,
                avatar,
                started_at,
                streams,
                media_headers,
                ..
            } => {
                self.handle_live(
                    streamer,
                    title,
                    category,
                    avatar,
                    started_at,
                    streams,
                    media_headers,
                )
                .await?;
            }
            LiveStatus::Offline => {
                self.handle_offline(streamer).await?;
            }
            LiveStatus::Filtered {
                reason,
                title,
                category,
            } => {
                self.handle_filtered(streamer, reason, title, category)
                    .await?;
            }
            // Fatal errors - stop monitoring until manually cleared
            LiveStatus::NotFound => {
                self.handle_fatal_error(
                    streamer,
                    StreamerState::NotFound,
                    "Streamer not found on platform",
                )
                .await?;
            }
            LiveStatus::Banned => {
                self.handle_fatal_error(
                    streamer,
                    StreamerState::FatalError,
                    "Streamer is banned on platform",
                )
                .await?;
            }
            LiveStatus::AgeRestricted => {
                self.handle_fatal_error(
                    streamer,
                    StreamerState::FatalError,
                    "Content is age-restricted",
                )
                .await?;
            }
            LiveStatus::RegionLocked => {
                self.handle_fatal_error(
                    streamer,
                    StreamerState::FatalError,
                    "Content is region-locked",
                )
                .await?;
            }
            LiveStatus::Private => {
                self.handle_fatal_error(streamer, StreamerState::FatalError, "Content is private")
                    .await?;
            }
            LiveStatus::UnsupportedPlatform => {
                self.handle_fatal_error(
                    streamer,
                    StreamerState::FatalError,
                    "Platform is not supported",
                )
                .await?;
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
        avatar: Option<String>,
        _started_at: Option<chrono::DateTime<chrono::Utc>>,
        streams: Vec<platforms_parser::media::StreamInfo>,
        media_headers: Option<HashMap<String, String>>,
    ) -> Result<()> {
        info!(
            "Streamer {} is LIVE: {} ({} streams available, {} media headers)",
            streamer.name,
            title,
            streams.len(),
            media_headers.as_ref().map(|h| h.len()).unwrap_or(0)
        );

        // Update state to Live
        self.streamer_manager
            .update_state(&streamer.id, StreamerState::Live)
            .await?;

        // Update metadata if changed (e.g. avatar)
        if let Some(ref new_avatar_url) = avatar {
            if !new_avatar_url.is_empty() && avatar != streamer.avatar_url {
                info!("Updating avatar for streamer {}", streamer.name);
                if let Err(e) = self
                    .streamer_manager
                    .update_avatar(&streamer.id, avatar)
                    .await
                {
                    warn!("Failed to update streamer avatar: {}", e);
                }
            }
        }
        // Record success (resets error count, mark as going live)
        self.streamer_manager
            .record_success(&streamer.id, true)
            .await?;

        // Logic for session management (creation or resumption)
        let merged_config = self
            .config_service
            .get_config_for_streamer(&streamer.id)
            .await?;
        let gap_secs = merged_config.session_gap_time_secs;

        // Check for last session
        let last_sessions = self
            .session_repo
            .list_sessions_for_streamer(&streamer.id, 1)
            .await?;
        let last_session = last_sessions.first();

        let session_id = if let Some(session) = last_session {
            // Check if active or recently ended
            if session.end_time.is_none() {
                // Already active, reuse
                debug!("Reusing active session {}", session.id);
                // Check if title changed and update if needed
                if let Err(e) = self.update_session_title(&session, &title).await {
                    warn!("Failed to update session title: {}", e);
                }
                session.id.clone()
            } else {
                let end_time_str = session.end_time.as_ref().unwrap();
                let end_time = chrono::DateTime::parse_from_rfc3339(end_time_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let now = chrono::Utc::now();

                if (now - end_time).num_seconds() < gap_secs {
                    // Resume session
                    info!(
                        "Resuming session {} (ended {:?} ago)",
                        session.id,
                        now - end_time
                    );
                    self.session_repo.resume_session(&session.id).await?;
                    // Check if title changed and update if needed
                    if let Err(e) = self.update_session_title(&session, &title).await {
                        warn!("Failed to update session title: {}", e);
                    }
                    session.id.clone()
                } else {
                    let new_id = uuid::Uuid::new_v4().to_string();
                    let initial_titles = vec![TitleEntry {
                        ts: chrono::Utc::now().to_rfc3339(),
                        title: title.clone(),
                    }];
                    let titles_json =
                        serde_json::to_string(&initial_titles).unwrap_or("[]".to_string());

                    let new_session = LiveSessionDbModel {
                        id: new_id.clone(),
                        streamer_id: streamer.id.clone(),
                        start_time: chrono::Utc::now().to_rfc3339(),
                        end_time: None,
                        titles: Some(titles_json),
                        danmu_statistics_id: None,
                        total_size_bytes: 0,
                    };
                    self.session_repo.create_session(&new_session).await?;
                    info!("Created new session {}", new_id);
                    new_id
                }
            }
        } else {
            // No previous session, create new
            let new_id = uuid::Uuid::new_v4().to_string();
            let initial_titles = vec![TitleEntry {
                ts: chrono::Utc::now().to_rfc3339(),
                title: title.clone(),
            }];
            let titles_json = serde_json::to_string(&initial_titles).unwrap_or("[]".to_string());

            let new_session = LiveSessionDbModel {
                id: new_id.clone(),
                streamer_id: streamer.id.clone(),
                start_time: chrono::Utc::now().to_rfc3339(),
                end_time: None,
                titles: Some(titles_json),
                danmu_statistics_id: None,
                total_size_bytes: 0,
            };
            self.session_repo.create_session(&new_session).await?;
            info!("Created new session {}", new_id);
            new_id
        };

        // Emit live event for notifications and download triggering
        let event = MonitorEvent::StreamerLive {
            streamer_id: streamer.id.clone(),
            session_id: session_id.clone(),
            streamer_name: streamer.name.clone(),
            streamer_url: streamer.url.clone(),
            title: title.clone(),
            category: category.clone(),
            streams,
            media_headers,
            timestamp: chrono::Utc::now(),
        };
        let _ = self.event_broadcaster.publish(event);

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

            // Resolve session_id if not provided
            let resolved_session_id = if let Some(id) = session_id {
                Some(id)
            } else {
                // Find active session
                self.session_repo
                    .get_active_session_for_streamer(&streamer.id)
                    .await?
                    .map(|s| s.id)
            };

            // End live session if found
            if let Some(ref sid) = resolved_session_id {
                debug!("Ending live session {}", sid);
                self.session_repo
                    .end_session(sid, &chrono::Utc::now().to_rfc3339())
                    .await?;
            }

            // Emit offline event for notifications
            let event = MonitorEvent::StreamerOffline {
                streamer_id: streamer.id.clone(),
                streamer_name: streamer.name.clone(),
                session_id: resolved_session_id,
                timestamp: chrono::Utc::now(),
            };
            let _ = self.event_broadcaster.publish(event);
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
        let consecutive_errors = self
            .streamer_manager
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

    /// Update session title if it has changed.
    async fn update_session_title(
        &self,
        session: &LiveSessionDbModel,
        current_title: &str,
    ) -> Result<()> {
        let mut titles: Vec<TitleEntry> = match &session.titles {
            Some(json) => serde_json::from_str(json).unwrap_or_default(),
            None => Vec::new(),
        };

        // Check if last title is different
        let needs_update = titles
            .last()
            .map(|t| t.title != current_title)
            .unwrap_or(true); // If no titles, we definitely need to add one

        if needs_update {
            titles.push(TitleEntry {
                ts: chrono::Utc::now().to_rfc3339(),
                title: current_title.to_string(),
            });

            let titles_json = serde_json::to_string(&titles).unwrap_or("[]".to_string());

            info!(
                "Updating title for session {}: {}",
                session.id, current_title
            );
            self.session_repo
                .update_session_titles(&session.id, &titles_json)
                .await?;
        }

        Ok(())
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
        assert_eq!(config.request_timeout, Duration::from_secs(0));
    }

    #[test]
    fn test_status_summary() {
        assert_eq!(status_summary(&LiveStatus::Offline), "Offline");

        let live_status = LiveStatus::Live {
            title: "Test".to_string(),
            avatar: None,
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
            media_headers: None,
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
        assert_eq!(
            status_summary(&LiveStatus::UnsupportedPlatform),
            "UnsupportedPlatform"
        );
    }
}
