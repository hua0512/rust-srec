use crate::error::AppError;
use std::time::Duration;

/// Function to parse time with units
/// # Arguments
///
/// * `time_str` - The string to parse
///
/// # Returns
///
/// * `Ok(time)` - The parsed time in seconds
/// * `Err(AppError)` - If the string is invalid
pub fn parse_time(time_str: &str) -> Result<f64, AppError> {
    let time_str = time_str.trim();
    if time_str.is_empty() {
        return Err(AppError::ParseError(
            "Invalid format: empty string".to_string(),
        ));
    }

    if let Ok(seconds) = time_str.parse::<f64>() {
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(AppError::ParseError("Invalid number".to_string()));
        }
        return Ok(seconds);
    }

    let split_index = time_str
        .char_indices()
        .find(|&(_, c)| !c.is_ascii_digit() && c != '.')
        .map_or(time_str.len(), |(idx, _)| idx);

    if split_index == 0 {
        return Err(AppError::ParseError("Invalid number".to_string()));
    }

    let numeric_part = &time_str[..split_index];
    let unit_part = time_str[split_index..].trim().to_ascii_lowercase();

    let value = numeric_part
        .parse::<f64>()
        .map_err(|_| AppError::ParseError("Invalid number".to_string()))?;
    if !value.is_finite() || value < 0.0 {
        return Err(AppError::ParseError("Invalid number".to_string()));
    }

    let seconds = match unit_part.as_str() {
        "ms" => value / 1000.0,
        "s" => value,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        _ => return Err(AppError::ParseError("Invalid unit".to_string())),
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(AppError::ParseError("Invalid number".to_string()));
    }
    Ok(seconds)
}

/// Parses a duration flag value, where `0` (or `0s`, `0ms`, ...) means
/// "disabled" and yields `None`.
///
/// The engine treats a zero duration as "no timeout"; the CLI spells that as
/// `0`, matching the existing `--timeout`/`--read-timeout` convention.
pub fn parse_duration_flag(value: &str) -> Result<Option<Duration>, AppError> {
    // Rejects negative, non-finite and out-of-range values.
    let duration = Duration::try_from_secs_f64(parse_time(value)?)
        .map_err(|_| AppError::ParseError(format!("Invalid duration: {value}")))?;
    Ok((!duration.is_zero()).then_some(duration))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units_and_bare_seconds() {
        assert_eq!(parse_time("90").unwrap(), 90.0);
        assert_eq!(parse_time("1.5s").unwrap(), 1.5);
        assert_eq!(parse_time("500ms").unwrap(), 0.5);
        assert_eq!(parse_time("2m").unwrap(), 120.0);
        assert_eq!(parse_time("1h").unwrap(), 3600.0);
        assert!(parse_time("10x").is_err());
        assert!(parse_time("-5").is_err());
        assert!(parse_time("-5s").is_err());
        assert!(parse_time("NaN").is_err());
        assert!(parse_time("inf").is_err());
        assert!(parse_time("-inf").is_err());
        assert!(parse_time("").is_err());
    }

    #[test]
    fn zero_duration_flag_disables_the_timeout() {
        assert_eq!(parse_duration_flag("0").unwrap(), None);
        assert_eq!(parse_duration_flag("0ms").unwrap(), None);
        assert_eq!(
            parse_duration_flag("90s").unwrap(),
            Some(Duration::from_secs(90))
        );
        assert_eq!(
            parse_duration_flag("500ms").unwrap(),
            Some(Duration::from_millis(500))
        );
        assert!(parse_duration_flag("-1").is_err());
        assert!(parse_duration_flag("1e30").is_err());
    }
}
