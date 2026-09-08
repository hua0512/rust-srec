//! Shared helpers for processor unit tests.

use chrono::{DateTime, TimeZone, Utc};

#[cfg(unix)]
pub(super) fn test_exit_status(succeeds: bool) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(if succeeds { 0 } else { 1 << 8 })
}

#[cfg(windows)]
pub(super) fn test_exit_status(succeeds: bool) -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(if succeeds { 0 } else { 1 })
}

/// Construct a fixed UTC instant from calendar components.
pub(super) fn utc_datetime(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, second)
        .unwrap()
}
