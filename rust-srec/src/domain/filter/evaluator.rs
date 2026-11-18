//! Evaluation of typed domain filters.

use jiff::{Timestamp, civil::DateTime};

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
    pub fn evaluate_cron(filter: &CronFilter, now: Timestamp) -> Result<bool, FilterEvalError> {
        let schedule = super::compiled::cron(&filter.expression)?;
        let timezone = super::timezone::FilterTimezone::parse(filter.timezone.as_deref())
            .map_err(|error| FilterEvalError::InvalidTimezone(error.to_string()))?;
        Ok(Self::time_matches_schedule(
            &schedule,
            timezone.0.to_datetime(now),
        ))
    }

    /// Whether `now` falls inside a minute the schedule fires on.
    ///
    /// Also called by `CronFilter::next_unmatch_time`, which must agree
    /// with `CronFilter::matches` on the current minute before scanning
    /// for the end of the matching run.
    pub(super) fn time_matches_schedule(schedule: &jiff_cron::Schedule, now: DateTime) -> bool {
        use jiff_cron::TimeUnitSpec;

        schedule.minutes().includes(now.minute() as u32)
            && schedule.hours().includes(now.hour() as u32)
            && schedule.days_of_month().includes(now.day() as u32)
            && schedule.months().includes(now.month() as u32)
            && schedule
                .days_of_week()
                .includes(now.weekday().to_sunday_one_offset() as u32)
            && schedule.years().includes(now.year() as u32)
    }

    pub fn evaluate_regex(filter: &RegexFilter, title: &str) -> Result<bool, FilterEvalError> {
        let regex = super::compiled::regex(&filter.pattern, filter.case_insensitive)?;

        Ok(regex.is_match(title) ^ filter.exclude)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_matches_day_of_week_names() {
        let filter = CronFilter::new("0 0 20 * * Sat");

        // 2024-12-07 was a Saturday, 2024-12-08 a Sunday.
        let saturday = "2024-12-07T20:00:10Z".parse().unwrap();
        assert!(FilterEvaluator::evaluate_cron(&filter, saturday).unwrap());

        let sunday = "2024-12-08T20:00:10Z".parse().unwrap();
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
