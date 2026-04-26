//! Session FSM types — `SessionState`, `TerminalCause`, `OfflineSignal`.
//!
//! `SessionState` is a three-variant FSM:
//!
//! - **`Recording`** — actively capturing bytes. DB `end_time IS NULL`.
//! - **`Hysteresis`** — the engine has reported a non-authoritative terminal
//!   event (mesio FLV clean disconnect, ffmpeg subprocess exit, network
//!   failure, …). The session is held in a quiet-period to absorb a possible
//!   resume; DB `end_time` is *not* yet written. A `LiveDetected` for the
//!   same streamer inside the window cancels the hysteresis timer and
//!   transitions back to `Recording`. The window expiring without a resume
//!   transitions to `Ended`. An authoritative end signal (danmu close,
//!   playlist 404) arriving during hysteresis cancels the timer and
//!   transitions directly to `Ended`.
//! - **`Ended`** — the recording is finished. DB `end_time` is set; the
//!   session-complete pipeline DAG fires (gated on the cause's
//!   [`TerminalCause::should_run_session_complete_pipeline`] policy).
//!
//! Only `Ended` writes `end_time`. The DB column means "this recording is
//! over" — never "tentatively over, might come back." That contract is what
//! makes API `is_live = end_time.is_none()` correct without flicker.
//!
//! See `rust-srec/src/session/hysteresis.rs` for the timer implementation
//! and `rust-srec/src/session/lifecycle.rs` for the FSM driver.

use crate::downloader::{DownloadFailureKind, DownloadStopCause};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;

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
/// Three states; transitions documented on the module doc and on
/// `SessionLifecycle`. The discriminator is the variant; payload differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// Actively capturing bytes.
    Recording {
        streamer_id: String,
        session_id: String,
        started_at: DateTime<Utc>,
    },
    /// Engine reported a non-authoritative end. Holding the session for a
    /// quiet-period before committing.
    Hysteresis {
        streamer_id: String,
        session_id: String,
        started_at: DateTime<Utc>,
        /// When the terminal event arrived.
        observed_at: DateTime<Utc>,
        /// What caused us to enter hysteresis.
        cause: TerminalCause,
        /// Monotonic deadline; the timer task wakes at this point. If still
        /// in `Hysteresis`, transitions to `Ended`.
        deadline: Instant,
    },
    /// Recording finished. `end_time` is set in DB.
    Ended {
        streamer_id: String,
        session_id: String,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        cause: TerminalCause,
    },
}

impl SessionState {
    /// Construct a freshly-recording session.
    pub fn recording(
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        started_at: DateTime<Utc>,
    ) -> Self {
        Self::Recording {
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            started_at,
        }
    }

    /// Build a session in `Hysteresis` state. The deadline must be a
    /// monotonic instant (use `Instant::now() + window`).
    pub fn hysteresis(
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        started_at: DateTime<Utc>,
        observed_at: DateTime<Utc>,
        cause: TerminalCause,
        deadline: Instant,
    ) -> Self {
        Self::Hysteresis {
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            started_at,
            observed_at,
            cause,
            deadline,
        }
    }

    /// Build an `Ended` session.
    pub fn ended(
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        started_at: DateTime<Utc>,
        ended_at: DateTime<Utc>,
        cause: TerminalCause,
    ) -> Self {
        Self::Ended {
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            started_at,
            ended_at,
            cause,
        }
    }

    /// Streamer id, present on every variant.
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::Recording { streamer_id, .. }
            | Self::Hysteresis { streamer_id, .. }
            | Self::Ended { streamer_id, .. } => streamer_id,
        }
    }

    /// Session id, present on every variant.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Recording { session_id, .. }
            | Self::Hysteresis { session_id, .. }
            | Self::Ended { session_id, .. } => session_id,
        }
    }

    /// When the recording started, present on every variant.
    pub fn started_at(&self) -> DateTime<Utc> {
        match self {
            Self::Recording { started_at, .. }
            | Self::Hysteresis { started_at, .. }
            | Self::Ended { started_at, .. } => *started_at,
        }
    }

    /// `true` if state is `Recording` (actively capturing). Used by the API's
    /// `is_session_active` query path.
    pub fn is_recording(&self) -> bool {
        matches!(self, Self::Recording { .. })
    }

    /// `true` if state is `Hysteresis` (tentative end, in quiet-period).
    pub fn is_hysteresis(&self) -> bool {
        matches!(self, Self::Hysteresis { .. })
    }

    /// `true` if state is `Ended` (recording finished, `end_time` written).
    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended { .. })
    }

    /// `true` for `Recording` OR `Hysteresis` — i.e. "the session has not
    /// committed to being over yet". This is the right semantic for
    /// API-level `is_live`-style queries: during hysteresis we still hope
    /// to absorb a resume, so the session is presented as active.
    pub fn is_active(&self) -> bool {
        !self.is_ended()
    }

    /// Short, stable string for logging / metrics labels.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Recording { .. } => "recording",
            Self::Hysteresis { .. } => "hysteresis",
            Self::Ended { .. } => "ended",
        }
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
    ///
    /// Whether this counts as authoritative for the hysteresis decision
    /// depends on the engine signal; see
    /// [`Self::is_authoritative_end`] which takes the
    /// [`crate::downloader::EngineEndSignal`] hint.
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

    /// Whether this cause should bypass the hysteresis quiet-period and end
    /// the session immediately.
    ///
    /// Authoritative signals come from sources that can confidently say "the
    /// stream is over": the platform's own websocket (`DanmuStreamClosed` →
    /// `Cancelled` plumbed via the danmu observer's `on_offline_detected`
    /// call), the platform's status API (`StreamerOffline`), or an engine
    /// boundary observation that constitutes platform-asserted end
    /// (`DefinitiveOffline { PlaylistGone }`, HLS `#EXT-X-ENDLIST` →
    /// `Completed` with [`crate::downloader::EngineEndSignal::HlsEndlist`]).
    ///
    /// `Completed` itself is ambiguous (mesio HLS-with-endlist is
    /// authoritative; mesio FLV clean disconnect is not). Callers that have
    /// access to the engine-side signal should prefer
    /// [`Self::is_authoritative_end_with_signal`].
    ///
    /// `Rejected` is treated as authoritative for the *direct* path because
    /// no recording happened — there's nothing to absorb a resume for.
    pub fn is_authoritative_end(&self) -> bool {
        match self {
            Self::DefinitiveOffline { .. } => true,
            Self::StreamerOffline => true,
            Self::Rejected { .. } => true,
            // Default: not authoritative without an engine signal.
            Self::Completed => false,
            Self::Failed { .. } => false,
            // Cancelled is intercepted as a no-op upstream of this check.
            Self::Cancelled { .. } => false,
        }
    }

    /// Authority decision when the engine's end signal is known. This is the
    /// preferred form for `on_download_terminal`: mesio HLS with EXT-X-ENDLIST
    /// is authoritative, mesio FLV clean disconnect is not.
    pub fn is_authoritative_end_with_signal(
        &self,
        signal: Option<crate::downloader::EngineEndSignal>,
    ) -> bool {
        if self.is_authoritative_end() {
            return true;
        }
        matches!(
            (self, signal),
            (
                Self::Completed,
                Some(crate::downloader::EngineEndSignal::HlsEndlist)
            )
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
    use crate::downloader::{DownloadStopCause, EngineEndSignal};
    use std::time::Duration;

    #[test]
    fn session_state_recording_constructor_marks_recording() {
        let now = Utc::now();
        let s = SessionState::recording("streamer-1", "session-1", now);
        assert!(s.is_recording());
        assert!(!s.is_hysteresis());
        assert!(!s.is_ended());
        assert!(s.is_active());
        assert_eq!(s.streamer_id(), "streamer-1");
        assert_eq!(s.session_id(), "session-1");
        assert_eq!(s.started_at(), now);
        assert_eq!(s.kind_str(), "recording");
    }

    #[test]
    fn session_state_hysteresis_state() {
        let now = Utc::now();
        let deadline = Instant::now() + Duration::from_secs(60);
        let s = SessionState::hysteresis(
            "s1",
            "session-1",
            now,
            now,
            TerminalCause::Failed {
                kind: DownloadFailureKind::Network,
            },
            deadline,
        );
        assert!(!s.is_recording());
        assert!(s.is_hysteresis());
        assert!(!s.is_ended());
        assert!(s.is_active(), "Hysteresis is still considered active for is_live queries");
        assert_eq!(s.kind_str(), "hysteresis");
    }

    #[test]
    fn session_state_ended_is_inactive() {
        let now = Utc::now();
        let s = SessionState::ended("s1", "sess", now, now, TerminalCause::Completed);
        assert!(!s.is_active());
        assert!(s.is_ended());
        assert_eq!(s.kind_str(), "ended");
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
    fn authority_definitive_offline_is_authoritative() {
        let c = TerminalCause::DefinitiveOffline {
            signal: OfflineSignal::PlaylistGone(404),
        };
        assert!(c.is_authoritative_end());
        assert!(c.is_authoritative_end_with_signal(None));
    }

    #[test]
    fn authority_streamer_offline_is_authoritative() {
        assert!(TerminalCause::StreamerOffline.is_authoritative_end());
    }

    #[test]
    fn authority_failed_is_not_authoritative_without_signal() {
        let c = TerminalCause::Failed {
            kind: DownloadFailureKind::Network,
        };
        assert!(!c.is_authoritative_end());
    }

    #[test]
    fn authority_completed_default_not_authoritative() {
        assert!(!TerminalCause::Completed.is_authoritative_end());
    }

    #[test]
    fn authority_completed_with_hls_endlist_is_authoritative() {
        assert!(
            TerminalCause::Completed
                .is_authoritative_end_with_signal(Some(EngineEndSignal::HlsEndlist))
        );
    }

    #[test]
    fn authority_completed_with_clean_disconnect_is_not_authoritative() {
        assert!(
            !TerminalCause::Completed.is_authoritative_end_with_signal(Some(
                EngineEndSignal::CleanDisconnect
            ))
        );
    }

    #[test]
    fn authority_rejected_is_authoritative_direct_to_ended() {
        let c = TerminalCause::Rejected {
            reason: "circuit breaker".into(),
        };
        assert!(c.is_authoritative_end());
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
