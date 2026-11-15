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
    // Trim whitespace and handle empty string
    let time_str = time_str.trim();
    if time_str.is_empty() {
        return Err(AppError::ParseError(
            "Invalid format: empty string".to_string(),
        ));
    }

    // Try to parse as a simple number (seconds) first for efficiency
    if let Ok(seconds) = time_str.parse::<f64>() {
        return Ok(seconds);
    }

    // Find the split point between numeric part and unit part
    let mut split_index = 0;
    for (i, c) in time_str.char_indices() {
        if !c.is_ascii_digit() && c != '.' {
            split_index = i;
            break;
        }
    }

    // If we didn't find any non-numeric characters, handle it as just a number
    if split_index == 0 && !time_str.is_empty() {
        // The entire string is numbers or decimal points
        return time_str
            .parse::<f64>()
            .map_err(|_| AppError::ParseError("Invalid number".to_string()));
    }

    // Split the string into numeric and unit parts
    let numeric_part = &time_str[0..split_index];
    let unit_part = time_str[split_index..].trim().to_lowercase();

    // Parse the numeric part
    let value = numeric_part
        .parse::<f64>()
        .map_err(|_| AppError::ParseError("Invalid number".to_string()))?;

    // Parse the unit and convert to seconds
    match unit_part.as_str() {
        "ms" => Ok(value / 1000.0),
        "s" => Ok(value),
        "m" => Ok(value * 60.0),
        "h" => Ok(value * 3600.0),
        _ => Err(AppError::ParseError("Invalid unit".to_string())),
    }
}

/// Parses a duration flag value, where `0` (or `0s`, `0ms`, ...) means
/// "disabled" and yields `None`.
///
/// The engine treats a zero duration as "no timeout"; the CLI spells that as
/// `0`, matching the existing `--timeout`/`--read-timeout` convention.
pub fn parse_duration_flag(value: &str) -> Result<Option<Duration>, AppError> {
    let seconds = parse_time(value)?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(AppError::ParseError(format!(
            "Invalid duration: {value}"
        )));
    }
    Ok((seconds > 0.0).then(|| Duration::from_secs_f64(seconds)))
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
    }
}
