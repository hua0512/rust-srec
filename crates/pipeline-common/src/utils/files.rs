use std::time::{SystemTime, UNIX_EPOCH};

/// Expand path template with placeholders similar to FFmpeg.
///
/// Unlike `expand_filename_template`, this function does NOT sanitize the result,
/// making it suitable for directory paths that may contain `:` (Windows drive letters)
/// or `\`/`/` path separators.
///
/// Supported placeholders:
/// - `%Y` - Year (YYYY)
/// - `%m` - Month (01-12)
/// - `%d` - Day (01-31)
/// - `%H` - Hour (00-23)
/// - `%M` - Minute (00-59)
/// - `%S` - Second (00-59)
/// - `%t` - Unix timestamp
/// - `%%` - Literal percent sign
pub fn expand_path_template(template: &str) -> String {
    expand_template_internal(template, None, false, None)
}

/// Expand path template with placeholders using a specific reference timestamp.
///
/// Same as `expand_path_template`, but uses the provided timestamp (Unix epoch milliseconds)
/// instead of the current time.
pub fn expand_path_template_at(template: &str, reference_timestamp_ms: Option<i64>) -> String {
    expand_template_internal(template, None, false, reference_timestamp_ms)
}

/// Expand filename template with placeholders similar to FFmpeg
pub fn expand_filename_template(template: &str, sequence_number: Option<u32>) -> String {
    expand_template_internal(template, sequence_number, true, None)
}

/// Internal implementation for expanding templates.
///
/// # Arguments
/// * `template` - The template string to expand
/// * `sequence_number` - Optional sequence number for `%i` placeholder
/// * `sanitize` - Whether to sanitize the result for use as a filename
/// * `reference_timestamp_ms` - Optional reference timestamp in Unix epoch milliseconds.
///   If None, uses the current time.
fn expand_template_internal(
    template: &str,
    sequence_number: Option<u32>,
    sanitize: bool,
    reference_timestamp_ms: Option<i64>,
) -> String {
    let now = if let Some(ts_ms) = reference_timestamp_ms {
        time::OffsetDateTime::from_unix_timestamp(ts_ms / 1000)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
    } else {
        time::OffsetDateTime::now_local().unwrap_or_else(|_| time::OffsetDateTime::now_utc())
    };
    let reference_timestamp_secs = reference_timestamp_ms
        .map(|ms| (ms / 1000) as u64)
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });
    let mut result = String::with_capacity(template.len() * 2);
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&next_char) = chars.peek() {
                match next_char {
                    // Date and time placeholders
                    'Y' => {
                        result.push_str(&format!("{:04}", now.year())); // Year (YYYY)
                        chars.next();
                    }
                    'm' => {
                        result.push_str(&format!("{:02}", now.month() as u8)); // Month (01-12)
                        chars.next();
                    }
                    'd' => {
                        result.push_str(&format!("{:02}", now.day())); // Day (01-31)
                        chars.next();
                    }
                    'H' => {
                        result.push_str(&format!("{:02}", now.hour())); // Hour (00-23)
                        chars.next();
                    }
                    'M' => {
                        result.push_str(&format!("{:02}", now.minute())); // Minute (00-59)
                        chars.next();
                    }
                    'S' => {
                        result.push_str(&format!("{:02}", now.second())); // Second (00-59)
                        chars.next();
                    }
                    'i' => {
                        if let Some(count) = sequence_number {
                            result.push_str(&format!("{count:03}")); // Output index with 3 decimals
                        } else {
                            result.push('1'); // Default to 1 if count is None
                        }
                        chars.next();
                    }
                    't' => {
                        result.push_str(&reference_timestamp_secs.to_string());
                        chars.next();
                    }

                    // Literal percent sign
                    '%' => {
                        result.push('%');
                        chars.next();
                    }

                    // Unrecognized placeholder, treat as literal
                    _ => {
                        result.push('%');
                        result.push(chars.next().unwrap());
                    }
                }
            } else {
                // % at the end of string, treat as literal
                result.push('%');
            }
        } else {
            result.push(c);
        }
    }

    // Sanitize the result only if requested (for filenames, not paths)
    if sanitize {
        sanitize_filename(&result)
    } else {
        result
    }
}

const DEFAULT_FILENAME: &str = "output";

/// Sanitize a string for use as a filename
pub fn sanitize_filename(input: &str) -> String {
    // Replace characters that are invalid in filenames
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut result = String::with_capacity(input.len());

    for c in input.chars() {
        if invalid_chars.contains(&c) || c < ' ' {
            result.push('_');
        } else {
            result.push(c);
        }
    }

    // Remove leading and trailing dots and spaces
    let remove_array = ['.', ' '];
    let result = result
        .trim_start_matches(|c| remove_array.contains(&c))
        .trim_end_matches(|c| remove_array.contains(&c))
        .to_string();

    // Use a default name if the result is empty
    if result.is_empty() {
        DEFAULT_FILENAME.to_string()
    } else {
        // Truncate to reasonable length if too long
        if result.len() > 200 {
            let mut truncated = result.chars().take(200).collect::<String>();
            truncated.push_str("...");
            truncated
        } else {
            result
        }
    }
}
