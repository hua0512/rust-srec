//! Parsers for the XML Schema datatypes the MPD uses (XSD 1.1 Part 2).
//!
//! Each parser returns a static reason on failure; callers attach the element
//! and attribute names. Leading and trailing whitespace is collapsed as the
//! schema's `whiteSpace="collapse"` facet requires.
//!
//! Decimal values are converted exactly from their digits (never through
//! `f64`) and rounded to the nearest nanosecond, ties rounding up.

use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};

use super::timeline::AvailabilityTimeOffset;

const NANOS_PER_SEC: u64 = 1_000_000_000;
const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 3_600;
const SECS_PER_DAY: u64 = 86_400;

/// Largest `@availabilityTimeOffset` accepted, in seconds (~31 700 years):
/// far beyond any meaningful offset.
const MAX_OFFSET_SECS: u64 = 1_000_000_000_000;

/// Parses an `xs:duration` (ISO 8601 `PnYnMnDTnHnMnS`) into a [`Duration`].
///
/// Negative durations are rejected: no MPD duration attribute has a meaning
/// for them. Years and months have no fixed length, so they are accepted only
/// when zero (some packagers write `P0Y0M0DT0H0M2.000S`). Days are exactly
/// 86 400 s. Seconds are rounded to the nearest nanosecond, ties up.
pub(crate) fn parse_duration(input: &str) -> Result<Duration, &'static str> {
    const TOO_LARGE: &str = "duration is too large";
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
    let mut add_secs = |value: u64, unit: u64| -> Result<(), &'static str> {
        let product = value.checked_mul(unit).ok_or(TOO_LARGE)?;
        secs = secs.checked_add(product).ok_or(TOO_LARGE)?;
        Ok(())
    };

    let mut second_nanos: u128 = 0;
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
                _ => second_nanos = decimal_nanos(value.integer, value.fraction.unwrap_or(""), 0)?,
            }
        }
    }
    let second_secs =
        u64::try_from(second_nanos / u128::from(NANOS_PER_SEC)).map_err(|_| TOO_LARGE)?;
    add_secs(second_secs, 1)?;
    // The remainder is below 1e9.
    let subsec = (second_nanos % u128::from(NANOS_PER_SEC)) as u32;
    Ok(Duration::new(secs, subsec))
}

/// A decimal numeral split at its decimal point; both parts are ASCII digits.
struct Numeral<'a> {
    integer: &'a str,
    fraction: Option<&'a str>,
}

impl Numeral<'_> {
    fn whole(&self) -> Result<u64, &'static str> {
        if self.fraction.is_some() {
            return Err("only seconds may have a fraction");
        }
        if self.integer.is_empty() {
            return Ok(0);
        }
        self.integer.parse().map_err(|_| "duration is too large")
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

/// Converts the decimal `integer.fraction × 10^exponent` seconds to
/// nanoseconds, rounding to the nearest nanosecond with ties rounded up.
///
/// `integer` and `fraction` must be ASCII digits (either may be empty).
fn decimal_nanos(integer: &str, fraction: &str, exponent: i64) -> Result<u128, &'static str> {
    const TOO_LARGE: &str = "value is too large";
    let digits: Vec<u8> = integer
        .bytes()
        .chain(fraction.bytes())
        .skip_while(|&digit| digit == b'0')
        .collect();
    if digits.is_empty() {
        return Ok(0);
    }
    let fraction_len = i64::try_from(fraction.len()).map_err(|_| TOO_LARGE)?;
    let length = i64::try_from(digits.len()).map_err(|_| TOO_LARGE)?;
    // The value is `digits × 10^shift` nanoseconds.
    let shift = exponent
        .checked_add(9)
        .and_then(|shift| shift.checked_sub(fraction_len))
        .ok_or(TOO_LARGE)?;
    // u128 holds 38 full decimal digits.
    let kept = length.checked_add(shift).ok_or(TOO_LARGE)?;
    if kept > 38 {
        return Err(TOO_LARGE);
    }
    let parse = |digits: &[u8]| -> u128 {
        digits
            .iter()
            .fold(0u128, |value, &digit| value * 10 + u128::from(digit - b'0'))
    };
    if shift >= 0 {
        // `kept <= 38` bounds both the digits and the scale.
        return Ok(parse(&digits) * 10u128.pow(shift as u32));
    }
    if kept < 0 {
        // Every significant digit is below half a nanosecond.
        return Ok(0);
    }
    let kept = kept as usize;
    let rounding = u128::from(digits.get(kept).is_some_and(|&digit| digit >= b'5'));
    Ok(parse(&digits[..kept]) + rounding)
}

/// Parses an `xs:dateTime`.
///
/// A value without a timezone is interpreted as UTC: the MPD's wall-clock
/// attributes (`MPD@availabilityStartTime`, `MPD@publishTime`, ISO/IEC 23009-1
/// §5.3.1.2) are UTC times, and the reference players read them that way.
/// Fractional seconds are rounded to the nearest nanosecond, ties up.
pub(crate) fn parse_date_time(input: &str) -> Result<DateTime<Utc>, &'static str> {
    const INVALID: &str = "expected an xs:dateTime such as 2024-01-01T00:00:00Z";
    let s = input.trim();
    // `YYYY-MM-DDThh:mm:ss`, then an optional fraction and timezone.
    let head = s.get(..19).ok_or(INVALID)?;
    let mut rest = &s[19..];
    let mut nanos = 0u128;
    if let Some(after_point) = rest.strip_prefix('.') {
        let digits = after_point.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return Err(INVALID);
        }
        nanos = decimal_nanos("", &after_point[..digits], 0)?;
        rest = &after_point[digits..];
    }
    let whole = if rest.is_empty() {
        NaiveDateTime::parse_from_str(head, "%Y-%m-%dT%H:%M:%S")
            .map(|time| time.and_utc())
            .map_err(|_| INVALID)?
    } else {
        DateTime::parse_from_rfc3339(&format!("{head}{rest}"))
            .map(|time| time.with_timezone(&Utc))
            .map_err(|_| INVALID)?
    };
    // At most one second after rounding.
    let fraction = TimeDelta::nanoseconds(i64::try_from(nanos).map_err(|_| INVALID)?);
    whole.checked_add_signed(fraction).ok_or(INVALID)
}

/// A parsed `@availabilityTimeOffset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RawOffset {
    Offset(AvailabilityTimeOffset),
    /// A negative value (including `-INF`): valid `xs:double`, but the
    /// standard gives it no meaning.
    Negative,
}

/// Parses `@availabilityTimeOffset` (`xs:double` seconds, `INF` allowed).
pub(crate) fn parse_availability_time_offset(input: &str) -> Result<RawOffset, &'static str> {
    const INVALID: &str = "expected a number of seconds or INF";
    let s = input.trim();
    let (negative, unsigned) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    if unsigned == "INF" {
        return Ok(if negative {
            RawOffset::Negative
        } else {
            RawOffset::Offset(AvailabilityTimeOffset::Infinite)
        });
    }
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => {
            let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(INVALID);
            }
            // Out-of-range exponents are either far too large or round to 0.
            let exponent = exponent
                .parse::<i64>()
                .map(|exponent| exponent.clamp(-1_000_000, 1_000_000))
                .unwrap_or(if exponent.starts_with('-') {
                    -1_000_000
                } else {
                    1_000_000
                });
            (mantissa, exponent)
        }
        None => (unsigned, 0),
    };
    let (integer, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if integer.is_empty() && fraction.is_empty()
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(INVALID);
    }
    let nanos = decimal_nanos(integer, fraction, exponent).map_err(|_| "offset is too large")?;
    if nanos == 0 {
        return Ok(RawOffset::Offset(AvailabilityTimeOffset::ZERO));
    }
    if negative {
        return Ok(RawOffset::Negative);
    }
    let secs = u64::try_from(nanos / u128::from(NANOS_PER_SEC))
        .ok()
        .filter(|&secs| secs <= MAX_OFFSET_SECS)
        .ok_or("offset is too large")?;
    // The remainder is below 1e9.
    let subsec = (nanos % u128::from(NANOS_PER_SEC)) as u32;
    Ok(RawOffset::Offset(AvailabilityTimeOffset::Finite(
        Duration::new(secs, subsec),
    )))
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
            ("PT86400S", Duration::from_secs(86_400)),
            ("PT00000000000000000000000000002S", Duration::from_secs(2)),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_duration(input), Ok(expected), "{input}");
        }
    }

    #[test]
    fn duration_seconds_round_to_the_nearest_nanosecond() {
        let cases = [
            ("PT0.123456789123S", Duration::new(0, 123_456_789)),
            ("PT0.1234567894S", Duration::new(0, 123_456_789)),
            // Ties round up.
            ("PT0.1234567895S", Duration::new(0, 123_456_790)),
            ("PT0.0000000005S", Duration::new(0, 1)),
            ("PT0.0000000004999S", Duration::ZERO),
            // Rounding carries into the seconds.
            ("PT1.9999999995S", Duration::from_secs(2)),
            ("PT59.99999999999M", Duration::ZERO),
        ];
        for (input, expected) in &cases[..6] {
            assert_eq!(parse_duration(input), Ok(*expected), "{input}");
        }
        assert!(
            parse_duration(cases[6].0).is_err(),
            "minutes take no fraction"
        );
        // Exact decimal: 0.1 + 0.2 style binary errors cannot appear.
        assert_eq!(
            parse_duration("PT0.3S"),
            Ok(Duration::from_nanos(300_000_000))
        );
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
    fn date_time_fractions_round_to_the_nearest_nanosecond() {
        let at = |nanos| Utc.timestamp_opt(1_714_979_289, nanos).unwrap();
        assert_eq!(
            parse_date_time("2024-05-06T07:08:09.1234567894Z"),
            Ok(at(123_456_789))
        );
        assert_eq!(
            parse_date_time("2024-05-06T07:08:09.1234567895Z"),
            Ok(at(123_456_790))
        );
        assert_eq!(
            parse_date_time("2024-05-06T07:08:09.9999999999+00:00"),
            Ok(Utc.timestamp_opt(1_714_979_290, 0).unwrap()),
            "rounding carries into the seconds"
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
            "2024-01-01T00:00:00.Z",
            "2024-01-01T00:00:00.5junk",
            "2024-02-30T00:00:00Z",
        ] {
            assert!(parse_date_time(input).is_err(), "{input:?} should fail");
        }
    }

    #[test]
    fn availability_time_offsets() {
        use AvailabilityTimeOffset::{Finite, Infinite};
        let finite = |nanos| Ok(RawOffset::Offset(Finite(Duration::from_nanos(nanos))));
        assert_eq!(
            parse_availability_time_offset("INF"),
            Ok(RawOffset::Offset(Infinite))
        );
        assert_eq!(
            parse_availability_time_offset("+INF"),
            Ok(RawOffset::Offset(Infinite))
        );
        assert_eq!(parse_availability_time_offset("1.8"), finite(1_800_000_000));
        assert_eq!(parse_availability_time_offset("0"), finite(0));
        assert_eq!(parse_availability_time_offset("-0"), finite(0));
        assert_eq!(parse_availability_time_offset("5E-1"), finite(500_000_000));
        assert_eq!(
            parse_availability_time_offset("1.5e+1"),
            finite(15_000_000_000)
        );
        assert_eq!(parse_availability_time_offset(".25"), finite(250_000_000));
        assert_eq!(parse_availability_time_offset("1e-9999999999"), finite(0));
        assert_eq!(
            parse_availability_time_offset("0.0000000015"),
            finite(2),
            "ties round up"
        );
        for input in ["-1", "-INF", "-0.5e1"] {
            assert_eq!(
                parse_availability_time_offset(input),
                Ok(RawOffset::Negative),
                "{input}"
            );
        }
        for input in [
            "NaN", "inf", "infinity", "", "1s", "1e300", "1e", ".", "e5", "1.2.3", "--1",
        ] {
            assert!(
                parse_availability_time_offset(input).is_err(),
                "{input:?} should fail"
            );
        }
    }

    #[test]
    fn decimal_conversion_is_exact() {
        assert_eq!(decimal_nanos("1", "", 0), Ok(1_000_000_000));
        assert_eq!(decimal_nanos("", "000000001", 0), Ok(1));
        assert_eq!(decimal_nanos("12", "5", -10), Ok(1));
        assert_eq!(decimal_nanos("12", "4", -10), Ok(1));
        assert_eq!(decimal_nanos("1", "", -10), Ok(0));
        assert_eq!(decimal_nanos("5", "", -10), Ok(1));
        assert_eq!(decimal_nanos("", "", 0), Ok(0));
        assert!(decimal_nanos("1", "", 40).is_err());
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
