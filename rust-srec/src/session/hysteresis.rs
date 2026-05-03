//! Hysteresis primitives for the session lifecycle.
//!
//! When [`crate::session::SessionLifecycle`] observes a *non-authoritative*
//! terminal event (mesio FLV clean disconnect, ffmpeg subprocess exit, network
//! failure that the classifier didn't promote, …), it doesn't commit `Ended`
//! immediately. Instead it parks the session in `SessionState::Hysteresis`
//! and arms a backstop timer. Three things can happen next:
//!
//! 1. A `LiveDetected` arrives before the deadline. The hysteresis handle's
//!    [`CancellationToken`] is tripped, the timer task exits without firing,
//!    and the session transitions back to `Recording` (same `session_id`).
//! 2. An *authoritative* terminal event arrives (`StreamerOffline` from the
//!    monitor, classifier-promoted `DefinitiveOffline`, HLS `EXT-X-ENDLIST`,
//!    …). The hysteresis is cancelled and the session transitions directly
//!    to `Ended`.
//! 3. The deadline elapses with no resume. The timer task fires `Ended` via
//!    the lifecycle's `enter_ended_state` path; the DB write commits and
//!    the pipeline-complete DAG is scheduled.
//!
//! ## Why one window, derived from the scheduler config
//!
//! The actor (`scheduler::actor::streamer_actor`) already runs an offline-
//! confirmation hysteresis: after seeing the streamer's status flip to
//! NotLive it polls `offline_check_interval_ms` (default 20 s) and only
//! emits `StreamerOffline` after `offline_check_count` (default 3)
//! observations. That cadence is the *primary* mechanism that gates whether
//! a stream end is real — `~ count × interval ≈ 60 s` of confirmation.
//!
//! The session-level hysteresis state is **driven by the actor's events**
//! (`on_live_detected` resumes, `on_offline_detected` confirms `Ended`).
//! The timer here is a **backstop**, not a parallel quiet period: it only
//! fires if the actor never calls back (e.g. the streamer was manually
//! disabled while in hysteresis, or the actor was removed). Sizing it to
//! `(count + 1) × interval` keeps it strictly larger than the actor's
//! confirmation latency, so under normal flow the actor's call always wins.
//!
//! No new tunable. The window tracks whatever the operator already set on
//! `global_config.offline_check_*`.
//!
//! This module defines the data types only. The driver lives in
//! [`crate::session::lifecycle::SessionLifecycle`] (it owns the
//! `tokio::spawn` for the timer task because cancellation needs access to
//! the lifecycle's state to decide whether the cancellation was a resume,
//! an authoritative override, or a no-op).

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::session::state::TerminalCause;

/// Hard cap on the derived hysteresis window. Defends against pathological
/// scheduler configs (e.g. `offline_check_interval_ms = 600_000`) that
/// would otherwise park sessions for unreasonable durations.
pub const MAX_HYSTERESIS_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Default scheduler-derived window: `(3 + 1) × 20 s = 80 s`. Chosen to
/// match the hard-coded defaults in
/// [`crate::scheduler::SchedulerConfig::default`] so a freshly-installed
/// instance has consistent behaviour without any explicit configuration.
const DEFAULT_OFFLINE_CHECK_COUNT: u32 = 3;
const DEFAULT_OFFLINE_CHECK_INTERVAL_MS: u64 = 20_000;

/// Tunable backstop window for the lifecycle's hysteresis state.
///
/// **Not a parallel tunable.** The window is derived from the existing
/// `offline_check_count × offline_check_interval_ms` scheduler config — a
/// single source of truth for "how long do we wait before declaring a
/// stream really offline." The value here is `(count + 1) × interval`,
/// adding one tick of slack so the actor's confirmed-offline event always
/// lands before the lifecycle's safety-net timer fires.
#[derive(Debug, Clone, Copy)]
pub struct HysteresisConfig {
    window: Duration,
}

impl HysteresisConfig {
    /// Build a config from the scheduler's existing offline-check tunables.
    /// Window = `(count + 1) × interval_ms`, capped at
    /// [`MAX_HYSTERESIS_WINDOW`].
    pub fn from_scheduler(offline_check_count: u32, offline_check_interval_ms: u64) -> Self {
        let count = offline_check_count.max(1) as u64;
        let interval = offline_check_interval_ms.max(1_000);
        let raw = Duration::from_millis((count + 1) * interval);
        Self {
            window: raw.min(MAX_HYSTERESIS_WINDOW),
        }
    }

    /// Construct directly from a `Duration`. Reserved for tests that need
    /// a sub-second window to drive timer expiry without sleeping.
    #[cfg(test)]
    pub fn from_window(window: Duration) -> Self {
        Self {
            window: window.min(MAX_HYSTERESIS_WINDOW),
        }
    }

    /// The backstop window applied by the lifecycle when entering
    /// `Hysteresis`. Always `≤ MAX_HYSTERESIS_WINDOW`.
    pub fn window(&self) -> Duration {
        self.window
    }
}

impl Default for HysteresisConfig {
    fn default() -> Self {
        Self::from_scheduler(
            DEFAULT_OFFLINE_CHECK_COUNT,
            DEFAULT_OFFLINE_CHECK_INTERVAL_MS,
        )
    }
}

/// Per-session hysteresis handle. Stored by the lifecycle in a
/// `DashMap<session_id, HysteresisHandle>` for the duration of the
/// quiet-period.
///
/// The handle is dropped (and the timer task exits) when one of:
///
/// - The deadline elapses (the timer task observes `cancel.is_cancelled()
///   == false` and proceeds to commit `Ended`).
/// - A resume cancels it (`cancel.cancel()` from the lifecycle on
///   `LiveDetected`; timer task observes the cancellation and exits without
///   firing).
/// - An authoritative end overrides it (lifecycle calls
///   `enter_ended_state` directly; cancels the token as part of the
///   atomic state-update so the pending timer task can't double-fire).
#[derive(Debug)]
pub struct HysteresisHandle {
    /// Monotonic clock instant when the terminal event was observed.
    /// `started_at + window = deadline`.
    pub started_at: Instant,
    /// Monotonic deadline at which the timer task fires `Ended`.
    pub deadline: Instant,
    /// Cancellation token tripped by `cancel()` from the lifecycle when a
    /// resume or authoritative end pre-empts the hysteresis.
    pub cancel: CancellationToken,
}

impl HysteresisHandle {
    /// Build a fresh handle starting at the current monotonic clock.
    pub fn new(window: Duration) -> Self {
        let now = Instant::now();
        Self {
            started_at: now,
            deadline: now + window,
            cancel: CancellationToken::new(),
        }
    }

    /// How long the session has been parked in hysteresis.
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// `true` if the cancellation token has been tripped (resume or
    /// authoritative override).
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Cancel this hysteresis handle. Idempotent. Cancellation is observed
    /// by the timer task on its next `tokio::select!` poll; if the deadline
    /// has already fired, this is a no-op.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Reason a hysteresis handle was cancelled or fired. Useful for logging
/// and for the eventual `SessionTransition::Resumed`/`Ended` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HysteresisOutcome {
    /// The deadline elapsed with no resume. Lifecycle commits `Ended`.
    Expired,
    /// A `LiveDetected` arrived before the deadline. Lifecycle transitions
    /// `Hysteresis → Recording`, same session id.
    Resumed,
    /// An authoritative cause (e.g. `DanmuStreamClosed`) pre-empted the
    /// quiet-period. Lifecycle commits `Ended` immediately with the
    /// authoritative cause.
    OverriddenByAuthority { cause: TerminalCause },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_scheduler_uses_count_plus_one_times_interval() {
        // (3 + 1) × 20 000 ms = 80 000 ms = 80 s
        let c = HysteresisConfig::from_scheduler(3, 20_000);
        assert_eq!(c.window(), Duration::from_secs(80));
    }

    #[test]
    fn from_scheduler_caps_at_max() {
        // Config typo: (3 + 1) × 1h = 4h, must cap at 5 min.
        let c = HysteresisConfig::from_scheduler(3, 60 * 60 * 1000);
        assert_eq!(c.window(), MAX_HYSTERESIS_WINDOW);
    }

    #[test]
    fn from_scheduler_floors_count_at_one() {
        // count = 0 collapses to 1 to keep the math sensible.
        let c = HysteresisConfig::from_scheduler(0, 20_000);
        assert_eq!(c.window(), Duration::from_secs(40));
    }

    #[test]
    fn from_scheduler_floors_interval_at_one_second() {
        // interval = 50 ms collapses to 1 s.
        let c = HysteresisConfig::from_scheduler(3, 50);
        assert_eq!(c.window(), Duration::from_secs(4));
    }

    #[test]
    fn default_matches_scheduler_default() {
        // Scheduler default is count=3, interval=20s → window = 80s.
        let c = HysteresisConfig::default();
        assert_eq!(c.window(), Duration::from_secs(80));
    }

    #[test]
    fn handle_starts_uncancelled() {
        let h = HysteresisHandle::new(Duration::from_secs(90));
        assert!(!h.is_cancelled());
        assert!(h.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn handle_cancel_is_idempotent() {
        let h = HysteresisHandle::new(Duration::from_secs(90));
        h.cancel();
        assert!(h.is_cancelled());
        h.cancel();
        assert!(h.is_cancelled());
    }
}
