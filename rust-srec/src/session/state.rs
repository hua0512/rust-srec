//! Session FSM types — `SessionState`, `TerminalCause`, `OfflineSignal`.
//!
//! Per-session state is intentionally narrow:
//!
//! - `Recording` (`ended_at.is_none()`): the recording is actively in progress.
//! - `Ended` (`ended_at.is_some()`): no more bytes will arrive. Pipeline work
//!   for the session may still be in flight — that is owned by `pipeline::manager`,
//!   not by `SessionLifecycle`.
//!
//! What previous designs called `Draining` / `AwaitingPipeline` / `PipelineRunning`
//! were pipeline-manager concerns and remain there. `SessionLifecycle` owns the
//! DB session row; `PipelineManager` owns DAG scheduling and pipeline phases.
//! See `rust-srec/src/session/mod.rs` for the architectural rationale.

use crate::downloader::{DownloadFailureKind, DownloadStopCause};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Note: `TerminalCause` carries `DownloadFailureKind` and `DownloadStopCause`
// which are not (yet) Serialize/Deserialize. `TerminalCause` is therefore an
// in-process type only — it is broadcast on a tokio channel to consumers in
// the same process. Persistence to the monitor outbox uses the existing
// `MonitorEvent::StreamerOffline` shape, which carries only the
// session_id / streamer_id / timestamp fields. If a future requirement needs
// to persist the cause across a restart, the cleanest fix is to mirror the
// shape into a serialisable record at the persistence boundary, not to plumb
// serde derives through the engine traits.

/// In-memory snapshot of a single recording session.
///
/// Mirrors the `live_sessions` DB row plus the terminal cause that closed it
/// (which today's DB schema does not persist; the cause is observable only
/// while the entry is in `SessionLifecycle`'s in-memory map).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub streamer_id: String,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    /// `None` while recording; `Some(t)` once a terminal event has been processed.
    pub ended_at: Option<DateTime<Utc>>,
    /// Set in the same write that sets `ended_at`. `None` while recording.
    pub terminal_cause: Option<TerminalCause>,
}

impl SessionState {
    /// Construct a freshly-recording session.
    pub fn recording(
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self {
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            started_at,
            ended_at: None,
            terminal_cause: None,
        }
    }

    /// Whether the session is currently recording (i.e. has not been terminally ended).
    pub fn is_recording(&self) -> bool {
        self.ended_at.is_none()
    }

    /// Whether the session is in a terminal `Ended` state.
    pub fn is_ended(&self) -> bool {
        self.ended_at.is_some()
    }
}

/// Why a recording session ended.
///
/// Each variant has a distinct policy on whether the session-complete pipeline
/// should fire. The policy is centralised in
/// [`Self::should_run_session_complete_pipeline`] so consumers do not re-derive
/// it per call site (the kind of drift that produced PR #524's bug).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCause {
    /// Engine reached a clean end of stream (e.g. EOF on FLV, `#EXT-X-ENDLIST` on HLS).
    Completed,
    /// Engine gave up due to error. Whatever output is on disk is final.
    Failed { kind: DownloadFailureKind },
    /// External stop request (user, scheduler, OutOfSchedule, Shutdown).
    /// A subsequent `Completed` may still arrive if the engine flushes the final segment.
    Cancelled { cause: DownloadStopCause },
    /// Download never started (circuit breaker, output-root unavailable, etc.).
    Rejected { reason: String },
    /// Monitor authoritatively determined the streamer went offline.
    StreamerOffline,
    /// A definitive offline signal observed at the engine boundary
    /// (e.g. HLS playlist 404, danmu stream closed, repeated stalls).
    /// Bypasses backoff for the session-end write.
    DefinitiveOffline { signal: OfflineSignal },
}

impl TerminalCause {
    /// Whether this terminal cause should trigger the per-streamer
    /// `session_complete_pipeline` DAG.
    ///
    /// - [`Self::Completed`]: yes — normal end, outputs finalised.
    /// - [`Self::Failed`]: yes — engine gave up; whatever's on disk is final.
    ///   Fixed by PR #524.
    /// - [`Self::Cancelled`]: **no** — cancellation is a stop *request*; a
    ///   `Completed` may still arrive once the engine flushes the final
    ///   segment. Firing the pipeline early would produce missing inputs.
    /// - [`Self::Rejected`]: no — never started, no outputs to process.
    /// - [`Self::StreamerOffline`]: yes — monitor said the streamer went offline.
    /// - [`Self::DefinitiveOffline`]: yes — engine boundary saw a definitive
    ///   offline signal (e.g. playlist 404).
    pub fn should_run_session_complete_pipeline(&self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed { .. }
                | Self::StreamerOffline
                | Self::DefinitiveOffline { .. }
        )
    }

    /// Short, stable string for logging / metrics labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed { .. } => "failed",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
            Self::StreamerOffline => "streamer_offline",
            Self::DefinitiveOffline { .. } => "definitive_offline",
        }
    }
}

/// A signal observed at the engine boundary that proves (with high confidence)
/// the upstream stream has truly ended — bypasses the slower hysteresis path.
///
/// Today the variants cover only signals strong enough to skip the
/// monitor-side re-check loop. Lower-confidence signals stay in the existing
/// hysteresis machinery on `StreamerActor`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OfflineSignal {
    /// HLS playlist returned 4xx (almost always 404). The platform deleted
    /// the live URL — the stream really ended.
    PlaylistGone(u16),
    /// Danmu websocket sent an explicit stream-closed control frame.
    DanmuStreamClosed,
    /// `n` consecutive engine failures of a kind that indicates "no more
    /// bytes are arriving" (network timeout, source unavailable) within the
    /// classifier's window. Engine-agnostic stall detection.
    ConsecutiveFailures(u32),
}

impl OfflineSignal {
    /// Short, stable string for logging / metrics labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PlaylistGone(_) => "playlist_gone",
            Self::DanmuStreamClosed => "danmu_stream_closed",
            Self::ConsecutiveFailures(_) => "consecutive_failures",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::DownloadStopCause;

    #[test]
    fn session_state_recording_constructor_marks_recording() {
        let now = Utc::now();
        let s = SessionState::recording("streamer-1", "session-1", now);
        assert!(s.is_recording());
        assert!(!s.is_ended());
        assert_eq!(s.streamer_id, "streamer-1");
        assert_eq!(s.session_id, "session-1");
        assert_eq!(s.started_at, now);
        assert_eq!(s.ended_at, None);
        assert_eq!(s.terminal_cause, None);
    }

    #[test]
    fn terminal_cause_completed_runs_session_complete_pipeline() {
        assert!(TerminalCause::Completed.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_failed_runs_session_complete_pipeline() {
        let c = TerminalCause::Failed {
            kind: DownloadFailureKind::Network,
        };
        assert!(c.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_streamer_offline_runs_session_complete_pipeline() {
        assert!(TerminalCause::StreamerOffline.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_definitive_offline_runs_session_complete_pipeline() {
        let c = TerminalCause::DefinitiveOffline {
            signal: OfflineSignal::PlaylistGone(404),
        };
        assert!(c.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_cancelled_does_not_run_session_complete_pipeline() {
        let c = TerminalCause::Cancelled {
            cause: DownloadStopCause::User,
        };
        assert!(!c.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_rejected_does_not_run_session_complete_pipeline() {
        let c = TerminalCause::Rejected {
            reason: "circuit breaker".into(),
        };
        assert!(!c.should_run_session_complete_pipeline());
    }

    #[test]
    fn terminal_cause_as_str_is_stable() {
        assert_eq!(TerminalCause::Completed.as_str(), "completed");
        assert_eq!(
            TerminalCause::Failed {
                kind: DownloadFailureKind::Network
            }
            .as_str(),
            "failed"
        );
        assert_eq!(
            TerminalCause::Cancelled {
                cause: DownloadStopCause::User
            }
            .as_str(),
            "cancelled"
        );
        assert_eq!(
            TerminalCause::Rejected {
                reason: "x".into()
            }
            .as_str(),
            "rejected"
        );
        assert_eq!(TerminalCause::StreamerOffline.as_str(), "streamer_offline");
        assert_eq!(
            TerminalCause::DefinitiveOffline {
                signal: OfflineSignal::DanmuStreamClosed
            }
            .as_str(),
            "definitive_offline"
        );
    }

    #[test]
    fn offline_signal_as_str_is_stable() {
        assert_eq!(OfflineSignal::PlaylistGone(404).as_str(), "playlist_gone");
        assert_eq!(
            OfflineSignal::DanmuStreamClosed.as_str(),
            "danmu_stream_closed"
        );
        assert_eq!(
            OfflineSignal::ConsecutiveFailures(2).as_str(),
            "consecutive_failures"
        );
    }
}
