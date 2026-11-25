//! Shared helpers for processor unit tests.

use jiff::Timestamp;

#[cfg(unix)]
pub(super) fn test_exit_code(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(code << 8)
}

#[cfg(windows)]
pub(super) fn test_exit_code(code: i32) -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    std::process::ExitStatus::from_raw(code as u32)
}

pub(super) fn test_exit_status(succeeds: bool) -> std::process::ExitStatus {
    test_exit_code(if succeeds { 0 } else { 1 })
}

/// Construct a fixed UTC instant from calendar components.
pub(super) fn utc_datetime(
    year: i16,
    month: i8,
    day: i8,
    hour: i8,
    minute: i8,
    second: i8,
) -> Timestamp {
    jiff::civil::DateTime::new(year, month, day, hour, minute, second, 0)
        .and_then(|wall| jiff::tz::TimeZone::UTC.to_timestamp(wall))
        .unwrap()
}
