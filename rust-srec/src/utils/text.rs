//! Small text utilities. Use these instead of hand-rolling truncation
//! per call site so behavior stays consistent and so we don't grow yet
//! another local helper variant.
//!
//! Two flavors are exposed because callers genuinely care about
//! different units:
//!
//! - [`truncate_chars`] for display-width budgets (UI strings, log
//!   diagnostics, snapshot messages).
//! - [`truncate_bytes`] for wire-size budgets (notification payloads,
//!   DB columns) where the cap is in bytes and naive slicing would
//!   panic mid-codepoint on multibyte UTF-8.
//!
//! Both append a single `…` (3 bytes) when clipping; if the input fits,
//! it is returned unchanged.

/// Truncate `s` to at most `max` characters. Appends `…` when clipped.
/// Returns `s.to_string()` when it already fits, or an empty string when
/// `max == 0`. Single-pass over `s.chars()` so cost is `O(min(len, max))`
/// rather than `O(len)`.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let mut iter = s.chars();
    let head: String = iter.by_ref().take(max).collect();
    if iter.next().is_some() {
        let mut out = String::with_capacity(head.len() + '…'.len_utf8());
        out.push_str(&head);
        out.push('…');
        out
    } else {
        head
    }
}

/// UTF-8-safe truncation by byte budget. Finds the last `char` boundary
/// at or before `max_bytes` and appends `…` when clipped. Use this for
/// caps that reflect a wire-size budget (notification body, DB column).
pub fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + '…'.len_utf8());
    out.push_str(&s[..end]);
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- truncate_chars ----

    #[test]
    fn chars_passthrough_when_input_fits() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        assert_eq!(truncate_chars("hello", 5), "hello");
    }

    #[test]
    fn chars_clips_with_ellipsis_when_too_long() {
        let out = truncate_chars("abcdef", 3);
        assert_eq!(out, "abc…");
        assert_eq!(out.chars().count(), 4);
    }

    #[test]
    fn chars_zero_max_returns_empty() {
        assert_eq!(truncate_chars("anything", 0), "");
    }

    #[test]
    fn chars_handles_multibyte_correctly() {
        // 3-byte CJK char × 5 = 15 bytes total, 5 chars
        let out = truncate_chars("字字字字字", 3);
        assert_eq!(out, "字字字…");
        assert_eq!(out.chars().count(), 4);
    }

    #[test]
    fn chars_empty_input_passthrough() {
        assert_eq!(truncate_chars("", 10), "");
        assert_eq!(truncate_chars("", 0), "");
    }

    // ---- truncate_bytes ----

    #[test]
    fn bytes_passthrough_when_input_fits() {
        assert_eq!(truncate_bytes("hello", 10), "hello");
        assert_eq!(truncate_bytes("hello", 5), "hello");
    }

    #[test]
    fn bytes_clips_at_char_boundary() {
        // ASCII: byte budget == char count
        let out = truncate_bytes("abcdefghij", 4);
        assert_eq!(out, "abcd…");
    }

    #[test]
    fn bytes_never_panics_on_codepoint_split() {
        // 3-byte CJK char × 10 = 30 bytes. Cap at 7 bytes — naive slicing
        // would panic mid-codepoint; we round down to byte 6 (= 2 chars).
        let s = "字".repeat(10);
        let out = truncate_bytes(&s, 7);
        assert_eq!(out, "字字…");
    }

    #[test]
    fn bytes_empty_input_passthrough() {
        assert_eq!(truncate_bytes("", 10), "");
        assert_eq!(truncate_bytes("", 0), "");
    }
}
