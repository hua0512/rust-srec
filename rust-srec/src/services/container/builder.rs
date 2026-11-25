use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Result;
use crate::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
use crate::credentials::CredentialProviderRegistry;
use crate::danmu::{DanmuService, events::danmu_coordination_channel};
use crate::database::repositories::{
    ConfigRepository, SqlxNotificationRepository,
    config::SqlxConfigRepository,
    dag::SqlxDagRepository,
    filter::SqlxFilterRepository,
    job::SqlxJobRepository,
    preset::{SqliteJobPresetRepository, SqlitePipelinePresetRepository},
    session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{
    DEFAULT_GATE_COOLDOWN_SECS, DownloadManager, OutputRootGate, download_coordination_channel,
};
use crate::metrics::{HealthChecker, MetricsCollector};
use crate::notification::web_push::WebPushService;
use crate::notification::{NotificationService, NotificationServiceConfig};
use crate::pipeline::PipelineManager;
use crate::services::maintenance::{MaintenanceConfig, MaintenanceScheduler};
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
            None,
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
        Self::build(pool, write_pool, config, None).await
    }

    #[cfg(test)]
    pub(super) async fn with_full_config_and_danmu_providers(
        pool: SqlitePool,
        write_pool: SqlitePool,
        config: ServiceContainerConfig,
        danmu_providers: crate::danmu::ProviderRegistry,
    ) -> Result<Self> {
        Self::build(pool, write_pool, config, Some(danmu_providers)).await
    }

    async fn build(
        pool: SqlitePool,
        write_pool: SqlitePool,
        options: ServiceContainerConfig,
        danmu_providers: Option<crate::danmu::ProviderRegistry>,
    ) -> Result<Self> {
        let ServiceContainerConfig {
            cache_ttl,
            event_capacity,
            download_config,
            pipeline_config,
        } = options;

        pipeline_config.throttle.validate()?;

        crate::i18n::init_from_env();

        let overall = Instant::now();
        let cancellation_token = CancellationToken::new();
        let task_supervisor = Arc::new(TaskSupervisor::with_cancellation(
            cancellation_token.clone(),
        ));
        let committed_write_supervisor = Arc::new(TaskSupervisor::for_committed_work());
        info!("Initializing service container");
        let committed_writer = Arc::new(crate::database::CommittedWriter::new(
            write_pool.clone(),
            committed_write_supervisor.clone(),
        )?);
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);
        let committed_streamers = Arc::new(crate::streamer::CommittedStreamerState::new(
            committed_writer.clone(),
            event_broadcaster.clone(),
        ));

        // Create repositories.
        let config_repo = Arc::new(
            SqlxConfigRepository::new(pool.clone(), write_pool.clone())
                .with_committed_writer(committed_writer.clone()),
        );
        let streamer_repo = Arc::new(
            SqlxStreamerRepository::new(pool.clone(), write_pool.clone())
                .with_commit_owner(committed_streamers.clone()),
        );

        // Load global config early for initial runtime knobs (worker pools, polling cadence, etc.).
        let global_config_start = Instant::now();
        let global_config = config_repo.get_global_config().await?;
        let global_config_ms = global_config_start.elapsed().as_millis();

        let filter_repo = Arc::new(
            SqlxFilterRepository::new(pool.clone(), write_pool.clone())
                .with_committed_writer(committed_writer.clone()),
        );
        let filter_store = Arc::new(crate::database::filter_store::FilterStore::new(
            filter_repo.clone(),
        ));
        let session_repo = Arc::new(SqlxSessionRepository::new(pool.clone(), write_pool.clone()));
        let session_event_repo: Arc<dyn crate::database::repositories::SessionEventRepository> =
            Arc::new(
                crate::database::repositories::SqlxSessionEventRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ),
            );

        // Create config service with custom cache.
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(
            ConfigService::with_cache_and_broadcaster(
                config_repo.clone(),
                streamer_repo.clone(),
                cache,
                event_broadcaster.clone(),
            )
            .with_filter_store(filter_store.clone()),
        );

        let weak_config = Arc::downgrade(&config_service);
        config_repo.bind_config_publication(Arc::new(move |write| {
            if let Some(config) = weak_config.upgrade() {
                config.publish_config_write(write);
            }
        }));
        let credential_profiles = Arc::new(
            crate::database::repositories::credential_profiles::CredentialProfileRepository::new(
                pool.clone(),
                write_pool.clone(),
            )
            .with_supervisor(committed_write_supervisor.clone()),
        );
        let weak_config = Arc::downgrade(&config_service);
        let publish_owner: Arc<dyn Fn(crate::credentials::CredentialOwner) + Send + Sync> =
            Arc::new(move |owner| {
                if let Some(config) = weak_config.upgrade() {
                    config.publish_credential_owner(owner);
                }
            });
        credential_profiles.bind_publication(publish_owner);
        let weak_config = Arc::downgrade(&config_service);
        credential_profiles.bind_material_publication(Arc::new(move |owner| {
            if let Some(config) = weak_config.upgrade() {
                config.publish_credential_material(owner);
            }
        }));
        let weak_config = Arc::downgrade(&config_service);
        committed_streamers.on_changed(Arc::new(move |id| {
            if let Some(config) = weak_config.upgrade() {
                config.invalidate_streamer(id);
            }
        }));
        let weak_config = Arc::downgrade(&config_service);
        committed_streamers.on_removed(Arc::new(move |id| {
            if let Some(config) = weak_config.upgrade() {
                config.invalidate_filter_snapshots(id);
                config.invalidate_streamer(id);
            }
        }));

        // Create streamer manager.
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo.clone(),
            event_broadcaster.clone(),
        ));

        // Shared platform extractor runner and admission budget.
        let monitor_config = crate::monitor::StreamMonitorConfig::default();
        let monitor_request_timeout = monitor_config.request_timeout;
        let extractor_runner = Arc::new(crate::recorder::PlatformExtractorRunner::new(
            monitor_config.request_timeout,
            monitor_config.max_concurrent_requests,
        ));
        let mut platform_admission =
            crate::credentials::PlatformAdmission::from_config(&monitor_config);
        for platform in config_service.list_platform_configs().await? {
            platform_admission.register_platform_name(&platform.id, &platform.platform_name);
        }
        let platform_admission = Arc::new(platform_admission);

        // Build credential refresh service (shared between recorder prober + API).
        let credential_service = CredentialProviderRegistry::new()
            .with_admission(platform_admission.clone())
            .with_platform_providers();
        let credential_service = Arc::new(credential_service);
        let credential_execution = Arc::new(crate::credentials::CredentialExecutionService::new(
            credential_profiles.clone(),
            credential_service.clone(),
        ));
        let credential_login_sessions = Arc::new(
            crate::credentials::login_sessions::CredentialLoginSessions::new(
                credential_profiles.clone(),
                pool.clone(),
                write_pool.clone(),
                platform_admission.clone(),
            )
            .with_supervisor(committed_write_supervisor.clone()),
        );
        let proxies = Arc::new(
            crate::proxies::ProxyService::new(
                pool.clone(),
                write_pool.clone(),
                config_service.clone(),
                credential_profiles.clone(),
                platform_admission.clone(),
            )
            .with_supervisor(committed_write_supervisor.clone()),
        );
        let bigo_notifications = Arc::new(crate::monitor::BigoNotifications::default());

        // Create download manager with custom config, overridden by global config for concurrency.
        let download_manager_start = Instant::now();
        let mut effective_download_config = download_config;
        effective_download_config.max_concurrent_downloads =
            (global_config.max_concurrent_downloads as i64).max(1) as usize;
        let (download_coordination_sender, download_coordination_receiver) =
            download_coordination_channel();
        let download_manager = Arc::new(
            DownloadManager::with_config(effective_download_config)
                .with_coordination_sender(download_coordination_sender)
                .with_config_repo(config_repo.clone()),
        );
        download_manager
            .set_queue_freshness_threshold_ms(global_config.queue_freshness_threshold_ms);
        let download_manager_ms = download_manager_start.elapsed().as_millis();

        // Create job and preset repositories for pipeline persistence.
        let job_repo = Arc::new(SqlxJobRepository::new(pool.clone(), write_pool.clone()));
        let preset_repo = Arc::new(SqliteJobPresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));
        let pipeline_preset_repo = Arc::new(SqlitePipelinePresetRepository::new(
            pool.clone().into(),
            write_pool.clone().into(),
        ));
        let upload_record_repo: Arc<dyn crate::database::repositories::UploadRecordRepository> =
            Arc::new(
                crate::database::repositories::SqlxUploadRecordRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ),
            );

        // Live upload-status fan-out; its wire encoder is installed by the API layer.
        let upload_status_broadcaster = crate::pipeline::UploadStatusBroadcaster::deferred();

        // Create pipeline manager with job repository for database persistence.
        let mut effective_pipeline_config = pipeline_config;
        effective_pipeline_config.cpu_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_cpu_jobs);
        effective_pipeline_config.io_pool.max_workers =
            autoscale_concurrency_limit(global_config.max_concurrent_io_jobs);
        effective_pipeline_config.max_concurrent_uploads =
            autoscale_concurrency_limit(global_config.max_concurrent_uploads);
        effective_pipeline_config.cpu_pool.job_timeout_secs =
            global_config.pipeline_cpu_job_timeout_secs.max(1) as u64;
        effective_pipeline_config.io_pool.job_timeout_secs =
            global_config.pipeline_io_job_timeout_secs.max(1) as u64;
        effective_pipeline_config.execute_timeout_secs =
            global_config.pipeline_execute_timeout_secs.max(1) as u64;
        let pipeline_config_port: Arc<dyn crate::pipeline::SessionPipelineConfigPort> = Arc::new(
            crate::services::pipeline_ports::ConfigServicePipelineConfigPort::new(
                config_service.clone(),
            ),
        );
        let pipeline_session_port: Arc<dyn crate::pipeline::PipelineSessionPort> = Arc::new(
            crate::services::pipeline_ports::DatabasePipelineSessionPort::new(session_repo.clone())
                .with_streamer_repository(streamer_repo.clone())
                .with_config_port(pipeline_config_port.clone()),
        );
        let pipeline_manager = Arc::new(PipelineManager::for_runtime(
            effective_pipeline_config,
            crate::pipeline::PipelineRuntimeDependencies {
                job_repository: job_repo,
                session_port: pipeline_session_port,
                config_port: pipeline_config_port,
                preset_repository: preset_repo.clone(),
                pipeline_preset_repository: pipeline_preset_repo.clone(),
                dag_repository: Arc::new(SqlxDagRepository::new(pool.clone(), write_pool.clone())),
                upload_record_repository: upload_record_repo.clone(),
                staged_output_repository: Arc::new(
                    crate::database::repositories::SqlxStagedOutputRepository::new(
                        pool.clone(),
                        write_pool.clone(),
                    ),
                ),
                upload_broadcaster: upload_status_broadcaster.clone(),
                download_limit_adjuster: download_manager.clone(),
                routes: Arc::new(crate::proxies::ServiceRoutes::new(pool.clone())),
            },
        ));

        // Create danmu service.
        let (danmu_coordination_sender, danmu_coordination_receiver) = danmu_coordination_channel();
        let base_danmu_service = match danmu_providers {
            Some(providers) => DanmuService::with_providers(providers),
            None => DanmuService::new(),
        };
        let danmu_service = Arc::new(
            base_danmu_service
                .with_session_repository(session_repo.clone())
                .with_coordination_sender(danmu_coordination_sender.clone()),
        );

        // Create notification service with default config.
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
        notification_service = notification_service
            .with_task_supervisor(task_supervisor.clone())
            .with_routes(Arc::new(crate::proxies::ServiceRoutes::new(pool.clone())));
        let notification_service = Arc::new(notification_service);
        notification_service.start_web_push_worker();
        credential_service.set_notification_service(Arc::clone(&notification_service));

        // Stored Baidu Netdisk login material, installed process-wide so the
        // /api/tools/baidupcs routes and BaiduPcsProcessor's automatic
        // re-login share one store (crate::baidupcs::authenticator).
        crate::baidupcs::set_authenticator(Arc::new(
            crate::services::baidupcs_auth::StoredLoginAuthenticator::new(Arc::new(
                crate::database::repositories::SqlxToolCredentialRepository::new(
                    pool.clone(),
                    write_pool.clone(),
                ),
            ))
            .with_notification_service(Arc::downgrade(&notification_service)),
        ));
        let web_push_enabled = web_push_service.is_some();

        // Build the output-root write gate AFTER both StreamerManager
        // and NotificationService are available.
        let output_root_gate = OutputRootGate::new(
            Arc::downgrade(&notification_service),
            build_output_root_gate_recovery_hook(streamer_manager.clone(), task_supervisor.clone()),
            parse_output_roots_env(),
            Duration::from_secs(DEFAULT_GATE_COOLDOWN_SECS),
        );
        download_manager.set_output_root_gate(output_root_gate.clone());

        // Create metrics collector for WebPushService delivery accounting.
        let metrics_collector = Arc::new(MetricsCollector::new());
        if let Some(web_push) = web_push_service.as_ref() {
            web_push.set_metrics_collector(metrics_collector);
        }

        // Create health checker.
        let health_checker = Arc::new(HealthChecker::new());

        // Retention values are loaded from global_config on every sweep.
        let maintenance_config = MaintenanceConfig::default();
        let maintenance_scheduler = Arc::new(
            MaintenanceScheduler::new(pool.clone(), write_pool.clone(), maintenance_config)
                .with_download_manager(Arc::downgrade(&download_manager))
                .with_output_files_gate(pipeline_manager.output_files_gate()),
        );

        // Wire the streamer-check-history pipeline (writer feeds the prober;
        // broadcaster feeds the WS route loop).
        let (check_history_writer, check_history_broadcaster, streamer_check_history_repository) =
            wire_check_history_pipeline(&pool, &write_pool, &cancellation_token, &task_supervisor);

        // Build the recorder control plane (ActorRegistry + RecorderStore +
        // RecorderProber + durable NotificationOutboxDrainer).
        let outbox_drainer = Arc::new(crate::notification::NotificationOutboxDrainer::new(
            write_pool.clone(),
            notification_service.clone(),
            cancellation_token.child_token(),
        ));
        let recorder_store = Arc::new(
            crate::recorder::RecorderStore::with_committed_state_and_supervisor(
                pool.clone(),
                write_pool.clone(),
                committed_streamers.clone(),
                committed_write_supervisor.clone(),
            )?
            .with_outbox_notify(outbox_drainer.notifier()),
        );
        let recorder_prober = Arc::new(crate::recorder::RecorderProber::new(
            crate::recorder::RecorderProberDependencies {
                extractor_runner: extractor_runner.clone(),
                streamer_manager: streamer_manager.clone(),
                config_service: config_service.clone(),
                filter_store: filter_store.clone(),
                store: recorder_store.clone(),
                admission: platform_admission.clone(),
                execution_service: credential_execution.clone(),
                bigo_notifications: bigo_notifications.clone(),
                check_history_writer: Some(check_history_writer),
                request_timeout: monitor_request_timeout,
            },
        ));
        let weak_prober = Arc::downgrade(&recorder_prober);
        download_manager.set_credential_start_validator(Arc::new(
            move |session_id, streamer_id, binding| {
                let prober = weak_prober.upgrade();
                Box::pin(async move {
                    let prober = prober.ok_or(crate::credentials::ProfileError::SourceChanged)?;
                    prober
                        .validate_download_binding(&session_id, &streamer_id, &binding)
                        .await
                })
            },
        ));
        let actor_registry =
            crate::recorder::ActorRegistry::new(crate::recorder::ActorRegistryDependencies {
                store: recorder_store.clone(),
                prober: recorder_prober,
                download_manager: download_manager.clone(),
                danmu_service: danmu_service.clone(),
                pipeline_manager: pipeline_manager.clone(),
                streamer_manager: streamer_manager.clone(),
                config_service: config_service.clone(),
                check_interval_ms: global_config.streamer_check_delay_ms as u64,
                root_cancel: cancellation_token.child_token(),
            });
        download_manager.set_recorder_feedback(actor_registry.download_feedback())?;
        recorder_store.set_post_publication_listener(actor_registry.clone());
        committed_streamers.add_post_publication_listener(actor_registry.clone());

        let total_ms = overall.elapsed().as_millis();
        info!(
            startup_container_global_config_ms = global_config_ms,
            startup_container_download_manager_ms = download_manager_ms,
            startup_container_total_ms = total_ms,
            web_push_enabled,
            "Startup: service container build summary"
        );

        info!("Service container initialized with recorder actor control plane");

        Ok(Self {
            pool,
            write_pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            session_repository: session_repo,
            session_event_repository: session_event_repo,
            streamer_check_history_repository,
            upload_record_repository: upload_record_repo,
            filter_repository: filter_repo,
            streamer_repository: streamer_repo,
            job_preset_repository: preset_repo,
            pipeline_preset_repository: pipeline_preset_repo,
            output_root_gate,
            gpu_health_monitor: std::sync::OnceLock::new(),
            pipeline_manager,
            download_coordination_receiver: parking_lot::Mutex::new(Some(
                download_coordination_receiver,
            )),
            danmu_service,
            danmu_coordination_sender,
            danmu_coordination_receiver: parking_lot::Mutex::new(Some(danmu_coordination_receiver)),
            notification_service,
            notification_repository,
            web_push_service,
            health_checker,
            maintenance_scheduler,
            bigo_notifications,
            platform_admission,
            credential_profiles,
            credential_execution,
            credential_login_sessions,
            proxies,
            credential_service,
            check_history_broadcaster,
            upload_status_broadcaster,
            cancellation_token,
            task_supervisor,
            committed_write_supervisor,
            logging_config: Arc::new(std::sync::OnceLock::new()),
            startup_recovery_complete: std::sync::atomic::AtomicBool::new(false),
            extractor_runner,
            actor_registry,
            outbox_drainer: parking_lot::Mutex::new(Some(outbox_drainer)),
        })
    }
}
