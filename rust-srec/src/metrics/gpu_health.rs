//! GPU health monitor.
//!
//! Background poller that probes `nvidia-smi` on a configurable cadence and
//! exposes a single `gpu` [`ComponentHealth`] entry through the
//! [`HealthChecker`](crate::metrics::HealthChecker). Designed to surface the
//! well-known **NVIDIA Container
//! Toolkit + cgroup-v2 reconciliation** failure pattern: the host's
//! `systemd` reloads the device cgroup (often during a Docker daemon
//! reload or `nvidia-ctk` reconfigure) and silently strips the running
//! container's GPU access. `/dev/nvidia*` nodes remain visible inside the
//! namespace, so the container *looks* fine, but the next `cuInit()` call
//! returns `CUDA_ERROR_NO_DEVICE` and NVML reports
//! `Failed to initialize NVML: Unknown Error`.
//!
//! The monitor mirrors the architecture of the output-root write gate
//! ([`crate::downloader::OutputRootGate`]):
//!
//! - Lock-free `AtomicU8` for the `Healthy` / `Unhealthy` / `Unknown` state
//!   so transition detection is a single `compare_exchange`.
//! - `ArcSwap<GpuSnapshot>` for the rich snapshot. Reads on the
//!   `/api/health` hot path are an `Arc::clone` plus a cheap struct copy
//!   of the pre-built [`ComponentHealth`] — no formatting and no
//!   allocation in the read path.
//! - `Weak<NotificationService>` so the monitor never extends the
//!   service's lifetime and there are no `Arc` cycles.
//! - One [`tokio::spawn`]'d probe loop, supervised by a
//!   [`tokio_util::sync::CancellationToken`], idle 99% of the time.
//!
//! Notification semantics intentionally match `OutputPathInaccessible`: a
//! single [`NotificationEvent::GpuUnavailable`] dispatch on the
//! `Healthy → Unhealthy` transition; recovery is silent.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use chrono::Utc;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::metrics::{ComponentHealth, HealthStatus};
use crate::notification::{NotificationEvent, NotificationService};

/// Each probe call is bounded by this timeout.
const PROBE_TIMEOUT_SECS: u64 = 5;
/// Truncation cap for stderr/stdout captured into the snapshot. Keeps the
/// snapshot footprint bounded if `nvidia-smi` ever emits a long error.
const MAX_DIAG_CHARS: usize = 256;
/// Minimum probe interval. Sub-second polling is rejected because each
/// probe is a `nvidia-smi` fork+exec (~50–200 ms).
const MIN_INTERVAL_SECS: u64 = 1;
/// Default probe interval. Matches
/// [`crate::downloader::DEFAULT_GATE_COOLDOWN_SECS`] so operators only
/// need to remember one cadence.
pub const DEFAULT_PROBE_INTERVAL_SECS: u64 = 30;
/// Bound for the startup `--version` gate that decides whether to
/// register the monitor at all.
const STARTUP_GATE_TIMEOUT_SECS: u64 = 2;
/// Component name used in `/api/health`. Must match the frontend
/// formatter (`formatComponentName('gpu')`).
const COMPONENT_NAME: &str = "gpu";

/// Stable string identifiers for GPU probe failure modes.
///
/// The `as_str` values are used as i18n discriminator keys
/// (`notification.gpu_unavailable.description.<kind>`) and as the
/// `kind=` field in tracing logs. Adding a variant requires adding the
/// matching key to `locales/{en,zh-CN}.yml` and a branch to the
/// `description()` arm in `notification/events.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuErrorKind {
    /// `Failed to initialize NVML: Unknown Error` — the cgroup-wipe
    /// signature described in the module docs.
    NvmlUnknownError,
    /// `Driver/library version mismatch` — host driver was upgraded
    /// without restarting `nvidia-uvm` or the container.
    DriverMismatch,
    /// `No devices were found` / `No CUDA-capable device is detected`.
    NoDevice,
    /// Our `PROBE_TIMEOUT_SECS` timeout fired before `nvidia-smi`
    /// returned. Often indicates a hung NVML call.
    TimedOut,
    /// `nvidia-smi` not found at exec time. Defensive: registration
    /// gates on a startup probe, but the binary could disappear later.
    NotInstalled,
    /// Catch-all.
    Other,
}

impl GpuErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NvmlUnknownError => "nvml_unknown_error",
            Self::DriverMismatch => "driver_mismatch",
            Self::NoDevice => "no_device",
            Self::TimedOut => "timed_out",
            Self::NotInstalled => "not_installed",
            Self::Other => "other",
        }
    }

    /// Classify a failed probe based on its stderr text.
    fn classify(stderr: &str) -> Self {
        if stderr.contains("Failed to initialize NVML: Unknown Error") {
            Self::NvmlUnknownError
        } else if stderr.contains("Driver/library version mismatch") {
            Self::DriverMismatch
        } else if stderr.contains("No devices were found")
            || stderr.contains("No CUDA-capable device")
            || stderr.contains("no CUDA-capable device is detected")
        {
            Self::NoDevice
        } else {
            Self::Other
        }
    }
}

/// State value stored in the `AtomicU8`. Numeric encoding is stable; do
/// not reorder or insert in the middle.
const STATE_UNKNOWN: u8 = 0;
const STATE_HEALTHY: u8 = 1;
const STATE_UNHEALTHY: u8 = 2;

/// Failure metadata threaded into the snapshot when the state is
/// [`HealthStatus::Unhealthy`]. Kept separate from the human-readable
/// `ComponentHealth::message` so the notification dispatcher can read
/// the kind directly without parsing the message string.
///
/// Module-private: the only consumer is [`GpuHealthMonitor::spawn_notification`].
#[derive(Debug, Clone)]
struct GpuFailure {
    kind: GpuErrorKind,
    /// Truncated diagnostic line for the operator. Bounded by
    /// [`MAX_DIAG_CHARS`].
    message: String,
}

/// Pre-built snapshot returned to the `/api/health` hot path. Cheap to
/// clone via [`Arc`].
#[derive(Debug, Clone)]
pub struct GpuSnapshot {
    /// Ready-to-clone component health entry. Allocated **once per
    /// probe** (every 30 s by default), then cloned on every
    /// `/api/health` read (every 10 s) — no formatting or allocation
    /// happens in the read path.
    pub health: ComponentHealth,
    /// Present iff the snapshot represents an unhealthy state.
    /// Module-private: read only by `spawn_notification`.
    failure: Option<GpuFailure>,
}

impl GpuSnapshot {
    fn unknown() -> Self {
        Self {
            health: ComponentHealth {
                name: COMPONENT_NAME.to_string(),
                status: HealthStatus::Unknown,
                message: Some("GPU health probe has not run yet".to_string()),
                last_check: None,
                check_duration_ms: None,
            },
            failure: None,
        }
    }
}

/// Background poller. Construct via [`GpuHealthMonitor::detect`] and
/// drive with [`GpuHealthMonitor::start`].
pub struct GpuHealthMonitor {
    interval_secs: AtomicU64,
    state: AtomicU8,
    snapshot: ArcSwap<GpuSnapshot>,
    notification_service: Weak<NotificationService>,
}

impl GpuHealthMonitor {
    /// Probe `nvidia-smi --version` with a short timeout. Returns
    /// `Some(monitor)` only when the binary is on `$PATH` and exits 0.
    /// Callers should skip registration when this returns `None`.
    ///
    /// Two-stage gate: a pure-`stat()` `$PATH` walk first short-circuits
    /// the no-GPU majority (~10 µs of syscalls vs. a ~1–5 ms
    /// `posix_spawn` of a binary that doesn't exist), and only when the
    /// binary is present do we actually spawn `--version` to confirm it
    /// runs (catches "binary on PATH but driver missing" cases too).
    pub async fn detect(
        notification_service: Weak<NotificationService>,
        initial_interval_secs: u64,
    ) -> Option<Arc<Self>> {
        if !binary_on_path("nvidia-smi") {
            debug!("GpuHealthMonitor: nvidia-smi not on PATH, skipping registration");
            return None;
        }

        let probe = tokio::time::timeout(
            Duration::from_secs(STARTUP_GATE_TIMEOUT_SECS),
            process_utils::tokio_command("nvidia-smi")
                .arg("--version")
                .kill_on_drop(true)
                .output(),
        )
        .await;

        match probe {
            Ok(Ok(out)) if out.status.success() => {
                let interval = initial_interval_secs.max(MIN_INTERVAL_SECS);
                Some(Arc::new(Self {
                    interval_secs: AtomicU64::new(interval),
                    state: AtomicU8::new(STATE_UNKNOWN),
                    snapshot: ArcSwap::from_pointee(GpuSnapshot::unknown()),
                    notification_service,
                }))
            }
            Ok(Ok(out)) => {
                debug!(
                    code = out.status.code().unwrap_or(-1),
                    "GpuHealthMonitor: nvidia-smi --version exited non-zero, skipping registration"
                );
                None
            }
            Ok(Err(e)) => {
                debug!(
                    error = %e,
                    "GpuHealthMonitor: nvidia-smi disappeared between PATH check and exec, skipping registration"
                );
                None
            }
            Err(_) => {
                debug!(
                    timeout_secs = STARTUP_GATE_TIMEOUT_SECS,
                    "GpuHealthMonitor: nvidia-smi --version timed out, skipping registration"
                );
                None
            }
        }
    }

    /// Update the probe interval. The change applies on the next tick;
    /// no restart required. Sub-second values are clamped to
    /// `MIN_INTERVAL_SECS`.
    pub fn set_interval(&self, secs: u64) {
        let clamped = secs.max(MIN_INTERVAL_SECS);
        let prev = self.interval_secs.swap(clamped, Ordering::AcqRel);
        if prev != clamped {
            info!(
                previous = prev,
                next = clamped,
                "GpuHealthMonitor: probe interval updated"
            );
        }
    }

    /// Cheap snapshot read for the [`crate::metrics::HealthChecker`]
    /// closure. One `Arc::clone`; no allocation.
    pub fn snapshot(&self) -> Arc<GpuSnapshot> {
        self.snapshot.load_full()
    }

    /// Spawn the probe loop. Returns the [`JoinHandle`]; cancellation is
    /// driven by the supplied [`CancellationToken`].
    pub fn start(self: &Arc<Self>, cancel: CancellationToken) -> JoinHandle<()> {
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            // First probe immediately so /api/health is populated within
            // seconds of startup, not 30 s later.
            monitor.probe_once().await;

            let mut last_interval = monitor.interval_secs.load(Ordering::Acquire);
            let mut ticker = build_interval(last_interval);

            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        debug!("GpuHealthMonitor: cancellation token fired, exiting probe loop");
                        return;
                    }
                    _ = ticker.tick() => {
                        let current = monitor.interval_secs.load(Ordering::Acquire);
                        if current != last_interval {
                            ticker = build_interval(current);
                            last_interval = current;
                        }
                        monitor.probe_once().await;
                    }
                }
            }
        })
    }

    async fn probe_once(self: &Arc<Self>) {
        let (new_state, snapshot) = self.run_probe().await;
        // Swap the snapshot before the state flip so any concurrent
        // /api/health read sees the new payload alongside the old state at
        // worst — never the new state with a stale payload.
        self.snapshot.store(Arc::new(snapshot));
        let prev_state = self.state.swap(new_state, Ordering::AcqRel);
        if prev_state == STATE_HEALTHY && new_state == STATE_UNHEALTHY {
            self.spawn_notification();
        } else if prev_state == STATE_UNHEALTHY && new_state == STATE_HEALTHY {
            info!("GpuHealthMonitor: GPU recovered (Unhealthy -> Healthy)");
        }
    }

    async fn run_probe(&self) -> (u8, GpuSnapshot) {
        let started = std::time::Instant::now();
        let exec = tokio::time::timeout(
            Duration::from_secs(PROBE_TIMEOUT_SECS),
            process_utils::tokio_command("nvidia-smi")
                .args([
                    "--query-gpu=name,driver_version,utilization.gpu,memory.used,memory.total,temperature.gpu",
                    "--format=csv,noheader,nounits",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await;

        let now = Some(Utc::now().to_rfc3339());
        let dur_ms = Some(started.elapsed().as_millis() as u64);

        let build_health = |status, message: String| ComponentHealth {
            name: COMPONENT_NAME.to_string(),
            status,
            message: Some(message),
            last_check: now.clone(),
            check_duration_ms: dur_ms,
        };

        match exec {
            Err(_) => {
                let kind = GpuErrorKind::TimedOut;
                let detail = format!("GPU probe timed out after {PROBE_TIMEOUT_SECS}s");
                (
                    STATE_UNHEALTHY,
                    GpuSnapshot {
                        health: build_health(
                            HealthStatus::Unhealthy,
                            format!("{detail} (kind={})", kind.as_str()),
                        ),
                        failure: Some(GpuFailure {
                            kind,
                            message: detail,
                        }),
                    },
                )
            }
            Ok(Err(e)) => {
                let kind = if e.kind() == std::io::ErrorKind::NotFound {
                    GpuErrorKind::NotInstalled
                } else {
                    GpuErrorKind::Other
                };
                let detail = truncate(&e.to_string(), MAX_DIAG_CHARS);
                (
                    STATE_UNHEALTHY,
                    GpuSnapshot {
                        health: build_health(
                            HealthStatus::Unhealthy,
                            format!(
                                "GPU probe failed to spawn (kind={}): {detail}",
                                kind.as_str()
                            ),
                        ),
                        failure: Some(GpuFailure {
                            kind,
                            message: detail,
                        }),
                    },
                )
            }
            Ok(Ok(out)) if out.status.success() => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let summary = stdout
                    .lines()
                    .next()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "GPU healthy (no devices reported)".to_string());
                (
                    STATE_HEALTHY,
                    GpuSnapshot {
                        health: build_health(
                            HealthStatus::Healthy,
                            truncate(&summary, MAX_DIAG_CHARS),
                        ),
                        failure: None,
                    },
                )
            }
            Ok(Ok(out)) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let kind = GpuErrorKind::classify(&stderr);
                let detail = truncate(&stderr, MAX_DIAG_CHARS);
                (
                    STATE_UNHEALTHY,
                    GpuSnapshot {
                        health: build_health(
                            HealthStatus::Unhealthy,
                            format!("GPU probe failed (kind={}): {detail}", kind.as_str()),
                        ),
                        failure: Some(GpuFailure {
                            kind,
                            message: detail,
                        }),
                    },
                )
            }
        }
    }

    fn spawn_notification(self: &Arc<Self>) {
        let weak = self.notification_service.clone();
        // Read the failure metadata from the snapshot we just wrote.
        // probe_once() always populates `failure` before flipping the
        // state to Unhealthy, so this `expect` is structurally sound; an
        // empty failure here would be a programming error.
        let failure = self
            .snapshot()
            .failure
            .clone()
            .expect("Unhealthy state requires a populated failure field");

        // Fire-and-forget. Notification delivery must never block the
        // probe loop or the download path; failures are logged at WARN
        // (matches OutputRootGate::spawn_notification).
        tokio::spawn(async move {
            let Some(service) = weak.upgrade() else {
                debug!("NotificationService dropped before GpuUnavailable notification could fire");
                return;
            };
            let event = NotificationEvent::GpuUnavailable {
                error_kind: failure.kind.as_str().to_string(),
                message: failure.message.clone(),
                timestamp: Utc::now(),
            };
            if let Err(e) = service.notify(event).await {
                warn!(
                    error = %e,
                    kind = failure.kind.as_str(),
                    "Failed to dispatch GpuUnavailable notification (non-fatal)"
                );
            }
        });
    }
}

/// Walk `$PATH` looking for `name` as an existing executable. Pure
/// `stat()` calls, no fork/exec, so cheap enough to short-circuit
/// `detect()` on hosts without `nvidia-smi` (the no-GPU majority) and
/// avoid paying process-spawn cost for a missing binary on every
/// container start.
///
/// Generic over the binary name so unit tests can exercise it against
/// something always-present (e.g. `cargo` in CI).
fn binary_on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let candidates = executable_candidates(name);
    std::env::split_paths(&path).any(|dir| {
        candidates
            .iter()
            .any(|candidate| dir.join(candidate).is_file())
    })
}

#[cfg(windows)]
fn executable_candidates(name: &str) -> Vec<OsString> {
    if std::path::Path::new(name).extension().is_some() {
        return vec![OsString::from(name)];
    }

    let mut candidates = vec![OsString::from(name)];
    let pathext =
        std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".COM;.EXE;.BAT;.CMD"));
    candidates.extend(
        pathext
            .to_string_lossy()
            .split(';')
            .map(str::trim)
            .filter(|ext| !ext.is_empty())
            .map(|ext| {
                let mut candidate = OsString::from(name);
                candidate.push(ext);
                candidate
            }),
    );
    candidates
}

#[cfg(not(windows))]
fn executable_candidates(name: &str) -> Vec<OsString> {
    vec![OsString::from(name)]
}

/// Builds an interval whose first tick occurs after the full period.
///
/// Missed ticks are skipped so a slow probe never queues up a backlog.
fn build_interval(secs: u64) -> tokio::time::Interval {
    let period = Duration::from_secs(secs.max(MIN_INTERVAL_SECS));
    let start = tokio::time::Instant::now() + period;
    let mut ticker = tokio::time::interval_at(start, period);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker
}

/// Trim trailing whitespace (nvidia-smi output ends in `\n`) and clamp
/// to at most `MAX_DIAG_CHARS` so an unbounded driver error can never
/// blow up the snapshot footprint.
fn truncate(s: &str, max: usize) -> String {
    crate::utils::text::truncate_chars(s.trim(), max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognizes_nvml_unknown_error() {
        let stderr = "Failed to initialize NVML: Unknown Error\n";
        assert_eq!(
            GpuErrorKind::classify(stderr),
            GpuErrorKind::NvmlUnknownError
        );
    }

    #[test]
    fn classify_recognizes_driver_mismatch() {
        let stderr = "Failed to initialize NVML: Driver/library version mismatch\n";
        assert_eq!(GpuErrorKind::classify(stderr), GpuErrorKind::DriverMismatch);
    }

    #[test]
    fn classify_recognizes_no_device_variants() {
        assert_eq!(
            GpuErrorKind::classify("No devices were found\n"),
            GpuErrorKind::NoDevice
        );
        assert_eq!(
            GpuErrorKind::classify("no CUDA-capable device is detected"),
            GpuErrorKind::NoDevice
        );
        assert_eq!(
            GpuErrorKind::classify("No CUDA-capable device"),
            GpuErrorKind::NoDevice
        );
    }

    #[test]
    fn classify_falls_back_to_other() {
        assert_eq!(
            GpuErrorKind::classify("anything else entirely"),
            GpuErrorKind::Other
        );
    }

    #[test]
    fn error_kind_strings_are_stable() {
        // Locked in because they are i18n discriminator keys; changing
        // any of these without updating events.rs and the locale files
        // breaks notification description rendering.
        assert_eq!(
            GpuErrorKind::NvmlUnknownError.as_str(),
            "nvml_unknown_error"
        );
        assert_eq!(GpuErrorKind::DriverMismatch.as_str(), "driver_mismatch");
        assert_eq!(GpuErrorKind::NoDevice.as_str(), "no_device");
        assert_eq!(GpuErrorKind::TimedOut.as_str(), "timed_out");
        assert_eq!(GpuErrorKind::NotInstalled.as_str(), "not_installed");
        assert_eq!(GpuErrorKind::Other.as_str(), "other");
    }

    #[test]
    fn unknown_snapshot_is_used_before_first_probe() {
        let snap = GpuSnapshot::unknown();
        assert_eq!(snap.health.status, HealthStatus::Unknown);
        assert_eq!(snap.health.name, COMPONENT_NAME);
        assert!(snap.failure.is_none());
    }

    #[test]
    fn truncate_keeps_short_strings_intact() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("  whitespace  ", 32), "whitespace");
    }

    #[test]
    fn truncate_caps_long_strings_with_ellipsis() {
        let long = "a".repeat(300);
        let t = truncate(&long, 256);
        assert_eq!(t.chars().count(), 257); // 256 chars + ellipsis
        assert!(t.ends_with('…'));
    }

    #[test]
    fn default_interval_matches_gate_cooldown() {
        // Sanity-check: keep our default in lockstep with the
        // output-root write gate so operators only need one cadence
        // value in their head.
        assert_eq!(
            DEFAULT_PROBE_INTERVAL_SECS,
            crate::downloader::DEFAULT_GATE_COOLDOWN_SECS
        );
    }

    #[tokio::test(start_paused = true)]
    async fn interval_waits_full_period_before_first_tick() {
        let started = tokio::time::Instant::now();
        let mut ticker = build_interval(30);
        let tick = tokio::spawn(async move { ticker.tick().await });

        tokio::task::yield_now().await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_secs(29)).await;
        assert!(!tick.is_finished());

        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(tick.await.unwrap().duration_since(started).as_secs(), 30);
    }

    #[test]
    fn binary_on_path_rejects_clearly_missing_names() {
        // A name with embedded NUL-style nonsense or absurd length is
        // guaranteed to not resolve. Any false positive here would
        // indicate a bug in the PATH-walk logic.
        assert!(!binary_on_path(
            "this-binary-definitely-does-not-exist-anywhere-on-path-9f3c2d1e"
        ));
    }

    #[test]
    fn binary_on_path_finds_existing_binary() {
        // `cargo` is always on PATH inside `cargo test`; Windows exposes
        // it as `cargo.exe`, which exercises PATHEXT handling.
        assert!(
            binary_on_path("cargo"),
            "expected `cargo` to be discoverable on $PATH inside cargo test"
        );
    }
}
