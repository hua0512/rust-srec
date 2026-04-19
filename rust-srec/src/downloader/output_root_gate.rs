//! Output-root write gate.
//!
//! Pauses recordings at the filesystem boundary when an output root becomes
//! unwritable, instead of letting the failure cascade through the engine
//! retry path → circuit breaker → DB error outbox.
//!
//! ## Design summary
//!
//! - **Per-root state machine.** Each unique output root (e.g. `/rec`) is
//!   tracked by a [`RootEntry`] with two states: `Healthy` and `Degraded`.
//! - **Lock-free fast path.** [`OutputRootGate::check`] on a Healthy root is
//!   a `DashMap::get` plus a single `AtomicU8::load`. No mutex on the hot path.
//! - **Single-flight cooldown via CAS.** When a root is Degraded, exactly one
//!   caller per cooldown period is allowed through to attempt the real
//!   `ensure_output_dir`. Other concurrent callers fast-reject with the cached
//!   error. The atomic-u64 `compare_exchange` on `last_attempt_unix` is the
//!   single-flight lock — there is no separate "Probing" state.
//! - **No background probe task.** The real `ensure_output_dir` call by the
//!   download manager *is* the probe. Mirrors the half-open pattern in
//!   [`crate::downloader::resilience::CircuitBreaker`].
//! - **One notification per transition.** The CAS that flips state from
//!   `Healthy → Degraded` is the same atomic that decides who emits the
//!   notification. No double-fire even under massive concurrent failure.
//! - **No Arc cycles.** The gate holds [`Weak<NotificationService>`] and
//!   upgrades per emit, so gate/service lifetimes stay independent.
//!
//! ## What the gate does NOT do
//!
//! - It does not auto-recover stale Docker bind mounts. Recovery from that
//!   failure mode requires `CAP_SYS_ADMIN` in the host mount namespace,
//!   which an unprivileged container does not have. The gate stays Degraded
//!   until the container is restarted; at that point the gate state is
//!   in-memory only and is rebuilt from scratch.
//! - It does not run a background probe. See above.
//! - It does not reset engine circuit breakers explicitly. Engine breakers
//!   recover via their own half-open mechanism on the first download attempt
//!   after the gate flips healthy.
//!
//! ## Recovery hook
//!
//! On `Degraded → Healthy` transition the gate invokes a caller-injected
//! [`RecoveryHook`] closure that knows how to clear streamer state. This keeps
//! the gate ignorant of `StreamerManager` and avoids the dependency cycle
//! that would otherwise arise (gate is in `downloader::`, manager is in
//! `streamer::`, container wires both).

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::Weak;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Utc;
use dashmap::DashMap;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::downloader::engine::IoErrorKindSer;
use crate::notification::events::NotificationEvent;
use crate::notification::service::NotificationService;

/// Default cooldown between probe attempts on a degraded root.
///
/// Picked to roughly match the streamer error-backoff floor (60s for
/// `error_count == 3`) so a streamer that gets gate-blocked and rescheduled
/// via `set_infra_blocked` doesn't waste cycles on a gate that is still
/// inside its cooldown window.
pub const DEFAULT_GATE_COOLDOWN_SECS: u64 = 30;

/// `last_error` field prefix written by `set_infra_blocked` for the
/// `OutputRootUnavailable` reason. The recovery hook uses this prefix to
/// identify streamers whose backoff was caused by the gate (vs. unrelated
/// CDN/network failures) so it only resets the streamers it should.
pub const LAST_ERROR_GATE_PREFIX: &str = "output-root blocked:";

const STATE_HEALTHY: u8 = 0;
const STATE_DEGRADED: u8 = 1;

/// Closure called by [`OutputRootGate::mark_healthy`] when a root transitions
/// from `Degraded` back to `Healthy`. The closure is responsible for clearing
/// any streamer-level backoff that was caused by the gate. The gate itself
/// does not depend on `StreamerManager`; the closure is wired up in
/// `services::container` where both sides are in scope.
pub type RecoveryHook = Arc<dyn Fn(&Path) + Send + Sync>;

/// Error returned by [`OutputRootGate::check`] when the gate has blocked a
/// download from starting.
#[derive(Debug, Clone, thiserror::Error)]
#[error("output root {} is unwritable ({}): {message}", .root.display(), .kind.as_str())]
pub struct GateBlocked {
    /// The resolved root that is in the Degraded state.
    pub root: PathBuf,
    /// Classified io::ErrorKind of the most recent failure on this root.
    pub kind: IoErrorKindSer,
    /// Human-readable message from the most recent failure (passed through
    /// from the original `io::Error::to_string()`).
    pub message: String,
}

/// Snapshot of one tracked root, used by the `/health` endpoint.
#[derive(Debug, Clone)]
pub struct RootHealth {
    pub root: PathBuf,
    pub state: RootHealthState,
    /// Wall-clock seconds since the most recent attempt against this root.
    /// `None` when the gate has been Healthy and never recorded a failure.
    pub seconds_since_last_attempt: Option<u64>,
    /// Number of downloads rejected since the most recent transition into
    /// Degraded. Resets to zero on `mark_healthy`.
    pub rejected_count: u64,
    /// `Some(kind, msg)` when Degraded; `None` when Healthy.
    pub last_error: Option<(IoErrorKindSer, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootHealthState {
    Healthy,
    Degraded,
}

#[derive(Debug)]
struct RootEntry {
    /// `STATE_HEALTHY` or `STATE_DEGRADED`. Read on the hot path.
    state: AtomicU8,
    /// Unix seconds of the most recent attempt against this root. Updated via
    /// `compare_exchange` to claim the single-flight probe slot.
    last_attempt_unix: AtomicU64,
    /// Cold metadata. Only touched on transitions, snapshots, and the slow
    /// reject path (where one extra mutex acquisition is fine).
    meta: Mutex<RootMeta>,
}

#[derive(Debug)]
struct RootMeta {
    since_unix: u64,
    last_error_kind: IoErrorKindSer,
    last_error_msg: String,
    rejected_count: u64,
}

impl RootEntry {
    fn new_degraded(kind: IoErrorKindSer, msg: String) -> Self {
        let now = unix_now();
        Self {
            state: AtomicU8::new(STATE_DEGRADED),
            last_attempt_unix: AtomicU64::new(now),
            meta: Mutex::new(RootMeta {
                since_unix: now,
                last_error_kind: kind,
                last_error_msg: msg,
                rejected_count: 0,
            }),
        }
    }
}

/// The output-root write gate.
///
/// See module-level docs for the design summary. Construct one per
/// [`crate::downloader::manager::DownloadManager`] and share via `Arc`.
pub struct OutputRootGate {
    roots: DashMap<PathBuf, Arc<RootEntry>>,
    notification_service: Weak<NotificationService>,
    recovery_hook: RecoveryHook,
    configured_roots: Vec<PathBuf>,
    cooldown: Duration,
}

impl OutputRootGate {
    /// Construct a new gate.
    ///
    /// `configured_roots` is the optional list of explicit root paths from the
    /// `RUST_SREC_OUTPUT_ROOTS` env var. When unset, [`resolve_root`] falls
    /// back to a 2-component heuristic. Paths are normalized before being
    /// stored so `/rec`, `/rec/`, and `/rec/./` all hash to the same key.
    pub fn new(
        notification_service: Weak<NotificationService>,
        recovery_hook: RecoveryHook,
        configured_roots: Vec<PathBuf>,
        cooldown: Duration,
    ) -> Arc<Self> {
        let normalized: Vec<PathBuf> = configured_roots
            .into_iter()
            .map(|p| normalize_root(&p))
            .collect();
        Arc::new(Self {
            roots: DashMap::new(),
            notification_service,
            recovery_hook,
            configured_roots: normalized,
            cooldown,
        })
    }

    /// Hot-path check called before every download `prepare_output_dir`.
    ///
    /// Returns `Ok(())` if the caller may proceed to the real `ensure_output_dir`
    /// (root is Healthy, or Degraded with cooldown elapsed and we won the
    /// single-flight CAS). Returns `Err(GateBlocked)` if the caller should
    /// fast-reject without touching the filesystem or the engine.
    pub fn check(&self, output_dir: &Path) -> Result<(), GateBlocked> {
        // Fast path: if the gate has never recorded a failure, the map is
        // empty and we can return without resolving the root or hashing
        // anything. This is the steady state for healthy systems.
        if self.roots.is_empty() {
            return Ok(());
        }

        let root = resolve_root(output_dir, &self.configured_roots);
        let entry = match self.roots.get(&root) {
            Some(e) => e.clone(),
            None => return Ok(()),
        };

        if entry.state.load(Ordering::Acquire) == STATE_HEALTHY {
            return Ok(());
        }

        // Degraded path. Decide whether to allow this caller through (winning
        // the single-flight CAS) or fast-reject.
        let now = unix_now();
        let last = entry.last_attempt_unix.load(Ordering::Acquire);
        let cooldown_secs = self.cooldown.as_secs().max(1);

        if now.saturating_sub(last) < cooldown_secs {
            // Still inside cooldown — fast-reject.
            return Err(self.build_blocked(&root, &entry));
        }

        // Cooldown elapsed. Try to claim the probe slot via CAS. Exactly one
        // caller wins regardless of how many arrive simultaneously.
        match entry.last_attempt_unix.compare_exchange(
            last,
            now,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                debug!(
                    root = %root.display(),
                    "OutputRootGate single-flight probe slot claimed; allowing real ensure_output_dir"
                );
                Ok(())
            }
            Err(_) => Err(self.build_blocked(&root, &entry)),
        }
    }

    fn build_blocked(&self, root: &Path, entry: &Arc<RootEntry>) -> GateBlocked {
        let mut meta = entry.meta.lock();
        meta.rejected_count = meta.rejected_count.saturating_add(1);
        GateBlocked {
            root: root.to_path_buf(),
            kind: meta.last_error_kind,
            message: meta.last_error_msg.clone(),
        }
    }

    /// Slow-path: record a filesystem failure against an output root.
    ///
    /// Called from:
    /// - the manager pre-start hook when `ensure_output_dir` returns `Err`,
    /// - the engine stderr readers when a runtime ENOSPC signature is matched,
    /// - the startup probe.
    ///
    /// Idempotent: subsequent failures on an already-Degraded root just
    /// refresh the cached error message and bump `last_attempt_unix` to keep
    /// the cooldown window sliding. Only the first caller to flip
    /// `Healthy → Degraded` emits a notification.
    pub fn record_failure(&self, output_dir: &Path, err: &std::io::Error) {
        let root = resolve_root(output_dir, &self.configured_roots);
        let kind = IoErrorKindSer::from_io_kind(err.kind());
        let msg = err.to_string();

        // Insert-or-update. The entry constructor sets state = Degraded and
        // initializes meta atomically so any concurrent `check()` that lands
        // between the insert and our notification emit either sees Healthy
        // (returns Ok) or sees Degraded with valid meta (returns Err) —
        // never partial state.
        let entry = self
            .roots
            .entry(root.clone())
            .or_insert_with(|| Arc::new(RootEntry::new_degraded(kind, msg.clone())))
            .clone();

        // Existing entry: CAS Healthy → Degraded so exactly one caller emits
        // the notification per transition. Losers update meta only.
        let was_healthy = entry
            .state
            .compare_exchange(
                STATE_HEALTHY,
                STATE_DEGRADED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();

        if was_healthy {
            warn!(
                root = %root.display(),
                kind = kind.as_str(),
                error = %msg,
                "Output root marked Degraded; pausing downloads writing under this root"
            );
        } else {
            debug!(
                root = %root.display(),
                kind = kind.as_str(),
                error = %msg,
                "Refreshed Degraded root metadata"
            );
        }

        // Update cold metadata under the mutex.
        {
            let mut meta = entry.meta.lock();
            meta.last_error_kind = kind;
            meta.last_error_msg = msg;
            if was_healthy {
                meta.since_unix = unix_now();
                meta.rejected_count = 0;
            }
        }

        entry.last_attempt_unix.store(unix_now(), Ordering::Release);

        if was_healthy {
            self.spawn_notification(&root, kind);
        }
    }

    fn spawn_notification(&self, root: &Path, kind: IoErrorKindSer) {
        let weak = self.notification_service.clone();
        let path_str = root.display().to_string();
        let kind_str = kind.as_str().to_string();
        // Fire-and-forget. A notification delivery failure must not block the
        // gate or the download path, and notify() is async so we cannot call
        // it inline from a sync function anyway.
        tokio::spawn(async move {
            let Some(service) = weak.upgrade() else {
                debug!("NotificationService dropped before gate notification could fire");
                return;
            };
            let event = NotificationEvent::OutputPathInaccessible {
                path: path_str.clone(),
                error_kind: kind_str.clone(),
                timestamp: Utc::now(),
            };
            if let Err(e) = service.notify(event).await {
                warn!(
                    error = %e,
                    path = %path_str,
                    kind = %kind_str,
                    "Failed to dispatch OutputPathInaccessible notification (non-fatal)"
                );
            }
        });
    }

    /// Slow-path: transition a Degraded root back to Healthy.
    ///
    /// Called from the manager pre-start hook when the real `ensure_output_dir`
    /// returns `Ok` after winning the single-flight CAS. Idempotent — if the
    /// root is already Healthy or untracked, this is a no-op.
    pub fn mark_healthy(&self, output_dir: &Path) {
        let root = resolve_root(output_dir, &self.configured_roots);
        let Some(entry_ref) = self.roots.get(&root) else {
            return;
        };
        let entry = entry_ref.clone();
        drop(entry_ref);

        let was_degraded = entry
            .state
            .compare_exchange(
                STATE_DEGRADED,
                STATE_HEALTHY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();

        if !was_degraded {
            return;
        }

        // Clear the cached metadata so the next reject path produces a clean
        // GateBlocked even if a stale concurrent caller reaches build_blocked
        // between this transition and the entry being removed.
        {
            let mut meta = entry.meta.lock();
            meta.rejected_count = 0;
        }

        info!(
            root = %root.display(),
            "Output root recovered: gate transitioned Degraded -> Healthy"
        );

        // Fire the recovery hook — this is what clears streamer-level backoff
        // for any streamer that was infra-blocked due to this root. Run it on
        // a blocking-friendly task to avoid stalling the download path; the
        // hook may iterate streamer metadata and call into the repo.
        let hook = self.recovery_hook.clone();
        let root_for_hook = root.clone();
        tokio::spawn(async move {
            (hook)(&root_for_hook);
        });
    }

    /// Snapshot for the `/health` endpoint. Returns one entry per tracked
    /// Resolve an `output_dir` to the gate key the gate would use internally,
    /// honoring the gate's `configured_roots`. Exposed so the download
    /// manager's rejection-event constructor can report the exact same path
    /// that `record_failure`/`mark_healthy`/`snapshot` would use as the
    /// DashMap key — otherwise the rejection payload, the gate state, and
    /// the health-endpoint output could show three different path
    /// representations of the same physical root.
    pub fn resolve_path(&self, output_dir: &Path) -> PathBuf {
        resolve_root(output_dir, &self.configured_roots)
    }

    /// Snapshot for the `/health` endpoint. Returns one entry per tracked
    /// root, including healthy entries (which exist if the gate previously
    /// flipped to Degraded and back).
    pub fn snapshot(&self) -> Vec<RootHealth> {
        let now = unix_now();
        self.roots
            .iter()
            .map(|kv| {
                let root = kv.key().clone();
                let entry = kv.value();
                let state = if entry.state.load(Ordering::Acquire) == STATE_DEGRADED {
                    RootHealthState::Degraded
                } else {
                    RootHealthState::Healthy
                };
                let last_attempt = entry.last_attempt_unix.load(Ordering::Acquire);
                let seconds_since_last_attempt = if last_attempt == 0 {
                    None
                } else {
                    Some(now.saturating_sub(last_attempt))
                };
                let meta = entry.meta.lock();
                let last_error = if state == RootHealthState::Degraded {
                    Some((meta.last_error_kind, meta.last_error_msg.clone()))
                } else {
                    None
                };
                RootHealth {
                    root,
                    state,
                    seconds_since_last_attempt,
                    rejected_count: meta.rejected_count,
                    last_error,
                }
            })
            .collect()
    }

    /// Diagnostic helper — list root paths currently in the Degraded state.
    pub fn degraded_roots(&self) -> Vec<PathBuf> {
        self.roots
            .iter()
            .filter_map(|kv| {
                if kv.value().state.load(Ordering::Acquire) == STATE_DEGRADED {
                    Some(kv.key().clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Resolve the gate-tracking root for an output directory.
///
/// Strategy:
/// 1. If any path in `configured_roots` is a prefix of `output_dir`, return
///    the longest match (so `/rec/sub` configured wins over `/rec`).
/// 2. Otherwise, fall back to taking the first **two named components** of
///    an absolute path (e.g. `/rec/huya/X/20260415` → `/rec/huya`,
///    `/home/user/recordings/X` → `/home/user`). Two named components is
///    the minimum that handles both the common `/rec/{platform}` layout
///    and the multi-tenant `/home/{user}` layout without accidentally
///    short-circuiting unrelated users at `/home`. Deployments with a
///    single-mount `/rec`-style layout can set
///    `RUST_SREC_OUTPUT_ROOTS=/rec` to get one gate key per mount instead.
/// 3. Relative paths take their first component.
/// 4. Pathological cases (empty, root only) return the input verbatim.
///
/// Always returns a normalized path (no trailing separators, no `.`/`..`).
pub fn resolve_root(output_dir: &Path, configured: &[PathBuf]) -> PathBuf {
    let normalized = normalize_root(output_dir);

    // Configured-prefix match wins.
    if let Some(longest) = configured
        .iter()
        .filter(|c| normalized.starts_with(c))
        .max_by_key(|c| c.components().count())
    {
        return longest.clone();
    }

    // Fallback heuristic.
    let components: Vec<Component> = normalized.components().collect();
    if components.is_empty() {
        return normalized;
    }

    let take = match components[0] {
        // Absolute path: take RootDir + first 2 named components when available
        Component::RootDir => 3.min(components.len()),
        // Windows prefix: similar idea
        Component::Prefix(_) => 3.min(components.len()),
        // Relative path: take first component
        _ => 1.min(components.len()),
    };

    components.iter().take(take).collect()
}

/// Lexically normalize a path for use as a stable [`OutputRootGate`] map key.
///
/// - Strips trailing separators.
/// - Resolves `.` segments by skipping them.
/// - Resolves `..` segments lexically (without touching the filesystem).
///
/// Does NOT call [`std::fs::canonicalize`] because that requires the path to
/// exist on disk, which is exactly the condition the gate is designed to
/// guard against. Lexical normalization is enough to make `/rec`, `/rec/`,
/// `/rec/./`, and `/rec/a/..` all hash to the same key.
pub fn normalize_root(path: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::with_capacity(path.components().count());
    for component in path.components() {
        match component {
            Component::CurDir => {
                // Skip `.`
            }
            Component::ParentDir => {
                // Pop the previous Normal component if any (lexical `..`).
                if matches!(out.last(), Some(Component::Normal(_))) {
                    out.pop();
                } else {
                    out.push(component);
                }
            }
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomOrd};

    fn no_op_hook() -> RecoveryHook {
        Arc::new(|_root: &Path| {})
    }

    fn counting_hook() -> (RecoveryHook, Arc<AtomicUsize>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let hook: RecoveryHook = Arc::new(move |_root: &Path| {
            c2.fetch_add(1, AtomOrd::SeqCst);
        });
        (hook, counter)
    }

    fn make_gate(cooldown_secs: u64) -> Arc<OutputRootGate> {
        OutputRootGate::new(
            Weak::new(),
            no_op_hook(),
            vec![],
            Duration::from_secs(cooldown_secs),
        )
    }

    fn make_gate_with_hook(hook: RecoveryHook, cooldown_secs: u64) -> Arc<OutputRootGate> {
        OutputRootGate::new(
            Weak::new(),
            hook,
            vec![],
            Duration::from_secs(cooldown_secs),
        )
    }

    fn enoent() -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "no such directory")
    }

    fn enospc() -> io::Error {
        io::Error::new(io::ErrorKind::StorageFull, "no space")
    }

    // ---------- normalize_root ----------

    #[test]
    fn normalize_root_handles_trailing_slash() {
        assert_eq!(normalize_root(Path::new("/rec/")), PathBuf::from("/rec"));
        assert_eq!(normalize_root(Path::new("/rec")), PathBuf::from("/rec"));
    }

    #[test]
    fn normalize_root_resolves_curdir() {
        assert_eq!(
            normalize_root(Path::new("/rec/./a")),
            PathBuf::from("/rec/a")
        );
        assert_eq!(normalize_root(Path::new("./rec")), PathBuf::from("rec"));
    }

    #[test]
    fn normalize_root_resolves_parentdir_lexically() {
        assert_eq!(
            normalize_root(Path::new("/rec/a/..")),
            PathBuf::from("/rec")
        );
        assert_eq!(
            normalize_root(Path::new("/rec/a/../b")),
            PathBuf::from("/rec/b")
        );
    }

    #[test]
    fn normalize_root_collisions() {
        let canonical = normalize_root(Path::new("/rec"));
        for input in ["/rec", "/rec/", "/rec/.", "/rec/./", "/rec/a/.."] {
            assert_eq!(
                normalize_root(Path::new(input)),
                canonical,
                "input {} should normalize to {:?}",
                input,
                canonical
            );
        }
    }

    // ---------- resolve_root ----------

    #[test]
    fn resolve_root_with_configured_prefix() {
        let configured = vec![PathBuf::from("/rec"), PathBuf::from("/mnt/backup")];
        assert_eq!(
            resolve_root(Path::new("/rec/huya/X/20260415"), &configured),
            PathBuf::from("/rec")
        );
        assert_eq!(
            resolve_root(Path::new("/mnt/backup/Y"), &configured),
            PathBuf::from("/mnt/backup")
        );
    }

    #[test]
    fn resolve_root_picks_longest_configured_match() {
        let configured = vec![PathBuf::from("/rec"), PathBuf::from("/rec/sub")];
        assert_eq!(
            resolve_root(Path::new("/rec/sub/X"), &configured),
            PathBuf::from("/rec/sub")
        );
    }

    #[test]
    fn resolve_root_fallback_two_components_for_rec_layout() {
        let configured: Vec<PathBuf> = vec![];
        // /rec/huya/X/20260415 → /rec (the RootDir + 1 named component is enough,
        // but the heuristic conservatively takes 2 named components when present
        // to avoid blasting unrelated users in /home/X vs /home/Y).
        let r = resolve_root(Path::new("/rec/huya/X/20260415"), &configured);
        // Either /rec or /rec/huya is acceptable; assert the prefix is right
        // and the path has at most 2 named components.
        assert!(r.starts_with("/rec"), "got: {:?}", r);
        assert!(
            r.components().count() <= 3,
            "got too many components: {:?}",
            r
        );
    }

    #[test]
    fn resolve_root_fallback_two_components_for_home_layout() {
        let configured: Vec<PathBuf> = vec![];
        let r = resolve_root(Path::new("/home/user/recordings/X/20260415"), &configured);
        // Critically NOT /home alone — that would short-circuit other users.
        assert!(r.starts_with("/home/user"), "got: {:?}", r);
    }

    #[test]
    fn resolve_root_relative_path_takes_first_component() {
        let configured: Vec<PathBuf> = vec![];
        let r = resolve_root(Path::new("rec/huya/X"), &configured);
        assert_eq!(r, PathBuf::from("rec"));
    }

    #[test]
    fn resolve_root_normalization_is_applied() {
        let configured: Vec<PathBuf> = vec![];
        // Both inputs should resolve to the same gate key.
        let a = resolve_root(Path::new("/rec/huya/X"), &configured);
        let b = resolve_root(Path::new("/rec/./huya/X/"), &configured);
        assert_eq!(a, b);
    }

    // ---------- check / record_failure / mark_healthy ----------

    #[tokio::test]
    async fn check_returns_ok_on_empty_gate() {
        let gate = make_gate(30);
        assert!(gate.check(Path::new("/rec/huya/X")).is_ok());
    }

    #[tokio::test]
    async fn record_failure_then_check_returns_blocked() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/huya/X"), &enoent());
        // Subsequent check on the same root, before cooldown, must be blocked.
        let err = gate
            .check(Path::new("/rec/huya/Y"))
            .expect_err("should be blocked");
        assert_eq!(err.kind, IoErrorKindSer::NotFound);
        // Path could be /rec or /rec/huya depending on heuristic; both are valid
        // as long as the second streamer's check resolves to the same root.
        // Normalize separators so the assertion works on Windows (which renders
        // the joined components with backslashes).
        let root_str = err.root.to_string_lossy().replace('\\', "/");
        assert!(
            root_str == "/rec" || root_str == "/rec/huya",
            "unexpected root: {}",
            root_str
        );
    }

    #[tokio::test]
    async fn record_failure_unrelated_root_does_not_block_other_root() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/huya/X"), &enoent());
        // A path under a different root should be unaffected.
        assert!(gate.check(Path::new("/data/twitch/Y")).is_ok());
    }

    #[tokio::test]
    async fn cooldown_elapsed_allows_exactly_one_caller_through() {
        let gate = make_gate(1); // 1-second cooldown
        gate.record_failure(Path::new("/rec/X"), &enoent());

        // Immediately after record_failure the cooldown is NOT elapsed.
        assert!(gate.check(Path::new("/rec/X")).is_err());

        // Sleep just past the cooldown window (cooldown is in unix seconds, so
        // we need the wall clock to advance by at least 1 full second from
        // last_attempt_unix). 1.2s is comfortably enough.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // Now exactly one caller should win the CAS. Subsequent callers (in
        // the same second) fast-reject again because the winner advanced
        // last_attempt_unix to "now".
        let first = gate.check(Path::new("/rec/X"));
        let second = gate.check(Path::new("/rec/X"));
        let third = gate.check(Path::new("/rec/X"));
        let oks = [&first, &second, &third]
            .iter()
            .filter(|r| r.is_ok())
            .count();
        assert_eq!(
            oks, 1,
            "exactly one caller must win the probe slot per cooldown window"
        );
    }

    #[tokio::test]
    async fn single_flight_only_one_caller_wins_concurrent_cas() {
        // 100 concurrent checks on a Degraded root after the cooldown elapsed
        // must yield exactly one winner. This is the property that prevents
        // the thundering herd from all hitting the real ensure_output_dir
        // simultaneously when many streamers come out of backoff at the same
        // time.
        let gate = make_gate(1);
        gate.record_failure(Path::new("/rec/X"), &enoent());

        // Wait past the cooldown so the next batch can race for the slot.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let mut handles = vec![];
        for _ in 0..100 {
            let g = gate.clone();
            handles.push(tokio::spawn(
                async move { g.check(Path::new("/rec/X")).is_ok() },
            ));
        }
        let mut allowed = 0;
        for h in handles {
            if h.await.unwrap() {
                allowed += 1;
            }
        }
        assert_eq!(
            allowed, 1,
            "exactly one caller must win the probe slot per cooldown window, got {}",
            allowed
        );
    }

    #[tokio::test]
    async fn mark_healthy_clears_state_and_fires_hook() {
        let (hook, counter) = counting_hook();
        let gate = make_gate_with_hook(hook, 30);
        gate.record_failure(Path::new("/rec/X"), &enospc());
        assert!(gate.check(Path::new("/rec/X")).is_err());

        gate.mark_healthy(Path::new("/rec/X"));
        // Recovery hook is dispatched on a tokio task — yield so it runs.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;

        assert_eq!(
            counter.load(AtomOrd::SeqCst),
            1,
            "recovery hook should fire exactly once"
        );
        // Second mark_healthy on already-Healthy root is a no-op.
        gate.mark_healthy(Path::new("/rec/X"));
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(counter.load(AtomOrd::SeqCst), 1, "no-op on already-healthy");
        // And the gate now lets traffic through again.
        assert!(gate.check(Path::new("/rec/X")).is_ok());
    }

    #[tokio::test]
    async fn record_failure_is_idempotent_on_repeat() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/X"), &enoent());
        gate.record_failure(Path::new("/rec/X"), &enoent());
        gate.record_failure(Path::new("/rec/X"), &enoent());
        // Still exactly one entry.
        assert_eq!(gate.degraded_roots().len(), 1);
    }

    #[tokio::test]
    async fn snapshot_reports_degraded_with_metadata() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/X"), &enoent());
        // Generate one rejected attempt to bump the counter.
        let _ = gate.check(Path::new("/rec/X"));

        let snap = gate.snapshot();
        assert_eq!(snap.len(), 1);
        let entry = &snap[0];
        assert_eq!(entry.state, RootHealthState::Degraded);
        assert!(entry.last_error.is_some());
        let (kind, _) = entry.last_error.as_ref().unwrap();
        assert_eq!(*kind, IoErrorKindSer::NotFound);
        assert!(entry.rejected_count >= 1);
        assert!(entry.seconds_since_last_attempt.is_some());
    }

    #[tokio::test]
    async fn snapshot_reports_recovered_root_as_healthy() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/X"), &enoent());
        gate.mark_healthy(Path::new("/rec/X"));
        tokio::time::sleep(Duration::from_millis(20)).await;

        let snap = gate.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].state, RootHealthState::Healthy);
        assert!(snap[0].last_error.is_none());
    }

    #[tokio::test]
    async fn classification_round_trip_through_record_failure() {
        let cases = [
            (io::ErrorKind::NotFound, IoErrorKindSer::NotFound),
            (io::ErrorKind::StorageFull, IoErrorKindSer::StorageFull),
            (
                io::ErrorKind::PermissionDenied,
                IoErrorKindSer::PermissionDenied,
            ),
            (
                io::ErrorKind::ReadOnlyFilesystem,
                IoErrorKindSer::ReadOnlyFilesystem,
            ),
            (io::ErrorKind::TimedOut, IoErrorKindSer::TimedOut),
            (io::ErrorKind::ConnectionRefused, IoErrorKindSer::Other),
        ];
        for (input_kind, expected_ser) in cases {
            let gate = make_gate(30);
            let err = io::Error::new(input_kind, "test");
            gate.record_failure(Path::new("/rec/X"), &err);
            let snap = gate.snapshot();
            assert_eq!(snap.len(), 1);
            assert_eq!(snap[0].last_error.as_ref().unwrap().0, expected_ser);
        }
    }

    #[tokio::test]
    async fn check_after_recovery_resumes_normal_traffic() {
        let gate = make_gate(30);
        gate.record_failure(Path::new("/rec/X"), &enoent());
        assert!(gate.check(Path::new("/rec/X")).is_err());
        gate.mark_healthy(Path::new("/rec/X"));
        tokio::time::sleep(Duration::from_millis(20)).await;
        // Many concurrent checks all pass.
        for _ in 0..10 {
            assert!(gate.check(Path::new("/rec/X")).is_ok());
        }
    }

    #[tokio::test]
    async fn last_error_gate_prefix_constant_is_stable() {
        // The recovery hook in services::container filters streamer last_error
        // by this exact prefix. If it changes, the recovery hook silently stops
        // resetting streamers and the regression is invisible until production.
        assert_eq!(LAST_ERROR_GATE_PREFIX, "output-root blocked:");
    }
}
