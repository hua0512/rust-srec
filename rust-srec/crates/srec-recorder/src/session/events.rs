//! Wire-format types for the `session_events` audit log and conversion from
//! runtime [`TerminalCause`] values.

use crate::downloader::DownloadFailureKind;
use crate::session::state::TerminalCause;
pub use srec_core::domain::session::{
    SessionEvent, SessionEventKind, SessionEventPayload, TerminalCauseDto,
};

impl From<&TerminalCause> for TerminalCauseDto {
    fn from(cause: &TerminalCause) -> Self {
        match cause {
            TerminalCause::Completed => Self::Completed,
            TerminalCause::Failed { kind } => Self::Failed {
                kind: failure_kind_label(*kind).to_string(),
            },
            TerminalCause::Cancelled { cause } => Self::Cancelled {
                cause: cause.as_str().to_string(),
            },
            TerminalCause::Rejected { reason } => Self::Rejected {
                reason: reason.clone(),
            },
            TerminalCause::StreamerOffline => Self::StreamerOffline,
            TerminalCause::DefinitiveOffline { signal } => Self::DefinitiveOffline {
                signal: signal.clone(),
            },
            TerminalCause::UserDisabled => Self::UserDisabled,
            TerminalCause::OutOfSchedule => Self::OutOfSchedule,
        }
    }
}

/// Stable lowercase string label for a [`DownloadFailureKind`] — used in the
/// `TerminalCauseDto::Failed.kind` field and as the i18n key on the frontend.
fn failure_kind_label(kind: DownloadFailureKind) -> &'static str {
    match kind {
        DownloadFailureKind::HttpClientError { .. } => "http_client_error",
        DownloadFailureKind::RateLimited => "rate_limited",
        DownloadFailureKind::HttpServerError { .. } => "http_server_error",
        DownloadFailureKind::Network => "network",
        DownloadFailureKind::Io => "io",
        DownloadFailureKind::OutputRootUnavailable { .. } => "output_root_unavailable",
        DownloadFailureKind::SourceUnavailable => "source_unavailable",
        DownloadFailureKind::Configuration => "configuration",
        DownloadFailureKind::UnsupportedStream => "unsupported_stream",
        DownloadFailureKind::ProcessExit { .. } => "process_exit",
        DownloadFailureKind::Processing => "processing",
        DownloadFailureKind::Cancelled => "cancelled",
        DownloadFailureKind::Other => "other",
        DownloadFailureKind::CredentialRecovery => "credential_recovery",
        DownloadFailureKind::CredentialUnavailable => "credential_unavailable",
    }
}
