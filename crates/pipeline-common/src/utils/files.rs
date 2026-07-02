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
        let utc = time::OffsetDateTime::from_unix_timestamp(ts_ms / 1000)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
        // Render the reference in the local offset that was in effect AT that
        // instant — not the offset at expansion time — so it matches what the
        // now_local() branch below produced when the reference instant was
        // "now" (e.g. a recording filename's %Y%m%d expanded at file open),
        // and so re-expanding the same reference later (job retries, DST
        // transitions in between) yields the same rendering.
        time::UtcOffset::local_offset_at(utc)
            .map(|offset| utc.to_offset(offset))
            .unwrap_or(utc)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn reference_timestamp_uses_offset_at_instant_not_at_expansion() {
        // cargo-nextest runs each test in its own process, so mutating TZ
        // here cannot race sibling tests. glibc initializes its timezone
        // state lazily on the FIRST localtime_r call and never re-reads the
        // env var, so tzset() forces a re-read in case another test in this
        // binary (e.g. one calling now_local) resolved the ambient zone
        // first — that ordering is unpredictable under plain `cargo test`,
        // which shares one process across test threads.
        // The POSIX TZ string carries its own DST rules (2nd Sunday of March
        // / 1st Sunday of November), so no tzdata files are needed.
        // POSIX tzset; not bound by the libc crate for unix targets.
        unsafe extern "C" {
            fn tzset();
        }
        unsafe {
            std::env::set_var("TZ", "EST5EDT,M3.2.0,M11.1.0");
            tzset();
        }

        // 2026-03-08T04:30:00Z is 23:30 EST (-05:00) on 2026-03-07, before
        // that day's 07:00Z spring-forward transition. Rendering must use
        // the offset at that instant regardless of the offset at test time.
        assert_eq!(
            expand_path_template_at("%Y%m%d-%H%M", Some(1_772_944_200_000)),
            "20260307-2330"
        );

        // 2026-03-08T12:00:00Z is 08:00 EDT (-04:00), after the transition.
        assert_eq!(
            expand_path_template_at("%Y%m%d-%H%M", Some(1_772_971_200_000)),
            "20260308-0800"
        );
    }

    #[test]
    fn referenced_now_matches_unreferenced_now_calendar_minute() {
        let template = "%Y%m%d%H%M";
        let before_ms = current_unix_ms();
        let expanded_now = expand_path_template(template);
        let after_ms = current_unix_ms();

        let before_ref = expand_path_template_at(template, Some(before_ms));
        let after_ref = expand_path_template_at(template, Some(after_ms));
        assert!(
            expanded_now == before_ref || expanded_now == after_ref,
            "unreferenced={expanded_now}, before_ref={before_ref}, after_ref={after_ref}"
        );
    }

    fn current_unix_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }
}
