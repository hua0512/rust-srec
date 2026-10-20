//! Filename sanitization utilities for cross-platform compatibility.
//!
//! This module provides functions to sanitize filenames by removing or replacing
//! characters that are invalid on Windows, Linux, or macOS, while preserving
//! valid Unicode characters like Chinese, Japanese, and Korean text.

/// Characters that are invalid in Windows filenames
const WINDOWS_INVALID_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows reserved filenames (case-insensitive)
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Sanitize a string for use in filenames across all platforms.
///
/// This function:
/// 1. Removes control characters (0x00-0x1F)
/// 2. Replaces Windows invalid characters with underscores
/// 3. Collapses consecutive underscores into one
/// 4. Trims leading/trailing spaces and dots
/// 5. Handles Windows reserved names
/// 6. Returns "unnamed" if result would be empty
///
/// # Arguments
///
/// * `input` - The string to sanitize
///
/// # Returns
///
/// A sanitized string safe for use as a filename on all platforms.
///
/// # Examples
///
/// ```
/// use rust_srec::utils::filename::sanitize_filename;
///
/// assert_eq!(sanitize_filename("hello?world"), "hello_world");
/// assert_eq!(sanitize_filename("观看一只青蛙?"), "观看一只青蛙_");
/// assert_eq!(sanitize_filename(""), "unnamed");
/// assert_eq!(sanitize_filename("CON"), "_CON");
/// ```
pub fn sanitize_filename(input: &str) -> String {
    if input.is_empty() {
        return "unnamed".to_string();
    }

    let mut result = String::with_capacity(input.len());
    let mut last_was_replacement = false;

    for c in input.chars() {
        if c.is_control() || WINDOWS_INVALID_CHARS.contains(&c) {
            // Replace invalid char with underscore, but collapse consecutive
            if !last_was_replacement {
                result.push('_');
                last_was_replacement = true;
            }
        } else {
            result.push(c);
            last_was_replacement = false;
        }
    }

    // Trim leading/trailing spaces and dots (Windows restriction)
    let trimmed = result.trim_matches(|c| c == ' ' || c == '.');

    // Handle empty result after trimming
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }

    // Check for Windows reserved names
    let upper = trimmed.to_uppercase();
    for reserved in WINDOWS_RESERVED_NAMES {
        if upper == *reserved || upper.starts_with(&format!("{}.", reserved)) {
            return format!("_{}", trimmed);
        }
    }

    trimmed.to_string()
}

/// Sanitize metadata inserted into a template that will undergo one later
/// percent expansion. Literal percent signs must survive that expansion.
pub fn sanitize_filename_for_template(input: &str) -> String {
    sanitize_filename(input).replace('%', "%%")
}

/// Expand placeholders in a path template.
///
/// This function handles both curly-brace placeholders (e.g., `{streamer}`) and
/// time-based percent placeholders (e.g., `%Y`, `%m`, `%d`).
///
/// # Supported Placeholders
///
/// ## Streamer/Session Placeholders
/// - `{streamer}` - Streamer name (sanitized for filesystem), falls back to streamer_id
/// - `{title}` - Session/stream title (sanitized for filesystem), falls back to empty
/// - `{streamer_id}` - Raw streamer ID
/// - `{session_id}` - Raw session ID
///
/// ## Time Placeholders (expanded to current local time)
/// - `%Y` - Year (4 digits)
/// - `%m` - Month (01-12)
/// - `%d` - Day (01-31)
/// - `%H` - Hour (00-23)
/// - `%M` - Minute (00-59)
/// - `%S` - Second (00-59)
/// - `%t` - Unix timestamp
/// - `%%` - Literal percent sign
///
/// # Arguments
///
/// * `template` - The path template to expand
/// * `streamer_id` - Raw streamer ID
/// * `session_id` - Raw session ID
/// * `streamer_name` - Optional human-readable streamer name
/// * `session_title` - Optional session/stream title
///
/// # Returns
///
/// The expanded path with all placeholders replaced.
///
/// # Examples
///
/// ```
/// use rust_srec::utils::filename::expand_placeholders;
///
/// let path = expand_placeholders(
///     "remote:/{streamer}/{title}",
///     "abc123",
///     "session-456",
///     Some("StreamerName"),
///     Some("Live Stream?"),
///     None, // platform
/// );
/// assert!(path.starts_with("remote:/StreamerName/Live Stream_"));
/// ```
pub fn expand_placeholders(
    template: &str,
    streamer_id: &str,
    session_id: &str,
    streamer_name: Option<&str>,
    session_title: Option<&str>,
    platform: Option<&str>,
) -> String {
    expand_placeholders_at(
        template,
        streamer_id,
        session_id,
        streamer_name,
        session_title,
        platform,
        None,
    )
}

/// Expand placeholders in a path template using a specific reference timestamp.
///
/// Same as `expand_placeholders`, but uses the provided timestamp (Unix epoch milliseconds)
/// instead of the current time for time-based placeholders. This is useful for retries
/// where the original timestamp should be preserved.
///
/// # Arguments
///
/// * `template` - The path template to expand
/// * `streamer_id` - Raw streamer ID
/// * `session_id` - Raw session ID
/// * `streamer_name` - Optional human-readable streamer name
/// * `session_title` - Optional session/stream title
/// * `platform` - Optional platform name
/// * `reference_timestamp_ms` - Optional reference timestamp in Unix epoch milliseconds.
///   If None, uses the current time.
pub fn expand_placeholders_at(
    template: &str,
    streamer_id: &str,
    session_id: &str,
    streamer_name: Option<&str>,
    session_title: Option<&str>,
    platform: Option<&str>,
    reference_timestamp_ms: Option<i64>,
) -> String {
    // Expand only the template's time tokens, before inserting literal metadata.
    let expanded = pipeline_common::expand_path_template_at(template, reference_timestamp_ms);
    let streamer_display = streamer_name
        .map(sanitize_filename)
        .unwrap_or_else(|| streamer_id.to_string());
    let title_display = session_title.map(sanitize_filename).unwrap_or_default();
    let platform_display = platform.unwrap_or_default();

    expanded
        .replace("{streamer}", &streamer_display)
        .replace("{title}", &title_display)
        .replace("{streamer_id}", streamer_id)
        .replace("{session_id}", session_id)
        .replace("{platform}", platform_display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_percent_tokens_remain_literal_in_paths_and_recording_templates() {
        let name = "频道%Y%n%%";
        let title = "Top 5%d %i";
        let path = expand_placeholders_at(
            "root/%Y/{streamer}/{title}",
            "id",
            "session",
            Some(name),
            Some(title),
            None,
            Some(1_700_000_000_000),
        );
        assert_eq!(path, format!("root/2023/{name}/{title}"));
        let template = format!(
            "{}-{}-%i",
            sanitize_filename_for_template(name),
            sanitize_filename_for_template(title)
        );
        assert_eq!(
            pipeline_common::expand_filename_template(&template, Some(7)),
            format!("{name}-{title}-007")
        );
        assert_eq!(
            pipeline_common::expand_path_template(&format!(
                "root/{}",
                sanitize_filename_for_template(name)
            )),
            format!("root/{name}")
        );
    }

    /// Platform-supplied names and titles become path components, so no value
    /// may add a directory level or climb out of the output folder.
    #[test]
    fn sanitized_names_are_a_single_path_component() {
        for (input, expected) in [
            ("../../etc/passwd", "_.._etc_passwd"),
            (r"..\..\Windows", "_.._Windows"),
            ("/etc/cron.d/x", "_etc_cron.d_x"),
            (r"C:\Users", "C_Users"),
            ("a/./b", "a_._b"),
            ("..", "unnamed"),
            (".", "unnamed"),
        ] {
            let sanitized = sanitize_filename(input);
            assert_eq!(sanitized, expected, "{input}");
            let mut components = std::path::Path::new(&sanitized).components();
            assert!(
                matches!(
                    (components.next(), components.next()),
                    (Some(std::path::Component::Normal(_)), None)
                ),
                "{input}"
            );
        }
    }

    #[test]
    fn test_empty_string() {
        assert_eq!(sanitize_filename(""), "unnamed");
    }

    #[test]
    fn test_reserved_name_with_extension() {
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("nul.exe"), "_nul.exe");
    }

    #[test]
    fn test_leading_trailing_spaces_and_dots() {
        assert_eq!(sanitize_filename("  hello  "), "hello");
        assert_eq!(sanitize_filename("...hello..."), "hello");
        assert_eq!(sanitize_filename(" . hello . "), "hello");
    }

    #[test]
    fn test_consecutive_invalid_chars_collapsed() {
        assert_eq!(sanitize_filename("hello???world"), "hello_world");
        assert_eq!(sanitize_filename("a<>:\"b"), "a_b");
    }

    #[test]
    fn test_control_characters() {
        assert_eq!(sanitize_filename("hello\x00world"), "hello_world");
        assert_eq!(sanitize_filename("test\x1Ffile"), "test_file");
    }

    #[test]
    fn test_expand_placeholders_streamer_fallback() {
        // When streamer_name is None, should fall back to streamer_id
        let result = expand_placeholders(
            "remote:/{streamer}/videos",
            "streamer-123",
            "session-456",
            None,
            None,
            None,
        );
        assert_eq!(result, "remote:/streamer-123/videos");
    }

    #[test]
    fn test_expand_placeholders_sanitizes_special_chars() {
        let result = expand_placeholders(
            "remote:/{streamer}/{title}",
            "streamer-123",
            "session-456",
            Some("Streamer<Name>"),
            Some("Title:With:Colons"),
            None,
        );
        // Characters are sanitized
        assert_eq!(result, "remote:/Streamer_Name_/Title_With_Colons");
    }
}
