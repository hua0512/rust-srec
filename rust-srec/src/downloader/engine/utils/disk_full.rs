//! Disk-full signature detection for engine stderr lines.
//!
//! Used by the ffmpeg and streamlink stderr reader tasks to catch mid-stream
//! ENOSPC events before the engine exits. A matching line triggers a
//! [`crate::downloader::engine::SegmentEvent::DiskFull`] event which the
//! download manager routes into the output-root write gate via
//! [`crate::downloader::output_root_gate::OutputRootGate::record_failure`].
//!
//! Kept in one place so the patterns stay consistent across engines and so
//! the unit tests can exercise every known ffmpeg/streamlink variation in
//! one module.

/// Return `true` if the given stderr line looks like a disk-full / ENOSPC
/// signal from ffmpeg, streamlink, or their underlying OS.
///
/// The matching is deliberately simple (case-insensitive substring) because
/// the exact wording varies by ffmpeg version and libav build, but the core
/// phrases we care about are stable:
///
/// - `"No space left on device"` — the standard `ENOSPC` strerror rendering
///   on Linux; ffmpeg reports this verbatim when a muxer write fails.
/// - `"Disk full"` — Windows equivalent; sometimes rendered this way on
///   macOS too.
/// - `"Error submitting a packet to the muxer"` combined with `"-28"` — the
///   older "code -28" errno style; we match the errno directly as a safety
///   net in case the human-readable string is localized.
pub fn is_disk_full_line(line: &str) -> bool {
    // Avoid heavy per-line allocation: check substrings directly.
    // `contains` is O(n*m) but n (line length) and m (needle length) are
    // both tiny and this runs once per stderr line, which is typically
    // 1-10 lines/sec during recording — not hot.
    let needles = [
        "no space left on device",
        "disk full",
        "error -28", // errno -28 = ENOSPC
    ];
    let lower = line.to_ascii_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ffmpeg_enospc_verbatim() {
        // Exact string ffmpeg prints in the 508 log.
        assert!(is_disk_full_line(
            "[out#0/segment @ 0x5b7ddc4105c0] Task finished with error code: -28 (No space left on device)"
        ));
        assert!(is_disk_full_line(
            "[vost#0:0/copy @ 0x5b7ddc512c80] Error submitting a packet to the muxer: No space left on device"
        ));
    }

    #[test]
    fn matches_case_insensitively() {
        assert!(is_disk_full_line("NO SPACE LEFT ON DEVICE"));
        assert!(is_disk_full_line("No Space Left On Device"));
    }

    #[test]
    fn matches_errno_only() {
        // Localized builds may render the human string in another language
        // but the errno stays numeric.
        assert!(is_disk_full_line("Task finished with error -28"));
    }

    #[test]
    fn matches_disk_full_variant() {
        assert!(is_disk_full_line("Error writing trailer: Disk full"));
    }

    #[test]
    fn does_not_match_unrelated_errors() {
        assert!(!is_disk_full_line(
            "Error during demuxing: Input/output error"
        ));
        assert!(!is_disk_full_line("Connection refused"));
        assert!(!is_disk_full_line("frame= 1234 fps= 30"));
        assert!(!is_disk_full_line(""));
    }

    #[test]
    fn does_not_match_substring_of_different_error() {
        // "-28" should not trigger unless preceded by "error " to avoid
        // matching unrelated numeric substrings. This is a deliberate
        // false-negative trade: better to miss an exotic formatting than
        // to trip the gate on a stray timestamp that happens to contain
        // "-28".
        assert!(!is_disk_full_line("pts=1234 dts=-28"));
    }
}
