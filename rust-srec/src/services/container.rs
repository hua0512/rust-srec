//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Result;
use crate::api::auth_service::{AuthConfig, AuthService};
use crate::api::{
    ApiServer, JwtService,
    server::{ApiServerConfig, AppState},
};
use crate::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
use crate::danmu::{
    DanmuService,
    service::{DanmuEvent, DanmuServiceConfig},
};
use crate::database::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use crate::database::repositories::{
    config::SqlxConfigRepository,
    filter::SqlxFilterRepository,
    job::SqlxJobRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    refresh_token::SqlxRefreshTokenRepository,
    session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
    user::SqlxUserRepository,
};
use crate::downloader::{
    DownloadConfig, DownloadManager, DownloadManagerConfig, DownloadManagerEvent,
};
use crate::metrics::{HealthChecker, MetricsCollector, PrometheusExporter};
use crate::monitor::{MonitorEvent, MonitorEventBroadcaster, StreamMonitor};
use crate::notification::{NotificationService, NotificationServiceConfig};
use crate::pipeline::{PipelineEvent, PipelineManager, PipelineManagerConfig};
use crate::scheduler::Scheduler;
use crate::streamer::StreamerManager;
use crate::utils::filename::sanitize_filename;
use pipeline_common::expand_path_template;

/// Default cache TTL (1 hour).
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Default shutdown timeout.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

/// Service container holding all application services.
pub struct ServiceContainer {
    /// Database connection pool.
    pub pool: SqlitePool,
    /// Configuration service.
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    /// Streamer manager.
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    /// Event broadcaster (shared between services).
    pub event_broadcaster: ConfigEventBroadcaster,
    /// Download manager.
    pub download_manager: Arc<DownloadManager>,
    /// Pipeline manager.
    pub pipeline_manager: Arc<PipelineManager>,
    /// Monitor event broadcaster.
    pub monitor_event_broadcaster: MonitorEventBroadcaster,
    /// Danmu service.
    pub danmu_service: Arc<DanmuService>,
    /// Notification service.
    pub notification_service: Arc<NotificationService>,
    /// Metrics collector.
    pub metrics_collector: Arc<MetricsCollector>,
    /// Health checker.
    pub health_checker: Arc<HealthChecker>,
    /// Database maintenance scheduler.
    pub maintenance_scheduler: Arc<MaintenanceScheduler>,
    /// Scheduler service
    pub scheduler: Arc<tokio::sync::RwLock<Scheduler<SqlxStreamerRepository>>>,
    /// Stream monitor for real status detection
    pub stream_monitor: Arc<
        StreamMonitor<
            SqlxStreamerRepository,
            SqlxFilterRepository,
            SqlxSessionRepository,
            SqlxConfigRepository,
        >,
    >,
    /// API server configuration.
    api_server_config: ApiServerConfig,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
}

impl ServiceContainer {
    /// Create a new service container with the given database pool.
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        Self::with_config(pool, DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY).await
    }

    /// Create a new service container with custom configuration.
    pub async fn with_config(
        pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
    ) -> Result<Self> {
        info!("Initializing service container");

        // Create repositories
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone()));

        // Create shared event broadcaster
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);

        // Create additional repositories for StreamMonitor
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone()));

        // Create config service with custom cache
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo.clone(),
            streamer_repo.clone(),
            cache,
        ));

        // Create streamer manager
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            event_broadcaster.clone(),
        ));

        // Create stream monitor for real status detection
        let stream_monitor = Arc::new(StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo.clone(),
            config_service.clone(),
        ));

        // Create download manager with default config
        let download_manager = Arc::new(
            DownloadManager::with_config(DownloadManagerConfig::default())
                .with_config_repo(config_repo.clone()),
        );

        // Create job repository for pipeline persistence
        let job_repo = Arc::new(SqlxJobRepository::new(pool.clone()));

        // Create job preset repository
        let preset_repo = Arc::new(SqliteJobPresetRepository::new(pool.clone().into()));

        // Create pipeline manager with job repository for database persistence
        let pipeline_manager = Arc::new(
            PipelineManager::with_repository(PipelineManagerConfig::default(), job_repo)
                .with_session_repository(session_repo)
                .with_preset_repository(preset_repo)
                .with_config_service(config_service.clone()),
        );

        // Event broadcaster
        let monitor_event_broadcaster = stream_monitor.event_broadcaster().clone();

        // Create danmu service with default config
        let danmu_service = Arc::new(DanmuService::new(DanmuServiceConfig::default()));

        // Create notification service with default config
        let notification_service = Arc::new(NotificationService::with_config(
            NotificationServiceConfig::default(),
        ));

        // Create metrics collector
        let metrics_collector = Arc::new(MetricsCollector::new());

        // Create health checker
        let health_checker = Arc::new(HealthChecker::new());

        // Create database maintenance scheduler with default config
        let maintenance_scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            MaintenanceConfig::default(),
        ));

        // Create cancellation token for graceful shutdown (before scheduler so it can be shared)
        let cancellation_token = CancellationToken::new();

        // Create scheduler with StreamMonitor for real status checking
        let scheduler = Arc::new(tokio::sync::RwLock::new(
            Scheduler::with_monitor_and_config(
                streamer_manager.clone(),
                event_broadcaster.clone(),
                stream_monitor.clone(),
                crate::scheduler::SchedulerConfig::default(),
                cancellation_token.child_token(),
            ),
        ));

        info!("Service container initialized");

        Ok(Self {
            pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            danmu_service,
            notification_service,
            metrics_collector,
            health_checker,
            maintenance_scheduler,
            scheduler,
            stream_monitor,
            api_server_config: ApiServerConfig::default(),
            cancellation_token,
        })
    }

    /// Create a new service container with custom download and pipeline configs.
    pub async fn with_full_config(
        pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
        download_config: DownloadManagerConfig,
        pipeline_config: PipelineManagerConfig,
        danmu_config: DanmuServiceConfig,
        api_config: ApiServerConfig,
    ) -> Result<Self> {
        info!("Initializing service container with full configuration");

        // Create repositories
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone()));

        // Create shared event broadcaster
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);

        // Create additional repositories for StreamMonitor
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone()));

        // Create config service with custom cache
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo.clone(),
            streamer_repo.clone(),
            cache,
        ));

        // Create streamer manager
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            event_broadcaster.clone(),
        ));

        // Create stream monitor for real status detection
        let stream_monitor = Arc::new(StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo.clone(),
            config_service.clone(),
        ));

        // Create download manager with custom config
        let download_manager = Arc::new(
            DownloadManager::with_config(download_config).with_config_repo(config_repo.clone()),
        );

        // Create job repository for pipeline persistence
        let job_repo = Arc::new(SqlxJobRepository::new(pool.clone()));

        // Create job preset repository
        let preset_repo = Arc::new(SqliteJobPresetRepository::new(pool.clone().into()));

        // Create pipeline manager with job repository for database persistence
        let pipeline_manager = Arc::new(
            PipelineManager::with_repository(pipeline_config, job_repo)
                .with_session_repository(session_repo.clone())
                .with_preset_repository(preset_repo),
        );

        // Get monitor event broadcaster
        let monitor_event_broadcaster = stream_monitor.event_broadcaster().clone();

        // Create danmu service with custom config
        let danmu_service = Arc::new(DanmuService::new(danmu_config));

        // Create notification service with default config
        let notification_service = Arc::new(NotificationService::with_config(
            NotificationServiceConfig::default(),
        ));

        // Create metrics collector
        let metrics_collector = Arc::new(MetricsCollector::new());

        // Create health checker
        let health_checker = Arc::new(HealthChecker::new());

        // Create database maintenance scheduler with default config
        let maintenance_scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            MaintenanceConfig::default(),
        ));

        // Create cancellation token for graceful shutdown (before scheduler so it can be shared)
        let cancellation_token = CancellationToken::new();

        // Create scheduler with StreamMonitor for real status checking
        let scheduler = Arc::new(tokio::sync::RwLock::new(
            Scheduler::with_monitor_and_config(
                streamer_manager.clone(),
                event_broadcaster.clone(),
                stream_monitor.clone(),
                crate::scheduler::SchedulerConfig::default(),
                cancellation_token.child_token(),
            ),
        ));

        info!("Service container initialized with full configuration and real status checking");

        Ok(Self {
            pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            danmu_service,
            notification_service,
            metrics_collector,
            health_checker,
            maintenance_scheduler,
            scheduler,
            stream_monitor,
            api_server_config: api_config,
            cancellation_token,
        })
    }

    /// Initialize all services (hydrate data, start background tasks, etc.).
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing services");

        // Hydrate streamer manager from database
        let streamer_count = self.streamer_manager.hydrate().await?;
        info!("Hydrated {} streamers", streamer_count);

        // Recover jobs from database on startup (Requirements 6.3, 7.4)
        // This resets PROCESSING jobs to PENDING for re-execution.
        // For sequential pipelines, no special handling is needed since only one job
        // per pipeline exists at a time.
        let recovered_jobs = self.pipeline_manager.recover_jobs().await?;
        if recovered_jobs > 0 {
            info!("Recovered {} jobs from database", recovered_jobs);
        }

        // Start pipeline manager
        self.pipeline_manager.start();
        info!("Pipeline manager started");

        // Subscribe streamer manager to config events
        self.setup_config_event_subscriptions();

        // Wire download events to pipeline manager
        self.setup_download_event_subscriptions();

        // Wire monitor events to download manager and danmu service
        self.setup_monitor_event_subscriptions();

        // Wire danmu events to download manager for segment coordination
        self.setup_danmu_event_subscriptions();

        // Wire notification service to system events
        self.setup_notification_event_subscriptions();

        // Register health checks
        self.register_health_checks().await;

        // Start database maintenance scheduler
        self.maintenance_scheduler.clone().start();
        info!("Database maintenance scheduler started");

        // Start scheduler in background
        self.start_scheduler().await;

        info!("Services initialized");
        Ok(())
    }

    /// Start the scheduler service in a background task.
    ///
    /// The scheduler uses a child token of the container's cancellation token,
    /// so it will automatically stop when the container is shut down.
    async fn start_scheduler(&self) {
        // Set download receiver before starting
        {
            let mut scheduler = self.scheduler.write().await;
            scheduler.set_download_receiver(self.download_manager.subscribe());
        }

        // Run scheduler in background task
        let scheduler = self.scheduler.clone();
        tokio::spawn(async move {
            let mut guard = scheduler.write().await;
            if let Err(e) = guard.run().await {
                tracing::error!("Scheduler error: {}", e);
            }
        });

        info!("Scheduler started");
    }

    /// Initialize and start the API server.
    /// This should be called after initialize() and runs the server in the background.
    pub async fn start_api_server(&self) -> Result<()> {
        // Create JWT service from environment if configured
        let jwt_service = Self::create_jwt_service_from_env();

        // Create AuthService if JWT is configured
        let auth_service = if let Some(ref jwt) = jwt_service {
            // Create user and refresh token repositories
            let user_repo = Arc::new(SqlxUserRepository::new(self.pool.clone()));
            let token_repo = Arc::new(SqlxRefreshTokenRepository::new(self.pool.clone()));

            // Create AuthService with default config
            let auth_config = AuthConfig::default();
            let auth_svc = AuthService::new(user_repo, token_repo, jwt.clone(), auth_config);
            info!("AuthService initialized with user database authentication");
            Some(Arc::new(auth_svc))
        } else {
            debug!("JWT not configured, AuthService disabled");
            None
        };

        let mut state = AppState::with_services(
            jwt_service,
            self.config_service.clone(),
            self.streamer_manager.clone(),
            self.pipeline_manager.clone(),
            self.danmu_service.clone(),
            self.download_manager.clone(),
        );

        // Wire AuthService into AppState if available
        if let Some(auth_svc) = auth_service {
            state = state.with_auth_service(auth_svc);
        }

        // Wire HealthChecker into AppState for health endpoints
        state = state.with_health_checker(self.health_checker.clone());

        // Wire SessionRepository, FilterRepository, and PipelinePresetRepository into AppState
        state = state
            .with_session_repository(Arc::new(SqlxSessionRepository::new(self.pool.clone())))
            .with_filter_repository(Arc::new(SqlxFilterRepository::new(self.pool.clone())))
            .with_streamer_repository(Arc::new(SqlxStreamerRepository::new(self.pool.clone())))
            .with_pipeline_preset_repository(Arc::new(SqlitePipelinePresetRepository::new(
                Arc::new(self.pool.clone()),
            )));

        let server = ApiServer::with_state(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        // Start server in background
        let addr = format!(
            "{}:{}",
            self.api_server_config.bind_address, self.api_server_config.port
        );
        info!("Starting API server on http://{}", addr);

        tokio::spawn(async move {
            if let Err(e) = server.run().await {
                tracing::error!("API server error: {}", e);
            }
        });

        Ok(())
    }

    /// Set up config event subscriptions between services.
    fn setup_config_event_subscriptions(&self) {
        let streamer_manager = self.streamer_manager.clone();
        let scheduler = self.scheduler.clone();
        let download_manager = self.download_manager.clone();
        let danmu_service = self.danmu_service.clone();
        let mut receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        // Spawn a task to handle config update events
        tokio::spawn(async move {
            use crate::config::ConfigUpdateEvent;
            use crate::domain::streamer::StreamerState;

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Config event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                match event {
                                    ConfigUpdateEvent::StreamerUpdated { streamer_id } => {
                                        debug!(
                                            "Received streamer config update event: {}",
                                            streamer_id
                                        );

                                        // Check if streamer is now disabled (Requirements 4.1)
                                        if let Some(metadata) = streamer_manager.get_streamer(&streamer_id) {
                                            if metadata.state == StreamerState::Disabled {
                                                info!("Streamer {} disabled, initiating cleanup", streamer_id);
                                                Self::handle_streamer_disabled(
                                                    &scheduler,
                                                    &download_manager,
                                                    &danmu_service,
                                                    &streamer_id,
                                                ).await;
                                            }
                                        }
                                    }
                                    ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                                        debug!(
                                            "Received platform config update event: {}",
                                            platform_id
                                        );
                                    }
                                    ConfigUpdateEvent::TemplateUpdated { template_id } => {
                                        debug!(
                                            "Received template config update event: {}",
                                            template_id
                                        );
                                    }
                                    ConfigUpdateEvent::GlobalUpdated => {
                                        debug!("Received global config update event");
                                    }
                                    ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                                        info!(
                                            "Streamer {} deleted, initiating cleanup",
                                            streamer_id
                                        );
                                        // Reuse the same cleanup logic as disabled state
                                        Self::handle_streamer_disabled(
                                            &scheduler,
                                            &download_manager,
                                            &danmu_service,
                                            &streamer_id,
                                        ).await;
                                    }
                                    ConfigUpdateEvent::EngineUpdated { engine_id } => {
                                        debug!(
                                            "Received engine config update event: {}",
                                            engine_id
                                        );
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    /// Set up download event subscriptions to pipeline manager.
    fn setup_download_event_subscriptions(&self) {
        let pipeline_manager = self.pipeline_manager.clone();
        let stream_monitor = self.stream_monitor.clone();
        let streamer_manager = self.streamer_manager.clone();
        let mut receiver = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(download_event) => {
                                // Handle download failure for error tracking
                                if let DownloadManagerEvent::DownloadFailed { ref streamer_id, ref error, .. } = download_event {
                                    // Record error for exponential backoff
                                    if let Some(metadata) = streamer_manager.get_streamer(streamer_id) {
                                        if let Err(e) = stream_monitor.handle_error(&metadata, error).await {
                                            warn!("Failed to record download error for {}: {}", streamer_id, e);
                                        } else {
                                            debug!("Recorded download error for {}: {}", streamer_id, error);
                                        }
                                    }
                                }

                                // Forward to pipeline manager
                                pipeline_manager.handle_download_event(download_event.clone()).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                warn!("Download event handler lagged {} events", n);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                debug!("Download event channel closed");
                                break;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Set up monitor event subscriptions to download manager and danmu service.
    fn setup_monitor_event_subscriptions(&self) {
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let danmu_service = self.danmu_service.clone();
        let mut receiver = self.monitor_event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Monitor event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                Self::handle_monitor_event(
                                    &download_manager,
                                    &streamer_manager,
                                    &config_service,
                                    &danmu_service,
                                    event,
                                ).await;
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    /// Set up danmu event subscriptions for segment coordination.
    fn setup_danmu_event_subscriptions(&self) {
        let mut receiver = self.danmu_service.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Danmu event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                match event {
                                    DanmuEvent::CollectionStarted { session_id, streamer_id } => {
                                        info!(
                                            "Danmu collection started for session {} (streamer: {})",
                                            session_id, streamer_id
                                        );
                                    }
                                    DanmuEvent::CollectionStopped { session_id, statistics } => {
                                        info!(
                                            "Danmu collection stopped for session {}: {} messages",
                                            session_id, statistics.total_count
                                        );
                                    }
                                    DanmuEvent::SegmentStarted { session_id, segment_id, output_path } => {
                                        debug!(
                                            "Danmu segment started: session={}, segment={}, path={:?}",
                                            session_id, segment_id, output_path
                                        );
                                    }
                                    DanmuEvent::SegmentCompleted { session_id, segment_id, message_count, .. } => {
                                        info!(
                                            "Danmu segment completed: session={}, segment={}, messages={}",
                                            session_id, segment_id, message_count
                                        );
                                    }
                                    DanmuEvent::Reconnecting { session_id, attempt } => {
                                        warn!(
                                            "Danmu reconnecting for session {}: attempt {}",
                                            session_id, attempt
                                        );
                                    }
                                    DanmuEvent::ReconnectFailed { session_id, error } => {
                                        warn!(
                                            "Danmu reconnect failed for session {}: {}",
                                            session_id, error
                                        );
                                    }
                                    DanmuEvent::Error { session_id, error } => {
                                        warn!(
                                            "Danmu error for session {}: {}",
                                            session_id, error
                                        );
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    /// Set up notification service event subscriptions.
    fn setup_notification_event_subscriptions(&self) {
        let notification_service = self.notification_service.clone();
        let monitor_rx = self.monitor_event_broadcaster.subscribe();
        let download_rx = self.download_manager.subscribe();
        let pipeline_rx = self.pipeline_manager.subscribe();

        notification_service.start_event_listeners(monitor_rx, download_rx, pipeline_rx);
        info!("Notification service event listeners started");
    }

    /// Register health checks for all components.
    async fn register_health_checks(&self) {
        use crate::metrics::ComponentHealth;

        let pool = self.pool.clone();
        let download_manager = self.download_manager.clone();
        let pipeline_manager = self.pipeline_manager.clone();
        let danmu_service = self.danmu_service.clone();

        // Database health check
        self.health_checker
            .register(
                "database",
                Arc::new(move || {
                    if pool.is_closed() {
                        ComponentHealth::unhealthy("database", "Connection pool is closed")
                    } else {
                        ComponentHealth::healthy("database")
                    }
                }),
            )
            .await;

        // Download manager health check
        let dm = download_manager.clone();
        self.health_checker
            .register(
                "download_manager",
                Arc::new(move || {
                    let active = dm.active_count();
                    if active > 50 {
                        ComponentHealth::degraded(
                            "download_manager",
                            format!("High number of active downloads: {}", active),
                        )
                    } else {
                        ComponentHealth::healthy("download_manager")
                    }
                }),
            )
            .await;

        // Pipeline manager health check
        let pm = pipeline_manager.clone();
        self.health_checker
            .register(
                "pipeline_manager",
                Arc::new(move || {
                    let depth = pm.queue_depth();
                    let status = pm.queue_status();
                    match status {
                        crate::pipeline::QueueDepthStatus::Critical => ComponentHealth::unhealthy(
                            "pipeline_manager",
                            format!("Queue depth critical: {}", depth),
                        ),
                        crate::pipeline::QueueDepthStatus::Warning => ComponentHealth::degraded(
                            "pipeline_manager",
                            format!("Queue depth warning: {}", depth),
                        ),
                        crate::pipeline::QueueDepthStatus::Normal => {
                            ComponentHealth::healthy("pipeline_manager")
                        }
                    }
                }),
            )
            .await;

        // Danmu service health check
        let ds = danmu_service.clone();
        self.health_checker
            .register(
                "danmu_service",
                Arc::new(move || {
                    let _active = ds.active_sessions().len();
                    ComponentHealth::healthy("danmu_service")
                }),
            )
            .await;

        // Scheduler health check
        // Check if scheduler is running (not cancelled)
        let cancellation_token = self.cancellation_token.clone();
        self.health_checker
            .register(
                "scheduler",
                Arc::new(move || {
                    if cancellation_token.is_cancelled() {
                        ComponentHealth::unhealthy("scheduler", "Scheduler has been cancelled")
                    } else {
                        ComponentHealth::healthy("scheduler")
                    }
                }),
            )
            .await;

        // Notification service health check
        // Notification service is healthy if it exists
        self.health_checker
            .register(
                "notification_service",
                Arc::new(|| ComponentHealth::healthy("notification_service")),
            )
            .await;

        // Maintenance scheduler health check
        // Maintenance scheduler is healthy if it exists
        self.health_checker
            .register(
                "maintenance_scheduler",
                Arc::new(|| ComponentHealth::healthy("maintenance_scheduler")),
            )
            .await;

        info!("Health checks registered");
    }

    /// Handle streamer disabled state transition.
    ///
    /// This method coordinates cleanup when a streamer is disabled:
    /// 1. Removes the streamer actor from the scheduler
    /// 2. Cancels any active downloads
    /// 3. Stops any active danmu collection
    ///
    /// All errors are logged but do not propagate - cleanup is best-effort
    /// and should not block other operations.
    ///
    /// # Arguments
    /// * `scheduler` - The scheduler service to remove the actor from
    /// * `download_manager` - The download manager to cancel downloads
    /// * `danmu_service` - The danmu service to stop collection
    /// * `streamer_id` - The ID of the streamer being disabled
    pub async fn handle_streamer_disabled(
        scheduler: &Arc<tokio::sync::RwLock<Scheduler<SqlxStreamerRepository>>>,
        download_manager: &Arc<DownloadManager>,
        danmu_service: &Arc<DanmuService>,
        streamer_id: &str,
    ) {
        // 1. Remove actor from scheduler
        {
            let mut scheduler_guard = scheduler.write().await;
            if scheduler_guard.remove_streamer(streamer_id) {
                info!("Removed actor for disabled streamer: {}", streamer_id);
            } else {
                debug!("No actor found for disabled streamer: {}", streamer_id);
            }
        }

        // 2. Cancel active download if exists
        if let Some(download_info) = download_manager.get_download_by_streamer(streamer_id) {
            match download_manager.stop_download(&download_info.id).await {
                Ok(()) => {
                    info!(
                        "Cancelled download {} for disabled streamer {}",
                        download_info.id, streamer_id
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to cancel download for disabled streamer {}: {}",
                        streamer_id, e
                    );
                }
            }
        } else {
            debug!(
                "No active download found for disabled streamer: {}",
                streamer_id
            );
        }

        // 3. Stop danmu collection if active
        if let Some(session_id) = danmu_service.get_session_by_streamer(streamer_id) {
            match danmu_service.stop_collection(&session_id).await {
                Ok(stats) => {
                    info!(
                        "Stopped danmu collection for disabled streamer {}: {} messages",
                        streamer_id, stats.total_count
                    );
                }
                Err(e) => {
                    warn!(
                        "Failed to stop danmu collection for disabled streamer {}: {}",
                        streamer_id, e
                    );
                }
            }
        } else {
            debug!(
                "No active danmu session found for disabled streamer: {}",
                streamer_id
            );
        }
    }

    /// Handle monitor events to trigger downloads and danmu collection.
    async fn handle_monitor_event(
        download_manager: &Arc<DownloadManager>,
        streamer_manager: &Arc<StreamerManager<SqlxStreamerRepository>>,
        config_service: &Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        danmu_service: &Arc<DanmuService>,
        event: MonitorEvent,
    ) {
        match event {
            MonitorEvent::StreamerLive {
                streamer_id,
                session_id,
                streamer_name,
                title,
                streams,
                streamer_url,
                media_headers,
                ..
            } => {
                info!(
                    "Streamer {} ({}) went live: {} ({} streams available, {} media headers)",
                    streamer_name,
                    streamer_id,
                    title,
                    streams.len(),
                    media_headers.as_ref().map(|h| h.len()).unwrap_or(0)
                );

                // Check if already downloading
                if download_manager.has_active_download(&streamer_id) {
                    debug!("Download already active for {}", streamer_id);

                    // DEBUG: Inspect conflicting downloads
                    let active = download_manager.get_active_downloads();
                    let conflicts: Vec<_> = active
                        .iter()
                        .filter(|d| d.streamer_id == streamer_id)
                        .collect();

                    for conflict in conflicts {
                        tracing::warn!(
                            "CONFLICTING DOWNLOAD: ID={}, Status={:?}, Started={:?}",
                            conflict.id,
                            conflict.status,
                            conflict.started_at
                        );
                    }
                    return;
                }

                // Validate we have streams to download
                if streams.is_empty() {
                    warn!(
                        "Streamer {} has no streams available, cannot start download",
                        streamer_id
                    );
                    return;
                }

                // Get streamer metadata for priority
                let streamer_metadata = streamer_manager.get_streamer(&streamer_id);
                let is_high_priority = streamer_metadata
                    .as_ref()
                    .map(|s| s.priority == crate::domain::Priority::High)
                    .unwrap_or(false);

                // Load merged config for this streamer
                let merged_config = match config_service.get_config_for_streamer(&streamer_id).await
                {
                    Ok(config) => config,
                    Err(e) => {
                        warn!(
                            "Failed to load config for streamer {}, using defaults: {}",
                            streamer_id, e
                        );
                        // Use default config if we can't load the merged config
                        crate::domain::config::MergedConfig::builder().build()
                    }
                };

                // The detector emits only the selected stream(s), so we take the first one
                let best_stream = &streams[0];
                let stream_url_selected = best_stream.url.clone();
                let stream_format = best_stream.stream_format.as_str();
                let media_format = best_stream.media_format.as_str();

                let headers = media_headers.as_ref().cloned().unwrap_or_default();

                if !headers.is_empty() {
                    debug!(
                        "Using {} merged headers for download: {:?}",
                        headers.len(),
                        headers.iter().map(|(k, _)| k).collect::<Vec<_>>()
                    );
                }

                // Sanitize streamer name and title for safe filename usage (Requirements 1.1, 2.1, 3.1)
                let sanitized_streamer = sanitize_filename(&streamer_name);
                let sanitized_title = sanitize_filename(&title);

                let dir = merged_config
                    .output_folder
                    .replace("{streamer}", &sanitized_streamer)
                    .replace("{title}", &sanitized_title)
                    .replace("{session_id}", &session_id);

                let output_dir = expand_path_template(&dir);

                let mut config = DownloadConfig::new(
                    stream_url_selected.clone(),
                    output_dir.clone(),
                    streamer_id.clone(),
                    session_id.clone(),
                )
                .with_filename_template(
                    &merged_config
                        .output_filename_template
                        .replace("{streamer}", &sanitized_streamer)
                        .replace("{title}", &sanitized_title),
                )
                .with_output_format(&merged_config.output_file_format)
                .with_max_segment_duration(merged_config.max_download_duration_secs as u64)
                .with_max_segment_size(merged_config.max_part_size_bytes as u64)
                .with_engines_override(merged_config.engines_override.clone());

                // Add headers if needed
                for (key, value) in headers {
                    config = config.with_header(key, value);
                }

                info!(
                    "Starting download for {} with stream URL: {} (stream_format: {}, media_format: {}, headers_needed: {}, output: {})",
                    streamer_name,
                    stream_url_selected,
                    stream_format,
                    media_format,
                    best_stream.is_headers_needed,
                    merged_config.output_folder
                );

                // Start download
                match download_manager
                    .start_download(
                        config,
                        Some(merged_config.download_engine),
                        is_high_priority,
                    )
                    .await
                {
                    Ok(download_id) => {
                        info!(
                            "Started download {} for streamer {} (priority: {})",
                            download_id,
                            streamer_id,
                            if is_high_priority { "high" } else { "normal" }
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to start download for streamer {}: {}",
                            streamer_id, e
                        );
                    }
                }

                // Start danmu collection if enabled
                if merged_config.record_danmu {
                    let sampling_config = Some(merged_config.danmu_sampling_config.clone());
                    match danmu_service
                        .start_collection(&session_id, &streamer_id, &streamer_url, sampling_config)
                        .await
                    {
                        Ok(handle) => {
                            info!(
                                "Started danmu collection for session {} (streamer: {})",
                                handle.session_id(),
                                streamer_id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to start danmu collection for streamer {}: {}",
                                streamer_id, e
                            );
                        }
                    }
                }
            }
            MonitorEvent::StreamerOffline {
                streamer_id,
                streamer_name,
                session_id,
                ..
            } => {
                info!("Streamer {} ({}) went offline", streamer_name, streamer_id);

                // Stop danmu collection if active
                if let Some(sid) = session_id {
                    if danmu_service.is_collecting(&sid) {
                        match danmu_service.stop_collection(&sid).await {
                            Ok(stats) => {
                                info!(
                                    "Stopped danmu collection for session {}: {} messages collected",
                                    sid, stats.total_count
                                );
                            }
                            Err(e) => {
                                warn!("Failed to stop danmu collection for session {}: {}", sid, e);
                            }
                        }
                    }
                }

                // Stop download if active
                if let Some(download_info) = download_manager.get_download_by_streamer(&streamer_id)
                {
                    match download_manager.stop_download(&download_info.id).await {
                        Ok(()) => {
                            info!(
                                "Stopped download {} for streamer {}",
                                download_info.id, streamer_id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to stop download for streamer {}: {}",
                                streamer_id, e
                            );
                        }
                    }
                }
            }
            _ => {
                // Other events don't trigger download actions
            }
        }
    }

    /// Shutdown all services gracefully.
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_timeout(DEFAULT_SHUTDOWN_TIMEOUT).await
    }

    /// Shutdown all services gracefully with a custom timeout.
    pub async fn shutdown_with_timeout(&self, timeout: Duration) -> Result<()> {
        info!("Shutting down services (timeout: {:?})", timeout);

        // Signal all background tasks to stop
        self.cancellation_token.cancel();

        // Stop database maintenance scheduler
        info!("Stopping maintenance scheduler...");
        self.maintenance_scheduler.stop();
        info!("Maintenance scheduler stopped");

        // Stop notification service
        info!("Stopping notification service...");
        self.notification_service.stop().await;
        info!("Notification service stopped");

        // Stop danmu service (finalize XML files)
        info!("Stopping danmu service...");
        self.danmu_service.shutdown().await;
        info!("Danmu service stopped");

        // Stop accepting new downloads
        info!("Stopping download manager...");
        let stopped_downloads = self.download_manager.stop_all().await;
        info!("Stopped {} active downloads", stopped_downloads.len());

        // Stop pipeline manager and drain job queue
        info!("Stopping pipeline manager...");
        self.pipeline_manager.stop().await;
        info!("Pipeline manager stopped");

        // Stop scheduler (cancellation already triggered via linked token above)
        info!("Stopping scheduler...");

        // Wait for background tasks with timeout
        let shutdown_result = tokio::time::timeout(timeout, async {
            // Give background tasks time to clean up
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;

        if shutdown_result.is_err() {
            warn!("Shutdown timeout reached, forcing shutdown");
        }

        // Close database pool
        info!("Closing database pool...");
        self.pool.close().await;

        info!("Services shut down");
        Ok(())
    }

    /// Get the cancellation token for external use.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Check if shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    /// Get service statistics.
    pub fn stats(&self) -> ServiceStats {
        // Try to get scheduler stats without blocking
        let scheduler_stats = self.scheduler.try_read().ok().map(|guard| guard.stats());

        ServiceStats {
            streamer_count: self.streamer_manager.count(),
            active_streamer_count: self.streamer_manager.active_count(),
            live_streamer_count: self.streamer_manager.live_count(),
            disabled_streamer_count: self.streamer_manager.disabled_count(),
            cache_stats: self.config_service.cache_stats(),
            event_subscriber_count: self.event_broadcaster.subscriber_count(),
            active_downloads: self.download_manager.active_count(),
            pipeline_queue_depth: self.pipeline_manager.queue_depth(),
            active_danmu_collections: self.danmu_service.active_sessions().len(),
            notification_stats: self.notification_service.stats(),
            scheduler_stats,
        }
    }

    /// Get the metrics collector.
    pub fn metrics_collector(&self) -> &Arc<MetricsCollector> {
        &self.metrics_collector
    }

    /// Get the health checker.
    pub fn health_checker(&self) -> &Arc<HealthChecker> {
        &self.health_checker
    }

    /// Get the notification service.
    pub fn notification_service(&self) -> &Arc<NotificationService> {
        &self.notification_service
    }

    /// Get Prometheus metrics export.
    pub fn prometheus_metrics(&self) -> String {
        let exporter = PrometheusExporter::new(self.metrics_collector.clone());
        exporter.export()
    }

    /// Subscribe to danmu events.
    pub fn subscribe_danmu_events(&self) -> tokio::sync::broadcast::Receiver<DanmuEvent> {
        self.danmu_service.subscribe()
    }

    /// Get the danmu service for direct access.
    pub fn danmu_service(&self) -> &Arc<DanmuService> {
        &self.danmu_service
    }

    /// Subscribe to pipeline events.
    pub fn subscribe_pipeline_events(&self) -> tokio::sync::broadcast::Receiver<PipelineEvent> {
        self.pipeline_manager.subscribe()
    }

    /// Subscribe to monitor events.
    pub fn subscribe_monitor_events(&self) -> tokio::sync::broadcast::Receiver<MonitorEvent> {
        self.monitor_event_broadcaster.subscribe()
    }

    /// Get the monitor event broadcaster for external use.
    pub fn monitor_broadcaster(&self) -> &MonitorEventBroadcaster {
        &self.monitor_event_broadcaster
    }

    /// Create JWT service from environment variables.
    ///
    /// Required environment variable:
    /// - `JWT_SECRET`: Secret key for signing tokens
    ///
    /// Optional environment variables:
    /// - `JWT_ISSUER`: Token issuer (default: "rust-srec")
    /// - `JWT_AUDIENCE`: Token audience (default: "rust-srec-api")
    /// - `JWT_EXPIRATION_SECS`: Token expiration in seconds (default: 3600)
    fn create_jwt_service_from_env() -> Option<Arc<JwtService>> {
        let secret = std::env::var("JWT_SECRET").ok()?;
        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "rust-srec".to_string());
        let audience =
            std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "rust-srec-api".to_string());
        let expiration_secs = std::env::var("JWT_EXPIRATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        info!(
            "JWT authentication enabled (issuer: {}, audience: {})",
            issuer, audience
        );

        Some(Arc::new(JwtService::new(
            &secret,
            &issuer,
            &audience,
            Some(expiration_secs),
        )))
    }
}

/// Service statistics.
#[derive(Debug, Clone)]
pub struct ServiceStats {
    /// Total number of streamers.
    pub streamer_count: usize,
    /// Number of active streamers.
    pub active_streamer_count: usize,
    /// Number of live streamers.
    pub live_streamer_count: usize,
    /// Number of disabled streamers.
    pub disabled_streamer_count: usize,
    /// Cache statistics.
    pub cache_stats: crate::config::CacheStats,
    /// Number of event subscribers.
    pub event_subscriber_count: usize,
    /// Number of active downloads.
    pub active_downloads: usize,
    /// Pipeline job queue depth.
    pub pipeline_queue_depth: usize,
    /// Number of active danmu collections.
    pub active_danmu_collections: usize,
    /// Notification service statistics.
    pub notification_stats: crate::notification::NotificationStats,
    /// Scheduler statistics (if available).
    pub scheduler_stats: Option<crate::scheduler::actor::SupervisorStats>,
}

#[cfg(test)]
mod tests {
    // Integration tests would go here with a test database
}
