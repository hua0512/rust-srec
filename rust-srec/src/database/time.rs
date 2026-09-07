//! Timestamp helpers for the database layer.
//!
//! We store timestamps as `INTEGER` Unix epoch milliseconds (UTC) in SQLite.

use chrono::{DateTime, TimeZone, Utc};

/// SQLx adapter for date-time fields backed by INTEGER epoch milliseconds.
///
/// SQLx's native SQLite DateTime decoder interprets integers as seconds.
#[derive(sqlx::Type)]
#[sqlx(transparent)]
pub struct EpochMillis(i64);

#[derive(Debug, thiserror::Error)]
#[error("epoch millisecond timestamp {0} is outside chrono's supported range")]
pub struct InvalidEpochMillis(i64);

impl TryFrom<EpochMillis> for DateTime<Utc> {
    type Error = InvalidEpochMillis;

    fn try_from(value: EpochMillis) -> Result<Self, Self::Error> {
        DateTime::from_timestamp_millis(value.0).ok_or(InvalidEpochMillis(value.0))
    }
}

/// Current time as Unix epoch milliseconds (UTC).
#[inline]
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Convert a `DateTime<Utc>` to Unix epoch milliseconds.
#[inline]
pub fn datetime_to_ms(dt: DateTime<Utc>) -> i64 {
    dt.timestamp_millis()
}

/// Convert Unix epoch milliseconds to `DateTime<Utc>`.
///
/// Values outside chrono's supported range will clamp to the nearest representable timestamp.
#[inline]
pub fn ms_to_datetime(ms: i64) -> DateTime<Utc> {
    // Prefer timestamp_millis_opt to avoid panics.
    match Utc.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(dt) => dt,
        chrono::LocalResult::Ambiguous(dt, _) => dt,
        chrono::LocalResult::None => {
            // Clamp to nearest representable value.
            if ms.is_negative() {
                Utc.timestamp_millis_opt(i64::MIN)
                    .earliest()
                    .unwrap_or_else(Utc::now)
            } else {
                Utc.timestamp_millis_opt(i64::MAX)
                    .latest()
                    .unwrap_or_else(Utc::now)
            }
        }
    }
}
