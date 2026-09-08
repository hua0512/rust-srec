//! Streamer entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::StreamerState;
use crate::domain::{Priority, StreamerUrl};

/// Streamer entity representing a content creator to be monitored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Streamer {
    pub id: String,
    pub name: String,
    pub url: StreamerUrl,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
    pub state: StreamerState,
    pub priority: Priority,
    pub last_live_time: Option<DateTime<Utc>>,
    pub streamer_specific_config: Option<serde_json::Value>,
    pub consecutive_error_count: i32,
    pub disabled_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Streamer {
    /// Create a new streamer.
    pub fn new(
        name: impl Into<String>,
        url: StreamerUrl,
        platform_config_id: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            url,
            platform_config_id: platform_config_id.into(),
            template_config_id: None,
            state: StreamerState::NotLive,
            priority: Priority::Normal,
            last_live_time: None,
            streamer_specific_config: None,
            consecutive_error_count: 0,
            disabled_until: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Set the template config.
    pub fn with_template(mut self, template_id: impl Into<String>) -> Self {
        self.template_config_id = Some(template_id.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_streamer() -> Streamer {
        Streamer::new(
            "test_streamer",
            StreamerUrl::from_trusted("https://www.twitch.tv/test"),
            "platform-1",
        )
    }

    #[test]
    fn test_new_streamer() {
        let streamer = create_test_streamer();
        assert_eq!(streamer.name, "test_streamer");
        assert_eq!(streamer.state, StreamerState::NotLive);
        assert_eq!(streamer.priority, Priority::Normal);
        assert_eq!(streamer.consecutive_error_count, 0);
    }
}
