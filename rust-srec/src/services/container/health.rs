use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::downloader::{DownloadManager, OutputRootGate};
use crate::metrics::{ComponentHealth, HealthChecker, HealthProbe, SystemMetricsSnapshot};
use crate::pipeline::PipelineManager;

use super::{
    ServiceContainer, parse_output_roots_env, sqlite_file_path_from_url, static_root_prefix,
};

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
            Some(disk) => HealthChecker::check_disk_space_with_thresholds(
                &self.display_path,
                disk.available_space,
                disk.total_space,
                self.warning_threshold,
                self.critical_threshold,
            ),
            None => ComponentHealth {
                name: self.component_name.clone(),
                status: crate::metrics::HealthStatus::Unknown,
                message: Some("Unable to resolve disk for path".to_string()),
                last_check: Some(chrono::Utc::now().to_rfc3339()),
                check_duration_ms: None,
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

impl ServiceContainer {
    /// Run the output-root write gate's one-shot startup probe.
    ///
    /// Collects the set of root paths to probe from:
    ///
    /// 1. `RUST_SREC_OUTPUT_ROOTS` env var (if set).
    /// 2. Otherwise, resolves each streamer's configured `output_folder`
    ///    through `expand_path_template` + `resolve_root` and deduplicates.
    ///
    /// Each root is probed in parallel via `spawn_blocking` (sync `tempfile`
    /// creation, write zero bytes, RAII unlink) wrapped in a 5-second
    /// tokio timeout. A timeout or any error feeds the synthetic
    /// `io::Error` into `gate.record_failure`, so broken mounts are
    /// visible in `/health` from second zero rather than waiting for the
    /// first monitor tick to attempt a download.
    ///
    /// This is the ONLY synthetic probe in the design — all other gate
    /// transitions are event-driven via real `ensure_output_dir` calls
    /// and engine stderr readers. See
    /// `crate::downloader::output_root_gate` for the rationale.
    pub(super) async fn run_output_root_startup_probe(&self) {
        use std::collections::HashSet;

        // Build the union of roots to probe from all sources. Every source
        // feeds through the gate's own `resolve_path` so the keys match
        // what the runtime hot path will use — and we dedupe via HashSet
        // so overlapping templates (e.g. three platforms all writing to
        // `/rec/...`) produce a single probe.
        let mut roots: HashSet<std::path::PathBuf> = HashSet::new();

        // 1. Explicit env var always wins — if the user configured it,
        //    they know exactly what mounts they want watched.
        for root in parse_output_roots_env() {
            roots.insert(root);
        }

        // 2. `OUTPUT_DIR` env var, only when the operator set it. When
        //    unset, step 3 (global config `output_folder`) covers the
        //    canonical default — probing `./output` here would register
        //    a root the downloader never uses on a typical install, and
        //    the tempfile probe would silently create that directory.
        if let Ok(raw) = std::env::var("OUTPUT_DIR")
            && !raw.trim().is_empty()
        {
            roots.insert(
                self.output_root_gate
                    .resolve_path(std::path::Path::new(raw.trim())),
            );
        }

        // 3. Global config's `output_folder` template. This is the
        //    source the download path actually consults; it may be
        //    `/rec/{platform}/{streamer}/...`-style.
        match self.config_service.get_global_config().await {
            Ok(global) => {
                if let Some(prefix) = static_root_prefix(&global.output_folder) {
                    roots.insert(
                        self.output_root_gate
                            .resolve_path(std::path::Path::new(&prefix)),
                    );
                }
            }
            Err(e) => warn!(
                error = %e,
                "Output-root startup probe: failed to read global config (continuing with env roots)"
            ),
        }

        // 4. Platform-level overrides. Platforms are a small fixed set
        //    (one entry per streaming site). One list call.
        match self.config_service.list_platform_configs().await {
            Ok(platforms) => {
                for p in platforms {
                    if let Some(folder) = p.output_folder.as_ref()
                        && let Some(prefix) = static_root_prefix(folder)
                    {
                        roots.insert(
                            self.output_root_gate
                                .resolve_path(std::path::Path::new(&prefix)),
                        );
                    }
                }
            }
            Err(e) => warn!(
                error = %e,
                "Output-root startup probe: failed to list platform configs (continuing)"
            ),
        }

        // 5. Template-level overrides. Templates are user-defined
        //    presets shared across streamers; there are typically a
        //    handful. One list call, cached.
        match self.config_service.list_template_configs().await {
            Ok(templates) => {
                for t in templates {
                    if let Some(folder) = t.output_folder.as_ref()
                        && let Some(prefix) = static_root_prefix(folder)
                    {
                        roots.insert(
                            self.output_root_gate
                                .resolve_path(std::path::Path::new(&prefix)),
                        );
                    }
                }
            }
            Err(e) => warn!(
                error = %e,
                "Output-root startup probe: failed to list template configs (continuing)"
            ),
        }

        // 6. Per-streamer overrides. `get_config_for_streamer` is cached
        //    with in-flight dedup, so the N merges we do here are the same
        //    N merges the first download of each streamer would have done
        //    lazily — we're just paying the cost concentrated at boot,
        //    which in turn pre-warms the cache for faster first-download
        //    latency. Runs in parallel via `join_all` so total wall-clock
        //    is bounded by the slowest single merge (typically < 50ms).
        let streamer_ids: Vec<String> = self
            .streamer_manager
            .get_all()
            .into_iter()
            .map(|s| s.id)
            .collect();
        if !streamer_ids.is_empty() {
            let merge_futures = streamer_ids.into_iter().map(|id| {
                let cs = self.config_service.clone();
                async move {
                    let result = cs.get_config_for_streamer(&id).await;
                    (id, result)
                }
            });
            let results = futures::future::join_all(merge_futures).await;
            for (id, result) in results {
                match result {
                    Ok(merged) => {
                        if let Some(prefix) = static_root_prefix(&merged.output_folder) {
                            roots.insert(
                                self.output_root_gate
                                    .resolve_path(std::path::Path::new(&prefix)),
                            );
                        }
                    }
                    Err(e) => debug!(
                        streamer_id = %id,
                        error = %e,
                        "Output-root startup probe: skipping streamer whose config failed to merge"
                    ),
                }
            }
        }

        if roots.is_empty() {
            debug!("Output-root startup probe: no roots to probe");
            return;
        }

        info!(count = roots.len(), "Running output-root startup probe");

        let mut handles = Vec::with_capacity(roots.len());
        for root in roots {
            let gate = self.output_root_gate.clone();
            handles.push(tokio::spawn(async move {
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
                        let synthetic =
                            std::io::Error::new(std::io::ErrorKind::TimedOut, "probe timed out");
                        gate.record_failure(&root, &synthetic);
                    }
                }
            }));
        }

        for h in handles {
            let _ = h.await;
        }

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

        // Disk space health checks (output dir and DB directory).
        // Priority: explicit OUTPUT_DIR env, then the static prefix of the
        // global config's `output_folder` template (matches what the
        // download path will actually consult), then `./output` as a last
        // resort for display when neither source is usable.
        let output_dir = {
            let env_dir = std::env::var("OUTPUT_DIR")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            match env_dir {
                Some(v) => v,
                None => match self.config_service.get_global_config().await {
                    Ok(cfg) => static_root_prefix(&cfg.output_folder)
                        .unwrap_or_else(|| "./output".to_string()),
                    Err(_) => "./output".to_string(),
                },
            }
        };
        // Ensure path is absolute for disk lookup
        let output_dir_path = if let Ok(cwd) = std::env::current_dir() {
            cwd.join(&output_dir)
        } else {
            PathBuf::from(output_dir.clone())
        };

        let disk_warning_threshold = self.health_checker.disk_warning_threshold();
        let disk_critical_threshold = self.health_checker.disk_critical_threshold();
        self.health_checker
            .register_probe(Arc::new(DiskSpaceProbe::new(
                output_dir,
                output_dir_path,
                disk_warning_threshold,
                disk_critical_threshold,
            )));

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
            .register_probe(Arc::new(StaticHealthyProbe {
                name: "danmu_service",
                cadence: Duration::from_secs(5),
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
