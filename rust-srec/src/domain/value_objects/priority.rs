//! Priority value object.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// Priority level for resource allocation.
///
/// Higher priority streamers get preferential treatment for download slots
/// and are the last to be paused during resource constraints.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Hash, utoipa::ToSchema,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    /// VIP streamers, never miss. First to get download slots, last to be paused.
    High,
    /// Standard streamers. Fair scheduling.
    #[default]
    Normal,
    /// Background/archive streamers. Paused first during resource constraints.
    Low,
}

impl Priority {
    /// Convert to database string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::High => "HIGH",
            Self::Normal => "NORMAL",
            Self::Low => "LOW",
        }
    }

    /// Parse from database string representation.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "HIGH" => Some(Self::High),
            "NORMAL" => Some(Self::Normal),
            "LOW" => Some(Self::Low),
            _ => None,
        }
    }

    /// Get numeric value for sorting (higher = more important).
    pub fn numeric_value(&self) -> i32 {
        match self {
            Self::High => 3,
            Self::Normal => 2,
            Self::Low => 1,
        }
    }

    /// Check if this priority is higher than another.
    pub fn is_higher_than(&self, other: &Self) -> bool {
        self.numeric_value() > other.numeric_value()
    }

    /// Check if this priority should bypass concurrency limits.
    pub fn bypasses_limits(&self) -> bool {
        matches!(self, Self::High)
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.numeric_value().cmp(&other.numeric_value())
    }
}

impl std::fmt::Display for Priority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for Priority {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| format!("Invalid priority: {}", s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::High > Priority::Low);
    }

    #[test]
    fn test_priority_default() {
        assert_eq!(Priority::default(), Priority::Normal);
    }

    #[test]
    fn test_priority_from_str() {
        assert_eq!(Priority::parse("HIGH"), Some(Priority::High));
        assert_eq!(Priority::parse("high"), Some(Priority::High));
        assert_eq!(Priority::parse("NORMAL"), Some(Priority::Normal));
        assert_eq!(Priority::parse("LOW"), Some(Priority::Low));
        assert_eq!(Priority::parse("invalid"), None);
    }

    #[test]
    fn test_priority_bypasses_limits() {
        assert!(Priority::High.bypasses_limits());
        assert!(!Priority::Normal.bypasses_limits());
        assert!(!Priority::Low.bypasses_limits());
    }

    #[test]
    fn test_priority_serialization() {
        let priority = Priority::High;
        let json = serde_json::to_string(&priority).unwrap();
        assert_eq!(json, "\"HIGH\"");

        let parsed: Priority = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, Priority::High);
    }
}
