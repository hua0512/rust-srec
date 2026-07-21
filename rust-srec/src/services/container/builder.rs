use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Result;
use crate::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
use crate::credentials::{
    CredentialRefreshService, CredentialResolver,
    platforms::{BilibiliCredentialManager, SoopCredentialManager},
};
use crate::danmu::DanmuService;
use crate::database::maintenance::{MaintenanceConfig, MaintenanceScheduler};
use crate::database::repositories::{
    ConfigRepository, SqlxCredentialStore, SqlxNotificationRepository,
    config::SqlxConfigRepository,
    dag::SqlxDagRepository,
    filter::SqlxFilterRepository,
    job::SqlxJobRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{DEFAULT_GATE_COOLDOWN_SECS, DownloadManager, OutputRootGate};
use crate::metrics::{HealthChecker, MetricsCollector};
use crate::monitor::StreamMonitor;
use crate::notification::web_push::WebPushService;
use crate::notification::{NotificationService, NotificationServiceConfig};
use crate::pipeline::PipelineManager;
use crate::scheduler::Scheduler;
use crate::services::session_cancels::SessionCancelTokens;
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

use super::{
    DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY, ServiceContainer, ServiceContainerConfig,
    autoscale_concurrency_limit, build_output_root_gate_recovery_hook, parse_output_roots_env,
    wire_check_history_pipeline,
};

impl ServiceContainer {
    /// Create a new service container with the given database pool.
    ///
    /// # Errors
    ///
    /// Returns an error if the persisted global configuration cannot be loaded or initialized.
    pub async fn new(pool: SqlitePool, write_pool: SqlitePool) -> Result<Self> {
        Self::with_config(pool, write_pool, DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY).await
    }

    /// Create a new service container with custom cache and event capacity.
    ///
    /// # Errors
    ///
    /// Returns an error if the persisted global configuration cannot be loaded or initialized.
    pub(crate) async fn with_config(
        pool: SqlitePool,
        write_pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
    ) -> Result<Self> {
        Self::build(
            pool,
            write_pool,
            ServiceContainerConfig::standard(cache_ttl, event_capacity),
        )
        .await
    }

    /// Create a service container with custom subsystem configs.
    ///
    /// Database-backed global settings remain authoritative for download concurrency,
    /// pipeline worker counts, pipeline timeouts, and queue freshness.
    ///
    /// # Errors
    ///
    /// Returns an error if the persisted global configuration cannot be loaded or initialized.
    pub async fn with_full_config(
        pool: SqlitePool,
        write_pool: SqlitePool,
        config: ServiceContainerConfig,
    ) -> Result<Self> {
        Self::build(pool, write_pool, config).await
    }

    async fn build(
        pool: SqlitePool,
        write_pool: SqlitePool,
        options: ServiceContainerConfig,
    ) -> Result<Self> {
        let ServiceContainerConfig {
            cache_ttl,
            event_capacity,
            download_config,
            pipeline_config,
            danmu_config,
            api_config,
        } = options;

        crate::i18n::init_from_env();

        let overall = Instant::now();
        let cancellation_token_start = Instant::now();
        let cancellation_token = CancellationToken::new();
        let cancellation_token_ms = cancellation_token_start.elapsed().as_millis();
        let task_supervisor = Arc::new(TaskSupervisor::with_cancellation(
            cancellation_token.clone(),
        ));
        info!("Initializing service container");

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

        // Construct the single-owner session lifecycle service up-front so
        // the stream monitor can delegate its atomic session+streamer+outbox
        // writes to it.
        //
        // The hysteresis backstop window is derived from the same scheduler
        // tunables the actor uses for offline confirmation — single source
        // of truth, no parallel tunable.
        let hysteresis_config = crate::session::HysteresisConfig::from_scheduler(
            global_config.offline_check_count as u32,
            global_config.offline_check_delay_ms as u64,
        );
        let sm_for_resolver = streamer_manager.clone();
        let hysteresis_resolver: crate::session::HysteresisWindowFn =
            std::sync::Arc::new(move |streamer_id: &str| {
                sm_for_resolver.get_streamer(streamer_id).map(|m| {
                    crate::session::HysteresisConfig::from_scheduler(
                        m.effective_offline_check_count,
                        m.effective_offline_check_delay_ms,
                    )
                })
            });
        let session_event_repo: Arc<dyn crate::database::repositories::SessionEventRepository> =
            Arc::new(
                crate::database::repositories::SqlxSessionEventRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ),
            );
        // Classifier window/threshold derived from the same scheduler
        // tunables — see the primary container site for rationale.
        let offline_classifier = Arc::new(crate::session::OfflineClassifier::from_scheduler(
            global_config.offline_check_count as u32,
            global_config.offline_check_delay_ms as u64,
        ));
        let (required_transition_sender, required_transition_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let session_lifecycle = Arc::new(
            crate::session::SessionLifecycle::with_config(
                Arc::new(
                    crate::database::repositories::SessionLifecycleRepository::new(
                        write_pool.clone(),
                    ),
                ),
                offline_classifier,
                crate::session::DEFAULT_TRANSITION_CHANNEL_CAPACITY,
                hysteresis_config,
            )
            .with_required_transition_sender(required_transition_sender)
            .with_hysteresis_resolver(hysteresis_resolver)
            .with_event_repo(session_event_repo.clone()),
        );

        // Create stream monitor for real status detection
        let stream_monitor_start = Instant::now();
        let (required_monitor_event_sender, required_monitor_event_receiver) =
            tokio::sync::mpsc::channel(256);
        let mut stream_monitor = StreamMonitor::with_runtime(
            streamer_manager.clone(),
            filter_repo,
            session_repo.clone(),
            config_service.clone(),
            write_pool.clone(),
            session_lifecycle.clone(),
            crate::monitor::StreamMonitorRuntimeConfig {
                monitor: crate::monitor::StreamMonitorConfig::default(),
                required_event_sender: Some(required_monitor_event_sender),
                task_supervisor: task_supervisor.clone(),
            },
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
        match SoopCredentialManager::new_lazy() {
            Ok(manager) => credential_service.register_manager(Arc::new(manager)),
            Err(e) => warn!(error = %e, "Failed to init SOOP credential manager; skipping"),
        }
        let credential_service = Arc::new(credential_service);
        stream_monitor.set_credential_service(Arc::clone(&credential_service));
        let stream_monitor = Arc::new(stream_monitor);
        let credential_service_ms = credential_service_start.elapsed().as_millis();

        // Create download manager with custom config, overridden by global config for concurrency.
        let download_manager_start = Instant::now();
        let mut effective_download_config = download_config;
        effective_download_config.max_concurrent_downloads =
            (global_config.max_concurrent_downloads as i64).max(1) as usize;
        let (required_terminal_sender, required_terminal_receiver) =
            tokio::sync::mpsc::unbounded_channel();
        let download_manager = Arc::new(
            DownloadManager::with_config(effective_download_config)
                .with_required_terminal_sender(required_terminal_sender)
                .with_config_repo(config_repo.clone()),
        );
        download_manager
            .set_queue_freshness_threshold_ms(global_config.queue_freshness_threshold_ms);
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
        let pipeline_manager = Arc::new(PipelineManager::for_runtime(
            effective_pipeline_config,
            crate::pipeline::PipelineRuntimeDependencies {
                job_repository: job_repo,
                session_repository: session_repo.clone(),
                streamer_repository: streamer_repo.clone(),
                preset_repository: preset_repo,
                pipeline_preset_repository: pipeline_preset_repo,
                config_service: config_service.clone(),
                dag_repository: Arc::new(SqlxDagRepository::new(pool.clone(), write_pool.clone())),
            },
        ));
        let pipeline_manager_ms = pipeline_manager_start.elapsed().as_millis();

        // Get monitor event broadcaster
        let monitor_event_broadcaster_start = Instant::now();
        let monitor_event_broadcaster = stream_monitor.event_broadcaster().clone();
        let monitor_event_broadcaster_ms = monitor_event_broadcaster_start.elapsed().as_millis();

        // Create danmu service with custom config
        let danmu_service_start = Instant::now();
        let danmu_service =
            Arc::new(DanmuService::new(danmu_config).with_session_repository(session_repo.clone()));
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
        notification_service = notification_service.with_task_supervisor(task_supervisor.clone());
        let notification_service = Arc::new(notification_service);
        notification_service.start_web_push_worker();
        credential_service.set_notification_service(Arc::clone(&notification_service));
        let notification_service_ms = notification_service_start.elapsed().as_millis();
        let web_push_enabled = web_push_service.is_some();

        // Build the output-root write gate AFTER both StreamerManager
        // and NotificationService are available. Late-bind into the already
        // Arc-wrapped download manager via `set_output_root_gate`, which
        // uses a OnceLock internally so subsequent reads on the hot path
        // stay lock-free.
        let output_root_gate = OutputRootGate::new(
            Arc::downgrade(&notification_service),
            build_output_root_gate_recovery_hook(streamer_manager.clone(), task_supervisor.clone()),
            parse_output_roots_env(),
            Duration::from_secs(DEFAULT_GATE_COOLDOWN_SECS),
        );
        download_manager.set_output_root_gate(output_root_gate.clone());

        // Create metrics collector. Its only consumer is
        // `WebPushService::set_metrics_collector` delivery accounting.
        let metrics_collector_start = Instant::now();
        let metrics_collector = Arc::new(MetricsCollector::new());
        if let Some(web_push) = web_push_service.as_ref() {
            web_push.set_metrics_collector(metrics_collector);
        }
        let metrics_collector_ms = metrics_collector_start.elapsed().as_millis();

        // Create health checker
        let health_checker_start = Instant::now();
        let health_checker = Arc::new(HealthChecker::new());
        let health_checker_ms = health_checker_start.elapsed().as_millis();

        // Retention values are loaded from global_config on every sweep.
        let maintenance_scheduler_start = Instant::now();
        let maintenance_config = MaintenanceConfig::default();
        let maintenance_scheduler = Arc::new(MaintenanceScheduler::new(
            pool.clone(),
            write_pool.clone(),
            maintenance_config,
        ));
        let maintenance_scheduler_ms = maintenance_scheduler_start.elapsed().as_millis();

        let scheduler_config = crate::scheduler::SchedulerConfig {
            check_interval_ms: global_config.streamer_check_delay_ms as u64,
            offline_check_interval_ms: global_config.offline_check_delay_ms as u64,
            offline_check_count: global_config.offline_check_count as u32,
            supervisor_config: crate::scheduler::actor::SupervisorConfig::default(),
        };

        // Wire the streamer-check-history pipeline (writer feeds the
        // monitor-side polling path; broadcaster feeds the WS route loop).
        let (check_history_writer, check_history_broadcaster) =
            wire_check_history_pipeline(&pool, &write_pool, &cancellation_token, &task_supervisor);

        // Create scheduler with StreamMonitor for real status checking
        let scheduler_start = Instant::now();
        let scheduler = Scheduler::with_monitor_history_and_config(
            streamer_manager.clone(),
            event_broadcaster.clone(),
            stream_monitor.clone(),
            Some(check_history_writer),
            scheduler_config,
            cancellation_token.child_token(),
        )
        .with_config_repo(config_repo.clone());
        let scheduler_handle = scheduler.handle();
        let scheduler = parking_lot::Mutex::new(Some(scheduler));
        let scheduler_ms = scheduler_start.elapsed().as_millis();

        let session_cancels = Arc::new(SessionCancelTokens::new());
        let pending_pipelines = Arc::new(DashMap::new());
        let runtime_coordinator = Arc::new(
            crate::services::runtime_coordinator::RuntimeCoordinator::new(
                crate::services::runtime_coordinator::RuntimeCoordinatorDependencies {
                    download_manager: download_manager.clone(),
                    streamer_manager: streamer_manager.clone(),
                    config_service: config_service.clone(),
                    danmu_service: danmu_service.clone(),
                    stream_monitor: stream_monitor.clone(),
                    session_repository: session_repo.clone(),
                    session_cancels,
                    pending_pipelines,
                    pipeline_manager: pipeline_manager.clone(),
                    session_lifecycle: session_lifecycle.clone(),
                    task_supervisor: task_supervisor.clone(),
                },
            ),
        );

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

        info!("Service container initialized with real status checking");

        Ok(Self {
            pool,
            write_pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            session_repository: session_repo,
            output_root_gate,
            gpu_health_monitor: std::sync::OnceLock::new(),
            pipeline_manager,
            monitor_event_broadcaster,
            monitor_event_receiver: parking_lot::Mutex::new(Some(required_monitor_event_receiver)),
            download_terminal_receiver: parking_lot::Mutex::new(Some(required_terminal_receiver)),
            session_lifecycle,
            session_transition_receiver: parking_lot::Mutex::new(Some(
                required_transition_receiver,
            )),
            runtime_coordinator,
            danmu_service,
            notification_service,
            notification_repository,
            web_push_service,
            health_checker,
            maintenance_scheduler,
            scheduler,
            scheduler_handle,
            stream_monitor,
            credential_service,
            check_history_broadcaster,
            api_server_config: api_config,
            cancellation_token,
            task_supervisor,
            logging_config: std::sync::OnceLock::new(),
            discarded_segment_keys: Arc::new(DashMap::new()),
        })
    }
}
