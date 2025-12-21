//! Streamer database model.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

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
    /// Timestamp of the last detected live event
    pub last_live_time: Option<String>,
    /// JSON blob for streamer-specific overrides
    pub streamer_specific_config: Option<String>,
    /// Number of consecutive errors encountered
    pub consecutive_error_count: Option<i32>,
    /// If temporarily disabled, the time it will be re-enabled
    pub disabled_until: Option<String>,
    /// Last recorded error message
    pub last_error: Option<String>,
    /// Creation timestamp
    pub created_at: String,
    /// Last update timestamp
    pub updated_at: String,
}

impl StreamerDbModel {
    /// Create a new streamer with default values.
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        platform_config_id: impl Into<String>,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
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
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

/// Streamer operational states.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StreamerState {
    /// The streamer is offline.
    NotLive,
    /// The streamer is currently live.
    Live,
    /// The streamer is online but outside the time window defined by filters.
    OutOfSchedule,
    /// The system has detected insufficient disk space.
    OutOfSpace,
    /// A persistent error is preventing monitoring.
    FatalError,
    /// Monitoring for this streamer has been manually stopped.
    Cancelled,
    /// The streamer's URL or ID is invalid on the platform.
    NotFound,
    /// The system is currently checking the streamer's status.
    InspectingLive,
    /// The streamer has been temporarily disabled due to repeated errors.
    TemporalDisabled,
}

impl StreamerState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotLive => "NOT_LIVE",
            Self::Live => "LIVE",
            Self::OutOfSchedule => "OUT_OF_SCHEDULE",
            Self::OutOfSpace => "OUT_OF_SPACE",
            Self::FatalError => "FATAL_ERROR",
            Self::Cancelled => "CANCELLED",
            Self::NotFound => "NOT_FOUND",
            Self::InspectingLive => "INSPECTING_LIVE",
            Self::TemporalDisabled => "TEMPORAL_DISABLED",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "NOT_LIVE" => Some(Self::NotLive),
            "LIVE" => Some(Self::Live),
            "OUT_OF_SCHEDULE" => Some(Self::OutOfSchedule),
            "OUT_OF_SPACE" => Some(Self::OutOfSpace),
            "FATAL_ERROR" => Some(Self::FatalError),
            "CANCELLED" => Some(Self::Cancelled),
            "NOT_FOUND" => Some(Self::NotFound),
            "INSPECTING_LIVE" => Some(Self::InspectingLive),
            "TEMPORAL_DISABLED" => Some(Self::TemporalDisabled),
            _ => None,
        }
    }

    /// Check if this state allows transitioning to another state.
    pub fn can_transition_to(&self, target: StreamerState) -> bool {
        use StreamerState::*;
        match (self, target) {
            // From NotLive, can go to Live, InspectingLive, or error states
            (NotLive, Live | InspectingLive | FatalError | NotFound | OutOfSpace) => true,
            // From Live, can go to NotLive, OutOfSchedule, or error states
            (Live, NotLive | OutOfSchedule | FatalError | OutOfSpace) => true,
            // From InspectingLive, can go to any state
            (InspectingLive, _) => true,
            // From OutOfSchedule, can go to Live, NotLive, or error states
            (OutOfSchedule, Live | NotLive | FatalError | OutOfSpace) => true,
            // From error states, can recover to NotLive or InspectingLive
            (FatalError | OutOfSpace | NotFound | TemporalDisabled, NotLive | InspectingLive) => {
                true
            }
            // Cancelled can only go to NotLive
            (Cancelled, NotLive) => true,
            // Any state can be cancelled
            (_, Cancelled) => true,
            // TemporalDisabled can be set from error states
            (FatalError, TemporalDisabled) => true,
            _ => false,
        }
    }
}

/// Streamer priority levels for resource allocation.
/// Note: Ord is manually implemented so High > Normal > Low
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    /// VIP streamers, never miss. First to get download slots.
    High,
    /// Standard streamers. Fair scheduling.
    #[default]
    Normal,
    /// Background/archive streamers. Paused first during resource constraints.
    Low,
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher priority = higher numeric value
        let self_val = match self {
            Priority::High => 3,
            Priority::Normal => 2,
            Priority::Low => 1,
        };
        let other_val = match other {
            Priority::High => 3,
            Priority::Normal => 2,
            Priority::Low => 1,
        };
        self_val.cmp(&other_val)
    }
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "HIGH",
            Self::Normal => "NORMAL",
            Self::Low => "LOW",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "HIGH" => Some(Self::High),
            "NORMAL" => Some(Self::Normal),
            "LOW" => Some(Self::Low),
            _ => None,
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

    #[test]
    fn test_state_transitions() {
        assert!(StreamerState::NotLive.can_transition_to(StreamerState::Live));
        assert!(StreamerState::Live.can_transition_to(StreamerState::NotLive));
        assert!(StreamerState::InspectingLive.can_transition_to(StreamerState::Live));
        assert!(!StreamerState::Cancelled.can_transition_to(StreamerState::Live));
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn test_state_serialization() {
        let state = StreamerState::Live;
        assert_eq!(state.as_str(), "LIVE");
        assert_eq!(StreamerState::parse("LIVE"), Some(StreamerState::Live));
    }
}
