//! Session domain types — [`EngineKind`], [`TerminalCause`], and [`OfflineSignal`].

use crate::downloader::engine::EngineType;
use crate::downloader::{DownloadFailureKind, DownloadProtocol, DownloadStopCause};

pub use srec_core::domain::session::OfflineSignal;

/// Which download engine and protocol produced a download outcome.
///
/// Distinguishing engines matters because Mesio consecutive network failures
/// can indicate a definitive stream end, whereas external subprocess errors
/// (`ffmpeg` / `streamlink`) are deferred to status probing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    MesioHls,
    MesioFlv,
    MesioDash,
    MesioUnknown,
    Ffmpeg,
    Streamlink,
}

impl EngineKind {
    pub fn is_mesio(&self) -> bool {
        matches!(
            self,
            Self::MesioHls | Self::MesioFlv | Self::MesioDash | Self::MesioUnknown
        )
    }

    pub fn from_engine_and_protocol(engine_type: EngineType, protocol: DownloadProtocol) -> Self {
        match engine_type {
            EngineType::Ffmpeg => Self::Ffmpeg,
            EngineType::Streamlink => Self::Streamlink,
            EngineType::Mesio => match protocol {
                DownloadProtocol::Hls => Self::MesioHls,
                DownloadProtocol::Flv => Self::MesioFlv,
                DownloadProtocol::Dash => Self::MesioDash,
                DownloadProtocol::Unknown => Self::MesioUnknown,
            },
        }
    }
}

/// Why a recording session ended.
///
/// Each variant carries a centralized policy on whether the session-complete
/// pipeline should fire ([`Self::should_run_session_complete_pipeline`]) and
/// whether the end is authoritative enough to bypass hysteresis
/// ([`Self::is_authoritative_end_with_signal`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCause {
    /// Engine reached a clean end of stream (e.g. EOF on FLV, `#EXT-X-ENDLIST` on HLS).
    ///
    /// Whether this counts as authoritative for hysteresis depends on the
    /// engine end signal; prefer [`Self::is_authoritative_end_with_signal`].
    Completed,
    /// Engine gave up due to error. Whatever output is on disk is final.
    Failed { kind: DownloadFailureKind },
    /// External stop request whose engine did not report clean completion.
    /// The control-plane stop owner applies the authoritative session policy.
    Cancelled { cause: DownloadStopCause },
    /// Download never started (circuit breaker, output-root unavailable, etc.).
    Rejected { reason: String },
    /// Status probing authoritatively confirmed the streamer went offline.
    StreamerOffline,
    /// A definitive offline signal observed at the engine or danmu boundary
    /// (e.g. danmu stream closed, repeated Mesio network failures).
    DefinitiveOffline { signal: OfflineSignal },
    /// User explicitly disabled (or deleted) the streamer.
    ///
    /// Authoritative tear-down: bypasses hysteresis, runs the session-complete
    /// pipeline for captured outputs, and suppresses `StreamOffline`.
    UserDisabled,
    /// Recording stopped because the stream is still live but outside the
    /// configured recording schedule.
    ///
    /// Authoritative tear-down: bypasses hysteresis, runs the session-complete
    /// pipeline for captured outputs, and suppresses `StreamOffline`.
    OutOfSchedule,
}

impl TerminalCause {
    /// Whether this terminal cause should trigger the per-streamer
    /// `session_complete_pipeline` DAG.
    pub fn should_run_session_complete_pipeline(&self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed { .. }
                | Self::StreamerOffline
                | Self::DefinitiveOffline { .. }
                | Self::UserDisabled
                | Self::OutOfSchedule
        )
    }

    /// Whether this cause bypasses the hysteresis quiet period without an
    /// accompanying engine end signal.
    pub fn is_authoritative_end(&self) -> bool {
        match self {
            Self::DefinitiveOffline { .. } => true,
            Self::StreamerOffline => true,
            Self::Rejected { .. } => true,
            Self::UserDisabled => true,
            Self::OutOfSchedule => true,
            Self::Failed {
                kind: DownloadFailureKind::UnsupportedStream,
            } => true,
            Self::Completed => false,
            Self::Failed { .. } => false,
            Self::Cancelled { .. } => false,
        }
    }

    /// Authority decision when the engine's end signal is known: Mesio HLS with
    /// `#EXT-X-ENDLIST` and DASH presentation end are authoritative, while FLV
    /// clean disconnect and subprocess exit 0 wait out hysteresis.
    pub fn is_authoritative_end_with_signal(
        &self,
        signal: Option<crate::downloader::EngineEndSignal>,
    ) -> bool {
        if self.is_authoritative_end() {
            return true;
        }
        matches!(self, Self::Completed) && signal.is_some_and(|signal| signal.is_authoritative())
    }

    /// Short, stable string for logging and metrics labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed { .. } => "failed",
            Self::Cancelled { .. } => "cancelled",
            Self::Rejected { .. } => "rejected",
            Self::StreamerOffline => "streamer_offline",
            Self::DefinitiveOffline { .. } => "definitive_offline",
            Self::UserDisabled => "user_disabled",
            Self::OutOfSchedule => "out_of_schedule",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every cause that leaves recorded output runs the post-recording
    /// pipeline; a rejected start recorded nothing and a cancellation is not
    /// an end. The offline and user-disabled ends are pinned by the recording
    /// scenarios in `tests/e2e/recording.rs`.
    #[test]
    fn session_complete_pipeline_runs_unless_rejected_or_cancelled() {
        let cases = [
            (TerminalCause::Completed, true),
            (
                TerminalCause::Failed {
                    kind: DownloadFailureKind::Network,
                },
                true,
            ),
            (
                TerminalCause::DefinitiveOffline {
                    signal: OfflineSignal::ConsecutiveFailures(2),
                },
                true,
            ),
            (TerminalCause::OutOfSchedule, true),
            (
                TerminalCause::Rejected {
                    reason: "circuit breaker".into(),
                },
                false,
            ),
            (
                TerminalCause::Cancelled {
                    cause: DownloadStopCause::User,
                },
                false,
            ),
        ];
        for (cause, runs) in cases {
            assert_eq!(
                cause.should_run_session_complete_pipeline(),
                runs,
                "{cause:?}"
            );
        }
    }

    /// A completed download ends the session at once only when the platform
    /// asserted the end (HLS ENDLIST, the end of a DASH presentation). A clean
    /// disconnect or an ffmpeg/streamlink exit 0 may be followed by a
    /// reconnect, so those wait out hysteresis instead of splitting the
    /// session. A schedule stop ends it whatever the engine saw.
    #[test]
    fn only_platform_asserted_or_policy_ends_skip_hysteresis() {
        use crate::downloader::EngineEndSignal;

        for (signal, authoritative) in [
            (EngineEndSignal::HlsEndlist, true),
            (EngineEndSignal::DashEnd, true),
            (EngineEndSignal::CleanDisconnect, false),
            (EngineEndSignal::SubprocessExitZero, false),
            (EngineEndSignal::Unknown, false),
        ] {
            assert_eq!(signal.is_authoritative(), authoritative, "{signal:?}");
            assert_eq!(
                TerminalCause::Completed.is_authoritative_end_with_signal(Some(signal)),
                authoritative,
                "{signal:?}"
            );
            assert!(
                TerminalCause::OutOfSchedule.is_authoritative_end_with_signal(Some(signal)),
                "{signal:?}"
            );
        }
        assert!(!TerminalCause::Completed.is_authoritative_end_with_signal(None));
        assert!(TerminalCause::OutOfSchedule.is_authoritative_end_with_signal(None));
    }
}
