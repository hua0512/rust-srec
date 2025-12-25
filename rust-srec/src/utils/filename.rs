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
    // First, expand curly-brace placeholders
    let streamer_display = streamer_name
        .map(sanitize_filename)
        .unwrap_or_else(|| streamer_id.to_string());
    let title_display = session_title.map(sanitize_filename).unwrap_or_default();
    let platform_display = platform.unwrap_or_default();

    let result = template
        .replace("{streamer}", &streamer_display)
        .replace("{title}", &title_display)
        .replace("{streamer_id}", streamer_id)
        .replace("{session_id}", session_id)
        .replace("{platform}", platform_display);

    // Then expand time-based placeholders using pipeline_common's expand_path_template
    pipeline_common::expand_path_template(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_string() {
        assert_eq!(sanitize_filename(""), "unnamed");
    }

    #[test]
    fn test_only_invalid_characters() {
        assert_eq!(sanitize_filename("???"), "_");
        assert_eq!(sanitize_filename("<>:"), "_");
    }

    #[test]
    fn test_windows_reserved_names() {
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("con"), "_con");
        assert_eq!(sanitize_filename("PRN"), "_PRN");
        assert_eq!(sanitize_filename("AUX"), "_AUX");
        assert_eq!(sanitize_filename("NUL"), "_NUL");
        assert_eq!(sanitize_filename("COM1"), "_COM1");
        assert_eq!(sanitize_filename("LPT1"), "_LPT1");
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
    fn test_chinese_characters() {
        assert_eq!(sanitize_filename("观看一只青蛙"), "观看一只青蛙");
        assert_eq!(sanitize_filename("观看一只青蛙?"), "观看一只青蛙_");
    }

    #[test]
    fn test_japanese_characters() {
        assert_eq!(sanitize_filename("こんにちは"), "こんにちは");
    }

    #[test]
    fn test_korean_characters() {
        assert_eq!(sanitize_filename("안녕하세요"), "안녕하세요");
    }

    #[test]
    fn test_mixed_valid_and_invalid() {
        assert_eq!(sanitize_filename("hello?world"), "hello_world");
        assert_eq!(sanitize_filename("file<name>test"), "file_name_test");
        assert_eq!(sanitize_filename("a:b:c"), "a_b_c");
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
    fn test_idempotency() {
        let inputs = vec![
            "hello?world",
            "观看一只青蛙?",
            "CON",
            "  test  ",
            "...dots...",
        ];
        for input in inputs {
            let once = sanitize_filename(input);
            let twice = sanitize_filename(&once);
            assert_eq!(once, twice, "Idempotency failed for input: {}", input);
        }
    }

    #[test]
    fn test_expand_placeholders_streamer() {
        let result = expand_placeholders(
            "remote:/{streamer}/videos",
            "streamer-123",
            "session-456",
            Some("TestStreamer"),
            None,
            None,
        );
        assert_eq!(result, "remote:/TestStreamer/videos");
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
    fn test_expand_placeholders_title() {
        let result = expand_placeholders(
            "remote:/{streamer}/{title}",
            "streamer-123",
            "session-456",
            Some("Streamer"),
            Some("Live Stream?"),
            None,
        );
        // The ? character is sanitized to _
        assert_eq!(result, "remote:/Streamer/Live Stream_");
    }

    #[test]
    fn test_expand_placeholders_ids() {
        let result = expand_placeholders(
            "remote:/{streamer_id}/{session_id}",
            "streamer-123",
            "session-456",
            Some("Streamer"),
            None,
            None,
        );
        assert_eq!(result, "remote:/streamer-123/session-456");
    }

    #[test]
    fn test_expand_placeholders_no_placeholders() {
        let result = expand_placeholders(
            "remote:/fixed/path",
            "streamer-123",
            "session-456",
            Some("Streamer"),
            None,
            None,
        );
        assert_eq!(result, "remote:/fixed/path");
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

    #[test]
    fn test_expand_placeholders_platform() {
        let result = expand_placeholders(
            "remote:/{platform}/{streamer}/videos",
            "streamer-123",
            "session-456",
            Some("TestStreamer"),
            None,
            Some("Twitch"),
        );
        assert_eq!(result, "remote:/Twitch/TestStreamer/videos");
    }

    #[test]
    fn test_expand_placeholders_platform_none() {
        // When platform is None, should expand to empty string
        let result = expand_placeholders(
            "remote:/{platform}/{streamer}/videos",
            "streamer-123",
            "session-456",
            Some("TestStreamer"),
            None,
            None,
        );
        assert_eq!(result, "remote://TestStreamer/videos");
    }
}
