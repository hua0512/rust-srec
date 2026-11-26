//! Filter types.

use jiff::civil::{DateTime, Time, Weekday};
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{SignedDuration, Timestamp, ToSpan};
use serde::{Deserialize, Serialize};

use super::{FilterEvaluator, timezone::FilterTimezone};

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
    pub fn matches(&self, title: &str, category: &str, now: Timestamp) -> bool {
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
    pub fn next_match_time(&self, now: Timestamp) -> Option<Timestamp> {
        match self {
            Self::TimeBased(f) => f.next_match_time(now),
            Self::Cron(f) => f.next_match_time(now),
            _ => None,
        }
    }

    /// Calculate the next time this filter will STOP matching.
    ///
    /// Returns `None` if it cannot be determined (e.g. content-based filters).
    pub fn next_unmatch_time(&self, now: Timestamp) -> Option<Timestamp> {
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
    pub fn matches(&self, now: Timestamp) -> bool {
        self.windows(now, 0)
            .is_some_and(|mut windows| windows.any(|(start, end)| start <= now && now < end))
    }

    pub fn next_match_time(&self, now: Timestamp) -> Option<Timestamp> {
        self.windows(now, 14)?
            .map(|(start, _)| start)
            .filter(|start| *start > now)
            .min()
    }

    pub fn next_unmatch_time(&self, now: Timestamp) -> Option<Timestamp> {
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
        now: Timestamp,
        future_days: i64,
    ) -> Option<impl Iterator<Item = (Timestamp, Timestamp)> + '_> {
        let timezone = FilterTimezone::parse(self.timezone.as_deref()).ok()?.0;
        let date = timezone.to_datetime(now).date();
        let start = parse_time(&self.start_time)?;
        let end = parse_time(&self.end_time)?;
        if start == end {
            return None;
        }
        Some((-1..=future_days).filter_map(move |offset| {
            let anchor = date.checked_add(offset.days()).ok()?;
            if !self
                .days_of_week
                .iter()
                .any(|day| day.eq_ignore_ascii_case(weekday_to_str(anchor.weekday())))
            {
                return None;
            }
            let end_date = if start > end {
                anchor.tomorrow().ok()?
            } else {
                anchor
            };
            let begins = resolve_local_boundary(&timezone, anchor.to_datetime(start), true)?;
            let ends = resolve_local_boundary(&timezone, end_date.to_datetime(end), false)?;
            (begins < ends).then_some((begins, ends))
        }))
    }
}

/// Fold boundaries choose the earliest start and latest end. A gap advances up to
/// three hours to its first representable minute; larger discontinuities (including
/// skipped dates) omit the window. Every query uses this same bounded policy.
fn resolve_local_boundary(timezone: &TimeZone, naive: DateTime, start: bool) -> Option<Timestamp> {
    for minute in 0..=180 {
        let mut wall = naive.checked_add(SignedDuration::from_mins(minute)).ok()?;
        if minute > 0 {
            wall = wall.with().second(0).subsec_nanosecond(0).build().ok()?;
        }
        let result = timezone.to_ambiguous_timestamp(wall);
        match result.offset() {
            AmbiguousOffset::Gap { .. } => {}
            _ if start => return result.earlier().ok(),
            _ => return result.later().ok(),
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
    pub fn matches(&self, now: Timestamp) -> bool {
        use crate::domain::filter::FilterEvaluator;

        FilterEvaluator::evaluate_cron(self, now).unwrap_or(false)
    }

    /// Calculate the next time this filter will match.
    pub fn next_match_time(&self, now: Timestamp) -> Option<Timestamp> {
        let schedule = super::compiled::cron(&self.expression).ok()?;
        let timezone = FilterTimezone::parse(self.timezone.as_deref()).ok()?.0;
        next_cron_match(&schedule, &timezone, now)
    }

    /// Calculate the next time this filter will stop matching.
    ///
    /// Cron matching is evaluated at minute granularity (see `FilterEvaluator::evaluate_cron`).
    /// When it matches, it is considered active for the current minute.
    ///
    /// This function returns the start of the first minute after the current contiguous
    /// matching run.
    pub fn next_unmatch_time(&self, now: Timestamp) -> Option<Timestamp> {
        let schedule = super::compiled::cron(&self.expression).ok()?;
        let timezone = FilterTimezone::parse(self.timezone.as_deref()).ok()?.0;
        next_cron_unmatch(&schedule, &timezone, now)
    }
}

fn minute_start(time: Timestamp, wall: DateTime) -> Option<Timestamp> {
    time.checked_sub(SignedDuration::new(
        i64::from(wall.second()),
        wall.subsec_nanosecond(),
    ))
    .ok()
}

fn next_cron_match(
    schedule: &jiff_cron::Schedule,
    timezone: &TimeZone,
    now: Timestamp,
) -> Option<Timestamp> {
    let next = next_cron_occurrence(schedule, timezone, now)?;
    let minute = minute_start(next, timezone.to_datetime(next))?;
    // A scheduled second activates its whole minute, matching evaluate_cron.
    Some(if minute > now { minute } else { next })
}

fn next_cron_unmatch(
    schedule: &jiff_cron::Schedule,
    timezone: &TimeZone,
    now: Timestamp,
) -> Option<Timestamp> {
    let wall = timezone.to_datetime(now);
    if !FilterEvaluator::time_matches_schedule(schedule, wall) {
        return None;
    }
    // Advance real instants rather than reconstructing local wall times. Both copies
    // of a repeated hour are checked, and spring gaps cannot fabricate a minute.
    let mut minute = minute_start(now, wall)?;
    const MAX_MINUTES: usize = 60 * 24 * 8;
    for _ in 0..MAX_MINUTES {
        minute = minute.checked_add(SignedDuration::from_mins(1)).ok()?;
        if !FilterEvaluator::time_matches_schedule(schedule, timezone.to_datetime(minute)) {
            return Some(minute);
        }
    }
    // Schedule appears to match continuously for a long period; treat as unbounded.
    None
}

/// Search each constant-offset period in instant order. The civil clock can
/// jump or repeat at a transition, so a candidate cannot cross that boundary.
fn next_cron_occurrence(
    schedule: &jiff_cron::Schedule,
    timezone: &TimeZone,
    now: Timestamp,
) -> Option<Timestamp> {
    use jiff_cron::TimeUnitSpec;

    let last_year = i16::try_from(schedule.years().iter().next_back()?).ok()?;
    let next_at_offset = |cursor: Timestamp, offset: jiff::tz::Offset| {
        schedule
            .after(cursor.to_zoned(offset.to_time_zone()))
            .next()
            .map(|time| time.timestamp())
    };
    let mut offset = timezone.to_offset(now);
    let mut cursor = now;
    for transition in timezone.following(now) {
        let at = transition.timestamp();
        let next = next_at_offset(cursor, offset);
        if next.is_some_and(|candidate| candidate < at) {
            return next;
        }
        offset = transition.offset();
        cursor = at.checked_sub(SignedDuration::from_secs(1)).ok()?;
        if offset.to_datetime(cursor).year() > last_year {
            return None;
        }
    }
    next_at_offset(cursor, offset)
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

fn parse_time(s: &str) -> Option<Time> {
    Time::strptime("%H:%M:%S", s)
        .or_else(|_| Time::strptime("%H:%M", s))
        .ok()
}

fn weekday_to_str(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Monday => "Monday",
        Weekday::Tuesday => "Tuesday",
        Weekday::Wednesday => "Wednesday",
        Weekday::Thursday => "Thursday",
        Weekday::Friday => "Friday",
        Weekday::Saturday => "Saturday",
        Weekday::Sunday => "Sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let filter = CategoryFilter::new(vec![]);
        assert!(filter.matches("Anything"));
    }
}
