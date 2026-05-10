//! Health-check infrastructure.
//!
//! `/api/health` is polled every 10 s by the dashboard; previously each
//! poll re-ran every registered closure plus four `sysinfo` walks-of-the-world
//! (two `Disks::new_with_refreshed_list()`, two `System::new_with_specifics()`).
//! That was ~30 ms of work and ~30 small allocations per request, repeating
//! every 10 s, producing data that doesn't change between polls.
//!
//! The current shape is **snapshot-based**: registered probes run on their
//! own per-probe cadence in a single background task ([`HealthChecker::start`]),
//! merging results into one [`Arc<SystemHealth>`] held under a
//! `parking_lot::RwLock`. `/api/health` reads do one `Arc::clone` and never
//! touch the registered probes — request cost drops from ms-scale to
//! tens of nanoseconds, the `sysinfo` inventory is allocated once at
//! startup and refreshed in place, and expensive checks (disk inventory)
//! happen at 30 s cadence regardless of how often the dashboard polls.
//!
//! New code should implement [`HealthProbe`] directly; sync closures
//! work via [`HealthChecker::register_fn`] with an explicit cadence.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex as PlMutex;
use parking_lot::RwLock as PlRwLock;
use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::notification::NotificationEvent;

/// Refresh-task tick rate. Per-probe cadence is honored on top of this:
/// a probe with a 30 s cadence runs at most once every 30 s even though
/// the loop wakes every second.
const REFRESH_TICK: Duration = Duration::from_secs(1);

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

    /// Compute the latest value. Allowed to be expensive — only the
    /// background refresh task calls this, never the request path.
    async fn probe(&self) -> ComponentHealth;
}

/// Backwards-compatible closure-based health check function.
///
/// Prefer implementing [`HealthProbe`] directly for new code — the
/// trait gives you a name, cadence, and an async probe in one place.
/// This alias is kept for callers using [`HealthChecker::register_fn`]
/// with simple sync closures.
pub type HealthCheckFn = Arc<dyn Fn() -> ComponentHealth + Send + Sync>;

/// Adapts a sync closure to the async [`HealthProbe`] trait.
struct ClosureProbe {
    name: String,
    cadence: Duration,
    closure: HealthCheckFn,
}

#[async_trait]
impl HealthProbe for ClosureProbe {
    fn name(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.name)
    }

    fn cadence(&self) -> Duration {
        self.cadence
    }

    async fn probe(&self) -> ComponentHealth {
        (self.closure)()
    }
}

/// Internal bookkeeping for a registered probe.
struct RegisteredProbe {
    probe: Arc<dyn HealthProbe>,
    /// Last instant the probe was kicked off. `None` until the first
    /// run completes; protected by a `parking_lot::Mutex` so the refresh
    /// task can read/update it without crossing await boundaries.
    last_run: PlMutex<Option<Instant>>,
}

/// Long-lived `sysinfo` inventory shared across probes.
///
/// Allocated once at [`HealthChecker::new`] and refreshed in place by
/// the refresh task; before this lived, every `/api/health` read
/// rebuilt the disk and process inventories from scratch (4 walks of
/// the world per request).
pub struct SysinfoCache {
    /// CPU + memory inventory. Refresh with [`refresh_cpu_mem`].
    pub system: System,
    /// Mounted filesystems. Refresh with [`refresh_disks`].
    pub disks: sysinfo::Disks,
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
    pub fn refresh_cpu_mem(&mut self) {
        self.system.refresh_cpu_all();
        self.system.refresh_memory();
    }

    /// In-place refresh of the mounted-filesystem inventory.
    pub fn refresh_disks(&mut self) {
        self.disks.refresh(true);
    }

    /// Global CPU usage percent (0–100).
    pub fn cpu_usage(&self) -> f32 {
        self.system.global_cpu_usage()
    }

    /// Memory usage percent (0–100). Returns 0 when total is unknown.
    pub fn memory_usage_pct(&self) -> f32 {
        let total = self.system.total_memory();
        if total == 0 {
            0.0
        } else {
            (self.system.used_memory() as f64 / total as f64 * 100.0) as f32
        }
    }

    /// Pick the disk whose mount point contains `path`. Falls back to
    /// the longest matching mount point so a nested mount wins over its
    /// parent.
    pub fn best_disk_for_path(&self, path: &std::path::Path) -> Option<&sysinfo::Disk> {
        self.disks
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().to_string_lossy().len())
    }
}

impl Default for SysinfoCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Health checker for the system.
pub struct HealthChecker {
    /// Latest snapshot. `/api/health` reads do `Arc::clone` of this.
    snapshot: PlRwLock<Arc<SystemHealth>>,
    /// Registered probes. New probes can be added at any time.
    probes: PlRwLock<Vec<Arc<RegisteredProbe>>>,
    /// Shared `sysinfo` inventory. Locked across refresh ticks AND from
    /// inside probe closures that need disk/CPU data; using a sync
    /// `parking_lot::Mutex` (rather than `tokio::sync::Mutex`) lets the
    /// closures stay sync. Holding the lock across a `sysinfo` refresh
    /// (~ms) briefly blocks the runtime thread, which is acceptable at
    /// our cadences and avoids the alternative of making every probe
    /// async + handing the checker into every closure.
    sysinfo: PlMutex<SysinfoCache>,
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
    /// snapshot. Callers must invoke [`HealthChecker::start`] (after
    /// registering probes) to spawn the refresh task.
    pub fn new() -> Self {
        Self::with_thresholds(0.80, 0.95)
    }

    /// Construct with custom disk warning/critical thresholds.
    pub fn with_thresholds(disk_warning: f64, disk_critical: f64) -> Self {
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
            snapshot: PlRwLock::new(initial),
            probes: PlRwLock::new(Vec::new()),
            sysinfo: PlMutex::new(SysinfoCache::new()),
            start_time: Instant::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            disk_warning_threshold: disk_warning,
            disk_critical_threshold: disk_critical,
        }
    }

    /// Borrow the shared `sysinfo` cache. Probes that need disk or
    /// CPU/memory data should `lock()` this and call the `refresh_*`
    /// helpers (or just read fields — the refresh task refreshes
    /// CPU/memory every tick, and disk-touching probes typically run on
    /// 30 s cadence so they refresh themselves).
    pub fn sysinfo(&self) -> &PlMutex<SysinfoCache> {
        &self.sysinfo
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

    /// Register a [`HealthProbe`].
    pub async fn register_probe(&self, probe: Arc<dyn HealthProbe>) {
        let entry = Arc::new(RegisteredProbe {
            probe,
            last_run: PlMutex::new(None),
        });
        self.probes.write().push(entry);
    }

    /// Register a sync closure as a probe with a chosen cadence. The
    /// caller supplies the name explicitly so probe identity is stable
    /// across runs even if the closure errors before producing a
    /// [`ComponentHealth`].
    pub async fn register_fn(
        &self,
        name: impl Into<String>,
        cadence: Duration,
        closure: HealthCheckFn,
    ) {
        let probe: Arc<dyn HealthProbe> = Arc::new(ClosureProbe {
            name: name.into(),
            cadence,
            closure,
        });
        self.register_probe(probe).await;
    }

    /// Unregister a probe by name. Returns true if a probe was removed.
    pub async fn unregister(&self, name: &str) -> bool {
        let mut probes = self.probes.write();
        let len_before = probes.len();
        probes.retain(|rp| rp.probe.name().as_ref() != name);
        let removed = len_before != probes.len();
        drop(probes);
        if removed {
            // Drop the component from the live snapshot too so the UI
            // doesn't show a stale entry that will never refresh.
            let mut next: SystemHealth = (**self.snapshot.read()).clone();
            next.components.remove(name);
            next.status = compute_overall_status(&next.components);
            *self.snapshot.write() = Arc::new(next);
        }
        removed
    }

    /// Cheap snapshot read for the request path. One read-lock acquire
    /// + one `Arc::clone` — no probe calls, no allocation.
    pub fn current(&self) -> Arc<SystemHealth> {
        Arc::clone(&self.snapshot.read())
    }

    /// Backwards-compatible alias for [`Self::current`]. Existing
    /// callers expect an async API and a value-by-value `SystemHealth`;
    /// preserve that shape by cloning out of the Arc.
    pub async fn check_all(&self) -> SystemHealth {
        (*self.current()).clone()
    }

    /// Check readiness (for Kubernetes probes).
    pub async fn check_ready(&self) -> bool {
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
        let checker = Arc::clone(self);
        tokio::spawn(async move {
            // First fill: every probe is "due" because last_run is None,
            // so refresh_due() runs them all and populates the snapshot.
            checker.refresh_due().await;

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
                        checker.refresh_due().await;
                    }
                }
            }
        })
    }

    /// Run every probe whose cadence has elapsed (or that has never
    /// run), in parallel; merge the new component values into the
    /// snapshot atomically along with refreshed CPU/memory metrics.
    async fn refresh_due(&self) {
        let now = Instant::now();
        let due: Vec<Arc<RegisteredProbe>> = self
            .probes
            .read()
            .iter()
            .filter(|rp| {
                rp.last_run
                    .lock()
                    .map(|t| now.duration_since(t) >= rp.probe.cadence())
                    .unwrap_or(true)
            })
            .cloned()
            .collect();

        // Always refresh CPU/mem on every tick — they're cheap and the
        // top-level fields drive the dashboard headline numbers; users
        // expect them to update faster than per-component cadences.
        let (cpu_usage, memory_usage) = {
            let mut cache = self.sysinfo.lock();
            cache.refresh_cpu_mem();
            (cache.cpu_usage(), cache.memory_usage_pct())
        };

        let new_components: HashMap<String, ComponentHealth> = if due.is_empty() {
            // No probe is due; just bump CPU/mem and the timestamp.
            self.snapshot.read().components.clone()
        } else {
            let results = futures::future::join_all(due.iter().map(|rp| {
                let rp = Arc::clone(rp);
                async move {
                    let started = Instant::now();
                    let mut health = rp.probe.probe().await;
                    health.check_duration_ms = Some(started.elapsed().as_millis() as u64);
                    *rp.last_run.lock() = Some(now);
                    health
                }
            }))
            .await;

            let mut components = self.snapshot.read().components.clone();
            for (rp, health) in due.iter().zip(results.into_iter()) {
                components.insert(rp.probe.name().into_owned(), health);
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
        *self.snapshot.write() = new_snapshot;
    }

    /// Check disk space and return health status.
    pub fn check_disk_space(&self, path: &str, available: u64, total: u64) -> ComponentHealth {
        if total == 0 {
            return ComponentHealth::unhealthy("disk", "Unable to determine disk space");
        }

        let used_ratio = 1.0 - (available as f64 / total as f64);

        if used_ratio >= self.disk_critical_threshold {
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
        } else if used_ratio >= self.disk_warning_threshold {
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
        let health = checker.check_all().await;
        assert!(health.components.is_empty());
        // Initial snapshot is Unknown until the first refresh runs.
        assert_eq!(health.status, HealthStatus::Unknown);
    }

    #[tokio::test]
    async fn test_health_checker_register_then_refresh() {
        let checker = Arc::new(HealthChecker::new());
        checker
            .register_fn(
                "test",
                Duration::from_secs(5),
                Arc::new(|| ComponentHealth::healthy("test")),
            )
            .await;

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        // First-fill is synchronous-ish: yield once and the immediate
        // pre-tick refresh has populated the snapshot.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let health = checker.check_all().await;
        assert!(health.components.contains_key("test"));
        assert_eq!(health.status, HealthStatus::Healthy);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_health_checker_unhealthy_component() {
        let checker = Arc::new(HealthChecker::new());
        checker
            .register_fn(
                "failing",
                Duration::from_secs(5),
                Arc::new(|| ComponentHealth::unhealthy("failing", "Error")),
            )
            .await;

        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        tokio::time::sleep(Duration::from_millis(50)).await;

        let health = checker.check_all().await;
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
        async fn probe(&self) -> ComponentHealth {
            tokio::time::sleep(self.delay).await;
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            ComponentHealth::healthy(self.name.clone())
        }
    }

    #[tokio::test]
    async fn cadence_is_honored_between_ticks() {
        // 10-second cadence; refresh ticks every second. Over a 200 ms
        // observation window we should see exactly one probe call (the
        // initial first-fill), not 200 of them.
        let checker = Arc::new(HealthChecker::new());
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        checker
            .register_probe(Arc::new(CountingProbe {
                name: "slow".to_string(),
                cadence: Duration::from_secs(10),
                delay: Duration::ZERO,
                counter: counter.clone(),
            }))
            .await;

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
        // Three probes that each sleep 100 ms. Sequential execution
        // would take 300 ms; parallel should be ~100 ms.
        let checker = Arc::new(HealthChecker::new());
        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        for name in ["a", "b", "c"] {
            checker
                .register_probe(Arc::new(CountingProbe {
                    name: name.to_string(),
                    cadence: Duration::from_secs(60),
                    delay: Duration::from_millis(100),
                    counter: counter.clone(),
                }))
                .await;
        }

        let cancel = CancellationToken::new();
        let started = Instant::now();
        let handle = checker.start(cancel.child_token());
        // Wait for the first-fill to complete (3 × 100 ms in parallel
        // ≈ 100 ms; pad to 200 ms).
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        let _ = handle.await;
        let elapsed = started.elapsed();

        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            3,
            "all three probes should have run on first-fill"
        );
        assert!(
            elapsed < Duration::from_millis(280),
            "probes ran sequentially: total elapsed {:?}",
            elapsed
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

    #[tokio::test]
    async fn unregister_removes_component_from_snapshot() {
        let checker = Arc::new(HealthChecker::new());
        checker
            .register_fn(
                "doomed",
                Duration::from_secs(5),
                Arc::new(|| ComponentHealth::healthy("doomed")),
            )
            .await;
        let cancel = CancellationToken::new();
        let handle = checker.start(cancel.child_token());
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(checker.current().components.contains_key("doomed"));
        assert!(checker.unregister("doomed").await);
        assert!(!checker.current().components.contains_key("doomed"));

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
