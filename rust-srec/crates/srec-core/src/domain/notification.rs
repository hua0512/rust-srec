//! Pure notification and outbox domain value objects shared across layers.

use std::sync::Arc;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::domain::StreamerState;
use crate::domain::credential::CredentialEvent;
use crate::domain::streamer::FatalErrorType;

/// Runtime alerts emitted by recording, credential, and filesystem subsystems
/// without depending on the notification delivery layer.
#[derive(Debug, Clone)]
pub enum RuntimeAlert {
    /// Credential lifecycle event (refresh, invalidation, expiry, save failure).
    Credential(CredentialEvent),
    /// A platform throttled requests on one route.
    PlatformThrottled {
        platform: String,
        route: String,
        proxy_name: Option<String>,
        retry_at: Timestamp,
        timestamp: Timestamp,
    },
    /// Recording output path became unwritable at the filesystem boundary.
    OutputPathInaccessible {
        path: String,
        error_kind: String,
        timestamp: Timestamp,
    },
    /// Bigo live push notifications stopped arriving or stayed disconnected.
    BigoNotificationsFailing {
        problem: BigoNotificationProblem,
        message: Option<String>,
        timestamp: Timestamp,
    },
    /// Bigo live push notifications recovered from a previously alerted problem.
    BigoNotificationsRestored {
        problem: BigoNotificationProblem,
        timestamp: Timestamp,
    },
}

/// Sink for fire-and-forget runtime alerts emitted by lower layers.
pub trait RuntimeAlertSink: Send + Sync {
    fn emit(self: Arc<Self>, alert: RuntimeAlert);
}

/// What stopped Bigo live notifications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BigoNotificationProblem {
    /// BIGO accepted the subscriptions but sends the account no pushes.
    NotArriving,
    /// The listener has been disconnected for several minutes.
    Disconnected,
}

/// How an active recording session was opened or resumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamOnlineKind {
    /// A brand-new session row was inserted.
    Created,
    /// An already-open session row (`end_time IS NULL`, e.g. after restart) was reused.
    ReusedActive,
    /// An open session resumed out of the in-memory hysteresis quiet period.
    HysteresisResumed,
}

/// Convert Unix epoch milliseconds to `Timestamp`, clamping out-of-range values
/// to the nearest representable bound.
#[inline]
fn ms_to_timestamp(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap_or(if ms.is_negative() {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    })
}

/// Notification-only durable outbox event persisted alongside state mutations.
///
/// Serializes in an externally tagged wire envelope (`{"StreamOnline": ...}`,
/// `{"FatalError": ...}`, etc.) with required epoch-millisecond fields so
/// outbox readers unambiguously distinguish these records from legacy
/// `MonitorEvent` rows while preserving the `FatalError` payload keys
/// (`message`, RFC 3339 `timestamp`) read from `monitor_event_outbox`.
#[derive(Debug, Clone, PartialEq)]
pub enum OutboxEvent {
    StreamOnline {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        title: String,
        category: Option<String>,
        started_at_ms: i64,
        kind: StreamOnlineKind,
    },
    StreamOffline {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        duration_secs: Option<f64>,
        ended_at_ms: i64,
    },
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        occurred_at_ms: i64,
    },
    ChecksFailing {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        last_error: String,
        consecutive_errors: i32,
        occurred_at_ms: i64,
    },
    ChecksRestored {
        streamer_id: String,
        streamer_name: String,
        occurred_at_ms: i64,
    },
}

impl OutboxEvent {
    /// Stable `monitor_event_outbox.event_type` column discriminator.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::StreamOnline { .. } => "StreamOnline",
            Self::StreamOffline { .. } => "StreamOffline",
            Self::FatalError { .. } => "FatalError",
            Self::ChecksFailing { .. } => "ChecksFailing",
            Self::ChecksRestored { .. } => "ChecksRestored",
        }
    }
}

#[derive(Serialize, Deserialize)]
enum OutboxEventWire {
    StreamOnline {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        title: String,
        category: Option<String>,
        started_at_ms: i64,
        kind: StreamOnlineKind,
    },
    StreamOffline {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        duration_secs: Option<f64>,
        ended_at_ms: i64,
    },
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        occurred_at_ms: i64,
        #[serde(default)]
        timestamp: Option<Timestamp>,
    },
    ChecksFailing {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        last_error: String,
        consecutive_errors: i32,
        occurred_at_ms: i64,
        #[serde(default)]
        timestamp: Option<Timestamp>,
    },
    ChecksRestored {
        streamer_id: String,
        streamer_name: String,
        occurred_at_ms: i64,
        #[serde(default)]
        timestamp: Option<Timestamp>,
    },
}

impl Serialize for OutboxEvent {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = match self {
            Self::StreamOnline {
                streamer_id,
                streamer_name,
                session_id,
                title,
                category,
                started_at_ms,
                kind,
            } => OutboxEventWire::StreamOnline {
                streamer_id: streamer_id.clone(),
                streamer_name: streamer_name.clone(),
                session_id: session_id.clone(),
                title: title.clone(),
                category: category.clone(),
                started_at_ms: *started_at_ms,
                kind: *kind,
            },
            Self::StreamOffline {
                streamer_id,
                streamer_name,
                session_id,
                duration_secs,
                ended_at_ms,
            } => OutboxEventWire::StreamOffline {
                streamer_id: streamer_id.clone(),
                streamer_name: streamer_name.clone(),
                session_id: session_id.clone(),
                duration_secs: *duration_secs,
                ended_at_ms: *ended_at_ms,
            },
            Self::FatalError {
                streamer_id,
                streamer_name,
                error_type,
                message,
                new_state,
                occurred_at_ms,
            } => OutboxEventWire::FatalError {
                streamer_id: streamer_id.clone(),
                streamer_name: streamer_name.clone(),
                error_type: *error_type,
                message: message.clone(),
                new_state: *new_state,
                occurred_at_ms: *occurred_at_ms,
                timestamp: Some(ms_to_timestamp(*occurred_at_ms)),
            },
            Self::ChecksFailing {
                streamer_id,
                streamer_name,
                platform,
                last_error,
                consecutive_errors,
                occurred_at_ms,
            } => OutboxEventWire::ChecksFailing {
                streamer_id: streamer_id.clone(),
                streamer_name: streamer_name.clone(),
                platform: platform.clone(),
                last_error: last_error.clone(),
                consecutive_errors: *consecutive_errors,
                occurred_at_ms: *occurred_at_ms,
                timestamp: Some(ms_to_timestamp(*occurred_at_ms)),
            },
            Self::ChecksRestored {
                streamer_id,
                streamer_name,
                occurred_at_ms,
            } => OutboxEventWire::ChecksRestored {
                streamer_id: streamer_id.clone(),
                streamer_name: streamer_name.clone(),
                occurred_at_ms: *occurred_at_ms,
                timestamp: Some(ms_to_timestamp(*occurred_at_ms)),
            },
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutboxEvent {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = OutboxEventWire::deserialize(deserializer)?;
        Ok(match wire {
            OutboxEventWire::StreamOnline {
                streamer_id,
                streamer_name,
                session_id,
                title,
                category,
                started_at_ms,
                kind,
            } => Self::StreamOnline {
                streamer_id,
                streamer_name,
                session_id,
                title,
                category,
                started_at_ms,
                kind,
            },
            OutboxEventWire::StreamOffline {
                streamer_id,
                streamer_name,
                session_id,
                duration_secs,
                ended_at_ms,
            } => Self::StreamOffline {
                streamer_id,
                streamer_name,
                session_id,
                duration_secs,
                ended_at_ms,
            },
            OutboxEventWire::FatalError {
                streamer_id,
                streamer_name,
                error_type,
                message,
                new_state,
                occurred_at_ms,
                ..
            } => Self::FatalError {
                streamer_id,
                streamer_name,
                error_type,
                message,
                new_state,
                occurred_at_ms,
            },
            OutboxEventWire::ChecksFailing {
                streamer_id,
                streamer_name,
                platform,
                last_error,
                consecutive_errors,
                occurred_at_ms,
                ..
            } => Self::ChecksFailing {
                streamer_id,
                streamer_name,
                platform,
                last_error,
                consecutive_errors,
                occurred_at_ms,
            },
            OutboxEventWire::ChecksRestored {
                streamer_id,
                streamer_name,
                occurred_at_ms,
                ..
            } => Self::ChecksRestored {
                streamer_id,
                streamer_name,
                occurred_at_ms,
            },
        })
    }
}
