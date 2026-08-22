//! Evaluation of typed domain filters.

use std::str::FromStr;

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::RegexBuilder;

use super::{CronFilter, RegexFilter};

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

/// Evaluates already-parsed domain filters.
pub struct FilterEvaluator;

impl FilterEvaluator {
    pub fn evaluate_cron(filter: &CronFilter, now: DateTime<Utc>) -> Result<bool, FilterEvalError> {
        let schedule = cron::Schedule::from_str(&filter.expression)
            .map_err(|error| FilterEvalError::InvalidCronExpression(error.to_string()))?;
        let timezone: Tz = match &filter.timezone {
            Some(timezone) => timezone.parse().map_err(|_| {
                FilterEvalError::InvalidTimezone(format!(
                    "'{timezone}' is not a valid IANA timezone"
                ))
            })?,
            None => chrono_tz::UTC,
        };

        Self::time_matches_schedule(&schedule, now.with_timezone(&timezone))
    }

    /// Whether `now` falls inside a minute the schedule fires on.
    ///
    /// Also called by `CronFilter::next_unmatch_time`, which must agree
    /// with `CronFilter::matches` on the current minute before scanning
    /// for the end of the matching run.
    pub(super) fn time_matches_schedule<T: TimeZone>(
        schedule: &cron::Schedule,
        now: DateTime<T>,
    ) -> Result<bool, FilterEvalError>
    where
        T::Offset: std::fmt::Display,
    {
        let now_truncated = now
            .with_second(0)
            .and_then(|time| time.with_nanosecond(0))
            .ok_or_else(|| {
                FilterEvalError::CronScheduleError("Failed to truncate time".to_string())
            })?;
        let one_minute_ago = now_truncated.clone() - chrono::Duration::minutes(1);

        for scheduled_time in schedule.after(&one_minute_ago).take(2) {
            if scheduled_time.year() == now_truncated.year()
                && scheduled_time.month() == now_truncated.month()
                && scheduled_time.day() == now_truncated.day()
                && scheduled_time.hour() == now_truncated.hour()
                && scheduled_time.minute() == now_truncated.minute()
            {
                return Ok(true);
            }

            if scheduled_time > now_truncated {
                break;
            }
        }

        Ok(false)
    }

    pub fn evaluate_regex(filter: &RegexFilter, title: &str) -> Result<bool, FilterEvalError> {
        let regex = RegexBuilder::new(&filter.pattern)
            .case_insensitive(filter.case_insensitive)
            .build()
            .map_err(|error| FilterEvalError::InvalidRegexPattern(error.to_string()))?;

        Ok(regex.is_match(title) ^ filter.exclude)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn cron_matches_at_minute_granularity() {
        let filter = CronFilter::new("0 0 22 * * *");
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 45).unwrap();

        assert!(FilterEvaluator::evaluate_cron(&filter, now).unwrap());
    }

    #[test]
    fn cron_only_matches_within_scheduled_minute() {
        let filter = CronFilter::new("0 30 12 * * *");

        let inside = Utc.with_ymd_and_hms(2024, 12, 4, 12, 30, 59).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&filter, inside).unwrap());

        let outside = Utc.with_ymd_and_hms(2024, 12, 4, 12, 31, 0).unwrap();
        assert!(!FilterEvaluator::evaluate_cron(&filter, outside).unwrap());
    }

    #[test]
    fn cron_evaluates_in_configured_timezone() {
        // 22:00 in Asia/Shanghai (UTC+8) is 14:00 UTC.
        let filter = CronFilter::with_timezone("0 0 22 * * *", "Asia/Shanghai");

        let matching_utc = Utc.with_ymd_and_hms(2024, 12, 4, 14, 0, 30).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&filter, matching_utc).unwrap());

        // 22:00 UTC is 06:00 the next day in Asia/Shanghai — no match.
        let non_matching_utc = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 30).unwrap();
        assert!(!FilterEvaluator::evaluate_cron(&filter, non_matching_utc).unwrap());
    }

    #[test]
    fn cron_matches_day_of_week() {
        let filter = CronFilter::new("0 0 20 * * Sat");

        // 2024-12-07 was a Saturday, 2024-12-08 a Sunday.
        let saturday = Utc.with_ymd_and_hms(2024, 12, 7, 20, 0, 10).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&filter, saturday).unwrap());

        let sunday = Utc.with_ymd_and_hms(2024, 12, 8, 20, 0, 10).unwrap();
        assert!(!FilterEvaluator::evaluate_cron(&filter, sunday).unwrap());
    }

    #[test]
    fn cron_rejects_invalid_timezone() {
        let filter = CronFilter::with_timezone("0 * * * * *", "Mars/Olympus");

        assert!(matches!(
            FilterEvaluator::evaluate_cron(&filter, Utc::now()),
            Err(FilterEvalError::InvalidTimezone(_))
        ));
    }

    #[test]
    fn regex_honors_case_and_exclusion() {
        let included = RegexFilter::case_insensitive("live.*gaming");
        let excluded = RegexFilter::exclude("rerun");

        assert!(FilterEvaluator::evaluate_regex(&included, "LIVE gaming").unwrap());
        assert!(!FilterEvaluator::evaluate_regex(&excluded, "rerun").unwrap());
    }

    #[test]
    fn regex_supports_anchors_and_escaped_metacharacters() {
        let filter = RegexFilter::new(r"^\[LIVE\]");

        assert!(FilterEvaluator::evaluate_regex(&filter, "[LIVE] speedrun night").unwrap());
        assert!(!FilterEvaluator::evaluate_regex(&filter, "Re: [LIVE] speedrun night").unwrap());
    }

    #[test]
    fn regex_supports_word_boundaries() {
        let filter = RegexFilter::new(r"\bspeedrun\b");

        assert!(FilterEvaluator::evaluate_regex(&filter, "casual speedrun today").unwrap());
        assert!(!FilterEvaluator::evaluate_regex(&filter, "speedrunning marathon").unwrap());
    }

    #[test]
    fn regex_empty_pattern_matches_everything() {
        assert!(FilterEvaluator::evaluate_regex(&RegexFilter::new(""), "any title").unwrap());
        assert!(!FilterEvaluator::evaluate_regex(&RegexFilter::exclude(""), "any title").unwrap());
    }

    #[test]
    fn regex_combines_case_insensitivity_with_exclusion() {
        let filter = RegexFilter {
            pattern: "rerun".to_string(),
            case_insensitive: true,
            exclude: true,
        };

        assert!(!FilterEvaluator::evaluate_regex(&filter, "ReRun of stream").unwrap());
        assert!(FilterEvaluator::evaluate_regex(&filter, "Fresh live show").unwrap());
    }
}
