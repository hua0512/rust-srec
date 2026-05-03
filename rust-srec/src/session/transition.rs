//! `SessionTransition` — the narrow event stream emitted by `SessionLifecycle`.
//!
//! Four variants:
//!
//! - [`SessionTransition::Started`] — entered `Recording`. Either fresh
//!   `Created` or resumed out of `Hysteresis` (`from_hysteresis: bool`).
//! - [`SessionTransition::Ending`] — entered `Hysteresis`. Pipeline /
//!   notification subscribers do nothing on this event; they wait for
//!   `Ended`. Actor uses it to switch to short-polling cadence.
//! - [`SessionTransition::Resumed`] — `Hysteresis` cancelled by a timely
//!   resume. Subscribers cancel any work scheduled by `Ending` (today: no
//!   such work; reserved for future async coordination).
//! - [`SessionTransition::Ended`] — confirmed final end. Pipeline fires.
//!   `end_time` is set in DB. The `via_hysteresis` flag tells subscribers
//!   whether the end was reached via timer expiry (true) or directly from
//!   an authoritative cause (false).
//!
//! `Ending` and `Resumed` are additive variants. Subscribers that care only
//! about the final state filter on `Ended` and continue working as before.
//!
//! Backwards compatibility: today's `MonitorEvent::StreamerLive` /
//! `StreamerOffline` are still emitted (now from `SessionLifecycle`, not
//! from `monitor::service`) so the SSE / notification frontend contract is
//! preserved byte-for-byte.

use crate::session::download_start::DownloadStartPayload;
use crate::session::state::TerminalCause;
use chrono::{DateTime, Utc};

/// A coarse-grained session lifecycle event.
///
/// Not `Serialize`/`Deserialize`: `TerminalCause` carries engine-internal types
/// (`DownloadFailureKind`, `DownloadStopCause`) that are not exposed across
/// process boundaries. Consumers that need a serialisable view (notification
/// outbox, SSE) construct their own representation from `SessionTransition`'s
/// fields at the persistence/wire boundary.
///
/// `PartialEq` / `Eq` not derived: `Started.download_start` carries
/// `Vec<StreamInfo>` from the platforms-parser crate, which doesn't implement
/// `Eq`. Tests use `matches!` instead — see `accessors_work_for_all_variants`.
#[derive(Debug, Clone)]
pub enum SessionTransition {
    /// Emitted after a session enters `Recording`.
    ///
    /// `from_hysteresis` indicates whether this is a fresh session or a
    /// resume out of the hysteresis quiet-period. Subscribers that want to
    /// dedup notifications between an `Ending`/`Resumed` cycle can ignore
    /// `from_hysteresis: true` events.
    Started {
        session_id: String,
        streamer_id: String,
        streamer_name: String,
        title: String,
        /// Stream category / game, when the monitor trigger carried one.
        /// Preserves byte-identical parity with `MonitorEvent::StreamerLive`
        /// for notification payload consumers.
        category: Option<String>,
        started_at: DateTime<Utc>,
        /// `true` when this `Started` is a resume out of `Hysteresis`
        /// (same `session_id` continues); `false` for a fresh session.
        from_hysteresis: bool,
        /// Sidecar payload for the container's resume-download subscriber.
        ///
        /// `Some(_)` means "please drive `start_download_for_streamer`" —
        /// always populated by `lifecycle::resume_from_hysteresis` and by
        /// `lifecycle::on_live_detected` for fresh sessions. `None` is
        /// the test-fixture / notification-only default. See
        /// [`DownloadStartPayload`] for the field rationale.
        ///
        /// Boxed so a `Started` without a payload stays one pointer wide
        /// — historical consumers that never read the payload don't pay
        /// the size of three media-metadata fields.
        download_start: Option<Box<DownloadStartPayload>>,
    },
    /// Emitted after a session enters `Hysteresis` (non-authoritative end).
    /// The session is held in a quiet-period; pipeline / notification
    /// consumers should NOT treat this as a final end.
    Ending {
        session_id: String,
        streamer_id: String,
        streamer_name: String,
        cause: TerminalCause,
        /// When the terminal event was observed (engine emitted Completed/Failed).
        observed_at: DateTime<Utc>,
        /// Wall-clock deadline by which a `LiveDetected` resume must arrive
        /// to absorb the session. After this, the timer fires `Ended`.
        resume_deadline: DateTime<Utc>,
    },
    /// Emitted when `Hysteresis` is cancelled by a `LiveDetected` arriving
    /// before the deadline. The session continues recording with the same
    /// `session_id`. Subscribers cancel any scheduled work that was waiting
    /// on `Ended` for this session.
    Resumed {
        session_id: String,
        streamer_id: String,
        resumed_at: DateTime<Utc>,
        /// How long we spent in `Hysteresis` before the resume arrived.
        hysteresis_duration: chrono::Duration,
    },
    /// Emitted after a session is moved to `Ended`. The session row's
    /// `end_time` has been written; the streamer's `is_live` flag is also
    /// flipped if the path was the monitor offline path (in the same
    /// atomic tx). Pipeline manager consumes this; it consults
    /// [`TerminalCause::should_run_session_complete_pipeline`] to decide
    /// whether to schedule the session-complete DAG.
    Ended {
        session_id: String,
        streamer_id: String,
        /// Needed by the notification layer to keep `StreamOffline`
        /// payloads byte-identical to the pre-refactor `MonitorEvent::
        /// StreamerOffline` shape.
        streamer_name: String,
        ended_at: DateTime<Utc>,
        cause: TerminalCause,
        /// `true` when the end was reached via `Hysteresis` timer expiry;
        /// `false` for a direct `Recording → Ended` transition (authoritative
        /// cause). Useful for telemetry / debugging.
        via_hysteresis: bool,
    },
}

impl SessionTransition {
    /// Streamer id, present on every variant.
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::Started { streamer_id, .. }
            | Self::Ending { streamer_id, .. }
            | Self::Resumed { streamer_id, .. }
            | Self::Ended { streamer_id, .. } => streamer_id,
        }
    }

    /// Session id, present on every variant.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. }
            | Self::Ending { session_id, .. }
            | Self::Resumed { session_id, .. }
            | Self::Ended { session_id, .. } => session_id,
        }
    }

    /// Short, stable label for logging / metrics.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Ending { .. } => "ending",
            Self::Resumed { .. } => "resumed",
            Self::Ended { .. } => "ended",
        }
    }

    /// `true` for `Ended` only — the moment subscribers should treat as
    /// the final state. Useful when filtering at the broadcast boundary.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::state::OfflineSignal;

    fn started() -> SessionTransition {
        SessionTransition::Started {
            session_id: "s1".into(),
            streamer_id: "r1".into(),
            streamer_name: "test".into(),
            title: "t".into(),
            category: None,
            started_at: Utc::now(),
            from_hysteresis: false,
            download_start: None,
        }
    }

    fn ending() -> SessionTransition {
        SessionTransition::Ending {
            session_id: "s1".into(),
            streamer_id: "r1".into(),
            streamer_name: "test".into(),
            cause: TerminalCause::Completed,
            observed_at: Utc::now(),
            resume_deadline: Utc::now() + chrono::Duration::seconds(90),
        }
    }

    fn resumed() -> SessionTransition {
        SessionTransition::Resumed {
            session_id: "s1".into(),
            streamer_id: "r1".into(),
            resumed_at: Utc::now(),
            hysteresis_duration: chrono::Duration::seconds(20),
        }
    }

    fn ended() -> SessionTransition {
        SessionTransition::Ended {
            session_id: "s1".into(),
            streamer_id: "r1".into(),
            streamer_name: "test".into(),
            ended_at: Utc::now(),
            cause: TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::ConsecutiveFailures(2),
            },
            via_hysteresis: false,
        }
    }

    #[test]
    fn accessors_work_for_all_variants() {
        for t in [started(), ending(), resumed(), ended()] {
            assert_eq!(t.streamer_id(), "r1");
            assert_eq!(t.session_id(), "s1");
        }
    }

    #[test]
    fn kind_str_is_stable() {
        assert_eq!(started().kind_str(), "started");
        assert_eq!(ending().kind_str(), "ending");
        assert_eq!(resumed().kind_str(), "resumed");
        assert_eq!(ended().kind_str(), "ended");
    }

    #[test]
    fn only_ended_is_terminal() {
        assert!(!started().is_terminal());
        assert!(!ending().is_terminal());
        assert!(!resumed().is_terminal());
        assert!(ended().is_terminal());
    }
}
