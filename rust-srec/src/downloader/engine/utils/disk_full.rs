//! Disk-full signature detection for engine stderr lines.
//!
//! Used by the ffmpeg and streamlink stderr reader tasks to catch mid-stream
//! ENOSPC events before the engine exits. A matching line triggers a
//! [`crate::downloader::engine::SegmentEvent::OutputIoError`] event which the
//! download manager routes into the output-root write gate via
//! [`crate::downloader::output_root_gate::OutputRootGate::record_failure`].
//!
//! Kept in one place so the patterns stay consistent across engines and so
//! the unit tests can exercise every known ffmpeg/streamlink variation in
//! one module.

use crate::downloader::engine::traits::IoErrorKindSer;

/// Classify subprocess diagnostics only when they identify an output operation.
/// Generic permission, missing-file and timeout messages can describe network
/// inputs, so those must not degrade the recording filesystem.
pub fn output_io_error_kind(line: &str) -> Option<IoErrorKindSer> {
    if is_disk_full_line(line) {
        return Some(IoErrorKindSer::StorageFull);
    }
    let lower = line.to_ascii_lowercase();
    if ![
        "[out#",
        "[vost#",
        "[aost#",
        "opening output",
        "writing output",
        "writing trailer",
        "write header",
        "packet to the muxer",
        "av_interleaved_write_frame",
    ]
    .iter()
    .any(|context| lower.contains(context))
    {
        return None;
    }
    [
        ("read-only file system", IoErrorKindSer::ReadOnlyFilesystem),
        ("permission denied", IoErrorKindSer::PermissionDenied),
        ("access is denied", IoErrorKindSer::PermissionDenied),
        ("no such file or directory", IoErrorKindSer::NotFound),
        ("not a directory", IoErrorKindSer::NotFound),
        ("timed out", IoErrorKindSer::TimedOut),
    ]
    .into_iter()
    .find_map(|(message, kind)| lower.contains(message).then_some(kind))
}

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
/// - `"error code: -28"` — the ENOSPC errno as ffmpeg prints it; matched
///   directly as a safety net in case the human-readable string is
///   localized.
/// - `"error -28"` — not an ffmpeg form: a generic safety net for other
///   output, such as streamlink's, that renders the errno this way.
///
/// ffmpeg's `Error number -28 occurred` form matches neither and stays
/// unmatched.
pub fn is_disk_full_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    if ["no space left on device", "disk full"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return true;
    }
    // The errno must end at a word boundary, or other codes such as -280
    // would match too.
    ["error -28", "error code: -28"].iter().any(|needle| {
        lower.match_indices(needle).any(|(start, _)| {
            !lower[start + needle.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_')
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_failures_are_distinct_from_input_failures() {
        for (message, kind) in [
            ("Read-only file system", IoErrorKindSer::ReadOnlyFilesystem),
            ("Permission denied", IoErrorKindSer::PermissionDenied),
            ("No such file or directory", IoErrorKindSer::NotFound),
            ("Connection timed out", IoErrorKindSer::TimedOut),
        ] {
            assert_eq!(
                output_io_error_kind(&format!(
                    "Error opening output file /recording.flv: {message}"
                )),
                Some(kind)
            );
            assert_eq!(
                output_io_error_kind(&format!("Error opening input: {message}")),
                None
            );
            assert_eq!(output_io_error_kind(message), None);
        }
    }

    #[test]
    fn matches_ffmpeg_enospc_verbatim() {
        // FFmpeg output identifying a full output device.
        assert!(is_disk_full_line(
            "[out#0/segment @ 0x5b7ddc4105c0] Task finished with error code: -28 (No space left on device)"
        ));
        assert!(is_disk_full_line(
            "[vost#0:0/copy @ 0x5b7ddc512c80] Error submitting a packet to the muxer: No space left on device"
        ));
    }

    #[test]
    fn matches_windows_wording_any_case_and_the_bare_errno() {
        for line in [
            "Error writing trailer: Disk full",
            "NO SPACE LEFT ON DEVICE",
            "Task finished with error -28",
        ] {
            assert!(is_disk_full_line(line), "{line}");
        }
    }

    /// When the strerror text is not English, the errno in ffmpeg's
    /// `error code: -28` form is the only signal left.
    #[test]
    fn localized_ffmpeg_enospc_is_detected_by_its_errno() {
        assert!(is_disk_full_line(
            "[out#0/segment @ 0x5b7ddc4105c0] Task finished with error code: -28 (Kein Platz mehr auf dem Gerät)"
        ));
    }

    #[test]
    fn does_not_match_substring_of_different_error() {
        // "-28" should not trigger unless it follows "error " or
        // "error code: " to avoid matching unrelated numeric substrings. This
        // is a deliberate false-negative trade: better to miss an exotic
        // formatting than to trip the gate on a stray timestamp that happens
        // to contain "-28".
        for line in [
            "pts=1234 dts=-28",
            "Task finished with error -280",
            "[out#0/segment @ 0x5b7ddc4105c0] Task finished with error code: -280 (Unknown error)",
        ] {
            assert!(!is_disk_full_line(line), "{line}");
        }
    }
}
