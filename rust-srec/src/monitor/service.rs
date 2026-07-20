//! Stream Monitor service implementation.
//!
//! The StreamMonitor coordinates live status detection, filter evaluation,
//! and state updates for streamers.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio::sync::OnceCell;
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;
use tokio_util::time::DelayQueue;
use tracing::{debug, info, trace, warn};

use crate::credentials::CredentialRefreshService;
use crate::database::ImmediateTransaction;
use crate::database::repositories::{
    ConfigRepository, FilterRepository, MonitorOutboxOps, MonitorOutboxTxOps, SessionRepository,
    SessionTxOps, StreamerRepository, StreamerTxOps,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::domain::StreamerState;
use crate::domain::filter::Filter;
use crate::streamer::{StreamerManager, StreamerMetadata};
use crate::{Error, Result};

use super::batch_detector::{BatchDetector, BatchResult};
use super::detector::{FilterReason, LiveStatus, StreamDetector};
use crate::domain::streamer::FatalErrorType;

use super::events::{MonitorEvent, MonitorEventBroadcaster};
use super::rate_limiter::{RateLimiterConfig, RateLimiterManager};

/// Result of [`StreamMonitor::process_status`].
///
/// This separates two outcomes that previously looked identical at the type level:
///
/// - the monitor accepted the observed [`LiveStatus`] and applied its normal side effects
///   (state changes, session updates, outbox events)
/// - the monitor intentionally suppressed those side effects because the streamer is disabled
///   or still inside temporary backoff
///
/// Callers such as the scheduler actor use this to preserve authoritative backoff while still
/// keeping their local runtime state recoverable. In particular, a suppressed LIVE observation
/// must not leave the actor stuck in a pseudo-live state when no [`MonitorEvent::StreamerLive`]
/// was emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessStatusResult {
    /// The status was accepted and normal side effects were applied.
    ///
    /// For example, a LIVE status may create or resume a session and enqueue a
    /// [`MonitorEvent::StreamerLive`] outbox entry.
    Applied,
    /// The status was intentionally suppressed.
    ///
    /// Suppression means the status was observed, but `process_status()` deliberately skipped
    /// downstream effects. Callers should inspect the [`ProcessStatusSuppression`] reason and
    /// decide how to schedule the next retry without assuming the streamer has fully transitioned.
    Suppressed(ProcessStatusSuppression),
}

/// Reason a status was suppressed by [`StreamMonitor::process_status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessStatusSuppression {
    /// The streamer is manually disabled.
    ///
    /// This is the strongest suppression mode: manual disable should block monitor-driven state
    /// changes and side effects until the user re-enables the streamer.
    Disabled,
    /// The streamer is temporarily disabled due to error backoff.
    ///
    /// Backoff remains authoritative: monitor side effects are skipped while cooldown is active.
    /// The optional `retry_after` value tells callers when status processing can be retried
    /// without re-entering the same suppression path.
    TemporarilyDisabled {
        /// Remaining delay before status processing should be retried.
        retry_after: Option<Duration>,
    },
}

/// Hard upper bound for a single streamer status check to avoid indefinitely-stuck in-flight
/// deduplication entries when upstream requests hang.
const STREAM_CHECK_HARD_TIMEOUT: Duration = Duration::from_secs(300);

/// In-flight deduplication window for per-streamer status checks.
///
/// This is a performance optimization only. If cleanup is delayed, the effect is limited
/// to temporarily reusing the most recent result for the same streamer.
const IN_FLIGHT_DEDUP_WINDOW: Duration = Duration::from_millis(100);

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
            request_timeout: Duration::ZERO,
            max_concurrent_requests: 10,
        }
    }
}

/// The Stream Monitor service.
pub struct StreamMonitor<
    SR: StreamerRepository + Send + Sync + 'static,
    FR: FilterRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
    CR: ConfigRepository + Send + Sync + 'static,
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
    /// Sender for in-flight cleanup requests (single worker processes these).
    cleanup_tx: mpsc::Sender<String>,
    /// Event broadcaster for notifications.
    event_broadcaster: MonitorEventBroadcaster,
    /// Single-owner session lifecycle service. StreamMonitor publishes
    /// Live / Offline observations here; the lifecycle owns the atomic DB
    /// bundle, the in-memory session map, and the `hard_ended` suppression
    /// cache that used to live on this type.
    session_lifecycle: Arc<crate::session::SessionLifecycle>,
    /// Database pool for transactional updates + outbox (serialized write pool).
    write_pool: SqlitePool,
    /// Notifies the outbox publisher that new events are available.
    outbox_notify: Arc<Notify>,
    /// Cancellation token for background tasks (outbox publisher and cleanup worker).
    cancellation: CancellationToken,
    /// Configuration.
    #[allow(dead_code)]
    config: StreamMonitorConfig,
    /// Optional credential refresh service for automatic cookie refresh.
    credential_service: Option<Arc<CredentialRefreshService<CR>>>,
}

/// Details for a streamer going live.
pub(crate) struct LiveStatusDetails {
    pub title: String,
    pub category: Option<String>,
    pub avatar: Option<String>,
    pub streams: Vec<platforms_parser::media::StreamInfo>,
    pub media_headers: Option<std::collections::HashMap<String, String>>,
    pub media_extras: Option<std::collections::HashMap<String, String>>,
}

impl<
    SR: StreamerRepository + Send + Sync + 'static,
    FR: FilterRepository + Send + Sync + 'static,
    SSR: SessionRepository + Send + Sync + 'static,
    CR: ConfigRepository + Send + Sync + 'static,
> StreamMonitor<SR, FR, SSR, CR>
{
    async fn reload_streamer_cache(&self, streamer_id: &str, context: &str) {
        if let Err(error) = self.streamer_manager.reload_from_repo(streamer_id).await {
            warn!(
                "Failed to reload streamer {} after {}: {}. Cache may be stale.",
                streamer_id, context, error
            );
        }
    }

    fn notify_outbox(&self) {
        self.outbox_notify.notify_one();
    }

    /// Create a new stream monitor.
    pub fn new(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config_service: Arc<crate::config::ConfigService<CR, SR>>,
        write_pool: SqlitePool,
        session_lifecycle: Arc<crate::session::SessionLifecycle>,
    ) -> Self {
        Self::with_config(
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            write_pool,
            session_lifecycle,
            StreamMonitorConfig::default(),
        )
    }

    /// Create a new stream monitor with custom configuration.
    pub fn with_config(
        streamer_manager: Arc<StreamerManager<SR>>,
        filter_repo: Arc<FR>,
        session_repo: Arc<SSR>,
        config_service: Arc<crate::config::ConfigService<CR, SR>>,
        write_pool: SqlitePool,
        session_lifecycle: Arc<crate::session::SessionLifecycle>,
        config: StreamMonitorConfig,
    ) -> Self {
        // Create rate limiter with platform-specific configs
        let default_rate_config = RateLimiterConfig::with_rps(config.default_rate_limit)
            .unwrap_or_else(|e| {
                warn!(
                    "Invalid default rate limit {}: {}. Falling back to defaults.",
                    config.default_rate_limit, e
                );
                RateLimiterConfig::default()
            });
        let mut rate_limiter = RateLimiterManager::with_config(default_rate_config);

        for (platform, rps) in &config.platform_rate_limits {
            match RateLimiterConfig::with_rps(*rps) {
                Ok(cfg) => rate_limiter.set_platform_config(platform, cfg),
                Err(e) => {
                    warn!(
                        "Invalid rate limit for platform {} ({}): {}. Skipping override.",
                        platform, rps, e
                    );
                }
            }
        }

        let detector = Arc::new(StreamDetector::with_http_config(
            config.request_timeout,
            config.max_concurrent_requests,
        ));

        // BatchDetector currently uses a single client (not per-streamer); keep the prior client config behavior.
        let mut client_builder = platforms_parser::extractor::create_client_builder(None);

        if config.request_timeout > Duration::ZERO {
            client_builder = client_builder.timeout(config.request_timeout);
        }

        if config.max_concurrent_requests > 0 {
            client_builder = client_builder.pool_max_idle_per_host(config.max_concurrent_requests);
        }

        let client = client_builder.build().unwrap_or_else(|error| {
            warn!(
                "Failed to create HTTP client via platforms-parser: {}. Falling back to reqwest defaults.",
                error
            );
            reqwest::Client::new()
        });

        let batch_detector = BatchDetector::with_client(client, rate_limiter.clone());

        let outbox_notify = Arc::new(Notify::new());
        let cancellation = CancellationToken::new();

        // Create bounded channel for cleanup requests (single worker pattern)
        // Buffer size of 4096 should be plenty for typical concurrent request counts.
        let (cleanup_tx, cleanup_rx) = mpsc::channel::<String>(4096);
        let in_flight = Arc::new(DashMap::new());

        let monitor = Self {
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            detector,
            batch_detector,
            rate_limiter,
            in_flight: in_flight.clone(),
            cleanup_tx,
            event_broadcaster: MonitorEventBroadcaster::new(),
            session_lifecycle,
            write_pool,
            outbox_notify: outbox_notify.clone(),
            cancellation: cancellation.clone(),
            config,
            credential_service: None,
        };

        monitor.spawn_outbox_publisher(outbox_notify, cancellation.clone());
        Self::spawn_cleanup_worker(in_flight, cleanup_rx, cancellation);

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
    /// This cancels the outbox publisher and cleanup worker tasks. Should be called
    /// during graceful shutdown to ensure clean task termination.
    pub fn stop(&self) {
        info!("Stopping StreamMonitor background tasks");
        self.cancellation.cancel();
    }

    /// Set the credential refresh service for automatic cookie refresh.
    pub fn set_credential_service(&mut self, service: Arc<CredentialRefreshService<CR>>) {
        self.credential_service = Some(service);
    }

    /// Spawn a single cleanup worker that processes delayed removal requests.
    fn spawn_cleanup_worker(
        in_flight: Arc<DashMap<String, Arc<OnceCell<LiveStatus>>>>,
        mut cleanup_rx: mpsc::Receiver<String>,
        cancellation_token: CancellationToken,
    ) {
        tokio::spawn(async move {
            let mut queue = DelayQueue::new();
            loop {
                tokio::select! {
                    biased;

                    _ = cancellation_token.cancelled() => {
                        debug!("In-flight cleanup worker shutting down");
                        break;
                    }
                    Some(id) = cleanup_rx.recv() => {
                        queue.insert(id, IN_FLIGHT_DEDUP_WINDOW);
                    }
                    Some(expired) = queue.next() => {
                        let id = expired.into_inner();
                        in_flight.remove(&id);
                    }
                }
            }
            debug!("In-flight cleanup worker stopped");
        });
    }

    fn spawn_outbox_publisher(
        &self,
        outbox_notify: Arc<Notify>,
        cancellation_token: CancellationToken,
    ) {
        let pool = self.write_pool.clone();
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

                if let Err(e) =
                    flush_outbox_until_wait(&pool, &broadcaster, &cancellation_token).await
                {
                    warn!("Monitor outbox flush failed: {}", e);
                }
            }
            debug!("Outbox publisher stopped");
        });
    }

    /// Start an immediate transaction to prevent locking issues.
    async fn begin_immediate(&self) -> Result<ImmediateTransaction> {
        retry_on_sqlite_busy("monitor_begin_immediate", || async {
            crate::database::begin_immediate(&self.write_pool)
                .await
                .map_err(Into::into)
        })
        .await
    }

    /// Check the status of a single streamer.
    ///
    /// This method deduplicates concurrent requests for the same streamer.
    /// If multiple requests come in for the same streamer simultaneously,
    /// only one will perform the actual HTTP check and others will wait
    /// for and share the result.
    pub async fn check_streamer(&self, streamer: &StreamerMetadata) -> Result<LiveStatus> {
        trace!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            "monitor status check"
        );

        // Correctness guard: if the streamer was disabled via API, we should not perform checks.
        // This avoids wasted network calls and prevents races where in-flight checks could
        // produce events/state updates after a user-initiated disable.
        if let Some(fresh) = self.streamer_manager.get_streamer(&streamer.id)
            && fresh.state == StreamerState::Disabled
        {
            debug!(
                streamer_id = %streamer.id,
                "streamer disabled; skipping status check"
            );
            return Ok(LiveStatus::Offline);
        }

        let hard_timeout = if self.config.request_timeout > Duration::ZERO {
            self.config.request_timeout
        } else {
            STREAM_CHECK_HARD_TIMEOUT
        };

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
        let credential_service = self.credential_service.clone();
        let streamer_id_owned = streamer.id.clone();
        let streamer_id = streamer.id.as_str();
        let platform_id = streamer.platform();

        // get_or_try_init ensures only ONE caller executes the closure,
        // all other concurrent callers wait for the result
        let result = cell
            .get_or_try_init(|| async move {
                // Acquire rate limit token
                let wait_time = rate_limiter.acquire(platform_id).await;
                if !wait_time.is_zero() {
                    debug!(
                        platform_id = %platform_id,
                        streamer_id = %streamer_id_owned,
                        wait = ?wait_time,
                        "rate limited"
                    );
                }

                let check = async {
                    // Load filters for this streamer
                    let filter_models = filter_repo.get_by_streamer(streamer_id).await?;
                    let filters: Vec<Filter> = filter_models
                        .into_iter()
                        .filter_map(|model| match Filter::try_from(&model) {
                            Ok(filter) => Some(filter),
                            Err(error) => {
                                warn!(
                                    filter_id = %model.id,
                                    filter_type = %model.filter_type,
                                    error = %error,
                                    "Skipping invalid streamer filter"
                                );
                                None
                            }
                        })
                        .collect();

                    // Get resolved context (merged config + credential source provenance).
                    let context = config_service.get_context_for_streamer(streamer_id).await?;
                    let config = &context.config;

                    // Use cookies from config, but attempt best-effort refresh first (non-fatal).
                    let mut cookies = config.cookies.clone();
                    if let Some(ref credential_service) = credential_service
                        && let Some(ref source) = context.credential_source
                    {
                        match credential_service.check_and_refresh_source(source).await {
                            Ok(Some(new_cookies)) => {
                                // Use refreshed cookies immediately for this check, and invalidate
                                // cached config so subsequent reads pick up the DB update.
                                cookies = Some(new_cookies);
                                match &source.scope {
                                    crate::credentials::CredentialScope::Streamer { .. } => {
                                        config_service.invalidate_streamer(streamer_id);
                                    }
                                    crate::credentials::CredentialScope::Template {
                                        template_id,
                                        ..
                                    } => {
                                        if let Err(e) = config_service
                                            .invalidate_template(template_id)
                                            .await
                                        {
                                            warn!(
                                                error = %e,
                                                "Failed to invalidate template configs after credential refresh"
                                            );
                                        }
                                    }
                                    crate::credentials::CredentialScope::Platform {
                                        platform_id,
                                        ..
                                    } => {
                                        if let Err(e) = config_service
                                            .invalidate_platform(platform_id)
                                            .await
                                        {
                                            warn!(
                                                error = %e,
                                                "Failed to invalidate platform configs after credential refresh"
                                            );
                                        }
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    "Credential check/refresh failed; continuing with existing cookies"
                                );
                            }
                        }
                    }

                    // Check status with filters, cookies, selection config, and platform extras
                    detector
                        .check_status_with_filters(
                            streamer,
                            &filters,
                            cookies,
                            Some(&config.stream_selection),
                            config.platform_extras.clone(),
                            &config.proxy_config,
                        )
                        .await
                };

                tokio::time::timeout(hard_timeout, check)
                    .await
                    .map_err(|_| {
                        Error::Monitor(format!(
                            "Stream monitor check timed out after {:?} (streamer_id={})",
                            hard_timeout, streamer_id_owned
                        ))
                    })?
            })
            .await;

        // Schedule delayed cleanup BEFORE checking result to ensure cleanup
        // happens regardless of success or error. This prevents in_flight map
        // from leaking entries when errors occur.
        // Use the cleanup worker (DelayQueue) to avoid spawning per-entry tasks.
        // If the channel is saturated, fall back to immediate cleanup (dedup is best-effort).
        if self.cleanup_tx.try_send(streamer.id.clone()).is_err() {
            self.in_flight.remove(&streamer.id);
        }

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
    ) -> Result<ProcessStatusResult> {
        debug!(
            "Processing status for {}: {:?}",
            streamer.id,
            status_summary(&status)
        );

        // Fetch fresh metadata to ensure we have the latest state
        // The StreamerActor might be holding stale metadata
        let fresh_streamer = self.streamer_manager.get_streamer(&streamer.id);
        let streamer = fresh_streamer.as_ref().unwrap_or(streamer);

        // Correctness guard: user-disabled streamers must not be reactivated by in-flight checks.
        // This also suppresses monitor events (StreamerLive/Offline/Error) after disable.
        if streamer.state == StreamerState::Disabled {
            debug!(
                "Ignoring monitor status for disabled streamer {}: {:?}",
                streamer.id,
                status_summary(&status)
            );
            return Ok(ProcessStatusResult::Suppressed(
                ProcessStatusSuppression::Disabled,
            ));
        }

        if streamer.is_disabled() {
            debug!(
                streamer_id = %streamer.id,
                streamer_name = %streamer.name,
                disabled_until = ?streamer.disabled_until,
                "Ignoring monitor status while temporarily disabled"
            );
            return Ok(ProcessStatusResult::Suppressed(
                ProcessStatusSuppression::TemporarilyDisabled {
                    retry_after: streamer.remaining_backoff_std(),
                },
            ));
        }

        match status {
            LiveStatus::Live {
                title,
                category,
                avatar,
                started_at: _,
                streams,
                media_headers,
                media_extras,
                ..
            } => {
                // Persist SOOP (etc.) session cookies minted by reactive login
                // during extract so the next poll can reuse them.
                if let Some(session_cookies) = media_extras
                    .as_ref()
                    .and_then(|e| e.get("session_cookies"))
                    .map(|s| s.as_str())
                    .filter(|s| !s.is_empty())
                    && let Some(ref credential_service) = self.credential_service
                {
                    match self
                        .config_service
                        .get_context_for_streamer(&streamer.id)
                        .await
                    {
                        Ok(context) => {
                            if let Some(ref source) = context.credential_source {
                                if let Err(e) = credential_service
                                    .persist_session_cookies(source, session_cookies.to_string())
                                    .await
                                {
                                    warn!(
                                        error = %e,
                                        streamer_id = %streamer.id,
                                        "Failed to persist session cookies from extract"
                                    );
                                } else {
                                    match &source.scope {
                                        crate::credentials::CredentialScope::Streamer {
                                            ..
                                        } => {
                                            self.config_service.invalidate_streamer(&streamer.id);
                                        }
                                        crate::credentials::CredentialScope::Template {
                                            template_id,
                                            ..
                                        } => {
                                            let _ = self
                                                .config_service
                                                .invalidate_template(template_id)
                                                .await;
                                        }
                                        crate::credentials::CredentialScope::Platform {
                                            platform_id,
                                            ..
                                        } => {
                                            let _ = self
                                                .config_service
                                                .invalidate_platform(platform_id)
                                                .await;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                streamer_id = %streamer.id,
                                "Failed to load context while persisting session cookies"
                            );
                        }
                    }
                }

                self.handle_live(
                    streamer,
                    LiveStatusDetails {
                        title,
                        category,
                        avatar,
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

        Ok(ProcessStatusResult::Applied)
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
            streams,
            media_headers,
            media_extras,
        } = details;
        info!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            title = %title,
            streams = streams.len(),
            media_headers = media_headers.as_ref().map_or(0, |h| h.len()),
            "status=LIVE (monitor)"
        );

        let now = chrono::Utc::now();

        // Delegate the atomic session+streamer+outbox bundle to
        // `SessionLifecycle`. It owns the in-memory session map and the
        // `hard_ended` suppression cache, and emits `SessionTransition::Started`
        // on success.
        self.session_lifecycle
            .on_live_detected(crate::session::LiveDetectedArgs {
                streamer_id: &streamer.id,
                streamer_name: &streamer.name,
                streamer_url: &streamer.url,
                current_avatar: streamer.avatar_url.as_deref(),
                new_avatar: avatar.as_deref(),
                title: &title,
                category: category.as_deref(),
                streams: &streams,
                media_headers: media_headers.as_ref(),
                media_extras: media_extras.as_ref(),
                now,
            })
            .await?;

        self.reload_streamer_cache(&streamer.id, "state update")
            .await;
        self.notify_outbox();

        Ok(())
    }

    /// Handle a streamer going offline.
    async fn handle_offline(&self, streamer: &StreamerMetadata) -> Result<()> {
        // Monitor-driven offline carries no engine-boundary signal; the
        // platform's status check is the source of truth here.
        self.handle_offline_with_session(streamer, None, None).await
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
    /// | TemporalDisabled     | Yes         | End active session if any, set NOT_LIVE, clear errors |
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
        signal: Option<crate::session::state::OfflineSignal>,
    ) -> Result<()> {
        trace!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            signal = signal.as_ref().map(|s| s.as_str()).unwrap_or("(none)"),
            "status=OFFLINE (monitor)"
        );

        // Check if we have accumulated errors that should be cleared on successful check
        let has_errors = streamer.consecutive_error_count > 0 || streamer.disabled_until.is_some();

        if streamer.state == StreamerState::Live
            || streamer.state == StreamerState::TemporalDisabled
        {
            let now = chrono::Utc::now();

            let outcome = self
                .session_lifecycle
                .on_offline_detected(crate::session::OfflineDetectedArgs {
                    streamer_id: &streamer.id,
                    streamer_name: &streamer.name,
                    session_id: session_id.as_deref(),
                    state_was_live: streamer.state == StreamerState::Live,
                    clear_errors: has_errors,
                    signal,
                    now,
                })
                .await?;

            if outcome.offline_event_emitted {
                info!(
                    streamer_id = %streamer.id,
                    streamer_name = %streamer.name,
                    streamer_url = %streamer.url,
                    "status=OFFLINE (monitor)"
                );
            }

            self.reload_streamer_cache(&streamer.id, "offline update")
                .await;
            if outcome.offline_event_emitted {
                self.notify_outbox();
            }
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

            self.reload_streamer_cache(&streamer.id, "clearing error state")
                .await;
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
        let is_out_of_schedule = matches!(&reason, FilterReason::OutOfSchedule { .. });
        let (new_state, state_change_reason) = match &reason {
            FilterReason::OutOfSchedule { .. } => (
                StreamerState::OutOfSchedule,
                Some("out_of_schedule".to_string()),
            ),
            FilterReason::TitleMismatch => {
                // For title/category mismatch, we still consider it "out of schedule".
                (
                    StreamerState::OutOfSchedule,
                    Some("title_mismatch".to_string()),
                )
            }
            FilterReason::CategoryMismatch => {
                // For title/category mismatch, we still consider it "out of schedule".
                (
                    StreamerState::OutOfSchedule,
                    Some("category_mismatch".to_string()),
                )
            }
        };

        if streamer.state == new_state && !is_out_of_schedule {
            return Ok(());
        }

        if is_out_of_schedule
            && streamer.state == StreamerState::OutOfSchedule
            && !self
                .session_lifecycle
                .has_active_session_for_streamer(&streamer.id)
            && self
                .session_repo
                .get_active_session_for_streamer(&streamer.id)
                .await?
                .is_none()
        {
            debug!(
                streamer_id = %streamer.id,
                streamer_name = %streamer.name,
                "out-of-schedule recheck already settled; skipping lifecycle write"
            );
            return Ok(());
        }

        debug!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            reason = ?reason,
            new_state = ?new_state,
            "filtered; updating state"
        );

        if is_out_of_schedule {
            self.session_lifecycle
                .end_for_out_of_schedule(&streamer.id, &streamer.name, streamer.state)
                .await?;

            self.reload_streamer_cache(&streamer.id, "state update")
                .await;
            self.notify_outbox();

            return Ok(());
        }

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
            reason: state_change_reason,
            timestamp: now,
        };
        MonitorOutboxTxOps::enqueue_event(&mut tx, &streamer.id, &event).await?;

        tx.commit().await?;

        self.reload_streamer_cache(&streamer.id, "state update")
            .await;
        self.notify_outbox();

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
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            reason = %reason,
            new_state = ?new_state,
            "fatal; updating state"
        );

        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        let _ = SessionTxOps::end_active_session(&mut tx, &streamer.id, now).await?;

        // Update state to the fatal error state and persist the reason
        StreamerTxOps::set_fatal_error(&mut tx, &streamer.id, &new_state.to_string(), reason)
            .await?;

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

        self.reload_streamer_cache(&streamer.id, "state update")
            .await;
        self.notify_outbox();

        Ok(())
    }

    /// Handle an error during status check.
    pub async fn handle_error(&self, streamer: &StreamerMetadata, error: &str) -> Result<()> {
        warn!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            streamer_url = %streamer.url,
            error = %error,
            "status check failed"
        );

        // If the user disabled the streamer, don't mutate error/backoff state or emit error events.
        // Disable is a user intent override, and we don't want in-flight checks to keep writing DB.
        if let Some(fresh) = self.streamer_manager.get_streamer(&streamer.id)
            && fresh.state == StreamerState::Disabled
        {
            debug!(
                streamer_id = %streamer.id,
                error = %error,
                "skipping error handling for disabled streamer"
            );
            return Ok(());
        }

        let now = chrono::Utc::now();

        let mut tx = self.begin_immediate().await?;

        let new_error_count = StreamerTxOps::increment_error(&mut tx, &streamer.id, error).await?;

        let disabled_until = self
            .streamer_manager
            .disabled_until_for_error_count(new_error_count);

        StreamerTxOps::set_disabled_until(&mut tx, &streamer.id, disabled_until).await?;

        if let Some(until) = disabled_until {
            info!(
                streamer_id = %streamer.id,
                streamer_name = %streamer.name,
                until = %until,
                consecutive_errors = new_error_count,
                "temporarily disabled (error backoff)"
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

        self.reload_streamer_cache(&streamer.id, "state update")
            .await;
        self.notify_outbox();

        Ok(())
    }

    /// Set a streamer to temporarily disabled due to an infrastructure-level
    /// block (circuit breaker, output-root gate, etc.).
    ///
    /// Writes `disabled_until` and the appropriate state/last_error without
    /// incrementing the error count — infrastructure issues are not counted
    /// against the streamer's own exponential backoff.
    ///
    /// See [`InfraBlockReason`] for the per-reason behavior.
    pub async fn set_infra_blocked(
        &self,
        streamer: &StreamerMetadata,
        reason: InfraBlockReason,
    ) -> Result<()> {
        // If the user disabled the streamer, don't mutate backoff state.
        if let Some(fresh) = self.streamer_manager.get_streamer(&streamer.id)
            && fresh.state == StreamerState::Disabled
        {
            debug!(
                streamer_id = %streamer.id,
                reason = reason.as_log_str(),
                "skipping infra block for disabled streamer"
            );
            return Ok(());
        }

        let now = chrono::Utc::now();
        let disabled_until = now + chrono::Duration::seconds(reason.retry_after_secs() as i64);
        let target_state = reason.target_state();
        let last_error_update = reason.last_error_override();

        info!(
            streamer_id = %streamer.id,
            streamer_name = %streamer.name,
            disabled_until = %disabled_until,
            retry_after_secs = reason.retry_after_secs(),
            reason = reason.as_log_str(),
            target_state = ?target_state,
            "temporarily disabled (infra block)"
        );

        let mut tx = self.begin_immediate().await?;

        // set_disabled_until writes state = TEMPORAL_DISABLED by default. For
        // reasons that require a different state (e.g. OutOfSpace for the
        // gate) we override with an explicit update_state call in the same tx.
        StreamerTxOps::set_disabled_until(&mut tx, &streamer.id, Some(disabled_until)).await?;
        if target_state != StreamerState::TemporalDisabled {
            StreamerTxOps::update_state(&mut tx, &streamer.id, &target_state.to_string()).await?;
        }

        // For reasons that carry a distinctive last_error marker (used later
        // by the gate's recovery hook to filter which streamers to reset),
        // write it now. Circuit breaker blocks leave last_error alone so any
        // recent legitimate error text is preserved.
        if let Some(msg) = last_error_update {
            StreamerTxOps::update_last_error(&mut tx, &streamer.id, Some(&msg)).await?;
        }

        tx.commit().await?;

        self.reload_streamer_cache(&streamer.id, "infra block")
            .await;

        Ok(())
    }
}

/// Reason a streamer is being put into infrastructure-level backoff via
/// [`StreamMonitor::set_infra_blocked`]. The reason determines the target
/// [`StreamerState`], the retry window, and whether `last_error` is rewritten.
#[derive(Debug, Clone)]
pub enum InfraBlockReason {
    /// Engine circuit breaker is open for the engine this streamer is trying
    /// to use. Preserves whatever `last_error` is already on the streamer
    /// (the CDN error that tripped the breaker is usually what the user
    /// actually wants to see).
    CircuitBreaker { retry_after_secs: u64 },
    /// Output-root write gate has the target filesystem in a Degraded state.
    /// Transitions the streamer to [`StreamerState::OutOfSpace`] and writes a
    /// distinctive `last_error` with the `"output-root blocked: "` prefix so
    /// the gate's recovery hook can filter affected streamers when the
    /// filesystem comes back.
    OutputRootUnavailable {
        path: std::path::PathBuf,
        io_kind: crate::downloader::IoErrorKindSer,
        retry_after_secs: u64,
    },
}

impl InfraBlockReason {
    fn retry_after_secs(&self) -> u64 {
        match self {
            Self::CircuitBreaker { retry_after_secs } => *retry_after_secs,
            Self::OutputRootUnavailable {
                retry_after_secs, ..
            } => *retry_after_secs,
        }
    }

    fn target_state(&self) -> StreamerState {
        match self {
            Self::CircuitBreaker { .. } => StreamerState::TemporalDisabled,
            Self::OutputRootUnavailable { .. } => StreamerState::OutOfSpace,
        }
    }

    fn last_error_override(&self) -> Option<String> {
        match self {
            Self::CircuitBreaker { .. } => None,
            Self::OutputRootUnavailable { path, io_kind, .. } => {
                // This string format is load-bearing: the gate recovery hook
                // filters streamers by matching the `LAST_ERROR_GATE_PREFIX`
                // prefix. If you change the prefix here, also update
                // `crate::downloader::output_root_gate::LAST_ERROR_GATE_PREFIX`.
                Some(format!(
                    "{} {} ({})",
                    crate::downloader::LAST_ERROR_GATE_PREFIX,
                    path.display(),
                    io_kind.as_str()
                ))
            }
        }
    }

    fn as_log_str(&self) -> &'static str {
        match self {
            Self::CircuitBreaker { .. } => "circuit_breaker",
            Self::OutputRootUnavailable { .. } => "output_root_unavailable",
        }
    }
}

const OUTBOX_NO_RECEIVER_MAX_ATTEMPTS: i64 = 5;
const OUTBOX_NO_RECEIVER_MAX_AGE_SECS: i64 = 30;

struct FlushOutboxResult {
    fetched: usize,
    needs_wait: bool,
}

async fn flush_outbox_until_wait(
    pool: &SqlitePool,
    broadcaster: &MonitorEventBroadcaster,
    cancellation_token: &CancellationToken,
) -> Result<()> {
    loop {
        if cancellation_token.is_cancelled() {
            return Ok(());
        }

        let result = flush_outbox_once(pool, broadcaster).await?;
        if result.fetched == 0 || result.needs_wait {
            return Ok(());
        }
    }
}
async fn flush_outbox_once(
    pool: &SqlitePool,
    broadcaster: &MonitorEventBroadcaster,
) -> Result<FlushOutboxResult> {
    let entries = MonitorOutboxOps::fetch_undelivered(pool, 100).await?;

    if entries.is_empty() {
        return Ok(FlushOutboxResult {
            fetched: 0,
            needs_wait: false,
        });
    }

    let fetched = entries.len();
    let mut needs_wait = false;
    let mut delivered_ids = Vec::with_capacity(fetched);
    let mut failed_entries: Vec<(i64, String)> = Vec::new();

    for entry in entries {
        match serde_json::from_str::<MonitorEvent>(&entry.payload) {
            Ok(event) => {
                // Attempt to broadcast the event
                match broadcaster.publish(event) {
                    Ok(receiver_count) => {
                        // Event was successfully sent to `receiver_count` receivers
                        debug!("Published event to {} receivers", receiver_count);
                        delivered_ids.push(entry.id);
                    }
                    Err(e) => {
                        // No receivers available - keep events briefly to handle listener startup.
                        let now = chrono::Utc::now();
                        let age_secs = now
                            .signed_duration_since(crate::database::time::ms_to_datetime(
                                entry.created_at,
                            ))
                            .num_seconds();

                        let should_drop = entry.attempts >= OUTBOX_NO_RECEIVER_MAX_ATTEMPTS
                            || age_secs >= OUTBOX_NO_RECEIVER_MAX_AGE_SECS;

                        if should_drop {
                            warn!(
                                "Monitor outbox event id={} has no receivers after {} attempts or {}s, discarding: {:?}",
                                entry.id, entry.attempts, age_secs, e.0
                            );
                            delivered_ids.push(entry.id);
                        } else {
                            debug!(
                                "Monitor outbox event id={} has no receivers, retrying later",
                                entry.id
                            );
                            failed_entries.push((entry.id, "no receivers".to_string()));
                            needs_wait = true;
                        }
                    }
                }
            }
            Err(e) => {
                // A JSON parse failure is not recoverable, and will otherwise permanently
                // poison the outbox head-of-line (ORDER BY id). Discard it.
                warn!(
                    "Invalid monitor outbox payload id={}, discarding: {}",
                    entry.id, e
                );
                delivered_ids.push(entry.id);
            }
        }
    }

    MonitorOutboxOps::mark_delivered_batch(pool, &delivered_ids).await?;
    MonitorOutboxOps::record_failure_batch(pool, &failed_entries).await?;

    Ok(FlushOutboxResult {
        fetched,
        needs_wait,
    })
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

    use std::sync::Arc;

    use sqlx::SqlitePool;

    use crate::config::{ConfigEventBroadcaster, ConfigService};
    use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
    use crate::database::repositories::{
        MonitorOutboxOps, SessionEventRepository as _, SqlxConfigRepository, SqlxFilterRepository,
        SqlxSessionEventRepository, SqlxSessionRepository, SqlxStreamerRepository,
    };
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::session::{SessionEventPayload, TerminalCauseDto};
    use crate::streamer::StreamerManager;

    async fn setup_monitor_test_db() -> SqlitePool {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    async fn build_test_monitor(
        pool: &SqlitePool,
    ) -> StreamMonitor<
        SqlxStreamerRepository,
        SqlxFilterRepository,
        SqlxSessionRepository,
        SqlxConfigRepository,
    > {
        let session_lifecycle = Arc::new(crate::session::SessionLifecycle::with_default_capacity(
            Arc::new(crate::database::repositories::SessionLifecycleRepository::new(pool.clone())),
            Arc::new(crate::session::OfflineClassifier::new()),
        ));

        build_test_monitor_with_lifecycle(pool, session_lifecycle).await
    }

    async fn build_test_monitor_with_lifecycle(
        pool: &SqlitePool,
        session_lifecycle: Arc<crate::session::SessionLifecycle>,
    ) -> StreamMonitor<
        SqlxStreamerRepository,
        SqlxFilterRepository,
        SqlxSessionRepository,
        SqlxConfigRepository,
    > {
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone()));
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), pool.clone()));
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            ConfigEventBroadcaster::new(),
        ));
        streamer_manager.hydrate().await.unwrap();
        let config_service = Arc::new(ConfigService::new(config_repo, streamer_repo));

        StreamMonitor::new(
            streamer_manager,
            filter_repo,
            session_repo,
            config_service,
            pool.clone(),
            session_lifecycle,
        )
    }

    async fn insert_streamer(
        pool: &SqlitePool,
        id: &str,
        state: StreamerState,
        consecutive_errors: i32,
        disabled_until: Option<i64>,
    ) {
        let mut streamer = StreamerDbModel::new(
            "Test Streamer",
            format!("https://example.com/{id}"),
            "platform-twitch",
        );
        streamer.id = id.to_string();
        streamer.state = state.to_string();
        streamer.consecutive_error_count = Some(consecutive_errors);
        streamer.disabled_until = disabled_until;
        streamer.last_error = Some("boom".to_string());

        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
    }

    async fn insert_active_session(pool: &SqlitePool, session_id: &str, streamer_id: &str) {
        let mut session = LiveSessionDbModel::new(streamer_id);
        session.id = session_id.to_string();

        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .create_session(&session)
            .await
            .unwrap();
    }

    async fn get_session(pool: &SqlitePool, session_id: &str) -> LiveSessionDbModel {
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .get_session(session_id)
            .await
            .unwrap()
    }

    async fn get_streamer(pool: &SqlitePool, streamer_id: &str) -> StreamerDbModel {
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .get_streamer(streamer_id)
            .await
            .unwrap()
    }

    async fn get_active_session_for_streamer(
        pool: &SqlitePool,
        streamer_id: &str,
    ) -> Option<LiveSessionDbModel> {
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .get_active_session_for_streamer(streamer_id)
            .await
            .unwrap()
    }

    async fn latest_session_ended_payload(
        pool: &SqlitePool,
        session_id: &str,
    ) -> Option<SessionEventPayload> {
        SqlxSessionEventRepository::new(pool.clone(), pool.clone())
            .list_for_session(session_id)
            .await
            .unwrap()
            .into_iter()
            .rev()
            .find(|row| row.kind == "session_ended")
            .and_then(|row| row.payload)
    }

    fn session_ended_cause(payload: &SessionEventPayload) -> &TerminalCauseDto {
        let SessionEventPayload::SessionEnded { cause, .. } = payload else {
            panic!("expected session_ended payload, got {payload:?}");
        };
        cause
    }

    async fn session_event_count_for_streamer(pool: &SqlitePool, streamer_id: &str) -> usize {
        SqlxSessionEventRepository::new(pool.clone(), pool.clone())
            .list_for_streamer(streamer_id)
            .await
            .unwrap()
            .len()
    }

    async fn outbox_events(pool: &SqlitePool) -> Vec<MonitorEvent> {
        MonitorOutboxOps::fetch_undelivered(pool, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|entry| serde_json::from_str(&entry.payload).expect("outbox event deserialises"))
            .collect()
    }

    fn outbox_event_type(event: &MonitorEvent) -> &'static str {
        match event {
            MonitorEvent::StreamerLive { .. } => "StreamerLive",
            MonitorEvent::StreamerOffline { .. } => "StreamerOffline",
            MonitorEvent::FatalError { .. } => "FatalError",
            MonitorEvent::TransientError { .. } => "TransientError",
            MonitorEvent::StateChanged { .. } => "StateChanged",
        }
    }

    #[test]
    fn test_stream_monitor_config_default() {
        let config = StreamMonitorConfig::default();
        assert_eq!(config.default_rate_limit, 1.0);
        assert_eq!(config.request_timeout, Duration::ZERO);
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
                is_audio_only: false,
            }],
            media_headers: None,
            media_extras: None,
            next_check_hint: None,
            candidates: vec![],
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

    #[tokio::test]
    async fn test_handle_offline_from_temporal_disabled_ends_active_session_and_enqueues_offline() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-1",
            StreamerState::TemporalDisabled,
            3,
            Some(chrono::Utc::now().timestamp_millis() + 60_000),
        )
        .await;
        insert_active_session(&pool, "session-1", "streamer-1").await;

        let monitor = build_test_monitor(&pool).await;
        let streamer = monitor.streamer_manager.get_streamer("streamer-1").unwrap();

        monitor
            .handle_offline_with_session(&streamer, None, None)
            .await
            .unwrap();

        assert!(get_session(&pool, "session-1").await.end_time.is_some());

        let streamer = get_streamer(&pool, "streamer-1").await;
        assert_eq!(streamer.state, "NOT_LIVE");
        assert_eq!(streamer.consecutive_error_count, Some(0));
        assert!(streamer.disabled_until.is_none());
        assert!(streamer.last_error.is_none());

        let events = outbox_events(&pool).await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            MonitorEvent::StreamerOffline { session_id, .. } => {
                assert_eq!(session_id.as_deref(), Some("session-1"));
            }
            other => panic!("expected StreamerOffline, got {other:?}"),
        }

        monitor.stop();
    }

    #[tokio::test]
    async fn test_handle_offline_from_temporal_disabled_without_session_only_clears_errors() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-2",
            StreamerState::TemporalDisabled,
            2,
            Some(chrono::Utc::now().timestamp_millis() + 60_000),
        )
        .await;

        let monitor = build_test_monitor(&pool).await;
        let streamer = monitor.streamer_manager.get_streamer("streamer-2").unwrap();

        monitor
            .handle_offline_with_session(&streamer, None, None)
            .await
            .unwrap();

        let streamer = get_streamer(&pool, "streamer-2").await;
        assert_eq!(streamer.state, "NOT_LIVE");
        assert_eq!(streamer.consecutive_error_count, Some(0));
        assert!(streamer.disabled_until.is_none());
        assert!(streamer.last_error.is_none());

        assert!(
            get_active_session_for_streamer(&pool, "streamer-2")
                .await
                .is_none()
        );

        assert!(outbox_events(&pool).await.is_empty());

        monitor.stop();
    }

    #[tokio::test]
    async fn test_process_status_offline_from_temporal_disabled_closes_active_session() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-3",
            StreamerState::TemporalDisabled,
            3,
            Some(chrono::Utc::now().timestamp_millis() - 1),
        )
        .await;
        insert_active_session(&pool, "session-3", "streamer-3").await;

        let monitor = build_test_monitor(&pool).await;
        let streamer = monitor.streamer_manager.get_streamer("streamer-3").unwrap();

        let outcome = monitor
            .process_status(&streamer, LiveStatus::Offline)
            .await
            .unwrap();
        assert_eq!(outcome, ProcessStatusResult::Applied);

        assert!(get_session(&pool, "session-3").await.end_time.is_some());

        let streamer = get_streamer(&pool, "streamer-3").await;
        assert_eq!(streamer.state, "NOT_LIVE");
        assert_eq!(streamer.consecutive_error_count, Some(0));
        assert!(streamer.disabled_until.is_none());
        assert!(streamer.last_error.is_none());

        let events = outbox_events(&pool).await;
        assert_eq!(events.len(), 1);
        match &events[0] {
            MonitorEvent::StreamerOffline { session_id, .. } => {
                assert_eq!(session_id.as_deref(), Some("session-3"));
            }
            other => panic!("expected StreamerOffline, got {other:?}"),
        }

        monitor.stop();
    }

    #[tokio::test]
    async fn test_process_status_out_of_schedule_ends_session_via_lifecycle() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "streamer-4", StreamerState::NotLive, 0, None).await;

        let monitor = build_test_monitor(&pool).await;
        let mut rx = monitor.session_lifecycle.subscribe();
        let streams = Vec::new();

        monitor
            .session_lifecycle
            .on_live_detected(crate::session::LiveDetectedArgs {
                streamer_id: "streamer-4",
                streamer_name: "Test Streamer",
                streamer_url: "https://example.com/streamer-4",
                current_avatar: None,
                new_avatar: None,
                title: "Schedule Live",
                category: None,
                streams: &streams,
                media_headers: None,
                media_extras: None,
                now: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let started = rx.recv().await.unwrap();
        let session_id = match started {
            crate::session::SessionTransition::Started { session_id, .. } => session_id,
            other => panic!("expected Started, got {other:?}"),
        };

        monitor
            .streamer_manager
            .reload_from_repo("streamer-4")
            .await
            .unwrap();
        let live_streamer = monitor.streamer_manager.get_streamer("streamer-4").unwrap();
        monitor
            .process_status(
                &live_streamer,
                LiveStatus::Filtered {
                    reason: FilterReason::OutOfSchedule {
                        next_available: None,
                    },
                    title: "Schedule Live".to_string(),
                    category: None,
                },
            )
            .await
            .unwrap();

        assert!(get_session(&pool, &session_id).await.end_time.is_some());

        assert_eq!(
            get_streamer(&pool, "streamer-4").await.state,
            "OUT_OF_SCHEDULE"
        );

        match rx.recv().await.unwrap() {
            crate::session::SessionTransition::Ended {
                session_id: ended_id,
                cause,
                ..
            } => {
                assert_eq!(ended_id, session_id);
                assert_eq!(cause, crate::session::TerminalCause::OutOfSchedule);
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        let payload = latest_session_ended_payload(&pool, &session_id)
            .await
            .unwrap();
        assert_eq!(
            session_ended_cause(&payload),
            &TerminalCauseDto::OutOfSchedule
        );

        let events = outbox_events(&pool).await;
        let event_types: Vec<&str> = events.iter().map(outbox_event_type).collect();
        assert_eq!(event_types, vec!["StreamerLive", "StateChanged"]);

        match &events[1] {
            MonitorEvent::StateChanged { reason, .. } => {
                assert_eq!(reason.as_deref(), Some("out_of_schedule"));
            }
            other => panic!("expected StateChanged, got {other:?}"),
        }

        monitor.stop();
    }

    #[tokio::test]
    async fn test_out_of_schedule_recheck_closes_active_session_even_when_state_matches() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(&pool, "streamer-5", StreamerState::OutOfSchedule, 0, None).await;

        let monitor = build_test_monitor(&pool).await;
        let mut rx = monitor.session_lifecycle.subscribe();
        let streams = Vec::new();

        monitor
            .session_lifecycle
            .on_live_detected(crate::session::LiveDetectedArgs {
                streamer_id: "streamer-5",
                streamer_name: "Test Streamer",
                streamer_url: "https://example.com/streamer-5",
                current_avatar: None,
                new_avatar: None,
                title: "Schedule Live",
                category: None,
                streams: &streams,
                media_headers: None,
                media_extras: None,
                now: chrono::Utc::now(),
            })
            .await
            .unwrap();

        let started = rx.recv().await.unwrap();
        let session_id = match started {
            crate::session::SessionTransition::Started { session_id, .. } => session_id,
            other => panic!("expected Started, got {other:?}"),
        };

        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .update_streamer_state("streamer-5", StreamerState::OutOfSchedule.as_str())
            .await
            .unwrap();
        monitor
            .streamer_manager
            .reload_from_repo("streamer-5")
            .await
            .unwrap();

        let already_out = monitor.streamer_manager.get_streamer("streamer-5").unwrap();
        assert_eq!(already_out.state, StreamerState::OutOfSchedule);

        monitor
            .process_status(
                &already_out,
                LiveStatus::Filtered {
                    reason: FilterReason::OutOfSchedule {
                        next_available: None,
                    },
                    title: "Schedule Live".to_string(),
                    category: None,
                },
            )
            .await
            .unwrap();

        assert!(get_session(&pool, &session_id).await.end_time.is_some());

        match rx.recv().await.unwrap() {
            crate::session::SessionTransition::Ended { cause, .. } => {
                assert_eq!(cause, crate::session::TerminalCause::OutOfSchedule);
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        monitor.stop();
    }

    #[tokio::test]
    async fn test_out_of_schedule_recheck_without_active_session_skips_lifecycle_write() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-oos-settled",
            StreamerState::OutOfSchedule,
            0,
            None,
        )
        .await;

        let monitor = build_test_monitor(&pool).await;
        let mut rx = monitor.session_lifecycle.subscribe();
        let already_out = monitor
            .streamer_manager
            .get_streamer("streamer-oos-settled")
            .unwrap();
        let before_session_events =
            session_event_count_for_streamer(&pool, "streamer-oos-settled").await;
        let before_outbox_events = outbox_events(&pool).await.len();

        let outcome = monitor
            .process_status(
                &already_out,
                LiveStatus::Filtered {
                    reason: FilterReason::OutOfSchedule {
                        next_available: None,
                    },
                    title: "Still Out".to_string(),
                    category: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome, ProcessStatusResult::Applied);
        assert!(
            rx.try_recv().is_err(),
            "settled recheck must not emit a lifecycle transition"
        );
        assert!(
            get_active_session_for_streamer(&pool, "streamer-oos-settled")
                .await
                .is_none()
        );
        assert_eq!(
            session_event_count_for_streamer(&pool, "streamer-oos-settled").await,
            before_session_events,
            "settled recheck must not write duplicate session_events rows"
        );
        assert_eq!(
            outbox_events(&pool).await.len(),
            before_outbox_events,
            "settled recheck must not enqueue duplicate outbox events"
        );

        monitor.stop();
    }

    #[tokio::test]
    async fn test_out_of_schedule_recheck_closes_cold_start_active_session_when_state_matches() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-oos-cold-start",
            StreamerState::OutOfSchedule,
            0,
            None,
        )
        .await;
        insert_active_session(&pool, "session-oos-cold-start", "streamer-oos-cold-start").await;

        let monitor = build_test_monitor(&pool).await;
        let mut rx = monitor.session_lifecycle.subscribe();
        let already_out = monitor
            .streamer_manager
            .get_streamer("streamer-oos-cold-start")
            .unwrap();

        let outcome = monitor
            .process_status(
                &already_out,
                LiveStatus::Filtered {
                    reason: FilterReason::OutOfSchedule {
                        next_available: None,
                    },
                    title: "Still Out".to_string(),
                    category: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(outcome, ProcessStatusResult::Applied);
        assert!(
            get_session(&pool, "session-oos-cold-start")
                .await
                .end_time
                .is_some()
        );

        match rx.recv().await.unwrap() {
            crate::session::SessionTransition::Ended {
                session_id, cause, ..
            } => {
                assert_eq!(session_id, "session-oos-cold-start");
                assert_eq!(cause, crate::session::TerminalCause::OutOfSchedule);
            }
            other => panic!("expected Ended, got {other:?}"),
        }

        let events = outbox_events(&pool).await;
        let event_types: Vec<&str> = events.iter().map(outbox_event_type).collect();
        assert_eq!(event_types, vec!["StateChanged"]);

        monitor.stop();
    }

    #[tokio::test]
    async fn test_process_status_live_is_suppressed_while_temporarily_disabled() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-live-suppressed",
            StreamerState::TemporalDisabled,
            3,
            Some(chrono::Utc::now().timestamp_millis() + 60_000),
        )
        .await;

        let monitor = build_test_monitor(&pool).await;
        let streamer = monitor
            .streamer_manager
            .get_streamer("streamer-live-suppressed")
            .unwrap();

        let outcome = monitor
            .process_status(
                &streamer,
                LiveStatus::Live {
                    title: "Suppressed Live".to_string(),
                    category: None,
                    avatar: None,
                    started_at: None,
                    viewer_count: None,
                    streams: vec![platforms_parser::media::StreamInfo {
                        url: "https://example.com/stream.m3u8".to_string(),
                        stream_format: platforms_parser::media::StreamFormat::Flv,
                        media_format: platforms_parser::media::formats::MediaFormat::Flv,
                        quality: "best".to_string(),
                        bitrate: 5_000_000,
                        priority: 1,
                        extras: None,
                        codec: "h264".to_string(),
                        fps: 30.0,
                        is_headers_needed: false,
                        is_audio_only: false,
                    }],
                    media_headers: None,
                    media_extras: None,
                    next_check_hint: None,
                    candidates: vec![],
                },
            )
            .await
            .unwrap();

        assert!(matches!(
            outcome,
            ProcessStatusResult::Suppressed(ProcessStatusSuppression::TemporarilyDisabled {
                retry_after: Some(_)
            })
        ));

        assert!(
            get_active_session_for_streamer(&pool, "streamer-live-suppressed")
                .await
                .is_none()
        );

        assert!(outbox_events(&pool).await.is_empty());

        monitor.stop();
    }

    #[tokio::test]
    async fn test_process_status_live_is_suppressed_for_user_disabled_streamer() {
        let pool = setup_monitor_test_db().await;
        insert_streamer(
            &pool,
            "streamer-user-disabled",
            StreamerState::Disabled,
            0,
            None,
        )
        .await;

        let monitor = build_test_monitor(&pool).await;
        let streamer = monitor
            .streamer_manager
            .get_streamer("streamer-user-disabled")
            .unwrap();

        let outcome = monitor
            .process_status(
                &streamer,
                LiveStatus::Live {
                    title: "Should Stay Disabled".to_string(),
                    category: None,
                    avatar: None,
                    started_at: None,
                    viewer_count: None,
                    streams: vec![platforms_parser::media::StreamInfo {
                        url: "https://example.com/stream.flv".to_string(),
                        stream_format: platforms_parser::media::StreamFormat::Flv,
                        media_format: platforms_parser::media::formats::MediaFormat::Flv,
                        quality: "best".to_string(),
                        bitrate: 5_000_000,
                        priority: 1,
                        extras: None,
                        codec: "h264".to_string(),
                        fps: 30.0,
                        is_headers_needed: false,
                        is_audio_only: false,
                    }],
                    media_headers: None,
                    media_extras: None,
                    next_check_hint: None,
                    candidates: vec![],
                },
            )
            .await
            .unwrap();

        assert_eq!(
            outcome,
            ProcessStatusResult::Suppressed(ProcessStatusSuppression::Disabled)
        );

        assert!(outbox_events(&pool).await.is_empty());

        monitor.stop();
    }
}
