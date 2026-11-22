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
    let size_str = size_str.trim();
    if size_str.is_empty() {
        return Err(AppError::ParseError(
            "Invalid format: empty string".to_string(),
        ));
    }

    // Split cleanly at the first non-digit / non-decimal-point character so
    // interleaved digits inside units (such as "1k2b") are rejected.
    let split_index = size_str
        .char_indices()
        .find(|&(_, c)| !c.is_ascii_digit() && c != '.')
        .map_or(size_str.len(), |(idx, _)| idx);

    if split_index == 0 {
        return Err(AppError::ParseError("Invalid number".to_string()));
    }

    let numeric_part = &size_str[..split_index];
    let unit_part = size_str[split_index..].trim().to_ascii_lowercase();

    if unit_part.is_empty() {
        return numeric_part
            .parse::<u64>()
            .map_err(|_| AppError::ParseError("Invalid number".to_string()));
    }

    let value = numeric_part
        .parse::<f64>()
        .map_err(|_| AppError::ParseError("Invalid number".to_string()))?;
    if !value.is_finite() || value < 0.0 {
        return Err(AppError::ParseError("Invalid number".to_string()));
    }

    let multiplier = match unit_part.as_str() {
        "b" => 1.0,
        "kb" => 1024.0,
        "mb" => 1024.0 * 1024.0,
        "gb" => 1024.0 * 1024.0 * 1024.0,
        "tb" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(AppError::ParseError("Invalid unit".to_string())),
    };
    let bytes = value * multiplier;
    if !bytes.is_finite() || bytes > u64::MAX as f64 {
        return Err(AppError::ParseError("Size out of range".to_string()));
    }
    Ok(bytes as u64)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_rejects_interleaved_digits_and_overflow() {
        assert_eq!(parse_size("1.5 MB").unwrap(), 1_572_864);
        assert!(parse_size("1k2b").is_err());
        assert!(parse_size("20000000tb").is_err());
    }
}
