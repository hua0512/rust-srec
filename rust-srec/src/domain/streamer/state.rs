//! Streamer state machine.

use serde::{Deserialize, Serialize};
use crate::Error;

/// Streamer operational states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StreamerState {
    /// The streamer is offline.
    #[default]
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
    /// Convert to database string representation.
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

    /// Parse from database string representation.
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

    /// Check if this is an error state.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::FatalError | Self::OutOfSpace | Self::NotFound | Self::TemporalDisabled)
    }

    /// Check if this state allows monitoring.
    pub fn allows_monitoring(&self) -> bool {
        matches!(self, Self::NotLive | Self::Live | Self::OutOfSchedule | Self::InspectingLive)
    }

    /// Check if this state indicates the streamer is active (being monitored).
    pub fn is_active(&self) -> bool {
        !matches!(self, Self::Cancelled | Self::FatalError | Self::NotFound)
    }

    /// Validate a state transition.
    pub fn can_transition_to(&self, target: StreamerState) -> bool {
        use StreamerState::*;
        
        match (self, target) {
            // Same state is always allowed
            (from, to) if from == &to => true,
            
            // From NotLive
            (NotLive, Live | InspectingLive | FatalError | NotFound | OutOfSpace | Cancelled) => true,
            
            // From Live
            (Live, NotLive | OutOfSchedule | FatalError | OutOfSpace | Cancelled) => true,
            
            // From InspectingLive - can go to any state
            (InspectingLive, _) => true,
            
            // From OutOfSchedule
            (OutOfSchedule, Live | NotLive | FatalError | OutOfSpace | Cancelled) => true,
            
            // From error states - can recover to NotLive or InspectingLive
            (FatalError | OutOfSpace | NotFound, NotLive | InspectingLive | Cancelled) => true,
            
            // From TemporalDisabled - can recover
            (TemporalDisabled, NotLive | InspectingLive | Cancelled) => true,
            
            // FatalError can transition to TemporalDisabled
            (FatalError, TemporalDisabled) => true,
            
            // Cancelled can only go to NotLive
            (Cancelled, NotLive) => true,
            
            // Any state can be cancelled
            (_, Cancelled) => true,
            
            _ => false,
        }
    }

    /// Attempt to transition to a new state.
    pub fn transition_to(&self, target: StreamerState) -> Result<StreamerState, Error> {
        if self.can_transition_to(target) {
            Ok(target)
        } else {
            Err(Error::InvalidStateTransition {
                from: self.as_str().to_string(),
                to: target.as_str().to_string(),
            })
        }
    }
}

impl std::fmt::Display for StreamerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_from_str() {
        assert_eq!(StreamerState::parse("LIVE"), Some(StreamerState::Live));
        assert_eq!(StreamerState::parse("NOT_LIVE"), Some(StreamerState::NotLive));
        assert_eq!(StreamerState::parse("invalid"), None);
    }

    #[test]
    fn test_state_is_error() {
        assert!(StreamerState::FatalError.is_error());
        assert!(StreamerState::OutOfSpace.is_error());
        assert!(!StreamerState::Live.is_error());
        assert!(!StreamerState::NotLive.is_error());
    }

    #[test]
    fn test_valid_transitions() {
        assert!(StreamerState::NotLive.can_transition_to(StreamerState::Live));
        assert!(StreamerState::Live.can_transition_to(StreamerState::NotLive));
        assert!(StreamerState::InspectingLive.can_transition_to(StreamerState::Live));
        assert!(StreamerState::FatalError.can_transition_to(StreamerState::NotLive));
    }

    #[test]
    fn test_invalid_transitions() {
        assert!(!StreamerState::Cancelled.can_transition_to(StreamerState::Live));
        assert!(!StreamerState::NotFound.can_transition_to(StreamerState::Live));
    }

    #[test]
    fn test_transition_to() {
        let state = StreamerState::NotLive;
        let new_state = state.transition_to(StreamerState::Live).unwrap();
        assert_eq!(new_state, StreamerState::Live);
    }

    #[test]
    fn test_transition_to_error() {
        let state = StreamerState::Cancelled;
        let result = state.transition_to(StreamerState::Live);
        assert!(result.is_err());
    }
}
