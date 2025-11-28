//! Filter database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Filter database model.
/// Conditions to decide whether a live stream should be recorded.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FilterDbModel {
    pub id: String,
    pub streamer_id: String,
    /// Filter type: TIME_BASED, KEYWORD, CATEGORY
    pub filter_type: String,
    /// JSON blob containing the filter's specific settings
    pub config: String,
}

impl FilterDbModel {
    pub fn new(
        streamer_id: impl Into<String>,
        filter_type: FilterType,
        config: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            streamer_id: streamer_id.into(),
            filter_type: filter_type.as_str().to_string(),
            config: config.into(),
        }
    }
}

/// Filter types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterType {
    /// Time-based filter with days of week and time ranges.
    TimeBased,
    /// Keyword filter with include/exclude lists.
    Keyword,
    /// Category filter for stream categories.
    Category,
}

impl FilterType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TimeBased => "TIME_BASED",
            Self::Keyword => "KEYWORD",
            Self::Category => "CATEGORY",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "TIME_BASED" => Some(Self::TimeBased),
            "KEYWORD" => Some(Self::Keyword),
            "CATEGORY" => Some(Self::Category),
            _ => None,
        }
    }
}

/// Time-based filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBasedFilterConfig {
    /// Days of the week (e.g., ["Monday", "Saturday"])
    pub days_of_week: Vec<String>,
    /// Start time in HH:MM format
    pub start_time: String,
    /// End time in HH:MM format (can be next day for overnight ranges)
    pub end_time: String,
}

/// Keyword filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordFilterConfig {
    /// Keywords that must be present in the title
    pub include: Vec<String>,
    /// Keywords that must NOT be present in the title
    pub exclude: Vec<String>,
}

/// Category filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryFilterConfig {
    /// Allowed categories
    pub categories: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_type_serialization() {
        assert_eq!(FilterType::TimeBased.as_str(), "TIME_BASED");
        assert_eq!(FilterType::parse("KEYWORD"), Some(FilterType::Keyword));
    }

    #[test]
    fn test_time_based_filter_config() {
        let config = TimeBasedFilterConfig {
            days_of_week: vec!["Saturday".to_string(), "Sunday".to_string()],
            start_time: "22:00".to_string(),
            end_time: "02:00".to_string(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: TimeBasedFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.days_of_week.len(), 2);
    }

    #[test]
    fn test_keyword_filter_config() {
        let config = KeywordFilterConfig {
            include: vec!["live".to_string()],
            exclude: vec!["rerun".to_string()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: KeywordFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.include, vec!["live"]);
    }
}
