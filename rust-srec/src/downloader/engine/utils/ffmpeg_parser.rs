//! FFmpeg output parsing utilities.
//!
//! Provides functions to parse FFmpeg's stderr progress output format.
//! These utilities are shared between FfmpegEngine and StreamlinkEngine.

use crate::downloader::engine::DownloadProgress;
use std::path::PathBuf;

/// Parse time string in HH:MM:SS.ms format to seconds.
///
/// # Arguments
/// * `time_str` - Time string in format "HH:MM:SS.ms" (e.g., "01:30:45.50")
///
/// # Returns
/// * `Some(f64)` - Total seconds if parsing succeeds
/// * `None` - If the format is invalid
///
/// # Examples
/// ```ignore
/// assert_eq!(parse_time("00:00:10.50"), Some(10.5));
/// assert_eq!(parse_time("01:30:00.00"), Some(5400.0));
/// assert_eq!(parse_time("invalid"), None);
/// ```
pub fn parse_time(time_str: &str) -> Option<f64> {
    let parts: Vec<&str> = time_str.split(':').collect();
    if parts.len() != 3 {
        return None;
    }

    let hours: f64 = parts[0].parse().ok()?;
    let minutes: f64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;

    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Parse size field from FFmpeg output (e.g., "size=    1024kB").
///
/// # Arguments
/// * `line` - FFmpeg progress output line
///
/// # Returns
/// * `Some(u64)` - Size in bytes if found and parsed
/// * `None` - If size field not found or invalid
pub fn parse_size(line: &str) -> Option<u64> {
    let size_start = line.find("size=")?;
    let size_str = &line[size_start + 5..].trim_start();
    let end = size_str.find(['k', 'K'])?;
    let size: u64 = size_str[..end].trim().parse().ok()?;
    Some(size * 1024)
}

/// Parse speed multiplier from FFmpeg output (e.g., "speed=1.00x").
///
/// # Arguments
/// * `line` - FFmpeg progress output line
///
/// # Returns
/// * `Some(f64)` - Speed multiplier if found and parsed
/// * `None` - If speed field not found or invalid
pub fn parse_speed(line: &str) -> Option<f64> {
    let speed_start = line.find("speed=")?;
    let speed_str = &line[speed_start + 6..];
    let end = speed_str.find('x')?;
    speed_str[..end].trim().parse().ok()
}

/// Parse bitrate from FFmpeg output (e.g., "bitrate=2097.2kbits/s").
///
/// # Arguments
/// * `line` - FFmpeg progress output line
///
/// # Returns
/// * `Some(u64)` - Bitrate in bytes per second if found and parsed
/// * `None` - If bitrate field not found or invalid
pub fn parse_bitrate(line: &str) -> Option<u64> {
    let bitrate_start = line.find("bitrate=")?;
    let bitrate_str = &line[bitrate_start + 8..];
    let end = bitrate_str.find("kbits/s")?;
    let bitrate: f64 = bitrate_str[..end].trim().parse().ok()?;
    Some((bitrate * 1024.0 / 8.0) as u64)
}

/// Parse time field from FFmpeg output line (e.g., "time=00:01:30.50").
///
/// # Arguments
/// * `line` - FFmpeg progress output line
///
/// # Returns
/// * `Some(f64)` - Time in seconds if found and parsed
/// * `None` - If time field not found or invalid
pub fn parse_time_field(line: &str) -> Option<f64> {
    let time_start = line.find("time=")?;
    let time_str = &line[time_start + 5..];
    let end = time_str.find(' ').unwrap_or(time_str.len());
    parse_time(&time_str[..end])
}

/// Parse FFmpeg progress output line.
///
/// FFmpeg progress format:
/// `frame=X fps=X q=X size=XkB time=HH:MM:SS.ms bitrate=Xkbits/s speed=Xx`
///
/// # Arguments
/// * `line` - FFmpeg progress output line
///
/// # Returns
/// * `Some(DownloadProgress)` - If the line contains progress info
/// * `None` - If the line is not a progress line
pub fn parse_progress(line: &str) -> Option<DownloadProgress> {
    // FFmpeg typically emits progress lines like:
    // `frame=... size=... time=... bitrate=... speed=...`
    //
    // When segmenting, some builds/loglevels may omit `size=` while still
    // reporting `time=`. Require `time=` plus at least one other known progress
    // marker to avoid parsing unrelated lines.
    if !line.contains("time=") || !(line.contains("frame=") || line.contains("size=")) {
        return None;
    }

    let progress = DownloadProgress {
        bytes_downloaded: parse_size(line).unwrap_or(0),
        media_duration_secs: parse_time_field(line).unwrap_or(0.0),
        duration_secs: parse_time_field(line).unwrap_or(0.0),
        playback_ratio: parse_speed(line).unwrap_or(0.0),
        speed_bytes_per_sec: parse_bitrate(line).unwrap_or(0),
        ..Default::default()
    };

    Some(progress)
}

/// Check if a line indicates a new segment is being written.
///
/// # Arguments
/// * `line` - FFmpeg output line
///
/// # Returns
/// * `true` - If the line indicates a segment is being opened for writing
/// * `false` - Otherwise
pub fn is_segment_start(line: &str) -> bool {
    line.contains("Opening") && line.contains("for writing")
}

/// Parse the output path from an FFmpeg "Opening '...'" message.
///
/// Example:
/// - `Opening 'output_001.ts' for writing`
/// - `[segment] Opening 'C:/out/%Y%m%d-%H%M%S.mp4' for writing`
pub fn parse_opened_path(line: &str) -> Option<PathBuf> {
    // Common FFmpeg format: Opening '...path...' for writing
    if let Some(start) = line.find("Opening '") {
        let rest = &line[start + "Opening '".len()..];
        let end = rest.find('\'')?;
        return Some(PathBuf::from(rest[..end].to_string()));
    }

    // Fallback: Opening "..." for writing
    if let Some(start) = line.find("Opening \"") {
        let rest = &line[start + "Opening \"".len()..];
        let end = rest.find('"')?;
        return Some(PathBuf::from(rest[..end].to_string()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_valid() {
        assert_eq!(parse_time("00:00:10.50"), Some(10.5));
        assert_eq!(parse_time("01:30:00.00"), Some(5400.0));
        assert_eq!(parse_time("00:01:30.50"), Some(90.5));
        assert_eq!(parse_time("10:00:00.00"), Some(36000.0));
    }

    #[test]
    fn test_parse_time_invalid() {
        assert_eq!(parse_time("invalid"), None);
        assert_eq!(parse_time("00:00"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("00:00:00:00"), None);
    }

    #[test]
    fn test_parse_size() {
        let line = "frame=  100 fps=25 q=-1.0 size=    1024kB time=00:00:04.00";
        assert_eq!(parse_size(line), Some(1024 * 1024));

        let line2 = "size=512KB time=00:00:10.00";
        assert_eq!(parse_size(line2), Some(512 * 1024));

        assert_eq!(parse_size("no size here"), None);
    }

    #[test]
    fn test_parse_speed() {
        let line = "size=1024kB time=00:00:04.00 bitrate=2097.2kbits/s speed=1.00x";
        assert_eq!(parse_speed(line), Some(1.0));

        let line2 = "speed=2.50x";
        assert_eq!(parse_speed(line2), Some(2.5));

        assert_eq!(parse_speed("no speed here"), None);
    }

    #[test]
    fn test_parse_bitrate() {
        let line = "bitrate=2097.2kbits/s speed=1.00x";
        let bitrate = parse_bitrate(line);
        assert!(bitrate.is_some());
        // 2097.2 kbits/s = 2097.2 * 1024 / 8 bytes/s
        assert_eq!(bitrate.unwrap(), (2097.2 * 1024.0 / 8.0) as u64);

        assert_eq!(parse_bitrate("no bitrate here"), None);
    }

    #[test]
    fn test_parse_progress_complete() {
        let line = "frame=  100 fps=25 q=-1.0 size=    1024kB time=00:00:04.00 bitrate=2097.2kbits/s speed=1.00x";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 1024 * 1024);
        assert_eq!(p.duration_secs, 4.0);
        assert_eq!(p.media_duration_secs, 4.0);
        assert_eq!(p.playback_ratio, 1.0);
    }

    #[test]
    fn test_parse_progress_partial() {
        let line = "size=512kB time=00:00:10.00";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 512 * 1024);
        assert_eq!(p.duration_secs, 10.0);
        assert_eq!(p.playback_ratio, 0.0); // No speed field
    }

    #[test]
    fn test_parse_progress_without_size_field() {
        // Some FFmpeg outputs can omit `size=` while still reporting `time=`.
        let line = "frame=  100 fps=25 q=-1.0 time=00:00:04.00 bitrate=2097.2kbits/s speed=1.00x";
        let progress = parse_progress(line);

        assert!(progress.is_some());
        let p = progress.unwrap();
        assert_eq!(p.bytes_downloaded, 0);
        assert_eq!(p.duration_secs, 4.0);
        assert_eq!(p.media_duration_secs, 4.0);
        assert_eq!(p.playback_ratio, 1.0);
    }

    #[test]
    fn test_parse_progress_time_only_is_not_progress() {
        assert!(parse_progress("time=00:00:10.00").is_none());
    }

    #[test]
    fn test_parse_progress_no_size() {
        let line = "frame=100 fps=25 q=-1.0";
        assert!(parse_progress(line).is_none());
    }

    #[test]
    fn test_is_segment_start() {
        assert!(is_segment_start("Opening 'output_001.ts' for writing"));
        assert!(is_segment_start("[segment] Opening 'file.mp4' for writing"));
        assert!(!is_segment_start("frame=100 fps=25"));
        assert!(!is_segment_start("Opening file"));
        assert!(!is_segment_start("for writing"));
    }

    #[test]
    fn test_parse_opened_path() {
        assert_eq!(
            parse_opened_path("Opening 'output_001.ts' for writing")
                .unwrap()
                .to_string_lossy(),
            "output_001.ts"
        );
        assert_eq!(
            parse_opened_path("[segment] Opening \"C:/out/file.mp4\" for writing")
                .unwrap()
                .to_string_lossy(),
            "C:/out/file.mp4"
        );
        assert!(parse_opened_path("frame=100 fps=25").is_none());
    }
}
