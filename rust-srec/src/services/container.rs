//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::Result;
use crate::api::server::ApiServerConfig;
use crate::config::{ConfigEventBroadcaster, ConfigService};
use crate::danmu::DanmuService;
use crate::database::maintenance::MaintenanceScheduler;
use crate::database::repositories::NotificationRepository;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{DownloadManager, DownloadManagerConfig, OutputRootGate};
use crate::logging::LoggingConfig;
use crate::metrics::HealthChecker;
use crate::monitor::{MonitorEventBroadcaster, StreamMonitor};
use crate::notification::NotificationService;
use crate::notification::web_push::WebPushService;
use crate::pipeline::{PipelineManager, PipelineManagerConfig};
use crate::scheduler::{Scheduler, SchedulerHandle};
use crate::services::runtime_coordinator::RuntimeCoordinator;
use crate::streamer::StreamerManager;
use crate::utils::task_supervisor::TaskSupervisor;

#[cfg(test)]
use event_policy::RECOVERY_PROGRESS_MIN_BYTES;
use event_policy::{
    broadcast_error_is_recoverable, has_transient_error_state,
    should_end_stream_on_danmu_stream_closed, should_record_recovery_from_progress,
};
use output_roots::{
    build_output_root_gate_recovery_hook, parse_output_roots_env, sqlite_file_path_from_url,
    static_root_prefix,
};
pub(crate) use shutdown::ServiceShutdownSchedule;

mod api;
mod builder;
#[cfg(test)]
mod config_refresh_tests;
#[cfg(test)]
mod credential_selection_tests;
mod event_policy;
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
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

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
    pub api_config: ApiServerConfig,
    /// Serve the API as the desktop app's built-in backend. The shell signs
    /// its user in without a password, so nobody is ever made to change the
    /// seeded default password, and `must_change_password` then means "no
    /// password has been set". With this set, an account with that flag is
    /// reachable only through sessions issued by
    /// [`ServiceContainer::issue_local_desktop_session`]: a password login for
    /// it never succeeds (the seeded password never signs in), its other
    /// sessions are refused like revoked ones, and its API keys are honoured.
    ///
    /// Part of the configuration so it is fixed before the container, and so
    /// the API server's `AuthService`, exists. Only the desktop shell's local
    /// mode sets it; the server and Docker builds never do.
    pub local_desktop_mode: bool,
}

/// Snapshot of currently active recordings for desktop status and exit guards.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActiveRecordingSummary {
    pub active_count: usize,
    pub total_speed_bytes_per_sec: u64,
}

impl ServiceContainerConfig {
    fn standard(cache_ttl: Duration, event_capacity: usize) -> Result<Self> {
        Ok(Self {
            cache_ttl,
            event_capacity,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: PipelineManagerConfig::default(),
            api_config: ApiServerConfig::from_env()?,
            local_desktop_mode: false,
        })
    }
}

/// Service container holding all application services.
pub struct ServiceContainer {
    /// Database connection pool (read-heavy).
    pub(crate) pool: SqlitePool,
    /// Serialized write pool (max_connections=1) for contention-free writes.
    write_pool: SqlitePool,
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
    session_event_repository: Arc<dyn crate::database::repositories::SessionEventRepository>,
    streamer_check_history_repository:
        Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository>,
    upload_record_repository: Arc<dyn crate::database::repositories::UploadRecordRepository>,
    filter_repository: Arc<SqlxFilterRepository>,
    streamer_repository: Arc<SqlxStreamerRepository>,
    pipeline_preset_repository:
        Arc<crate::database::repositories::preset::SqlitePipelinePresetRepository>,
    configuration_import_service: Arc<crate::services::config_import::ConfigurationImportService>,
    configuration_export_service: Arc<crate::services::config_export::ConfigurationExportService>,
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
    /// Monitor event broadcaster.
    pub(crate) monitor_event_broadcaster: MonitorEventBroadcaster,
    /// Required, lossless monitor-event receiver used for runtime state changes.
    monitor_event_receiver: parking_lot::Mutex<
        Option<tokio::sync::mpsc::Receiver<crate::monitor::MonitorEventDelivery>>,
    >,
    /// Single required download lifecycle consumer, moved into its supervised task.
    download_coordination_receiver:
        parking_lot::Mutex<Option<crate::downloader::DownloadCoordinationReceiver>>,
    /// Single-owner session lifecycle service. Owns the in-memory session map,
    /// hysteresis timers, and the `SessionTransition` broadcast
    /// channel consumed by pipeline/notification/API layers.
    pub(crate) session_lifecycle: Arc<crate::session::SessionLifecycle>,
    /// Required session-transition receiver used for runtime side effects.
    session_transition_sender: crate::session::SessionTransitionSender,
    session_transition_receiver:
        parking_lot::Mutex<Option<crate::session::SessionTransitionReceiver>>,
    /// Operational policy for required runtime events.
    runtime_coordinator: Arc<RuntimeCoordinator>,
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
    /// Scheduler instance before its one-shot move into the runtime task.
    scheduler: parking_lot::Mutex<Option<Scheduler<SqlxStreamerRepository>>>,
    /// Read-only scheduler state available while the runtime task owns the scheduler.
    scheduler_handle: SchedulerHandle,
    /// Asks streamer actors for checks while the runtime task owns the scheduler.
    scheduler_feedback: Arc<crate::scheduler::feedback::SchedulerFeedback>,
    /// Stream monitor for real status detection
    pub(crate) stream_monitor:
        Arc<StreamMonitor<SqlxStreamerRepository, SqlxSessionRepository, SqlxConfigRepository>>,
    /// Bigo's live notifications, read by the monitor and the API.
    pub(crate) bigo_notifications: Arc<crate::monitor::BigoNotifications>,
    /// Credential refresh service (shared between monitor + API).
    pub(crate) credential_service: Arc<crate::credentials::CredentialProviderRegistry>,
    /// One platform rate budget shared by monitor, parse and credential providers.
    pub(crate) platform_admission: Arc<crate::credentials::PlatformAdmission>,
    pub(crate) credential_profiles:
        Arc<crate::database::repositories::credential_profiles::CredentialProfileRepository>,
    pub(crate) credential_login_sessions:
        Arc<crate::credentials::login_sessions::CredentialLoginSessions>,
    pub(crate) credential_execution: Arc<crate::credentials::CredentialExecutionService>,
    pub(crate) proxies: Arc<crate::proxies::ProxyService>,
    pub(crate) playback_contexts: Arc<crate::services::playback_context::PlaybackContextService>,
    /// Live broadcaster for committed check-history rows. Cloned into the
    /// downloads WS route so per-streamer subscribers see new bars appear
    /// without polling. Same fan-out pattern as
    /// [`crate::downloader::DownloadManager::subscribe`].
    pub(crate) check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    /// Live broadcaster for upload status events (started/progress/terminal).
    /// Published by `JobQueue`, consumed by the downloads WS route.
    pub(crate) upload_status_broadcaster: crate::pipeline::UploadStatusBroadcaster,
    /// API server configuration.
    api_server_config: ApiServerConfig,
    /// Token and password settings, read while building so an invalid value fails startup
    /// before any service starts.
    auth_config: crate::api::auth_service::AuthConfig,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Owner for background tasks started by the application runtime.
    task_supervisor: Arc<TaskSupervisor>,
    /// Database commits and their publication/completion survive forced task aborts.
    committed_write_supervisor: Arc<TaskSupervisor>,
    /// Logging configuration
    logging_config: Arc<std::sync::OnceLock<Arc<LoggingConfig>>>,
    /// Installed when the API server starts with database authentication.
    auth_service: std::sync::OnceLock<Arc<crate::api::auth_service::AuthService>>,
    /// See [`ServiceContainerConfig::local_desktop_mode`]; read when the API
    /// server's `AuthService` is built.
    local_desktop_mode: bool,
    startup_recovery_complete: std::sync::atomic::AtomicBool,
    /// Segment keys that should be discarded (min-size gate) to prevent danmu/xml and video
    /// from racing into the pipeline while being deleted.
    discarded_segment_keys: Arc<DashMap<(String, String), Instant>>,
    /// Selected recording control plane (`Legacy` by default in production).
    recorder_runtime: crate::recorder::RecorderRuntime,
    /// Shared stateless extractor runner used by `/api/parse` and the actor prober.
    pub(crate) extractor_runner: Arc<crate::recorder::PlatformExtractorRunner>,
    /// Present only in [`crate::recorder::RecorderRuntime::Actor`] mode.
    pub(crate) actor_registry: Option<Arc<crate::recorder::ActorRegistry>>,
    /// Present only in [`crate::recorder::RecorderRuntime::Actor`] mode; moved into
    /// its supervised task during [`Self::initialize`].
    outbox_drainer: parking_lot::Mutex<Option<Arc<crate::notification::NotificationOutboxDrainer>>>,
    /// Sessions whose danmu link is currently down, and when it went down.
    ///
    /// `CollectionRunner` keeps reconnecting for the life of the session, so a
    /// down link still counts as an active collection; without this the
    /// `danmu_service` health probe would report a silent outage as healthy.
    /// Maintained from `DanmuEvent::Reconnecting`/`Reconnected` and cleared when
    /// collection stops.
    danmu_link_down: Arc<DashMap<String, Instant>>,
}

/// Wire the streamer-check-history pipeline:
/// - One repository on top of the shared SQLite pools.
/// - One bounded MPSC; senders are cloned into every monitor poll.
/// - One broadcaster cloned into the downloads WS route loop, so live bars
///   stream to subscribed clients without polling.
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
    use prost::Message;

    let repo: Arc<dyn crate::database::repositories::StreamerCheckHistoryRepository> = Arc::new(
        crate::database::repositories::SqlxStreamerCheckHistoryRepository::new(
            pool.clone(),
            write_pool.clone(),
        ),
    );
    let (writer, rx) = crate::monitor::CheckHistoryWriter::new();

    // WS encoder: builds the protobuf payload + serializes it to bytes.
    // Stored on the broadcaster so encoding runs once per record (in the
    // drain task) instead of once per subscriber (in the WS route's
    // select loop). With N connected clients, this saves N − 1 protobuf
    // encodes per record.
    let encoder: crate::monitor::check_history_writer::WsEncoder = Arc::new(|record| {
        let msg = crate::api::routes::downloads::map_check_record_to_protobuf(record);
        bytes::Bytes::from(msg.encode_to_vec())
    });
    let broadcaster = crate::monitor::CheckHistoryBroadcaster::new(encoder);

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
    /// Active recording control plane mode for this container.
    pub fn recorder_runtime(&self) -> crate::recorder::RecorderRuntime {
        self.recorder_runtime
    }

    /// Active credential blocks store for the selected runtime mode.
    pub(crate) fn credential_blocks(&self) -> &Arc<crate::credentials::CredentialBlocks> {
        self.actor_registry
            .as_ref()
            .map(|registry| registry.prober().credential_blocks())
            .unwrap_or_else(|| self.stream_monitor.credential_blocks())
    }

    /// Access the actor registry when running in [`crate::recorder::RecorderRuntime::Actor`] mode.
    #[cfg(test)]
    pub(crate) fn actor_registry(&self) -> Option<&Arc<crate::recorder::ActorRegistry>> {
        self.actor_registry.as_ref()
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
        // for every hydrated streamer. Without this, freshly hydrated metadata
        // sits at the defaults (3 / 10 s) and platform/template/streamer overrides
        // wouldn't take effect until each streamer's config was independently
        // resolved (e.g. on first config-update event).
        self.runtime_coordinator
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

        // Subscribe streamer manager to config events
        self.setup_config_event_subscriptions();

        match self.recorder_runtime {
            crate::recorder::RecorderRuntime::Legacy => {
                // Wire download events to pipeline manager
                self.setup_download_event_subscriptions();

                // Wire download terminal events into SessionLifecycle so it can close
                // the session row and emit SessionTransition::Ended for every
                // terminal download outcome.
                self.setup_session_lifecycle_subscriptions();

                // Wire monitor events to download manager and danmu service
                self.setup_monitor_event_subscriptions();

                // Wire danmu events to download manager for segment coordination
                self.setup_danmu_event_subscriptions();

                // Wire notification service to system events
                self.setup_notification_event_subscriptions();
            }
            crate::recorder::RecorderRuntime::Actor => {
                if let Some(registry) = &self.actor_registry {
                    if let Some(coordination_rx) = self.download_coordination_receiver.lock().take()
                    {
                        registry.spawn_download_bridges(coordination_rx, &self.task_supervisor);
                    }
                    if let Some(danmu_rx) = self.danmu_coordination_receiver.lock().take() {
                        registry.spawn_danmu_bridge(danmu_rx, &self.task_supervisor);
                    }
                }
                self.notification_service.start_actor_mode_event_listeners(
                    self.download_manager.subscribe(),
                    self.pipeline_manager.subscribe(),
                );
            }
        }

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

        // Start the active recording control plane (either Legacy scheduler or Actor registry).
        match self.recorder_runtime {
            crate::recorder::RecorderRuntime::Legacy => {
                self.start_scheduler()?;
            }
            crate::recorder::RecorderRuntime::Actor => {
                if let Some(drainer) = self.outbox_drainer.lock().take() {
                    self.task_supervisor
                        .spawn("notification outbox drainer", async move {
                            if let Err(error) = drainer.drain_until_wait().await {
                                warn!(%error, "Initial notification outbox drain failed");
                            }
                            drainer.run().await;
                        });
                }
                if let Some(registry) = &self.actor_registry {
                    registry.start();
                }
                info!("Recorder actor registry started");
            }
        }

        // After the control plane starts, so `RuntimeCoordinator::retire_streamer` can
        // observe active actors on its first pass.
        self.spawn_streamer_reaper();
        self.spawn_login_session_pruning();
        let bigo_feedback: Arc<dyn crate::monitor::BigoCheckTrigger> = match &self.actor_registry {
            Some(registry) => registry.clone(),
            None => Arc::new(self.scheduler_feedback.clone()),
        };
        // After the control plane starts, which owns the actors its pushes ask to check.
        self.bigo_notifications.spawn(
            crate::monitor::BigoNotificationDeps {
                config_service: self.config_service.clone(),
                streamer_manager: self.streamer_manager.clone(),
                credentials: self.credential_execution.clone(),
                feedback: bigo_feedback,
                notifications: self.notification_service.clone(),
            },
            &self.task_supervisor,
            self.cancellation_token.child_token(),
        );

        info!("Startup: scheduler task started");

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
    /// `RuntimeCoordinator::retire_streamer` bound expired while the session's
    /// post-processing was running, or a row a crash left between the mark and
    /// the reap. The first pass runs immediately, which is the startup recovery;
    /// after that one pass per `STREAMER_REAP_INTERVAL` observes each owner once
    /// and reaps whatever has gone quiet.
    fn spawn_streamer_reaper(&self) {
        let runtime_coordinator = self.runtime_coordinator.clone();
        let write_pool = self.write_pool.clone();
        let cancellation_token = self.cancellation_token.child_token();

        self.task_supervisor.spawn("streamer reaper", async move {
            loop {
                let reaped = runtime_coordinator
                    .reap_marked_streamers(crate::services::runtime_coordinator::OBSERVE_RETIREMENT)
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

    /// Start the scheduler service in a background task.
    ///
    /// The scheduler uses a child token of the container's cancellation token,
    /// so it will automatically stop when the container is shut down.
    fn start_scheduler(&self) -> Result<()> {
        let mut scheduler =
            self.scheduler.lock().take().ok_or_else(|| {
                crate::Error::Other("scheduler has already been started".to_string())
            })?;

        if !self
            .task_supervisor
            .spawn_critical("scheduler", async move { scheduler.run().await })
        {
            return Err(crate::Error::Other(
                "scheduler task was rejected during shutdown".to_string(),
            ));
        }

        info!("Scheduler started");
        Ok(())
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

    /// The scheduler's control surface.
    ///
    /// `start_scheduler` moves the `Scheduler` itself into a task, so this is the
    /// only way to reach it afterwards.
    pub fn scheduler_handle(&self) -> &SchedulerHandle {
        &self.scheduler_handle
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

    /// Issue a local desktop session for the active admin user when the
    /// loopback desktop app operates without password sign-in.
    pub async fn issue_local_desktop_session(
        &self,
    ) -> Result<(String, crate::api::auth_service::AuthResponse)> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.issue_local_desktop_session()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    /// The account local desktop sessions are issued for, or `None` when
    /// there is no active admin.
    pub async fn local_desktop_account(
        &self,
    ) -> Result<Option<crate::api::auth_service::LocalDesktopAccount>> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.local_desktop_account()
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    /// Replace the initial password of the local desktop account. Fails once
    /// that account has a password of its own.
    pub async fn set_local_desktop_password(&self, new_password: &str) -> Result<()> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.set_local_desktop_password(new_password)
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
    }

    /// Restore previously persisted local desktop `(session_id, user_id)` pairs
    /// on startup when password sign-in remains disabled.
    pub fn restore_local_desktop_sessions(
        &self,
        sessions: impl IntoIterator<Item = (String, String)>,
    ) {
        if let Some(auth) = self.auth_service.get() {
            auth.restore_local_desktop_sessions(sessions);
        }
    }

    /// Return the currently tracked local desktop `(session_id, user_id)` pairs.
    pub fn list_local_desktop_sessions(&self) -> Vec<(String, String)> {
        self.auth_service
            .get()
            .map(|auth| auth.list_local_desktop_sessions())
            .unwrap_or_default()
    }

    /// Revoke all tracked local-desktop sessions (plus any persisted entries) in
    /// SQLite and clear their in-memory exemptions.
    pub async fn revoke_local_desktop_sessions(
        &self,
        extra_sessions: &[(String, String)],
    ) -> Result<usize> {
        let auth = self.auth_service.get().ok_or_else(|| {
            crate::Error::ApiError("Authentication service is not initialized".to_string())
        })?;
        auth.revoke_local_desktop_sessions(extra_sessions)
            .await
            .map_err(|error| crate::Error::ApiError(error.to_string()))
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
