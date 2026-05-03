//! Wire-format types for the `session_events` audit log.
//!
//! Two parallel views of the same data:
//!
//! - [`SessionEventKind`] — a 4-variant discriminator that mirrors the
//!   `kind` column on the `session_events` table (`session_started`,
//!   `hysteresis_entered`, `session_resumed`, `session_ended`). Used for
//!   indexing, filtering, and the API response's `kind` field.
//! - [`SessionEventPayload`] — a `#[serde(tag = "kind")]` discriminated
//!   union that carries the typed details (cause, hysteresis duration,
//!   etc.). Serialised into `session_events.payload` and into the API
//!   response's `payload` field.
//!
//! The two views agree by construction: `payload.kind() == kind` for every
//! valid row. Tests assert the round-trip.
//!
//! [`TerminalCauseDto`] mirrors [`crate::session::TerminalCause`] for the
//! wire — the domain type stays serde-free so we don't accidentally couple
//! its on-disk JSON shape to other call sites.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::downloader::DownloadFailureKind;
use crate::session::state::{OfflineSignal, TerminalCause};

/// Discriminator for the `session_events.kind` column.
///
/// Stays in sync with the SQL `CHECK` constraint on the table — a typo here
/// blows up at insert time rather than producing rows the deserializer can't
/// read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEventKind {
    SessionStarted,
    HysteresisEntered,
    SessionResumed,
    SessionEnded,
}

impl SessionEventKind {
    /// Stable string label. Identical to the SQL `CHECK` constraint values
    /// and to the JSON discriminator string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session_started",
            Self::HysteresisEntered => "hysteresis_entered",
            Self::SessionResumed => "session_resumed",
            Self::SessionEnded => "session_ended",
        }
    }
}

/// Typed payload for a session event. Serialised into the `session_events.
/// payload` column with the discriminator under the `"kind"` key — the
/// frontend reads it as a Zod discriminated union.
///
/// The `kind` discriminator is chosen so it lines up with the top-level
/// `kind` column on the row (no separate JSON key for type tagging).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventPayload {
    /// A new session began. Emitted from
    /// [`crate::session::SessionLifecycle::on_live_detected`] on the
    /// `Created` outcome of `start_or_resume`. `from_hysteresis` is `true`
    /// only on the resume path (`Hysteresis → Recording`).
    SessionStarted {
        from_hysteresis: bool,
        title: Option<String>,
    },
    /// The download ended with a non-authoritative cause; the lifecycle is
    /// waiting for either a resume or the backstop timer to elapse before
    /// committing `Ended`.
    HysteresisEntered {
        cause: TerminalCauseDto,
        resume_deadline: DateTime<Utc>,
    },
    /// A `LiveDetected` arrived inside the hysteresis window — the session
    /// continues with the same `session_id`.
    SessionResumed { hysteresis_duration_secs: u64 },
    /// The session committed to `Ended`. `via_hysteresis = true` if it was
    /// the backstop timer that fired; `false` for direct authoritative ends.
    SessionEnded {
        cause: TerminalCauseDto,
        via_hysteresis: bool,
    },
}

impl SessionEventPayload {
    /// Discriminator for the row's `kind` column. Preserves the invariant
    /// `payload.kind().as_str() == row.kind`.
    pub fn kind(&self) -> SessionEventKind {
        match self {
            Self::SessionStarted { .. } => SessionEventKind::SessionStarted,
            Self::HysteresisEntered { .. } => SessionEventKind::HysteresisEntered,
            Self::SessionResumed { .. } => SessionEventKind::SessionResumed,
            Self::SessionEnded { .. } => SessionEventKind::SessionEnded,
        }
    }
}

/// Wire-format DTO for [`TerminalCause`]. Keeps the domain enum serde-free
/// (some inner types like [`DownloadStopCause`] are not `Serialize`, and
/// adding the derive would risk drift in unrelated call sites).
///
/// The shape is:
/// - `{ "type": "completed" }`
/// - `{ "type": "failed", "kind": "network" }`
/// - `{ "type": "cancelled", "cause": "danmu_stream_closed" }`
/// - `{ "type": "rejected", "reason": "..." }`
/// - `{ "type": "streamer_offline" }`
/// - `{ "type": "definitive_offline", "signal": { "type": "danmu_stream_closed" } }`
/// - `{ "type": "user_disabled" }`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalCauseDto {
    Completed,
    Failed { kind: String },
    Cancelled { cause: String },
    Rejected { reason: String },
    StreamerOffline,
    DefinitiveOffline { signal: OfflineSignal },
    UserDisabled,
}

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
        }
    }
}

/// Stable lowercase string label for a [`DownloadFailureKind`] — used in the
/// `TerminalCauseDto::Failed.kind` field and as the i18n key on the
/// frontend. We carry only the variant tag (no inner status code or
/// `io_kind`) because the audit log surfaces the cause class, not the
/// underlying error detail.
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
        DownloadFailureKind::ProcessExit { .. } => "process_exit",
        DownloadFailureKind::Processing => "processing",
        DownloadFailureKind::Cancelled => "cancelled",
        DownloadFailureKind::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloader::DownloadStopCause;

    fn round_trip(payload: &SessionEventPayload) {
        let json = serde_json::to_string(payload).expect("serialise");
        let back: SessionEventPayload = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(payload, &back, "round trip for {json}");
    }

    #[test]
    fn payload_round_trip_session_started() {
        round_trip(&SessionEventPayload::SessionStarted {
            from_hysteresis: false,
            title: Some("Hello".to_string()),
        });
        round_trip(&SessionEventPayload::SessionStarted {
            from_hysteresis: true,
            title: None,
        });
    }

    #[test]
    fn payload_round_trip_hysteresis_entered() {
        round_trip(&SessionEventPayload::HysteresisEntered {
            cause: TerminalCauseDto::Cancelled {
                cause: "danmu_stream_closed".into(),
            },
            resume_deadline: Utc::now(),
        });
    }

    #[test]
    fn payload_round_trip_session_resumed() {
        round_trip(&SessionEventPayload::SessionResumed {
            hysteresis_duration_secs: 42,
        });
    }

    #[test]
    fn payload_round_trip_session_ended() {
        round_trip(&SessionEventPayload::SessionEnded {
            cause: TerminalCauseDto::DefinitiveOffline {
                signal: OfflineSignal::DanmuStreamClosed,
            },
            via_hysteresis: true,
        });
    }

    #[test]
    fn kind_string_matches_table_check_constraint() {
        // The `CHECK (kind IN (...))` constraint on `session_events` lists
        // these exact strings. If a label here drifts, every insert would
        // fail at runtime — pin it down with a test.
        assert_eq!(SessionEventKind::SessionStarted.as_str(), "session_started");
        assert_eq!(
            SessionEventKind::HysteresisEntered.as_str(),
            "hysteresis_entered"
        );
        assert_eq!(SessionEventKind::SessionResumed.as_str(), "session_resumed");
        assert_eq!(SessionEventKind::SessionEnded.as_str(), "session_ended");
    }

    #[test]
    fn payload_kind_matches_serialised_discriminator() {
        let cases = [
            (
                SessionEventPayload::SessionStarted {
                    from_hysteresis: false,
                    title: None,
                },
                "session_started",
            ),
            (
                SessionEventPayload::HysteresisEntered {
                    cause: TerminalCauseDto::Completed,
                    resume_deadline: Utc::now(),
                },
                "hysteresis_entered",
            ),
            (
                SessionEventPayload::SessionResumed {
                    hysteresis_duration_secs: 1,
                },
                "session_resumed",
            ),
            (
                SessionEventPayload::SessionEnded {
                    cause: TerminalCauseDto::StreamerOffline,
                    via_hysteresis: false,
                },
                "session_ended",
            ),
        ];
        for (payload, expected) in cases {
            assert_eq!(payload.kind().as_str(), expected);
            let json = serde_json::to_value(&payload).unwrap();
            assert_eq!(
                json.get("kind").and_then(|v| v.as_str()),
                Some(expected),
                "JSON discriminator must equal the kind column"
            );
        }
    }

    #[test]
    fn terminal_cause_dto_from_definitive_offline_preserves_signal() {
        let cause = TerminalCause::DefinitiveOffline {
            signal: OfflineSignal::DanmuStreamClosed,
        };
        let dto = TerminalCauseDto::from(&cause);
        assert_eq!(
            dto,
            TerminalCauseDto::DefinitiveOffline {
                signal: OfflineSignal::DanmuStreamClosed,
            }
        );
        // Snapshot the exact JSON shape the frontend will see.
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["type"], "definitive_offline");
        assert_eq!(json["signal"]["type"], "danmu_stream_closed");
    }

    #[test]
    fn terminal_cause_dto_from_failed_uses_label() {
        let dto = TerminalCauseDto::from(&TerminalCause::Failed {
            kind: DownloadFailureKind::Network,
        });
        assert_eq!(
            dto,
            TerminalCauseDto::Failed {
                kind: "network".into()
            }
        );
    }

    #[test]
    fn terminal_cause_dto_from_user_disabled_round_trip() {
        let cause = TerminalCause::UserDisabled;
        let dto = TerminalCauseDto::from(&cause);
        assert_eq!(dto, TerminalCauseDto::UserDisabled);

        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["type"], "user_disabled");

        // Round-trip through SessionEnded payload.
        let payload = SessionEventPayload::SessionEnded {
            cause: dto,
            via_hysteresis: false,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: SessionEventPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(payload, back);
    }

    #[test]
    fn terminal_cause_dto_from_cancelled_uses_stop_cause_label() {
        let dto = TerminalCauseDto::from(&TerminalCause::Cancelled {
            cause: DownloadStopCause::DanmuStreamClosed,
        });
        assert_eq!(
            dto,
            TerminalCauseDto::Cancelled {
                cause: "danmu_stream_closed".into()
            }
        );
    }
}
