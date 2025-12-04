//! Filter database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::str::FromStr;

/// Validation errors for filter configurations.
#[derive(Debug, thiserror::Error)]
pub enum FilterValidationError {
    #[error("Invalid cron expression: {0}")]
    InvalidCronExpression(String),

    #[error("Invalid regex pattern: {0}")]
    InvalidRegexPattern(String),

    #[error("Invalid timezone: {0}")]
    InvalidTimezone(String),

    #[error("Invalid JSON config: {0}")]
    InvalidJson(String),

    #[error("Missing required field: {0}")]
    MissingField(String),
}

/// Trait for validating filter configurations.
pub trait FilterConfigValidator {
    /// Validates the configuration and returns Ok(()) or an error with details.
    fn validate(&self) -> Result<(), FilterValidationError>;
}

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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterType {
    /// Time-based filter with days of week and time ranges.
    TimeBased,
    /// Keyword filter with include/exclude lists.
    Keyword,
    /// Category filter for stream categories.
    Category,
    /// Cron-based filter using standard cron expressions.
    Cron,
    /// Regex-based filter for stream title pattern matching.
    Regex,
}

impl FilterType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TimeBased => "TIME_BASED",
            Self::Keyword => "KEYWORD",
            Self::Category => "CATEGORY",
            Self::Cron => "CRON",
            Self::Regex => "REGEX",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "TIME_BASED" => Some(Self::TimeBased),
            "KEYWORD" => Some(Self::Keyword),
            "CATEGORY" => Some(Self::Category),
            "CRON" => Some(Self::Cron),
            "REGEX" => Some(Self::Regex),
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

/// Cron-based filter configuration using standard cron expressions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronFilterConfig {
    /// Cron expression (6 fields, with seconds)
    /// Format: "second minute hour day-of-month month day-of-week"
    /// Example: "0 0 22 * * 5,6" (10 PM on Fridays and Saturdays)
    pub expression: String,

    /// Optional timezone (IANA format, e.g., "Asia/Shanghai")
    /// Defaults to UTC if not specified
    #[serde(default)]
    pub timezone: Option<String>,
}

impl FilterConfigValidator for CronFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        // Validate cron expression using the cron crate parser
        cron::Schedule::from_str(&self.expression).map_err(|e| {
            FilterValidationError::InvalidCronExpression(e.to_string())
        })?;

        // Validate timezone if provided
        if let Some(ref tz) = self.timezone {
            tz.parse::<chrono_tz::Tz>().map_err(|_| {
                FilterValidationError::InvalidTimezone(format!(
                    "'{}' is not a valid IANA timezone",
                    tz
                ))
            })?;
        }

        Ok(())
    }
}

/// Regex-based filter configuration for stream title matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegexFilterConfig {
    /// Regex pattern to match against stream title.
    pub pattern: String,

    /// Whether to perform case-insensitive matching.
    /// Defaults to false if not specified.
    #[serde(default)]
    pub case_insensitive: bool,

    /// If true, filter matches when pattern does NOT match the title.
    /// Defaults to false if not specified.
    #[serde(default)]
    pub exclude: bool,
}

impl FilterConfigValidator for RegexFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        // Validate regex pattern using the regex crate
        regex::Regex::new(&self.pattern).map_err(|e| {
            FilterValidationError::InvalidRegexPattern(e.to_string())
        })?;

        Ok(())
    }
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
    fn test_filter_type_cron_serialization() {
        assert_eq!(FilterType::Cron.as_str(), "CRON");
        assert_eq!(FilterType::parse("CRON"), Some(FilterType::Cron));
    }

    #[test]
    fn test_filter_type_regex_serialization() {
        assert_eq!(FilterType::Regex.as_str(), "REGEX");
        assert_eq!(FilterType::parse("REGEX"), Some(FilterType::Regex));
    }

    #[test]
    fn test_filter_type_unknown_returns_none() {
        assert_eq!(FilterType::parse("UNKNOWN"), None);
        assert_eq!(FilterType::parse("invalid"), None);
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

    #[test]
    fn test_cron_filter_config_serialization() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 0 22 * * 5,6".to_string(),
            timezone: Some("Asia/Shanghai".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: CronFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.expression, "0 0 22 * * 5,6");
        assert_eq!(parsed.timezone, Some("Asia/Shanghai".to_string()));
    }

    #[test]
    fn test_cron_filter_config_without_timezone() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 */5 * * * *".to_string(),
            timezone: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: CronFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.expression, "0 */5 * * * *");
        assert_eq!(parsed.timezone, None);
    }

    #[test]
    fn test_cron_filter_config_timezone_defaults_to_none() {
        // Test that timezone defaults to None when not present in JSON
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let json = r#"{"expression": "0 0 0 * * *"}"#;
        let parsed: CronFilterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.expression, "0 0 0 * * *");
        assert_eq!(parsed.timezone, None);
    }

    #[test]
    fn test_cron_filter_config_validation_valid_expression() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 0 22 * * 5,6".to_string(),
            timezone: None,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cron_filter_config_validation_valid_with_timezone() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 */5 * * * *".to_string(),
            timezone: Some("Asia/Shanghai".to_string()),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cron_filter_config_validation_invalid_expression() {
        let config = CronFilterConfig {
            expression: "invalid cron".to_string(),
            timezone: None,
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterValidationError::InvalidCronExpression(_)));
    }

    #[test]
    fn test_cron_filter_config_validation_invalid_timezone() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 0 0 * * *".to_string(),
            timezone: Some("Invalid/Timezone".to_string()),
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterValidationError::InvalidTimezone(_)));
    }

    #[test]
    fn test_cron_filter_config_validation_utc_timezone() {
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        let config = CronFilterConfig {
            expression: "0 0 0 * * *".to_string(),
            timezone: Some("UTC".to_string()),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_cron_filter_config_validation_various_valid_expressions() {
        // Test various valid cron expressions
        // cron crate uses 6-field format: sec min hour day-of-month month day-of-week
        // Day of week: 1=Monday, 7=Sunday (or SUN, MON, etc.)
        let valid_expressions = vec![
            "* * * * * *",           // Every second
            "0 * * * * *",           // Every minute
            "0 0 * * * *",           // Every hour
            "0 0 0 * * *",           // Every day at midnight
            "0 0 0 * * SUN",         // Every Sunday at midnight
            "0 0 0 1 * *",           // First day of every month
            "0 */15 * * * *",        // Every 15 minutes
            "0 0 9-17 * * MON-FRI",  // 9 AM to 5 PM, Monday to Friday
            "0 0 22 * * FRI,SAT",    // 10 PM on Fridays and Saturdays
        ];

        for expr in valid_expressions {
            let config = CronFilterConfig {
                expression: expr.to_string(),
                timezone: None,
            };
            assert!(
                config.validate().is_ok(),
                "Expected '{}' to be valid",
                expr
            );
        }
    }

    // RegexFilterConfig tests

    #[test]
    fn test_regex_filter_config_serialization() {
        let config = RegexFilterConfig {
            pattern: "(?i)live.*gaming".to_string(),
            case_insensitive: true,
            exclude: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RegexFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pattern, "(?i)live.*gaming");
        assert!(parsed.case_insensitive);
        assert!(!parsed.exclude);
    }

    #[test]
    fn test_regex_filter_config_defaults() {
        // Test that case_insensitive and exclude default to false
        let json = r#"{"pattern": "test"}"#;
        let parsed: RegexFilterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.pattern, "test");
        assert!(!parsed.case_insensitive);
        assert!(!parsed.exclude);
    }

    #[test]
    fn test_regex_filter_config_with_exclude() {
        let config = RegexFilterConfig {
            pattern: "rerun".to_string(),
            case_insensitive: false,
            exclude: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: RegexFilterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.pattern, "rerun");
        assert!(!parsed.case_insensitive);
        assert!(parsed.exclude);
    }

    #[test]
    fn test_regex_filter_config_validation_valid_pattern() {
        let config = RegexFilterConfig {
            pattern: "^live.*stream$".to_string(),
            case_insensitive: false,
            exclude: false,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_regex_filter_config_validation_valid_complex_pattern() {
        let config = RegexFilterConfig {
            pattern: r"(?i)\b(live|streaming)\b.*\d{4}".to_string(),
            case_insensitive: true,
            exclude: false,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_regex_filter_config_validation_invalid_pattern() {
        let config = RegexFilterConfig {
            pattern: "[invalid".to_string(), // Unclosed bracket
            case_insensitive: false,
            exclude: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterValidationError::InvalidRegexPattern(_)));
    }

    #[test]
    fn test_regex_filter_config_validation_invalid_unmatched_paren() {
        let config = RegexFilterConfig {
            pattern: "(unclosed".to_string(),
            case_insensitive: false,
            exclude: false,
        };
        let result = config.validate();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterValidationError::InvalidRegexPattern(_)));
    }

    #[test]
    fn test_regex_filter_config_validation_various_valid_patterns() {
        let valid_patterns = vec![
            "simple",
            ".*",
            "^start",
            "end$",
            r"\d+",
            r"\w+",
            "a|b|c",
            "(group)",
            "[abc]",
            "[^abc]",
            "a{2,5}",
            "a+?",
            r"(?i)case",
            r"(?:non-capturing)",
        ];

        for pattern in valid_patterns {
            let config = RegexFilterConfig {
                pattern: pattern.to_string(),
                case_insensitive: false,
                exclude: false,
            };
            assert!(
                config.validate().is_ok(),
                "Expected pattern '{}' to be valid",
                pattern
            );
        }
    }

    #[test]
    fn test_regex_filter_config_equality() {
        let config1 = RegexFilterConfig {
            pattern: "test".to_string(),
            case_insensitive: true,
            exclude: false,
        };
        let config2 = RegexFilterConfig {
            pattern: "test".to_string(),
            case_insensitive: true,
            exclude: false,
        };
        let config3 = RegexFilterConfig {
            pattern: "test".to_string(),
            case_insensitive: false,
            exclude: false,
        };
        assert_eq!(config1, config2);
        assert_ne!(config1, config3);
    }
}
