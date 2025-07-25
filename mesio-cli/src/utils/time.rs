use crate::error::AppError;
use std::fmt::Write;

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
        "s" => Ok(value),
        "m" => Ok(value * 60.0),
        "h" => Ok(value * 3600.0),
        _ => Err(AppError::ParseError("Invalid unit".to_string())),
    }
}

/// Convert seconds to a human-readable format
#[allow(dead_code)]
pub fn format_duration(seconds: f64) -> String {
    // Pre-allocate
    let mut result = String::with_capacity(10);

    if seconds >= 3600.0 {
        let hours = seconds / 3600.0;
        write!(result, "{hours:.2}h").unwrap();
    } else if seconds >= 60.0 {
        let minutes = seconds / 60.0;
        write!(result, "{minutes:.2}m").unwrap();
    } else {
        write!(result, "{seconds:.2}s").unwrap();
    }

    result
}
