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
fn invalid_timezone_fails_closed_and_explicit_local_preserves_legacy_defaults() {
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
    )
    .with_timezone("local");
    assert!(legacy.matches(now));
    assert_eq!(legacy.timezone.as_deref(), Some("local"));
    assert_eq!(serde_json::to_value(&legacy).unwrap()["timezone"], "local");
    assert!(CronFilter::new("0 34 12 * * *").matches(now));
}

#[test]
fn omitted_defaults_are_utc_for_all_matching_and_wake_operations() {
    assert!(matches!(
        super::FilterTimezone::parse(None).unwrap(),
        super::FilterTimezone::Named(chrono_tz::UTC)
    ));
    let time = TimeBasedFilter::new(vec!["Monday".into()], "16:00", "17:00");
    let cron = CronFilter::new("0 * 16 * * Mon");
    for moment in [
        "2024-01-01T15:59:00Z",
        "2024-01-01T16:30:00Z",
        "2024-01-01T17:00:00Z",
    ] {
        let now = utc(moment);
        assert_eq!(
            time.matches(now),
            time.clone().with_timezone("UTC").matches(now)
        );
        assert_eq!(time.matches(now), cron.matches(now));
        assert_eq!(time.next_unmatch_time(now), cron.next_unmatch_time(now));
    }
    let before = utc("2024-01-01T15:59:00Z");
    assert_eq!(
        time.next_match_time(before),
        Some(utc("2024-01-01T16:00:00Z"))
    );
    assert_eq!(cron.next_match_time(before), time.next_match_time(before));
}

#[test]
fn explicit_local_uses_system_calendar_rules_for_match_and_wakes() {
    use chrono::{Local, TimeZone};
    assert!(matches!(
        super::FilterTimezone::parse(Some("local")).unwrap(),
        super::FilterTimezone::Local
    ));
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
    let time = TimeBasedFilter::new(days, "12:00", "13:00").with_timezone("local");
    let cron = CronFilter::with_timezone("0 * 12 * * *", "local");
    // Resolve each date in Local independently; a current fixed UTC offset must
    // never stand in for the system's winter/summer timezone rules.
    for month in [1, 7] {
        let local = |hour, minute| {
            Local
                .with_ymd_and_hms(2024, month, 15, hour, minute, 0)
                .single()
                .unwrap()
                .with_timezone(&Utc)
        };
        let before = local(11, 59);
        let start = local(12, 0);
        let inside = local(12, 30);
        let end = local(13, 0);
        assert!(!time.matches(before));
        assert!(!cron.matches(before));
        assert!(time.matches(inside));
        assert!(cron.matches(inside));
        assert_eq!(time.next_match_time(before), Some(start));
        assert_eq!(cron.next_match_time(before), Some(start));
        assert_eq!(time.next_unmatch_time(inside), Some(end));
        assert_eq!(cron.next_unmatch_time(inside), Some(end));
        assert!(!time.matches(end));
        assert!(!cron.matches(end));
    }
}

#[test]
fn invalid_explicit_zones_fail_closed_for_matching_and_wakes() {
    for timezone in ["", "not/a/timezone"] {
        for filter in [
            Filter::TimeBased(
                TimeBasedFilter::new(vec!["Monday".into()], "00:00", "23:59")
                    .with_timezone(timezone),
            ),
            Filter::Cron(CronFilter::with_timezone("0 * * * * *", timezone)),
        ] {
            let now = utc("2024-01-01T12:00:00Z");
            assert!(!filter.matches("title", "category", now));
            assert!(filter.next_match_time(now).is_none());
            assert!(filter.next_unmatch_time(now).is_none());
        }
    }
}
