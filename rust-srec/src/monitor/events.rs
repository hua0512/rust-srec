//! Monitor events for notification system.
//!
//! This module defines events that are emitted by the Stream Monitor
//! for consumption by the notification system.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::domain::StreamerState;

/// Re-export StreamInfo from platforms_parser for convenience.
pub use platforms_parser::media::StreamInfo;

/// Events emitted by the Stream Monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MonitorEvent {
    /// Streamer went live.
    StreamerLive {
        streamer_id: String,
        streamer_name: String,
        streamer_url: String,
        title: String,
        category: Option<String>,
        /// Available streams for download from platform parser.
        /// Note: Some platforms require calling get_url() to resolve the final URL.
        /// The StreamInfo contains headers in extras if is_headers_needed is true.
        streams: Vec<StreamInfo>,
        /// HTTP headers extracted from MediaInfo.extras (user-agent, referer, etc.).
        /// These should be merged with StreamInfo headers and passed to download engines.
        media_headers: Option<HashMap<String, String>>,
        timestamp: DateTime<Utc>,
    },
    /// Streamer went offline.
    StreamerOffline {
        streamer_id: String,
        streamer_name: String,
        /// Session ID if a session was active.
        session_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
    /// Fatal error occurred - monitoring stopped.
    FatalError {
        streamer_id: String,
        streamer_name: String,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        timestamp: DateTime<Utc>,
    },
    /// Transient error occurred - will retry.
    TransientError {
        streamer_id: String,
        streamer_name: String,
        error_message: String,
        consecutive_errors: i32,
        timestamp: DateTime<Utc>,
    },
    /// Streamer state changed.
    StateChanged {
        streamer_id: String,
        streamer_name: String,
        old_state: StreamerState,
        new_state: StreamerState,
        timestamp: DateTime<Utc>,
    },
}

/// Types of fatal errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FatalErrorType {
    /// Streamer not found on platform.
    NotFound,
    /// Streamer is banned.
    Banned,
    /// Content is age-restricted.
    AgeRestricted,
    /// Content is region-locked.
    RegionLocked,
    /// Content is private.
    Private,
    /// Platform is not supported.
    UnsupportedPlatform,
}

impl MonitorEvent {
    /// Get a human-readable description of the event.
    pub fn description(&self) -> String {
        match self {
            MonitorEvent::StreamerLive {
                streamer_name,
                title,
                ..
            } => {
                format!("{} is now live: {}", streamer_name, title)
            }
            MonitorEvent::StreamerOffline { streamer_name, .. } => {
                format!("{} went offline", streamer_name)
            }
            MonitorEvent::FatalError {
                streamer_name,
                error_type,
                message,
                ..
            } => {
                format!("{}: {:?} - {}", streamer_name, error_type, message)
            }
            MonitorEvent::TransientError {
                streamer_name,
                error_message,
                consecutive_errors,
                ..
            } => {
                format!(
                    "{}: {} (attempt {})",
                    streamer_name, error_message, consecutive_errors
                )
            }
            MonitorEvent::StateChanged {
                streamer_name,
                old_state,
                new_state,
                ..
            } => {
                format!("{}: {} -> {}", streamer_name, old_state, new_state)
            }
        }
    }

    /// Check if this event should trigger a notification.
    pub fn should_notify(&self) -> bool {
        match self {
            MonitorEvent::StreamerLive { .. } => true,
            MonitorEvent::StreamerOffline { .. } => true,
            MonitorEvent::FatalError { .. } => true,
            MonitorEvent::TransientError {
                consecutive_errors, ..
            } => {
                // Only notify after multiple consecutive errors
                *consecutive_errors >= 3
            }
            MonitorEvent::StateChanged { .. } => false,
        }
    }
}

/// Broadcaster for monitor events.
pub struct MonitorEventBroadcaster {
    sender: broadcast::Sender<MonitorEvent>,
}

impl MonitorEventBroadcaster {
    /// Create a new broadcaster with default capacity (256).
    pub fn new() -> Self {
        Self::with_capacity(256)
    }

    /// Create a new broadcaster with specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to monitor events.
    pub fn subscribe(&self) -> broadcast::Receiver<MonitorEvent> {
        self.sender.subscribe()
    }

    /// Publish a monitor event.
    pub fn publish(
        &self,
        event: MonitorEvent,
    ) -> Result<usize, broadcast::error::SendError<MonitorEvent>> {
        self.sender.send(event)
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for MonitorEventBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for MonitorEventBroadcaster {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use platforms_parser::media::{StreamFormat, formats::MediaFormat};

    fn create_test_stream() -> StreamInfo {
        StreamInfo {
            url: "https://example.com/stream.flv".to_string(),
            stream_format: StreamFormat::Flv,
            media_format: MediaFormat::Flv,
            quality: "best".to_string(),
            bitrate: 5000000,
            priority: 1,
            extras: None,
            codec: "h264".to_string(),
            fps: 30.0,
            is_headers_needed: false,
        }
    }

    #[test]
    fn test_event_description() {
        let event = MonitorEvent::StreamerLive {
            streamer_id: "123".to_string(),
            streamer_name: "TestStreamer".to_string(),
            streamer_url: "https://example.com/streamer".to_string(),
            title: "Playing Games".to_string(),
            category: Some("Gaming".to_string()),
            streams: vec![create_test_stream()],
            media_headers: None,
            timestamp: Utc::now(),
        };
        assert!(event.description().contains("TestStreamer"));
        assert!(event.description().contains("Playing Games"));
    }

    #[test]
    fn test_should_notify() {
        let live_event = MonitorEvent::StreamerLive {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            streamer_url: "https://example.com/test".to_string(),
            title: "Test".to_string(),
            category: None,
            streams: vec![],
            media_headers: None,
            timestamp: Utc::now(),
        };
        assert!(live_event.should_notify());

        let transient_error = MonitorEvent::TransientError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_message: "Network error".to_string(),
            consecutive_errors: 2,
            timestamp: Utc::now(),
        };
        assert!(!transient_error.should_notify());

        let transient_error_many = MonitorEvent::TransientError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_message: "Network error".to_string(),
            consecutive_errors: 3,
            timestamp: Utc::now(),
        };
        assert!(transient_error_many.should_notify());
    }

    #[test]
    fn test_broadcaster_publish_subscribe() {
        let broadcaster = MonitorEventBroadcaster::new();
        let mut receiver = broadcaster.subscribe();

        let event = MonitorEvent::StreamerOffline {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            session_id: None,
            timestamp: Utc::now(),
        };

        broadcaster.publish(event.clone()).unwrap();

        let received = receiver.try_recv().unwrap();
        assert!(matches!(received, MonitorEvent::StreamerOffline { .. }));
    }

    #[test]
    fn test_fatal_error_type() {
        let event = MonitorEvent::FatalError {
            streamer_id: "123".to_string(),
            streamer_name: "Test".to_string(),
            error_type: FatalErrorType::NotFound,
            message: "Streamer not found".to_string(),
            new_state: StreamerState::NotFound,
            timestamp: Utc::now(),
        };
        assert!(event.should_notify());
    }
}
