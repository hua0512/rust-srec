//! Filter types.

use chrono::{Datelike, NaiveTime, Weekday};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

fn parse_db_filter_config<T: DeserializeOwned>(raw: &str, label: &'static str) -> crate::Result<T> {
    serde_json::from_str(raw).map_err(|e| {
        crate::Error::validation(format!("Failed to parse {label} filter config: {e}"))
    })
}

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

    /// Calculate the next time this filter will match.
    /// Returns None if it cannot be determined (e.g. content-based filters) or won't match again.
    pub fn next_match_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::TimeBased(f) => f.next_match_time(now),
            Self::Cron(f) => f.next_match_time(now),
            _ => None,
        }
    }

    /// Calculate the next time this filter will STOP matching.
    ///
    /// Returns `None` if it cannot be determined (e.g. content-based filters).
    pub fn next_unmatch_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::TimeBased(f) => f.next_unmatch_time(now),
            Self::Cron(f) => f.next_unmatch_time(now),
            _ => None,
        }
    }

    /// Create a Filter from a database model.
    pub fn from_db_model(model: &crate::database::models::FilterDbModel) -> crate::Result<Self> {
        use crate::database::models::filter::FilterType as DbFilterType;

        let filter_type = DbFilterType::parse(&model.filter_type).ok_or_else(|| {
            crate::Error::validation(format!("Unknown filter type: {}", model.filter_type))
        })?;

        match filter_type {
            DbFilterType::TimeBased => {
                let config: crate::database::models::filter::TimeBasedFilterConfig =
                    parse_db_filter_config(&model.config, "time-based")?;
                Ok(Filter::TimeBased(TimeBasedFilter {
                    days_of_week: config.days_of_week,
                    start_time: config.start_time,
                    end_time: config.end_time,
                }))
            }
            DbFilterType::Keyword => {
                let config: crate::database::models::filter::KeywordFilterConfig =
                    parse_db_filter_config(&model.config, "keyword")?;
                Ok(Filter::Keyword(KeywordFilter {
                    include: config.include,
                    exclude: config.exclude,
                }))
            }
            DbFilterType::Category => {
                let config: crate::database::models::filter::CategoryFilterConfig =
                    parse_db_filter_config(&model.config, "category")?;
                Ok(Filter::Category(CategoryFilter {
                    categories: config.categories,
                }))
            }
            DbFilterType::Cron => {
                let config: crate::database::models::filter::CronFilterConfig =
                    parse_db_filter_config(&model.config, "cron")?;
                Ok(Filter::Cron(CronFilter {
                    expression: config.expression,
                    timezone: config.timezone,
                }))
            }
            DbFilterType::Regex => {
                let config: crate::database::models::filter::RegexFilterConfig =
                    parse_db_filter_config(&model.config, "regex")?;
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

        // Check if in range.
        //
        // The end boundary is exclusive. This matches user expectations for schedules like
        // "09:00 - 17:00" meaning "stop at 17:00".
        if start <= end {
            // Normal range (e.g., 09:00 - 17:00)
            current_time >= start && current_time < end
        } else {
            // Overnight range (e.g., 22:00 - 02:00)
            current_time >= start || current_time < end
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
        current_time < end
    }

    /// Calculate the next time this filter will match.
    pub fn next_match_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        use chrono::{Days, TimeZone};

        let local_now = now.with_timezone(&chrono::Local);
        let start_time = parse_time(&self.start_time)?;

        // Scan upcoming days (today + 7 days to cover a full week)
        for i in 0..8 {
            let target_date = match local_now.date_naive().checked_add_days(Days::new(i)) {
                Some(d) => d,
                None => continue,
            };
            let weekday = target_date.weekday();
            let day_name = weekday_to_string(weekday);

            if self
                .days_of_week
                .iter()
                .any(|d| d.eq_ignore_ascii_case(&day_name))
            {
                // Found a valid day. Construct the candidate start time.
                // We use the start_time of the filter on this valid day.
                let candidate_time =
                    match chrono::Local.from_local_datetime(&target_date.and_time(start_time)) {
                        chrono::LocalResult::Single(t) => t,
                        _ => continue,
                    };

                let candidate_utc = candidate_time.with_timezone(&chrono::Utc);

                // If the start time is in the future, that's our next match start
                if candidate_utc > now {
                    return Some(candidate_utc);
                }
            }
        }
        None
    }

    /// Calculate the next time this filter will stop matching.
    ///
    /// This is used to schedule a boundary wake to stop recording exactly at the end of a
    /// time-based window while a download is active.
    pub fn next_unmatch_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        use chrono::{Datelike as _, Days, TimeZone as _};

        if !self.matches(now) {
            return None;
        }

        let local_now = now.with_timezone(&chrono::Local);
        let weekday = local_now.weekday();

        let start = parse_time(&self.start_time)?;
        let end = parse_time(&self.end_time)?;

        let day_name = weekday_to_string(weekday);
        let today_allowed = self
            .days_of_week
            .iter()
            .any(|d| d.eq_ignore_ascii_case(&day_name));

        let end_date = if start <= end {
            // Normal window ends the same day.
            local_now.date_naive()
        } else {
            // Overnight window: end is on the day after the "anchor" day.
            //
            // This mirrors the matching logic which prioritizes today's allowed status.
            let anchor_date = if today_allowed {
                local_now.date_naive()
            } else {
                local_now.date_naive().checked_sub_days(Days::new(1))?
            };

            anchor_date.checked_add_days(Days::new(1))?
        };

        let naive = end_date.and_time(end);

        // Convert local naive datetime to a concrete instant.
        // Prefer the latest instant for ambiguous times (DST fall-back) to avoid early stops.
        let mut dt_local = match chrono::Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(t) => Some(t),
            chrono::LocalResult::Ambiguous(_, latest) => Some(latest),
            chrono::LocalResult::None => None,
        };

        if dt_local.is_none() {
            // Handle DST gaps (nonexistent local times) by searching forward for the first
            // representable instant.
            for mins in 1..=180 {
                let candidate = naive + chrono::Duration::minutes(mins);
                match chrono::Local.from_local_datetime(&candidate) {
                    chrono::LocalResult::Single(t) => {
                        dt_local = Some(t);
                        break;
                    }
                    chrono::LocalResult::Ambiguous(_, latest) => {
                        dt_local = Some(latest);
                        break;
                    }
                    chrono::LocalResult::None => continue,
                }
            }
        }

        dt_local.map(|t| t.with_timezone(&chrono::Utc))
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

    /// Calculate the next time this filter will match.
    pub fn next_match_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        use chrono_tz::Tz;
        use std::str::FromStr;

        let schedule = cron::Schedule::from_str(&self.expression).ok()?;

        let tz: Tz = match &self.timezone {
            Some(tz_str) => tz_str.parse().ok()?,
            None => chrono_tz::UTC,
        };

        let now_in_tz = now.with_timezone(&tz);

        // Find next occurrence after now
        schedule
            .after(&now_in_tz)
            .next()
            .map(|t| t.with_timezone(&chrono::Utc))
    }

    /// Calculate the next time this filter will stop matching.
    ///
    /// Cron matching is evaluated at minute granularity (see `FilterEvaluator::evaluate_cron`).
    /// When it matches, it is considered active for the current minute.
    ///
    /// This function returns the start of the first minute after the current contiguous
    /// matching run.
    pub fn next_unmatch_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        use chrono::Timelike as _;
        use chrono_tz::Tz;
        use std::str::FromStr;

        let schedule = cron::Schedule::from_str(&self.expression).ok()?;

        let tz: Tz = match &self.timezone {
            Some(tz_str) => tz_str.parse().ok()?,
            None => chrono_tz::UTC,
        };

        let now_in_tz = now.with_timezone(&tz);
        let current_minute = now_in_tz
            .with_second(0)
            .and_then(|t| t.with_nanosecond(0))?;

        // Determine if the schedule matches the current minute.
        //
        // Mirrors `FilterEvaluator::time_matches_schedule`.
        let one_minute_ago = current_minute - chrono::Duration::minutes(1);
        let mut current_minute_matches = false;
        for scheduled_time in schedule.after(&one_minute_ago).take(2) {
            if scheduled_time.year() == current_minute.year()
                && scheduled_time.month() == current_minute.month()
                && scheduled_time.day() == current_minute.day()
                && scheduled_time.hour() == current_minute.hour()
                && scheduled_time.minute() == current_minute.minute()
            {
                current_minute_matches = true;
                break;
            }

            if scheduled_time > current_minute {
                break;
            }
        }
        if !current_minute_matches {
            return None;
        }

        // Walk forward minute-by-minute at the *minute* level (not per occurrence).
        //
        // We do this by jumping to the first occurrence strictly after the end of the current
        // minute. This avoids iterating every-second schedules.
        const MAX_MINUTES: usize = 60 * 24 * 8; // 8 days
        let mut minute = current_minute;
        for _ in 0..MAX_MINUTES {
            let end_of_minute =
                minute + chrono::Duration::minutes(1) - chrono::Duration::nanoseconds(1);

            let Some(next_occurrence) = schedule.after(&end_of_minute).next() else {
                return Some((minute + chrono::Duration::minutes(1)).with_timezone(&chrono::Utc));
            };

            let next_minute = next_occurrence
                .with_second(0)
                .and_then(|t| t.with_nanosecond(0))?;

            let expected_next = minute + chrono::Duration::minutes(1);
            if next_minute != expected_next {
                return Some(expected_next.with_timezone(&chrono::Utc));
            }

            minute = next_minute;
        }

        // Schedule appears to match continuously for a long period; treat as unbounded.
        None
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
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
        .ok()
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
        // NOTE: time-based filters evaluate using `chrono::Local`.
        // Use fixed local datetimes that are likely unambiguous.
        let filter = TimeBasedFilter::new(vec!["Monday".to_string()], "09:00", "17:00");

        let date = chrono::NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(); // Monday

        let in_window_local = chrono::Local
            .from_local_datetime(&date.and_hms_opt(10, 0, 0).unwrap())
            .single();
        let Some(in_window_local) = in_window_local else {
            // If local time is ambiguous/nonexistent (rare), skip.
            return;
        };
        let now = in_window_local.with_timezone(&chrono::Utc);

        assert!(filter.matches(now));

        // End is exclusive: 17:00 should not match.
        let end_local = chrono::Local
            .from_local_datetime(&date.and_hms_opt(17, 0, 0).unwrap())
            .single();
        let Some(end_local) = end_local else {
            return;
        };
        assert!(!filter.matches(end_local.with_timezone(&chrono::Utc)));

        // next_unmatch_time should point to the end boundary.
        let end_at = filter.next_unmatch_time(now).expect("end boundary");
        assert_eq!(end_at, end_local.with_timezone(&chrono::Utc));
    }

    #[test]
    fn test_time_filter_overnight_end_boundary_from_next_day() {
        // Allow Monday overnight into Tuesday.
        let filter = TimeBasedFilter::new(vec!["Monday".to_string()], "22:00", "02:00");

        let tuesday = chrono::NaiveDate::from_ymd_opt(2024, 1, 2).unwrap();
        let now_local = chrono::Local
            .from_local_datetime(&tuesday.and_hms_opt(1, 0, 0).unwrap())
            .single();
        let Some(now_local) = now_local else {
            return;
        };
        let now = now_local.with_timezone(&chrono::Utc);

        assert!(filter.matches(now));

        let end_local = chrono::Local
            .from_local_datetime(&tuesday.and_hms_opt(2, 0, 0).unwrap())
            .single();
        let Some(end_local) = end_local else {
            return;
        };

        let end_at = filter.next_unmatch_time(now).expect("end boundary");
        assert_eq!(end_at, end_local.with_timezone(&chrono::Utc));
        assert!(!filter.matches(end_local.with_timezone(&chrono::Utc)));
    }

    #[test]
    fn test_cron_filter_end_boundary_is_next_minute() {
        // This cron matches at 10:05:00 (and is considered active for that minute).
        let filter = CronFilter::with_timezone("0 5 10 * * Mon", "UTC");

        let now = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 10, 5, 30).unwrap();
        assert!(filter.matches(now));

        let end = filter.next_unmatch_time(now).expect("end boundary");
        let expected = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 10, 6, 0).unwrap();
        assert_eq!(end, expected);
    }

    #[test]
    fn test_cron_filter_end_boundary_for_continuous_hour_window() {
        // Matches every minute during hour 10 on Mondays.
        let filter = CronFilter::with_timezone("0 * 10 * * Mon", "UTC");

        let now = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 10, 5, 30).unwrap();
        assert!(filter.matches(now));

        // End boundary is the first minute after the last matching minute (11:00).
        let end = filter.next_unmatch_time(now).expect("end boundary");
        let expected = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 11, 0, 0).unwrap();
        assert_eq!(end, expected);
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
