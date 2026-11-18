//! Timestamp helpers for the database layer.
//!
//! We store timestamps as `INTEGER` Unix epoch milliseconds (UTC) in SQLite.

use jiff::Timestamp;

/// SQLx adapter for date-time fields backed by INTEGER epoch milliseconds.
///
/// SQLite timestamp columns in this application store integer milliseconds, not seconds.
#[derive(sqlx::Type)]
#[sqlx(transparent)]
pub struct EpochMillis(i64);

#[derive(Debug, thiserror::Error)]
#[error("epoch millisecond timestamp {0} is outside Jiff's supported range")]
pub struct InvalidEpochMillis(i64);

impl TryFrom<EpochMillis> for Timestamp {
    type Error = InvalidEpochMillis;

    fn try_from(value: EpochMillis) -> Result<Self, Self::Error> {
        Timestamp::from_millisecond(value.0).map_err(|_| InvalidEpochMillis(value.0))
    }
}

/// Current time as Unix epoch milliseconds (UTC).
#[inline]
pub fn now_ms() -> i64 {
    datetime_to_ms(Timestamp::now())
}

/// Convert a `Timestamp` to Unix epoch milliseconds, rounding down.
#[inline]
pub fn datetime_to_ms(dt: Timestamp) -> i64 {
    dt.as_nanosecond().div_euclid(1_000_000) as i64
}

/// Convert Unix epoch milliseconds to `Timestamp`.
///
/// Values outside Jiff's supported range clamp to the nearest representable timestamp.
#[inline]
pub fn ms_to_datetime(ms: i64) -> Timestamp {
    Timestamp::from_millisecond(ms).unwrap_or(if ms.is_negative() {
        Timestamp::MIN
    } else {
        Timestamp::MAX
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_millisecond_timestamp_before_epoch_uses_preceding_millisecond() {
        let time = "1969-12-31T23:59:59.999999Z".parse().unwrap();
        assert_eq!(datetime_to_ms(time), -1);
        assert!(ms_to_datetime(-1) <= time && time < ms_to_datetime(0));
    }
}
