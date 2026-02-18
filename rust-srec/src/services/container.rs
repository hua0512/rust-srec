//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
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
use crate::credentials::{
    CredentialRefreshService, CredentialResolver, platforms::BilibiliCredentialManager,
};
use crate::danmu::{DanmuEvent, DanmuService, service::DanmuServiceConfig};
use crate::database::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use crate::database::repositories::{
    ConfigRepository, NotificationRepository, SqlxNotificationRepository,
};
use crate::database::repositories::{
    SqlxCredentialStore,
    config::SqlxConfigRepository,
    dag::SqlxDagRepository,
    filter::SqlxFilterRepository,
    job::SqlxJobRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    refresh_token::SqlxRefreshTokenRepository,
    session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
    user::SqlxUserRepository,
};
use crate::domain::{Priority, StreamerState};
use crate::downloader::{
    DownloadConfig, DownloadManager, DownloadManagerConfig, DownloadManagerEvent,
};
use crate::logging::LoggingConfig;
use crate::metrics::{HealthChecker, MetricsCollector, PrometheusExporter};
use crate::monitor::{MonitorEvent, MonitorEventBroadcaster, StreamMonitor};
use crate::notification::web_push::WebPushService;
use crate::notification::{NotificationService, NotificationServiceConfig};
use crate::pipeline::{PipelineEvent, PipelineManager, PipelineManagerConfig};
use crate::scheduler::Scheduler;
use crate::streamer::StreamerManager;
use crate::utils::filename::sanitize_filename;
use pipeline_common::expand_path_template;

fn should_end_stream_on_danmu_stream_closed(platform_specific_config: Option<&str>) -> bool {
    platform_specific_config
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|value| {
            value
                .get("end_stream_on_danmu_stream_closed")
                .and_then(|v| v.as_bool())
        })
        .unwrap_or(true)
}

/// Default cache TTL (1 hour).
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Default shutdown timeout.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

fn autoscale_concurrency_limit(raw: i32) -> usize {
    if raw > 0 {
        return raw as usize;
    }

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);

    (cores / 2).max(1)
}

/// Service container holding all application services.
pub struct ServiceContainer {
    /// Database connection pool (read-heavy).
    pub pool: SqlitePool,
    /// Serialized write pool (max_connections=1) for contention-free writes.
    write_pool: SqlitePool,
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
    /// Notification repository.
    pub notification_repository: Arc<dyn NotificationRepository>,
    /// Web push service for browser notifications (VAPID), if configured.
    pub web_push_service: Option<Arc<WebPushService>>,
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
    /// Credential refresh service (shared between monitor + API).
    pub credential_service: Arc<crate::credentials::CredentialRefreshService<SqlxConfigRepository>>,
    /// API server configuration.
    api_server_config: ApiServerConfig,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Logging configuration
    logging_config: std::sync::OnceLock<Arc<LoggingConfig>>,
    /// Segment keys that should be discarded (min-size gate) to prevent danmu/xml and video
    /// from racing into the pipeline while being deleted.
    discarded_segment_keys: Arc<DashMap<(String, String), Instant>>,
    /// Handle to the scheduler background task for graceful shutdown.
    scheduler_task_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ServiceContainer {
    /// Create a new service container with the given database pool.
    pub async fn new(pool: SqlitePool, write_pool: SqlitePool) -> Result<Self> {
        Self::with_config(pool, write_pool, DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY).await
    }

    /// Create a new service container with custom configuration.
    pub async fn with_config(
        pool: SqlitePool,
        write_pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
    ) -> Result<Self> {
        info!("Initializing service container");

        // Create repositories
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), write_pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(
            pool.clone(),
            write_pool.clone(),
        ));

        // Load global config early for initial runtime knobs (worker pools, scheduler timing, etc.).
        let global_config = config_repo.get_global_config().await?;

        // Create shared event broadcaster
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);

        // Create additional repositories for StreamMonitor
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), write_pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), write_pool.clone()));

        // Create config service with custom cache
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(ConfigService::with_cache_and_broadcaster(
            config_repo.clone(),
            streamer_repo.clone(),
            cache,
            event_broadcaster.clone(),
        ));

        // Create streamer manager
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            event_broadcaster.clone(),
        ));

        // Create stream monitor for real status detection
        let mut stream_monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo.clone(),
            config_service.clone(),
            write_pool.clone(),
        );

        // Create notification service with default config (also used for credential events).
        let notification_repository = Arc::new(SqlxNotificationRepository::new(
            pool.clone(),
            write_pool.clone(),
        ));
        let web_push_service = WebPushService::from_env(pool.clone(), write_pool.clone())
            .unwrap_or_else(|e| {
                warn!(error = %e, "Web push service disabled due to configuration error");
                None
            })
            .map(Arc::new);

        let mut notification_service = NotificationService::with_repository(
            NotificationServiceConfig::default(),
            notification_repository.clone(),
        );
        if let Some(web_push) = web_push_service.clone() {
            notification_service = notification_service.with_web_push_service(web_push);
        }
        let notification_service = Arc::new(notification_service);
        notification_service.start_web_push_worker();

        // Build credential refresh service (shared between StreamMonitor + API).
        let credential_resolver = Arc::new(CredentialResolver::new(config_repo.clone()));
        let credential_store = Arc::new(SqlxCredentialStore::new(pool.clone(), write_pool.clone()));
        let mut credential_service =
            CredentialRefreshService::new(credential_resolver, credential_store);
        credential_service.set_notification_service(Arc::clone(&notification_service));
        match BilibiliCredentialManager::new_lazy() {
            Ok(manager) => credential_service.register_manager(Arc::new(manager)),
            Err(e) => warn!(error = %e, "Failed to init bilibili credential manager; skipping"),
        }
        let credential_service = Arc::new(credential_service);
        stream_monitor.set_credential_service(Arc::clone(&credential_service));
        let stream_monitor = Arc::new(stream_monitor);

        // Create download manager with default config, overridden by global config.
        let download_config = DownloadManagerConfig {
            max_concurrent_downloads: (global_config.max_concurrent_downloads as i64).max(1)
                as usize,
            ..Default::default()
        };
        let download_manager = Arc::new(
            DownloadManager::with_config(download_config).with_config_repo(config_repo.clone()),
        );

        // Create job repository for pipeline persistence
        let job_repo = Arc::new(SqlxJobRepository::new(pool.clone(), write_pool.clone()));

        // Create job preset repository
        let preset_repo = Arc::new(SqliteJobPresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));

        // Create pipeline preset repository
        let pipeline_preset_repo = Arc::new(SqlitePipelinePresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));

        // Create pipeline manager with job repository for database persistence.
        // Wire global-config concurrency knobs into CPU/IO worker pool sizes.
        let mut pipeline_config = PipelineManagerConfig::default();
        pipeline_config.cpu_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_cpu_jobs);
        pipeline_config.io_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_io_jobs);

        // Pipeline job timeouts are configured via global config.
        // NOTE: These are applied at startup; changing them at runtime requires restart.
        pipeline_config.cpu_pool.job_timeout_secs =
            global_config.pipeline_cpu_job_timeout_secs.max(1) as u64;
        pipeline_config.io_pool.job_timeout_secs =
            global_config.pipeline_io_job_timeout_secs.max(1) as u64;
        pipeline_config.execute_timeout_secs =
            global_config.pipeline_execute_timeout_secs.max(1) as u64;
        let pipeline_manager = Arc::new(
            PipelineManager::with_repository(pipeline_config, job_repo)
                .with_session_repository(session_repo.clone())
                .with_streamer_repository(streamer_repo.clone())
                .with_preset_repository(preset_repo)
                .with_pipeline_preset_repository(pipeline_preset_repo)
                .with_config_service(config_service.clone())
                .with_dag_repository(Arc::new(SqlxDagRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ))),
        );

        // Event broadcaster
        let monitor_event_broadcaster = stream_monitor.event_broadcaster().clone();

        // Create danmu service with default config
        let danmu_service = Arc::new(
            DanmuService::new(DanmuServiceConfig::default())
                .with_session_repository(session_repo.clone()),
        );

        // Create metrics collector
        let metrics_collector = Arc::new(MetricsCollector::new());
        if let Some(web_push) = web_push_service.as_ref() {
            web_push.set_metrics_collector(metrics_collector.clone());
        }

        // Create health checker
        let health_checker = Arc::new(HealthChecker::new());

        // Create database maintenance scheduler (retention settings are user-configurable via global config)
        let maintenance_config = MaintenanceConfig {
            job_retention_days: global_config.job_history_retention_days.max(0),
            notification_event_log_retention_days: global_config
                .notification_event_log_retention_days
                .max(0),
            ..Default::default()
        };
        let maintenance_scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            write_pool.clone(),
            maintenance_config,
        ));

        // Create cancellation token for graceful shutdown (before scheduler so it can be shared)
        let cancellation_token = CancellationToken::new();

        let scheduler_config = crate::scheduler::SchedulerConfig {
            check_interval_ms: global_config.streamer_check_delay_ms as u64,
            offline_check_interval_ms: global_config.offline_check_delay_ms as u64,
            offline_check_count: global_config.offline_check_count as u32,
            supervisor_config: crate::scheduler::actor::SupervisorConfig::default(),
        };

        // Create scheduler with StreamMonitor for real status checking
        let scheduler = Arc::new(tokio::sync::RwLock::new(
            Scheduler::with_monitor_and_config(
                streamer_manager.clone(),
                event_broadcaster.clone(),
                stream_monitor.clone(),
                scheduler_config,
                cancellation_token.child_token(),
            )
            .with_config_repo(config_repo.clone()),
        ));

        info!("Service container initialized");

        Ok(Self {
            pool,
            write_pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            danmu_service,
            notification_service,
            notification_repository,
            web_push_service,
            metrics_collector,
            health_checker,
            maintenance_scheduler,
            scheduler,
            stream_monitor,
            credential_service,
            api_server_config: ApiServerConfig::from_env_or_default(),
            cancellation_token,
            logging_config: std::sync::OnceLock::new(),
            discarded_segment_keys: Arc::new(DashMap::new()),
            scheduler_task_handle: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    /// Create a new service container with custom download and pipeline configs.
    #[allow(clippy::too_many_arguments)]
    pub async fn with_full_config(
        pool: SqlitePool,
        write_pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
        download_config: DownloadManagerConfig,
        pipeline_config: PipelineManagerConfig,
        danmu_config: DanmuServiceConfig,
        api_config: ApiServerConfig,
    ) -> Result<Self> {
        let overall = Instant::now();
        info!("Initializing service container with full configuration");

        // Create repositories
        let repos_start = Instant::now();
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone(), write_pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(
            pool.clone(),
            write_pool.clone(),
        ));
        let repos_ms = repos_start.elapsed().as_millis();

        // Load global config early for initial runtime knobs (worker pools, scheduler timing, etc.).
        let global_config_start = Instant::now();
        let global_config = config_repo.get_global_config().await?;
        let global_config_ms = global_config_start.elapsed().as_millis();

        // Create shared event broadcaster
        let event_broadcaster_start = Instant::now();
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);
        let event_broadcaster_ms = event_broadcaster_start.elapsed().as_millis();

        // Create additional repositories for StreamMonitor
        let monitor_repos_start = Instant::now();
        let filter_repo = Arc::new(SqlxFilterRepository::new(pool.clone(), write_pool.clone()));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), write_pool.clone()));
        let monitor_repos_ms = monitor_repos_start.elapsed().as_millis();

        // Create config service with custom cache
        let config_service_start = Instant::now();
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(ConfigService::with_cache_and_broadcaster(
            config_repo.clone(),
            streamer_repo.clone(),
            cache,
            event_broadcaster.clone(),
        ));
        let config_service_ms = config_service_start.elapsed().as_millis();

        // Create streamer manager
        let streamer_manager_start = Instant::now();
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            event_broadcaster.clone(),
        ));
        let streamer_manager_ms = streamer_manager_start.elapsed().as_millis();

        // Create stream monitor for real status detection
        let stream_monitor_start = Instant::now();
        let mut stream_monitor = StreamMonitor::new(
            streamer_manager.clone(),
            filter_repo,
            session_repo.clone(),
            config_service.clone(),
            write_pool.clone(),
        );
        let stream_monitor_ms = stream_monitor_start.elapsed().as_millis();

        // Build credential refresh service (shared between StreamMonitor + API).
        let credential_service_start = Instant::now();
        let credential_resolver = Arc::new(CredentialResolver::new(config_repo.clone()));
        let credential_store = Arc::new(SqlxCredentialStore::new(pool.clone(), write_pool.clone()));
        let mut credential_service =
            CredentialRefreshService::new(credential_resolver, credential_store);
        match BilibiliCredentialManager::new_lazy() {
            Ok(manager) => credential_service.register_manager(Arc::new(manager)),
            Err(e) => warn!(error = %e, "Failed to init bilibili credential manager; skipping"),
        }
        let credential_service = Arc::new(credential_service);
        stream_monitor.set_credential_service(Arc::clone(&credential_service));
        let stream_monitor = Arc::new(stream_monitor);
        let credential_service_ms = credential_service_start.elapsed().as_millis();

        // Create download manager with custom config, overridden by global config for concurrency.
        let download_manager_start = Instant::now();
        let mut effective_download_config = download_config.clone();
        effective_download_config.max_concurrent_downloads =
            (global_config.max_concurrent_downloads as i64).max(1) as usize;
        let download_manager = Arc::new(
            DownloadManager::with_config(effective_download_config)
                .with_config_repo(config_repo.clone()),
        );
        let download_manager_ms = download_manager_start.elapsed().as_millis();

        // Create job repository for pipeline persistence
        let pipeline_repo_start = Instant::now();
        let job_repo = Arc::new(SqlxJobRepository::new(pool.clone(), write_pool.clone()));

        // Create job preset repository
        let preset_repo = Arc::new(SqliteJobPresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));

        // Create pipeline preset repository (for workflow expansion)
        let pipeline_preset_repo = Arc::new(SqlitePipelinePresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));
        let pipeline_repo_ms = pipeline_repo_start.elapsed().as_millis();

        // Create pipeline manager with job repository for database persistence.
        // Wire global-config concurrency knobs into CPU/IO worker pool sizes.
        let pipeline_manager_start = Instant::now();
        let mut effective_pipeline_config = pipeline_config;
        effective_pipeline_config.cpu_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_cpu_jobs);
        effective_pipeline_config.io_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_io_jobs);

        // Apply global-config pipeline timeouts (startup-only).
        effective_pipeline_config.cpu_pool.job_timeout_secs =
            global_config.pipeline_cpu_job_timeout_secs.max(1) as u64;
        effective_pipeline_config.io_pool.job_timeout_secs =
            global_config.pipeline_io_job_timeout_secs.max(1) as u64;
        effective_pipeline_config.execute_timeout_secs =
            global_config.pipeline_execute_timeout_secs.max(1) as u64;
        let pipeline_manager = Arc::new(
            PipelineManager::with_repository(effective_pipeline_config, job_repo)
                .with_session_repository(session_repo.clone())
                .with_streamer_repository(streamer_repo.clone())
                .with_preset_repository(preset_repo)
                .with_pipeline_preset_repository(pipeline_preset_repo)
                .with_config_service(config_service.clone())
                .with_dag_repository(Arc::new(SqlxDagRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ))),
        );
        let pipeline_manager_ms = pipeline_manager_start.elapsed().as_millis();

        // Get monitor event broadcaster
        let monitor_event_broadcaster_start = Instant::now();
        let monitor_event_broadcaster = stream_monitor.event_broadcaster().clone();
        let monitor_event_broadcaster_ms = monitor_event_broadcaster_start.elapsed().as_millis();

        // Create danmu service with custom config
        let danmu_service_start = Instant::now();
        let danmu_service =
            Arc::new(DanmuService::new(danmu_config).with_session_repository(session_repo));
        let danmu_service_ms = danmu_service_start.elapsed().as_millis();

        // Create notification service with default config
        let notification_service_start = Instant::now();
        let notification_repository = Arc::new(SqlxNotificationRepository::new(
            pool.clone(),
            write_pool.clone(),
        ));
        let web_push_service = WebPushService::from_env(pool.clone(), write_pool.clone())
            .unwrap_or_else(|e| {
                warn!(error = %e, "Web push service disabled due to configuration error");
                None
            })
            .map(Arc::new);

        let mut notification_service = NotificationService::with_repository(
            NotificationServiceConfig::default(),
            notification_repository.clone(),
        );
        if let Some(web_push) = web_push_service.clone() {
            notification_service = notification_service.with_web_push_service(web_push);
        }
        let notification_service = Arc::new(notification_service);
        notification_service.start_web_push_worker();
        let notification_service_ms = notification_service_start.elapsed().as_millis();
        let web_push_enabled = web_push_service.is_some();

        // Create metrics collector
        let metrics_collector_start = Instant::now();
        let metrics_collector = Arc::new(MetricsCollector::new());
        if let Some(web_push) = web_push_service.as_ref() {
            web_push.set_metrics_collector(metrics_collector.clone());
        }
        let metrics_collector_ms = metrics_collector_start.elapsed().as_millis();

        // Create health checker
        let health_checker_start = Instant::now();
        let health_checker = Arc::new(HealthChecker::new());
        let health_checker_ms = health_checker_start.elapsed().as_millis();

        // Create database maintenance scheduler (retention settings are user-configurable via global config)
        let maintenance_scheduler_start = Instant::now();
        let maintenance_config = MaintenanceConfig {
            job_retention_days: global_config.job_history_retention_days.max(0),
            notification_event_log_retention_days: global_config
                .notification_event_log_retention_days
                .max(0),
            ..Default::default()
        };
        let maintenance_scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            write_pool.clone(),
            maintenance_config,
        ));
        let maintenance_scheduler_ms = maintenance_scheduler_start.elapsed().as_millis();

        // Create cancellation token for graceful shutdown (before scheduler so it can be shared)
        let cancellation_token_start = Instant::now();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_ms = cancellation_token_start.elapsed().as_millis();

        let scheduler_config = crate::scheduler::SchedulerConfig {
            check_interval_ms: global_config.streamer_check_delay_ms as u64,
            offline_check_interval_ms: global_config.offline_check_delay_ms as u64,
            offline_check_count: global_config.offline_check_count as u32,
            supervisor_config: crate::scheduler::actor::SupervisorConfig::default(),
        };

        // Create scheduler with StreamMonitor for real status checking
        let scheduler_start = Instant::now();
        let scheduler = Arc::new(tokio::sync::RwLock::new(
            Scheduler::with_monitor_and_config(
                streamer_manager.clone(),
                event_broadcaster.clone(),
                stream_monitor.clone(),
                scheduler_config,
                cancellation_token.child_token(),
            )
            .with_config_repo(config_repo.clone()),
        ));
        let scheduler_ms = scheduler_start.elapsed().as_millis();

        let total_ms = overall.elapsed().as_millis();
        info!(
            startup_container_repos_ms = repos_ms,
            startup_container_global_config_ms = global_config_ms,
            startup_container_event_broadcaster_ms = event_broadcaster_ms,
            startup_container_monitor_repos_ms = monitor_repos_ms,
            startup_container_config_service_ms = config_service_ms,
            startup_container_streamer_manager_ms = streamer_manager_ms,
            startup_container_stream_monitor_ms = stream_monitor_ms,
            startup_container_credential_service_ms = credential_service_ms,
            startup_container_download_manager_ms = download_manager_ms,
            startup_container_pipeline_repos_ms = pipeline_repo_ms,
            startup_container_pipeline_manager_ms = pipeline_manager_ms,
            startup_container_monitor_event_broadcaster_ms = monitor_event_broadcaster_ms,
            startup_container_danmu_service_ms = danmu_service_ms,
            startup_container_notification_service_ms = notification_service_ms,
            startup_container_metrics_collector_ms = metrics_collector_ms,
            startup_container_health_checker_ms = health_checker_ms,
            startup_container_maintenance_scheduler_ms = maintenance_scheduler_ms,
            startup_container_cancellation_token_ms = cancellation_token_ms,
            startup_container_scheduler_ms = scheduler_ms,
            startup_container_total_ms = total_ms,
            web_push_enabled,
            "Startup: service container build summary"
        );

        info!("Service container initialized with full configuration and real status checking");

        Ok(Self {
            pool,
            write_pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            danmu_service,
            notification_service,
            notification_repository,
            web_push_service,
            metrics_collector,
            health_checker,
            maintenance_scheduler,
            scheduler,
            stream_monitor,
            credential_service,
            api_server_config: api_config,
            cancellation_token,
            logging_config: std::sync::OnceLock::new(),
            discarded_segment_keys: Arc::new(DashMap::new()),
            scheduler_task_handle: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    /// Initialize all services (hydrate data, start background tasks, etc.).
    pub async fn initialize(&self) -> Result<()> {
        let overall = Instant::now();
        info!("Initializing services");

        let hydrate_start = Instant::now();
        let (streamer_count, recovered_jobs) = tokio::try_join!(
            self.streamer_manager.hydrate(),
            self.pipeline_manager.recover_jobs(),
        )?;

        let hydrate_recover_ms = hydrate_start.elapsed().as_millis();

        info!(
            elapsed_ms = hydrate_recover_ms,
            "Startup: hydrate streamers + recover jobs"
        );

        info!("Hydrated {} streamers", streamer_count);

        // Recover jobs from database on startup.
        // This resets PROCESSING jobs to PENDING for re-execution.
        // For sequential pipelines, no special handling is needed since only one job
        // per pipeline exists at a time.
        if recovered_jobs > 0 {
            info!("Recovered {} jobs from database", recovered_jobs);
        }

        // Start pipeline manager
        let pipeline_start = Instant::now();
        self.pipeline_manager.clone().start();
        let pipeline_start_ms = pipeline_start.elapsed().as_millis();
        info!(
            elapsed_ms = pipeline_start_ms,
            "Startup: pipeline manager started"
        );

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

        // Load notification channels/subscriptions from DB (best-effort) and register health checks.
        // Neither is required for the core runtime to start, so keep them concurrent.
        let health_checks_start = Instant::now();
        let (reload_result, _) = tokio::join!(
            self.notification_service.reload_from_db(),
            self.register_health_checks(),
        );
        let notifications_health_checks_ms = health_checks_start.elapsed().as_millis();
        if let Err(e) = reload_result {
            warn!("Failed to load notification configuration from DB: {}", e);
        }
        info!(
            elapsed_ms = notifications_health_checks_ms,
            "Startup: notifications + health checks"
        );

        // Start database maintenance scheduler
        let maintenance_start = Instant::now();
        self.maintenance_scheduler.clone().start();
        let maintenance_start_ms = maintenance_start.elapsed().as_millis();
        info!("Database maintenance scheduler started");

        // Start scheduler in background
        let scheduler_start = Instant::now();
        self.start_scheduler().await;

        let scheduler_start_ms = scheduler_start.elapsed().as_millis();

        info!(
            elapsed_ms = scheduler_start_ms,
            "Startup: scheduler task started"
        );

        let total_ms = overall.elapsed().as_millis();
        info!(elapsed_ms = total_ms, "Services initialized");

        info!(
            startup_hydrate_recover_ms = hydrate_recover_ms,
            startup_pipeline_start_ms = pipeline_start_ms,
            startup_notifications_health_checks_ms = notifications_health_checks_ms,
            startup_maintenance_start_ms = maintenance_start_ms,
            startup_scheduler_start_ms = scheduler_start_ms,
            startup_total_ms = total_ms,
            streamer_count,
            recovered_jobs,
            "Startup: initialize summary"
        );
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
        let handle = tokio::spawn(async move {
            let mut guard = scheduler.write().await;
            if let Err(e) = guard.run().await {
                tracing::error!("Scheduler error: {}", e);
            }
        });

        // Store the handle for graceful shutdown
        *self.scheduler_task_handle.lock().await = Some(handle);

        info!("Scheduler started");
    }

    /// Initialize and start the API server.
    /// This should be called after initialize() and runs the server in the background.
    pub async fn start_api_server(&self) -> Result<()> {
        let _ = self.start_api_server_bound().await?;
        Ok(())
    }

    /// Initialize and start the API server, returning the resolved bind address.
    ///
    /// This is required when binding to port `0` (ephemeral port), where the actual port is only
    /// known after binding.
    pub async fn start_api_server_bound(&self) -> Result<std::net::SocketAddr> {
        // Create AuthConfig from environment first (single source of truth for token expiration)
        let auth_config = AuthConfig::from_env();

        let jwt_service =
            JwtService::from_env(auth_config.access_token_expiration_secs).map(Arc::new);

        // Create AuthService if JWT is configured
        let auth_service = if let Some(ref jwt) = jwt_service {
            // Create user and refresh token repositories
            let user_repo = Arc::new(SqlxUserRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            ));
            let token_repo = Arc::new(SqlxRefreshTokenRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            ));

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

        // Wire credential refresh service into AppState for API endpoints.
        state = state.with_credential_service(self.credential_service.clone());

        // Wire SessionRepository, FilterRepository, and PipelinePresetRepository into AppState
        state = state
            .with_session_repository(Arc::new(SqlxSessionRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_filter_repository(Arc::new(SqlxFilterRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_streamer_repository(Arc::new(SqlxStreamerRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_pipeline_preset_repository(Arc::new(SqlitePipelinePresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )))
            .with_job_preset_repository(Arc::new(SqliteJobPresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )))
            .with_notification_repository(self.notification_repository.clone())
            .with_notification_service(self.notification_service.clone());

        if let Some(web_push) = self.web_push_service.clone() {
            state = state.with_web_push_service(web_push);
        }

        // Wire logging config if available
        if let Some(logging_config) = self.logging_config.get().cloned() {
            state = state.with_logging_config(logging_config);
        }

        let server = ApiServer::with_state(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        tokio::spawn(async move {
            if let Err(e) = server.run_with_listener(listener).await {
                tracing::error!("API server error: {}", e);
            }
        });

        Ok(local_addr)
    }

    /// Initialize and start the API server, returning the resolved bind address, using a
    /// caller-provided JWT secret.
    ///
    /// This is primarily intended for the desktop (Tauri) wrapper, which should not depend on
    /// `.env` loading / shell environment setup for authentication to work.
    pub async fn start_api_server_bound_with_jwt_secret(
        &self,
        jwt_secret: String,
    ) -> Result<std::net::SocketAddr> {
        let auth_config = AuthConfig::from_env();

        let issuer = std::env::var("JWT_ISSUER").unwrap_or_else(|_| "rust-srec".to_string());
        let audience =
            std::env::var("JWT_AUDIENCE").unwrap_or_else(|_| "rust-srec-api".to_string());
        let jwt_service = Arc::new(JwtService::new(
            &jwt_secret,
            &issuer,
            &audience,
            Some(auth_config.access_token_expiration_secs),
        ));

        // Create AuthService (always enabled when a JWT secret is provided)
        let user_repo = Arc::new(SqlxUserRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        ));
        let token_repo = Arc::new(SqlxRefreshTokenRepository::new(
            self.pool.clone(),
            self.write_pool.clone(),
        ));
        let auth_svc = AuthService::new(user_repo, token_repo, jwt_service.clone(), auth_config);
        info!(
            issuer = %issuer,
            audience = %audience,
            "AuthService initialized with desktop-provided JWT secret"
        );

        let mut state = AppState::with_services(
            Some(jwt_service),
            self.config_service.clone(),
            self.streamer_manager.clone(),
            self.pipeline_manager.clone(),
            self.danmu_service.clone(),
            self.download_manager.clone(),
        )
        .with_auth_service(Arc::new(auth_svc));

        // Wire HealthChecker into AppState for health endpoints
        state = state.with_health_checker(self.health_checker.clone());

        // Wire credential refresh service into AppState for API endpoints.
        state = state.with_credential_service(self.credential_service.clone());

        // Wire SessionRepository, FilterRepository, and PipelinePresetRepository into AppState
        state = state
            .with_session_repository(Arc::new(SqlxSessionRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_filter_repository(Arc::new(SqlxFilterRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_streamer_repository(Arc::new(SqlxStreamerRepository::new(
                self.pool.clone(),
                self.write_pool.clone(),
            )))
            .with_pipeline_preset_repository(Arc::new(SqlitePipelinePresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )))
            .with_job_preset_repository(Arc::new(SqliteJobPresetRepository::new(
                Arc::new(self.pool.clone()),
                Arc::new(self.write_pool.clone()),
            )))
            .with_notification_repository(self.notification_repository.clone())
            .with_notification_service(self.notification_service.clone());

        if let Some(web_push) = self.web_push_service.clone() {
            state = state.with_web_push_service(web_push);
        }

        // Wire logging config if available
        if let Some(logging_config) = self.logging_config.get().cloned() {
            state = state.with_logging_config(logging_config);
        }

        let server = ApiServer::with_state(self.api_server_config.clone(), state);
        let cancel_token = self.cancellation_token.clone();

        // Link server shutdown to container shutdown
        let server_cancel = server.cancel_token();
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            server_cancel.cancel();
        });

        let (listener, local_addr) = server.bind().await?;
        info!("Starting API server on http://{}", local_addr);

        tokio::spawn(async move {
            if let Err(e) = server.run_with_listener(listener).await {
                tracing::error!("API server error: {}", e);
            }
        });

        Ok(local_addr)
    }

    /// Set up config event subscriptions between services.
    fn setup_config_event_subscriptions(&self) {
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let download_manager = self.download_manager.clone();
        let pipeline_manager = self.pipeline_manager.clone();
        let danmu_service = self.danmu_service.clone();
        let stream_monitor = self.stream_monitor.clone();
        let mut receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        // Spawn a task to handle config update events
        tokio::spawn(async move {
            use crate::config::ConfigUpdateEvent;

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
                                    ConfigUpdateEvent::StreamerMetadataUpdated { streamer_id } => {
                                        // Ensure merged config cache is not stale after streamer/template/platform changes.
                                        config_service.invalidate_streamer(&streamer_id);

                                        // Config update event - handles name, URL, priority, template changes.
                                        // If the update includes a state transition to an inactive state
                                        // (e.g., user disables a streamer via API), we must still perform
                                        // best-effort cleanup to stop active downloads and danmu collection.
                                        debug!(
                                            "Received streamer config update event: {}",
                                            streamer_id
                                        );

                                        // Align with ConfigUpdateEvent docs: handlers should check
                                        // `metadata.is_active()` to determine if cleanup is needed.
                                        match streamer_manager.get_streamer(&streamer_id) {
                                            Some(metadata) if !metadata.is_active() => {
                                                info!(
                                                    "Streamer {} is inactive after update (state: {}), initiating cleanup",
                                                    streamer_id, metadata.state
                                                );
                                                Self::handle_streamer_disabled(
                                                    &download_manager,
                                                    &danmu_service,
                                                    &stream_monitor,
                                                    &streamer_id,
                                                )
                                                .await;
                                            }
                                            Some(_) => {}
                                            None => {
                                                // Streamer not in memory (race with delete/hydration issues).
                                                // Best-effort cleanup anyway.
                                                warn!(
                                                    "Streamer {} not found after update, initiating best-effort cleanup",
                                                    streamer_id
                                                );
                                                Self::handle_streamer_disabled(
                                                    &download_manager,
                                                    &danmu_service,
                                                    &stream_monitor,
                                                    &streamer_id,
                                                )
                                                .await;
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

                                        match config_service.get_global_config().await {
                                            Ok(global) => {
                                                let new_limit =
                                                    (global.max_concurrent_downloads as i64)
                                                        .max(1)
                                                        as usize;
                                                let old_limit =
                                                    download_manager.max_concurrent_downloads();

                                                if new_limit != old_limit {
                                                    download_manager
                                                        .set_max_concurrent_downloads(new_limit);
                                                    info!(
                                                        "Updated download concurrency: max_concurrent_downloads {} -> {}",
                                                        old_limit, new_limit
                                                    );
                                                }

                                                // Wire CPU/IO pipeline job concurrency knobs (best-effort).
                                                let cpu_jobs = autoscale_concurrency_limit(
                                                    global.max_concurrent_cpu_jobs,
                                                );
                                                let io_jobs =
                                                    autoscale_concurrency_limit(
                                                        global.max_concurrent_io_jobs,
                                                    );
                                                pipeline_manager
                                                    .set_worker_concurrency(cpu_jobs, io_jobs);
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "Failed to reload global config for download concurrency: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                    ConfigUpdateEvent::StreamerDeleted { streamer_id } => {
                                        // Best-effort: drop any stale cache entry (usually already removed).
                                        config_service.invalidate_streamer(&streamer_id);

                                        info!(
                                            "Streamer {} deleted, initiating cleanup",
                                            streamer_id
                                        );
                                        // Reuse the same cleanup logic as disabled state
                                        Self::handle_streamer_disabled(
                                            &download_manager,
                                            &danmu_service,
                                            &stream_monitor,
                                            &streamer_id,
                                        ).await;
                                    }
                                    ConfigUpdateEvent::EngineUpdated { engine_id } => {
                                        debug!(
                                            "Received engine config update event: {}",
                                            engine_id
                                        );
                                    }
                                    ConfigUpdateEvent::StreamerStateSyncedFromDb { streamer_id, is_active } => {
                                        debug!(
                                            "Received streamer state change event: {} (active={})",
                                            streamer_id, is_active
                                        );
                                        // If streamer became inactive (error, disabled, etc.), clean up
                                        if !is_active {
                                            info!("Streamer {} became inactive, initiating cleanup", streamer_id);
                                            Self::handle_streamer_disabled(
                                                &download_manager,
                                                &danmu_service,
                                                &stream_monitor,
                                                &streamer_id,
                                            ).await;
                                        }
                                    }
                                    ConfigUpdateEvent::StreamerFiltersUpdated { streamer_id } => {
                                        // Filters are evaluated by StreamMonitor on each check, but changing
                                        // them can affect OutOfSchedule smart-wake behavior. Invalidate merged
                                        // config and let the scheduler/actors re-check soon.
                                        config_service.invalidate_streamer(&streamer_id);
                                        debug!(
                                            "Received streamer filters update event: {}",
                                            streamer_id
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
        let danmu_service = self.danmu_service.clone();
        let config_service = self.config_service.clone();
        let discarded_segment_keys = self.discarded_segment_keys.clone();
        let mut receiver = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        const DOWNLOAD_EVENT_QUEUE_CAPACITY: usize = 8192;
        let (event_tx, mut event_rx) =
            tokio::sync::mpsc::channel::<DownloadManagerEvent>(DOWNLOAD_EVENT_QUEUE_CAPACITY);

        // Prevent unbounded growth if danmu events are missed (best-effort cleanup).
        let cleanup_token = cancellation_token.clone();
        let cleanup_keys = discarded_segment_keys.clone();
        tokio::spawn(async move {
            const CLEANUP_INTERVAL_SECS: u64 = 600;
            const MAX_AGE_SECS: u64 = 3600;
            let mut interval = tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));
            loop {
                tokio::select! {
                    _ = cleanup_token.cancelled() => break,
                    _ = interval.tick() => {
                        cleanup_keys.retain(|_, inserted_at| inserted_at.elapsed() < Duration::from_secs(MAX_AGE_SECS));
                    }
                }
            }
        });

        // Fast path: drain broadcast channel quickly so we don't drop critical session events under backpressure.
        let drain_token = cancellation_token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = drain_token.cancelled() => {
                        debug!("Download event drain shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(download_event) => {
                                // Progress can be extremely frequent; downstream coordination does not need it.
                                if matches!(download_event, DownloadManagerEvent::Progress { .. }) {
                                    continue;
                                }
                                if event_tx.send(download_event).await.is_err() {
                                    break;
                                }
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

        let process_token = cancellation_token.clone();
        tokio::spawn(async move {
            while let Some(download_event) = event_rx.recv().await {
                if process_token.is_cancelled() {
                    debug!("Download event processor shutting down");
                    break;
                }

                // Handle download failure for error tracking
                if let DownloadManagerEvent::DownloadFailed {
                    ref streamer_id,
                    ref session_id,
                    ref error,
                    ..
                } = download_event
                {
                    // Record error for exponential backoff
                    if let Some(metadata) = streamer_manager.get_streamer(streamer_id) {
                        if let Err(e) = stream_monitor.handle_error(&metadata, error).await {
                            warn!("Failed to record download error for {}: {}", streamer_id, e);
                        } else {
                            debug!("Recorded download error for {}: {}", streamer_id, error);
                        }
                    }

                    // Stop danmu collection when download fails
                    if danmu_service.is_collecting(session_id) {
                        match danmu_service.stop_collection(session_id).await {
                            Ok(stats) => {
                                info!(
                                    "Stopped danmu collection after download failure for session {}: {} messages",
                                    session_id, stats.total_count
                                );
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to stop danmu collection for session {}: {}",
                                    session_id, e
                                );
                            }
                        }
                    }
                }

                // Handle download cancellation - stop danmu collection
                if let DownloadManagerEvent::DownloadCancelled { ref session_id, .. } =
                    download_event
                    && danmu_service.is_collecting(session_id)
                {
                    match danmu_service.stop_collection(session_id).await {
                        Ok(stats) => {
                            info!(
                                "Stopped danmu collection after download cancelled for session {}: {} messages",
                                session_id, stats.total_count
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to stop danmu collection for session {}: {}",
                                session_id, e
                            );
                        }
                    }
                }

                // Handle danmu segmentation
                match &download_event {
                    DownloadManagerEvent::SegmentStarted {
                        session_id,
                        segment_path,
                        segment_index,
                        ..
                    } => {
                        if let Some(handle) = danmu_service.get_handle(session_id) {
                            let path = std::path::Path::new(segment_path);
                            let segment_id = segment_index.to_string();

                            // Start danmu segment
                            // We change extension to .xml for danmu file
                            let mut danmu_path = path.to_path_buf();
                            danmu_path.set_extension("xml");

                            if let Err(e) = handle
                                .start_segment(&segment_id, danmu_path, chrono::Utc::now())
                                .await
                            {
                                warn!("Failed to start danmu segment: {}", e);
                            }
                        }
                    }
                    DownloadManagerEvent::SegmentCompleted {
                        session_id,
                        streamer_id,
                        segment_path,
                        segment_index,
                        size_bytes,
                        ..
                    } => {
                        // Decide discard *before* ending danmu segment so we can suppress the
                        // imminent DanmuEvent::SegmentCompleted (avoids pipeline race with deletion).
                        let mut discard = false;
                        let effective_size_bytes = tokio::fs::metadata(segment_path)
                            .await
                            .map(|m| m.len())
                            .unwrap_or(*size_bytes);

                        // Resolve config to check min_size.
                        match config_service.get_config_for_streamer(streamer_id).await {
                            Ok(config) => {
                                let min = u64::try_from(config.min_segment_size_bytes)
                                    .ok()
                                    .filter(|v| *v > 0);
                                if let Some(min) = min
                                    && effective_size_bytes < min
                                {
                                    info!(
                                        "Segment {} is too small ({} bytes < min {}), discarding",
                                        segment_path, effective_size_bytes, min
                                    );
                                    discard = true;
                                    discarded_segment_keys.insert(
                                        (session_id.clone(), segment_index.to_string()),
                                        Instant::now(),
                                    );
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Failed to resolve config for streamer {} during segment completion: {}",
                                    streamer_id, e
                                );
                            }
                        }

                        // Always finish the danmu segment first (Flush/Close XML).
                        if let Some(handle) = danmu_service.get_handle(session_id) {
                            let segment_id = segment_index.to_string();

                            if let Err(e) = handle.end_segment(&segment_id).await {
                                warn!("Failed to end danmu segment: {}", e);
                            }
                        }

                        if discard {
                            let path = std::path::Path::new(segment_path);
                            match tokio::fs::remove_file(path).await {
                                Ok(()) => debug!("Deleted small segment: {}", segment_path),
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                Err(e) => {
                                    warn!("Failed to delete small segment {}: {}", segment_path, e)
                                }
                            }

                            let mut danmu_path = path.to_path_buf();
                            danmu_path.set_extension("xml");
                            match tokio::fs::remove_file(&danmu_path).await {
                                Ok(()) => {
                                    debug!("Deleted small segment danmu: {}", danmu_path.display())
                                }
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                Err(e) => warn!(
                                    "Failed to delete small segment danmu {}: {}",
                                    danmu_path.display(),
                                    e
                                ),
                            }
                            continue;
                        }
                    }
                    _ => {}
                }

                // Forward to pipeline manager
                pipeline_manager
                    .handle_download_event(download_event.clone())
                    .await;
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
        let pipeline_manager = self.pipeline_manager.clone();
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let stream_monitor = self.stream_monitor.clone();
        let discarded_segment_keys = self.discarded_segment_keys.clone();
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
                                match &event {
                                    DanmuEvent::CollectionStarted { session_id, streamer_id } => {
                                        info!(
                                            "Danmu collection started for session {} (streamer: {})",
                                            session_id, streamer_id
                                        );
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::CollectionStopped { session_id, statistics } => {
                                        info!(
                                            "Danmu collection stopped for session {}: {} messages",
                                            session_id, statistics.total_count
                                        );
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::SegmentStarted { session_id, segment_id, output_path, start_time, .. } => {
                                        debug!(
                                            "Danmu segment started: session={}, segment={}, path={:?}, start_time={}",
                                            session_id, segment_id, output_path, start_time
                                        );
                                    }
                                    DanmuEvent::SegmentCompleted { session_id, segment_id, output_path, message_count, .. } => {
                                        info!(
                                            "Danmu segment completed: session={}, segment={}, messages={}",
                                            session_id, segment_id, message_count
                                        );
                                        if discarded_segment_keys
                                            .remove(&(session_id.clone(), segment_id.clone()))
                                            .is_some()
                                        {
                                            match tokio::fs::remove_file(output_path).await {
                                                Ok(()) => debug!(
                                                    "Deleted discarded danmu segment: {}",
                                                    output_path.display()
                                                ),
                                                Err(e)
                                                    if e.kind() == std::io::ErrorKind::NotFound => {}
                                                Err(e) => warn!(
                                                    "Failed to delete discarded danmu segment {}: {}",
                                                    output_path.display(),
                                                    e
                                                ),
                                            }
                                            debug!(
                                                "Skipping danmu segment {} for session {} (paired video discarded)",
                                                segment_id, session_id
                                            );
                                            continue;
                                        }
                                        // Forward to pipeline manager for processing
                                        pipeline_manager.handle_danmu_event(event.clone()).await;
                                    }
                                    DanmuEvent::Control { session_id, streamer_id, platform, control } => {
                                        warn!(
                                            "Danmu control event for session {} (streamer={} platform={}): {:?}",
                                            session_id, streamer_id, platform, control
                                        );

                                        // Forward to pipeline manager (e.g., title updates).
                                        pipeline_manager.handle_danmu_event(event.clone()).await;

                                        // Treat stream-closed as authoritative end-of-stream:
                                        // - stop downloads promptly
                                        // - end session and bypass resume hysteresis
                                        if matches!(control, crate::danmu::DanmuControlEvent::StreamClosed { .. }) {
                                            let should_end_stream = match config_service
                                                .get_platform_config_by_name(platform)
                                                .await
                                            {
                                                Ok(platform_config) => {
                                                    should_end_stream_on_danmu_stream_closed(
                                                        platform_config
                                                            .platform_specific_config
                                                            .as_deref(),
                                                    )
                                                }
                                                Err(e) => {
                                                    warn!(
                                                        "Failed to load platform config for '{}' while handling danmu stream closed: {}",
                                                        platform, e
                                                    );
                                                    true
                                                }
                                            };

                                            if !should_end_stream {
                                                info!(
                                                    session_id = %session_id,
                                                    streamer_id = %streamer_id,
                                                    platform = %platform,
                                                    "Ignoring danmu stream-closed signal due to platform config"
                                                );
                                                continue;
                                            }

                                            debug!(
                                                session_id = %session_id,
                                                streamer_id = %streamer_id,
                                                "Danmu stream closed; forcing end-of-stream handling"
                                            );

                                            if let Some(download_info) =
                                                download_manager.get_download_by_streamer(streamer_id)
                                            {
                                                match download_manager
                                                    .stop_download_with_reason(
                                                        &download_info.id,
                                                        crate::downloader::DownloadStopCause::DanmuStreamClosed,
                                                    )
                                                    .await
                                                {
                                                    Ok(()) => info!(
                                                        session_id = %session_id,
                                                        streamer_id = %streamer_id,
                                                        download_id = %download_info.id,
                                                        "Stopped download after danmu stream closed"
                                                    ),
                                                    Err(e) => warn!(
                                                        "Failed to stop download {} after danmu stream closed (streamer={}): {}",
                                                        download_info.id, streamer_id, e
                                                    ),
                                                }
                                            } else {
                                                debug!(
                                                    session_id = %session_id,
                                                    streamer_id = %streamer_id,
                                                    "No active download found to stop after danmu stream closed"
                                                );
                                            }

                                            stream_monitor.mark_session_hard_ended(streamer_id, session_id);
                                            if let Some(streamer) = streamer_manager.get_streamer(streamer_id) {
                                                if let Err(e) = stream_monitor
                                                    .handle_offline_with_session(&streamer, Some(session_id.clone()))
                                                    .await
                                                {
                                                    warn!(
                                                        "Failed to mark streamer offline after danmu stream closed (streamer={} session={}): {}",
                                                        streamer_id, session_id, e
                                                    );
                                                }
                                            } else {
                                                warn!(
                                                    "Streamer metadata not found for stream-closed danmu control (streamer={} session={})",
                                                    streamer_id, session_id
                                                );
                                            }
                                        }
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
        use std::path::{Path, PathBuf};

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

        fn best_disk_for_path<'a>(
            disks: &'a sysinfo::Disks,
            path: &Path,
        ) -> Option<&'a sysinfo::Disk> {
            disks
                .iter()
                .filter(|d| path.starts_with(d.mount_point()))
                .max_by_key(|d| d.mount_point().as_os_str().to_string_lossy().len())
        }

        fn sqlite_file_path_from_url(url: &str) -> Option<PathBuf> {
            let url = url.strip_prefix("sqlite:")?;
            let path_part = url.split('?').next().unwrap_or(url);

            if path_part.is_empty() || path_part == ":memory:" || path_part.starts_with(":memory:")
            {
                return None;
            }

            let normalized = path_part.strip_prefix("///").unwrap_or(path_part);
            Some(PathBuf::from(normalized))
        }

        // Disk space health checks (output dir and DB directory).
        let output_dir = std::env::var("OUTPUT_DIR").unwrap_or_else(|_| "./output".to_string());
        // Ensure path is absolute for disk lookup
        let output_dir_path = if let Ok(cwd) = std::env::current_dir() {
            cwd.join(&output_dir)
        } else {
            PathBuf::from(output_dir.clone())
        };

        let disk_checker = self.health_checker.clone();
        self.health_checker
            .register(
                format!("disk:{}", output_dir),
                Arc::new(move || {
                    let disks = sysinfo::Disks::new_with_refreshed_list();
                    let disk = best_disk_for_path(&disks, &output_dir_path);
                    match disk {
                        Some(d) => disk_checker.check_disk_space(
                            &output_dir,
                            d.available_space(),
                            d.total_space(),
                        ),
                        None => ComponentHealth {
                            name: format!("disk:{}", output_dir),
                            status: crate::metrics::HealthStatus::Unknown,
                            message: Some("Unable to resolve disk for path".to_string()),
                            last_check: Some(chrono::Utc::now().to_rfc3339()),
                            check_duration_ms: None,
                        },
                    }
                }),
            )
            .await;

        if let Ok(database_url) = std::env::var("DATABASE_URL")
            && let Some(db_file) = sqlite_file_path_from_url(&database_url)
        {
            let db_dir = db_file.parent().unwrap_or(db_file.as_path()).to_path_buf();
            let db_dir_str = db_dir.to_string_lossy().to_string();
            // Ensure path is absolute for disk lookup
            let db_dir_path = if db_dir.is_absolute() {
                db_dir.clone()
            } else if let Ok(cwd) = std::env::current_dir() {
                cwd.join(&db_dir)
            } else {
                db_dir.clone()
            };
            let disk_checker = self.health_checker.clone();
            self.health_checker
                .register(
                    format!("disk:{}", db_dir_str),
                    Arc::new(move || {
                        let disks = sysinfo::Disks::new_with_refreshed_list();
                        let disk = best_disk_for_path(&disks, &db_dir_path);
                        match disk {
                            Some(d) => disk_checker.check_disk_space(
                                &db_dir_str,
                                d.available_space(),
                                d.total_space(),
                            ),
                            None => ComponentHealth {
                                name: format!("disk:{}", db_dir_str),
                                status: crate::metrics::HealthStatus::Unknown,
                                message: Some("Unable to resolve disk for path".to_string()),
                                last_check: Some(chrono::Utc::now().to_rfc3339()),
                                check_duration_ms: None,
                            },
                        }
                    }),
                )
                .await;
        }

        // Download manager health check
        let dm = download_manager.clone();
        self.health_checker
            .register(
                "download_manager",
                Arc::new(move || {
                    let active = dm.active_count();
                    let total_slots = dm.total_concurrent_slots();

                    if total_slots == 0 {
                        return ComponentHealth::degraded(
                            "download_manager",
                            "No download slots configured (total_concurrent_slots=0)",
                        );
                    }

                    if active > total_slots {
                        return ComponentHealth::unhealthy(
                            "download_manager",
                            format!(
                                "Active downloads exceed capacity: {}/{}",
                                active, total_slots
                            ),
                        );
                    }

                    let cpu_threshold = 85.0_f32;
                    let mem_threshold = 90.0_f32;

                    let (cpu_usage, mem_usage) = {
                        let mut system = sysinfo::System::new_with_specifics(
                            sysinfo::RefreshKind::nothing()
                                .with_cpu(sysinfo::CpuRefreshKind::everything())
                                .with_memory(sysinfo::MemoryRefreshKind::everything()),
                        );
                        system.refresh_cpu_all();
                        system.refresh_memory();

                        let cpu = system.global_cpu_usage();
                        let total_mem = system.total_memory();
                        let used_mem = system.used_memory();
                        let mem = if total_mem > 0 {
                            (used_mem as f64 / total_mem as f64 * 100.0) as f32
                        } else {
                            0.0
                        };
                        (cpu, mem)
                    };

                    let utilization = active as f32 / total_slots as f32;

                    if utilization >= 0.95 && (cpu_usage >= cpu_threshold || mem_usage >= mem_threshold) {
                        ComponentHealth::degraded(
                            "download_manager",
                            format!(
                                "Near capacity under resource pressure: active {}/{}, cpu {:.1}%, mem {:.1}%",
                                active, total_slots, cpu_usage, mem_usage
                            ),
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
    /// This method coordinates cleanup when a streamer is **disabled via UI/API**.
    /// The key challenge is that the actor is removed before it can process the
    /// DownloadCancelled event, so we must explicitly end the session here.
    ///
    /// ## Cleanup Steps
    ///
    /// 1. **End active streaming session** - Close the session in the database
    ///    BEFORE removing the actor. This ensures the session is properly closed
    ///    even though the actor won't be around to process the DownloadCancelled event.
    /// 2. **Remove the streamer actor** - Stop monitoring this streamer
    /// 3. **Cancel active downloads** - Stop any ongoing download tasks
    /// 4. **Stop danmu collection** - Stop any active comment collection
    ///
    /// ## Session Cleanup: Two Scenarios
    ///
    /// This function handles **Scenario 1: Streamer Disable/Delete**:
    /// - User disables/deletes a streamer via UI/API
    /// - Actor is being removed from the scheduler
    /// - We explicitly end the session HERE before actor removal
    /// - DownloadCancelled event sent, but actor is already gone
    ///
    /// **Scenario 2: Manual Download Cancellation** is handled separately by
    /// `StreamerActor::handle_download_ended(Cancelled)`:
    /// - User cancels download without disabling the streamer
    /// - Actor is still active and processes the DownloadCancelled event
    /// - Actor calls `process_status(Offline)` to end the session
    /// - Actor then stops itself
    ///
    /// Both paths are necessary for complete session cleanup coverage.
    ///
    /// ## Error Handling
    ///
    /// All errors are logged but do not propagate - cleanup is best-effort
    /// and should not block other operations.
    ///
    /// # Arguments
    /// * `download_manager` - The download manager to cancel downloads
    /// * `danmu_service` - The danmu service to stop collection
    /// * `stream_monitor` - The stream monitor to end active sessions
    /// * `streamer_id` - The ID of the streamer being disabled
    ///
    /// # Note
    /// Actor removal is handled by the Scheduler's own config event handler.
    /// We don't do it here to avoid RwLock deadlock (scheduler.run() holds the write lock).
    pub async fn handle_streamer_disabled(
        download_manager: &Arc<DownloadManager>,
        danmu_service: &Arc<DanmuService>,
        stream_monitor: &Arc<
            StreamMonitor<
                SqlxStreamerRepository,
                SqlxFilterRepository,
                SqlxSessionRepository,
                SqlxConfigRepository,
            >,
        >,
        streamer_id: &str,
    ) {
        // 1. Cancel active downloads (best-effort).
        //
        // Do this before ending the session so the session's `total_size_bytes` snapshot
        // is less likely to be stale due to late segment persistence.
        let downloads: Vec<_> = download_manager
            .get_active_downloads()
            .into_iter()
            .filter(|d| d.streamer_id == streamer_id)
            .collect();

        if downloads.is_empty() {
            debug!(
                "No active download found for disabled streamer: {}",
                streamer_id
            );
        } else {
            for download in downloads {
                match download_manager
                    .stop_download_with_reason(
                        &download.id,
                        crate::downloader::DownloadStopCause::StreamerDisabled,
                    )
                    .await
                {
                    Ok(()) => {
                        info!(
                            "Cancelled download {} for disabled streamer {}",
                            download.id, streamer_id
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to cancel download {} for disabled streamer {}: {}",
                            download.id, streamer_id, e
                        );
                    }
                }
            }
        }

        // 2. Stop danmu collection if active (best-effort).
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

        // 3. End active streaming session (best-effort).
        //
        // NOTE: We use force_end_active_session instead of handle_offline_with_session
        // because by the time this handler runs, the streamer's in-memory state has
        // already been updated to Disabled by partial_update_streamer.
        if let Err(e) = stream_monitor.force_end_active_session(streamer_id).await {
            warn!(
                "Failed to end session for disabled streamer {}: {}",
                streamer_id, e
            );
        }

        // Note: Actor removal is handled by the Scheduler's own config event handler.
        // We don't do it here because scheduler.run() holds the RwLock write lock forever.
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
                media_extras,
                ..
            } => {
                info!(
                    "Streamer {} ({}) went live: {} ({} streams available, {} media headers, {} media extras)",
                    streamer_name,
                    streamer_id,
                    title,
                    streams.len(),
                    media_headers.as_ref().map(|h| h.len()).unwrap_or(0),
                    media_extras.as_ref().map(|h| h.len()).unwrap_or(0)
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

                // Correctness guard: if the streamer was disabled/cancelled while a live check
                // was in-flight, ignore the live event and don't start downloads/danmu.
                if let Some(metadata) = streamer_manager.get_streamer(&streamer_id)
                    && !metadata.is_active()
                {
                    info!(
                        "Ignoring StreamerLive for inactive streamer {} (state: {})",
                        streamer_id, metadata.state
                    );
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
                    .map(|s| s.priority == Priority::High)
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
                        Arc::new(crate::domain::config::MergedConfig::builder().build())
                    }
                };

                // The detector emits only the selected stream(s), so we take the first one
                let best_stream = &streams[0];
                let stream_url_selected = best_stream.url.clone();
                let stream_format = best_stream.stream_format.as_str();
                let media_format = best_stream.media_format.as_str();

                let mut headers = media_headers.as_ref().cloned().unwrap_or_default();

                // Merge per-stream headers (e.g., Douyu hs-h5 Host override).
                if let Some(extras) = best_stream.extras.as_ref() {
                    if let Some(extra_headers) = extras.get("headers").and_then(|v| v.as_object()) {
                        for (k, v) in extra_headers {
                            if let Some(v) = v.as_str() {
                                headers.insert(k.clone(), v.to_string());
                            }
                        }
                    }

                    // Backward-compat: some extractors use a flat host_header field.
                    if let Some(host_header) = extras.get("host_header").and_then(|v| v.as_str()) {
                        headers.insert("Host".to_string(), host_header.to_string());
                    }
                }

                if !headers.is_empty() {
                    debug!(
                        "Using {} merged headers for download: {:?}",
                        headers.len(),
                        headers.keys().collect::<Vec<_>>()
                    );
                }

                // Sanitize streamer name and title for safe filename usage
                let sanitized_streamer = sanitize_filename(&streamer_name);
                let sanitized_title = sanitize_filename(&title);

                // Extract platform from streamer metadata
                let platform = streamer_metadata
                    .as_ref()
                    .map(|s| s.platform())
                    .unwrap_or("unknown");

                let dir = merged_config
                    .output_folder
                    .replace("{streamer}", &sanitized_streamer)
                    .replace("{title}", &sanitized_title)
                    .replace("{session_id}", &session_id)
                    .replace("{platform}", platform);

                let output_dir = expand_path_template(&dir);

                let mut config = DownloadConfig::new(
                    stream_url_selected.clone(),
                    output_dir.clone(),
                    streamer_id.clone(),
                    streamer_name.clone(),
                    session_id.clone(),
                )
                .with_filename_template(
                    merged_config
                        .output_filename_template
                        .replace("{streamer}", &sanitized_streamer)
                        .replace("{title}", &sanitized_title)
                        .replace("{platform}", platform),
                )
                .with_output_format(&merged_config.output_file_format)
                .with_max_segment_duration(merged_config.max_download_duration_secs as u64)
                .with_max_segment_size(merged_config.max_part_size_bytes as u64)
                .with_engines_override(merged_config.engines_override.clone());

                // Add cookies from merged config if present
                if let Some(ref cookies) = merged_config.cookies {
                    debug!(
                        "Applying cookies from merged config to download (length: {} chars)",
                        cookies.len()
                    );
                    config = config.with_cookies(cookies);
                }

                // Apply proxy settings from merged config
                // Priority: 1) Explicit proxy URL (with auth), 2) System proxy, 3) No proxy
                let proxy_config = &merged_config.proxy_config;
                if proxy_config.enabled {
                    if let Some(effective_proxy_url) = proxy_config.effective_url() {
                        // Explicit proxy URL configured
                        debug!(
                            "Applying explicit proxy from merged config to download: {}",
                            effective_proxy_url
                        );
                        config = config.with_proxy(effective_proxy_url);
                    } else if proxy_config.use_system_proxy {
                        // Use system proxy settings
                        debug!("Enabling system proxy for download");
                        config = config.with_system_proxy(true);
                    }
                    // else: enabled but no URL and no system proxy -> no proxy
                }
                // else: proxy disabled -> use_system_proxy remains false (default)

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

                let cookies = merged_config.cookies.clone();
                // Start download
                match download_manager
                    .start_download(
                        config,
                        Some(merged_config.download_engine.clone()),
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
                        .start_collection(
                            &session_id,
                            &streamer_id,
                            &streamer_url,
                            sampling_config,
                            cookies,
                            media_extras,
                        )
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
                let sid = session_id
                    .filter(|sid| danmu_service.is_collecting(sid))
                    .or_else(|| danmu_service.get_session_by_streamer(&streamer_id));
                if let Some(sid) = sid {
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

                // Stop download if active
                if let Some(download_info) = download_manager.get_download_by_streamer(&streamer_id)
                {
                    match download_manager
                        .stop_download_with_reason(
                            &download_info.id,
                            crate::downloader::DownloadStopCause::StreamerOffline,
                        )
                        .await
                    {
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
            MonitorEvent::StateChanged {
                streamer_id,
                streamer_name,
                new_state: StreamerState::OutOfSchedule,
                reason,
                ..
            } => {
                // Only stop recording when the state transition is due to schedule.
                // Title/category mismatch currently also maps to OutOfSchedule, but we
                // intentionally don't stop downloads for those reasons here.
                if reason.as_deref() != Some("out_of_schedule") {
                    return;
                }

                info!(
                    "Streamer {} ({}) became OutOfSchedule; stopping active download/danmu if any",
                    streamer_name, streamer_id
                );

                // Stop danmu collection if active.
                if let Some(sid) = danmu_service.get_session_by_streamer(&streamer_id) {
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

                // Stop download if active.
                if let Some(download_info) = download_manager.get_download_by_streamer(&streamer_id)
                {
                    match download_manager
                        .stop_download_with_reason(
                            &download_info.id,
                            crate::downloader::DownloadStopCause::OutOfSchedule,
                        )
                        .await
                    {
                        Ok(()) => {
                            info!(
                                "Stopped download {} for streamer {} (out_of_schedule)",
                                download_info.id, streamer_id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to stop download for streamer {} (out_of_schedule): {}",
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

        // Stop stream monitor outbox publisher
        info!("Stopping stream monitor...");
        self.stream_monitor.stop();
        info!("Stream monitor stopped");

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

        // Stop scheduler - wait for it to complete its shutdown sequence
        // (cancellation already triggered via linked token above)
        info!("Stopping scheduler...");

        // Wait for the scheduler task to complete with timeout
        // The scheduler's run() loop will exit on cancellation and call its own shutdown()
        // which waits for all actors to stop gracefully
        if let Some(handle) = self.scheduler_task_handle.lock().await.take() {
            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => {
                    info!("Scheduler stopped gracefully");
                }
                Ok(Err(e)) => {
                    warn!("Scheduler task panicked: {}", e);
                }
                Err(_) => {
                    warn!("Scheduler shutdown timed out");
                }
            }
        } else {
            debug!("No scheduler task handle to wait for");
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

    /// Set the logging configuration
    pub fn set_logging_config(&self, config: Arc<LoggingConfig>) {
        self.logging_config.get_or_init(|| config);
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
    use super::should_end_stream_on_danmu_stream_closed;

    #[test]
    fn test_should_end_stream_on_danmu_stream_closed_defaults_true() {
        assert!(should_end_stream_on_danmu_stream_closed(None));
        assert!(should_end_stream_on_danmu_stream_closed(Some("{}")));
        assert!(should_end_stream_on_danmu_stream_closed(Some(
            "{invalid json"
        )));
    }

    #[test]
    fn test_should_end_stream_on_danmu_stream_closed_honors_false() {
        assert!(!should_end_stream_on_danmu_stream_closed(Some(
            r#"{"end_stream_on_danmu_stream_closed":false}"#,
        )));
    }
}
