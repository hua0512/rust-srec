use crate::error::AppError;

/// Function to parse size with units
/// # Arguments
///
/// * `size_str` - The string to parse
///
/// # Returns
///
/// * `Ok(size)` - The parsed size in bytes
/// * `Err(AppError)` - If the string is invalid
///
pub fn parse_size(size_str: &str) -> Result<u64, AppError> {
    // Trim whitespace and handle case-insensitivity
    let size_str = size_str.trim().to_lowercase();

    // Handle empty string
    if size_str.is_empty() {
        return Err(AppError::ParseError(
            "Invalid format: empty string".to_string(),
        ));
    }

    // Split the numeric part and the unit
    let mut numeric_part = String::new();
    let mut unit_part = String::new();

    for c in size_str.chars() {
        if c.is_ascii_digit() || c == '.' {
            numeric_part.push(c);
        } else {
            unit_part.push(c);
        }
    }

    // Handle cases with no unit (assume bytes)
    if unit_part.is_empty() {
        let bytes = numeric_part
            .parse::<u64>()
            .map_err(|_| AppError::ParseError("Invalid number".to_string()))?;
        return Ok(bytes);
    }

    // Parse the numeric part
    let value = numeric_part
        .parse::<f64>()
        .map_err(|_| AppError::ParseError("Invalid number".to_string()))?;

    // Parse the unit and convert to bytes
    match unit_part.trim() {
        "b" => Ok(value as u64),
        "kb" => Ok((value * 1024.0) as u64),
        "mb" => Ok((value * 1024.0 * 1024.0) as u64),
        "gb" => Ok((value * 1024.0 * 1024.0 * 1024.0) as u64),
        "tb" => Ok((value * 1024.0 * 1024.0 * 1024.0 * 1024.0) as u64),
        _ => Err(AppError::ParseError("Invalid unit".to_string())),
    }
}

/// Convert bytes to a human-readable format
pub fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}
