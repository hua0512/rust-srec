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
}
