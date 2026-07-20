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
use crate::danmu::{DanmuEvent, DanmuService, service::DanmuServiceConfig};
use crate::database::maintenance::MaintenanceScheduler;
use crate::database::repositories::{NotificationRepository, SessionRepository};
use crate::database::repositories::{
    config::SqlxConfigRepository, filter::SqlxFilterRepository, session::SqlxSessionRepository,
    streamer::SqlxStreamerRepository,
};
use crate::domain::{Priority, StreamerState};
use crate::downloader::{
    DownloadConfig, DownloadManager, DownloadManagerConfig, DownloadProtocol,
    LAST_ERROR_GATE_PREFIX, OutputRootGate, RecoveryHook, engine::DownloadProgress,
};
use crate::logging::LoggingConfig;
use crate::metrics::{HealthChecker, MetricsCollector, PrometheusExporter};
use crate::monitor::{MonitorEvent, MonitorEventBroadcaster, StreamMonitor};
use crate::notification::NotificationService;
use crate::notification::web_push::WebPushService;
use crate::pipeline::{PipelineEvent, PipelineManager, PipelineManagerConfig};
use crate::scheduler::Scheduler;
use crate::services::session_cancels::SessionCancelTokens;
use crate::streamer::StreamerManager;
use crate::utils::filename::sanitize_filename;
use pipeline_common::expand_path_template;

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
/// transition. Runs inside a `tokio::spawn`'d task (see gate implementation),
/// so blocking inside the closure is OK.
fn build_output_root_gate_recovery_hook<R>(
    streamer_manager: Arc<StreamerManager<R>>,
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

        // The hook is called inside a tokio task that the gate spawned on
        // its own runtime, so we can safely spawn another task here for
        // the per-streamer async DB writes. We fire them all in parallel
        // because a single slow write shouldn't hold up fleet recovery.
        let sm = streamer_manager.clone();
        tokio::spawn(async move {
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
    let raw = match std::env::var("RUST_SREC_OUTPUT_ROOTS") {
        Ok(v) => v,
        Err(_) => return Vec::new(),
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

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);

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
    /// Session repository shared by monitor, pipeline, danmu, and download startup.
    pub session_repository: Arc<SqlxSessionRepository>,
    /// Output-root write gate (#508). Shared by the download manager for
    /// pre-start checks + runtime ENOSPC routing and by the health checker
    /// for aggregated `/health` reporting.
    pub output_root_gate: Arc<OutputRootGate>,
    /// GPU health monitor (#555). Empty when `nvidia-smi` is not available
    /// at startup; otherwise the background probe loop is owned by the
    /// container's cancellation token. Use [`std::sync::OnceLock::get`]
    /// to read; installation is owned by the private runtime initializer.
    pub gpu_health_monitor: std::sync::OnceLock<Arc<crate::metrics::GpuHealthMonitor>>,
    /// Pipeline manager.
    pub pipeline_manager: Arc<PipelineManager>,
    /// Monitor event broadcaster.
    pub monitor_event_broadcaster: MonitorEventBroadcaster,
    /// Single-owner session lifecycle service. Owns the in-memory session map,
    /// hard-ended suppression cache, and the `SessionTransition` broadcast
    /// channel consumed by pipeline/notification/API layers.
    pub session_lifecycle: Arc<crate::session::SessionLifecycle>,
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
    /// Live broadcaster for committed check-history rows. Cloned into the
    /// downloads WS route so per-streamer subscribers see new bars appear
    /// without polling. Same fan-out pattern as
    /// [`crate::downloader::DownloadManager::subscribe`].
    pub check_history_broadcaster: crate::monitor::CheckHistoryBroadcaster,
    /// Per-session cancellation tokens. The
    /// [`MonitorEvent::StreamerLive`] handler spawns a download
    /// pipeline as a tokio task; the matching
    /// [`MonitorEvent::StreamerOffline`] (and any other terminal
    /// monitor event for that session) cancels its token so a queued
    /// pipeline can bail without spinning up an engine for an
    /// already-offline streamer.
    pub session_cancels: Arc<SessionCancelTokens>,
    /// Per-streamer "a download pipeline is in flight for this
    /// streamer right now" reservation set. Held from the start of
    /// `run_live_download_pipeline` (before preflight) through the
    /// final step (after danmu start). Protects against two concurrent
    /// `StreamerLive` events spawning duplicate pipelines for the
    /// same streamer in the window between
    /// [`crate::downloader::DownloadManager::has_active_download`]
    /// returning `false` and
    /// [`crate::downloader::DownloadManager::start_with_slot`]
    /// inserting into `active_downloads`.
    pub pending_pipelines: Arc<DashMap<String, ()>>,
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
    /// Handle to the database maintenance task for graceful shutdown.
    maintenance_task_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
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

    tokio::spawn(crate::monitor::check_history_writer::run(
        repo,
        rx,
        Some(broadcaster.clone()),
        cancellation_token.child_token(),
    ));
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
            Self::refresh_metadata_offline_check(
                &self.streamer_manager,
                &self.config_service,
                &metadata.id,
            )
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

        // Detect and install the GPU health monitor (#555) BEFORE wiring
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

        // Wire `SessionTransition::Started { from_hysteresis: true }` →
        // synthetic `MonitorEvent::StreamerLive` so a hysteresis resume
        // restarts the download. See `setup_resume_download_subscriber`.
        self.setup_resume_download_subscriber();

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

        // One-shot output-root write gate startup probe (#508). Discovers
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
        *self.maintenance_task_handle.lock().await = Some(maintenance_handle);
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
        session_lifecycle: &Arc<crate::session::SessionLifecycle>,
        streamer_manager: &Arc<StreamerManager<SqlxStreamerRepository>>,
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

        // 3. End active streaming session via the lifecycle FSM (best-effort).
        //
        // Replaces the old `stream_monitor.force_end_active_session`, which
        // wrote `live_sessions.end_time` directly via SQL but never touched
        // `SessionLifecycle`'s in-memory state. That divergence caused the
        // disable/re-enable bug: re-enable found the stale Hysteresis handle
        // and silently restarted a download under an already-ended session_id.
        let streamer_name = streamer_manager
            .get_streamer(streamer_id)
            .map(|m| m.name.clone())
            .unwrap_or_default();
        if let Err(e) = session_lifecycle
            .end_for_disable(streamer_id, &streamer_name)
            .await
        {
            warn!(
                "Failed to end session via lifecycle for disabled streamer {}: {}",
                streamer_id, e
            );
        }

        // Note: Actor removal is handled by the Scheduler's own config event handler.
        // We don't do it here because scheduler.run() holds the RwLock write lock forever.
    }

    /// Refresh `effective_offline_check_*` on `StreamerMetadata` from the
    /// freshly resolved merged config. The actor's `StreamerConfig` and the
    /// `SessionLifecycle` hysteresis backstop both read from these cached
    /// fields, so calling this keeps both consumers in lockstep with the
    /// 4-layer config hierarchy.
    async fn refresh_metadata_offline_check(
        streamer_manager: &Arc<StreamerManager<SqlxStreamerRepository>>,
        config_service: &Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        streamer_id: &str,
    ) {
        match config_service.get_config_for_streamer(streamer_id).await {
            Ok(merged) => streamer_manager.apply_resolved_config(streamer_id, &merged),
            Err(e) => debug!("Skipping offline_check refresh for {}: {}", streamer_id, e),
        }
    }

    /// Handle monitor events to trigger downloads and danmu collection.
    #[allow(clippy::too_many_arguments)]
    async fn handle_monitor_event(
        download_manager: &Arc<DownloadManager>,
        streamer_manager: &Arc<StreamerManager<SqlxStreamerRepository>>,
        config_service: &Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        danmu_service: &Arc<DanmuService>,
        stream_monitor: &Arc<
            StreamMonitor<
                SqlxStreamerRepository,
                SqlxFilterRepository,
                SqlxSessionRepository,
                SqlxConfigRepository,
            >,
        >,
        session_repository: &Arc<SqlxSessionRepository>,
        session_cancels: &Arc<SessionCancelTokens>,
        pending_pipelines: &Arc<DashMap<String, ()>>,
        event: MonitorEvent,
        from_hysteresis_resume: bool,
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

                // Hand off the per-streamer pipeline to a spawned task.
                // Without this, an `acquire_slot` parked on a saturated
                // queue would block the monitor-event loop and stall
                // every other streamer's events (live, offline, etc.).
                // Each pipeline owns a per-session cancellation token
                // that StreamerOffline fires to abort cleanly.
                let download_manager = download_manager.clone();
                let streamer_manager = streamer_manager.clone();
                let config_service = config_service.clone();
                let danmu_service = danmu_service.clone();
                let stream_monitor = stream_monitor.clone();
                let session_repository = session_repository.clone();
                let session_cancels = session_cancels.clone();
                let pending_pipelines = pending_pipelines.clone();
                tokio::spawn(async move {
                    run_live_download_pipeline(
                        download_manager,
                        streamer_manager,
                        config_service,
                        danmu_service,
                        stream_monitor,
                        session_repository,
                        session_cancels,
                        pending_pipelines,
                        StreamerLivePayload {
                            streamer_id,
                            session_id,
                            streamer_name,
                            title,
                            streams,
                            streamer_url,
                            media_headers,
                            media_extras,
                        },
                        from_hysteresis_resume,
                    )
                    .await;
                });
            }
            MonitorEvent::StreamerOffline {
                streamer_id,
                streamer_name,
                session_id,
                ..
            } => {
                info!("Streamer {} ({}) went offline", streamer_name, streamer_id);

                // Cancel any queued/in-flight pipeline for this
                // session. A pipeline parked on `acquire_slot` wakes
                // up and bails without spinning up an engine for an
                // already-offline streamer; one already in
                // `start_with_slot` is past the cancel point and
                // proceeds to run normally — the existing
                // stop-download path below handles tearing it down.
                if let Some(sid) = session_id.as_deref() {
                    session_cancels.cancel(sid);
                }

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

                // Cancel any queued pipeline for this streamer so a
                // download parked on `acquire_slot` doesn't acquire +
                // start a recording the moment a slot frees, even
                // though the schedule window has just closed.
                for entry in download_manager.snapshot_pending() {
                    if entry.streamer_id == streamer_id {
                        session_cancels.cancel(&entry.session_id);
                    }
                }

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

        info!("Stopping maintenance scheduler...");
        if let Some(handle) = self.maintenance_task_handle.lock().await.take() {
            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => info!("Maintenance scheduler stopped gracefully"),
                Ok(Err(error)) => warn!(error = %error, "Maintenance scheduler task panicked"),
                Err(_) => warn!("Maintenance scheduler shutdown timed out"),
            }
        }

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
        // Shut down the queue first so any pipelines parked on
        // `acquire_slot` wake up and return `ShuttingDown` instead of
        // racing with `stop_all` to grab a slot from a just-released
        // active download.
        self.download_manager.shutdown_queue();
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

/// Owned payload carrying the per-streamer data needed by
/// [`run_live_download_pipeline`]. Mirrors the relevant fields of
/// [`MonitorEvent::StreamerLive`] but is decoupled from the enum so
/// the spawned task can capture exactly what it needs.
struct StreamerLivePayload {
    streamer_id: String,
    session_id: String,
    streamer_name: String,
    title: String,
    streams: Vec<crate::monitor::StreamInfo>,
    streamer_url: String,
    media_headers: Option<std::collections::HashMap<String, String>>,
    media_extras: Option<std::collections::HashMap<String, String>>,
}

/// Per-streamer download pipeline.
///
/// Runs in a `tokio::spawn`'d task per `StreamerLive` event. Walks the
/// split startup flow:
///
/// 1. **Dedup / pre-checks** — bail if the streamer is already
///    downloading, no longer active, disabled, or has no streams.
/// 2. **Preflight** — engine resolution, circuit breaker, output-root
///    write gate, `prepare_output_dir`. Failures emit
///    `DownloadRejected` events directly (the manager handles that)
///    and the pipeline exits without consuming a queue slot.
/// 3. **Acquire slot** — parks on the priority-aware download queue,
///    emitting `DownloadQueued` if it had to wait. Honours the
///    per-session [`CancellationToken`] so a `StreamerOffline`
///    arriving mid-wait aborts cleanly with no engine startup.
/// 4. **Freshness re-check** — when the wait was non-trivial
///    (`waited_ms > queue_freshness_threshold_ms()`), refetches the
///    live state via `StreamMonitor::check_streamer`; on
///    Offline / Filtered / Error, drops the slot and exits without
///    starting the engine. Below the threshold, only does a cheap
///    state re-check via the streamer manager.
/// 5. **Start engine** — calls `start_with_slot`, which moves the
///    slot into the active downloads map and emits `DownloadStarted`.
/// 6. **Danmu** — gated on download success, so danmu collection
///    never opens a platform connection for a stream that's still
///    queued or got aborted.
#[allow(clippy::too_many_arguments)]
async fn run_live_download_pipeline(
    download_manager: Arc<DownloadManager>,
    streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    danmu_service: Arc<DanmuService>,
    stream_monitor: Arc<
        StreamMonitor<
            SqlxStreamerRepository,
            SqlxFilterRepository,
            SqlxSessionRepository,
            SqlxConfigRepository,
        >,
    >,
    session_repository: Arc<SqlxSessionRepository>,
    session_cancels: Arc<SessionCancelTokens>,
    pending_pipelines: Arc<DashMap<String, ()>>,
    payload: StreamerLivePayload,
    // `true` when called for a session that just resumed out of
    // hysteresis. The resume-download subscriber synthesises a
    // `MonitorEvent::StreamerLive` from the lifecycle's
    // `SessionTransition::Started { from_hysteresis: true, .. }` and routes
    // it through the same pipeline as a fresh-live event; this flag
    // tells the short-queue-wait branch to trust the lifecycle signal
    // instead of re-reading the streamer-manager cache.
    from_hysteresis_resume: bool,
) {
    use crate::downloader::{AcquireRequest, PreflightRequest, Priority as QueuePriority};

    let StreamerLivePayload {
        streamer_id,
        session_id,
        streamer_name,
        title,
        mut streams,
        streamer_url,
        mut media_headers,
        mut media_extras,
    } = payload;

    // Per-streamer reservation. The earliest atomic point we can grab
    // — before any await, before preflight, before queue acquire —
    // covers the window where two concurrent `StreamerLive` events
    // could otherwise both pass `has_active_download` and both
    // proceed to `start_with_slot`. Hysteresis-resume synthetic
    // events for the same streamer also funnel through here. The
    // queue's session_id dedup catches the rarer case of duplicate
    // session_ids; this catches the common case of duplicate
    // streamer_ids racing.
    if pending_pipelines.insert(streamer_id.clone(), ()).is_some() {
        debug!(
            "Skipping StreamerLive for {} — pipeline already in flight",
            streamer_id
        );
        return;
    }
    struct PipelineReservationGuard<'a> {
        map: &'a Arc<DashMap<String, ()>>,
        streamer_id: &'a str,
    }
    impl<'a> Drop for PipelineReservationGuard<'a> {
        fn drop(&mut self) {
            self.map.remove(self.streamer_id);
        }
    }
    let _pipeline_guard = PipelineReservationGuard {
        map: &pending_pipelines,
        streamer_id: &streamer_id,
    };

    // Per-session cancellation token. The registration handle clears
    // itself on exit, but only if it still owns the same token; this
    // keeps cleanup local to the cancellation registry instead of
    // spreading token lifetime rules through the pipeline.
    let cancel_handle = session_cancels.register(&session_id);
    let cancel = cancel_handle.token();

    // Dedup and pre-checks.
    if download_manager.has_active_download(&streamer_id) {
        debug!("Download already active for {}", streamer_id);
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

    let streamer_metadata = streamer_manager.get_streamer(&streamer_id);
    if let Some(metadata) = &streamer_metadata {
        if !metadata.is_active() {
            info!(
                "Ignoring StreamerLive for inactive streamer {} (state: {})",
                streamer_id, metadata.state
            );
            return;
        }
        if metadata.is_disabled() {
            // Returning silently here would strand the pipeline: the session
            // lifecycle has already committed this session to Recording (via
            // `start_or_resume` or `resume_from_hysteresis`), and with no
            // download there is never a DownloadStarted/DownloadEnded to move
            // the streamer actor out of Live — the `(Live, Live)` arm of
            // `HysteresisState::should_emit` then suppresses every future
            // check and recording stays dead for the rest of the broadcast.
            // Emit the same Rejected terminal `preflight` uses so the session
            // lifecycle closes the session (`TerminalCause::Rejected` is an
            // authoritative end) and the actor re-checks once the backoff
            // expires.
            let retry_after_secs = metadata
                .remaining_backoff_std()
                .map_or(0, |d| d.as_secs())
                .saturating_add(2);
            info!(
                streamer_id = %streamer_id,
                streamer_name = %streamer_name,
                disabled_until = ?metadata.disabled_until,
                retry_after_secs,
                "Ignoring StreamerLive while temporarily disabled"
            );
            download_manager.emit_rejected(
                streamer_id.clone(),
                streamer_name.clone(),
                session_id.clone(),
                "streamer temporarily disabled (error backoff)".to_string(),
                Some(retry_after_secs),
                crate::downloader::DownloadRejectedKind::StreamerBackoff,
            );
            return;
        }
    }

    if streams.is_empty() {
        warn!(
            "Streamer {} has no streams available, cannot start download",
            streamer_id
        );
        return;
    }

    let is_high_priority = streamer_metadata
        .as_ref()
        .map(|s| s.priority == Priority::High)
        .unwrap_or(false);
    // Load merged config for this streamer.
    let merged_config = match config_service.get_config_for_streamer(&streamer_id).await {
        Ok(config) => config,
        Err(e) => {
            warn!(
                "Failed to load config for streamer {}, using defaults: {}",
                streamer_id, e
            );
            Arc::new(crate::config::MergedConfig::builder().build())
        }
    };

    // Sanitize names for filename usage.
    let sanitized_streamer = sanitize_filename(&streamer_name);
    let sanitized_title = sanitize_filename(&title);
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

    // Preflight.
    let preflight_req = PreflightRequest {
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        session_id: session_id.clone(),
        output_dir: output_dir.clone().into(),
        engine_id: Some(merged_config.download_engine.clone()),
        engines_override: merged_config.engines_override.clone(),
    };
    let engine = match download_manager.preflight(preflight_req).await {
        Ok(e) => e,
        Err(e) => {
            warn!("Preflight failed for streamer {}: {}", streamer_id, e);
            return; // Manager has already emitted DownloadRejected if applicable.
        }
    };
    let engine_type = engine.engine_type;

    // Honour cancellation that fired between preflight and slot acquire.
    if cancel.is_cancelled() {
        debug!("Streamer {} cancelled before slot acquire", streamer_id);
        return;
    }

    // Acquire slot.
    let acquire_req = AcquireRequest {
        session_id: session_id.clone(),
        streamer_id: streamer_id.clone(),
        streamer_name: streamer_name.clone(),
        engine_type,
        priority: if is_high_priority {
            QueuePriority::High
        } else {
            QueuePriority::Normal
        },
    };
    let slot = match download_manager
        .acquire_slot(acquire_req, cancel.clone())
        .await
    {
        Ok(slot) => slot,
        Err(e) => {
            // Cancelled / duplicate session / shutdown are benign
            // exits. If a visible queued event fired, the manager has
            // already emitted the matching `DownloadDequeued`.
            debug!(
                "acquire_slot returned without a slot for streamer {}: {}",
                streamer_id, e
            );
            return;
        }
    };

    let waited_ms = slot.waited_ms();

    // Freshness re-check.
    if waited_ms > download_manager.queue_freshness_threshold_ms() {
        debug!(
            streamer_id = %streamer_id,
            waited_ms,
            "Queue wait exceeded freshness threshold; refetching live state"
        );
        // Re-fetch via the monitor's deduped, rate-limited check.
        let metadata_for_check = streamer_manager.get_streamer(&streamer_id);
        if let Some(meta) = metadata_for_check {
            match stream_monitor.check_streamer(&meta).await {
                Ok(crate::monitor::LiveStatus::Live {
                    streams: fresh_streams,
                    media_headers: fresh_headers,
                    media_extras: fresh_extras,
                    ..
                }) => {
                    if fresh_streams.is_empty() {
                        debug!(
                            streamer_id = %streamer_id,
                            "Refetch returned Live with no streams; aborting"
                        );
                        download_manager.emit_dequeued_for_slot(
                            &slot,
                            &streamer_id,
                            &streamer_name,
                        );
                        return;
                    }
                    // Replace BOTH the URLs and the associated
                    // headers/extras. On platforms whose signed
                    // URLs rotate together with required headers
                    // (e.g. Host overrides, signed referer),
                    // keeping the old headers with new URLs would
                    // 403 just as reliably as keeping the old
                    // URLs.
                    streams = fresh_streams;
                    media_headers = fresh_headers;
                    media_extras = fresh_extras;
                }
                Ok(_) => {
                    debug!(
                        streamer_id = %streamer_id,
                        "Streamer no longer live after queue wait; aborting"
                    );
                    download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
                    return;
                }
                Err(e) => {
                    warn!(
                        streamer_id = %streamer_id,
                        error = %e,
                        "Refetch failed; falling back to cached URLs"
                    );
                }
            }
        } else {
            debug!(
                streamer_id = %streamer_id,
                "Streamer metadata vanished during queue wait; aborting"
            );
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return;
        }
    } else if !from_hysteresis_resume {
        // Cheap re-check for the short-wait case: is the streamer
        // still in a state that permits a fresh recording? `Live`
        // specifically, NOT just `is_active()` — `OutOfSchedule`
        // counts as active in the metadata sense (the streamer is
        // still being monitored) but recording is not allowed.
        // Without this tighter check, a schedule window could close
        // mid-wait and we'd start an out-of-schedule recording.
        //
        // Skipped on hysteresis resume: the lifecycle writes
        // `state=LIVE` before broadcasting, but `StreamMonitor::handle_live`
        // reloads the streamer-manager cache only after the broadcast
        // returns, so this check can read a stale `NotLive`. Out-of-schedule
        // streamers never reach this code path because
        // `monitor::service::handle_live` runs only for `LiveStatus::Live`
        // (filtered events take a different branch), and any window that
        // closes mid-recording is caught later by the
        // `MonitorEvent::StateChanged { OutOfSchedule }` handler.
        let meta = streamer_manager.get_streamer(&streamer_id);
        let permits_start = meta
            .as_ref()
            .map(|m| m.state == StreamerState::Live && !m.is_disabled())
            .unwrap_or(false);
        if !permits_start {
            debug!(
                streamer_id = %streamer_id,
                state = ?meta.as_ref().map(|m| m.state),
                "Streamer no longer in LIVE state after short queue wait; aborting"
            );
            download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
            return;
        }
    }

    if cancel.is_cancelled() {
        debug!(
            "Streamer {} cancelled between freshness check and engine start",
            streamer_id
        );
        download_manager.emit_dequeued_for_slot(&slot, &streamer_id, &streamer_name);
        return;
    }

    // ── Build full DownloadConfig with possibly-refreshed URLs ──
    let best_stream = &streams[0];
    let stream_url_selected = best_stream.url.clone();
    let stream_format = best_stream.stream_format.as_str();
    let media_format = best_stream.media_format.as_str();
    let initial_segment_index = match session_repository
        .next_session_segment_index(&session_id)
        .await
    {
        Ok(index) => index,
        Err(e) => {
            warn!(
                session_id = %session_id,
                streamer_id = %streamer_id,
                error = %e,
                "Failed to load next persisted session segment index; starting from zero"
            );
            0
        }
    };

    let mut headers = media_headers.as_ref().cloned().unwrap_or_default();
    if let Some(extras) = best_stream.extras.as_ref() {
        if let Some(extra_headers) = extras.get("headers").and_then(|v| v.as_object()) {
            for (k, v) in extra_headers {
                if let Some(v) = v.as_str() {
                    headers.insert(k.clone(), v.to_string());
                }
            }
        }
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

    let mut config = DownloadConfig::new(
        stream_url_selected.clone(),
        output_dir.clone(),
        streamer_id.clone(),
        streamer_name.clone(),
        session_id.clone(),
    )
    .with_initial_segment_index(initial_segment_index)
    .with_filename_template(
        merged_config
            .output_filename_template
            .replace("{streamer}", &sanitized_streamer)
            .replace("{title}", &sanitized_title)
            .replace("{platform}", platform),
    )
    .with_output_format(&merged_config.output_file_format)
    .with_protocol(DownloadProtocol::from_format_label(stream_format))
    .with_max_segment_duration(merged_config.max_download_duration_secs as u64)
    .with_max_segment_size(merged_config.max_part_size_bytes as u64)
    .with_engines_override(merged_config.engines_override.clone());

    if let Some(ref cookies) = merged_config.cookies {
        debug!(
            "Applying cookies from merged config to download (length: {} chars)",
            cookies.len()
        );
        config = config.with_cookies(cookies);
    }

    let proxy_config = &merged_config.proxy_config;
    if proxy_config.enabled {
        if let Some(effective_proxy_url) = proxy_config.effective_url() {
            debug!(
                "Applying explicit proxy from merged config to download: {}",
                effective_proxy_url
            );
            config = config.with_proxy(effective_proxy_url);
        } else if proxy_config.use_system_proxy {
            debug!("Enabling system proxy for download");
            config = config.with_system_proxy(true);
        }
    }

    for (key, value) in headers {
        config = config.with_header(key, value);
    }

    info!(
        "Starting download for {} with stream URL: {} (stream_format: {}, media_format: {}, headers_needed: {}, output: {}, queue_wait_ms: {}, initial_segment_index: {})",
        streamer_name,
        stream_url_selected,
        stream_format,
        media_format,
        best_stream.is_headers_needed,
        merged_config.output_folder,
        waited_ms,
        initial_segment_index,
    );

    let cookies = merged_config.cookies.clone();

    // Start engine on the slot.
    let started = match download_manager.start_with_slot(slot, config, engine).await {
        Ok(download_id) => {
            info!(
                "Started download {} for streamer {} (priority: {})",
                download_id,
                streamer_id,
                if is_high_priority { "high" } else { "normal" }
            );
            true
        }
        Err(e) => {
            warn!(
                "Failed to start download for streamer {}: {}",
                streamer_id, e
            );
            false
        }
    };

    // Danmu.
    // Gated on the download having a real id. If `start_with_slot`
    // failed, the slot is already released by SlotGuard's drop and
    // there's no engine to interleave danmu with — opening a danmu
    // socket for a stream we're not recording would leak a platform
    // connection.
    if started && merged_config.record_danmu {
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

    // ========== Output-root gate recovery hook filter (P1 regression) ==========

    /// The recovery hook filters streamers by a per-root prefix built from
    /// `set_infra_blocked`'s `last_error` format. Earlier versions filtered
    /// on just `"output-root blocked:"`, which caused a Degraded → Healthy
    /// transition on root A to also reset streamers blocked on root B.
    /// This test locks in the fix: the prefix must include the root path +
    /// a trailing space, so `/rec` cannot match `/rec/huya` entries and
    /// vice versa.
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
