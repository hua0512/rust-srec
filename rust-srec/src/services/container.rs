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
use crate::api::server::ApiServerConfig;
use crate::config::{ConfigEventBroadcaster, ConfigService};
use crate::danmu::DanmuService;
use crate::database::maintenance::MaintenanceScheduler;
use crate::database::repositories::NotificationRepository;
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{
    DownloadManager, DownloadManagerConfig, LAST_ERROR_GATE_PREFIX, OutputRootGate, RecoveryHook,
    engine::DownloadProgress,
};
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

mod api;
mod builder;
mod events;
mod health;

/// Build the recovery hook closure for the output-root write gate.
///
/// The closure iterates the streamer metadata store, finds every streamer
/// whose `last_error` starts with [`LAST_ERROR_GATE_PREFIX`] (i.e., was
/// placed in backoff by `InfraBlockReason::OutputRootUnavailable`), and
/// clears their error state via `StreamerManager::clear_error_state` so
/// they immediately re-enter the live-check rotation.
///
/// Invoked by [`OutputRootGate::mark_healthy`] on every `Degraded → Healthy`
/// transition. The synchronous portion only snapshots IDs; database writes
/// are handed to the application task supervisor.
fn build_output_root_gate_recovery_hook<R>(
    streamer_manager: Arc<StreamerManager<R>>,
    task_supervisor: Arc<TaskSupervisor>,
) -> RecoveryHook
where
    R: crate::database::repositories::streamer::StreamerRepository + Send + Sync + 'static,
    StreamerManager<R>: Send + Sync + 'static,
{
    Arc::new(move |root: &std::path::Path| {
        // Build the exact prefix this root's streamers would carry in
        // `last_error`. `set_infra_blocked` writes
        //     "output-root blocked: {root.display()} ({io_kind})"
        // so we filter by "output-root blocked: {root.display()} " (with
        // trailing space) to discriminate between streamers blocked on THIS
        // root vs a different Degraded root. Without the trailing space,
        // "/rec" would also match "/rec/huya" — which would wipe streamers
        // that are still legitimately blocked.
        let root_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root.display());

        // Snapshot affected streamer IDs first so we don't hold a DashMap
        // iterator across await points. `metadata_store()` returns an
        // `Arc<DashMap<_>>`; iteration holds per-bucket read locks.
        let affected_ids: Vec<String> = streamer_manager
            .metadata_store()
            .iter()
            .filter(|entry| {
                entry
                    .last_error
                    .as_deref()
                    .is_some_and(|s| s.starts_with(&root_marker))
            })
            .map(|entry| entry.key().clone())
            .collect();

        if affected_ids.is_empty() {
            debug!(
                root = %root.display(),
                "Output-root gate recovery hook fired but no affected streamers found"
            );
            return;
        }

        info!(
            root = %root.display(),
            count = affected_ids.len(),
            "Output-root gate recovered; clearing error state for affected streamers"
        );

        // Keep database writes outside the synchronous gate callback and
        // serialize them to avoid a write burst during fleet recovery.
        let sm = streamer_manager.clone();
        task_supervisor.spawn("output-root recovery", async move {
            for id in affected_ids {
                if let Err(e) = sm.clear_error_state(&id).await {
                    warn!(
                        streamer_id = %id,
                        error = %e,
                        "Failed to clear error state during gate recovery (non-fatal)"
                    );
                }
            }
        });
    })
}

/// Extract the static root-prefix from a user-configured `output_folder`
/// template (e.g. `"/rec/{platform}/{streamer}/%Y%m%d"`), used by the
/// startup probe to derive a mount root from a template without
/// evaluating its placeholders.
///
/// Algorithm:
/// 1. Truncate at the first `{` (curly-brace variable) or `%` (strftime
///    placeholder) — everything after is streamer/date-dependent and not
///    part of the mount.
/// 2. Trim to end at the last `/` so we don't emit a partial directory
///    name (e.g. `/recordings-` from `/recordings-{streamer}/files`).
/// 3. Return `None` for relative, empty, or root-only prefixes that
///    carry no useful probe signal (relative templates would anchor to
///    the container's CWD, which is unpredictable).
///
/// Examples:
///   `"/rec/{platform}/{streamer}"` → `Some("/rec/")`
///   `"/home/{user}/recordings/"` → `Some("/home/")`
///   `"/app/output"` (no placeholder) → `Some("/app/")`
///   `"/recordings-{streamer}/files"` → `None` (last-complete-segment is `/`)
///   `"{streamer}/files"` (no root) → `None`
///   `"recordings/{streamer}"` (relative) → `None`
fn static_root_prefix(template: &str) -> Option<String> {
    if !template.starts_with('/') {
        return None;
    }
    let cut = template.find(['{', '%']).unwrap_or(template.len());
    let prefix = &template[..cut];
    let last_slash = prefix.rfind('/')?;
    let result = &prefix[..=last_slash];
    if result.is_empty() || result == "/" {
        None
    } else {
        Some(result.to_string())
    }
}

/// Read `RUST_SREC_OUTPUT_ROOTS` from the environment and parse it into a
/// list of absolute paths. The value is comma-separated; empty entries are
/// skipped. Relative paths are rejected with a warning (they would anchor
/// to the current working directory, which is unpredictable inside Docker).
fn parse_output_roots_env() -> Vec<std::path::PathBuf> {
    let Ok(raw) = std::env::var("RUST_SREC_OUTPUT_ROOTS") else {
        return Vec::new();
    };
    raw.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| {
            let p = std::path::PathBuf::from(s);
            if p.is_absolute() {
                Some(p)
            } else {
                warn!(
                    entry = %s,
                    "Ignoring non-absolute entry in RUST_SREC_OUTPUT_ROOTS"
                );
                None
            }
        })
        .collect()
}

fn sqlite_file_path_from_url(url: &str) -> Option<std::path::PathBuf> {
    let url = url.strip_prefix("sqlite:")?;
    let path_part = url.split('?').next().unwrap_or(url);

    if path_part.is_empty() || path_part == ":memory:" || path_part.starts_with(":memory:") {
        return None;
    }

    let normalized = path_part.strip_prefix("///").unwrap_or(path_part);
    Some(std::path::PathBuf::from(normalized))
}

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

const RECOVERY_PROGRESS_MIN_BYTES: u64 = 8 * 1024 * 1024;

fn has_transient_error_state(metadata: &crate::streamer::StreamerMetadata) -> bool {
    metadata.consecutive_error_count > 0
        || metadata.disabled_until.is_some()
        || metadata.last_error.is_some()
}

fn should_record_recovery_from_progress(progress: &DownloadProgress) -> bool {
    progress.segments_completed > 0
        || (progress.bytes_downloaded >= RECOVERY_PROGRESS_MIN_BYTES
            && progress.speed_bytes_per_sec > 0)
}

/// Default cache TTL (1 hour).
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Default shutdown timeout.
const DEFAULT_SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(30);

/// Floor for the window `shutdown_until` hands `TaskSupervisor::shutdown`.
///
/// The producer drains run first against the same absolute deadline, so a slow
/// recording flush can leave nothing behind. Without a floor every supervised
/// task that merely had not been reaped yet would be reported as an overrun.
const BACKGROUND_TASK_MIN_GRACE_PERIOD: Duration = Duration::from_secs(2);

/// Time reserved inside a hard cap for aborted tasks to settle. Joining an
/// aborted task proves its future was dropped, which is when an engine child
/// spawned with `kill_on_drop` is killed.
const ABORT_REAP_WINDOW: Duration = Duration::from_secs(5);

/// Absolute deadlines consumed by the service shutdown module.
///
/// The cooperative drain may keep containing owned work after
/// `cooperative_deadline`, but at `force_deadline` it is dropped and the
/// remaining supervised tasks are aborted. All abort/reap work shares
/// `hard_deadline`; no phase extends the caller's hard bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServiceShutdownSchedule {
    cooperative_deadline: tokio::time::Instant,
    force_deadline: tokio::time::Instant,
    hard_deadline: tokio::time::Instant,
}

impl ServiceShutdownSchedule {
    pub(crate) fn new(
        cooperative_deadline: tokio::time::Instant,
        force_deadline: tokio::time::Instant,
        hard_deadline: tokio::time::Instant,
    ) -> Result<Self> {
        if cooperative_deadline > force_deadline || force_deadline > hard_deadline {
            return Err(crate::Error::validation(
                "service shutdown deadlines must satisfy cooperative <= force <= hard",
            ));
        }
        Ok(Self {
            cooperative_deadline,
            force_deadline,
            hard_deadline,
        })
    }

    fn from_now(grace_period: Duration, hard_cap: Duration) -> Result<Self> {
        let started_at = tokio::time::Instant::now();
        let hard_deadline = started_at.checked_add(hard_cap).ok_or_else(|| {
            crate::Error::validation("service shutdown hard cap exceeds the monotonic clock range")
        })?;
        // A fixed five-second reserve would consume an entire short cap and
        // force even quiescent services immediately. Preserve at least half of
        // a short cap for cooperative shutdown while retaining the full reap
        // window for normal production-sized caps.
        let abort_reap_window = ABORT_REAP_WINDOW.min(hard_cap / 2);
        let force_deadline = hard_deadline
            .checked_sub(abort_reap_window)
            .unwrap_or(started_at)
            .max(started_at);
        let cooperative_deadline = started_at
            .checked_add(grace_period)
            .unwrap_or(force_deadline)
            .min(force_deadline);
        Self::new(cooperative_deadline, force_deadline, hard_deadline)
    }
}

fn autoscale_concurrency_limit(raw: i32) -> usize {
    if raw > 0 {
        return raw as usize;
    }

    let cores = std::thread::available_parallelism().map_or(2, std::num::NonZeroUsize::get);

    (cores / 2).max(1)
}

fn broadcast_error_is_recoverable(
    subscriber: &'static str,
    error: tokio::sync::broadcast::error::RecvError,
) -> bool {
    match error {
        tokio::sync::broadcast::error::RecvError::Lagged(skipped) => {
            warn!(
                subscriber,
                skipped, "Broadcast subscriber lagged; continuing from the newest available event"
            );
            true
        }
        tokio::sync::broadcast::error::RecvError::Closed => {
            debug!(subscriber, "Broadcast channel closed");
            false
        }
    }
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
    /// hard-ended suppression cache, and the `SessionTransition` broadcast
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

        // Populate resolved offline_check_* on the in-memory metadata cache
        // for every hydrated streamer. Without this, freshly hydrated metadata
        // sits at default (3 / 20_000) and platform/template/streamer overrides
        // wouldn't take effect until each streamer's config was independently
        // resolved (e.g. on first config-update event).
        for metadata in self.streamer_manager.get_all() {
            self.runtime_coordinator
                .refresh_metadata_offline_check(&metadata.id)
                .await;
        }

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

        // One-shot output-root write gate startup probe. Discovers
        // broken mounts (e.g., stale Docker bind mounts from host-side
        // cleanup) on container boot rather than waiting for the first
        // monitor tick to try starting a download. Per-root probes run in
        // parallel with a bounded per-root timeout so a hung mount can't
        // wedge startup.
        self.run_output_root_startup_probe().await;

        // Start the single database maintenance task. It performs an immediate
        // retention sweep before waiting for its periodic cadence.
        let maintenance_start = Instant::now();
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
        let maintenance_start_ms = maintenance_start.elapsed().as_millis();
        info!("Database maintenance scheduler started");

        // Start scheduler in background
        let scheduler_start = Instant::now();
        self.start_scheduler()?;

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

    /// Shutdown all services, waiting as long as containment takes.
    ///
    /// `DEFAULT_SHUTDOWN_GRACE_PERIOD` bounds only the cooperative
    /// phase. The drains in `TaskSupervisor::shutdown`,
    /// `DownloadManager::shutdown_until` and `PipelineManager::stop` all keep
    /// joining past it so no producer outlives the database pools, so this call
    /// has no wall-clock bound and a wedged engine holds it open indefinitely.
    /// Only use it under a parent process that force-kills the process tree;
    /// in-process embedders want [`Self::shutdown_with_hard_cap`].
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_grace_period(DEFAULT_SHUTDOWN_GRACE_PERIOD)
            .await
    }

    /// Shutdown all services within `hard_cap`.
    ///
    /// [`Self::shutdown`] waits as long as containment takes: the drains in
    /// `TaskSupervisor::shutdown`, `DownloadManager::shutdown_until` and
    /// `PipelineManager::stop` all keep joining past the grace period so no
    /// producer outlives the database pools. That is only safe with a parent
    /// process that enforces a wall-clock deadline and force-kills the process
    /// tree. Embedders that run the container in-process call
    /// this instead: before `hard_cap` expires the phased drain is dropped and
    /// the remaining work is aborted, so each attempt/job future is dropped and
    /// the ffmpeg/streamlink child it owns through `kill_on_drop` is killed
    /// rather than orphaned by a later `std::process::exit`.
    ///
    /// Returns `Err` naming what was still running when the cap fired. On that
    /// path the database pools are left open, matching the quiescence gate in
    /// the cooperative shutdown path; the caller is expected to exit the
    /// process. `ABORT_REAP_WINDOW` is reserved inside `hard_cap`, so this
    /// method does not add a second timeout after the caller's deadline.
    pub async fn shutdown_with_hard_cap(
        &self,
        grace_period: Duration,
        hard_cap: Duration,
    ) -> Result<()> {
        let schedule = ServiceShutdownSchedule::from_now(grace_period, hard_cap)?;
        self.shutdown_with_schedule(schedule).await
    }

    /// Run the phased shutdown against caller-owned absolute deadlines.
    pub(crate) async fn shutdown_with_schedule(
        &self,
        schedule: ServiceShutdownSchedule,
    ) -> Result<()> {
        let mut graceful = Box::pin(self.shutdown_until(schedule.cooperative_deadline));
        if let Ok(result) = tokio::time::timeout_at(schedule.force_deadline, &mut graceful).await {
            return result;
        }

        // Drop the drain before aborting. Every drain it owns hands its
        // `JoinSet` back to its service through `DrainedTasks` when its future
        // is dropped, which is what lets the abort hatches below reach the
        // tasks that are still running instead of silently aborting them here.
        drop(graceful);
        warn!(
            "Service shutdown reached its force deadline; aborting the remaining supervised work"
        );

        let (
            aborted_downloads,
            aborted_collections,
            aborted_timers,
            aborted_pipeline_tasks,
            aborted_background_tasks,
        ) = tokio::join!(
            self.download_manager.abort_attempts(schedule.hard_deadline),
            self.danmu_service.abort_collections(schedule.hard_deadline),
            self.session_lifecycle.abort_timers(schedule.hard_deadline),
            self.pipeline_manager.abort(schedule.hard_deadline),
            self.task_supervisor.abort_all(schedule.hard_deadline),
        );

        warn!(
            downloads = ?aborted_downloads,
            collections = ?aborted_collections,
            hysteresis_timers = aborted_timers,
            pipeline_tasks = aborted_pipeline_tasks,
            background_tasks = aborted_background_tasks,
            "Aborted supervised work after the shutdown force deadline"
        );

        Err(crate::Error::Other(format!(
            "service shutdown exceeded its force deadline: aborted {} recording attempt(s) {aborted_downloads:?}, {} danmu collection(s) {aborted_collections:?}, {aborted_timers} hysteresis timer(s), {aborted_pipeline_tasks} pipeline task(s) and {aborted_background_tasks} background task(s) by the hard deadline; database pools were left open",
            aborted_downloads.len(),
            aborted_collections.len()
        )))
    }

    /// Shutdown all services with a bounded cooperative grace period.
    ///
    /// If that period expires, containment may run longer while owned tasks
    /// cancel and join. Database pools are never closed until every producer
    /// that can write through them is proven quiescent.
    pub(crate) async fn shutdown_with_grace_period(&self, grace_period: Duration) -> Result<()> {
        info!("Shutting down services (grace period: {:?})", grace_period);
        let deadline = tokio::time::Instant::now() + grace_period;
        self.shutdown_until(deadline).await
    }

    async fn shutdown_until(&self, deadline: tokio::time::Instant) -> Result<()> {
        // Containment failures: something could not be proven to have stopped
        // writing. Only these make this method return `Err`, which `server.rs`
        // turns into a nonzero worker exit and `classify_settled_exit` records
        // as a crash that retains a dirty generation marker.
        let mut failures = Vec::new();
        // Soft-budget overruns and errors from work that ended earlier in the
        // run. Both are reported and then ignored: a drain that overran its
        // grace period still finalized every recording, and a download that
        // failed hours ago says nothing about this shutdown.
        let mut overruns = Vec::new();
        let mut runtime_failures = Vec::new();
        let mut coordination_drained = true;

        // Fence producers before cancellation. Required coordination consumers
        // are marker-driven, so they remain alive long enough to persist every
        // final segment and terminal fact after auxiliary loops stop.
        self.stream_monitor.stop();
        self.download_manager.shutdown_queue();
        self.cancellation_token.cancel();

        info!("Stopping download manager...");
        let download_report = self.download_manager.shutdown_until(deadline).await;
        info!(
            count = download_report.stopped_download_ids.len(),
            deadline_exceeded = download_report.deadline_exceeded_download_ids.len(),
            "Stopped active downloads and drained required download events"
        );
        coordination_drained &= download_report.coordination_drained;
        runtime_failures.extend(
            download_report
                .runtime_failures
                .into_iter()
                .map(|failure| format!("download run: {failure}")),
        );
        overruns.extend(
            download_report
                .overruns
                .into_iter()
                .map(|overrun| format!("download shutdown: {overrun}")),
        );
        failures.extend(
            download_report
                .failures
                .into_iter()
                .map(|failure| format!("download shutdown: {failure}")),
        );

        info!("Stopping danmu service...");
        let danmu_report = self.danmu_service.shutdown_until(deadline).await;
        if !danmu_report.forced_session_ids.is_empty() {
            warn!(
                sessions = ?danmu_report.forced_session_ids,
                "Danmu collections exceeded the graceful deadline and were drained"
            );
        }
        runtime_failures.extend(
            danmu_report
                .runtime_failures
                .into_iter()
                .map(|failure| format!("danmu run: {failure}")),
        );
        overruns.extend(
            danmu_report
                .overruns
                .into_iter()
                .map(|overrun| format!("danmu shutdown: {overrun}")),
        );
        failures.extend(
            danmu_report
                .shutdown_failures
                .into_iter()
                .map(|failure| format!("danmu shutdown: {failure}")),
        );

        if self.danmu_coordination_receiver.lock().is_none() {
            match tokio::time::timeout_at(deadline, self.danmu_coordination_sender.shutdown()).await
            {
                Ok(Ok(event_failures)) => {
                    // Per-event handler errors accumulated across the whole run,
                    // not shutdown-phase containment failures.
                    runtime_failures.extend(
                        event_failures
                            .into_iter()
                            .map(|failure| format!("danmu coordination event failed: {failure}")),
                    );
                }
                Ok(Err(error)) => {
                    let message = format!("failed to drain danmu coordination events: {error}");
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
                Err(_) => {
                    let message = "danmu coordination drain deadline exceeded".to_string();
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
            }
        } else {
            debug!("Danmu coordination handler was not started; skipping its shutdown barrier");
        }

        let lifecycle_report = self.session_lifecycle.shutdown_until(deadline).await;
        if lifecycle_report.forced_timer_count > 0 {
            warn!(
                count = lifecycle_report.forced_timer_count,
                "Session hysteresis timers exceeded the graceful deadline and were drained"
            );
        }
        overruns.extend(
            lifecycle_report
                .overruns
                .into_iter()
                .map(|overrun| format!("session lifecycle: {overrun}")),
        );
        failures.extend(
            lifecycle_report
                .failures
                .into_iter()
                .map(|failure| format!("session lifecycle task failed: {failure}")),
        );

        if self.session_transition_receiver.lock().is_none() {
            match tokio::time::timeout_at(deadline, self.session_transition_sender.shutdown()).await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    let message = format!("failed to drain session transitions: {error}");
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
                Err(_) => {
                    let message = "session transition drain deadline exceeded".to_string();
                    warn!(%message);
                    failures.push(message);
                    coordination_drained = false;
                }
            }
        } else {
            debug!("Session transition coordinator was not started; skipping its shutdown barrier");
        }

        // Every producer above joins past `deadline` rather than abandoning
        // work, so reaching here means they are quiesced regardless of whether
        // they stayed inside their budget. Downstream services can stop.
        info!("Stopping pipeline manager...");
        self.pipeline_manager.stop().await;
        info!("Stopping notification service...");
        self.notification_service.stop().await;

        // The producer drains above may have consumed the whole window. Give
        // the background tasks their own grace period rather than a zero-length
        // one that reports an overrun for every task that is merely unreaped.
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(BACKGROUND_TASK_MIN_GRACE_PERIOD);
        if !self.task_supervisor.shutdown(remaining).await {
            let message =
                "one or more background tasks exceeded the graceful shutdown period".to_string();
            warn!(%message);
            overruns.push(message);
        }

        // `TaskSupervisor::shutdown` has joined the required consumers by now,
        // so nothing can still be mid-write. An undrained marker means one of
        // them stopped without acknowledging, and leaving the pools open keeps
        // the last observed state visible to the operator who has to reconcile
        // it — the process exits immediately after this either way.
        if coordination_drained {
            info!("Closing database pools...");
            tokio::join!(self.write_pool.close(), self.pool.close());
        } else {
            warn!("Required coordination markers did not drain; leaving database pools open");
        }

        for failure in &runtime_failures {
            warn!(%failure, "Work ended with an error during the run");
        }
        for overrun in &overruns {
            warn!(%overrun, "Shutdown phase exceeded its grace period but was contained");
        }

        info!("Services shut down");
        if failures.is_empty() {
            Ok(())
        } else {
            Err(crate::Error::Other(format!(
                "service shutdown incomplete: {}",
                failures.join("; ")
            )))
        }
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use tokio::io::AsyncWriteExt;
    use tokio::sync::{Notify, mpsc};

    use super::{
        RECOVERY_PROGRESS_MIN_BYTES, broadcast_error_is_recoverable,
        should_end_stream_on_danmu_stream_closed, should_record_recovery_from_progress,
    };
    use crate::danmu::test_support::FakeProvider;
    use crate::danmu::{CollectionSpec, DanmuService, ProviderRegistry};
    use crate::database::models::{LiveSessionDbModel, StreamerDbModel, StreamerState};
    use crate::database::repositories::{
        SessionRepository, SqlxSessionRepository, SqlxStreamerRepository, StreamerRepository,
    };
    use crate::downloader::engine::{DownloadProgress, EngineStartError, EngineType};
    use crate::downloader::{
        DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadManagerConfig,
        DownloadManagerEvent, DownloadProgressEvent, DownloadProtocol, DownloadStopCause,
        DownloadTerminalEvent, EngineEndSignal, SegmentEvent, SegmentInfo,
    };

    const SHUTDOWN_STREAMER_ID: &str = "shutdown-tracer-streamer";
    const SHUTDOWN_SESSION_ID: &str = "shutdown-tracer-session";
    const OPEN_SEGMENT_BYTES: &[u8] = b"recording";
    const FINAL_SEGMENT_BYTES: &[u8] = b"-finalized";
    const EXPECTED_SEGMENT_BYTES: &[u8] = b"recording-finalized";

    #[derive(Clone)]
    struct ShutdownFlushEngine {
        segment_path: PathBuf,
        started: Arc<Notify>,
    }

    #[async_trait]
    impl DownloadEngine for ShutdownFlushEngine {
        fn engine_type(&self) -> EngineType {
            EngineType::Ffmpeg
        }

        async fn run(
            &self,
            handle: Arc<DownloadHandle>,
        ) -> std::result::Result<(), EngineStartError> {
            let started_at = Utc::now();
            let mut file = tokio::fs::File::create(&self.segment_path)
                .await
                .map_err(|error| {
                    EngineStartError::new(
                        DownloadFailureKind::Io,
                        format!("failed to create shutdown tracer segment: {error}"),
                    )
                })?;
            file.write_all(OPEN_SEGMENT_BYTES).await.map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to write shutdown tracer segment: {error}"),
                )
            })?;
            file.flush().await.map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to flush shutdown tracer segment: {error}"),
                )
            })?;
            handle
                .event_tx
                .send(SegmentEvent::SegmentStarted {
                    path: self.segment_path.clone(),
                    sequence: 0,
                    started_at,
                })
                .await
                .map_err(|error| {
                    EngineStartError::new(
                        DownloadFailureKind::Other,
                        format!("failed to emit shutdown tracer segment start: {error}"),
                    )
                })?;
            self.started.notify_one();

            handle.cancellation_token.cancelled().await;

            file.write_all(FINAL_SEGMENT_BYTES).await.map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to finalize shutdown tracer segment: {error}"),
                )
            })?;
            file.flush().await.map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to flush finalized shutdown tracer segment: {error}"),
                )
            })?;
            file.sync_all().await.map_err(|error| {
                EngineStartError::new(
                    DownloadFailureKind::Io,
                    format!("failed to sync finalized shutdown tracer segment: {error}"),
                )
            })?;
            drop(file);

            let size_bytes = tokio::fs::metadata(&self.segment_path)
                .await
                .map_err(|error| {
                    EngineStartError::new(
                        DownloadFailureKind::Io,
                        format!("failed to stat finalized shutdown tracer segment: {error}"),
                    )
                })?
                .len();
            handle
                .event_tx
                .send(SegmentEvent::SegmentCompleted(SegmentInfo {
                    path: self.segment_path.clone(),
                    duration_secs: 1.0,
                    size_bytes,
                    index: 0,
                    started_at: Some(started_at),
                    completed_at: Utc::now(),
                    split_reason_code: None,
                    split_reason_details_json: None,
                }))
                .await
                .map_err(|error| {
                    EngineStartError::new(
                        DownloadFailureKind::Other,
                        format!("failed to emit shutdown tracer segment completion: {error}"),
                    )
                })?;
            handle
                .event_tx
                .send(SegmentEvent::DownloadCompleted {
                    total_bytes: size_bytes,
                    total_duration_secs: 1.0,
                    total_segments: 1,
                    engine_signal: EngineEndSignal::CleanDisconnect,
                })
                .await
                .map_err(|error| {
                    EngineStartError::new(
                        DownloadFailureKind::Other,
                        format!("failed to emit shutdown tracer terminal event: {error}"),
                    )
                })?;
            Ok(())
        }

        fn is_available(&self) -> bool {
            true
        }

        fn version(&self) -> Option<String> {
            Some("shutdown-tracer".to_string())
        }
    }

    async fn migrated_test_pool() -> sqlx::SqlitePool {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .expect("test database should initialize");
        crate::database::run_migrations(&pool)
            .await
            .expect("test migrations should succeed");
        pool
    }

    #[tokio::test]
    async fn graceful_shutdown_flushes_and_persists_the_final_segment_before_pool_close() {
        let temp_dir = tempfile::tempdir().expect("test directory should initialize");
        let database_path = temp_dir.path().join("shutdown-tracer.sqlite");
        let database_url = format!("sqlite:{}?mode=rwc", database_path.to_string_lossy());
        let output_dir = temp_dir.path().join("recordings");
        let segment_path = output_dir.join("segment-0.flv");
        let segment_path_string = segment_path.to_string_lossy().into_owned();
        let expected_size = 19;
        let danmu_path = segment_path.with_extension("xml");

        let (pool, write_pool) = crate::database::init_database_pools(&database_url)
            .await
            .expect("test database pools should initialize");
        crate::database::run_migrations(&pool)
            .await
            .expect("test migrations should succeed");
        let paired_segment_pipeline = crate::database::models::DagPipelineDefinition::new(
            "Shutdown Paired Segment Pipeline",
            vec![crate::database::models::DagStep::new(
                "record-final-segment",
                crate::database::models::PipelineStep::Inline {
                    processor: "execute".to_string(),
                    config: serde_json::json!({
                        "command": if cfg!(windows) { "exit /B 0" } else { "true" }
                    }),
                },
            )],
        );
        let paired_segment_pipeline_json = serde_json::to_string(&paired_segment_pipeline)
            .expect("test pipeline should serialize");
        let updated = sqlx::query(
            "UPDATE global_config \
             SET min_segment_size_bytes = 0, auto_thumbnail = FALSE, record_danmu = TRUE, paired_segment_pipeline = ? \
             WHERE id = 'global-configuration'",
        )
        .bind(paired_segment_pipeline_json)
        .execute(&write_pool)
        .await
        .expect("shutdown tracer config should update");
        assert_eq!(updated.rows_affected(), 1);

        let mut streamer = StreamerDbModel::new(
            "Shutdown tracer",
            "https://example.com/shutdown-tracer",
            "platform-twitch",
        );
        streamer.id = SHUTDOWN_STREAMER_ID.to_string();
        streamer.state = StreamerState::Live.as_str().to_string();
        SqlxStreamerRepository::new(pool.clone(), write_pool.clone())
            .create_streamer(&streamer)
            .await
            .expect("shutdown tracer streamer should persist");

        let mut session = LiveSessionDbModel::new(SHUTDOWN_STREAMER_ID);
        session.id = SHUTDOWN_SESSION_ID.to_string();
        SqlxSessionRepository::new(pool.clone(), write_pool.clone())
            .create_session(&session)
            .await
            .expect("shutdown tracer session should persist");

        let mut container = super::ServiceContainer::with_full_config(
            pool,
            write_pool,
            super::ServiceContainerConfig {
                cache_ttl: Duration::from_secs(60),
                event_capacity: 8,
                download_config: DownloadManagerConfig::default(),
                pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
                api_config: crate::api::server::ApiServerConfig::default(),
            },
        )
        .await
        .expect("service container should initialize");

        let (_danmu_items_tx, danmu_items_rx) = mpsc::channel(8);
        let mut providers = ProviderRegistry::new();
        providers.register(Arc::new(FakeProvider::new(vec![danmu_items_rx])));
        let (danmu_coordination_sender, danmu_coordination_receiver) =
            crate::danmu::events::danmu_coordination_channel();
        let danmu_service = Arc::new(
            DanmuService::with_providers(providers)
                .with_session_repository(container.session_repository.clone())
                .with_coordination_sender(danmu_coordination_sender.clone()),
        );
        container.danmu_service = danmu_service.clone();
        container.danmu_coordination_sender = danmu_coordination_sender;
        *container.danmu_coordination_receiver.lock() = Some(danmu_coordination_receiver);

        container.pipeline_manager.clone().start();
        container.setup_download_event_subscriptions();
        container.setup_session_lifecycle_subscriptions();
        container.setup_danmu_event_subscriptions();
        danmu_service
            .start_collection(CollectionSpec {
                session_id: SHUTDOWN_SESSION_ID.to_string(),
                streamer_id: SHUTDOWN_STREAMER_ID.to_string(),
                streamer_url: FakeProvider::URL.to_string(),
                cookies: None,
                extras: None,
                statistics: crate::domain::DanmuStatisticsConfig::default(),
            })
            .await
            .expect("shutdown tracer danmu collection should start");

        let started = Arc::new(Notify::new());
        container
            .download_manager
            .register_engine(Arc::new(ShutdownFlushEngine {
                segment_path: segment_path.clone(),
                started: started.clone(),
            }));
        let mut events = container.download_manager.subscribe();
        let download_id = container
            .download_manager
            .start_download(
                DownloadConfig::new(
                    "https://example.invalid/live.flv",
                    output_dir.clone(),
                    SHUTDOWN_STREAMER_ID,
                    "Shutdown tracer",
                    SHUTDOWN_SESSION_ID,
                )
                .with_protocol(DownloadProtocol::Flv),
                Some("ffmpeg".to_string()),
                false,
            )
            .await
            .expect("shutdown tracer download should start");
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .expect("shutdown tracer engine should open its segment");
        assert_eq!(container.download_manager.active_count(), 1);

        // A collection that ended earlier in the run reaches `shutdown_until`
        // as a `runtime_failures` entry. Only `shutdown_failures` may make
        // `shutdown_with_grace_period` return `Err`.
        danmu_service.seed_runtime_failure_for_test(
            "danmu collection earlier-session failed: websocket connect timed out".to_string(),
        );

        tokio::time::timeout(
            Duration::from_secs(10),
            container.shutdown_with_grace_period(Duration::from_secs(10)),
        )
        .await
        .expect("service container shutdown should complete within its deadline")
        .expect("service container should shut down gracefully");

        assert_eq!(container.download_manager.active_count(), 0);
        assert!(container.pool.is_closed());
        assert!(container.write_pool.is_closed());
        assert_eq!(
            tokio::fs::read(&segment_path)
                .await
                .expect("finalized segment should exist"),
            EXPECTED_SEGMENT_BYTES
        );
        let xml = tokio::fs::read_to_string(&danmu_path)
            .await
            .expect("finalized danmu segment should exist");
        assert!(xml.trim_end().ends_with("</i>"));
        let renamed_segment_path = output_dir.join("segment-0-renamed.flv");
        tokio::fs::rename(&segment_path, &renamed_segment_path)
            .await
            .expect("finalized segment should be renamable after shutdown");
        tokio::fs::rename(&renamed_segment_path, &segment_path)
            .await
            .expect("renamed segment should move back to its persisted path");

        let mut segment_completed_positions = Vec::new();
        let mut terminal_positions = Vec::new();
        let mut event_position = 0;
        loop {
            match events.try_recv() {
                Ok(DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentCompleted {
                    download_id: event_download_id,
                    session_id,
                    segment_path: event_segment_path,
                    segment_index,
                    ..
                })) => {
                    assert_eq!(event_download_id, download_id);
                    assert_eq!(session_id, SHUTDOWN_SESSION_ID);
                    assert_eq!(event_segment_path, segment_path_string);
                    assert_eq!(segment_index, 0);
                    segment_completed_positions.push(event_position);
                }
                Ok(DownloadManagerEvent::Terminal(DownloadTerminalEvent::Cancelled {
                    download_id: event_download_id,
                    session_id,
                    cause,
                    ..
                })) => {
                    assert_eq!(event_download_id, download_id);
                    assert_eq!(session_id, SHUTDOWN_SESSION_ID);
                    assert_eq!(cause, DownloadStopCause::Shutdown);
                    terminal_positions.push(event_position);
                }
                Ok(DownloadManagerEvent::Terminal(terminal)) => {
                    panic!("unexpected shutdown tracer terminal event: {terminal:?}");
                }
                Ok(DownloadManagerEvent::Progress(_)) => {}
                Err(
                    tokio::sync::broadcast::error::TryRecvError::Empty
                    | tokio::sync::broadcast::error::TryRecvError::Closed,
                ) => break,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                    panic!("shutdown tracer event receiver lagged by {skipped}");
                }
            }
            event_position += 1;
        }
        assert_eq!(segment_completed_positions.len(), 1);
        assert_eq!(terminal_positions.len(), 1);
        assert!(segment_completed_positions[0] < terminal_positions[0]);

        let (reopened_pool, reopened_write_pool) =
            crate::database::init_database_pools(&database_url)
                .await
                .expect("closed shutdown tracer database should reopen");
        let session_repo =
            SqlxSessionRepository::new(reopened_pool.clone(), reopened_write_pool.clone());

        let outputs = session_repo
            .get_media_outputs_for_session(SHUTDOWN_SESSION_ID)
            .await
            .expect("shutdown tracer outputs should load");
        assert_eq!(outputs.len(), 2);
        let video_output = outputs
            .iter()
            .find(|output| output.file_type == "VIDEO")
            .expect("video output should persist");
        assert_eq!(video_output.file_path, segment_path_string);
        assert_eq!(video_output.size_bytes, expected_size);
        let danmu_output = outputs
            .iter()
            .find(|output| output.file_type == "DANMU_XML")
            .expect("danmu XML output should persist");
        assert_eq!(
            danmu_output.file_path,
            danmu_path.to_string_lossy().into_owned()
        );
        assert!(danmu_output.size_bytes > 0);

        let segments = session_repo
            .list_session_segments_for_session(SHUTDOWN_SESSION_ID, 10)
            .await
            .expect("shutdown tracer segments should load");
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].segment_index, 0);
        assert_eq!(segments[0].file_path, segment_path_string);
        assert_eq!(segments[0].size_bytes, expected_size);
        let next_segment_index = session_repo
            .next_session_segment_index(SHUTDOWN_SESSION_ID)
            .await
            .expect("next segment index should load");
        assert_eq!(next_segment_index, 1);
        let dag_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM dag_execution \
             WHERE session_id = ? AND dag_definition LIKE '%Shutdown Paired Segment Pipeline%'",
        )
        .bind(SHUTDOWN_SESSION_ID)
        .fetch_one(&reopened_pool)
        .await
        .expect("shutdown tracer DAG count should load");
        assert_eq!(
            dag_count, 1,
            "the final paired-segment DAG must be created once"
        );
        let all_dag_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM dag_execution WHERE session_id = ?")
                .bind(SHUTDOWN_SESSION_ID)
                .fetch_one(&reopened_pool)
                .await
                .expect("shutdown tracer total DAG count should load");
        assert_eq!(
            all_dag_count, 1,
            "shutdown must not duplicate the paired-segment DAG"
        );
        let dag_statuses: Vec<String> = sqlx::query_scalar(
            "SELECT status FROM dag_execution WHERE session_id = ? ORDER BY created_at",
        )
        .bind(SHUTDOWN_SESSION_ID)
        .fetch_all(&reopened_pool)
        .await
        .expect("shutdown tracer DAG statuses should load");
        assert_eq!(dag_statuses, vec!["COMPLETED"]);
        let job_statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM job WHERE session_id = ? ORDER BY created_at")
                .bind(SHUTDOWN_SESSION_ID)
                .fetch_all(&reopened_pool)
                .await
                .expect("shutdown tracer job statuses should load");
        assert_eq!(
            job_statuses,
            vec!["COMPLETED"],
            "the configured paired-segment job must execute exactly once"
        );

        let active_session = session_repo
            .get_active_session_for_streamer(SHUTDOWN_STREAMER_ID)
            .await
            .expect("active shutdown tracer session should load")
            .expect("shutdown should preserve the active session");
        assert_eq!(active_session.id, SHUTDOWN_SESSION_ID);
        assert!(active_session.end_time.is_none());

        let resumed_output_dir = output_dir.to_string_lossy().into_owned();
        let updated = sqlx::query(
            "UPDATE global_config \
             SET record_danmu = FALSE, default_download_engine = 'ffmpeg', output_folder = ? \
             WHERE id = 'global-configuration'",
        )
        .bind(&resumed_output_dir)
        .execute(&reopened_write_pool)
        .await
        .expect("resume probe config should update");
        assert_eq!(updated.rows_affected(), 1);

        let resumed_container = super::ServiceContainer::with_full_config(
            reopened_pool.clone(),
            reopened_write_pool.clone(),
            super::ServiceContainerConfig {
                cache_ttl: Duration::from_secs(60),
                event_capacity: 8,
                download_config: DownloadManagerConfig::default(),
                pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
                api_config: crate::api::server::ApiServerConfig::default(),
            },
        )
        .await
        .expect("resumed service container should initialize");
        resumed_container
            .streamer_manager
            .hydrate()
            .await
            .expect("resumed streamer metadata should hydrate");
        resumed_container.setup_download_event_subscriptions();
        resumed_container.setup_session_lifecycle_subscriptions();

        let resumed_segment_path = output_dir.join("segment-1.flv");
        let resumed_started = Arc::new(Notify::new());
        resumed_container
            .download_manager
            .register_engine(Arc::new(ShutdownFlushEngine {
                segment_path: resumed_segment_path,
                started: resumed_started.clone(),
            }));
        let mut resumed_events = resumed_container.download_manager.subscribe();
        let resume_streams = vec![
            platforms_parser::media::StreamInfo::builder(
                "https://example.invalid/resumed.flv",
                platforms_parser::media::StreamFormat::Flv,
                platforms_parser::media::formats::MediaFormat::Flv,
            )
            .build(),
        ];
        let live_args = |now| crate::session::LiveDetectedArgs {
            streamer_id: SHUTDOWN_STREAMER_ID,
            streamer_name: "Shutdown tracer",
            streamer_url: "https://example.com/shutdown-tracer",
            current_avatar: None,
            new_avatar: None,
            title: "Resumed shutdown tracer",
            category: None,
            streams: &resume_streams,
            media_headers: None,
            media_extras: None,
            now,
        };

        let hydrated = resumed_container
            .session_lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .expect("active persisted session should hydrate into the lifecycle");
        assert_eq!(hydrated.session_id(), SHUTDOWN_SESSION_ID);
        resumed_container
            .session_lifecycle
            .on_download_terminal(&DownloadTerminalEvent::Completed {
                download_id: "pre-restart-download".to_string(),
                streamer_id: SHUTDOWN_STREAMER_ID.to_string(),
                streamer_name: "Shutdown tracer".to_string(),
                session_id: SHUTDOWN_SESSION_ID.to_string(),
                total_bytes: expected_size as u64,
                total_duration_secs: 1.0,
                total_segments: 1,
                file_path: Some(segment_path_string.clone()),
                engine_signal: EngineEndSignal::CleanDisconnect,
                stop_cause: None,
            })
            .await
            .expect("clean disconnect should enter hysteresis");
        let resumed = resumed_container
            .session_lifecycle
            .on_live_detected(live_args(Utc::now()))
            .await
            .expect("hysteresis resume should publish the production restart payload");
        assert_eq!(resumed.session_id(), SHUTDOWN_SESSION_ID);
        tokio::time::timeout(Duration::from_secs(5), resumed_started.notified())
            .await
            .expect("production resume pipeline should open its segment");
        let resumed_index = loop {
            let event = tokio::time::timeout(Duration::from_secs(5), resumed_events.recv())
                .await
                .expect("resumed segment start should arrive")
                .expect("resumed event channel should remain open");
            if let DownloadManagerEvent::Progress(DownloadProgressEvent::SegmentStarted {
                segment_index,
                ..
            }) = event
            {
                break segment_index;
            }
        };
        assert_eq!(resumed_index, 1);
        resumed_container
            .shutdown_with_grace_period(Duration::from_secs(10))
            .await
            .expect("resumed service container should shut down cleanly");
        assert!(reopened_pool.is_closed());
        assert!(reopened_write_pool.is_closed());
    }

    /// The phased drain waits for containment, so a task that ignores the
    /// cancellation token holds `shutdown_with_grace_period` open forever.
    /// `shutdown_with_hard_cap` must stop waiting, abort the task, and report
    /// what it aborted instead of leaving an embedder wedged.
    #[tokio::test]
    async fn hard_cap_aborts_supervised_work_that_never_settles() {
        let pool = migrated_test_pool().await;
        let container = super::ServiceContainer::with_config(
            pool.clone(),
            pool.clone(),
            Duration::from_secs(60),
            8,
        )
        .await
        .expect("service container should initialize");

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        assert!(container.task_supervisor.spawn("wedged", async move {
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        }));
        started_rx.await.expect("wedged task should start");

        let hard_cap = Duration::from_millis(200);
        let started_at = std::time::Instant::now();
        let error = container
            .shutdown_with_hard_cap(Duration::from_millis(50), hard_cap)
            .await
            .expect_err("the wedged task must keep the drain from completing");
        let elapsed = started_at.elapsed();

        assert!(
            elapsed < hard_cap + Duration::from_millis(250),
            "aborting must stay inside the hard cap (allowing scheduler jitter): {elapsed:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("force deadline") && message.contains("1 background task(s)"),
            "the error must name the work that was aborted: {message}"
        );
        // Producer quiescence was never proven, so the pools stay open exactly
        // as in `shutdown_with_grace_period`; the caller exits the process.
        assert!(!pool.is_closed());
    }

    #[tokio::test]
    async fn hard_cap_shutdown_under_the_cap_closes_pools() {
        let pool = migrated_test_pool().await;
        let container = super::ServiceContainer::with_config(
            pool.clone(),
            pool.clone(),
            Duration::from_secs(60),
            8,
        )
        .await
        .expect("service container should initialize");
        container.setup_download_event_subscriptions();
        container.setup_session_lifecycle_subscriptions();

        container
            .shutdown_with_hard_cap(Duration::from_millis(500), Duration::from_secs(5))
            .await
            .expect("a quiescent container should shut down within the cap");

        assert!(pool.is_closed());
    }

    #[tokio::test]
    async fn broadcast_lag_is_recoverable_and_receiver_remains_usable() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel(1);
        assert!(sender.send(1).is_ok());
        assert!(sender.send(2).is_ok());

        let error = receiver.recv().await.expect_err("receiver should lag");
        assert!(broadcast_error_is_recoverable("test", error));
        assert_eq!(receiver.recv().await, Ok(2));
    }

    #[tokio::test]
    async fn closed_broadcast_channel_is_terminal() {
        let (sender, mut receiver) = tokio::sync::broadcast::channel::<u8>(1);
        drop(sender);

        let error = receiver.recv().await.expect_err("channel should be closed");
        assert!(!broadcast_error_is_recoverable("test", error));
    }

    #[tokio::test]
    async fn full_config_wires_credential_notifications() {
        let pool = migrated_test_pool().await;

        let container = super::ServiceContainer::with_full_config(
            pool.clone(),
            pool,
            super::ServiceContainerConfig {
                cache_ttl: std::time::Duration::from_secs(60),
                event_capacity: 8,
                download_config: crate::downloader::DownloadManagerConfig::default(),
                pipeline_config: crate::pipeline::PipelineManagerConfig::default(),
                api_config: crate::api::server::ApiServerConfig::default(),
            },
        )
        .await
        .expect("full service container should initialize");

        assert!(container.credential_service.has_notification_service());
        container.cancellation_token().cancel();
    }

    #[tokio::test]
    async fn standard_config_uses_the_unified_build_path() {
        let pool = migrated_test_pool().await;
        let container = super::ServiceContainer::with_config(
            pool.clone(),
            pool,
            std::time::Duration::from_secs(60),
            8,
        )
        .await
        .expect("standard service container should initialize");

        assert!(container.credential_service.has_notification_service());
        container.cancellation_token().cancel();
    }

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

    #[test]
    fn test_recovery_progress_requires_strong_signal() {
        assert!(!should_record_recovery_from_progress(&DownloadProgress {
            bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES - 1,
            speed_bytes_per_sec: 1024,
            ..DownloadProgress::default()
        }));

        assert!(should_record_recovery_from_progress(&DownloadProgress {
            bytes_downloaded: RECOVERY_PROGRESS_MIN_BYTES,
            speed_bytes_per_sec: 1024,
            ..DownloadProgress::default()
        }));

        assert!(should_record_recovery_from_progress(&DownloadProgress {
            segments_completed: 1,
            ..DownloadProgress::default()
        }));
    }

    // ========== Output-root gate recovery hook filter ==========

    /// The recovery hook filters streamers by a per-root prefix built from
    /// `set_infra_blocked`'s `last_error` format. The prefix must include
    /// the root path + a trailing space so a Degraded → Healthy transition
    /// on one root only resets streamers blocked on that root: `/rec`
    /// cannot match `/rec/huya` entries and vice versa.
    #[test]
    fn recovery_hook_prefix_discriminates_between_sibling_roots() {
        use crate::downloader::LAST_ERROR_GATE_PREFIX;
        use std::path::Path;

        let root_a = Path::new("/rec/huya");
        let root_b = Path::new("/rec/douyu");

        let marker_a = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root_a.display());
        let marker_b = format!("{} {} ", LAST_ERROR_GATE_PREFIX, root_b.display());

        // Realistic `last_error` values as written by set_infra_blocked.
        let le_a_not_found = format!(
            "{} {} (not_found)",
            LAST_ERROR_GATE_PREFIX,
            root_a.display()
        );
        let le_a_storage = format!(
            "{} {} (storage_full)",
            LAST_ERROR_GATE_PREFIX,
            root_a.display()
        );
        let le_b_not_found = format!(
            "{} {} (not_found)",
            LAST_ERROR_GATE_PREFIX,
            root_b.display()
        );
        let le_unrelated = "connection refused".to_string();

        // Root A marker must match root A entries regardless of io_kind.
        assert!(le_a_not_found.starts_with(&marker_a));
        assert!(le_a_storage.starts_with(&marker_a));
        // Root A marker must NOT match root B entries.
        assert!(!le_b_not_found.starts_with(&marker_a));
        // Root B marker must match its own entries.
        assert!(le_b_not_found.starts_with(&marker_b));
        // Neither marker should match unrelated errors.
        assert!(!le_unrelated.starts_with(&marker_a));
        assert!(!le_unrelated.starts_with(&marker_b));
    }

    /// Even more important: a shorter root marker must not accidentally
    /// match longer sibling roots that share its prefix. If the gate
    /// ever gets two roots where one is a prefix of the other (e.g. a
    /// user sets `RUST_SREC_OUTPUT_ROOTS=/rec` and `/rec/archive`), the
    /// `/rec` recovery must NOT reset streamers blocked on `/rec/archive`.
    /// The trailing space in the marker is what makes this safe.
    #[test]
    fn recovery_hook_prefix_is_safe_against_prefix_collisions() {
        use crate::downloader::LAST_ERROR_GATE_PREFIX;

        let short_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, "/rec");
        let long_entry = format!("{} /rec/archive (not_found)", LAST_ERROR_GATE_PREFIX);

        // Without the trailing space, this would match. With it, it doesn't.
        assert!(!long_entry.starts_with(&short_marker));

        // Sanity: the long root's own marker matches.
        let long_marker = format!("{} {} ", LAST_ERROR_GATE_PREFIX, "/rec/archive");
        assert!(long_entry.starts_with(&long_marker));
    }

    // ========== static_root_prefix (startup probe config discovery) ==========

    #[test]
    fn static_root_prefix_typical_rust_srec_template() {
        // The default rust-srec template uses {platform}/{streamer}/%Y%m%d.
        // Everything after `/rec/` is dynamic so the prefix is `/rec/`.
        assert_eq!(
            super::static_root_prefix("/rec/{platform}/{streamer}/%Y%m%d"),
            Some("/rec/".to_string())
        );
    }

    #[test]
    fn static_root_prefix_strftime_only() {
        // No `{...}` variables — only strftime placeholders. Prefix is
        // everything before the first `%`.
        assert_eq!(
            super::static_root_prefix("/rec/recordings/%Y-%m-%d"),
            Some("/rec/recordings/".to_string())
        );
    }

    #[test]
    fn static_root_prefix_static_template_no_placeholders() {
        // Literal path. Whole string is the "prefix". Still trims to the
        // last slash to keep the result a complete directory path.
        assert_eq!(
            super::static_root_prefix("/app/output"),
            Some("/app/".to_string())
        );
        // If it already ends with a slash, preserve it.
        assert_eq!(
            super::static_root_prefix("/app/output/"),
            Some("/app/output/".to_string())
        );
    }

    #[test]
    fn static_root_prefix_partial_directory_name_rejected() {
        // Template interpolates into the middle of a directory name
        // (`/recordings-{streamer}/...`). The prefix `/recordings-` is
        // not a complete directory — the last slash is at position 0, so
        // the result is just `/`, which we reject as too broad.
        assert_eq!(
            super::static_root_prefix("/recordings-{streamer}/files"),
            None
        );
    }

    #[test]
    fn static_root_prefix_no_leading_slash_rejected() {
        // Relative template (no root `/`). Can't produce a probe key.
        assert_eq!(super::static_root_prefix("{streamer}/files"), None);
        assert_eq!(super::static_root_prefix("recordings/{streamer}"), None);
    }

    #[test]
    fn static_root_prefix_empty_template() {
        assert_eq!(super::static_root_prefix(""), None);
    }

    #[test]
    fn static_root_prefix_multi_level_static_prefix() {
        // Deep static prefix before the first placeholder.
        assert_eq!(
            super::static_root_prefix("/mnt/storage/recordings/{platform}/{streamer}"),
            Some("/mnt/storage/recordings/".to_string())
        );
    }
}
