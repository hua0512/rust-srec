//! Parsers for the XML Schema datatypes the MPD uses (XSD 1.1 Part 2).
//!
//! Each parser returns a static reason on failure; callers attach the element
//! and attribute names. Leading and trailing whitespace is collapsed as the
//! schema's `whiteSpace="collapse"` facet requires.

use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};

use super::timeline::AvailabilityTimeOffset;

const NANOS_PER_SEC: u64 = 1_000_000_000;
const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 3_600;
const SECS_PER_DAY: u64 = 86_400;

/// Parses an `xs:duration` (ISO 8601 `PnYnMnDTnHnMnS`) into a [`Duration`].
///
/// Negative durations are rejected: no MPD duration attribute has a meaning
/// for them. Years and months have no fixed length, so they are accepted only
/// when zero (some packagers write `P0Y0M0DT0H0M2.000S`). Days are exactly
/// 86 400 s. Fractional seconds beyond nanosecond precision are truncated.
pub(crate) fn parse_duration(input: &str) -> Result<Duration, &'static str> {
    let s = input.trim();
    if s.starts_with('-') {
        return Err("negative durations are not allowed");
    }
    let rest = s
        .strip_prefix('P')
        .ok_or("an ISO 8601 duration starts with `P`")?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((date, time)) => {
            if time.is_empty() {
                return Err("`T` must be followed by a time component");
            }
            (date, Some(time))
        }
        None => (rest, None),
    };
    if date_part.is_empty() && time_part.is_none() {
        return Err("duration has no components");
    }

    let mut secs: u64 = 0;
    let mut nanos: u32 = 0;
    let mut add_secs = |value: u64, unit: u64| -> Result<(), &'static str> {
        let product = value.checked_mul(unit).ok_or("duration is too large")?;
        secs = secs.checked_add(product).ok_or("duration is too large")?;
        Ok(())
    };

    for (value, unit) in components(date_part, &['Y', 'M', 'D'])? {
        let whole = value.whole()?;
        match unit {
            'Y' | 'M' if whole != 0 => return Err("years and months have no fixed length"),
            'D' => add_secs(whole, SECS_PER_DAY)?,
            _ => {}
        }
    }
    if let Some(time_part) = time_part {
        for (value, unit) in components(time_part, &['H', 'M', 'S'])? {
            match unit {
                'H' => add_secs(value.whole()?, SECS_PER_HOUR)?,
                'M' => add_secs(value.whole()?, SECS_PER_MINUTE)?,
                _ => {
                    add_secs(value.integer()?, 1)?;
                    nanos = value.fraction_nanos();
                }
            }
        }
    }
    Ok(Duration::new(secs, nanos))
}

/// A decimal numeral split at its decimal point.
struct Numeral<'a> {
    integer: &'a str,
    fraction: Option<&'a str>,
}

impl Numeral<'_> {
    fn integer(&self) -> Result<u64, &'static str> {
        if self.integer.is_empty() {
            return Ok(0);
        }
        self.integer.parse().map_err(|_| "duration is too large")
    }

    fn whole(&self) -> Result<u64, &'static str> {
        if self.fraction.is_some() {
            return Err("only seconds may have a fraction");
        }
        self.integer()
    }

    fn fraction_nanos(&self) -> u32 {
        let mut nanos = 0u32;
        let digits = self
            .fraction
            .unwrap_or("")
            .bytes()
            .chain(std::iter::repeat(b'0'));
        for digit in digits.take(9) {
            nanos = nanos * 10 + u32::from(digit - b'0');
        }
        nanos
    }
}

/// Splits `part` into `(numeral, designator)` pairs, enforcing the designator
/// order given by `units` with each designator at most once.
fn components<'a>(part: &'a str, units: &[char]) -> Result<Vec<(Numeral<'a>, char)>, &'static str> {
    let mut out = Vec::new();
    let mut next_unit = 0;
    let mut start = 0;
    for (index, ch) in part.char_indices() {
        if ch.is_ascii_digit() || ch == '.' {
            continue;
        }
        let position = units[next_unit..]
            .iter()
            .position(|&unit| unit == ch)
            .ok_or("unexpected or out-of-order designator")?;
        next_unit += position + 1;
        let numeral = &part[start..index];
        let (integer, fraction) = match numeral.split_once('.') {
            Some((integer, fraction)) => (integer, Some(fraction)),
            None => (numeral, None),
        };
        if integer.is_empty() && fraction.is_none_or(str::is_empty) {
            return Err("designator without a number");
        }
        if fraction.is_some_and(|fraction| fraction.contains('.')) {
            return Err("more than one decimal point");
        }
        out.push((Numeral { integer, fraction }, ch));
        start = index + ch.len_utf8();
    }
    if start != part.len() {
        return Err("number without a designator");
    }
    Ok(out)
}

/// Parses an `xs:dateTime`.
///
/// A value without a timezone is interpreted as UTC: the MPD's wall-clock
/// attributes (`MPD@availabilityStartTime`, `MPD@publishTime`, ISO/IEC 23009-1
/// §5.3.1.2) are UTC times, and the reference players read them that way.
pub(crate) fn parse_date_time(input: &str) -> Result<DateTime<Utc>, &'static str> {
    let s = input.trim();
    const INVALID: &str = "expected an xs:dateTime such as 2024-01-01T00:00:00Z";
    // The date part is `YYYY-MM-DD`; a timezone can only follow the time part.
    let time_part = s.get(10..).ok_or(INVALID)?;
    let has_zone = time_part.ends_with(['Z', 'z']) || time_part.contains(['+', '-']);
    if has_zone {
        DateTime::parse_from_rfc3339(s)
            .map(|time| time.with_timezone(&Utc))
            .map_err(|_| INVALID)
    } else {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            .map(|time| time.and_utc())
            .map_err(|_| INVALID)
    }
}

/// Parses `@availabilityTimeOffset` (`xs:double` seconds, `INF` allowed).
///
/// Negative, `-INF` and `NaN` values are rejected: the offset moves
/// availability earlier and has no defined meaning otherwise. The value is
/// rounded to whole nanoseconds.
pub(crate) fn parse_availability_time_offset(
    input: &str,
) -> Result<AvailabilityTimeOffset, &'static str> {
    let s = input.trim();
    if s == "INF" || s == "+INF" {
        return Ok(AvailabilityTimeOffset::Infinite);
    }
    // `f64::from_str` also accepts `inf`, `nan` and `infinity`, which are not
    // xs:double lexical forms.
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return Err("expected a non-negative number of seconds or INF");
    }
    let value: f64 = s
        .parse()
        .map_err(|_| "expected a non-negative number of seconds or INF")?;
    if !value.is_finite() || value < 0.0 {
        return Err("expected a non-negative number of seconds or INF");
    }
    // 1e12 s (~31 700 years) is far beyond any meaningful offset and keeps the
    // conversion below exact for every value with nanosecond precision.
    if value > 1e12 {
        return Err("offset is too large");
    }
    let nanos = (value * 1e9).round() as u128;
    let secs = u64::try_from(nanos / 1_000_000_000).map_err(|_| "offset is too large")?;
    // The remainder is below 1e9 and fits.
    let subsec = (nanos % 1_000_000_000) as u32;
    Ok(AvailabilityTimeOffset::Finite(Duration::new(secs, subsec)))
}

/// Parses an `xs:boolean`.
pub(crate) fn parse_bool(input: &str) -> Result<bool, &'static str> {
    match input.trim() {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => Err("expected true, false, 1 or 0"),
    }
}

/// Parses an unsigned integer (`xs:unsignedInt` / `xs:unsignedLong`).
pub(crate) fn parse_u64(input: &str) -> Result<u64, &'static str> {
    input
        .trim()
        .parse()
        .map_err(|_| "expected an unsigned integer")
}

/// Parses an `xs:integer` that must fit in `i64`.
pub(crate) fn parse_i64(input: &str) -> Result<i64, &'static str> {
    input.trim().parse().map_err(|_| "expected an integer")
}

/// Converts a [`Duration`] to signed nanoseconds for exact timing arithmetic.
pub(crate) fn duration_nanos(duration: Duration) -> i128 {
    i128::from(duration.as_secs()) * i128::from(NANOS_PER_SEC) + i128::from(duration.subsec_nanos())
}

/// Converts a UTC instant to nanoseconds since the Unix epoch, without the
/// `i64` range limit of `timestamp_nanos_opt`.
pub(crate) fn instant_nanos(time: DateTime<Utc>) -> i128 {
    i128::from(time.timestamp()) * i128::from(NANOS_PER_SEC)
        + i128::from(time.timestamp_subsec_nanos())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn durations() {
        let cases = [
            ("PT0S", Duration::ZERO),
            ("PT2S", Duration::from_secs(2)),
            ("PT1.5S", Duration::from_millis(1500)),
            ("PT0.25S", Duration::from_millis(250)),
            ("PT.5S", Duration::from_millis(500)),
            ("PT1H2M3.5S", Duration::from_millis(3_723_500)),
            ("PT5M", Duration::from_secs(300)),
            ("P1D", Duration::from_secs(86_400)),
            ("P1DT12H", Duration::from_secs(129_600)),
            ("P0Y0M0DT0H0M2.000S", Duration::from_secs(2)),
            (" PT30S ", Duration::from_secs(30)),
            ("PT0.123456789123S", Duration::new(0, 123_456_789)),
            ("PT86400S", Duration::from_secs(86_400)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_duration(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn malformed_durations_are_rejected() {
        for input in [
            "",
            "P",
            "PT",
            "T30S",
            "PT30",
            "PT30X",
            "-PT5S",
            "P1Y",
            "P2M",
            "PT1.5M",
            "P1.5D",
            "PTS",
            "PT.S",
            "PT1..5S",
            "PT1S2M",
            "PT1H1H",
            "P1DT",
            "PT1M1M",
            "PT1.2.3S",
            "PT99999999999999999999S",
            "P99999999999999999D",
            "PT１S",
        ] {
            assert!(parse_duration(input).is_err(), "{input:?} should fail");
        }
    }

    #[test]
    fn date_times() {
        let epoch = Utc.timestamp_opt(0, 0).unwrap();
        assert_eq!(parse_date_time("1970-01-01T00:00:00Z"), Ok(epoch));
        assert_eq!(parse_date_time("1970-01-01T00:00:00"), Ok(epoch));
        assert_eq!(
            parse_date_time("1970-01-01T01:00:00+01:00"),
            Ok(epoch),
            "offsets are applied"
        );
        assert_eq!(
            parse_date_time("2024-05-06T07:08:09.123Z"),
            Ok(Utc.timestamp_opt(1_714_979_289, 123_000_000).unwrap())
        );
        assert_eq!(
            parse_date_time("2024-05-06T07:08:09.5"),
            Ok(Utc.timestamp_opt(1_714_979_289, 500_000_000).unwrap())
        );
    }

    #[test]
    fn malformed_date_times_are_rejected() {
        for input in [
            "",
            "2024",
            "2024-01-01",
            "2024-13-01T00:00:00Z",
            "2024-01-01T25:00:00Z",
            "2024-01-01 00:00",
            "yesterday",
            "2024-01-01T00:00:00+25:00",
            "2024-01-01T00:00Z",
        ] {
            assert!(parse_date_time(input).is_err(), "{input:?} should fail");
        }
    }

    #[test]
    fn availability_time_offsets() {
        use AvailabilityTimeOffset::{Finite, Infinite};
        assert_eq!(parse_availability_time_offset("INF"), Ok(Infinite));
        assert_eq!(
            parse_availability_time_offset("1.8"),
            Ok(Finite(Duration::from_millis(1800)))
        );
        assert_eq!(
            parse_availability_time_offset("0"),
            Ok(Finite(Duration::ZERO))
        );
        assert_eq!(
            parse_availability_time_offset("5E-1"),
            Ok(Finite(Duration::from_millis(500)))
        );
        for input in ["-1", "-INF", "NaN", "inf", "infinity", "", "1s", "1e300"] {
            assert!(
                parse_availability_time_offset(input).is_err(),
                "{input:?} should fail"
            );
        }
    }

    #[test]
    fn scalars() {
        assert_eq!(parse_bool("true"), Ok(true));
        assert_eq!(parse_bool("0"), Ok(false));
        assert!(parse_bool("yes").is_err());
        assert_eq!(parse_u64(" 42 "), Ok(42));
        assert!(parse_u64("-1").is_err());
        assert_eq!(parse_i64("-1"), Ok(-1));
    }
}
