//! Shared helpers for processor unit tests.

use chrono::{DateTime, TimeZone, Utc};

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
