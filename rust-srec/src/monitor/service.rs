//! Stream Monitor service implementation.
//!
//! The StreamMonitor coordinates live status detection, filter evaluation,
//! and state updates for streamers.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio::sync::Notify;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::ImmediateTransaction;
use crate::database::repositories::{
    FilterRepository, MonitorOutboxOps, MonitorOutboxTxOps, SessionRepository, SessionTxOps,
    StreamerRepository, StreamerTxOps,
};
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
    detector: Arc<StreamDetector>,
    /// Batch detector.
    batch_detector: BatchDetector,
    /// Rate limiter manager.
    rate_limiter: RateLimiterManager,
    /// In-flight request deduplication.
    in_flight: Arc<DashMap<String, Arc<OnceCell<LiveStatus>>>>,
    /// Event broadcaster for notifications.
    event_broadcaster: MonitorEventBroadcaster,
    /// Database pool for transactional updates + outbox.
    pool: SqlitePool,
    /// Notifies the outbox publisher that new events are available.
    outbox_notify: Arc<Notify>,
    /// Cancellation token for the outbox publisher task.
    outbox_cancellation: CancellationToken,
    /// Configuration.
    #[allow(dead_code)]
    config: StreamMonitorConfig,
}

/// Details for a streamer going live.
pub(crate) struct LiveStatusDetails {
    pub title: String,
    pub category: Option<String>,
    pub avatar: Option<String>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub streams: Vec<platforms_parser::media::StreamInfo>,
    pub media_headers: Option<std::collections::HashMap<String, String>>,
    pub media_extras: Option<std::collections::HashMap<String, String>>,
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
        pool: SqlitePool,
    ) -> Self {
        Self::with_config(
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            pool,
            StreamMonitorConfig::default(),
        )
    }

    /// Create a new stream monitor with custom configuration.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config_service: Arc<crate::config::ConfigService<CR, SR>>,
        pool: SqlitePool,
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

        let detector = Arc::new(StreamDetector::with_client(client.clone()));
        let batch_detector = BatchDetector::with_client(client, rate_limiter.clone());

        let outbox_notify = Arc::new(Notify::new());
        let outbox_cancellation = CancellationToken::new();

        let monitor = Self {
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            detector,
            batch_detector,
            rate_limiter,
            in_flight: Arc::new(DashMap::new()),
            event_broadcaster: MonitorEventBroadcaster::new(),
            pool,
            outbox_notify: outbox_notify.clone(),
            outbox_cancellation: outbox_cancellation.clone(),
            config,
        };

        monitor.spawn_outbox_publisher(outbox_notify, outbox_cancellation);

        monitor
    }

    /// Subscribe to monitor events.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<MonitorEvent> {
        self.event_broadcaster.subscribe()
    }

    /// Get the event broadcaster for external use.
    pub fn event_broadcaster(&self) -> &MonitorEventBroadcaster {
        &self.event_broadcaster
    }

    /// Stop the stream monitor's background tasks.
    ///
    /// This cancels the outbox publisher task. Should be called during
    /// graceful shutdown to ensure clean task termination.
    pub fn stop(&self) {
        info!("Stopping StreamMonitor outbox publisher");
        self.outbox_cancellation.cancel();
    }

    fn spawn_outbox_publisher(
        &self,
        outbox_notify: Arc<Notify>,
        cancellation_token: CancellationToken,
    ) {
        let pool = self.pool.clone();
        let broadcaster = self.event_broadcaster.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    // Check for cancellation first
                    _ = cancellation_token.cancelled() => {
                        info!("Outbox publisher shutting down");
                        break;
                    }
                    _ = outbox_notify.notified() => {}
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                }

                // Check cancellation again before processing
                if cancellation_token.is_cancelled() {
                    break;
                }

                if let Err(e) = flush_outbox_once(&pool, &broadcaster).await {
                    warn!("Monitor outbox flush failed: {}", e);
                }
            }
            debug!("Outbox publisher stopped");
        });
    }

    /// Start an immediate transaction to prevent locking issues.
    async fn begin_immediate(&self) -> Result<ImmediateTransaction> {
        let mut conn = self.pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        Ok(ImmediateTransaction(conn))
    }

    /// Check the status of a single streamer.
    ///
    /// This method deduplicates concurrent requests for the same streamer.
    /// If multiple requests come in for the same streamer simultaneously,
    /// only one will perform the actual HTTP check and others will wait
    /// for and share the result.
    pub async fn check_streamer(&self, streamer: &StreamerMetadata) -> Result<LiveStatus> {
        debug!("Checking status for streamer: {}", streamer.id);

        // Get or create the deduplication cell for this streamer
        let cell = self
            .in_flight
            .entry(streamer.id.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();

        // Clone what we need for the async closure
        let rate_limiter = self.rate_limiter.clone();
        let filter_repo = self.filter_repo.clone();
        let config_service = self.config_service.clone();
        let detector = self.detector.clone();
        let streamer_id = streamer.id.clone();
        let platform_config_id = streamer.platform_config_id.clone();
        let streamer_clone = streamer.clone();

        // get_or_try_init ensures only ONE caller executes the closure,
        // all other concurrent callers wait for the result
        let result = cell
            .get_or_try_init(|| async move {
                // Acquire rate limit token
                let wait_time = rate_limiter.acquire(&platform_config_id).await;
                if !wait_time.is_zero() {
                    debug!("Rate limited for {:?}", wait_time);
                }

                // Load filters for this streamer
                let filter_models = filter_repo.get_by_streamer(&streamer_id).await?;
                let filters: Vec<Filter> = filter_models
                    .into_iter()
                    .filter_map(|model| Filter::from_db_model(&model).ok())
                    .collect();

                // Get merged configuration to access stream selection preference and cookies
                let config = config_service.get_config_for_streamer(&streamer_id).await?;

                // Check status with filters, cookies, and selection config
                detector
                    .check_status_with_filters(
                        &streamer_clone,
                        &filters,
                        config.cookies.clone(),
                        Some(&config.stream_selection),
                    )
                    .await
            })
            .await;

        // Schedule delayed cleanup BEFORE checking result to ensure cleanup
        // happens regardless of success or error. This prevents in_flight map
        // from leaking entries when errors occur.
        let in_flight = self.in_flight.clone();
        let id = streamer.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            in_flight.remove(&id);
        });

        // Now check result - cleanup is already scheduled
        let status = result?.clone();

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

        // Fetch fresh metadata to ensure we have the latest state
        // The StreamerActor might be holding stale metadata
        let fresh_streamer = self.streamer_manager.get_streamer(&streamer.id);
        let streamer = fresh_streamer.as_ref().unwrap_or(streamer);

        match status {
            LiveStatus::Live {
                title,
                category,
                avatar,
                started_at,
                streams,
                media_headers,
                media_extras,
                ..
            } => {
                self.handle_live(
                    streamer,
                    LiveStatusDetails {
                        title,
                        category,
                        avatar,
                        started_at,
                        streams,
                        media_headers,
                        media_extras,
                    },
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
        details: LiveStatusDetails,
    ) -> Result<()> {
        let LiveStatusDetails {
            title,
            category,
            avatar,
            started_at,
            streams,
            media_headers,
            media_extras,
        } = details;
        info!(
            "Streamer {} is LIVE: {} ({} streams available, {} media headers)",
            streamer.name,
            title,
            streams.len(),
            media_headers.as_ref().map(|h| h.len()).unwrap_or(0)
        );

        let now = chrono::Utc::now();

        // Transaction: (session create/resume + streamer state update + outbox event).
        // If anything fails, the database remains consistent and no event is emitted.
        // Use BEGIN IMMEDIATE to prevent deadlocks during concurrent checks.
        let mut tx = self.begin_immediate().await?;

        // Logic for session management (creation or resumption)
        let merged_config = self
            .config_service
            .get_config_for_streamer(&streamer.id)
            .await?;
        let gap_secs = merged_config.session_gap_time_secs;

        // Check for last session
        let last_session = SessionTxOps::get_last_session(&mut tx, &streamer.id).await?;

        let session_id = if let Some(session) = last_session {
            // Check if active or recently ended
            if session.end_time.is_none() {
                // Already active, reuse
                debug!("Reusing active session {}", session.id);
                SessionTxOps::update_titles(
                    &mut tx,
                    &session.id,
                    session.titles.as_deref(),
                    &title,
                    now,
                )
                .await?;
                session.id.clone()
            } else {
                let end_time_str = session.end_time.as_ref().unwrap();
                let end_time = chrono::DateTime::parse_from_rfc3339(end_time_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());

                // Check if the stream is a continuation (monitoring gap)
                if SessionTxOps::should_resume_by_continuation(end_time, started_at) {
                    info!(
                        "Resuming session {} (stream started at {:?}, before session end at {})",
                        session.id, started_at, end_time_str
                    );
                    SessionTxOps::resume_session(&mut tx, &session.id).await?;
                    SessionTxOps::update_titles(
                        &mut tx,
                        &session.id,
                        session.titles.as_deref(),
                        &title,
                        now,
                    )
                    .await?;
                    session.id.clone()
                } else if SessionTxOps::should_resume_by_gap(end_time, now, gap_secs) {
                    // Resume within gap threshold
                    let offline_duration_secs = (now - end_time).num_seconds();
                    info!(
                        "Resuming session {} (offline for {}s, threshold: {}s)",
                        session.id, offline_duration_secs, gap_secs
                    );
                    SessionTxOps::resume_session(&mut tx, &session.id).await?;
                    SessionTxOps::update_titles(
                        &mut tx,
                        &session.id,
                        session.titles.as_deref(),
                        &title,
                        now,
                    )
                    .await?;
                    session.id.clone()
                } else {
                    // Create new session
                    let offline_duration_secs = (now - end_time).num_seconds();
                    info!(
                        "Creating new session for {} (offline for {}s exceeded threshold of {}s)",
                        streamer.name, offline_duration_secs, gap_secs
                    );
                    let new_id = uuid::Uuid::new_v4().to_string();
                    SessionTxOps::create_session(&mut tx, &new_id, &streamer.id, now, &title)
                        .await?;
                    info!("Created new session {}", new_id);
                    new_id
                }
            }
        } else {
            // No previous session, create new
            let new_id = uuid::Uuid::new_v4().to_string();
            SessionTxOps::create_session(&mut tx, &new_id, &streamer.id, now, &title).await?;
            info!("Created new session {}", new_id);
            new_id
        };

        // Update streamer state and clear error backoff as part of the same transaction.
        StreamerTxOps::set_live(&mut tx, &streamer.id, now).await?;

        if let Some(ref new_avatar_url) = avatar
            && !new_avatar_url.is_empty()
            && avatar != streamer.avatar_url
        {
            StreamerTxOps::update_avatar(&mut tx, &streamer.id, new_avatar_url).await?;
        }

        // Enqueue live event for notifications and download triggering.
        let event = MonitorEvent::StreamerLive {
            streamer_id: streamer.id.clone(),
            session_id: session_id.clone(),
            streamer_name: streamer.name.clone(),
            streamer_url: streamer.url.clone(),
            title: title.clone(),
            category: category.clone(),
            streams,
            media_headers,
            media_extras,
            timestamp: now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

        tx.commit().await?;

        // Reload metadata from DB to sync in-memory cache.
        // Errors here are logged but not propagated since the DB transaction succeeded.
        if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
            warn!(
                "Failed to reload streamer {} after state update: {}. Cache may be stale.",
                streamer.id, e
            );
        }
        self.outbox_notify.notify_one();

        Ok(())
    }

    /// Handle a streamer going offline.
    async fn handle_offline(&self, streamer: &StreamerMetadata) -> Result<()> {
        self.handle_offline_with_session(streamer, None).await
    }

    /// Handle a streamer going offline with optional session ID.
    ///
    /// A successful Offline check (no network error) proves connectivity to the platform,
    /// so we aggressively clear any accumulated transient errors.
    ///
    /// # State Transition Table
    ///
    /// | Previous State       | Has Errors? | Action                                          |
    /// |----------------------|-------------|------------------------------------------------|
    /// | Live                 | No          | End session, set state to NOT_LIVE             |
    /// | Live                 | Yes         | End session, set NOT_LIVE, clear errors        |
    /// | TemporalDisabled     | Yes         | Set state to NOT_LIVE, clear errors            |
    /// | NotLive              | Yes         | Clear errors only                              |
    /// | NotLive              | No          | No action (already clean)                      |
    /// | OutOfSchedule        | Yes         | Clear errors only                              |
    /// | OutOfSchedule        | No          | No action                                      |
    ///
    /// "Has Errors" means `consecutive_error_count > 0` or `disabled_until` is set.
    pub async fn handle_offline_with_session(
        &self,
        streamer: &StreamerMetadata,
        session_id: Option<String>,
    ) -> Result<()> {
        debug!("Streamer {} is OFFLINE", streamer.name);

        // Check if we have accumulated errors that should be cleared on successful check
        let has_errors = streamer.consecutive_error_count > 0 || streamer.disabled_until.is_some();

        if streamer.state == StreamerState::Live {
            // Live -> Offline transition: end session and update state
            info!("Streamer {} went offline", streamer.name);

            let now = chrono::Utc::now();

            let mut tx = self.begin_immediate().await?;

            let resolved_session_id = if let Some(id) = session_id {
                SessionTxOps::end_session(&mut tx, &id, now).await?;
                Some(id)
            } else {
                SessionTxOps::end_active_session(&mut tx, &streamer.id, now).await?
            };

            StreamerTxOps::set_offline(&mut tx, &streamer.id).await?;

            // Clear any accumulated errors since we successfully checked
            if has_errors {
                StreamerTxOps::clear_error_state(&mut tx, &streamer.id).await?;
            }

            let event = MonitorEvent::StreamerOffline {
                streamer_id: streamer.id.clone(),
                streamer_name: streamer.name.clone(),
                session_id: resolved_session_id.clone(),
                timestamp: now,
            };
            MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

            tx.commit().await?;

            // Reload metadata from DB to sync in-memory cache.
            if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
                warn!(
                    "Failed to reload streamer {} after offline update: {}. Cache may be stale.",
                    streamer.id, e
                );
            }
            self.outbox_notify.notify_one();
        } else if has_errors {
            // Successful check with accumulated errors: clear them
            // This handles TemporalDisabled -> NotLive and NotLive with errors -> NotLive clean
            info!(
                "Streamer {} successful check, clearing {} accumulated errors",
                streamer.name, streamer.consecutive_error_count
            );

            let mut tx = self.begin_immediate().await?;

            // Clear error state: reset consecutive_error_count, disabled_until, last_error
            StreamerTxOps::clear_error_state(&mut tx, &streamer.id).await?;

            // If was TemporalDisabled, also set state back to NOT_LIVE
            if streamer.state == StreamerState::TemporalDisabled {
                StreamerTxOps::set_offline(&mut tx, &streamer.id).await?;
            }

            tx.commit().await?;

            // Reload metadata from DB to sync in-memory cache.
            if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
                warn!(
                    "Failed to reload streamer {} after clearing error state: {}. Cache may be stale.",
                    streamer.id, e
                );
            }
        }

        Ok(())
    }

    /// Force end any active session for a streamer.
    ///
    /// This is used during streamer disable/delete operations where we need to
    /// end sessions regardless of the streamer's current in-memory state.
    /// Unlike `handle_offline_with_session`, this does not check state transitions
    /// and directly ends any active session in the database.
    ///
    /// # Arguments
    /// * `streamer_id` - The ID of the streamer whose session should be ended
    ///
    /// # Returns
    /// * `Ok(Some(session_id))` if a session was ended
    /// * `Ok(None)` if no active session existed
    pub async fn force_end_active_session(&self, streamer_id: &str) -> Result<Option<String>> {
        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        let session_id = SessionTxOps::end_active_session(&mut tx, streamer_id, now).await?;

        if let Some(ref id) = session_id {
            info!(
                "Force ended active session {} for streamer {}",
                id, streamer_id
            );
        }

        tx.commit().await?;

        Ok(session_id)
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
            FilterReason::OutOfSchedule { .. } => StreamerState::OutOfSchedule,
            FilterReason::TitleMismatch | FilterReason::CategoryMismatch => {
                // For title/category mismatch, we still consider it "out of schedule"
                StreamerState::OutOfSchedule
            }
        };

        if streamer.state == new_state {
            return Ok(());
        }

        debug!(
            "Streamer {} filtered ({:?}), setting state to {:?}",
            streamer.name, reason, new_state
        );

        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        StreamerTxOps::update_state(&mut tx, &streamer.id, &new_state.to_string()).await?;

        // Enqueue a state change event (non-notifying) so consumers see the same ordering/guarantees
        // as live/offline/fatal transitions.
        let event = MonitorEvent::StateChanged {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            old_state: streamer.state,
            new_state,
            timestamp: now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

        tx.commit().await?;

        // Reload metadata from DB to sync in-memory cache.
        // Errors here are logged but not propagated since the DB transaction succeeded.
        if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
            warn!(
                "Failed to reload streamer {} after state update: {}. Cache may be stale.",
                streamer.id, e
            );
        }
        self.outbox_notify.notify_one();

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

        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        let _ = SessionTxOps::end_active_session(&mut tx, &streamer.id, now).await?;

        // Update state to the fatal error state
        StreamerTxOps::set_fatal_error(&mut tx, &streamer.id, &new_state.to_string()).await?;

        // Determine the fatal error type from the state
        let error_type = match new_state {
            StreamerState::NotFound => FatalErrorType::NotFound,
            _ => FatalErrorType::Banned, // Default to Banned for other fatal errors
        };

        // Emit fatal error event via outbox.
        let event = MonitorEvent::FatalError {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            error_type,
            message: reason.to_string(),
            new_state,
            timestamp: now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

        tx.commit().await?;

        // Reload metadata from DB to sync in-memory cache.
        // Errors here are logged but not propagated since the DB transaction succeeded.
        if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
            warn!(
                "Failed to reload streamer {} after state update: {}. Cache may be stale.",
                streamer.id, e
            );
        }
        self.outbox_notify.notify_one();

        Ok(())
    }

    /// Handle an error during status check.
    pub async fn handle_error(&self, streamer: &StreamerMetadata, error: &str) -> Result<()> {
        warn!("Error checking streamer {}: {}", streamer.name, error);

        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        let new_error_count = StreamerTxOps::increment_error(&mut tx, &streamer.id, error).await?;

        let disabled_until = self
            .streamer_manager
            .disabled_until_for_error_count(new_error_count);

        StreamerTxOps::set_disabled_until(&mut tx, &streamer.id, disabled_until).await?;

        if let Some(until) = disabled_until {
            info!(
                "Streamer {} disabled until {} due to {} consecutive errors",
                streamer.id, until, new_error_count
            );
        }

        // Emit transient error event via outbox so DB + event are consistent.
        let event = MonitorEvent::TransientError {
            streamer_id: streamer.id.clone(),
            streamer_name: streamer.name.clone(),
            error_message: error.to_string(),
            consecutive_errors: new_error_count,
            timestamp: now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

        tx.commit().await?;

        // Reload metadata from DB to sync in-memory cache.
        // Errors here are logged but not propagated since the DB transaction succeeded.
        if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
            warn!(
                "Failed to reload streamer {} after state update: {}. Cache may be stale.",
                streamer.id, e
            );
        }
        self.outbox_notify.notify_one();

        Ok(())
    }

    /// Set a streamer to temporarily disabled due to circuit breaker block.
    ///
    /// This sets the state to `TemporalDisabled` and stores the disabled_until timestamp
    /// without incrementing the error count (since it's an infrastructure issue,
    /// not a streamer issue).
    ///
    /// # Arguments
    /// * `streamer` - The streamer metadata
    /// * `retry_after_secs` - Seconds until the circuit breaker allows retries
    pub async fn set_circuit_breaker_blocked(
        &self,
        streamer: &StreamerMetadata,
        retry_after_secs: u64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let disabled_until = now + chrono::Duration::seconds(retry_after_secs as i64);

        info!(
            "Streamer {} blocked by circuit breaker, disabled until {} ({}s)",
            streamer.name, disabled_until, retry_after_secs
        );

        let mut tx = self.begin_immediate().await?;

        // Set state to TEMPORAL_DISABLED with disabled_until timestamp
        // Note: We don't increment error count since this is infrastructure-level, not streamer-level
        StreamerTxOps::set_disabled_until(&mut tx, &streamer.id, Some(disabled_until)).await?;

        tx.commit().await?;

        // Reload metadata from DB to sync in-memory cache.
        if let Err(e) = self.streamer_manager.reload_from_repo(&streamer.id).await {
            warn!(
                "Failed to reload streamer {} after circuit breaker block: {}. Cache may be stale.",
                streamer.id, e
            );
        }

        Ok(())
    }
}

async fn flush_outbox_once(pool: &SqlitePool, broadcaster: &MonitorEventBroadcaster) -> Result<()> {
    let entries = MonitorOutboxOps::fetch_undelivered(pool, 100).await?;

    if entries.is_empty() {
        return Ok(());
    }

    for entry in entries {
        match serde_json::from_str::<MonitorEvent>(&entry.payload) {
            Ok(event) => {
                // Attempt to broadcast the event
                match broadcaster.publish(event) {
                    Ok(receiver_count) => {
                        // Event was successfully sent to `receiver_count` receivers
                        debug!("Published event to {} receivers", receiver_count);
                        MonitorOutboxOps::mark_delivered(pool, entry.id).await?;
                    }
                    Err(e) => {
                        // No receivers available - this is not a permanent failure
                        // Log the condition but still mark as delivered to avoid infinite retry
                        // The event will be lost if no receivers exist (expected during shutdown)
                        warn!(
                            "Monitor outbox event id={} has no receivers, discarding: {:?}",
                            entry.id, e.0
                        );
                        MonitorOutboxOps::mark_delivered(pool, entry.id).await?;
                    }
                }
            }
            Err(e) => {
                warn!("Invalid monitor outbox payload id={}: {}", entry.id, e);
                MonitorOutboxOps::record_failure(pool, entry.id, &e.to_string()).await?;
            }
        }
    }

    Ok(())
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
            media_extras: None,
        };
        assert_eq!(status_summary(&live_status), "Live");

        assert_eq!(
            status_summary(&LiveStatus::Filtered {
                reason: FilterReason::OutOfSchedule {
                    next_available: None,
                },
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
