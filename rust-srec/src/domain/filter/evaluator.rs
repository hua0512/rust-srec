//! Evaluation of typed domain filters.

use chrono::{DateTime, TimeZone, Timelike, Utc};

use super::{CronFilter, RegexFilter};

/// Error type for filter evaluation failures.
#[derive(Debug, Clone, thiserror::Error)]
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
        let schedule = super::compiled::cron(&filter.expression)?;
        let timezone = super::timezone::FilterTimezone::parse(filter.timezone.as_deref())
            .map_err(|error| FilterEvalError::InvalidTimezone(error.to_string()))?;
        match timezone {
            super::timezone::FilterTimezone::Named(timezone) => {
                Self::time_matches_schedule(&schedule, now.with_timezone(&timezone))
            }
            super::timezone::FilterTimezone::Local => {
                Self::time_matches_schedule(&schedule, now.with_timezone(&chrono::Local))
            }
        }
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
        // Membership is a wall-clock rule. Inspect it as a naive UTC calendar value to
        // avoid reconstructing an ambiguous zoned DateTime during a fall-back minute.
        use cron::TimeUnitSpec;
        let Some(second) = schedule.seconds().iter().next() else {
            return Ok(false);
        };
        let wall_minute = now
            .naive_local()
            .and_utc()
            .with_second(second)
            .and_then(|time| time.with_nanosecond(0))
            .ok_or_else(|| {
                FilterEvalError::CronScheduleError("Failed to truncate time".to_string())
            })?;
        Ok(schedule.includes(wall_minute))
    }

    pub fn evaluate_regex(filter: &RegexFilter, title: &str) -> Result<bool, FilterEvalError> {
        let regex = super::compiled::regex(&filter.pattern, filter.case_insensitive)?;

        Ok(regex.is_match(title) ^ filter.exclude)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn cron_matches_day_of_week_names() {
        let filter = CronFilter::new("0 0 20 * * Sat");

        // 2024-12-07 was a Saturday, 2024-12-08 a Sunday.
        let saturday = Utc.with_ymd_and_hms(2024, 12, 7, 20, 0, 10).unwrap();
        assert!(FilterEvaluator::evaluate_cron(&filter, saturday).unwrap());

        let sunday = Utc.with_ymd_and_hms(2024, 12, 8, 20, 0, 10).unwrap();
        assert!(!FilterEvaluator::evaluate_cron(&filter, sunday).unwrap());
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
