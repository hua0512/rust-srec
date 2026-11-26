//! Filename sanitization and path template expansion utilities.
//!
//! Removes or replaces characters that are invalid on Windows, Linux, or macOS
//! while preserving valid Unicode text, and expands recording/pipeline path
//! templates cleanly without double-expanding percent tokens inside metadata.

/// Characters that are invalid in Windows filenames.
const WINDOWS_INVALID_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows reserved filenames (case-insensitive).
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
/// # Examples
///
/// ```
/// use srec_core::domain::filename::sanitize_filename;
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
            if !last_was_replacement {
                result.push('_');
                last_was_replacement = true;
            }
        } else {
            result.push(c);
            last_was_replacement = false;
        }
    }

    let trimmed = result.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        return "unnamed".to_string();
    }

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

/// Longest `{title}` value, in bytes. A file name is limited to 255 bytes, and
/// the rest of a recording template (streamer, date, time, sequence) must still
/// fit so that each segment keeps its own name.
const MAX_TEMPLATE_TITLE_BYTES: usize = 150;

/// [`sanitize_filename_for_template`] for a stream title, shortened on a
/// character boundary to `MAX_TEMPLATE_TITLE_BYTES` before escaping, so a
/// `%%` pair is never split.
pub fn sanitize_title_for_template(title: &str) -> String {
    let title = sanitize_filename(title);
    let cut = title.floor_char_boundary(MAX_TEMPLATE_TITLE_BYTES);
    let shortened = title[..cut].trim_end_matches([' ', '.']);
    if shortened.is_empty() {
        return "unnamed".to_string();
    }
    shortened.replace('%', "%%")
}

/// Expand placeholders in a path template.
///
/// This function handles both curly-brace placeholders (e.g., `{streamer}`) and
/// time-based percent placeholders (e.g., `%Y`, `%m`, `%d`).
///
/// # Examples
///
/// ```
/// use srec_core::domain::filename::expand_placeholders;
///
/// let path = expand_placeholders(
///     "remote:/{streamer}/{title}",
///     "abc123",
///     "session-456",
///     Some("StreamerName"),
///     Some("Live Stream?"),
///     None,
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
/// Same as [`expand_placeholders`], but uses the provided timestamp (Unix epoch
/// milliseconds) instead of the current time for time-based placeholders.
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
    fn placeholders_fall_back_to_the_streamer_id_and_keep_metadata_percent_tokens_literal() {
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

        // Without a name, `{streamer}` falls back to the streamer id.
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
    fn a_long_title_leaves_the_time_parts_of_a_recording_name() {
        let template = format!(
            "{}-%Y%m%d-%H%M%S",
            sanitize_title_for_template(&"直播%".repeat(100))
        );
        let name = pipeline_common::expand_filename_template(&template, None);
        assert!(!name.ends_with("..."), "{name}");
        let time = &name.as_bytes()[name.len() - 16..];
        assert!(
            time.iter().enumerate().all(|(i, b)| if i == 0 || i == 9 {
                *b == b'-'
            } else {
                b.is_ascii_digit()
            }),
            "{name}"
        );
    }

    #[test]
    fn sanitize_filename_avoids_reserved_names_edge_dots_and_control_characters() {
        for (input, sanitized) in [
            ("CON.txt", "_CON.txt"),
            ("nul.exe", "_nul.exe"),
            ("  hello  ", "hello"),
            ("...hello...", "hello"),
            (" . hello . ", "hello"),
            ("hello\x00world", "hello_world"),
            ("test\x1Ffile", "test_file"),
        ] {
            assert_eq!(sanitize_filename(input), sanitized, "{input:?}");
        }
    }
}
