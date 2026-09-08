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
mod event_policy;
mod events;
mod health;
mod output_roots;
mod shutdown;
#[cfg(test)]
mod streamer_retirement_tests;
#[cfg(test)]
mod tests;

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
}

impl ServiceContainerConfig {
    fn standard(cache_ttl: Duration, event_capacity: usize) -> Self {
        Self {
            cache_ttl,
            event_capacity,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: PipelineManagerConfig::default(),
            api_config: ApiServerConfig::from_env_or_default(),
        }
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
    /// Stream monitor for real status detection
    pub(crate) stream_monitor: Arc<
        StreamMonitor<
            SqlxStreamerRepository,
            SqlxFilterRepository,
            SqlxSessionRepository,
            SqlxConfigRepository,
        >,
    >,
    /// Credential refresh service (shared between monitor + API).
    pub(crate) credential_service:
        Arc<crate::credentials::CredentialRefreshService<SqlxConfigRepository>>,
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
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
    /// Owner for background tasks started by the application runtime.
    task_supervisor: Arc<TaskSupervisor>,
    /// Logging configuration
    logging_config: std::sync::OnceLock<Arc<LoggingConfig>>,
    startup_recovery_complete: std::sync::atomic::AtomicBool,
    /// Segment keys that should be discarded (min-size gate) to prevent danmu/xml and video
    /// from racing into the pipeline while being deleted.
    discarded_segment_keys: Arc<DashMap<(String, String), Instant>>,
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
            repo,
            rx,
            Some(broadcaster.clone()),
            cancellation_token.child_token(),
        ),
    );
    (writer, broadcaster)
}

impl ServiceContainer {
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
        // sits at default (3 / 20_000) and platform/template/streamer overrides
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

        // Start scheduler in background
        self.start_scheduler()?;

        // After the scheduler, so `RuntimeCoordinator::retire_streamer` can get
        // an answer from `SchedulerHandle::remove_streamer_awaitable` on its
        // first pass instead of treating every actor as unobserved.
        self.spawn_streamer_reaper();

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
        let cancellation_token = self.cancellation_token.child_token();

        self.task_supervisor.spawn("streamer reaper", async move {
            loop {
                let reaped = runtime_coordinator
                    .reap_marked_streamers(crate::services::runtime_coordinator::OBSERVE_RETIREMENT)
                    .await;
                if reaped > 0 {
                    info!(reaped, "Removed streamers whose retirement completed");
                }

                tokio::select! {
                    () = cancellation_token.cancelled() => break,
                    () = tokio::time::sleep(STREAMER_REAP_INTERVAL) => {}
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
        scheduler.set_download_receiver(self.download_manager.subscribe());

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

    /// Build a point-in-time [`ServiceStats`] snapshot from live service
    /// counters.
    ///
    /// Exported through [`crate::backend`] for embedders; nothing inside
    /// the crate calls it. Every field is read at call time from the
    /// owning service; `scheduler_stats` reads the `SchedulerHandle`
    /// watch channel, so it stays current after `start_scheduler` moves
    /// the `Scheduler` into its runtime task.
    pub fn stats(&self) -> ServiceStats {
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
            scheduler_stats: Some(self.scheduler_handle.stats()),
        }
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

/// Point-in-time service counters returned by [`ServiceContainer::stats`],
/// exported through [`crate::backend`] for embedders.
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
    /// Scheduler supervisor statistics. [`ServiceContainer::stats`] always
    /// populates this from the scheduler's watch channel.
    pub scheduler_stats: Option<crate::scheduler::actor::SupervisorStats>,
}
