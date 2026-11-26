//! Pure wire-format types for recording session lifecycle events and offline signals.
//!
//! Two parallel views of the same audit-log data:
//!
//! - [`SessionEventKind`] — a 4-variant discriminator matching the `kind`
//!   column on the `session_events` table (`session_started`,
//!   `hysteresis_entered`, `session_resumed`, `session_ended`).
//! - [`SessionEventPayload`] — a `#[serde(tag = "kind")]` discriminated union
//!   carrying the typed details serialized into `session_events.payload`.
//!
//! The two views agree by construction: `payload.kind() == kind` for every
//! valid row.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// A high-confidence offline signal observed at the engine or danmu boundary
/// that bypasses the hysteresis quiet period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "count", rename_all = "snake_case")]
pub enum OfflineSignal {
    /// Danmu websocket sent an explicit stream-closed control frame.
    DanmuStreamClosed,
    /// `n` consecutive network failures inside the sliding failure window.
    ConsecutiveFailures(u32),
}

impl OfflineSignal {
    /// Short, stable string for logging and metrics labels.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DanmuStreamClosed => "danmu_stream_closed",
            Self::ConsecutiveFailures(_) => "consecutive_failures",
        }
    }
}

/// Persisted lifecycle audit event for a recording session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEvent {
    pub id: i64,
    pub session_id: String,
    pub streamer_id: String,
    pub kind: String,
    pub occurred_at: Timestamp,
    pub payload: Option<SessionEventPayload>,
}

/// Discriminator for the `session_events.kind` column.
///
/// Stays in sync with the SQL `CHECK` constraint on the table — a typo here
/// fails at insert time rather than producing rows the deserializer cannot
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
    /// A new session began. `from_hysteresis` is `true` only on the resume path.
    SessionStarted {
        from_hysteresis: bool,
        title: Option<String>,
    },
    /// The download ended with a non-authoritative cause; the lifecycle is
    /// waiting for either a resume or the backstop timer to elapse before
    /// committing `Ended`.
    HysteresisEntered {
        cause: TerminalCauseDto,
        resume_deadline: Timestamp,
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

/// Wire-format DTO for a session terminal cause, decoupled from the runtime
/// downloader types so the on-disk JSON shape stays stable.
///
/// The shape is:
/// - `{ "type": "completed" }`
/// - `{ "type": "failed", "kind": "network" }`
/// - `{ "type": "cancelled", "cause": "danmu_stream_closed" }`
/// - `{ "type": "rejected", "reason": "..." }`
/// - `{ "type": "streamer_offline" }`
/// - `{ "type": "definitive_offline", "signal": { "type": "danmu_stream_closed" } }`
/// - `{ "type": "definitive_offline", "signal": { "type": "consecutive_failures", "count": 2 } }`
/// - `{ "type": "user_disabled" }`
/// - `{ "type": "out_of_schedule" }`
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
    OutOfSchedule,
}
