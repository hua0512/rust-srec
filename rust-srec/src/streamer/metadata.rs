//! Streamer metadata for in-memory storage.
//!
//! This module defines the lightweight metadata structure used for
//! in-memory streamer state management.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::{Priority, StreamerState};

/// Lightweight streamer metadata for in-memory storage.
///
/// This struct contains only the essential fields needed for
/// scheduling and state management, avoiding full entity hydration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamerMetadata {
    /// Unique identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Stream URL.
    pub url: String,
    /// Platform configuration ID.
    pub platform_config_id: String,
    /// Template configuration ID (optional).
    pub template_config_id: Option<String>,
    /// Current state.
    pub state: StreamerState,
    /// Priority level.
    pub priority: Priority,
    /// Consecutive error count for backoff calculation.
    pub consecutive_error_count: i32,
    /// Disabled until timestamp (for exponential backoff).
    pub disabled_until: Option<DateTime<Utc>>,
    /// Last time the streamer was live.
    pub last_live_time: Option<DateTime<Utc>>,
}

impl StreamerMetadata {
    /// Create new metadata from database model.
    pub fn from_db_model(model: &crate::database::models::StreamerDbModel) -> Self {
        Self {
            id: model.id.clone(),
            name: model.name.clone(),
            url: model.url.clone(),
            platform_config_id: model.platform_config_id.clone(),
            template_config_id: model.template_config_id.clone(),
            state: StreamerState::parse(&model.state).unwrap_or_default(),
            priority: Priority::parse(&model.priority).unwrap_or_default(),
            consecutive_error_count: model.consecutive_error_count.unwrap_or(0),
            disabled_until: model.disabled_until.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
            last_live_time: model.last_live_time.as_ref().and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            }),
        }
    }

    /// Check if the streamer is currently disabled (in backoff).
    pub fn is_disabled(&self) -> bool {
        self.disabled_until
            .map(|until| Utc::now() < until)
            .unwrap_or(false)
    }

    /// Check if the streamer is in an active state (can be checked/recorded).
    pub fn is_active(&self) -> bool {
        matches!(
            self.state,
            StreamerState::NotLive
                | StreamerState::Live
                | StreamerState::OutOfSchedule
                | StreamerState::InspectingLive
        )
    }

    /// Check if the streamer is ready for live checking.
    ///
    /// A streamer is ready if it's active and not currently disabled.
    pub fn is_ready_for_check(&self) -> bool {
        self.is_active() && !self.is_disabled()
    }

    /// Get the remaining backoff duration, if any.
    pub fn remaining_backoff(&self) -> Option<chrono::Duration> {
        self.disabled_until.and_then(|until| {
            let now = Utc::now();
            if until > now {
                Some(until - now)
            } else {
                None
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_metadata() -> StreamerMetadata {
        StreamerMetadata {
            id: "streamer-1".to_string(),
            name: "Test Streamer".to_string(),
            url: "https://twitch.tv/test".to_string(),
            platform_config_id: "twitch".to_string(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            consecutive_error_count: 0,
            disabled_until: None,
            last_live_time: None,
        }
    }

    #[test]
    fn test_is_disabled_when_not_set() {
        let metadata = create_test_metadata();
        assert!(!metadata.is_disabled());
    }

    #[test]
    fn test_is_disabled_when_in_future() {
        let mut metadata = create_test_metadata();
        metadata.disabled_until = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(metadata.is_disabled());
    }

    #[test]
    fn test_is_disabled_when_in_past() {
        let mut metadata = create_test_metadata();
        metadata.disabled_until = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(!metadata.is_disabled());
    }

    #[test]
    fn test_is_active_states() {
        let mut metadata = create_test_metadata();

        // Active states
        metadata.state = StreamerState::NotLive;
        assert!(metadata.is_active());

        metadata.state = StreamerState::Live;
        assert!(metadata.is_active());

        metadata.state = StreamerState::OutOfSchedule;
        assert!(metadata.is_active());

        metadata.state = StreamerState::InspectingLive;
        assert!(metadata.is_active());

        // Inactive states
        metadata.state = StreamerState::Disabled;
        assert!(!metadata.is_active());

        metadata.state = StreamerState::Error;
        assert!(!metadata.is_active());
    }

    #[test]
    fn test_is_ready_for_check() {
        let mut metadata = create_test_metadata();

        // Active and not disabled
        assert!(metadata.is_ready_for_check());

        // Active but disabled
        metadata.disabled_until = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(!metadata.is_ready_for_check());

        // Not active
        metadata.disabled_until = None;
        metadata.state = StreamerState::Disabled;
        assert!(!metadata.is_ready_for_check());
    }

    #[test]
    fn test_remaining_backoff() {
        let mut metadata = create_test_metadata();

        // No backoff
        assert!(metadata.remaining_backoff().is_none());

        // Future backoff
        metadata.disabled_until = Some(Utc::now() + chrono::Duration::minutes(30));
        let remaining = metadata.remaining_backoff();
        assert!(remaining.is_some());
        assert!(remaining.unwrap().num_minutes() > 0);

        // Past backoff
        metadata.disabled_until = Some(Utc::now() - chrono::Duration::hours(1));
        assert!(metadata.remaining_backoff().is_none());
    }
}
