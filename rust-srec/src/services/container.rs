//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Result;
use crate::config::{ConfigEventBroadcaster, ConfigService};
use crate::danmu::DanmuService;
use crate::database::repositories::NotificationRepository;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{DownloadManager, DownloadManagerConfig, OutputRootGate};
use crate::logging::LoggingConfig;
use crate::metrics::HealthChecker;
use crate::notification::NotificationService;
use crate::notification::web_push::WebPushService;
use crate::pipeline::{PipelineManager, PipelineManagerConfig};
use crate::services::maintenance::MaintenanceScheduler;
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

use output_roots::{
    build_output_root_gate_recovery_hook, parse_output_roots_env, sqlite_file_path_from_url,
    static_root_prefix,
};
pub(crate) use shutdown::ServiceShutdownSchedule;

mod builder;
#[cfg(test)]
mod config_refresh_tests;
#[cfg(test)]
mod credential_selection_tests;
mod events;
mod health;
mod output_roots;
mod shutdown;
#[cfg(test)]
mod shutdown_tests;
#[cfg(test)]
mod streamer_retirement_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod throttle_tests;

/// Default cache TTL (1 hour).
pub(crate) const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
pub(crate) const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Gap between two passes of `ServiceContainer::spawn_streamer_reaper`.
///
/// A pass costs one `PipelineManager::outstanding_for_session` observation per
/// marked row and nothing at all when none is marked, so the cadence is set by
/// how long a finished deletion may keep its row rather than by cost.
const STREAMER_REAP_INTERVAL: Duration = Duration::from_secs(30);

fn autoscale_concurrency_limit(raw: i32) -> usize {
    if raw > 0 {
        return raw as usize;
    }

    let cores = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);

    (cores / 2).max(1)
}

/// Configuration used to assemble a [`ServiceContainer`].
pub struct ServiceContainerConfig {
    pub cache_ttl: Duration,
    pub event_capacity: usize,
    pub download_config: DownloadManagerConfig,
    pub pipeline_config: PipelineManagerConfig,
}

/// Snapshot of currently active recordings for desktop status and exit guards.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActiveRecordingSummary {
    pub active_count: usize,
    pub total_speed_bytes_per_sec: u64,
}

impl ServiceContainerConfig {
    pub(crate) fn standard(cache_ttl: Duration, event_capacity: usize) -> Self {
        Self {
            cache_ttl,
            event_capacity,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: PipelineManagerConfig::default(),
        }
    }
}

/// Service container holding all core application services (independent of the
/// HTTP/WebSocket API presentation layer).
pub struct ServiceContainer {
    /// Database connection pool (read-heavy).
    pub(crate) pool: SqlitePool,
    /// Serialized write pool (max_connections=1) for contention-free writes.
    pub(crate) write_pool: SqlitePool,
    /// Configuration service.
    pub(crate) config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    /// Streamer manager.
    pub(crate) streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    /// Event broadcaster (shared between services).
    pub(crate) event_broadcaster: ConfigEventBroadcaster,
    /// Download manager.
    pub(crate) download_manager: Arc<DownloadManager>,
    /// Session repository shared by monitor, pipeline, danmu, and download startup.
    pub(crate) session_repository: Arc<SqlxSessionRepository>,
    pub(crate) session_event_repository:
        Arc<dyn crate::database::repositories::SessionEventRepository>,
    pub(crate) streamer_check_history_repository:
        Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository>,
    pub(crate) upload_record_repository:
        Arc<dyn crate::database::repositories::UploadRecordRepository>,
    pub(crate) filter_repository: Arc<SqlxFilterRepository>,
    pub(crate) streamer_repository: Arc<SqlxStreamerRepository>,
    pub(crate) job_preset_repository:
        Arc<crate::database::repositories::preset::SqliteJobPresetRepository>,
    pub(crate) pipeline_preset_repository:
        Arc<crate::database::repositories::preset::SqlitePipelinePresetRepository>,
    /// Output-root write gate. Shared by the download manager for
    /// pre-start checks + runtime ENOSPC routing and by the health checker
    /// for aggregated `/health` reporting.
    pub(crate) output_root_gate: Arc<OutputRootGate>,
    /// GPU health monitor. Empty when `nvidia-smi` is not available
    /// at startup; otherwise the background probe loop is owned by the
    /// container's cancellation token. Use [`std::sync::OnceLock::get`]
    /// to read; installation is owned by the private runtime initializer.
    pub(crate) gpu_health_monitor: std::sync::OnceLock<Arc<crate::metrics::GpuHealthMonitor>>,
    /// Pipeline manager.
    pub(crate) pipeline_manager: Arc<PipelineManager>,
    /// Single required download lifecycle consumer, moved into its supervised task.
    download_coordination_receiver:
        parking_lot::Mutex<Option<crate::downloader::DownloadCoordinationReceiver>>,
    /// Danmu service.
    pub(crate) danmu_service: Arc<DanmuService>,
    /// Required, lossless danmu-event path used for runtime coordination.
    danmu_coordination_sender: crate::danmu::events::DanmuCoordinationSender,
    /// Single required danmu-event consumer, moved into its supervised task.
    danmu_coordination_receiver:
        parking_lot::Mutex<Option<crate::danmu::events::DanmuCoordinationReceiver>>,
    /// Notification service.
    pub(crate) notification_service: Arc<NotificationService>,
    /// Notification repository.
    pub(crate) notification_repository: Arc<dyn NotificationRepository>,
    /// Web push service for browser notifications (VAPID), if configured.
    pub(crate) web_push_service: Option<Arc<WebPushService>>,
    /// Health checker.
    pub(crate) health_checker: Arc<HealthChecker>,
    /// Database maintenance scheduler.
    pub(crate) maintenance_scheduler: Arc<MaintenanceScheduler>,
    /// Bigo's live notifications, read by the recorder prober and the API.
    pub(crate) bigo_notifications: Arc<crate::monitor::BigoNotifications>,
    /// Credential refresh service (shared between recorder prober + API).
    pub(crate) credential_service: Arc<crate::credentials::CredentialProviderRegistry>,
    /// One platform rate budget shared by prober, parse, and credential providers.
    pub(crate) platform_admission: Arc<crate::credentials::PlatformAdmission>,
    pub(crate) credential_profiles:
        Arc<crate::database::repositories::credential_profiles::CredentialProfileRepository>,
    pub(crate) credential_login_sessions:
        Arc<crate::credentials::login_sessions::CredentialLoginSessions>,
    pub(crate) credential_execution: Arc<crate::credentials::CredentialExecutionService>,
    pub(crate) proxies: Arc<crate::proxies::ProxyService>,
    /// Live broadcaster for committed check-history rows. Cloned into the
    /// downloads WS route so per-streamer subscribers see new bars appear
    /// without polling. Its wire encoder is injected by the API layer.
    pub(crate) check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    /// Live broadcaster for upload status events (started/progress/terminal).
    /// Published by `JobQueue`, consumed by the downloads WS route. Its wire
    /// encoder is injected by the API layer.
    pub(crate) upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster,
    /// Cancellation token for graceful shutdown.
    pub(crate) cancellation_token: CancellationToken,
    /// Owner for background tasks started by the application runtime.
    pub(crate) task_supervisor: Arc<TaskSupervisor>,
    /// Database commits and their publication/completion survive forced task aborts.
    committed_write_supervisor: Arc<TaskSupervisor>,
    /// Logging configuration
    pub(crate) logging_config: Arc<std::sync::OnceLock<Arc<LoggingConfig>>>,
    startup_recovery_complete: std::sync::atomic::AtomicBool,
    /// Shared stateless extractor runner used by `/api/parse` and the actor prober.
    pub(crate) extractor_runner: Arc<crate::recorder::PlatformExtractorRunner>,
    /// Per-streamer recording actor registry and crash supervisor.
    pub(crate) actor_registry: Arc<crate::recorder::ActorRegistry>,
    /// Durable notification outbox drainer; moved into its supervised task
    /// during [`Self::initialize`].
    outbox_drainer: parking_lot::Mutex<Option<Arc<crate::notification::NotificationOutboxDrainer>>>,
}

/// Wire the streamer-check-history pipeline:
/// - One repository on top of the shared SQLite pools.
/// - One bounded MPSC; senders are cloned into every monitor poll.
/// - One broadcaster whose wire encoder is installed by the API layer.
/// - One drain task that survives until shutdown cancels it.
///
/// The polling hot path uses `try_send` so DB latency never blocks the
/// lifecycle FSM; the drain task absorbs bursts and fans out committed
/// rows after they've durably landed in SQLite.
fn wire_check_history_pipeline(
    pool: &SqlitePool,
    write_pool: &SqlitePool,
    cancellation_token: &CancellationToken,
    task_supervisor: &TaskSupervisor,
) -> (
    crate::monitor::CheckHistoryWriter,
    crate::monitor::CheckHistoryBroadcaster,
    Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository>,
) {
    let repo: Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository> = Arc::new(
        crate::database::repositories::SqlxStreamerCheckHistoryRepository::new(
            pool.clone(),
            write_pool.clone(),
        ),
    );
    let (writer, rx) = crate::monitor::CheckHistoryWriter::new();
    let broadcaster = crate::monitor::CheckHistoryBroadcaster::deferred();

    task_supervisor.spawn(
        "check-history writer",
        crate::monitor::check_history_writer::run(
            repo.clone(),
            rx,
            Some(broadcaster.clone()),
            cancellation_token.child_token(),
        ),
    );
    (writer, broadcaster, repo)
}

impl ServiceContainer {
    /// Shared credential blocks store populated by the recorder prober.
    pub(crate) fn credential_blocks(&self) -> &Arc<crate::credentials::CredentialBlocks> {
        self.actor_registry.prober().credential_blocks()
    }

    /// True only after initialization confirmed every persistent recovery phase.
    /// Resumed jobs need not finish: their durable ownership has been restored.
    pub(crate) fn startup_recovery_complete(&self) -> bool {
        self.startup_recovery_complete
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Initialize all services (hydrate data, start background tasks, etc.).
    pub async fn initialize(&self) -> Result<()> {
        self.startup_recovery_complete
            .store(false, std::sync::atomic::Ordering::Release);
        let overall = Instant::now();
        info!("Initializing services");

        let hydrate_start = Instant::now();
        let ((streamer_count, hydration_complete), recovery) = tokio::try_join!(
            self.streamer_manager.hydrate_with_recovery_status(),
            self.pipeline_manager.recover_jobs_with_status(),
        )?;
        let recovered_jobs = recovery.recovered_jobs;
        let mut recovery_complete = hydration_complete && recovery.complete;

        let hydrate_recover_ms = hydrate_start.elapsed().as_millis();

        info!(
            elapsed_ms = hydrate_recover_ms,
            "Startup: hydrate streamers + recover jobs"
        );

        info!("Hydrated {} streamers", streamer_count);

        // Populate resolved offline_check_* on the in-memory metadata cache
        // for every hydrated streamer.
        self.actor_registry
            .refresh_metadata_offline_checks(
                self.streamer_manager
                    .get_all()
                    .into_iter()
                    .map(|metadata| metadata.id),
            )
            .await;

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

        // Detect and install the GPU health monitor BEFORE wiring
        // the config-event subscription, so the latter can capture a
        // plain `Option<Arc<GpuHealthMonitor>>` clone for hot-reload.
        self.init_gpu_health_monitor().await;

        // Subscribe streamer manager and actor registry to config events.
        self.setup_config_event_subscriptions();

        if let Some(coordination_rx) = self.download_coordination_receiver.lock().take() {
            self.actor_registry
                .spawn_download_bridges(coordination_rx, &self.task_supervisor);
        }
        if let Some(danmu_rx) = self.danmu_coordination_receiver.lock().take() {
            self.actor_registry
                .spawn_danmu_bridge(danmu_rx, &self.task_supervisor);
        }
        crate::services::notification_bridge::start_runtime_notification_listeners(
            &self.notification_service,
            self.download_manager.subscribe(),
            self.pipeline_manager.subscribe(),
        );

        // Load notifications and discover output paths concurrently, both best-effort.
        // Health registration and the startup write probe share the same root snapshot.
        let health_checks_start = Instant::now();
        let (reload_result, output_roots) = tokio::join!(
            self.notification_service.reload_from_db(),
            self.collect_output_roots(),
        );
        self.register_health_checks(&output_roots).await;
        let notifications_health_checks_ms = health_checks_start.elapsed().as_millis();
        if let Err(e) = reload_result {
            recovery_complete = false;
            warn!("Failed to load notification configuration from DB: {}", e);
        }
        info!(
            elapsed_ms = notifications_health_checks_ms,
            "Startup: notifications + health checks"
        );

        // One-shot output-root write gate startup probe. Discovers
        // broken mounts (e.g., stale Docker bind mounts from host-side
        // cleanup) on container boot rather than waiting for the first
        // monitor tick to try starting a download. Per-root probes run in
        // parallel with a bounded per-root timeout so a hung mount can't
        // wedge startup.
        self.run_output_root_startup_probe(&output_roots).await;

        // Start the single database maintenance task. It performs an immediate
        // retention sweep before waiting for its periodic cadence.
        let maintenance_handle = self
            .maintenance_scheduler
            .clone()
            .start(self.cancellation_token.child_token());
        self.task_supervisor
            .spawn("database maintenance", async move {
                if let Err(error) = maintenance_handle.await {
                    warn!(error = %error, "Database maintenance task failed");
                }
            });
        info!("Database maintenance scheduler started");

        // Start durable notification outbox drainer and recorder actor registry.
        if let Some(drainer) = self.outbox_drainer.lock().take() {
            self.task_supervisor
                .spawn("notification outbox drainer", async move {
                    if let Err(error) = drainer.drain_until_wait().await {
                        warn!(%error, "Initial notification outbox drain failed");
                    }
                    drainer.run().await;
                });
        }
        self.actor_registry.start();
        info!("Recorder actor registry started");

        // After the control plane starts, so `ActorRegistry::retire_streamer` can
        // observe active actors on its first pass.
        self.spawn_streamer_reaper();
        self.spawn_login_session_pruning();
        // After the control plane starts, which owns the actors its pushes ask to check.
        self.bigo_notifications.spawn(
            crate::monitor::BigoNotificationDeps {
                config_service: self.config_service.clone(),
                streamer_manager: self.streamer_manager.clone(),
                credentials: self.credential_execution.clone(),
                feedback: self.actor_registry.clone(),
                notifications: self.notification_service.clone(),
            },
            &self.task_supervisor,
            self.cancellation_token.child_token(),
        );

        info!("Startup: recorder control plane started");

        let total_ms = overall.elapsed().as_millis();
        self.startup_recovery_complete
            .store(recovery_complete, std::sync::atomic::Ordering::Release);
        info!(
            elapsed_ms = total_ms,
            recovery_complete, "Services initialized"
        );

        info!(
            startup_hydrate_recover_ms = hydrate_recover_ms,
            startup_pipeline_start_ms = pipeline_start_ms,
            startup_notifications_health_checks_ms = notifications_health_checks_ms,
            startup_total_ms = total_ms,
            streamer_count,
            recovered_jobs,
            "Startup: initialize summary"
        );
        Ok(())
    }

    /// Start the task that finishes deletions the runtime has not let go of yet.
    ///
    /// Every row with `streamers.deleted_at` set is a deletion whose physical
    /// `DELETE` is still owed: either an interactive delete whose
    /// `ActorRegistry::retire_streamer` bound expired while the session's
    /// post-processing was running, or a row a crash left between the mark and
    /// the reap. The first pass runs immediately, which is the startup recovery;
    /// after that one pass per `STREAMER_REAP_INTERVAL` observes each owner once
    /// and reaps whatever has gone quiet.
    fn spawn_streamer_reaper(&self) {
        let actor_registry = self.actor_registry.clone();
        let write_pool = self.write_pool.clone();
        let cancellation_token = self.cancellation_token.child_token();

        self.task_supervisor.spawn("streamer reaper", async move {
            loop {
                let reaped = actor_registry
                    .reap_marked_streamers(crate::recorder::OBSERVE_RETIREMENT)
                    .await;
                if reaped > 0 {
                    info!(reaped, "Removed streamers whose retirement completed");
                }
                // Templates can also be deferred by a recording bound to one of their
                // profiles. That session ends without deleting a streamer, so reap
                // retired definitions and profiles here as well.
                let definition_reap: crate::Result<()> = async {
                    let mut transaction = crate::database::begin_immediate(&write_pool).await?;
                    crate::database::repositories::config_retirement::reap(&mut transaction)
                        .await?;
                    transaction.commit().await?;
                    Ok(())
                }
                .await;
                if let Err(error) = definition_reap {
                    tracing::warn!(%error, "Configuration retirement will be retried");
                }

                tokio::select! {
                    () = cancellation_token.cancelled() => break,
                    () = tokio::time::sleep(STREAMER_REAP_INTERVAL) => {}
                }
            }
        });
    }

    fn spawn_login_session_pruning(&self) {
        let sessions = self.credential_login_sessions.clone();
        let cancellation_token = self.cancellation_token.child_token();
        self.task_supervisor
            .spawn("credential login pruning", async move {
                loop {
                    if let Err(error) = sessions.prune().await {
                        tracing::warn!(%error, "Expired credential logins will be pruned on the next pass");
                    }
                    tokio::select! {
                        () = cancellation_token.cancelled() => break,
                        () = tokio::time::sleep(crate::credentials::login_sessions::PRUNE_INTERVAL) => {}
                    }
                }
            });
    }

    /// Get the cancellation token for external use.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Wait until a critical runtime task fails.
    pub async fn wait_for_runtime_failure(&self) -> crate::Error {
        crate::Error::Other(self.task_supervisor.wait_for_failure().await.to_string())
    }

    /// Get the notification service.
    pub fn notification_service(&self) -> &Arc<NotificationService> {
        &self.notification_service
    }

    /// Return the configuration service used by the runtime.
    pub fn config_service(
        &self,
    ) -> &Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>> {
        &self.config_service
    }

    /// Resolve a recorded media output ID to its `(file_type, normalized_path)`.
    pub async fn media_output_location(&self, id: &str) -> Result<(String, std::path::PathBuf)> {
        use crate::database::repositories::SessionRepository;

        let media = self.session_repository.get_media_output(id).await?;
        let path = crate::utils::fs::normalize_media_path(&media.file_path);
        Ok((media.file_type, path))
    }

    /// Snapshot the number of active recordings and their aggregate throughput.
    pub fn active_recording_summary(&self) -> ActiveRecordingSummary {
        let downloads = self.download_manager.get_active_downloads();
        let total_speed_bytes_per_sec = downloads
            .iter()
            .map(|download| download.progress.speed_bytes_per_sec)
            .fold(0_u64, u64::saturating_add);
        ActiveRecordingSummary {
            active_count: downloads.len(),
            total_speed_bytes_per_sec,
        }
    }

    /// Subscribe to live download manager lifecycle and progress events.
    pub fn subscribe_download_events(
        &self,
    ) -> tokio::sync::broadcast::Receiver<crate::downloader::DownloadManagerEvent> {
        self.download_manager.subscribe()
    }

    /// Set the logging configuration
    pub fn set_logging_config(&self, config: Arc<LoggingConfig>) {
        if self.logging_config.set(config.clone()).is_err() {
            warn!("Logging configuration was already installed");
            return;
        }

        let cancellation = self.cancellation_token.child_token();
        self.task_supervisor.spawn("log retention", async move {
            config.run_retention_cleanup(cancellation).await;
        });
    }
}
