//! Evaluation of typed domain filters.

use std::str::FromStr;

use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use regex::RegexBuilder;
use tracing::warn;

use super::{CronFilter, Filter, RegexFilter};

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

/// Stream data used to evaluate a set of filters.
#[derive(Debug, Clone)]
pub struct EvalContext {
    pub current_time: DateTime<Utc>,
    pub stream_title: Option<String>,
    pub stream_category: Option<String>,
}

impl EvalContext {
    pub fn new() -> Self {
        Self {
            current_time: Utc::now(),
            stream_title: None,
            stream_category: None,
        }
    }

    pub fn with_time(time: DateTime<Utc>) -> Self {
        Self {
            current_time: time,
            stream_title: None,
            stream_category: None,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.stream_title = Some(title.into());
        self
    }

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

    fn time_matches_schedule<T: TimeZone>(
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

    /// Evaluate all filters with AND semantics.
    ///
    /// Missing title or category data does not reject a stream. Invalid cron
    /// and regex filters fail closed and are logged.
    pub fn evaluate_all(filters: &[Filter], context: &EvalContext) -> bool {
        filters
            .iter()
            .all(|filter| match Self::evaluate_single(filter, context) {
                Ok(matches) => matches,
                Err(error) => {
                    warn!(
                        filter_type = filter.filter_type().as_str(),
                        error = %error,
                        "filter evaluation failed, treating as non-matching"
                    );
                    false
                }
            })
    }

    fn evaluate_single(filter: &Filter, context: &EvalContext) -> Result<bool, FilterEvalError> {
        match filter {
            Filter::TimeBased(filter) => Ok(filter.matches(context.current_time)),
            Filter::Keyword(filter) => Ok(context
                .stream_title
                .as_deref()
                .is_none_or(|title| filter.matches(title))),
            Filter::Category(filter) => Ok(context
                .stream_category
                .as_deref()
                .is_none_or(|category| filter.matches(category))),
            Filter::Cron(filter) => Self::evaluate_cron(filter, context.current_time),
            Filter::Regex(filter) => match context.stream_title.as_deref() {
                Some(title) => Self::evaluate_regex(filter, title),
                None => Ok(true),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::filter::{CategoryFilter, KeywordFilter};

    #[test]
    fn cron_matches_at_minute_granularity() {
        let filter = CronFilter::new("0 0 22 * * *");
        let now = Utc.with_ymd_and_hms(2024, 12, 4, 22, 0, 45).unwrap();

        assert!(FilterEvaluator::evaluate_cron(&filter, now).unwrap());
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
    fn evaluate_all_uses_and_semantics() {
        let filters = vec![
            Filter::Keyword(KeywordFilter::new(vec!["live".to_string()], vec![])),
            Filter::Category(CategoryFilter::new(vec!["gaming".to_string()])),
        ];

        assert!(FilterEvaluator::evaluate_all(
            &filters,
            &EvalContext::new().title("Going live").category("Gaming")
        ));
        assert!(!FilterEvaluator::evaluate_all(
            &filters,
            &EvalContext::new().title("Going live").category("Music")
        ));
    }

    #[test]
    fn missing_optional_metadata_does_not_reject() {
        let filters = vec![Filter::Regex(RegexFilter::new("live"))];

        assert!(FilterEvaluator::evaluate_all(&filters, &EvalContext::new()));
    }

    #[test]
    fn invalid_filter_fails_closed() {
        let filters = vec![Filter::Regex(RegexFilter::new("[invalid"))];

        assert!(!FilterEvaluator::evaluate_all(
            &filters,
            &EvalContext::new().title("live")
        ));
    }
}
