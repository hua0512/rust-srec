//! `SessionTransition` — the narrow event stream emitted by `SessionLifecycle`.
//!
//! Two variants only:
//!
//! - [`SessionTransition::Started`]: a new recording session entered the
//!   `Recording` state. Emitted after the atomic write of the new session row.
//! - [`SessionTransition::Ended`]: a recording session entered the `Ended`
//!   state. Emitted after the atomic write of `ended_at` (and the streamer's
//!   offline flag, where applicable).
//!
//! Consumers that need finer pipeline-stage notifications subscribe to
//! `pipeline::manager`'s own event channel — that's the right boundary.
//! `SessionTransition` is intentionally narrow so the broadcaster does not
//! need to fan out per-segment / per-DAG noise.
//!
//! Backwards compatibility: today's `MonitorEvent::StreamerLive` /
//! `StreamerOffline` are still emitted (now from `SessionLifecycle`, not
//! from `monitor::service`) so the SSE / notification frontend contract is
//! preserved byte-for-byte.

use crate::session::state::TerminalCause;
use chrono::{DateTime, Utc};

/// A coarse-grained session lifecycle event.
///
/// Not `Serialize`/`Deserialize`: `TerminalCause` carries engine-internal types
/// (`DownloadFailureKind`, `DownloadStopCause`) that are not exposed across
/// process boundaries. Consumers that need a serialisable view (notification
/// outbox, SSE) construct their own representation from `SessionTransition`'s
/// fields at the persistence/wire boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTransition {
    /// Emitted after a new session row is written. The session is now in the
    /// `Recording` state.
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
    },
    /// Emitted after a session is moved to the `Ended` state. The session row
    /// has `ended_at` set; the streamer's `is_live` flag is also flipped in
    /// the same atomic transaction.
    ///
    /// Pipeline manager consumes this; it consults
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
    },
}

impl SessionTransition {
    /// Streamer id, present on every variant.
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::Started { streamer_id, .. } | Self::Ended { streamer_id, .. } => streamer_id,
        }
    }

    /// Session id, present on every variant.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Started { session_id, .. } | Self::Ended { session_id, .. } => session_id,
        }
    }

    /// Short, stable label for logging / metrics.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Ended { .. } => "ended",
        }
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
        }
    }

    fn ended() -> SessionTransition {
        SessionTransition::Ended {
            session_id: "s1".into(),
            streamer_id: "r1".into(),
            streamer_name: "test".into(),
            ended_at: Utc::now(),
            cause: TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::PlaylistGone(404),
            },
        }
    }

    #[test]
    fn accessors_work_for_started() {
        let t = started();
        assert_eq!(t.streamer_id(), "r1");
        assert_eq!(t.session_id(), "s1");
        assert_eq!(t.kind_str(), "started");
    }

    #[test]
    fn accessors_work_for_ended() {
        let t = ended();
        assert_eq!(t.streamer_id(), "r1");
        assert_eq!(t.session_id(), "s1");
        assert_eq!(t.kind_str(), "ended");
    }

}
