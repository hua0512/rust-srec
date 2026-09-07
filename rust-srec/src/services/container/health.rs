use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use dashmap::DashMap;
use futures::StreamExt;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::ConfigService;
use crate::danmu::DanmuService;
use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};
use crate::downloader::{DownloadManager, OutputRootGate};
use crate::metrics::{
    ComponentHealth, DiskUsage, HealthChecker, HealthProbe, SystemMetricsSnapshot,
};
use crate::pipeline::PipelineManager;

use super::{ServiceContainer, sqlite_file_path_from_url, static_root_prefix};

/// Upper bound on `DiskSpaceProbe` registrations, so a deployment with many
/// per-streamer `output_folder` overrides can't fill the health snapshot
/// with hundreds of `disk:` components. Real deployments span a handful of
/// filesystems; roots beyond the limit are dropped with a warning.
const MAX_OUTPUT_ROOT_PROBES: usize = 16;
const MAX_CONCURRENT_OUTPUT_PROBES: usize = 4;
const MAX_CONCURRENT_OUTPUT_CONFIG_MERGES: usize = 16;

struct DatabaseProbe {
    pool: SqlitePool,
}

#[async_trait]
impl HealthProbe for DatabaseProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("database")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        if self.pool.is_closed() {
            ComponentHealth::unhealthy("database", "Connection pool is closed")
        } else {
            ComponentHealth::healthy("database")
        }
    }
}

struct DiskSpaceProbe {
    /// Component identifier rendered as `disk:{display_path}`. Pre-built
    /// once so [`HealthProbe::name`] doesn't allocate per call (the
    /// refresh loop reads it twice per tick).
    component_name: String,
    display_path: String,
    lookup_path: std::path::PathBuf,
    warning_threshold: f64,
    critical_threshold: f64,
}

impl DiskSpaceProbe {
    fn new(
        display_path: String,
        lookup_path: std::path::PathBuf,
        warning_threshold: f64,
        critical_threshold: f64,
    ) -> Self {
        Self {
            component_name: format!("disk:{}", display_path),
            display_path,
            lookup_path,
            warning_threshold,
            critical_threshold,
        }
    }
}

#[async_trait]
impl HealthProbe for DiskSpaceProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.component_name)
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn needs_disk_snapshot(&self) -> bool {
        true
    }

    async fn probe(&self, metrics: SystemMetricsSnapshot) -> ComponentHealth {
        match metrics.best_disk_for_path(&self.lookup_path) {
            // The capacity figures ride along on every status, healthy
            // included: `check_disk_space_with_thresholds` leaves `message`
            // empty below the warning threshold, so this is the only place
            // free space is reported while nothing is wrong.
            Some(disk) => HealthChecker::check_disk_space_with_thresholds(
                &self.display_path,
                disk.available_space,
                disk.total_space,
                self.warning_threshold,
                self.critical_threshold,
            )
            .with_disk(DiskUsage::new(
                &self.display_path,
                disk.mount_point.to_string_lossy(),
                disk.available_space,
                disk.total_space,
            )),
            None => ComponentHealth {
                name: self.component_name.clone(),
                status: crate::metrics::HealthStatus::Unknown,
                message: Some("Unable to resolve disk for path".to_string()),
                last_check: Some(chrono::Utc::now().to_rfc3339()),
                check_duration_ms: None,
                disk: None,
            },
        }
    }
}

struct OutputRootProbe {
    gate: Arc<OutputRootGate>,
}

#[async_trait]
impl HealthProbe for OutputRootProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("output-root")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        HealthChecker::check_output_root_gate(&self.gate)
    }
}

struct GpuProbe {
    monitor: Arc<crate::metrics::GpuHealthMonitor>,
}

#[async_trait]
impl HealthProbe for GpuProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("gpu")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        self.monitor.snapshot().health.clone()
    }
}

struct DownloadManagerProbe {
    download_manager: Arc<DownloadManager>,
}

#[async_trait]
impl HealthProbe for DownloadManagerProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("download_manager")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, metrics: SystemMetricsSnapshot) -> ComponentHealth {
        let active = self.download_manager.active_count();
        let total_slots = self.download_manager.total_concurrent_slots();
        let pending = self.download_manager.pending_count();

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

        if active >= total_slots && pending > 0 {
            return ComponentHealth::degraded(
                "download_manager",
                format!(
                    "Concurrency limit reached: {}/{} active, {} streamer(s) queued",
                    active, total_slots, pending
                ),
            );
        }

        let cpu_threshold = 85.0_f32;
        let mem_threshold = 90.0_f32;
        let utilization = active as f32 / total_slots as f32;

        if utilization >= 0.95
            && (metrics.cpu_usage >= cpu_threshold || metrics.memory_usage >= mem_threshold)
        {
            ComponentHealth::degraded(
                "download_manager",
                format!(
                    "Near capacity under resource pressure: active {}/{}, cpu {:.1}%, mem {:.1}%",
                    active, total_slots, metrics.cpu_usage, metrics.memory_usage
                ),
            )
        } else {
            ComponentHealth::healthy("download_manager")
        }
    }
}

struct PipelineManagerProbe {
    pipeline_manager: Arc<PipelineManager>,
}

#[async_trait]
impl HealthProbe for PipelineManagerProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("pipeline_manager")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        let depth = self.pipeline_manager.queue_depth();
        let status = self.pipeline_manager.queue_status();
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
    }
}

struct SchedulerProbe {
    cancellation_token: CancellationToken,
}

#[async_trait]
impl HealthProbe for SchedulerProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("scheduler")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(5)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        if self.cancellation_token.is_cancelled() {
            ComponentHealth::unhealthy("scheduler", "Scheduler has been cancelled")
        } else {
            ComponentHealth::healthy("scheduler")
        }
    }
}

/// Reports sessions that are still recording while their danmu collector is
/// gone.
///
/// `DanmuService` removes a session's entry as soon as its runner exits, for any
/// reason including an unrecoverable transport failure. A live download whose
/// resolved config has `record_danmu` set but which has no collector therefore
/// means chat capture ended early and the rest of the recording will have none —
/// a condition that is otherwise only visible as a single `warn!` at the moment
/// it happens.
struct DanmuServiceProbe {
    danmu_service: Arc<DanmuService>,
    download_manager: Arc<DownloadManager>,
    config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    danmu_link_down: Arc<DashMap<String, Instant>>,
}

impl DanmuServiceProbe {
    /// Downloads younger than this are ignored: `run_live_download_pipeline`
    /// starts the engine before `DanmuService::start_collection`, so a brand-new
    /// session legitimately has a download and no collector for a moment.
    const STARTUP_GRACE: chrono::Duration = chrono::Duration::seconds(60);

    /// A link must stay down this long before it is reported, so an ordinary
    /// reconnect between two provider cycles does not flap the health status.
    const OUTAGE_GRACE: Duration = Duration::from_secs(120);
}

#[async_trait]
impl HealthProbe for DanmuServiceProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed("danmu_service")
    }

    fn cadence(&self) -> Duration {
        Duration::from_secs(30)
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        let now = chrono::Utc::now();
        let mut stopped_early = Vec::new();

        for download in self.download_manager.get_active_downloads() {
            if download.session_id.is_empty()
                || now - download.started_at < Self::STARTUP_GRACE
                || self.danmu_service.is_collecting(&download.session_id)
            {
                continue;
            }

            // Reads through `ConfigCache`, so this stays cheap at probe cadence.
            let expects_danmu = self
                .config_service
                .get_config_for_streamer(&download.streamer_id)
                .await
                .is_ok_and(|config| config.record_danmu);

            if expects_danmu {
                stopped_early.push(download.session_id);
            }
        }

        // A collector that is present but not receiving is invisible to
        // `is_collecting`, since the runner reconnects for the life of the session
        // rather than exiting.
        let mut link_down: Vec<String> = self
            .danmu_link_down
            .iter()
            .filter(|entry| entry.value().elapsed() >= Self::OUTAGE_GRACE)
            .map(|entry| format!("{} ({}s)", entry.key(), entry.value().elapsed().as_secs()))
            .collect();
        link_down.sort();

        if stopped_early.is_empty() && link_down.is_empty() {
            return ComponentHealth::healthy("danmu_service");
        }

        let mut reasons = Vec::new();
        if !stopped_early.is_empty() {
            reasons.push(format!(
                "collection stopped while recording continues for {} session(s): {}",
                stopped_early.len(),
                stopped_early.join(", ")
            ));
        }
        if !link_down.is_empty() {
            reasons.push(format!(
                "chat link down for {} session(s): {}",
                link_down.len(),
                link_down.join(", ")
            ));
        }

        ComponentHealth::degraded("danmu_service", format!("Danmu {}", reasons.join("; ")))
    }
}

struct StaticHealthyProbe {
    name: &'static str,
    cadence: Duration,
}

#[async_trait]
impl HealthProbe for StaticHealthyProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.name)
    }

    fn cadence(&self) -> Duration {
        self.cadence
    }

    async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
        ComponentHealth::healthy(self.name)
    }
}

/// Synchronous writability probe used by `run_output_root_startup_probe`.
///
/// Creates a temp file inside `root` with restrictive permissions, writes
/// zero bytes, and drops it (RAII unlink via the `tempfile` crate). Returns
/// the underlying `io::Error` on any failure so the gate can classify via
/// `IoErrorKindSer::from_io_kind`.
///
/// Kept separate from `OutputRootGate::record_failure` so the gate itself
/// stays ignorant of how failures are discovered — it just accepts an
/// `io::Error` from any caller.
fn probe_root_writable(root: &std::path::Path) -> std::io::Result<()> {
    // Ensure the root itself is a directory. `std::fs::metadata` follows
    // symlinks, which is what we want — a dangling symlink would trip the
    // gate with ENOENT, correctly.
    let meta = std::fs::metadata(root)?;
    if !meta.is_dir() {
        return Err(std::io::Error::other(format!(
            "root path {} is not a directory",
            root.display()
        )));
    }

    // tempfile::Builder::tempfile_in uses O_EXCL + restrictive mode by
    // default on Unix, which is what we want: no symlink/TOCTOU window
    // and no leftover probe file even if the process is killed.
    let mut file = tempfile::Builder::new()
        .prefix(".rust-srec-probe-")
        .tempfile_in(root)?;
    std::io::Write::write_all(&mut file, b"")?;
    // `file` drops here and the tempfile crate unlinks it.
    Ok(())
}

/// Discover concrete probe paths from current configuration. Failed reads only skip their
/// source; discovery never changes or retires existing gate state.
pub(super) async fn discover_output_probe_paths(
    config_service: &ConfigService<SqlxConfigRepository, SqlxStreamerRepository>,
    streamer_manager: &crate::streamer::StreamerManager<SqlxStreamerRepository>,
    gate: &OutputRootGate,
) -> std::collections::HashSet<std::path::PathBuf> {
    let mut templates = Vec::new();
    // OUTPUT_DIR is applied to persisted global config during database initialization.
    // Read that effective value, rather than retaining an obsolete environment path.
    match config_service.get_global_config().await {
        Ok(global) => templates.push(global.output_folder),
        Err(error) => {
            warn!(%error, "Output-root discovery could not read global config");
        }
    }
    match config_service.list_platform_configs().await {
        Ok(platforms) => templates.extend(platforms.into_iter().filter_map(|p| p.output_folder)),
        Err(error) => {
            warn!(%error, "Output-root discovery could not read platform configs");
        }
    }
    match config_service.list_template_configs().await {
        Ok(configs) => templates.extend(configs.into_iter().filter_map(|t| t.output_folder)),
        Err(error) => {
            warn!(%error, "Output-root discovery could not read template configs");
        }
    }
    let merged = futures::stream::iter(streamer_manager.get_all())
        .map(|streamer| async move {
            let result = config_service.get_config_for_streamer(&streamer.id).await;
            (streamer, result)
        })
        .buffer_unordered(MAX_CONCURRENT_OUTPUT_CONFIG_MERGES)
        .collect::<Vec<_>>()
        .await;
    for (streamer, result) in merged {
        match result {
            Ok(merged) => {
                // Match runtime directory expansion order. Title/session/date values are not
                // known here; retaining their placeholders prevents inventing a probe root.
                templates.push(
                    merged
                        .output_folder
                        .replace(
                            "{streamer}",
                            &crate::utils::filename::sanitize_filename(&streamer.name),
                        )
                        .replace("{platform}", streamer.platform()),
                );
            }
            Err(error) => {
                warn!(streamer_id = %streamer.id, %error, "Output-root discovery could not merge streamer config")
            }
        }
    }
    select_output_probe_paths(gate, &templates)
}

fn select_output_probe_paths(
    gate: &OutputRootGate,
    templates: &[String],
) -> std::collections::HashSet<std::path::PathBuf> {
    let mut targets = std::collections::BTreeMap::<std::path::PathBuf, std::path::PathBuf>::new();
    for template in templates {
        if let Some(path) = gate.probe_path_for_template(template) {
            let key = gate.resolve_path(&path);
            let selected = targets.entry(key).or_insert_with(|| path.clone());
            // Sample one concrete directory per key. Prefer the deepest known destination,
            // then lexical order for deterministic results independent of config load order.
            // A sample cannot certify sibling permissions; runtime writes remain authoritative.
            if path.components().count() > selected.components().count()
                || (path.components().count() == selected.components().count() && path < *selected)
            {
                *selected = path;
            }
        }
    }
    for root in gate.configured_paths() {
        targets.insert(gate.resolve_path(root), root.clone());
    }
    targets.into_values().collect()
}

fn bounded_output_probe_paths(
    gate: &OutputRootGate,
    paths: std::collections::HashSet<std::path::PathBuf>,
) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort_by(|left, right| {
        (!gate.configured_paths().contains(left))
            .cmp(&(!gate.configured_paths().contains(right)))
            .then_with(|| left.cmp(right))
    });
    if paths.len() > MAX_OUTPUT_ROOT_PROBES {
        let skipped_keys: Vec<_> = paths[MAX_OUTPUT_ROOT_PROBES..]
            .iter()
            .take(4)
            .map(|path| gate.resolve_path(path))
            .collect();
        warn!(
            skipped = paths.len() - MAX_OUTPUT_ROOT_PROBES,
            ?skipped_keys,
            "Output-root probe limit reached; remaining roots are checked on real writes"
        );
        paths.truncate(MAX_OUTPUT_ROOT_PROBES);
    }
    paths
}

impl ServiceContainer {
    /// Concrete directories safe to probe, each resolving to the runtime gate key.
    /// Disk probes use these paths too, without testing write access to ancestor keys.
    pub(super) async fn collect_output_roots(
        &self,
    ) -> std::collections::HashSet<std::path::PathBuf> {
        discover_output_probe_paths(
            &self.config_service,
            &self.streamer_manager,
            &self.output_root_gate,
        )
        .await
    }

    /// Run the output-root write gate's one-shot startup probe.
    ///
    /// Each root from [`Self::collect_output_roots`] is probed in parallel
    /// via `spawn_blocking` (sync `tempfile` creation, write zero bytes,
    /// RAII unlink) wrapped in a 5-second tokio timeout. A timeout or any
    /// error feeds the synthetic `io::Error` into `gate.record_failure`, so
    /// broken mounts are visible in `/health` from second zero rather than
    /// waiting for the first monitor tick to attempt a download.
    /// At most 16 keys are attempted, four at a time. A timed-out blocking OS call
    /// can outlive its attempt; the total cap also bounds those outstanding calls.
    ///
    /// This is the ONLY synthetic probe in the design — all other gate
    /// transitions are event-driven via real `ensure_output_dir` calls
    /// and engine stderr readers. See
    /// `crate::downloader::output_root_gate` for the rationale.
    pub(super) async fn run_output_root_startup_probe(&self) {
        let roots =
            bounded_output_probe_paths(&self.output_root_gate, self.collect_output_roots().await);

        if roots.is_empty() {
            debug!("Output-root startup probe: no roots to probe");
            return;
        }

        info!(count = roots.len(), "Running output-root startup probe");

        futures::stream::iter(roots)
            .map(|root| {
                let gate = self.output_root_gate.clone();
                async move {
                    let probe_result = tokio::time::timeout(
                        Duration::from_secs(5),
                        tokio::task::spawn_blocking({
                            let root = root.clone();
                            move || probe_root_writable(&root)
                        }),
                    )
                    .await;

                    match probe_result {
                        Ok(Ok(Ok(()))) => {
                            debug!(root = %root.display(), "Startup probe: healthy");
                        }
                        Ok(Ok(Err(io_err))) => {
                            warn!(
                                root = %root.display(),
                                error = %io_err,
                                "Startup probe: output root unwritable"
                            );
                            gate.record_failure(&root, &io_err);
                        }
                        Ok(Err(join_err)) => {
                            warn!(
                                root = %root.display(),
                                error = %join_err,
                                "Startup probe: spawn_blocking failed (likely panic)"
                            );
                            let synthetic = std::io::Error::other("probe task panicked");
                            gate.record_failure(&root, &synthetic);
                        }
                        Err(_timeout) => {
                            warn!(
                                root = %root.display(),
                                "Startup probe: timed out after 5s (hung mount?)"
                            );
                            let synthetic = std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "probe timed out",
                            );
                            gate.record_failure(&root, &synthetic);
                        }
                    }
                }
            })
            .buffer_unordered(MAX_CONCURRENT_OUTPUT_PROBES)
            .for_each(|()| async {})
            .await;

        info!("Output-root startup probe complete");
    }

    /// Detect the host GPU and install the [`crate::metrics::GpuHealthMonitor`] on the
    /// container if `nvidia-smi` is available. Called from
    /// [`Self::initialize`] **before** subscription wiring so the
    /// config-event handler can capture a plain `Option<Arc<…>>` clone
    /// of `gpu_health_monitor` for hot-reloading the probe interval.
    ///
    /// Idempotent: if the field is already populated (e.g. a future
    /// caller invokes this twice), the second call is a no-op and logs
    /// at warn.
    pub(super) async fn init_gpu_health_monitor(&self) {
        if self.gpu_health_monitor.get().is_some() {
            return;
        }

        let default = crate::metrics::DEFAULT_GPU_PROBE_INTERVAL_SECS;
        let initial_interval = match self.config_service.get_global_config().await {
            Ok(cfg) => match cfg.gpu_health_probe_interval_secs {
                n if n > 0 => n as u64,
                _ => default,
            },
            Err(_) => default,
        };

        let Some(monitor) = crate::metrics::GpuHealthMonitor::detect(
            Arc::downgrade(&self.notification_service),
            initial_interval,
        )
        .await
        else {
            debug!("GPU health monitor not registered: nvidia-smi unavailable at startup");
            return;
        };

        let handle = monitor.start(self.cancellation_token.child_token());
        self.task_supervisor
            .spawn("GPU health monitor", async move {
                if let Err(error) = handle.await {
                    warn!(error = %error, "GPU health monitor task failed");
                }
            });

        if self.gpu_health_monitor.set(monitor).is_err() {
            warn!("GpuHealthMonitor was already installed; ignoring duplicate registration");
            return;
        }

        info!(
            interval_secs = initial_interval,
            "GPU health monitor started"
        );
    }

    /// Register health checks for all components.
    pub(super) async fn register_health_checks(&self) {
        use std::path::PathBuf;

        // Database health check — atomic pool-closed check; cheap.
        self.health_checker.register_probe(Arc::new(DatabaseProbe {
            pool: self.pool.clone(),
        }));

        let disk_warning_threshold = self.health_checker.disk_warning_threshold();
        let disk_critical_threshold = self.health_checker.disk_critical_threshold();

        // Disk space health checks: one per output root the downloader may
        // write to, so a streamer whose `output_folder` override lands on a
        // second disk reports that disk's free space rather than the global
        // one's. Sorted so component names stay stable across restarts, and
        // capped because these run for the process lifetime (30 s cadence)
        // unlike the one-shot write test in
        // `run_output_root_startup_probe`, which shares the same root set.
        let output_roots =
            bounded_output_probe_paths(&self.output_root_gate, self.collect_output_roots().await);

        if output_roots.is_empty() {
            // No concrete target has a provable runtime key. Inventory the available
            // static prefix (or working-directory filesystem) without a write probe.
            let output_dir = match self.config_service.get_global_config().await {
                Ok(cfg) => {
                    static_root_prefix(&cfg.output_folder).unwrap_or_else(|| ".".to_string())
                }
                Err(_) => ".".to_string(),
            };
            let output_dir_path = match std::env::current_dir() {
                Ok(cwd) => cwd.join(&output_dir),
                Err(_) => PathBuf::from(output_dir.clone()),
            };
            self.health_checker
                .register_probe(Arc::new(DiskSpaceProbe::new(
                    output_dir,
                    output_dir_path,
                    disk_warning_threshold,
                    disk_critical_threshold,
                )));
        } else {
            for root in output_roots {
                // Probe directories may be relative, just like the runtime output folder.
                // Disk inventory uses absolute mount points, so anchor only this lookup.
                let lookup_path = match std::env::current_dir() {
                    Ok(cwd) => cwd.join(&root),
                    Err(_) => root.clone(),
                };
                self.health_checker
                    .register_probe(Arc::new(DiskSpaceProbe::new(
                        root.to_string_lossy().to_string(),
                        lookup_path,
                        disk_warning_threshold,
                        disk_critical_threshold,
                    )));
            }
        }

        if let Ok(database_url) = std::env::var("DATABASE_URL")
            && let Some(db_file) = sqlite_file_path_from_url(&database_url)
        {
            let db_dir = db_file.parent().unwrap_or(db_file.as_path()).to_path_buf();
            let db_dir_str = db_dir.to_string_lossy().to_string();
            let db_dir_path = if db_dir.is_absolute() {
                db_dir
            } else if let Ok(cwd) = std::env::current_dir() {
                cwd.join(&db_dir)
            } else {
                db_dir
            };
            self.health_checker
                .register_probe(Arc::new(DiskSpaceProbe::new(
                    db_dir_str,
                    db_dir_path,
                    disk_warning_threshold,
                    disk_critical_threshold,
                )));
        }

        // Output-root write gate health check. Aggregated: one
        // "output-root" component whose status reflects the worst state
        // across all tracked roots, with a detailed message listing each
        // Degraded root by kind and age. See
        // `HealthChecker::check_output_root_gate` for the shape.
        self.health_checker
            .register_probe(Arc::new(OutputRootProbe {
                gate: self.output_root_gate.clone(),
            }));

        // GPU health monitor. Detection + probe-loop spawn happen
        // earlier in `initialize()` (see [`Self::init_gpu_health_monitor`])
        // so the config-event subscription handler can capture a clone
        // for hot-reload. Here we only register the probe if the monitor
        // is installed.
        if let Some(monitor) = self.gpu_health_monitor.get().cloned() {
            self.health_checker
                .register_probe(Arc::new(GpuProbe { monitor }));
        }

        self.health_checker
            .register_probe(Arc::new(DownloadManagerProbe {
                download_manager: self.download_manager.clone(),
            }));

        self.health_checker
            .register_probe(Arc::new(PipelineManagerProbe {
                pipeline_manager: self.pipeline_manager.clone(),
            }));

        self.health_checker
            .register_probe(Arc::new(DanmuServiceProbe {
                danmu_service: self.danmu_service.clone(),
                download_manager: self.download_manager.clone(),
                config_service: self.config_service.clone(),
                danmu_link_down: self.danmu_link_down.clone(),
            }));

        self.health_checker.register_probe(Arc::new(SchedulerProbe {
            cancellation_token: self.cancellation_token.clone(),
        }));

        self.health_checker
            .register_probe(Arc::new(StaticHealthyProbe {
                name: "notification_service",
                cadence: Duration::from_secs(10),
            }));

        self.health_checker
            .register_probe(Arc::new(StaticHealthyProbe {
                name: "maintenance_scheduler",
                cadence: Duration::from_secs(10),
            }));

        // Spawn the snapshot-refresh task so `/api/health` reads see
        // populated data within seconds.
        let handle = self
            .health_checker
            .start(self.cancellation_token.child_token());
        self.task_supervisor.spawn("health snapshots", async move {
            if let Err(error) = handle.await {
                warn!(error = %error, "Health snapshot task failed");
            }
        });

        info!("Health checks registered");
    }
}

#[cfg(test)]
mod output_root_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{DiskSnapshot, HealthStatus};

    fn snapshot_with_disk(mount_point: &str, available: u64, total: u64) -> SystemMetricsSnapshot {
        SystemMetricsSnapshot {
            cpu_usage: 0.0,
            memory_usage: 0.0,
            disks: Arc::from(
                vec![DiskSnapshot {
                    mount_point: std::path::PathBuf::from(mount_point),
                    available_space: available,
                    total_space: total,
                }]
                .into_boxed_slice(),
            ),
        }
    }

    #[tokio::test]
    async fn disk_probe_reports_capacity_while_healthy() {
        let probe = DiskSpaceProbe::new(
            "/rec".to_string(),
            std::path::PathBuf::from("/rec/huya"),
            0.80,
            0.95,
        );
        let health = probe
            .probe(snapshot_with_disk(
                "/",
                60 * 1024 * 1024 * 1024,
                100 * 1024 * 1024 * 1024,
            ))
            .await;

        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.message.is_none());
        let disk = health
            .disk
            .expect("a resolved filesystem must report its capacity");
        assert_eq!(disk.path, "/rec");
        assert_eq!(disk.mount_point, "/");
        assert_eq!(disk.available_bytes, 60 * 1024 * 1024 * 1024);
        assert_eq!(disk.total_bytes, 100 * 1024 * 1024 * 1024);
    }

    #[tokio::test]
    async fn disk_probe_reports_capacity_while_degraded() {
        let probe = DiskSpaceProbe::new(
            "/rec".to_string(),
            std::path::PathBuf::from("/rec"),
            0.80,
            0.95,
        );
        let health = probe
            .probe(snapshot_with_disk(
                "/",
                10 * 1024 * 1024 * 1024,
                100 * 1024 * 1024 * 1024,
            ))
            .await;

        assert_eq!(health.status, HealthStatus::Degraded);
        let disk = health.disk.expect("capacity rides along on every status");
        assert!((disk.used_percent - 90.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn disk_probe_without_matching_filesystem_reports_unknown() {
        let probe = DiskSpaceProbe::new(
            "/rec".to_string(),
            std::path::PathBuf::from("/rec"),
            0.80,
            0.95,
        );
        let health = probe.probe(SystemMetricsSnapshot::empty()).await;

        assert_eq!(health.status, HealthStatus::Unknown);
        assert!(health.disk.is_none());
    }
}
