//! Hysteresis primitives for the session lifecycle.
//!
//! When [`crate::session::SessionLifecycle`] observes a *non-authoritative*
//! terminal event (mesio FLV clean disconnect, ffmpeg subprocess exit, network
//! failure, …), it doesn't commit `Ended` immediately. Instead it parks the
//! session in `SessionState::Hysteresis` and arms a timer. Three things can
//! happen next:
//!
//! 1. A `LiveDetected` arrives before the deadline. The hysteresis handle's
//!    [`CancellationToken`] is tripped, the timer task exits without firing,
//!    and the session transitions back to `Recording` (same `session_id`).
//! 2. An *authoritative* terminal event arrives (e.g. `DanmuStreamClosed`,
//!    HLS playlist 404, monitor `StreamerOffline`). The hysteresis is
//!    cancelled and the session transitions directly to `Ended`.
//! 3. The deadline elapses with no resume. The timer task fires `Ended` via
//!    the lifecycle's `enter_ended_state` path; the DB write commits and
//!    the pipeline-complete DAG is scheduled.
//!
//! This module defines the data types only. The driver lives in
//! [`crate::session::lifecycle::SessionLifecycle`] (it owns the
//! `tokio::spawn` for the timer task because cancellation needs access to
//! the lifecycle's state to decide whether the cancellation was a resume,
//! an authoritative override, or a no-op).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::session::state::TerminalCause;

/// Default hysteresis quiet-period applied when no platform-specific override is set.
///
/// Justification: the 2026-04-26 production log showed a Huya stream where
/// the platform reissued a fresh stream URL ~90 seconds after the engine
/// observed a clean FLV disconnect. 90 s is long enough to absorb that
/// reconnect cycle, short enough that the pipeline-complete DAG isn't
/// delayed egregiously when the streamer really did stop.
pub const DEFAULT_HYSTERESIS_WINDOW: Duration = Duration::from_secs(90);

/// Maximum permitted hysteresis window. Defends against config typos that
/// would park sessions for unreasonable durations.
pub const MAX_HYSTERESIS_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Tunable configuration for the hysteresis quiet-period.
///
/// Loaded once at lifecycle construction; overrides can come from the
/// per-platform configuration layer in a future PR. Today only the global
/// default and per-platform overrides are honoured; per-streamer overrides
/// are an explicit non-goal (would just push the tuning surface further out
/// without measurably improving correctness).
#[derive(Debug, Clone)]
pub struct HysteresisConfig {
    /// Window applied when no platform-specific override matches.
    pub default_window: Duration,
    /// Optional per-platform overrides. Key is the `platform_config.platform_name`
    /// string (e.g. `"huya"`, `"douyin"`). Useful when one platform has
    /// faster URL reissue than another.
    pub per_platform: HashMap<String, Duration>,
}

impl Default for HysteresisConfig {
    fn default() -> Self {
        Self {
            default_window: DEFAULT_HYSTERESIS_WINDOW,
            per_platform: HashMap::new(),
        }
    }
}

impl HysteresisConfig {
    /// Resolve the window for a given platform name. Falls back to
    /// `default_window` when no override is set. Capped at
    /// [`MAX_HYSTERESIS_WINDOW`] to defend against pathological configs.
    pub fn window_for_platform(&self, platform: Option<&str>) -> Duration {
        let raw = platform
            .and_then(|p| self.per_platform.get(p).copied())
            .unwrap_or(self.default_window);
        raw.min(MAX_HYSTERESIS_WINDOW)
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
    fn config_default_uses_default_window() {
        let c = HysteresisConfig::default();
        assert_eq!(c.window_for_platform(None), DEFAULT_HYSTERESIS_WINDOW);
        assert_eq!(c.window_for_platform(Some("huya")), DEFAULT_HYSTERESIS_WINDOW);
    }

    #[test]
    fn config_per_platform_override() {
        let mut c = HysteresisConfig::default();
        c.per_platform
            .insert("douyin".into(), Duration::from_secs(45));
        assert_eq!(c.window_for_platform(Some("douyin")), Duration::from_secs(45));
        assert_eq!(c.window_for_platform(Some("huya")), DEFAULT_HYSTERESIS_WINDOW);
        assert_eq!(c.window_for_platform(None), DEFAULT_HYSTERESIS_WINDOW);
    }

    #[test]
    fn config_caps_window_at_max() {
        let mut c = HysteresisConfig::default();
        c.per_platform
            .insert("buggy".into(), Duration::from_secs(60 * 60));
        assert_eq!(c.window_for_platform(Some("buggy")), MAX_HYSTERESIS_WINDOW);
    }

    #[test]
    fn config_caps_default_at_max() {
        let c = HysteresisConfig {
            default_window: Duration::from_secs(60 * 60),
            per_platform: HashMap::new(),
        };
        assert_eq!(c.window_for_platform(None), MAX_HYSTERESIS_WINDOW);
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
