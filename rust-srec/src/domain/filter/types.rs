//! Filter types.

use chrono::{Datelike, NaiveTime, Weekday};
use serde::{Deserialize, Serialize};

use super::timezone::FilterTimezone;

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
    /// IANA timezone or `local` for the system timezone. Omission uses UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl TimeBasedFilter {
    pub fn new(days: Vec<String>, start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            days_of_week: days,
            start_time: start.into(),
            end_time: end.into(),
            timezone: None,
        }
    }

    pub fn with_timezone(mut self, timezone: impl Into<String>) -> Self {
        self.timezone = Some(timezone.into());
        self
    }

    /// Match the same concrete intervals used for boundary wakeups. Overnight windows
    /// belong to their start day; a Tuesday morning tail therefore requires Monday.
    pub fn matches(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        self.windows(now, 0)
            .is_some_and(|mut windows| windows.any(|(start, end)| start <= now && now < end))
    }

    pub fn next_match_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        self.windows(now, 14)?
            .map(|(start, _)| start)
            .filter(|start| *start > now)
            .min()
    }

    pub fn next_unmatch_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        // A fold can make tomorrow's window begin before today's overnight
        // window ends. Merge the continuous run, including windows that have
        // not begun yet, rather than stopping at an endpoint that still matches.
        let mut windows: Vec<_> = self.windows(now, 14)?.collect();
        windows.sort_unstable_by_key(|(start, _)| *start);
        let mut current_end = windows
            .iter()
            .filter(|(start, end)| *start <= now && now < *end)
            .map(|(_, end)| *end)
            .max()?;
        for (start, end) in windows {
            if start > current_end {
                break;
            }
            current_end = current_end.max(end);
        }
        // Keep the scan bounded without claiming a stop at the horizon when
        // a further window already covers that instant.
        (!self.matches(current_end)).then_some(current_end)
    }

    fn windows(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        future_days: i64,
    ) -> Option<
        impl Iterator<Item = (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> + '_,
    > {
        let timezone = FilterTimezone::parse(self.timezone.as_deref()).ok()?;
        let date = timezone.date(now);
        let start = parse_time(&self.start_time)?;
        let end = parse_time(&self.end_time)?;
        if start == end {
            return None;
        }
        Some((-1..=future_days).filter_map(move |offset| {
            let anchor = date.checked_add_signed(chrono::Duration::days(offset))?;
            if !self
                .days_of_week
                .iter()
                .any(|day| day.eq_ignore_ascii_case(weekday_to_str(anchor.weekday())))
            {
                return None;
            }
            let end_date = if start > end {
                anchor.checked_add_days(chrono::Days::new(1))?
            } else {
                anchor
            };
            let begins = resolve_local_boundary(timezone, anchor.and_time(start), true)?;
            let ends = resolve_local_boundary(timezone, end_date.and_time(end), false)?;
            (begins < ends).then_some((begins, ends))
        }))
    }
}

/// Fold boundaries choose the earliest start and latest end. A gap advances up to
/// three hours to its first representable minute; larger discontinuities (including
/// skipped dates) omit the window. Every query uses this same bounded policy.
fn resolve_local_boundary(
    timezone: FilterTimezone,
    naive: chrono::NaiveDateTime,
    start: bool,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::Timelike;
    for minute in 0..=180 {
        let mut wall = naive.checked_add_signed(chrono::Duration::minutes(minute))?;
        if minute > 0 {
            wall = wall.with_second(0)?.with_nanosecond(0)?;
        }
        let result = timezone.resolve(wall);
        match result {
            chrono::LocalResult::Single(time) => return Some(time),
            chrono::LocalResult::Ambiguous(earliest, latest) => {
                return Some(if start { earliest } else { latest });
            }
            chrono::LocalResult::None => {}
        }
    }
    None
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
        use crate::domain::filter::FilterEvaluator;

        FilterEvaluator::evaluate_cron(self, now).unwrap_or(false)
    }

    /// Calculate the next time this filter will match.
    pub fn next_match_time(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<chrono::DateTime<chrono::Utc>> {
        let schedule = super::compiled::cron(&self.expression).ok()?;
        match FilterTimezone::parse(self.timezone.as_deref()).ok()? {
            FilterTimezone::Named(timezone) => next_cron_match(&schedule, timezone, now),
            FilterTimezone::Local => next_cron_match(&schedule, chrono::Local, now),
        }
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
        let schedule = super::compiled::cron(&self.expression).ok()?;
        match FilterTimezone::parse(self.timezone.as_deref()).ok()? {
            FilterTimezone::Named(timezone) => next_cron_unmatch(&schedule, timezone, now),
            FilterTimezone::Local => next_cron_unmatch(&schedule, chrono::Local, now),
        }
    }
}

fn next_cron_match<T: chrono::TimeZone + Copy>(
    schedule: &cron::Schedule,
    tz: T,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>>
where
    T::Offset: std::fmt::Display,
{
    use chrono::Timelike;
    let next = next_cron_occurrence(schedule, tz, now)?;
    let wall = next.with_timezone(&tz);
    let minute = next
        .checked_sub_signed(chrono::Duration::seconds(i64::from(wall.second())))?
        .checked_sub_signed(chrono::Duration::nanoseconds(i64::from(wall.nanosecond())))?;
    // A scheduled second activates its whole minute, matching evaluate_cron.
    Some(if minute > now { minute } else { next })
}

fn next_cron_unmatch<T: chrono::TimeZone + Copy>(
    schedule: &cron::Schedule,
    tz: T,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>>
where
    T::Offset: std::fmt::Display,
{
    use chrono::Timelike;
    let now_in_tz = now.with_timezone(&tz);
    use crate::domain::filter::FilterEvaluator;
    if !FilterEvaluator::time_matches_schedule(schedule, now_in_tz.clone()).ok()? {
        return None;
    }
    // Advance real instants rather than reconstructing local wall times. Both copies
    // of a repeated hour are checked, and spring gaps cannot fabricate a minute.
    let mut minute = now
        .checked_sub_signed(chrono::Duration::seconds(i64::from(now_in_tz.second())))?
        .checked_sub_signed(chrono::Duration::nanoseconds(i64::from(
            now_in_tz.nanosecond(),
        )))?;
    const MAX_MINUTES: usize = 60 * 24 * 8;
    for _ in 0..MAX_MINUTES {
        minute = minute.checked_add_signed(chrono::Duration::minutes(1))?;
        if !FilterEvaluator::time_matches_schedule(schedule, minute.with_timezone(&tz)).ok()? {
            return Some(minute);
        }
    }
    // Schedule appears to match continuously for a long period; treat as unbounded.
    None
}

fn next_cron_occurrence<T: chrono::TimeZone + Copy>(
    schedule: &cron::Schedule,
    timezone: T,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>>
where
    T::Offset: std::fmt::Display,
{
    use chrono::Timelike;
    let local = now.with_timezone(&timezone);
    let normal = schedule
        .after(&local)
        .take(2)
        .map(|time| time.with_timezone(&chrono::Utc))
        .filter(|time| *time > now)
        .min();
    let chrono::LocalResult::Ambiguous(_, later) =
        timezone.from_local_datetime(&local.naive_local())
    else {
        return normal;
    };
    if now >= later.with_timezone(&chrono::Utc) {
        return normal;
    }

    // In the first pass through a fold, a smaller wall time may still occur again.
    // Find the start of that repeated range, bounded even for historical date-line shifts.
    let mut beginning = local.naive_local().with_nanosecond(0)?;
    for _ in 0..(48 * 60) {
        let Some(previous) = beginning.checked_sub_signed(chrono::Duration::minutes(1)) else {
            break;
        };
        if !matches!(
            timezone.from_local_datetime(&previous),
            chrono::LocalResult::Ambiguous(..)
        ) {
            break;
        }
        beginning = previous;
    }
    for _ in 0..60 {
        let Some(previous) = beginning.checked_sub_signed(chrono::Duration::seconds(1)) else {
            break;
        };
        if !matches!(
            timezone.from_local_datetime(&previous),
            chrono::LocalResult::Ambiguous(..)
        ) {
            break;
        }
        beginning = previous;
    }
    let wall_before = beginning
        .and_utc()
        .checked_sub_signed(chrono::Duration::seconds(1))?;
    let repeated = schedule.after(&wall_before).next().and_then(|wall| {
        match timezone.from_local_datetime(&wall.naive_utc()) {
            chrono::LocalResult::Ambiguous(_, second) => {
                let second = second.with_timezone(&chrono::Utc);
                (second > now).then_some(second)
            }
            _ => None,
        }
    });
    normal.into_iter().chain(repeated).min()
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
        use crate::domain::filter::FilterEvaluator;

        FilterEvaluator::evaluate_regex(self, title).unwrap_or(false)
    }
}

fn parse_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(s, "%H:%M"))
        .ok()
}

fn weekday_to_str(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
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
        let filter =
            TimeBasedFilter::new(vec!["Monday".to_string()], "09:00", "17:00").with_timezone("UTC");
        let now = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 10, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 17, 0, 0).unwrap();
        assert!(filter.matches(now));
        assert!(!filter.matches(end));
        assert_eq!(filter.next_unmatch_time(now), Some(end));
    }

    #[test]
    fn test_time_filter_overnight_end_boundary_from_next_day() {
        let filter =
            TimeBasedFilter::new(vec!["Monday".to_string()], "22:00", "02:00").with_timezone("UTC");
        let now = chrono::Utc.with_ymd_and_hms(2024, 1, 2, 1, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2024, 1, 2, 2, 0, 0).unwrap();
        assert!(filter.matches(now));
        assert_eq!(filter.next_unmatch_time(now), Some(end));
        assert!(!filter.matches(end));
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
