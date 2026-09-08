//! Streamer database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::domain::{Priority, StreamerState};

/// Streamer database model.
/// The central entity representing a content creator to be monitored.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct StreamerDbModel {
    pub id: String,
    pub name: String,
    pub url: String,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
    /// Current operational state (NOT_LIVE, LIVE, OUT_OF_SCHEDULE, etc.)
    pub state: String,
    /// Priority level for resource allocation (HIGH, NORMAL, LOW)
    pub priority: String,
    /// Avatar URL (optional).
    pub avatar: Option<String>,
    /// Unix epoch milliseconds (UTC) of the last detected live event.
    pub last_live_time: Option<i64>,
    /// JSON blob for streamer-specific overrides
    pub streamer_specific_config: Option<String>,
    /// Number of consecutive errors encountered
    pub consecutive_error_count: Option<i32>,
    /// If temporarily disabled, the time it will be re-enabled (epoch ms).
    pub disabled_until: Option<i64>,
    /// Last recorded error message
    pub last_error: Option<String>,
    /// Unix epoch milliseconds (UTC) when created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when last updated.
    pub updated_at: i64,
    /// Unix epoch milliseconds (UTC) when the streamer was deleted, or `None`
    /// while it is a normal streamer.
    ///
    /// Set by `StreamerRepository::mark_streamer_deleted` (or, for a
    /// configuration import, by the same statement inside the import's
    /// transaction). While it is set the row still exists but
    /// `StreamerMetadata::is_active` is false, so no runtime owner starts new
    /// work for it; `StreamerManager::reap_deleted` issues the physical
    /// `DELETE` once `RuntimeCoordinator::retire_streamer` reports the owners
    /// have stopped.
    pub deleted_at: Option<i64>,
}

impl StreamerDbModel {
    /// Create a new streamer with default values.
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        platform_config_id: impl Into<String>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            url: url.into(),
            platform_config_id: platform_config_id.into(),
            template_config_id: None,
            state: StreamerState::NotLive.as_str().to_string(),
            priority: Priority::Normal.as_str().to_string(),
            avatar: None,
            last_live_time: None,
            streamer_specific_config: None,
            consecutive_error_count: Some(0),
            disabled_until: None,
            last_error: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_streamer_new() {
        let streamer = StreamerDbModel::new("test", "https://example.com/test", "platform-1");
        assert_eq!(streamer.name, "test");
        assert_eq!(streamer.state, "NOT_LIVE");
        assert_eq!(streamer.priority, "NORMAL");
        assert_eq!(streamer.consecutive_error_count, Some(0));
    }
}
