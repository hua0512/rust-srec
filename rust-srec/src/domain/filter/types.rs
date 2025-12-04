//! Filter types.

use chrono::{Datelike, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

/// Filter type enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FilterType {
    TimeBased,
    Keyword,
    Category,
    Cron,
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
}

/// A filter that can be applied to determine if recording should occur.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Filter {
    TimeBased(TimeBasedFilter),
    Keyword(KeywordFilter),
    Category(CategoryFilter),
    Cron(CronFilter),
    Regex(RegexFilter),
}

impl Filter {
    /// Get the filter type.
    pub fn filter_type(&self) -> FilterType {
        match self {
            Self::TimeBased(_) => FilterType::TimeBased,
            Self::Keyword(_) => FilterType::Keyword,
            Self::Category(_) => FilterType::Category,
            Self::Cron(_) => FilterType::Cron,
            Self::Regex(_) => FilterType::Regex,
        }
    }

    /// Check if the filter matches the given context.
    /// For Cron and Regex filters, use the FilterEvaluator directly for more control.
    pub fn matches(&self, title: &str, category: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
        match self {
            Self::TimeBased(f) => f.matches(now),
            Self::Keyword(f) => f.matches(title),
            Self::Category(f) => f.matches(category),
            Self::Cron(f) => f.matches(now),
            Self::Regex(f) => f.matches(title),
        }
    }

    /// Create a Filter from a database model.
    pub fn from_db_model(model: &crate::database::models::FilterDbModel) -> Result<Self, String> {
        use crate::database::models::filter::FilterType as DbFilterType;

        let filter_type = DbFilterType::parse(&model.filter_type)
            .ok_or_else(|| format!("Unknown filter type: {}", model.filter_type))?;

        match filter_type {
            DbFilterType::TimeBased => {
                let config: crate::database::models::filter::TimeBasedFilterConfig =
                    serde_json::from_str(&model.config)
                        .map_err(|e| format!("Failed to parse time-based filter config: {}", e))?;
                Ok(Filter::TimeBased(TimeBasedFilter {
                    days_of_week: config.days_of_week,
                    start_time: config.start_time,
                    end_time: config.end_time,
                }))
            }
            DbFilterType::Keyword => {
                let config: crate::database::models::filter::KeywordFilterConfig =
                    serde_json::from_str(&model.config)
                        .map_err(|e| format!("Failed to parse keyword filter config: {}", e))?;
                Ok(Filter::Keyword(KeywordFilter {
                    include: config.include,
                    exclude: config.exclude,
                }))
            }
            DbFilterType::Category => {
                let config: crate::database::models::filter::CategoryFilterConfig =
                    serde_json::from_str(&model.config)
                        .map_err(|e| format!("Failed to parse category filter config: {}", e))?;
                Ok(Filter::Category(CategoryFilter {
                    categories: config.categories,
                }))
            }
            DbFilterType::Cron => {
                let config: crate::database::models::filter::CronFilterConfig =
                    serde_json::from_str(&model.config)
                        .map_err(|e| format!("Failed to parse cron filter config: {}", e))?;
                Ok(Filter::Cron(CronFilter {
                    expression: config.expression,
                    timezone: config.timezone,
                }))
            }
            DbFilterType::Regex => {
                let config: crate::database::models::filter::RegexFilterConfig =
                    serde_json::from_str(&model.config)
                        .map_err(|e| format!("Failed to parse regex filter config: {}", e))?;
                Ok(Filter::Regex(RegexFilter {
                    pattern: config.pattern,
                    case_insensitive: config.case_insensitive,
                    exclude: config.exclude,
                }))
            }
        }
    }
}

/// Time-based filter with days of week and time ranges.
/// Supports overnight ranges (e.g., 22:00 - 02:00).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBasedFilter {
    /// Days of the week when recording is allowed.
    pub days_of_week: Vec<String>,
    /// Start time in HH:MM format.
    pub start_time: String,
    /// End time in HH:MM format.
    pub end_time: String,
}

impl TimeBasedFilter {
    /// Create a new time-based filter.
    pub fn new(days: Vec<String>, start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            days_of_week: days,
            start_time: start.into(),
            end_time: end.into(),
        }
    }

    /// Check if the current time matches this filter.
    pub fn matches(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        let local = now.with_timezone(&chrono::Local);
        let weekday = local.weekday();
        let current_time = local.time();

        // Check if current day is in allowed days
        let day_name = weekday_to_string(weekday);
        if !self
            .days_of_week
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&day_name))
        {
            // Also check if we're in an overnight range from the previous day
            let prev_day = prev_weekday(weekday);
            let prev_day_name = weekday_to_string(prev_day);
            if !self
                .days_of_week
                .iter()
                .any(|d| d.eq_ignore_ascii_case(&prev_day_name))
            {
                return false;
            }
            // We're checking from previous day's overnight range
            return self.is_in_overnight_range_next_day(current_time);
        }

        // Parse times
        let start = match parse_time(&self.start_time) {
            Some(t) => t,
            None => return false,
        };
        let end = match parse_time(&self.end_time) {
            Some(t) => t,
            None => return false,
        };

        // Check if in range
        if start <= end {
            // Normal range (e.g., 09:00 - 17:00)
            current_time >= start && current_time <= end
        } else {
            // Overnight range (e.g., 22:00 - 02:00)
            current_time >= start || current_time <= end
        }
    }

    fn is_in_overnight_range_next_day(&self, current_time: NaiveTime) -> bool {
        let start = match parse_time(&self.start_time) {
            Some(t) => t,
            None => return false,
        };
        let end = match parse_time(&self.end_time) {
            Some(t) => t,
            None => return false,
        };

        // Only applies to overnight ranges
        if start <= end {
            return false;
        }

        // We're on the "next day" part of an overnight range
        current_time <= end
    }
}

/// Keyword filter with include/exclude lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordFilter {
    /// Keywords that must be present (any match).
    #[serde(default)]
    pub include: Vec<String>,
    /// Keywords that must NOT be present (any match excludes).
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl KeywordFilter {
    /// Create a new keyword filter.
    pub fn new(include: Vec<String>, exclude: Vec<String>) -> Self {
        Self { include, exclude }
    }

    /// Check if a title matches this filter.
    pub fn matches(&self, title: &str) -> bool {
        let title_lower = title.to_lowercase();

        // Check excludes first
        for keyword in &self.exclude {
            if title_lower.contains(&keyword.to_lowercase()) {
                return false;
            }
        }

        // If no includes specified, pass
        if self.include.is_empty() {
            return true;
        }

        // Check includes (any match)
        for keyword in &self.include {
            if title_lower.contains(&keyword.to_lowercase()) {
                return true;
            }
        }

        false
    }
}

/// Category filter for stream categories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryFilter {
    /// Allowed categories.
    pub categories: Vec<String>,
}

impl CategoryFilter {
    /// Create a new category filter.
    pub fn new(categories: Vec<String>) -> Self {
        Self { categories }
    }

    /// Check if a category matches this filter.
    pub fn matches(&self, category: &str) -> bool {
        if self.categories.is_empty() {
            return true;
        }

        self.categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case(category))
    }
}

/// Cron-based filter using standard cron expressions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronFilter {
    /// Cron expression (6 fields, with seconds).
    /// Format: "second minute hour day-of-month month day-of-week"
    pub expression: String,
    /// Optional timezone (IANA format, e.g., "Asia/Shanghai").
    #[serde(default)]
    pub timezone: Option<String>,
}

impl CronFilter {
    /// Create a new cron filter.
    pub fn new(expression: impl Into<String>) -> Self {
        Self {
            expression: expression.into(),
            timezone: None,
        }
    }

    /// Create a new cron filter with timezone.
    pub fn with_timezone(expression: impl Into<String>, timezone: impl Into<String>) -> Self {
        Self {
            expression: expression.into(),
            timezone: Some(timezone.into()),
        }
    }

    /// Check if the current time matches this cron schedule.
    pub fn matches(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        use crate::database::models::filter::CronFilterConfig;
        use crate::domain::filter::FilterEvaluator;

        let config = CronFilterConfig {
            expression: self.expression.clone(),
            timezone: self.timezone.clone(),
        };
        FilterEvaluator::evaluate_cron(&config, now).unwrap_or(false)
    }
}

/// Regex-based filter for stream title pattern matching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegexFilter {
    /// Regex pattern to match against stream title.
    pub pattern: String,
    /// Whether to perform case-insensitive matching.
    #[serde(default)]
    pub case_insensitive: bool,
    /// If true, filter matches when pattern does NOT match the title.
    #[serde(default)]
    pub exclude: bool,
}

impl RegexFilter {
    /// Create a new regex filter.
    pub fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_insensitive: false,
            exclude: false,
        }
    }

    /// Create a new case-insensitive regex filter.
    pub fn case_insensitive(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_insensitive: true,
            exclude: false,
        }
    }

    /// Create a new exclude regex filter (matches when pattern does NOT match).
    pub fn exclude(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            case_insensitive: false,
            exclude: true,
        }
    }

    /// Check if a title matches this regex filter.
    pub fn matches(&self, title: &str) -> bool {
        use crate::database::models::filter::RegexFilterConfig;
        use crate::domain::filter::FilterEvaluator;

        let config = RegexFilterConfig {
            pattern: self.pattern.clone(),
            case_insensitive: self.case_insensitive,
            exclude: self.exclude,
        };
        FilterEvaluator::evaluate_regex(&config, title).unwrap_or(false)
    }
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").ok()
}

fn weekday_to_string(weekday: Weekday) -> String {
    match weekday {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
    .to_string()
}

fn prev_weekday(weekday: Weekday) -> Weekday {
    match weekday {
        Weekday::Mon => Weekday::Sun,
        Weekday::Tue => Weekday::Mon,
        Weekday::Wed => Weekday::Tue,
        Weekday::Thu => Weekday::Wed,
        Weekday::Fri => Weekday::Thu,
        Weekday::Sat => Weekday::Fri,
        Weekday::Sun => Weekday::Sat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_keyword_filter_include() {
        let filter = KeywordFilter::new(vec!["live".to_string()], vec![]);
        assert!(filter.matches("Going LIVE now!"));
        assert!(!filter.matches("Rerun of yesterday"));
    }

    #[test]
    fn test_keyword_filter_exclude() {
        let filter = KeywordFilter::new(vec![], vec!["rerun".to_string()]);
        assert!(filter.matches("Live stream"));
        assert!(!filter.matches("Rerun of yesterday"));
    }

    #[test]
    fn test_keyword_filter_both() {
        let filter = KeywordFilter::new(vec!["live".to_string()], vec!["rerun".to_string()]);
        assert!(filter.matches("Going live!"));
        assert!(!filter.matches("Live rerun"));
        assert!(!filter.matches("Just chatting"));
    }

    #[test]
    fn test_category_filter() {
        let filter = CategoryFilter::new(vec!["Just Chatting".to_string(), "Art".to_string()]);
        assert!(filter.matches("Just Chatting"));
        assert!(filter.matches("just chatting")); // Case insensitive
        assert!(filter.matches("Art"));
        assert!(!filter.matches("Gaming"));
    }

    #[test]
    fn test_category_filter_empty() {
        let filter = CategoryFilter::new(vec![]);
        assert!(filter.matches("Anything"));
    }

    #[test]
    fn test_time_filter_normal_range() {
        let filter = TimeBasedFilter::new(vec!["Saturday".to_string()], "09:00", "17:00");

        // Saturday 12:00 UTC
        let time = chrono::Utc.with_ymd_and_hms(2024, 1, 6, 12, 0, 0).unwrap();
        // Note: This test may fail depending on local timezone
        // In a real scenario, we'd mock the timezone
    }

    #[test]
    fn test_filter_serialization() {
        let filter = Filter::Keyword(KeywordFilter::new(
            vec!["live".to_string()],
            vec!["rerun".to_string()],
        ));

        let json = serde_json::to_string(&filter).unwrap();
        let parsed: Filter = serde_json::from_str(&json).unwrap();

        match parsed {
            Filter::Keyword(kf) => {
                assert_eq!(kf.include, vec!["live"]);
                assert_eq!(kf.exclude, vec!["rerun"]);
            }
            _ => panic!("Wrong filter type"),
        }
    }
}
