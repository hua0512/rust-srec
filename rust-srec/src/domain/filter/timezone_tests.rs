use super::*;
use chrono::{DateTime, Timelike, Utc};

fn utc(value: &str) -> DateTime<Utc> {
    value.parse().unwrap()
}

#[test]
fn explicit_timezone_agrees_for_time_and_cron_match_and_boundaries() {
    let time = TimeBasedFilter::new(vec!["Monday".to_owned()], "16:00", "17:00")
        .with_timezone("Asia/Shanghai");
    let cron = CronFilter::with_timezone("0 * 16 * * Mon", "Asia/Shanghai");
    let before = utc("2024-01-01T07:59:00Z");
    let inside = utc("2024-01-01T08:05:00Z");
    let end = utc("2024-01-01T09:00:00Z");
    assert!(!time.matches(before));
    assert!(!cron.matches(before));
    assert!(time.matches(inside));
    assert!(cron.matches(inside));
    assert_eq!(
        time.next_match_time(before),
        Some(utc("2024-01-01T08:00:00Z"))
    );
    assert_eq!(cron.next_match_time(before), time.next_match_time(before));
    assert_eq!(time.next_unmatch_time(inside), Some(end));
    assert_eq!(cron.next_unmatch_time(inside), Some(end));
    assert!(!time.matches(end));
    assert!(!cron.matches(end));
}

#[test]
fn overnight_windows_use_the_previous_allowed_day_for_early_morning() {
    let time = TimeBasedFilter::new(
        vec!["Monday".to_owned(), "Tuesday".to_owned()],
        "22:00",
        "02:00",
    )
    .with_timezone("UTC");
    assert!(
        !time.matches(utc("2024-01-01T01:00:00Z")),
        "Monday morning is Sunday's tail"
    );
    let tuesday = utc("2024-01-02T01:00:00Z");
    assert!(time.matches(tuesday));
    assert_eq!(
        time.next_unmatch_time(tuesday),
        Some(utc("2024-01-02T02:00:00Z"))
    );
}

#[test]
fn time_window_fold_is_one_interval_with_earliest_start_and_latest_end() {
    let time = TimeBasedFilter::new(vec!["Sunday".to_owned()], "02:15", "02:45")
        .with_timezone("Europe/Madrid");
    assert_eq!(
        time.next_match_time(utc("2024-10-27T00:00:00Z")),
        Some(utc("2024-10-27T00:15:00Z"))
    );
    for moment in [
        "2024-10-27T00:30:00Z",
        "2024-10-27T00:50:00Z",
        "2024-10-27T01:30:00Z",
    ] {
        let now = utc(moment);
        assert!(time.matches(now));
        assert_eq!(
            time.next_unmatch_time(now),
            Some(utc("2024-10-27T01:45:00Z"))
        );
    }
    assert!(!time.matches(utc("2024-10-27T01:45:00Z")));
}

#[test]
fn overlapping_overnight_fold_windows_stop_at_the_end_of_the_continuous_run() {
    let time = TimeBasedFilter::new(
        vec!["Saturday".to_owned(), "Sunday".to_owned()],
        "02:30",
        "02:15",
    )
    .with_timezone("Europe/Madrid");
    let actual_end = utc("2024-10-28T01:15:00Z");
    for now in [
        utc("2024-10-27T00:10:00Z"),
        utc("2024-10-27T00:30:00Z"),
        utc("2024-10-27T01:15:00Z"),
    ] {
        assert!(time.matches(now));
        assert_eq!(time.next_unmatch_time(now), Some(actual_end));
    }
    assert!(time.matches(actual_end - chrono::Duration::nanoseconds(1)));
    assert!(!time.matches(actual_end));
}

#[test]
fn touching_gap_adjusted_windows_have_no_false_stop_at_the_shared_boundary() {
    let time = TimeBasedFilter::new(
        vec!["Saturday".to_owned(), "Sunday".to_owned()],
        "03:00",
        "02:30",
    )
    .with_timezone("Europe/Madrid");
    let now = utc("2024-03-31T00:30:00Z");
    let shared = utc("2024-03-31T01:00:00Z");
    let actual_end = utc("2024-04-01T00:30:00Z");
    assert!(time.matches(now));
    assert!(time.matches(shared));
    assert_eq!(time.next_unmatch_time(now), Some(actual_end));
    assert_eq!(time.next_unmatch_time(shared), Some(actual_end));
    assert!(time.matches(actual_end - chrono::Duration::nanoseconds(1)));
    assert!(!time.matches(actual_end));
}

#[test]
fn gaps_and_skipped_dates_have_bounded_consistent_windows() {
    let gap = TimeBasedFilter::new(vec!["Sunday".to_owned()], "02:15:45", "03:30")
        .with_timezone("Europe/Madrid");
    let start = utc("2024-03-31T01:00:00Z");
    assert_eq!(
        gap.next_match_time(utc("2024-03-31T00:59:00Z")),
        Some(start)
    );
    assert!(gap.matches(start));
    assert_eq!(
        gap.next_unmatch_time(start),
        Some(utc("2024-03-31T01:30:00Z"))
    );
    let swallowed = TimeBasedFilter::new(vec!["Sunday".to_owned()], "02:15", "02:45")
        .with_timezone("Europe/Madrid");
    assert!(!swallowed.matches(start));
    assert_eq!(swallowed.next_unmatch_time(start), None);
    let skipped = TimeBasedFilter::new(vec!["Friday".to_owned()], "09:00", "17:00")
        .with_timezone("Pacific/Apia");
    let before = utc("2011-12-29T20:00:00Z");
    assert!(!skipped.matches(before));
    assert_eq!(skipped.next_unmatch_time(before), None);
    assert_eq!(
        skipped.next_match_time(before),
        Some(utc("2012-01-05T19:00:00Z"))
    );
}

#[test]
fn cron_fold_next_start_is_strictly_future_and_both_copies_match() {
    let cron = CronFilter::with_timezone("0 30 2 * * Sun", "Europe/Madrid");
    let first = utc("2024-10-27T00:30:30Z");
    let second = utc("2024-10-27T01:30:30Z");
    assert!(cron.matches(first));
    assert!(cron.matches(second));
    assert_eq!(
        cron.next_unmatch_time(first),
        Some(utc("2024-10-27T00:31:00Z"))
    );
    assert_eq!(
        cron.next_unmatch_time(second),
        Some(utc("2024-10-27T01:31:00Z"))
    );
    assert_eq!(
        cron.next_match_time(utc("2024-10-27T00:45:00Z")),
        Some(utc("2024-10-27T01:30:00Z"))
    );
    let continuous = CronFilter::with_timezone("0 * 2 * * Sun", "Europe/Madrid");
    assert_eq!(
        continuous.next_unmatch_time(first),
        Some(utc("2024-10-27T02:00:00Z"))
    );
    let gap = CronFilter::with_timezone("0 30 2 * * Sun", "Europe/Madrid");
    assert_eq!(
        gap.next_match_time(utc("2024-03-31T00:59:00Z")),
        Some(utc("2024-04-07T00:30:00Z"))
    );
}

#[test]
fn cron_second_occurrence_opens_its_entire_matching_minute() {
    let cron = CronFilter::with_timezone("45 30 2 * * *", "UTC");
    let start = utc("2024-01-01T02:30:00Z");
    assert_eq!(
        cron.next_match_time(utc("2024-01-01T02:29:59Z")),
        Some(start)
    );
    assert!(cron.matches(start));
    assert_eq!(
        cron.next_unmatch_time(start),
        Some(utc("2024-01-01T02:31:00Z"))
    );
    let frequent = CronFilter::new("* * * * * *");
    assert!(frequent.matches(start));
    assert_eq!(frequent.next_unmatch_time(start), None);
}

#[test]
fn optional_timezone_roundtrips_validates_and_preserves_legacy_defaults() {
    use crate::database::models::filter::{
        FilterConfigValidator, FilterValidationError, TimeBasedFilterConfig,
    };
    let raw = r#"{"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","timezone":"Asia/Shanghai"}"#;
    let mut config: TimeBasedFilterConfig = serde_json::from_str(raw).unwrap();
    config.validate().unwrap();
    let stored = crate::database::models::FilterDbModel::new(
        "streamer",
        crate::database::models::filter::FilterType::TimeBased,
        raw,
    );
    let domain = Filter::try_from(&stored).unwrap();
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
    let invalid = TimeBasedFilter::new(vec!["Monday".to_owned()], "09:00", "17:00")
        .with_timezone("Mars/Olympus");
    assert!(!invalid.matches(utc("2024-01-01T10:00:00Z")));
    assert_eq!(invalid.next_match_time(utc("2024-01-01T10:00:00Z")), None);
    let now = utc("2024-01-01T12:34:56Z");
    let local = now.with_timezone(&chrono::Local);
    let days = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ]
    .map(str::to_owned)
    .to_vec();
    let legacy = TimeBasedFilter::new(
        days,
        format!("{:02}:00", local.hour()),
        format!("{:02}:00", (local.hour() + 1) % 24),
    );
    assert!(legacy.matches(now));
    assert!(legacy.timezone.is_none());
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("timezone")
            .is_none()
    );
    assert!(CronFilter::new("0 34 12 * * *").matches(now));
}
