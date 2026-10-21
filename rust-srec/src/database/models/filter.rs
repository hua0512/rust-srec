//! Filter database model.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::borrow::Cow;
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

    #[error("Invalid time format for {field}: '{value}' (expected HH:MM or HH:MM:SS)")]
    InvalidTimeFormat { field: &'static str, value: String },

    #[error("Invalid day of week: '{0}' (expected Monday..Sunday)")]
    InvalidDayOfWeek(String),

    #[error("At least one day of week is required")]
    NoDaysOfWeek,

    #[error("Start and end time must differ: '{0}'")]
    EmptyTimeRange(String),
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

fn parse_filter_config<T: DeserializeOwned>(raw: &str, label: &'static str) -> crate::Result<T> {
    serde_json::from_str(raw).map_err(|error| {
        crate::Error::validation(format!("Failed to parse {label} filter config: {error}"))
    })
}

impl TryFrom<&FilterDbModel> for crate::domain::filter::Filter {
    type Error = crate::Error;

    fn try_from(model: &FilterDbModel) -> Result<Self, Self::Error> {
        use crate::domain::filter::{
            CategoryFilter, CronFilter, Filter, KeywordFilter, RegexFilter, TimeBasedFilter,
        };

        let filter_type = FilterType::parse(&model.filter_type).ok_or_else(|| {
            crate::Error::validation(format!("Unknown filter type: {}", model.filter_type))
        })?;

        match filter_type {
            FilterType::TimeBased => {
                let config: TimeBasedFilterConfig =
                    parse_filter_config(&model.config, "time-based")?;
                Ok(Filter::TimeBased(TimeBasedFilter {
                    days_of_week: config.days_of_week,
                    start_time: config.start_time,
                    end_time: config.end_time,
                    timezone: config.timezone,
                }))
            }
            FilterType::Keyword => {
                let config: KeywordFilterConfig = parse_filter_config(&model.config, "keyword")?;
                Ok(Filter::Keyword(KeywordFilter {
                    include: config.include,
                    exclude: config.exclude,
                }))
            }
            FilterType::Category => {
                let config: CategoryFilterConfig = parse_filter_config(&model.config, "category")?;
                Ok(Filter::Category(CategoryFilter {
                    categories: config.categories,
                }))
            }
            FilterType::Cron => {
                let config: CronFilterConfig = parse_filter_config(&model.config, "cron")?;
                Ok(Filter::Cron(CronFilter {
                    expression: config.expression,
                    timezone: config.timezone,
                }))
            }
            FilterType::Regex => {
                let config: RegexFilterConfig = parse_filter_config(&model.config, "regex")?;
                Ok(Filter::Regex(RegexFilter {
                    pattern: config.pattern,
                    case_insensitive: config.case_insensitive,
                    exclude: config.exclude,
                }))
            }
        }
    }
}

/// Filter types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

impl std::fmt::Display for FilterType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Time-based filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBasedFilterConfig {
    /// Days of the week (e.g., ["Monday", "Saturday"])
    pub days_of_week: Vec<String>,
    /// Start time in HH:MM:SS format (HH:MM accepted and normalized)
    pub start_time: String,
    /// End time in HH:MM:SS format (HH:MM accepted and normalized; can be next day for overnight ranges)
    pub end_time: String,
    /// IANA timezone or `local` for the system timezone; absent/null uses UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl TimeBasedFilterConfig {
    /// Normalize day names and time formats into the canonical backend form.
    pub fn normalize(&mut self) {
        self.days_of_week = self
            .days_of_week
            .iter()
            .filter_map(|d| normalize_day_of_week(d))
            .map(|d| d.to_string())
            .collect();

        if let Some(t) = normalize_time_hh_mm_ss(&self.start_time) {
            self.start_time = t;
        }
        if let Some(t) = normalize_time_hh_mm_ss(&self.end_time) {
            self.end_time = t;
        }
    }
}

impl FilterConfigValidator for TimeBasedFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        crate::domain::filter::FilterTimezone::parse(self.timezone.as_deref())
            .map_err(|error| FilterValidationError::InvalidTimezone(error.to_string()))?;
        for day in &self.days_of_week {
            if normalize_day_of_week(day).is_none() {
                return Err(FilterValidationError::InvalidDayOfWeek(day.clone()));
            }
        }

        // Neither shape can produce a window, so the filter would block every
        // recording without reporting why.
        if self.days_of_week.is_empty() {
            return Err(FilterValidationError::NoDaysOfWeek);
        }

        let Some(start) = normalize_time_hh_mm_ss(&self.start_time) else {
            return Err(FilterValidationError::InvalidTimeFormat {
                field: "start_time",
                value: self.start_time.clone(),
            });
        };

        let Some(end) = normalize_time_hh_mm_ss(&self.end_time) else {
            return Err(FilterValidationError::InvalidTimeFormat {
                field: "end_time",
                value: self.end_time.clone(),
            });
        };

        if start == end {
            return Err(FilterValidationError::EmptyTimeRange(start));
        }

        Ok(())
    }
}

/// Keyword filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordFilterConfig {
    /// Keywords that must be present in the title
    pub include: Vec<String>,
    /// Keywords that must NOT be present in the title
    pub exclude: Vec<String>,
}

impl KeywordFilterConfig {
    pub fn normalize(&mut self) {
        self.include = normalize_string_list(&self.include);
        self.exclude = normalize_string_list(&self.exclude);
    }
}

impl FilterConfigValidator for KeywordFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        Ok(())
    }
}

/// Category filter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryFilterConfig {
    /// Allowed categories
    pub categories: Vec<String>,
}

impl CategoryFilterConfig {
    pub fn normalize(&mut self) {
        self.categories = normalize_string_list(&self.categories);
    }
}

impl FilterConfigValidator for CategoryFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        Ok(())
    }
}

/// Cron-based filter configuration using standard cron expressions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronFilterConfig {
    /// Cron expression (6 fields, with seconds)
    /// Format: "second minute hour day-of-month month day-of-week"
    /// Example: "0 0 22 * * 5,6" (10 PM on Fridays and Saturdays)
    pub expression: String,

    /// Optional IANA timezone or `local` for the system timezone.
    /// Defaults to UTC if not specified
    #[serde(default)]
    pub timezone: Option<String>,
}

impl FilterConfigValidator for CronFilterConfig {
    fn validate(&self) -> Result<(), FilterValidationError> {
        // Validate cron expression using the cron crate parser
        cron::Schedule::from_str(&self.expression)
            .map_err(|e| FilterValidationError::InvalidCronExpression(e.to_string()))?;

        crate::domain::filter::FilterTimezone::parse(self.timezone.as_deref())
            .map_err(|error| FilterValidationError::InvalidTimezone(error.to_string()))?;

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
        // Keep validation behavior aligned with runtime evaluation.
        regex::RegexBuilder::new(&self.pattern)
            .case_insensitive(self.case_insensitive)
            .build()
            .map_err(|e| FilterValidationError::InvalidRegexPattern(e.to_string()))?;

        Ok(())
    }
}

fn normalize_string_list(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn normalize_day_of_week(raw: &str) -> Option<Cow<'static, str>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let lowered = trimmed.to_ascii_lowercase();
    let canonical = match lowered.as_str() {
        "mon" | "monday" => "Monday",
        "tue" | "tues" | "tuesday" => "Tuesday",
        "wed" | "wednesday" => "Wednesday",
        "thu" | "thur" | "thurs" | "thursday" => "Thursday",
        "fri" | "friday" => "Friday",
        "sat" | "saturday" => "Saturday",
        "sun" | "sunday" => "Sunday",
        _ => return None,
    };

    Some(Cow::Owned(canonical.to_string()))
}

fn normalize_time_hh_mm_ss(raw: &str) -> Option<String> {
    use chrono::NaiveTime;

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Accept both HH:MM and HH:MM:SS, but normalize storage to HH:MM:SS so the
    // evaluator can support second-level precision end-to-end.
    let parsed = NaiveTime::parse_from_str(trimmed, "%H:%M")
        .or_else(|_| NaiveTime::parse_from_str(trimmed, "%H:%M:%S"))
        .ok()?;

    Some(parsed.format("%H:%M:%S").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_based_filter_accepts_seconds_and_normalizes() {
        let mut config = TimeBasedFilterConfig {
            timezone: None,
            days_of_week: vec!["Mon".to_string()],
            start_time: "09:00:00".to_string(),
            end_time: "17:30:59".to_string(),
        };

        config.normalize();

        assert_eq!(config.days_of_week, vec!["Monday"]);
        assert_eq!(config.start_time, "09:00:00");
        assert_eq!(config.end_time, "17:30:59");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn optional_timezone_validates_and_roundtrips_to_domain_filter() {
        let raw = r#"{"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","timezone":"Asia/Shanghai"}"#;
        let mut config: TimeBasedFilterConfig = serde_json::from_str(raw).unwrap();
        config.validate().unwrap();
        let stored = FilterDbModel::new("streamer", FilterType::TimeBased, raw);
        let domain = crate::domain::filter::Filter::try_from(&stored).unwrap();
        let utc = |value: &str| {
            chrono::DateTime::parse_from_rfc3339(value)
                .unwrap()
                .with_timezone(&chrono::Utc)
        };
        assert!(domain.matches("", "", utc("2024-01-01T02:00:00Z")));
        assert!(!domain.matches("", "", utc("2024-01-01T09:00:00Z")));
        assert_eq!(
            serde_json::to_value(&config).unwrap()["timezone"],
            "Asia/Shanghai"
        );
        config.timezone = Some("Mars/Olympus".to_owned());
        assert!(matches!(
            config.validate(),
            Err(FilterValidationError::InvalidTimezone(_))
        ));
    }

    #[test]
    fn invalid_explicit_zones_remain_restrictive_filters_instead_of_being_skipped() {
        for kind in [FilterType::TimeBased, FilterType::Cron] {
            for timezone in ["", "not/a/timezone"] {
                let config = if kind == FilterType::TimeBased {
                    serde_json::json!({"days_of_week":["Monday"], "start_time":"00:00", "end_time":"23:59", "timezone":timezone})
                } else {
                    serde_json::json!({"expression":"0 * * * * *", "timezone":timezone})
                };
                let model = FilterDbModel::new("streamer", kind, config.to_string());
                let filter = crate::domain::filter::Filter::try_from(&model).expect(
                    "invalid timezone must not turn the entire restrictive filter into a skipped row",
                );
                let now = "2024-01-01T12:00:00Z"
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .unwrap();
                assert!(!filter.matches("title", "category", now));
                assert!(filter.next_match_time(now).is_none());
                assert!(filter.next_unmatch_time(now).is_none());
            }
        }
    }
}
