//! Filter evaluation logic for cron and regex filters.

use crate::database::models::filter::{
    CronFilterConfig, FilterValidationError, RegexFilterConfig, FilterType, FilterDbModel,
    TimeBasedFilterConfig, KeywordFilterConfig, CategoryFilterConfig,
};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::RegexBuilder;
use std::str::FromStr;
use tracing::warn;

/// Error type for filter evaluation failures.
#[derive(Debug, thiserror::Error)]
pub enum FilterEvalError {
    #[error("Invalid cron expression: {0}")]
    InvalidCronExpression(String),

    #[error("Invalid timezone: {0}")]
    InvalidTimezone(String),

    #[error("Cron schedule error: {0}")]
    CronScheduleError(String),

    #[error("Invalid regex pattern: {0}")]
    InvalidRegexPattern(String),
}

impl From<FilterValidationError> for FilterEvalError {
    fn from(err: FilterValidationError) -> Self {
        match err {
            FilterValidationError::InvalidCronExpression(msg) => {
                FilterEvalError::InvalidCronExpression(msg)
            }
            FilterValidationError::InvalidTimezone(msg) => FilterEvalError::InvalidTimezone(msg),
            _ => FilterEvalError::CronScheduleError(err.to_string()),
        }
    }
}

/// Context for evaluating filters against stream data.
///
/// Contains all the information needed to evaluate different filter types:
/// - `current_time`: Used for cron and time-based filters
/// - `stream_title`: Used for regex and keyword filters
/// - `stream_category`: Used for category filters
#[derive(Debug, Clone)]
pub struct EvalContext {
    /// Current time in UTC for time-based evaluations.
    pub current_time: DateTime<Utc>,
    /// Stream title for regex and keyword filter matching.
    pub stream_title: Option<String>,
    /// Stream category for category filter matching.
    pub stream_category: Option<String>,
}

impl EvalContext {
    /// Create a new evaluation context with the current time.
    pub fn new() -> Self {
        Self {
            current_time: Utc::now(),
            stream_title: None,
            stream_category: None,
        }
    }

    /// Create a new evaluation context with a specific time.
    pub fn with_time(time: DateTime<Utc>) -> Self {
        Self {
            current_time: time,
            stream_title: None,
            stream_category: None,
        }
    }

    /// Set the stream title.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.stream_title = Some(title.into());
        self
    }

    /// Set the stream category.
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.stream_category = Some(category.into());
        self
    }
}

impl Default for EvalContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Evaluator for filter conditions.
pub struct FilterEvaluator;

impl FilterEvaluator {
    /// Evaluates a cron filter against the given time.
    ///
    /// Returns `true` if the current time matches the cron schedule.
    ///
    /// # Arguments
    /// * `config` - The cron filter configuration containing the expression and optional timezone
    /// * `now` - The current time in UTC
    ///
    /// # Returns
    /// * `Ok(true)` if the time matches the cron schedule
    /// * `Ok(false)` if the time does not match
    /// * `Err(FilterEvalError)` if the cron expression or timezone is invalid
    ///
    /// # Example
    /// ```ignore
    /// use rust_srec::database::models::filter::CronFilterConfig;
    /// use rust_srec::domain::filter::FilterEvaluator;
    /// use chrono::Utc;
    ///
    /// let config = CronFilterConfig {
    ///     expression: "0 0 22 * * FRI,SAT".to_string(),
    ///     timezone: Some("Asia/Shanghai".to_string()),
    /// };
    /// let result = FilterEvaluator::evaluate_cron(&config, Utc::now());
    /// ```
    pub fn evaluate_cron(config: &CronFilterConfig, now: DateTime<Utc>) -> Result<bool, FilterEvalError> {
        // Parse the cron expression
        let schedule = cron::Schedule::from_str(&config.expression)
            .map_err(|e| FilterEvalError::InvalidCronExpression(e.to_string()))?;

        // Determine the timezone to use
        let tz: Tz = match &config.timezone {
            Some(tz_str) => tz_str
                .parse()
                .map_err(|_| FilterEvalError::InvalidTimezone(format!("'{}' is not a valid IANA timezone", tz_str)))?,
            None => chrono_tz::UTC,
        };

        // Convert the current time to the target timezone
        let now_in_tz = now.with_timezone(&tz);

        // Check if the current time matches the cron schedule
        // We do this by checking if there's a scheduled time within a small window around now
        Self::time_matches_schedule(&schedule, now_in_tz)
    }

    /// Checks if the given time matches the cron schedule.
    ///
    /// The cron crate's Schedule provides an iterator of upcoming times.
    /// To check if "now" matches, we look at the previous scheduled time
    /// and see if it's within the same minute as now.
    fn time_matches_schedule<T: TimeZone>(
        schedule: &cron::Schedule,
        now: DateTime<T>,
    ) -> Result<bool, FilterEvalError>
    where
        T::Offset: std::fmt::Display,
    {
        // Get the upcoming scheduled times starting from the beginning of the current minute
        // We truncate to the start of the minute since cron operates at minute granularity
        let now_truncated = now
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))
            .ok_or_else(|| FilterEvalError::CronScheduleError("Failed to truncate time".to_string()))?;

        // Check if there's a scheduled time at exactly this minute
        // by looking at upcoming times from one minute before
        let one_minute_ago = now_truncated.clone() - chrono::Duration::minutes(1);
        
        for scheduled_time in schedule.after(&one_minute_ago).take(2) {
            // Check if the scheduled time is at the same minute as now
            if scheduled_time.year() == now_truncated.year()
                && scheduled_time.month() == now_truncated.month()
                && scheduled_time.day() == now_truncated.day()
                && scheduled_time.hour() == now_truncated.hour()
                && scheduled_time.minute() == now_truncated.minute()
            {
                return Ok(true);
            }
            
            // If we've passed the current minute, no need to continue
            if scheduled_time > now_truncated {
                break;
            }
        }

        Ok(false)
    }

    /// Evaluates a regex filter against a stream title.
    ///
    /// Returns `true` if the pattern matches the title (or does NOT match when exclude is set).
    ///
    /// # Arguments
    /// * `config` - The regex filter configuration containing the pattern and flags
    /// * `title` - The stream title to match against
    ///
    /// # Returns
    /// * `Ok(true)` if the filter condition is satisfied:
    ///   - Pattern matches AND exclude is false, OR
    ///   - Pattern does NOT match AND exclude is true
    /// * `Ok(false)` if the filter condition is not satisfied
    /// * `Err(FilterEvalError)` if the regex pattern is invalid
    ///
    /// # Example
    /// ```ignore
    /// use rust_srec::database::models::filter::RegexFilterConfig;
    /// use rust_srec::domain::filter::FilterEvaluator;
    ///
    /// let config = RegexFilterConfig {
    ///     pattern: "live.*gaming".to_string(),
    ///     case_insensitive: true,
    ///     exclude: false,
    /// };
    /// let result = FilterEvaluator::evaluate_regex(&config, "LIVE streaming gaming");
    /// assert!(result.unwrap()); // Matches because case_insensitive is true
    /// ```
    pub fn evaluate_regex(config: &RegexFilterConfig, title: &str) -> Result<bool, FilterEvalError> {
        // Build regex with case-insensitive flag when configured
        let regex = RegexBuilder::new(&config.pattern)
            .case_insensitive(config.case_insensitive)
            .build()
            .map_err(|e| FilterEvalError::InvalidRegexPattern(e.to_string()))?;

        // Check if the pattern matches the title
        let matches = regex.is_match(title);

        // Invert match result when exclude flag is set
        // - exclude=false: return true if pattern matches
        // - exclude=true: return true if pattern does NOT match
        Ok(matches ^ config.exclude)
    }

    /// Evaluates all filters for a streamer, combining results with AND logic.
    ///
    /// Returns `true` if ALL filters pass (AND logic), or if the filter list is empty.
    /// Failed filter evaluations are logged and treated as non-matching (fail-closed).
    ///
    /// # Arguments
    /// * `filters` - Slice of filter database models to evaluate
    /// * `context` - Evaluation context containing current time, stream title, and category
    ///
    /// # Returns
    /// * `true` if all filters pass or if the filter list is empty
    /// * `false` if any filter fails to match or encounters an evaluation error
    ///
    /// # Example
    /// ```ignore
    /// use rust_srec::domain::filter::{FilterEvaluator, EvalContext};
    /// use rust_srec::database::models::FilterDbModel;
    ///
    /// let filters: Vec<FilterDbModel> = vec![]; // Load from database
    /// let context = EvalContext::new()
    ///     .title("Live gaming stream")
    ///     .category("Gaming");
    /// let should_record = FilterEvaluator::evaluate_all(&filters, &context);
    /// ```
    pub fn evaluate_all(filters: &[FilterDbModel], context: &EvalContext) -> bool {
        // Empty filter list defaults to allowing recording (Requirement 5.3)
        if filters.is_empty() {
            return true;
        }

        // Evaluate all filters with AND logic (Requirement 5.1)
        for filter in filters {
            match Self::evaluate_single(filter, context) {
                Ok(true) => continue,
                Ok(false) => return false,
                Err(e) => {
                    // Log error and treat as non-matching (Requirement 5.2)
                    warn!(
                        filter_id = %filter.id,
                        filter_type = %filter.filter_type,
                        error = %e,
                        "Filter evaluation failed, treating as non-matching"
                    );
                    return false;
                }
            }
        }

        true
    }

    /// Evaluates a single filter against the context.
    fn evaluate_single(filter: &FilterDbModel, context: &EvalContext) -> Result<bool, FilterEvalError> {
        let filter_type = FilterType::parse(&filter.filter_type)
            .ok_or_else(|| FilterEvalError::CronScheduleError(
                format!("Unknown filter type: {}", filter.filter_type)
            ))?;

        match filter_type {
            FilterType::Cron => {
                let config: CronFilterConfig = serde_json::from_str(&filter.config)
                    .map_err(|e| FilterEvalError::CronScheduleError(
                        format!("Failed to parse cron config: {}", e)
                    ))?;
                Self::evaluate_cron(&config, context.current_time)
            }
            FilterType::Regex => {
                let config: RegexFilterConfig = serde_json::from_str(&filter.config)
                    .map_err(|e| FilterEvalError::InvalidRegexPattern(
                        format!("Failed to parse regex config: {}", e)
                    ))?;
                // If no title provided, regex filter passes (similar to existing behavior)
                match &context.stream_title {
                    Some(title) => Self::evaluate_regex(&config, title),
                    None => Ok(true),
                }
            }
            FilterType::TimeBased => {
                let config: TimeBasedFilterConfig = serde_json::from_str(&filter.config)
                    .map_err(|e| FilterEvalError::CronScheduleError(
                        format!("Failed to parse time-based config: {}", e)
                    ))?;
                Ok(Self::evaluate_time_based(&config, context.current_time))
            }
            FilterType::Keyword => {
                let config: KeywordFilterConfig = serde_json::from_str(&filter.config)
                    .map_err(|e| FilterEvalError::CronScheduleError(
                        format!("Failed to parse keyword config: {}", e)
                    ))?;
                // If no title provided, keyword filter passes
                match &context.stream_title {
                    Some(title) => Ok(Self::evaluate_keyword(&config, title)),
                    None => Ok(true),
                }
            }
            FilterType::Category => {
                let config: CategoryFilterConfig = serde_json::from_str(&filter.config)
                    .map_err(|e| FilterEvalError::CronScheduleError(
                        format!("Failed to parse category config: {}", e)
                    ))?;
                // If no category provided, category filter passes
                match &context.stream_category {
                    Some(category) => Ok(Self::evaluate_category(&config, category)),
                    None => Ok(true),
                }
            }
        }
    }

    /// Evaluates a time-based filter.
    fn evaluate_time_based(config: &TimeBasedFilterConfig, now: DateTime<Utc>) -> bool {
        use chrono::{Datelike as _, Local, NaiveTime};

        let local = now.with_timezone(&Local);
        let weekday = local.weekday();
        let current_time = local.time();

        // Check if current day is in allowed days
        let day_name = Self::weekday_to_string(weekday);
        if !config.days_of_week.iter().any(|d| d.eq_ignore_ascii_case(&day_name)) {
            // Also check if we're in an overnight range from the previous day
            let prev_day = Self::prev_weekday(weekday);
            let prev_day_name = Self::weekday_to_string(prev_day);
            if !config.days_of_week.iter().any(|d| d.eq_ignore_ascii_case(&prev_day_name)) {
                return false;
            }
            // We're checking from previous day's overnight range
            return Self::is_in_overnight_range_next_day(config, current_time);
        }

        // Parse times
        let start = match NaiveTime::parse_from_str(&config.start_time, "%H:%M").ok() {
            Some(t) => t,
            None => return false,
        };
        let end = match NaiveTime::parse_from_str(&config.end_time, "%H:%M").ok() {
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

    fn is_in_overnight_range_next_day(config: &TimeBasedFilterConfig, current_time: chrono::NaiveTime) -> bool {
        use chrono::NaiveTime;

        let start = match NaiveTime::parse_from_str(&config.start_time, "%H:%M").ok() {
            Some(t) => t,
            None => return false,
        };
        let end = match NaiveTime::parse_from_str(&config.end_time, "%H:%M").ok() {
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

    fn weekday_to_string(weekday: chrono::Weekday) -> String {
        match weekday {
            chrono::Weekday::Mon => "Monday",
            chrono::Weekday::Tue => "Tuesday",
            chrono::Weekday::Wed => "Wednesday",
            chrono::Weekday::Thu => "Thursday",
            chrono::Weekday::Fri => "Friday",
            chrono::Weekday::Sat => "Saturday",
            chrono::Weekday::Sun => "Sunday",
        }
        .to_string()
    }

    fn prev_weekday(weekday: chrono::Weekday) -> chrono::Weekday {
        match weekday {
            chrono::Weekday::Mon => chrono::Weekday::Sun,
            chrono::Weekday::Tue => chrono::Weekday::Mon,
            chrono::Weekday::Wed => chrono::Weekday::Tue,
            chrono::Weekday::Thu => chrono::Weekday::Wed,
            chrono::Weekday::Fri => chrono::Weekday::Thu,
            chrono::Weekday::Sat => chrono::Weekday::Fri,
            chrono::Weekday::Sun => chrono::Weekday::Sat,
        }
    }

    /// Evaluates a keyword filter.
    fn evaluate_keyword(config: &KeywordFilterConfig, title: &str) -> bool {
        let title_lower = title.to_lowercase();

        // Check excludes first
        for keyword in &config.exclude {
            if title_lower.contains(&keyword.to_lowercase()) {
                return false;
            }
        }

        // If no includes specified, pass
        if config.include.is_empty() {
            return true;
        }

        // Check includes (any match)
        for keyword in &config.include {
            if title_lower.contains(&keyword.to_lowercase()) {
                return true;
            }
        }

        false
    }

    /// Evaluates a category filter.
    fn evaluate_category(config: &CategoryFilterConfig, category: &str) -> bool {
        if config.categories.is_empty() {
            return true;
        }

        config.categories.iter().any(|c| c.eq_ignore_ascii_case(category))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_evaluate_cron_every_minute() {
        // "* * * * * *" matches every second (cron crate uses 6 fields)
        // "0 * * * * *" matches every minute at second 0
        let config = CronFilterConfig {
            expression: "0 * * * * *".to_string(),
            timezone: None,
        };
        
        // Any time should match since it runs every minute
        let now = Utc::now();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn test_evaluate_cron_specific_time_matches() {
        // 10 PM (22:00) every day
        let config = CronFilterConfig {
            expression: "0 0 22 * * *".to_string(),
            timezone: None,
        };
        
        // Create a time at exactly 22:00 UTC
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_ok());
        assert!(result.unwrap(), "22:00 should match the cron schedule");
    }

    #[test]
    fn test_evaluate_cron_specific_time_no_match() {
        // 10 PM (22:00) every day
        let config = CronFilterConfig {
            expression: "0 0 22 * * *".to_string(),
            timezone: None,
        };
        
        // Create a time at 15:00 UTC (should not match)
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 15, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "15:00 should not match the 22:00 cron schedule");
    }

    #[test]
    fn test_evaluate_cron_with_timezone() {
        // 10 PM in Asia/Shanghai timezone
        let config = CronFilterConfig {
            expression: "0 0 22 * * *".to_string(),
            timezone: Some("Asia/Shanghai".to_string()),
        };
        
        // Asia/Shanghai is UTC+8, so 22:00 Shanghai = 14:00 UTC
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 14, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_ok());
        assert!(result.unwrap(), "14:00 UTC should match 22:00 Asia/Shanghai");
    }

    #[test]
    fn test_evaluate_cron_with_timezone_no_match() {
        // 10 PM in Asia/Shanghai timezone
        let config = CronFilterConfig {
            expression: "0 0 22 * * *".to_string(),
            timezone: Some("Asia/Shanghai".to_string()),
        };
        
        // 22:00 UTC is 06:00 next day in Shanghai, should not match
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "22:00 UTC should not match 22:00 Asia/Shanghai");
    }

    #[test]
    fn test_evaluate_cron_day_of_week() {
        // Friday and Saturday at 22:00
        let config = CronFilterConfig {
            expression: "0 0 22 * * FRI,SAT".to_string(),
            timezone: None,
        };
        
        // December 6, 2024 is a Friday
        let friday = Utc.with_ymd_and_hms(2024, 12, 6, 22, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, friday);
        assert!(result.is_ok());
        assert!(result.unwrap(), "Friday 22:00 should match");
        
        // December 7, 2024 is a Saturday
        let saturday = Utc.with_ymd_and_hms(2024, 12, 7, 22, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, saturday);
        assert!(result.is_ok());
        assert!(result.unwrap(), "Saturday 22:00 should match");
        
        // December 4, 2024 is a Wednesday
        let wednesday = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 0).unwrap();
        let result = FilterEvaluator::evaluate_cron(&config, wednesday);
        assert!(result.is_ok());
        assert!(!result.unwrap(), "Wednesday 22:00 should not match");
    }

    #[test]
    fn test_evaluate_cron_invalid_expression() {
        let config = CronFilterConfig {
            expression: "invalid cron".to_string(),
            timezone: None,
        };
        
        let now = Utc::now();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FilterEvalError::InvalidCronExpression(_)));
    }

    #[test]
    fn test_evaluate_cron_invalid_timezone() {
        let config = CronFilterConfig {
            expression: "0 0 22 * * *".to_string(),
            timezone: Some("Invalid/Timezone".to_string()),
        };
        
        let now = Utc::now();
        let result = FilterEvaluator::evaluate_cron(&config, now);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), FilterEvalError::InvalidTimezone(_)));
    }

    #[test]
    fn test_evaluate_cron_every_15_minutes() {
        // Every 15 minutes
        let config = CronFilterConfig {
            expression: "0 */15 * * * *".to_string(),
            timezone: None,
        };
        
        // 14:00 should match (0 minutes)
        let at_00 = Utc.with_ymd_and_hms(2024, 12, 4, 14, 0, 0).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&config, at_00).unwrap());
        
        // 14:15 should match
        let at_15 = Utc.with_ymd_and_hms(2024, 12, 4, 14, 15, 0).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&config, at_15).unwrap());
        
        // 14:30 should match
        let at_30 = Utc.with_ymd_and_hms(2024, 12, 4, 14, 30, 0).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&config, at_30).unwrap());
        
        // 14:45 should match
        let at_45 = Utc.with_ymd_and_hms(2024, 12, 4, 14, 45, 0).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&config, at_45).unwrap());
        
        // 14:10 should not match
        let at_10 = Utc.with_ymd_and_hms(2024, 12, 4, 14, 10, 0).unwrap();
        assert!(!FilterEvaluator::evaluate_cron(&config, at_10).unwrap());
    }

    // Regex evaluation tests

    mod regex_tests {
        use super::*;
        use crate::database::models::filter::RegexFilterConfig;

        #[test]
        fn test_evaluate_regex_simple_match() {
            let config = RegexFilterConfig {
                pattern: "live".to_string(),
                case_insensitive: false,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "Going live now!").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Going LIVE now!").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Rerun of yesterday").unwrap());
        }

        #[test]
        fn test_evaluate_regex_case_insensitive() {
            let config = RegexFilterConfig {
                pattern: "live".to_string(),
                case_insensitive: true,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "Going live now!").unwrap());
            assert!(FilterEvaluator::evaluate_regex(&config, "Going LIVE now!").unwrap());
            assert!(FilterEvaluator::evaluate_regex(&config, "Going LiVe now!").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Rerun of yesterday").unwrap());
        }

        #[test]
        fn test_evaluate_regex_exclude_mode() {
            let config = RegexFilterConfig {
                pattern: "rerun".to_string(),
                case_insensitive: true,
                exclude: true,
            };
            
            // With exclude=true, returns true when pattern does NOT match
            assert!(FilterEvaluator::evaluate_regex(&config, "Going live now!").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "This is a RERUN").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "rerun of yesterday").unwrap());
        }

        #[test]
        fn test_evaluate_regex_complex_pattern() {
            let config = RegexFilterConfig {
                pattern: r"live.*gaming".to_string(),
                case_insensitive: true,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "LIVE streaming gaming").unwrap());
            assert!(FilterEvaluator::evaluate_regex(&config, "live - gaming session").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "gaming live").unwrap()); // Wrong order
            assert!(!FilterEvaluator::evaluate_regex(&config, "just chatting").unwrap());
        }

        #[test]
        fn test_evaluate_regex_anchored_pattern() {
            let config = RegexFilterConfig {
                pattern: r"^live".to_string(),
                case_insensitive: false,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "live stream today").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Going live now").unwrap());
        }

        #[test]
        fn test_evaluate_regex_invalid_pattern() {
            let config = RegexFilterConfig {
                pattern: "[invalid".to_string(), // Unclosed bracket
                case_insensitive: false,
                exclude: false,
            };
            
            let result = FilterEvaluator::evaluate_regex(&config, "any title");
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), FilterEvalError::InvalidRegexPattern(_)));
        }

        #[test]
        fn test_evaluate_regex_empty_title() {
            let config = RegexFilterConfig {
                pattern: "live".to_string(),
                case_insensitive: false,
                exclude: false,
            };
            
            assert!(!FilterEvaluator::evaluate_regex(&config, "").unwrap());
        }

        #[test]
        fn test_evaluate_regex_empty_pattern() {
            let config = RegexFilterConfig {
                pattern: "".to_string(),
                case_insensitive: false,
                exclude: false,
            };
            
            // Empty pattern matches everything
            assert!(FilterEvaluator::evaluate_regex(&config, "any title").unwrap());
            assert!(FilterEvaluator::evaluate_regex(&config, "").unwrap());
        }

        #[test]
        fn test_evaluate_regex_special_characters() {
            let config = RegexFilterConfig {
                pattern: r"\[LIVE\]".to_string(),
                case_insensitive: false,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "[LIVE] Gaming session").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "LIVE Gaming session").unwrap());
        }

        #[test]
        fn test_evaluate_regex_word_boundary() {
            let config = RegexFilterConfig {
                pattern: r"\blive\b".to_string(),
                case_insensitive: true,
                exclude: false,
            };
            
            assert!(FilterEvaluator::evaluate_regex(&config, "Going live now").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Delivered today").unwrap()); // "live" is part of "Delivered"
        }

        #[test]
        fn test_evaluate_regex_exclude_with_case_insensitive() {
            let config = RegexFilterConfig {
                pattern: "rerun|replay".to_string(),
                case_insensitive: true,
                exclude: true,
            };
            
            // Should return true for titles that don't contain "rerun" or "replay"
            assert!(FilterEvaluator::evaluate_regex(&config, "Live gaming session").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "RERUN of yesterday").unwrap());
            assert!(!FilterEvaluator::evaluate_regex(&config, "Replay available").unwrap());
        }
    }

    mod evaluate_all_tests {
        use super::*;
        use crate::database::models::filter::{FilterDbModel, FilterType};

        fn create_filter(filter_type: FilterType, config: &str) -> FilterDbModel {
            FilterDbModel {
                id: uuid::Uuid::new_v4().to_string(),
                streamer_id: "test-streamer".to_string(),
                filter_type: filter_type.as_str().to_string(),
                config: config.to_string(),
            }
        }

        #[test]
        fn test_evaluate_all_empty_filters() {
            // Empty filter list should return true (Requirement 5.3)
            let filters: Vec<FilterDbModel> = vec![];
            let context = EvalContext::new();
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_single_cron_filter_matches() {
            // Create a cron filter that matches every minute
            let filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "0 * * * * *"}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new();
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_single_cron_filter_no_match() {
            // Create a cron filter for a specific time that won't match now
            // 10 PM on December 31st only
            let filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "0 0 22 31 12 *"}"#,
            );
            let filters = vec![filter];
            // Use a time that doesn't match
            let context = EvalContext::with_time(Utc.with_ymd_and_hms(2024, 6, 15, 10, 0, 0).unwrap());
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_single_regex_filter_matches() {
            let filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": true, "exclude": false}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new().title("Going LIVE now!");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_single_regex_filter_no_match() {
            let filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": false, "exclude": false}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new().title("Rerun of yesterday");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_regex_filter_no_title_passes() {
            // If no title is provided, regex filter should pass
            let filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": true, "exclude": false}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new(); // No title
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_multiple_filters_all_pass() {
            // Both filters should pass (AND logic)
            let cron_filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "0 * * * * *"}"#, // Every minute
            );
            let regex_filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": true, "exclude": false}"#,
            );
            let filters = vec![cron_filter, regex_filter];
            let context = EvalContext::new().title("Going LIVE now!");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_multiple_filters_one_fails() {
            // Cron passes, regex fails -> overall should fail (AND logic)
            let cron_filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "0 * * * * *"}"#, // Every minute
            );
            let regex_filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": false, "exclude": false}"#,
            );
            let filters = vec![cron_filter, regex_filter];
            let context = EvalContext::new().title("Rerun of yesterday");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_invalid_config_fails() {
            // Invalid JSON config should cause filter to fail (treated as non-matching)
            let filter = create_filter(
                FilterType::Cron,
                r#"{"invalid": "config"}"#, // Missing required 'expression' field
            );
            let filters = vec![filter];
            let context = EvalContext::new();
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_invalid_cron_expression_fails() {
            // Invalid cron expression should cause filter to fail
            let filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "invalid cron"}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new();
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_invalid_regex_pattern_fails() {
            // Invalid regex pattern should cause filter to fail
            let filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "[invalid", "case_insensitive": false, "exclude": false}"#,
            );
            let filters = vec![filter];
            let context = EvalContext::new().title("any title");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_keyword_filter() {
            let filter = create_filter(
                FilterType::Keyword,
                r#"{"include": ["live"], "exclude": ["rerun"]}"#,
            );
            let filters = vec![filter];
            
            // Should pass - contains "live", no "rerun"
            let context = EvalContext::new().title("Going live now!");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
            
            // Should fail - contains "rerun"
            let context = EvalContext::new().title("Live rerun");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
            
            // Should fail - no "live"
            let context = EvalContext::new().title("Just chatting");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_category_filter() {
            let filter = create_filter(
                FilterType::Category,
                r#"{"categories": ["Just Chatting", "Art"]}"#,
            );
            let filters = vec![filter];
            
            // Should pass - matching category
            let context = EvalContext::new().category("Just Chatting");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
            
            // Should pass - case insensitive
            let context = EvalContext::new().category("just chatting");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
            
            // Should fail - non-matching category
            let context = EvalContext::new().category("Gaming");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_unknown_filter_type_fails() {
            // Unknown filter type should cause filter to fail
            let filter = FilterDbModel {
                id: uuid::Uuid::new_v4().to_string(),
                streamer_id: "test-streamer".to_string(),
                filter_type: "UNKNOWN_TYPE".to_string(),
                config: "{}".to_string(),
            };
            let filters = vec![filter];
            let context = EvalContext::new();
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_evaluate_all_mixed_filter_types() {
            // Test with multiple different filter types
            let cron_filter = create_filter(
                FilterType::Cron,
                r#"{"expression": "0 * * * * *"}"#,
            );
            let regex_filter = create_filter(
                FilterType::Regex,
                r#"{"pattern": "live", "case_insensitive": true, "exclude": false}"#,
            );
            let category_filter = create_filter(
                FilterType::Category,
                r#"{"categories": ["Gaming"]}"#,
            );
            let filters = vec![cron_filter, regex_filter, category_filter];
            
            // All should pass
            let context = EvalContext::new()
                .title("Going LIVE now!")
                .category("Gaming");
            assert!(FilterEvaluator::evaluate_all(&filters, &context));
            
            // Category fails
            let context = EvalContext::new()
                .title("Going LIVE now!")
                .category("Art");
            assert!(!FilterEvaluator::evaluate_all(&filters, &context));
        }

        #[test]
        fn test_eval_context_builder() {
            let time = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 0).unwrap();
            let context = EvalContext::with_time(time)
                .title("Test title")
                .category("Test category");
            
            assert_eq!(context.current_time, time);
            assert_eq!(context.stream_title, Some("Test title".to_string()));
            assert_eq!(context.stream_category, Some("Test category".to_string()));
        }

        #[test]
        fn test_eval_context_default() {
            let context = EvalContext::default();
            assert!(context.stream_title.is_none());
            assert!(context.stream_category.is_none());
        }
    }
}
