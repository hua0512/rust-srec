//! Health-check infrastructure.
//!
//! `/api/health` reads do **one `Arc::clone`** of a cached snapshot —
//! they don't run any probes and don't acquire a lock. Probes refresh in
//! the background on per-probe cadences via [`HealthChecker::start`],
//! merging their results into a single [`Arc<SystemHealth>`] published
//! through `ArcSwap`.
//!
//! The cadence machinery is the lever that keeps cost low: cheap
//! probes (atomics) refresh every 5 s; expensive ones (disk inventory)
//! every 30 s. Runtime scheduling state and the `sysinfo` inventory live
//! inside the refresh task, so probes receive owned metric snapshots
//! instead of sharing locks.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::notification::NotificationEvent;

/// Refresh-task tick rate. Per-probe cadence is honored on top of this:
/// a probe with a 30 s cadence runs at most once every 30 s even though
/// the loop wakes on this tick. Set to match the cheapest registered
/// probe cadence — finer ticks just rebuild the snapshot more often
/// without observable benefit (the dashboard polls every 10 s).
const REFRESH_TICK: Duration = Duration::from_secs(5);

/// Health status of a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// Component is healthy.
    Healthy,
    /// Component is degraded but functional.
    Degraded,
    /// Component is unhealthy.
    Unhealthy,
    /// Component status is unknown.
    #[default]
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Healthy => write!(f, "healthy"),
            HealthStatus::Degraded => write!(f, "degraded"),
            HealthStatus::Unhealthy => write!(f, "unhealthy"),
            HealthStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// Health information for a single component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name.
    pub name: String,
    /// Health status.
    pub status: HealthStatus,
    /// Optional message.
    pub message: Option<String>,
    /// Last check time (ISO 8601).
    pub last_check: Option<String>,
    /// Check duration in milliseconds.
    pub check_duration_ms: Option<u64>,
}

impl ComponentHealth {
    /// Create a healthy component.
    pub fn healthy(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Healthy,
            message: None,
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Create an unhealthy component.
    pub fn unhealthy(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Unhealthy,
            message: Some(message.into()),
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Create a degraded component.
    pub fn degraded(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: HealthStatus::Degraded,
            message: Some(message.into()),
            last_check: Some(chrono::Utc::now().to_rfc3339()),
            check_duration_ms: None,
        }
    }

    /// Set the check duration.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.check_duration_ms = Some(duration.as_millis() as u64);
        self
    }
}

/// Overall system health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    /// Overall status.
    pub status: HealthStatus,
    /// Component health details.
    pub components: HashMap<String, ComponentHealth>,
    /// System version.
    pub version: String,
    /// Uptime in seconds.
    pub uptime_secs: u64,
    /// Timestamp of the health check.
    pub timestamp: String,
    /// CPU usage percentage (0-100).
    pub cpu_usage: f32,
    /// Memory usage percentage (0-100).
    pub memory_usage: f32,
}

impl SystemHealth {
    /// Check if the system is ready to serve requests.
    pub fn is_ready(&self) -> bool {
        matches!(self.status, HealthStatus::Healthy | HealthStatus::Degraded)
    }

    /// Check if the system is healthy.
    pub fn is_healthy(&self) -> bool {
        self.status == HealthStatus::Healthy
    }
}

/// Point-in-time disk information copied out of `sysinfo` by the
/// refresh task. Health probes receive this owned snapshot so they never
/// need to share the `sysinfo` inventory itself.
#[derive(Debug, Clone)]
pub struct DiskSnapshot {
    /// Filesystem mount point.
    pub mount_point: PathBuf,
    /// Available bytes on the filesystem.
    pub available_space: u64,
    /// Total bytes on the filesystem.
    pub total_space: u64,
}

/// Point-in-time system metrics passed to each due health probe.
#[derive(Debug, Clone)]
pub struct SystemMetricsSnapshot {
    /// CPU usage percentage (0-100).
    pub cpu_usage: f32,
    /// Memory usage percentage (0-100).
    pub memory_usage: f32,
    /// Mounted filesystems when any due probe requested disk data.
    pub disks: Arc<[DiskSnapshot]>,
}

impl SystemMetricsSnapshot {
    /// Return an empty metrics snapshot.
    pub fn empty() -> Self {
        Self {
            cpu_usage: 0.0,
            memory_usage: 0.0,
            disks: Arc::from(Vec::<DiskSnapshot>::new().into_boxed_slice()),
        }
    }

    /// Pick the disk whose mount point contains `path`. Falls back to
    /// the longest matching mount point so a nested mount wins over its
    /// parent.
    pub fn best_disk_for_path(&self, path: &Path) -> Option<&DiskSnapshot> {
        self.disks
            .iter()
            .filter(|d| path.starts_with(&d.mount_point))
            .max_by_key(|d| d.mount_point.as_os_str().to_string_lossy().len())
    }
}

impl Default for SystemMetricsSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

/// One periodic component health probe.
///
/// Probes run in the background by [`HealthChecker::start`] at their
/// own [`cadence`](Self::cadence). Their results are merged into the
/// snapshot consumed by `/api/health` reads. The cadence is the lever
/// for trading freshness against cost: cheap probes (atomics) should
/// poll every few seconds; expensive probes (sysinfo, disk inventory)
/// should poll every 30 s or longer so the per-probe cost amortizes.
#[async_trait]
pub trait HealthProbe: Send + Sync {
    /// Component name; matches the key the dashboard renders. Stable
    /// across probe runs.
    fn name(&self) -> Cow<'_, str>;

    /// How often the refresh task should call [`probe`](Self::probe).
    /// At least one tick of [`REFRESH_TICK`] elapses between probes
    /// regardless of cadence.
    fn cadence(&self) -> Duration;

    /// Whether this probe needs a disk inventory snapshot. Disk refresh
    /// is materially heavier than CPU/memory refresh, so the refresh task
    /// only asks `sysinfo` for disk data when at least one due probe opts
    /// in.
    fn needs_disk_snapshot(&self) -> bool {
        false
    }

    /// Compute the latest value. Allowed to be expensive — only the
    /// background refresh task calls this, never the request path.
    async fn probe(&self, metrics: SystemMetricsSnapshot) -> ComponentHealth;
}

/// Long-lived `sysinfo` inventory owned by the refresh task.
struct SysinfoCache {
    /// CPU + memory inventory. Refresh with [`refresh_cpu_mem`].
    system: System,
    /// Mounted filesystems. Refresh with [`refresh_disks`].
    disks: sysinfo::Disks,
}

impl SysinfoCache {
    fn new() -> Self {
        Self {
            system: System::new_with_specifics(
                RefreshKind::nothing()
                    .with_cpu(CpuRefreshKind::everything())
                    .with_memory(MemoryRefreshKind::everything()),
            ),
            disks: sysinfo::Disks::new_with_refreshed_list(),
        }
    }

    /// In-place refresh of CPU and memory metrics.
    fn refresh_cpu_mem(&mut self) {
        self.system.refresh_cpu_all();
        self.system.refresh_memory();
    }

    /// In-place refresh of the mounted-filesystem inventory.
    fn refresh_disks(&mut self) {
        self.disks.refresh(true);
    }

    /// Global CPU usage percent (0–100).
    fn cpu_usage(&self) -> f32 {
        self.system.global_cpu_usage()
    }

    /// Memory usage percent (0–100). Returns 0 when total is unknown.
    fn memory_usage_pct(&self) -> f32 {
        let total = self.system.total_memory();
        if total == 0 {
            0.0
        } else {
            (self.system.used_memory() as f64 / total as f64 * 100.0) as f32
        }
    }

    /// Copy the current `sysinfo` view into an owned probe snapshot.
    fn snapshot(&self, include_disks: bool) -> SystemMetricsSnapshot {
        let disks = if include_disks {
            self.disks
                .iter()
                .map(|d| DiskSnapshot {
                    mount_point: d.mount_point().to_path_buf(),
                    available_space: d.available_space(),
                    total_space: d.total_space(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
        } else {
            Vec::<DiskSnapshot>::new().into_boxed_slice()
        };

        SystemMetricsSnapshot {
            cpu_usage: self.cpu_usage(),
            memory_usage: self.memory_usage_pct(),
            disks: Arc::from(disks),
        }
    }
}

/// Health checker for the system.
pub struct HealthChecker {
    /// Latest snapshot. `/api/health` reads do one lock-free
    /// `Arc::clone` of this.
    snapshot: ArcSwap<SystemHealth>,
    /// Registered probes. Setup can append probes before `start()`; the
    /// refresh task snapshots the vector once and owns that frozen view.
    probes: ArcSwap<Vec<Arc<dyn HealthProbe>>>,
    /// Prevent accidentally starting two refresh loops for one checker.
    started: AtomicBool,
    /// System start time.
    start_time: Instant,
    /// System version.
    version: String,
    /// Disk space warning threshold (percentage).
    disk_warning_threshold: f64,
    /// Disk space critical threshold (percentage).
    disk_critical_threshold: f64,
}

impl HealthChecker {
    /// Construct a new checker with empty probe set and a baseline
    /// snapshot. Prefer [`HealthChecker::with_probes`] for the runtime
    /// service container.
    pub fn new() -> Self {
        Self::with_thresholds(0.80, 0.95)
    }

    /// Construct with custom disk warning/critical thresholds.
    pub fn with_thresholds(disk_warning: f64, disk_critical: f64) -> Self {
        Self::with_probes_and_thresholds(Vec::new(), disk_warning, disk_critical)
    }

    /// Construct with a frozen probe set and default disk thresholds.
    pub fn with_probes(probes: Vec<Arc<dyn HealthProbe>>) -> Self {
        Self::with_probes_and_thresholds(probes, 0.80, 0.95)
    }

    /// Construct with a frozen probe set and custom disk thresholds.
    pub fn with_probes_and_thresholds(
        probes: Vec<Arc<dyn HealthProbe>>,
        disk_warning: f64,
        disk_critical: f64,
    ) -> Self {
        let initial = Arc::new(SystemHealth {
            status: HealthStatus::Unknown,
            components: HashMap::new(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: 0,
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage: 0.0,
            memory_usage: 0.0,
        });
        Self {
            snapshot: ArcSwap::from(initial),
            probes: ArcSwap::from_pointee(probes),
            started: AtomicBool::new(false),
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            disk_warning_threshold: disk_warning,
            disk_critical_threshold: disk_critical,
        }
    }

    /// Disk-warning threshold (used by [`Self::check_disk_space`] and
    /// [`Self::disk_space_notification`]).
    pub fn disk_warning_threshold(&self) -> f64 {
        self.disk_warning_threshold
    }

    /// Disk-critical threshold.
    pub fn disk_critical_threshold(&self) -> f64 {
        self.disk_critical_threshold
    }

    /// Register a concrete probe before [`Self::start`] freezes the
    /// runtime probe set. Late registrations are ignored because the
    /// refresh task deliberately owns its probe schedule after startup.
    pub fn register_probe(&self, probe: Arc<dyn HealthProbe>) {
        let name = probe.name().into_owned();
        if self.started.load(Ordering::Acquire) {
            warn!(
                probe = %name,
                "HealthChecker: ignoring probe registered after refresh loop start"
            );
            return;
        }

        let mut next = self.probes.load_full().as_ref().clone();
        next.push(probe);
        self.probes.store(Arc::new(next));
    }

    /// Cheap snapshot read for the request path. One lock-free
    /// `Arc::clone` — no probe calls, no allocation.
    pub fn current(&self) -> Arc<SystemHealth> {
        self.snapshot.load_full()
    }

    /// Check readiness (for Kubernetes probes).
    pub fn check_ready(&self) -> bool {
        self.current().is_ready()
    }

    /// Spawn the background refresh task. Runs an immediate first
    /// refresh of every registered probe so the snapshot is populated
    /// within seconds of process start, then ticks at
    /// [`REFRESH_TICK`] cadence for the lifetime of the supplied
    /// [`CancellationToken`].
    ///
    /// Returns the [`JoinHandle`]; cancellation is driven by the token
    /// so callers normally don't need to await it.
    pub fn start(self: &Arc<Self>, cancel: CancellationToken) -> JoinHandle<()> {
        if self.started.swap(true, Ordering::AcqRel) {
            return tokio::spawn(async {
                warn!(
                    "HealthChecker: start called more than once; ignoring duplicate refresh loop"
                );
            });
        }

        let checker = Arc::clone(self);
        let probes = self.probes.load_full().as_ref().clone();
        tokio::spawn(async move {
            let mut sysinfo = SysinfoCache::new();
            let mut last_run = HashMap::<String, Instant>::new();

            // First fill: every probe is "due" because last_run is None,
            // so refresh_due() runs them all and populates the snapshot.
            checker
                .refresh_due(&probes, &mut last_run, &mut sysinfo)
                .await;

            let mut ticker = tokio::time::interval(REFRESH_TICK);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            // Discard the immediate tick — `tokio::time::interval` always
            // fires once on first call, but we already did the first
            // refresh above.
            ticker.tick().await;

            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        debug!("HealthChecker: cancellation token fired, exiting refresh loop");
                        return;
                    }
                    _ = ticker.tick() => {
                        checker
                            .refresh_due(&probes, &mut last_run, &mut sysinfo)
                            .await;
                    }
                }
            }
        })
    }

    /// Run every probe whose cadence has elapsed (or that has never
    /// run), in parallel; merge the new component values into the
    /// snapshot atomically along with refreshed CPU/memory metrics.
    async fn refresh_due(
        &self,
        probes: &[Arc<dyn HealthProbe>],
        last_run: &mut HashMap<String, Instant>,
        sysinfo: &mut SysinfoCache,
    ) {
        let now = Instant::now();
        let due: Vec<Arc<dyn HealthProbe>> = probes
            .iter()
            .filter(|probe| {
                let name = probe.name();
                last_run
                    .get(name.as_ref())
                    .is_none_or(|t| now.duration_since(*t) >= probe.cadence())
            })
            .cloned()
            .collect();

        // Always refresh CPU/mem on every tick — they're cheap and the
        // top-level fields drive the dashboard headline numbers; users
        // expect them to update faster than per-component cadences.
        sysinfo.refresh_cpu_mem();
        let include_disks = due.iter().any(|probe| probe.needs_disk_snapshot());
        if include_disks {
            sysinfo.refresh_disks();
        }

        let metrics = sysinfo.snapshot(include_disks);
        let cpu_usage = metrics.cpu_usage;
        let memory_usage = metrics.memory_usage;

        let new_components: HashMap<String, ComponentHealth> = if due.is_empty() {
            // No probe is due; just bump CPU/mem and the timestamp.
            self.current().components.clone()
        } else {
            // Each probe runs in its own task so the runtime isolates
            // panics — a panicking probe is reported as Unhealthy with
            // the panic payload while the refresh task and other probes
            // keep running.
            let handles: Vec<_> = due
                .iter()
                .map(|probe| {
                    let probe = Arc::clone(probe);
                    let metrics = metrics.clone();
                    tokio::spawn(async move {
                        let started = Instant::now();
                        let mut health = probe.probe(metrics).await;
                        health.check_duration_ms = Some(started.elapsed().as_millis() as u64);
                        health
                    })
                })
                .collect();
            let join_results = futures::future::join_all(handles).await;

            let mut components = self.current().components.clone();
            for (probe, join_result) in due.iter().zip(join_results.into_iter()) {
                let name = probe.name().into_owned();
                last_run.insert(name.clone(), now);
                let health = match join_result {
                    Ok(h) => h,
                    Err(join_err) if join_err.is_panic() => {
                        let payload = join_err.into_panic();
                        let panic_msg = payload
                            .downcast_ref::<&'static str>()
                            .map(|s| (*s).to_string())
                            .or_else(|| payload.downcast_ref::<String>().cloned())
                            .unwrap_or_else(|| "<non-string panic payload>".to_string());
                        warn!(probe = %name, panic = %panic_msg, "Health probe panicked");
                        ComponentHealth::unhealthy(
                            name.clone(),
                            format!("probe panicked: {panic_msg}"),
                        )
                    }
                    Err(join_err) => {
                        // Cancellation: rare during normal operation.
                        // Leave the previous component value untouched.
                        warn!(
                            probe = %name,
                            error = %join_err,
                            "Health probe task ended without producing a value"
                        );
                        continue;
                    }
                };
                components.insert(name, health);
            }
            components
        };

        let new_snapshot = Arc::new(SystemHealth {
            status: compute_overall_status(&new_components),
            components: new_components,
            version: self.version.clone(),
            uptime_secs: self.start_time.elapsed().as_secs(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage,
            memory_usage,
        });
        self.snapshot.store(new_snapshot);
    }

    /// Check disk space and return health status.
    pub fn check_disk_space(&self, path: &str, available: u64, total: u64) -> ComponentHealth {
        Self::check_disk_space_with_thresholds(
            path,
            available,
            total,
            self.disk_warning_threshold,
            self.disk_critical_threshold,
        )
    }

    /// Check disk space with explicit warning/critical thresholds.
    pub fn check_disk_space_with_thresholds(
        path: &str,
        available: u64,
        total: u64,
        warning_threshold: f64,
        critical_threshold: f64,
    ) -> ComponentHealth {
        if total == 0 {
            return ComponentHealth::unhealthy(
                format!("disk:{}", path),
                "Unable to determine disk space",
            );
        }

        let used_ratio = 1.0 - (available as f64 / total as f64);

        if used_ratio >= critical_threshold {
            warn!(
                "Disk space critical on {}: {:.1}% used",
                path,
                used_ratio * 100.0
            );
            ComponentHealth::unhealthy(
                format!("disk:{}", path),
                format!(
                    "Disk space critical: {:.1}% used ({} available)",
                    used_ratio * 100.0,
                    format_bytes(available)
                ),
            )
        } else if used_ratio >= warning_threshold {
            warn!(
                "Disk space warning on {}: {:.1}% used",
                path,
                used_ratio * 100.0
            );
            ComponentHealth::degraded(
                format!("disk:{}", path),
                format!(
                    "Disk space warning: {:.1}% used ({} available)",
                    used_ratio * 100.0,
                    format_bytes(available)
                ),
            )
        } else {
            debug!("Disk space OK on {}: {:.1}% used", path, used_ratio * 100.0);
            ComponentHealth::healthy(format!("disk:{}", path))
        }
    }

    /// Build a single aggregated `ComponentHealth` entry for the output-root
    /// write gate, suitable for exposing under the name `"output-root"` in
    /// [`SystemHealth::components`].
    ///
    /// Healthy if every tracked root is in the `Healthy` state (or the gate
    /// has never recorded a failure). Degraded if any root is in `Degraded`
    /// — in that case the message field lists each affected root with its
    /// classified `io_kind` and the seconds-since-last-attempt, so users can
    /// tell at a glance whether the degradation is fresh or stale.
    ///
    /// The aggregation is intentional: individual roots are ephemeral and
    /// dynamically created by `record_failure`, so registering a per-root
    /// health check upfront isn't possible. One aggregated entry with rich
    /// message text gives users a single place to look in the UI.
    pub fn check_output_root_gate(gate: &crate::downloader::OutputRootGate) -> ComponentHealth {
        let snapshot = gate.snapshot();
        let degraded: Vec<_> = snapshot
            .iter()
            .filter(|r| r.state == crate::downloader::RootHealthState::Degraded)
            .collect();

        if degraded.is_empty() {
            return ComponentHealth::healthy("output-root");
        }

        let mut lines: Vec<String> = Vec::with_capacity(degraded.len());
        for r in &degraded {
            let (kind, _msg) = r.last_error.as_ref().map_or(
                (crate::downloader::IoErrorKindSer::Other, String::new()),
                |(k, m)| (*k, m.clone()),
            );
            let age = r
                .seconds_since_last_attempt
                .map(|s| format!("{}s ago", s))
                .unwrap_or_else(|| "unknown".to_string());
            lines.push(format!(
                "{} (kind={}, rejected={}, last_attempt={})",
                r.root.display(),
                kind.as_str(),
                r.rejected_count,
                age
            ));
        }
        ComponentHealth::degraded(
            "output-root",
            format!(
                "{} output root(s) unwritable: {}",
                degraded.len(),
                lines.join("; ")
            ),
        )
    }

    /// Generate notification event for disk space issues.
    pub fn disk_space_notification(
        &self,
        path: &str,
        available: u64,
        total: u64,
    ) -> Option<NotificationEvent> {
        if total == 0 {
            return None;
        }

        let used_ratio = 1.0 - (available as f64 / total as f64);
        let threshold = if used_ratio >= self.disk_critical_threshold {
            (total as f64 * (1.0 - self.disk_critical_threshold)) as u64
        } else if used_ratio >= self.disk_warning_threshold {
            (total as f64 * (1.0 - self.disk_warning_threshold)) as u64
        } else {
            return None;
        };

        Some(NotificationEvent::OutOfSpace {
            path: path.to_string(),
            available_bytes: available,
            threshold_bytes: threshold,
            timestamp: chrono::Utc::now(),
        })
    }
}

impl Default for HealthChecker {
    fn default() -> Self {
        Self::new()
    }
}

/// Roll up component statuses into a single overall status.
fn compute_overall_status(components: &HashMap<String, ComponentHealth>) -> HealthStatus {
    let mut overall = HealthStatus::Healthy;
    for c in components.values() {
        match c.status {
            HealthStatus::Unhealthy => return HealthStatus::Unhealthy,
            HealthStatus::Degraded => overall = HealthStatus::Degraded,
            _ => {}
        }
    }
    overall
}

/// Format bytes into human-readable string.
fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticTestProbe {
        name: &'static str,
        cadence: Duration,
        health: ComponentHealth,
    }

    #[async_trait]
    impl HealthProbe for StaticTestProbe {
        fn name(&self) -> Cow<'_, str> {
            Cow::Borrowed(self.name)
        }

        fn cadence(&self) -> Duration {
            self.cadence
        }

        async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
            self.health.clone()
        }
    }

    #[test]
    fn test_health_status_default() {
        assert_eq!(HealthStatus::default(), HealthStatus::Unknown);
    }

    #[test]
    fn test_component_health_healthy() {
        let health = ComponentHealth::healthy("test");
        assert_eq!(health.status, HealthStatus::Healthy);
        assert!(health.message.is_none());
    }

    #[test]
    fn test_component_health_unhealthy() {
        let health = ComponentHealth::unhealthy("test", "Something went wrong");
        assert_eq!(health.status, HealthStatus::Unhealthy);
        assert_eq!(health.message, Some("Something went wrong".to_string()));
    }

    #[test]
    fn test_component_health_with_duration() {
        let health = ComponentHealth::healthy("test").with_duration(Duration::from_millis(100));
        assert_eq!(health.check_duration_ms, Some(100));
    }

    #[tokio::test]
    async fn test_health_checker_creation() {
        let checker = HealthChecker::new();
        let health = checker.current();
        assert!(health.components.is_empty());
        // Initial snapshot is Unknown until the first refresh runs.
        assert_eq!(health.status, HealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_health_checker_register_then_refresh() {
        let checker = Arc::new(HealthChecker::new());
        checker.register_probe(Arc::new(StaticTestProbe {
            name: "test",
            cadence: Duration::from_secs(5),
            health: ComponentHealth::healthy("test"),
        }));

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        // First-fill is synchronous-ish: yield once and the immediate
        // pre-tick refresh has populated the snapshot.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let health = checker.current();
        assert!(health.components.contains_key("test"));
        assert_eq!(health.status, HealthStatus::Healthy);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_health_checker_unhealthy_component() {
        let checker = Arc::new(HealthChecker::new());
        checker.register_probe(Arc::new(StaticTestProbe {
            name: "failing",
            cadence: Duration::from_secs(5),
            health: ComponentHealth::unhealthy("failing", "Error"),
        }));

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        tokio::time::sleep(Duration::from_millis(50)).await;

        let health = checker.current();
        assert_eq!(health.status, HealthStatus::Unhealthy);

        cancel.cancel();
        let _ = handle.await;
    }

    /// A counting probe used to verify cadence honoring + parallelism.
    struct CountingProbe {
        name: String,
        cadence: Duration,
        delay: Duration,
        counter: Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait]
    impl HealthProbe for CountingProbe {
        fn name(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.name)
        }
        fn cadence(&self) -> Duration {
            self.cadence
        }
        async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
            tokio::time::sleep(self.delay).await;
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            ComponentHealth::healthy(self.name.clone())
        }
    }

    /// A probe that records observed overlap with other probes.
    struct ConcurrentProbe {
        name: String,
        delay: Duration,
        in_flight: Arc<std::sync::atomic::AtomicU32>,
        max_in_flight: Arc<std::sync::atomic::AtomicU32>,
    }

    #[async_trait]
    impl HealthProbe for ConcurrentProbe {
        fn name(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.name)
        }

        fn cadence(&self) -> Duration {
            Duration::from_secs(60)
        }

        async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
            let current = self
                .in_flight
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                + 1;
            self.max_in_flight
                .fetch_max(current, std::sync::atomic::Ordering::AcqRel);
            tokio::time::sleep(self.delay).await;
            self.in_flight
                .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
            ComponentHealth::healthy(self.name.clone())
        }
    }

    #[tokio::test]
    async fn cadence_is_honored_between_ticks() {
        // 10-second cadence; over a 200 ms observation window we should
        // see exactly one probe call (the initial first-fill), not many.
        let checker = Arc::new(HealthChecker::new());
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        checker.register_probe(Arc::new(CountingProbe {
            name: "slow".to_string(),
            cadence: Duration::from_secs(10),
            delay: Duration::ZERO,
            counter: counter.clone(),
        }));

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        let _ = handle.await;

        let observed = counter.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            observed, 1,
            "expected exactly one probe call (the first-fill); got {observed}"
        );
    }

    #[tokio::test]
    async fn probes_run_concurrently_within_a_tick() {
        // Three probes that each sleep should overlap. Assert the
        // observed overlap directly instead of using wall-clock elapsed,
        // which is noisy on Windows test binaries.
        let checker = Arc::new(HealthChecker::new());
        let in_flight = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let max_in_flight = Arc::new(std::sync::atomic::AtomicU32::new(0));
        for name in ["a", "b", "c"] {
            checker.register_probe(Arc::new(ConcurrentProbe {
                name: name.to_string(),
                delay: Duration::from_millis(100),
                in_flight: in_flight.clone(),
                max_in_flight: max_in_flight.clone(),
            }));
        }

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        tokio::time::sleep(Duration::from_millis(300)).await;
        cancel.cancel();
        let _ = handle.await;

        let observed = max_in_flight.load(std::sync::atomic::Ordering::Acquire);
        assert!(
            observed >= 2,
            "expected overlapping probe execution, max in-flight was {observed}"
        );
    }

    #[tokio::test]
    async fn cancellation_stops_refresh_loop_promptly() {
        let checker = Arc::new(HealthChecker::new());
        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());

        // Let one tick land.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let started = Instant::now();
        cancel.cancel();
        let _ = handle.await;
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "refresh task did not shut down within 2 s of cancellation"
        );
    }

    /// A probe that panics on every call. Used to verify that a single
    /// bad probe doesn't tear down the refresh task or freeze the
    /// snapshot — it should be reported as Unhealthy and other probes
    /// should keep refreshing on their own cadences.
    struct PanickingProbe {
        name: String,
    }

    #[async_trait]
    impl HealthProbe for PanickingProbe {
        fn name(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.name)
        }
        fn cadence(&self) -> Duration {
            Duration::from_secs(60)
        }
        async fn probe(&self, _metrics: SystemMetricsSnapshot) -> ComponentHealth {
            panic!("synthetic probe panic");
        }
    }

    #[tokio::test]
    async fn panicking_probe_is_isolated_from_refresh_task() {
        let checker = Arc::new(HealthChecker::new());
        // Register one panicking probe and one well-behaved probe so we
        // can verify the latter still produces fresh values after the
        // former blows up.
        checker.register_probe(Arc::new(PanickingProbe {
            name: "bomb".to_string(),
        }));
        checker.register_probe(Arc::new(StaticTestProbe {
            name: "good",
            cadence: Duration::from_secs(60),
            health: ComponentHealth::healthy("good"),
        }));

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        // The first-fill triggers both probes; the panic happens inside
        // its spawned task, the good probe completes normally.
        tokio::time::sleep(Duration::from_millis(150)).await;

        let snap = checker.current();
        let bomb = snap
            .components
            .get("bomb")
            .expect("panicking probe should still produce a component entry");
        assert_eq!(
            bomb.status,
            HealthStatus::Unhealthy,
            "expected panic-translated Unhealthy, got {:?}: msg={:?}",
            bomb.status,
            bomb.message
        );
        let msg = bomb
            .message
            .as_ref()
            .expect("panic message should be populated");
        assert!(
            msg.contains("synthetic probe panic"),
            "expected panic payload in message, got {msg}"
        );

        let good = snap
            .components
            .get("good")
            .expect("well-behaved probe should have refreshed");
        assert_eq!(good.status, HealthStatus::Healthy);

        // The refresh task must still be alive — overall snapshot status
        // reflects the unhealthy probe, not a frozen pre-refresh state.
        assert_eq!(snap.status, HealthStatus::Unhealthy);

        cancel.cancel();
        let _ = handle.await;
    }

    #[test]
    fn test_disk_space_check_healthy() {
        let checker = HealthChecker::new();
        let health =
            checker.check_disk_space("/data", 50 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Healthy);
    }

    #[test]
    fn test_disk_space_check_warning() {
        let checker = HealthChecker::new();
        // 85% used = 15% available
        let health =
            checker.check_disk_space("/data", 15 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Degraded);
    }

    #[test]
    fn test_disk_space_check_critical() {
        let checker = HealthChecker::new();
        // 97% used = 3% available
        let health =
            checker.check_disk_space("/data", 3 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024);
        assert_eq!(health.status, HealthStatus::Unhealthy);
    }

    #[test]
    fn test_system_health_is_ready() {
        let health = SystemHealth {
            status: HealthStatus::Healthy,
            components: HashMap::new(),
            version: "0.1.0".to_string(),
            uptime_secs: 100,
            timestamp: chrono::Utc::now().to_rfc3339(),
            cpu_usage: 0.0,
            memory_usage: 0.0,
        };
        assert!(health.is_ready());
        assert!(health.is_healthy());

        let degraded = SystemHealth {
            status: HealthStatus::Degraded,
            ..health.clone()
        };
        assert!(degraded.is_ready());
        assert!(!degraded.is_healthy());

        let unhealthy = SystemHealth {
            status: HealthStatus::Unhealthy,
            ..health
        };
        assert!(!unhealthy.is_ready());
    }

    // ---------- output-root gate aggregation ----------

    #[tokio::test]
    async fn check_output_root_gate_reports_healthy_when_empty() {
        let gate = crate::downloader::OutputRootGate::new(
            std::sync::Weak::new(),
            Arc::new(|_: &std::path::Path| {}),
            vec![],
            Duration::from_secs(30),
        );
        let health = HealthChecker::check_output_root_gate(&gate);
        assert_eq!(health.status, HealthStatus::Healthy);
        assert_eq!(health.name, "output-root");
    }

    #[tokio::test]
    async fn check_output_root_gate_reports_degraded_with_message() {
        let gate = crate::downloader::OutputRootGate::new(
            std::sync::Weak::new(),
            Arc::new(|_: &std::path::Path| {}),
            vec![],
            Duration::from_secs(30),
        );
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such dir");
        gate.record_failure(std::path::Path::new("/rec/huya/X"), &err);

        let health = HealthChecker::check_output_root_gate(&gate);
        assert_eq!(health.status, HealthStatus::Degraded);
        let msg = health
            .message
            .expect("degraded component must have message");
        assert!(msg.contains("not_found"), "msg={}", msg);
        // Either "/rec" (2-component fallback) or "/rec/huya" is acceptable.
        // Normalize separators so the assertion works on Windows, where the
        // joined PathBuf renders as \rec\huya via Display.
        let msg_norm = msg.replace('\\', "/");
        assert!(msg_norm.contains("/rec"), "msg={}", msg);
    }

    #[tokio::test]
    async fn check_output_root_gate_reports_healthy_after_recovery() {
        let gate = crate::downloader::OutputRootGate::new(
            std::sync::Weak::new(),
            Arc::new(|_: &std::path::Path| {}),
            vec![],
            Duration::from_secs(30),
        );
        let err = std::io::Error::new(std::io::ErrorKind::StorageFull, "full");
        gate.record_failure(std::path::Path::new("/rec/X"), &err);
        assert_eq!(
            HealthChecker::check_output_root_gate(&gate).status,
            HealthStatus::Degraded
        );

        gate.mark_healthy(std::path::Path::new("/rec/X"));
        // mark_healthy spawns a tokio task for the recovery hook; the state
        // flip itself is synchronous so the snapshot is already Healthy.
        assert_eq!(
            HealthChecker::check_output_root_gate(&gate).status,
            HealthStatus::Healthy
        );
    }
}
