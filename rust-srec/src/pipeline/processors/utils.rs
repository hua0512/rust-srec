//! Utility functions for processors.

use crate::pipeline::job_queue::{JobLogEntry, LogLevel};
use crate::pipeline::{JobProgressSnapshot, ProgressKind, ProgressReporter};
use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use super::traits::ProcessorContext;
use process_utils::NoWindowExt;

const LOG_CHANNEL_CAPACITY: usize = 1024;
const MAX_LOG_ENTRIES: usize = 2000;

/// Video file extensions that support processing.
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "webm", "mov", "flv", "avi", "wmv", "m4v", "ts", "mts", "m2ts", "3gp", "ogv",
    "m4s",
];

/// Audio file extensions.
pub const AUDIO_EXTENSIONS: &[&str] = &["mp3", "aac", "m4a", "ogg", "opus", "flac", "wav"];

/// Image file extensions that should be passed through.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tiff", "tif", "ico", "avif",
];

pub fn parse_config_or_default<T: DeserializeOwned + Default>(
    raw: Option<&str>,
    ctx: &ProcessorContext,
    processor: &'static str,
    logs: Option<&mut Vec<JobLogEntry>>,
) -> T {
    let Some(raw) = raw else {
        return T::default();
    };

    match serde_json::from_str(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            warn!(
                job_id = %ctx.job_id,
                processor,
                raw_len = raw.len(),
                error = %error,
                "Failed to parse processor config; using defaults"
            );

            let msg = format!(
                "Failed to parse {} config; using defaults: {}",
                processor, error
            );
            if let Some(logs) = logs {
                logs.push(JobLogEntry::warn(msg));
            } else {
                ctx.warn(msg);
            }

            T::default()
        }
    }
}

/// Get the lowercase extension from a path.
pub fn get_extension(path: &str) -> Option<String> {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_lowercase())
}

/// Check if the extension is a video format.
pub fn is_video(ext: &str) -> bool {
    VIDEO_EXTENSIONS.contains(&ext)
}

/// Check if the extension is an audio format.
pub fn is_audio(ext: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&ext)
}

/// Check if the extension is an image format.
pub fn is_image(ext: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&ext)
}

/// Check if the extension is a media format (video or audio).
pub fn is_media(ext: &str) -> bool {
    is_video(ext) || is_audio(ext)
}

/// Output from a command execution including captured logs.
pub struct CommandOutput {
    pub status: std::process::ExitStatus,
    pub duration: f64,
    pub logs: Vec<JobLogEntry>,
}

fn push_log_with_cap(
    logs: &mut VecDeque<JobLogEntry>,
    entry: JobLogEntry,
    cap: usize,
    truncated_count: &mut usize,
) {
    if logs.len() < cap {
        logs.push_back(entry);
        return;
    }

    let is_important = matches!(entry.level, LogLevel::Warn | LogLevel::Error);
    if is_important
        && let Some(index) = logs
            .iter()
            .position(|e| !matches!(e.level, LogLevel::Warn | LogLevel::Error))
    {
        let _ = logs.remove(index);
        *truncated_count += 1;
        logs.push_back(entry);
        return;
    }

    logs.pop_front();
    *truncated_count += 1;
    logs.push_back(entry);
}

/// Helper function to create a log entry.
pub fn create_log_entry(level: LogLevel, message: impl Into<String>) -> JobLogEntry {
    JobLogEntry::new(level, message)
}

/// Run a command and capture its output (stdout/stderr) as logs.
/// This helper handles spawning the process, reading output streams asynchronously,
/// and collecting them into a structured log format.
pub async fn run_command_with_logs(
    command: &mut Command,
    log_sink: Option<super::traits::JobLogSink>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    command.no_window();

    // Ensure pipes are set up
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));

    // Handle stdout
    let stdout_handle = if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!("stdout: {}", line);
                if tx.try_send(create_log_entry(LogLevel::Info, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }))
    } else {
        None
    };

    // Handle stderr
    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // FFmpeg outputs progress to stderr, so we check for error indicators
                // Use more specific patterns to avoid false positives
                let level = if line.starts_with("[error]")
                    || line.contains("Error ")
                    || line.contains("error:")
                    || line.contains("failed")
                    || line.contains("Invalid ")
                {
                    warn!("stderr: {}", line);
                    LogLevel::Error
                } else {
                    debug!("stderr: {}", line);
                    LogLevel::Info
                };

                if tx.try_send(create_log_entry(level, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }))
    } else {
        None
    };

    // Drop original sender so channel closes when tasks complete
    drop(tx);

    let mut logs = VecDeque::new();
    let mut truncated_count = 0usize;

    // Drain logs while waiting for the process to exit so the bounded channel
    // doesn't fill up and drop important trailing output.
    let mut status: Option<std::process::ExitStatus> = None;
    let mut wait_fut = Box::pin(child.wait());

    loop {
        tokio::select! {
            res = &mut wait_fut, if status.is_none() => {
                status = Some(res.map_err(|e| {
                    crate::Error::Other(format!("Failed to wait for command: {}", e))
                })?);
            }
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        if let Some(sink) = &log_sink {
                            sink.try_send(entry.clone());
                        }
                        push_log_with_cap(&mut logs, entry, MAX_LOG_ENTRIES, &mut truncated_count);
                    }
                    None => {
                        // All reader tasks finished and dropped their senders.
                        // If the process hasn't exited yet (e.g. no pipes), wait for it now.
                        if status.is_none() {
                            status = Some(wait_fut.await.map_err(|e| {
                                crate::Error::Other(format!("Failed to wait for command: {}", e))
                            })?);
                        }
                        break;
                    }
                }
            }
        }
    }

    // Wait for reader tasks to complete to ensure streams are fully consumed.
    if let Some(handle) = stdout_handle {
        let _ = handle.await;
    }
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let duration = start.elapsed().as_secs_f64();
    let status =
        status.ok_or_else(|| crate::Error::Other("process exit status missing".to_string()))?;

    let dropped = dropped_count.load(Ordering::Relaxed);
    if dropped > 0 {
        push_log_with_cap(
            &mut logs,
            JobLogEntry::warn(format!(
                "Dropped {} log lines due to backpressure (capacity={})",
                dropped, LOG_CHANNEL_CAPACITY
            )),
            MAX_LOG_ENTRIES,
            &mut truncated_count,
        );
    }
    if truncated_count > 0 {
        push_log_with_cap(
            &mut logs,
            JobLogEntry::warn(format!(
                "Truncated {} older log entries (kept last {} entries)",
                truncated_count, MAX_LOG_ENTRIES
            )),
            MAX_LOG_ENTRIES,
            &mut truncated_count,
        );
    }

    Ok(CommandOutput {
        status,
        duration,
        logs: logs.into_iter().collect(),
    })
}

#[derive(Default)]
struct FfmpegProgressState {
    out_time_ms: Option<u64>,
    total_size: Option<u64>,
    speed_x: Option<f64>,
    raw: serde_json::Map<String, serde_json::Value>,
}

fn parse_speed_x(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('x');
    s.parse::<f64>().ok()
}

fn parse_ffmpeg_kv_line(
    line: &str,
    state: &mut FfmpegProgressState,
) -> Option<JobProgressSnapshot> {
    let (k, v) = line.split_once('=')?;
    let key = k.trim();
    let value = v.trim();

    state.raw.insert(
        key.to_string(),
        serde_json::Value::String(value.to_string()),
    );

    match key {
        "out_time_ms" => state.out_time_ms = value.parse::<u64>().ok(),
        "total_size" => state.total_size = value.parse::<u64>().ok(),
        "speed" => state.speed_x = parse_speed_x(value),
        "progress" => {
            let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
            snapshot.out_time_ms = state.out_time_ms;
            snapshot.bytes_done = state.total_size;
            snapshot.raw = serde_json::Value::Object(state.raw.clone());
            return Some(snapshot);
        }
        _ => {}
    }

    None
}

fn parse_size_to_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let mut parts = s.split_whitespace();
    let number = parts.next()?;
    let unit = parts.next().unwrap_or("B");
    let value = number.replace(',', "").parse::<f64>().ok()?;
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "b" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        "tb" | "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((value * multiplier).max(0.0) as u64)
}

fn parse_speed_to_bytes_per_sec(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("/s").unwrap_or(s);
    let bytes = parse_size_to_bytes(s)? as f64;
    Some(bytes)
}

fn parse_eta_to_secs(s: &str) -> Option<f64> {
    let mut total = 0f64;
    let mut current = String::new();
    for ch in s.trim().chars() {
        if ch.is_ascii_digit() || ch == '.' {
            current.push(ch);
            continue;
        }
        let value = current.parse::<f64>().ok()?;
        current.clear();
        match ch {
            's' => total += value,
            'm' => total += value * 60.0,
            'h' => total += value * 3600.0,
            'd' => total += value * 86400.0,
            _ => return None,
        }
    }
    if !current.is_empty() {
        total += current.parse::<f64>().ok()?;
    }
    Some(total)
}

fn parse_rclone_stats_line(line: &str) -> Option<JobProgressSnapshot> {
    let idx = line.find("Transferred:")?;
    let rest = line[idx + "Transferred:".len()..].trim();
    let parts: Vec<&str> = rest.split(',').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    let (done_str, total_str) = parts[0].split_once('/')?;
    let bytes_done = parse_size_to_bytes(done_str)?;
    let bytes_total = parse_size_to_bytes(total_str)?;

    let percent = parts
        .get(1)
        .and_then(|p| p.strip_suffix('%'))
        .and_then(|p| p.trim().parse::<f32>().ok());

    let speed_bytes_per_sec = parts.get(2).and_then(|p| parse_speed_to_bytes_per_sec(p));

    let eta_secs = parts.get(3).and_then(|p| {
        let p = p.strip_prefix("ETA").unwrap_or(p).trim();
        parse_eta_to_secs(p)
    });

    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
    snapshot.bytes_done = Some(bytes_done);
    snapshot.bytes_total = Some(bytes_total);
    snapshot.percent = percent;
    snapshot.speed_bytes_per_sec = speed_bytes_per_sec;
    snapshot.eta_secs = eta_secs;
    snapshot.raw = serde_json::json!({
        "line": line,
        "bytes_done": bytes_done,
        "bytes_total": bytes_total,
        "percent": percent,
        "speed_bytes_per_sec": speed_bytes_per_sec,
        "eta_secs": eta_secs,
    });
    Some(snapshot)
}

/// Determine log level from an FFmpeg stderr line.
fn determine_ffmpeg_log_level(line: &str) -> LogLevel {
    let lower = line.to_lowercase();
    if lower.contains("error") || lower.starts_with("fatal") || lower.contains("failed") {
        LogLevel::Error
    } else if lower.contains("warning") || lower.contains("warn") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

/// Determine log level from an rclone stderr line.
/// Rclone uses prefixes like ERROR, NOTICE, WARNING in its log output.
fn determine_rclone_log_level(line: &str) -> LogLevel {
    // Rclone log format typically: "YYYY/MM/DD HH:MM:SS LEVEL: message"
    // or just contains these keywords
    if line.contains("ERROR") || line.contains("Failed") || line.contains("error:") {
        LogLevel::Error
    } else if line.contains("NOTICE") || line.contains("WARNING") || line.contains("WARN") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

/// Run an ffmpeg-style command that emits `-progress pipe:1` key=value lines on stdout.
/// This parses progress snapshots and emits them via `progress` while capturing only stderr logs.
pub async fn run_ffmpeg_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<super::traits::JobLogSink>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    command.no_window();

    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));

    let stdout_handle = if let Some(stdout) = child.stdout.take() {
        let progress = progress.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            let mut state = FfmpegProgressState::default();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(snapshot) = parse_ffmpeg_kv_line(&line, &mut state) {
                    progress.report(snapshot);
                }
            }
        }))
    } else {
        None
    };

    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                // Determine log level based on content
                let level = determine_ffmpeg_log_level(&line);
                if tx.try_send(create_log_entry(level, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }))
    } else {
        None
    };

    drop(tx);

    let mut logs = VecDeque::new();
    let mut truncated_count = 0usize;
    let mut status: Option<std::process::ExitStatus> = None;
    let mut wait_fut = Box::pin(child.wait());

    loop {
        tokio::select! {
            res = &mut wait_fut, if status.is_none() => {
                status = Some(res.map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?);
            }
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        if let Some(sink) = &log_sink {
                            sink.try_send(entry.clone());
                        }
                        push_log_with_cap(&mut logs, entry, MAX_LOG_ENTRIES, &mut truncated_count)
                    },
                    None => {
                        if status.is_none() {
                            status = Some(wait_fut.await.map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?);
                        }
                        break;
                    }
                }
            }
        }
    }

    if let Some(handle) = stdout_handle {
        let _ = handle.await;
    }
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let duration = start.elapsed().as_secs_f64();
    let status =
        status.ok_or_else(|| crate::Error::Other("process exit status missing".to_string()))?;

    let dropped = dropped_count.load(Ordering::Relaxed);
    if dropped > 0 {
        push_log_with_cap(
            &mut logs,
            JobLogEntry::warn(format!(
                "Dropped {} log lines due to backpressure (capacity={})",
                dropped, LOG_CHANNEL_CAPACITY
            )),
            MAX_LOG_ENTRIES,
            &mut truncated_count,
        );
    }

    Ok(CommandOutput {
        status,
        duration,
        logs: logs.into_iter().collect(),
    })
}

/// Run an rclone-style command configured with `--stats-one-line --stats=1s`.
/// This parses progress snapshots and emits them via `progress` while capturing only stderr logs.
pub async fn run_rclone_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<super::traits::JobLogSink>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    command.no_window();

    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));

    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        let progress = progress.clone();
        Some(tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Some(snapshot) = parse_rclone_stats_line(&line) {
                    progress.report(snapshot);
                    continue;
                }
                // Determine log level based on rclone output patterns
                let level = determine_rclone_log_level(&line);
                if tx.try_send(create_log_entry(level, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }))
    } else {
        None
    };

    drop(tx);

    let mut logs = VecDeque::new();
    let mut truncated_count = 0usize;
    let mut status: Option<std::process::ExitStatus> = None;
    let mut wait_fut = Box::pin(child.wait());

    loop {
        tokio::select! {
            res = &mut wait_fut, if status.is_none() => {
                status = Some(res.map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?);
            }
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        if let Some(sink) = &log_sink {
                            sink.try_send(entry.clone());
                        }
                        push_log_with_cap(&mut logs, entry, MAX_LOG_ENTRIES, &mut truncated_count)
                    },
                    None => {
                        if status.is_none() {
                            status = Some(wait_fut.await.map_err(|e| crate::Error::Other(format!("Failed to wait for command: {}", e)))?);
                        }
                        break;
                    }
                }
            }
        }
    }

    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    let duration = start.elapsed().as_secs_f64();
    let status =
        status.ok_or_else(|| crate::Error::Other("process exit status missing".to_string()))?;

    let dropped = dropped_count.load(Ordering::Relaxed);
    if dropped > 0 {
        push_log_with_cap(
            &mut logs,
            JobLogEntry::warn(format!(
                "Dropped {} log lines due to backpressure (capacity={})",
                dropped, LOG_CHANNEL_CAPACITY
            )),
            MAX_LOG_ENTRIES,
            &mut truncated_count,
        );
    }

    Ok(CommandOutput {
        status,
        duration,
        logs: logs.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_ffmpeg_log_level() {
        assert_eq!(
            determine_ffmpeg_log_level("Error: something went wrong"),
            LogLevel::Error
        );
        assert_eq!(
            determine_ffmpeg_log_level("[error] broken frame"),
            LogLevel::Error
        );
        assert_eq!(
            determine_ffmpeg_log_level("Fatal error occurred"),
            LogLevel::Error
        );
        assert_eq!(
            determine_ffmpeg_log_level("Warning: buffer underrun"),
            LogLevel::Warn
        );
        assert_eq!(
            determine_ffmpeg_log_level("[warn] something mild"),
            LogLevel::Warn
        );
        assert_eq!(
            determine_ffmpeg_log_level("Input #0, mov,mp4,m4a,3gp,3g2,mj2, from..."),
            LogLevel::Info
        );
        assert_eq!(
            determine_ffmpeg_log_level(
                "  Stream #0:0(und): Video: h264 (High) (avc1 / 0x31637661)"
            ),
            LogLevel::Info
        );
    }
}
