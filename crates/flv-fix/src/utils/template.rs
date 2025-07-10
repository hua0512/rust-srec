/// Expand filename template with placeholders similar to FFmpeg
pub fn expand_filename_template(template: &str, file_count: Option<u32>) -> String {
    use chrono::Local;

    let now = Local::now();
    let mut result = String::with_capacity(template.len() * 2);
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(&next_char) = chars.peek() {
                match next_char {
                    // Date and time placeholders
                    'Y' => {
                        result.push_str(&now.format("%Y").to_string()); // Year (YYYY)
                        chars.next();
                    }
                    'm' => {
                        result.push_str(&now.format("%m").to_string()); // Month (01-12)
                        chars.next();
                    }
                    'd' => {
                        result.push_str(&now.format("%d").to_string()); // Day (01-31)
                        chars.next();
                    }
                    'H' => {
                        result.push_str(&now.format("%H").to_string()); // Hour (00-23)
                        chars.next();
                    }
                    'M' => {
                        result.push_str(&now.format("%M").to_string()); // Minute (00-59)
                        chars.next();
                    }
                    'S' => {
                        result.push_str(&now.format("%S").to_string()); // Second (00-59)
                        chars.next();
                    }
                    'i' => {
                        if let Some(count) = file_count {
                            result.push_str(&format!("{count:03}")); // Output index with 3 decimals
                        } else {
                            result.push('1'); // Default to 1 if count is None
                        }
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

    // Sanitize the entire filename to ensure it's valid
    sanitize_filename(&result)
}

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
        "file".to_string()
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
