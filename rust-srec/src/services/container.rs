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
use crate::danmu::{DanmuService, service::DanmuServiceConfig};
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
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

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

struct ServiceContainerBuildOptions {
    cache_ttl: Duration,
    event_capacity: usize,
    download_config: DownloadManagerConfig,
    pipeline_config: PipelineManagerConfig,
    danmu_config: DanmuServiceConfig,
    api_config: ApiServerConfig,
}

impl ServiceContainerBuildOptions {
    fn standard(cache_ttl: Duration, event_capacity: usize) -> Self {
        Self {
            cache_ttl,
            event_capacity,
            download_config: DownloadManagerConfig::default(),
            pipeline_config: PipelineManagerConfig::default(),
            danmu_config: DanmuServiceConfig::default(),
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
    /// Required terminal-download receiver used for session state changes.
    download_terminal_receiver: parking_lot::Mutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::downloader::DownloadTerminalEvent>>,
    >,
    /// Single-owner session lifecycle service. Owns the in-memory session map,
    /// hard-ended suppression cache, and the `SessionTransition` broadcast
    /// channel consumed by pipeline/notification/API layers.
    pub(crate) session_lifecycle: Arc<crate::session::SessionLifecycle>,
    /// Required session-transition receiver used for runtime side effects.
    session_transition_receiver: parking_lot::Mutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::session::SessionTransition>>,
    >,
    /// Operational policy for required runtime events.
    runtime_coordinator: Arc<RuntimeCoordinator>,
    /// Danmu service.
    pub(crate) danmu_service: Arc<DanmuService>,
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

        // Populate effective_offline_check_* on the in-memory metadata cache
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

    /// Shutdown all services gracefully.
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_timeout(DEFAULT_SHUTDOWN_TIMEOUT).await
    }

    /// Shutdown all services gracefully with a custom timeout.
    pub(crate) async fn shutdown_with_timeout(&self, timeout: Duration) -> Result<()> {
        info!("Shutting down services (timeout: {:?})", timeout);
        let deadline = tokio::time::Instant::now() + timeout;

        // Stop accepting new work and signal all background tasks first.
        self.cancellation_token.cancel();
        self.stream_monitor.stop();
        self.download_manager.shutdown_queue();

        let service_shutdown = async {
            info!("Stopping notification service...");
            self.notification_service.stop().await;

            info!("Stopping danmu service...");
            self.danmu_service.shutdown().await;

            info!("Stopping download manager...");
            let stopped_downloads = self.download_manager.stop_all().await;
            info!(count = stopped_downloads.len(), "Stopped active downloads");

            info!("Stopping pipeline manager...");
            self.pipeline_manager.stop().await;
        };
        if tokio::time::timeout_at(deadline, service_shutdown)
            .await
            .is_err()
        {
            warn!("Service shutdown deadline exceeded");
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if !self.task_supervisor.shutdown(remaining).await {
            warn!("One or more background tasks required forced cancellation");
        }

        info!("Closing database pools...");
        let close_pools = async {
            tokio::join!(self.write_pool.close(), self.pool.close());
        };
        if tokio::time::timeout_at(deadline, close_pools)
            .await
            .is_err()
        {
            warn!("Database pool shutdown deadline exceeded");
        }

        info!("Services shut down");
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
    use super::{
        RECOVERY_PROGRESS_MIN_BYTES, broadcast_error_is_recoverable,
        should_end_stream_on_danmu_stream_closed, should_record_recovery_from_progress,
    };
    use crate::downloader::engine::DownloadProgress;

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
            std::time::Duration::from_secs(60),
            8,
            crate::downloader::DownloadManagerConfig::default(),
            crate::pipeline::PipelineManagerConfig::default(),
            crate::danmu::service::DanmuServiceConfig::default(),
            crate::api::server::ApiServerConfig::default(),
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
