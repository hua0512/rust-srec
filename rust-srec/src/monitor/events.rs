//! Legacy monitor outbox wire-format schema.
//!
//! Kept solely so [`crate::notification::NotificationOutboxDrainer`] can decode
//! undelivered `monitor_event_outbox` rows written before migration to
//! [`crate::recorder::OutboxEvent`].

use std::collections::HashMap;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::domain::StreamerState;
use crate::domain::streamer::FatalErrorType;

/// Re-export StreamInfo from platforms_parser for convenience.
pub use platforms_parser::media::StreamInfo;

/// Legacy `monitor_event_outbox` event wire format retained for upgrade decoding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitorEvent {
    /// Streamer went live.
    StreamerLive {
        /// A prior process cannot authorize reuse of its extracted media.
        #[serde(default)]
        runtime_instance: Option<String>,
        #[serde(default)]
        credential_binding: Option<Box<crate::credentials::CredentialBinding>>,
        streamer_id: String,
        session_id: String,
        streamer_name: String,
        streamer_url: String,
        title: String,
        category: Option<String>,
        streams: Vec<StreamInfo>,
        media_headers: Option<HashMap<String, String>>,
        media_extras: Option<Box<HashMap<String, String>>>,
        timestamp: Timestamp,
    },
    /// Streamer went offline.
    StreamerOffline {
        streamer_id: String,
        streamer_name: String,
        session_id: Option<String>,
        timestamp: Timestamp,
    },
    /// Fatal error occurred - monitoring stopped.
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        timestamp: Timestamp,
    },
    /// Transient error occurred - will retry.
    TransientError {
        streamer_id: String,
        streamer_name: String,
        error_message: String,
        consecutive_errors: i32,
        timestamp: Timestamp,
    },
    /// Checks for a streamer failed often enough to start the error backoff.
    ChecksFailing {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        last_error: String,
        consecutive_errors: i32,
        timestamp: Timestamp,
    },
    /// A successful status check or recording progress cleared a run of
    /// failures that had sent [`Self::ChecksFailing`].
    ChecksRestored {
        streamer_id: String,
        streamer_name: String,
        timestamp: Timestamp,
    },
    /// Streamer state changed.
    StateChanged {
        streamer_id: String,
        streamer_name: String,
        old_state: StreamerState,
        new_state: StreamerState,
        #[serde(default)]
        reason: Option<String>,
        timestamp: Timestamp,
    },
}
