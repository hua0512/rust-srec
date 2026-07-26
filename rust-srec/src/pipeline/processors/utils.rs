//! Utility functions for processors.

use crate::pipeline::job_queue::{JobLogEntry, LogLevel};
use crate::pipeline::{JobProgressSnapshot, ProgressKind, ProgressReporter};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tracing::{debug, warn};

use super::traits::ProcessorContext;
use process_utils::NoWindowExt;

const LOG_CHANNEL_CAPACITY: usize = 1024;
const MAX_LOG_ENTRIES: usize = 2000;
/// Hard per-line byte cap for child stdout/stderr readers (`CappedLines`).
/// Bounds the line buffer against streams that never emit a newline —
/// e.g. rclone's `--progress` display redraws with control codes instead
/// of `\n`, which would otherwise grow a single String for the entire
/// transfer. Generous enough for any real log or `--use-json-log` stats
/// line (a few KiB even with a full `transferring` array).
const MAX_LINE_BYTES: usize = 64 * 1024;

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

async fn wait_for_reader_task(
    stream_name: &'static str,
    handle: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
) -> crate::Result<()> {
    let Some(handle) = handle else {
        return Ok(());
    };

    handle
        .await
        .map_err(|error| crate::Error::Other(format!("{stream_name} reader task failed: {error}")))?
        .map_err(|error| crate::Error::Other(format!("Failed to read {stream_name}: {error}")))
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

/// Line reader over a child process stream holding at most `MAX_LINE_BYTES`
/// of an unterminated line: once a line crosses the cap its buffered prefix
/// is dropped, input is discarded up to the next newline, and reading
/// resumes with the following line. Each skipped line is counted in
/// `overlong_count` so callers can surface one summary warning next to the
/// `dropped_count` backpressure summary. (A codec-based reader is unsuitable
/// here: `FramedRead` treats a decode error as terminal, so a single
/// overlong line would silently end the stream instead of being skipped.)
struct CappedLines<R> {
    reader: BufReader<R>,
    partial: Vec<u8>,
    skipping_to_newline: bool,
    overlong_count: Arc<AtomicUsize>,
}

impl<R: AsyncRead + Unpin> CappedLines<R> {
    fn new(reader: R, overlong_count: Arc<AtomicUsize>) -> Self {
        Self {
            reader: BufReader::new(reader),
            partial: Vec::new(),
            skipping_to_newline: false,
            overlong_count,
        }
    }

    fn mark_overlong(&mut self) {
        self.partial.clear();
        self.skipping_to_newline = true;
        self.overlong_count.fetch_add(1, Ordering::Relaxed);
    }

    fn take_line(&mut self) -> String {
        if self.partial.last() == Some(&b'\r') {
            self.partial.pop();
        }
        let line = String::from_utf8_lossy(&self.partial).into_owned();
        self.partial.clear();
        line
    }

    /// Next complete line (newline and trailing `\r` stripped), `Ok(None)`
    /// at EOF. A final unterminated line is returned as-is.
    async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                if self.partial.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(self.take_line()));
            }

            match chunk.iter().position(|&byte| byte == b'\n') {
                Some(newline_index) => {
                    if self.skipping_to_newline {
                        // Tail of a line already counted overlong.
                        self.skipping_to_newline = false;
                    } else if self.partial.len() + newline_index > MAX_LINE_BYTES {
                        self.partial.clear();
                        self.overlong_count.fetch_add(1, Ordering::Relaxed);
                    } else {
                        self.partial.extend_from_slice(&chunk[..newline_index]);
                        self.reader.consume(newline_index + 1);
                        return Ok(Some(self.take_line()));
                    }
                    self.reader.consume(newline_index + 1);
                }
                None => {
                    let chunk_len = chunk.len();
                    if !self.skipping_to_newline {
                        if self.partial.len() + chunk_len > MAX_LINE_BYTES {
                            self.mark_overlong();
                        } else {
                            self.partial.extend_from_slice(chunk);
                        }
                    }
                    self.reader.consume(chunk_len);
                }
            }
        }
    }
}

/// One capped warn entry summarizing lines skipped for exceeding
/// `MAX_LINE_BYTES`; mirrors the `dropped_count` backpressure summary each
/// spawn helper emits after its wait loop.
fn push_overlong_summary(
    logs: &mut VecDeque<JobLogEntry>,
    overlong_count: &AtomicUsize,
    truncated_count: &mut usize,
) {
    let overlong = overlong_count.load(Ordering::Relaxed);
    if overlong > 0 {
        push_log_with_cap(
            logs,
            JobLogEntry::warn(format!(
                "Skipped {} output lines longer than {} bytes",
                overlong, MAX_LINE_BYTES
            )),
            MAX_LOG_ENTRIES,
            truncated_count,
        );
    }
}

/// Child-process settings shared by every spawn helper in this module:
/// hide the console window on Windows and kill the child when the owning
/// future is dropped, because worker cancellation and job timeouts drop
/// the processor future mid-run and the child must not outlive its job.
fn configure_child_process(command: &mut Command) {
    command.no_window();
    command.kill_on_drop(true);
}

/// Build a sibling temp path for `final_path` (`<name>.tmp-<uuid>`).
/// Writing to this path and renaming into place keeps a crashed or
/// cancelled job from leaving a partial file under the final name.
pub(super) fn tmp_output_path(final_path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "{}.tmp-{}",
        final_path.display(),
        uuid::Uuid::new_v4()
    ))
}

/// Run a command and capture its output (stdout/stderr) as logs.
/// This helper handles spawning the process, reading output streams asynchronously,
/// and collecting them into a structured log format.
pub async fn run_command_with_logs(
    command: &mut Command,
    log_sink: Option<super::traits::JobLogSink>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    configure_child_process(command);

    // Ensure pipes are set up
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));
    let overlong_count = Arc::new(AtomicUsize::new(0));

    // Handle stdout
    let stdout_handle = if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        let mut lines = CappedLines::new(stdout, overlong_count.clone());
        Some(tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                debug!("stdout: {}", line);
                if tx.try_send(create_log_entry(LogLevel::Info, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
        }))
    } else {
        None
    };

    // Handle stderr
    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        let mut lines = CappedLines::new(stderr, overlong_count.clone());
        Some(tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
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
            Ok(())
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
    let stdout_result = wait_for_reader_task("command stdout", stdout_handle).await;
    let stderr_result = wait_for_reader_task("command stderr", stderr_handle).await;
    stdout_result?;
    stderr_result?;

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
    push_overlong_summary(&mut logs, &overlong_count, &mut truncated_count);
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
    // Parsed fields only — never the source line. The snapshot is cloned per
    // aggregator flush into progress_cache, the WS broadcast, and the
    // job_execution_progress row, so raw must stay small and fixed-shape.
    snapshot.raw = serde_json::json!({
        "bytes_done": bytes_done,
        "bytes_total": bytes_total,
        "percent": percent,
        "speed_bytes_per_sec": speed_bytes_per_sec,
        "eta_secs": eta_secs,
    });
    Some(snapshot)
}

/// One stderr line from rclone under `--use-json-log`. Only the fields this
/// module consumes are declared; serde drops the rest at parse time —
/// deliberately including `stats.transferring`, whose per-file entries would
/// otherwise ride along on every snapshot clone.
#[derive(Deserialize)]
struct RcloneJsonLine {
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    stats: Option<RcloneJsonStats>,
}

#[derive(Deserialize)]
struct RcloneJsonStats {
    #[serde(default)]
    bytes: Option<u64>,
    #[serde(default, rename = "totalBytes")]
    total_bytes: Option<u64>,
    #[serde(default)]
    speed: Option<f64>,
    /// JSON `null` while rclone has no estimate yet.
    #[serde(default)]
    eta: Option<f64>,
    #[serde(default)]
    transfers: Option<u64>,
    #[serde(default, rename = "totalTransfers")]
    total_transfers: Option<u64>,
}

/// Maps rclone's JSON `level` values onto [`LogLevel`]. Error mapping
/// matters downstream: `RcloneProcessor` extracts its failure message from
/// the last `LogLevel::Error` entry in the captured logs.
fn map_rclone_json_level(level: Option<&str>) -> LogLevel {
    let Some(level) = level else {
        return LogLevel::Info;
    };
    if level.eq_ignore_ascii_case("error")
        || level.eq_ignore_ascii_case("critical")
        || level.eq_ignore_ascii_case("alert")
        || level.eq_ignore_ascii_case("emergency")
    {
        LogLevel::Error
    } else if level.eq_ignore_ascii_case("warning") {
        LogLevel::Warn
    } else if level.eq_ignore_ascii_case("debug") {
        LogLevel::Debug
    } else {
        LogLevel::Info
    }
}

fn snapshot_from_json_stats(stats: RcloneJsonStats) -> JobProgressSnapshot {
    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
    snapshot.bytes_done = stats.bytes;
    // totalBytes is 0 until rclone has sized the transfer; map that to None
    // so UploadProgress's optional bytes_total keeps meaning "unknown".
    snapshot.bytes_total = stats.total_bytes.filter(|total| *total > 0);
    snapshot.percent = match (snapshot.bytes_done, snapshot.bytes_total) {
        (Some(done), Some(total)) => {
            Some((done as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32)
        }
        _ => None,
    };
    snapshot.speed_bytes_per_sec = stats.speed;
    snapshot.eta_secs = stats.eta;
    // File counters only — see the raw-size note on parse_rclone_stats_line.
    snapshot.raw = serde_json::json!({
        "transfers": stats.transfers,
        "totalTransfers": stats.total_transfers,
    });
    snapshot
}

enum ParsedRcloneLine {
    Progress(JobProgressSnapshot),
    Log(JobLogEntry),
}

/// Classifies one rclone stderr line. JSON entries (the `--use-json-log`
/// contract set up by `RcloneProcessor`) carrying a `stats` object become
/// progress snapshots; other JSON entries become log entries using their own
/// `level`/`msg` fields. Non-JSON lines (user `extra_args` overriding the
/// log flags) fall back to the legacy `Transferred:` stats parser and the
/// keyword-based level heuristic.
fn parse_rclone_line(line: &str) -> ParsedRcloneLine {
    let trimmed = line.trim();
    if trimmed.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<RcloneJsonLine>(trimmed)
    {
        if let Some(stats) = parsed.stats {
            return ParsedRcloneLine::Progress(snapshot_from_json_stats(stats));
        }
        let level = map_rclone_json_level(parsed.level.as_deref());
        let message = match parsed.msg.map(|msg| msg.trim().to_string()) {
            Some(msg) if !msg.is_empty() => msg,
            _ => trimmed.to_string(),
        };
        return ParsedRcloneLine::Log(create_log_entry(level, message));
    }

    if let Some(snapshot) = parse_rclone_stats_line(line) {
        return ParsedRcloneLine::Progress(snapshot);
    }
    ParsedRcloneLine::Log(create_log_entry(determine_rclone_log_level(line), line))
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

    configure_child_process(command);

    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));
    let overlong_count = Arc::new(AtomicUsize::new(0));

    let stdout_handle = if let Some(stdout) = child.stdout.take() {
        let progress = progress.clone();
        let mut lines = CappedLines::new(stdout, overlong_count.clone());
        Some(tokio::spawn(async move {
            let mut state = FfmpegProgressState::default();
            while let Some(line) = lines.next_line().await? {
                if let Some(snapshot) = parse_ffmpeg_kv_line(&line, &mut state) {
                    progress.report(snapshot);
                }
            }
            Ok(())
        }))
    } else {
        None
    };

    let stderr_handle = if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let dropped_count = dropped_count.clone();
        let mut lines = CappedLines::new(stderr, overlong_count.clone());
        Some(tokio::spawn(async move {
            while let Some(line) = lines.next_line().await? {
                // Determine log level based on content
                let level = determine_ffmpeg_log_level(&line);
                if tx.try_send(create_log_entry(level, line)).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            Ok(())
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

    let stdout_result = wait_for_reader_task("ffmpeg progress", stdout_handle).await;
    let stderr_result = wait_for_reader_task("ffmpeg stderr", stderr_handle).await;
    stdout_result?;
    stderr_result?;

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
    push_overlong_summary(&mut logs, &overlong_count, &mut truncated_count);

    Ok(CommandOutput {
        status,
        duration,
        logs: logs.into_iter().collect(),
    })
}

/// Reads rclone stderr to EOF, routing each line through
/// [`parse_rclone_line`]: stats entries go to `progress`, everything else to
/// the bounded log channel (`dropped_count` tracks `try_send` overflow).
/// Overlong lines are skipped and counted by [`CappedLines`].
async fn consume_rclone_stderr<R: AsyncRead + Unpin>(
    reader: R,
    progress: ProgressReporter,
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped_count: Arc<AtomicUsize>,
    overlong_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut lines = CappedLines::new(reader, overlong_count);
    while let Some(line) = lines.next_line().await? {
        match parse_rclone_line(&line) {
            ParsedRcloneLine::Progress(snapshot) => progress.report(snapshot),
            ParsedRcloneLine::Log(entry) => {
                if tx.try_send(entry).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    Ok(())
}

/// Run an rclone command configured by `RcloneProcessor` with
/// `--use-json-log --stats 1s --stats-log-level NOTICE`, so stderr is one
/// JSON object per line. Lines carrying a `stats` object are reported via
/// `progress`; all other lines become captured log entries.
pub async fn run_rclone_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<super::traits::JobLogSink>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();

    configure_child_process(command);

    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|e| crate::Error::Other(format!("Failed to spawn command: {}", e)))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<JobLogEntry>(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));
    let overlong_count = Arc::new(AtomicUsize::new(0));

    let stderr_handle = child.stderr.take().map(|stderr| {
        tokio::spawn(consume_rclone_stderr(
            stderr,
            progress.clone(),
            tx.clone(),
            dropped_count.clone(),
            overlong_count.clone(),
        ))
    });

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

    wait_for_reader_task("rclone stderr", stderr_handle).await?;

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
    push_overlong_summary(&mut logs, &overlong_count, &mut truncated_count);

    Ok(CommandOutput {
        status,
        duration,
        logs: logs.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured from rclone v1.72.1 stderr under the flag set built by
    // RcloneProcessor (`--use-json-log --stats 1s --stats-log-level NOTICE`),
    // including the `transferring` array the parser must drop.
    const RCLONE_JSON_STATS_LINE: &str = r#"{"time":"2026-07-26T14:06:28.7986513+02:00","level":"notice","msg":"50.059 MiB / 100 MiB, 50%, 25.090 MiB/s, ETA 1s","stats":{"bytes":229376,"checks":0,"deletedDirs":0,"deletes":0,"elapsedTime":1.999943,"errors":0,"eta":2,"fatalError":false,"listed":1,"renames":0,"retryError":false,"speed":131072,"totalBytes":508811,"totalChecks":0,"totalTransfers":1,"transferTime":1.999943,"transferring":[{"bytes":229376,"eta":2,"group":"global_stats","name":"clip.mp4","percentage":45,"size":508811,"speed":114720.5,"speedAvg":131072}],"transfers":0},"source":"slog/logger.go:256"}"#;

    #[test]
    fn parse_rclone_line_json_stats_becomes_progress() {
        let ParsedRcloneLine::Progress(snapshot) = parse_rclone_line(RCLONE_JSON_STATS_LINE)
        else {
            panic!("stats JSON line should parse as progress");
        };

        assert_eq!(snapshot.kind, ProgressKind::Rclone);
        assert_eq!(snapshot.bytes_done, Some(229376));
        assert_eq!(snapshot.bytes_total, Some(508811));
        let percent = snapshot.percent.expect("percent computed from totals");
        assert!((percent - 45.08).abs() < 0.05, "got {percent}");
        assert_eq!(snapshot.speed_bytes_per_sec, Some(131072.0));
        assert_eq!(snapshot.eta_secs, Some(2.0));
        // Only the file counters survive into raw; `transferring` and the
        // human msg must not.
        assert_eq!(
            snapshot.raw,
            serde_json::json!({"transfers": 0, "totalTransfers": 1})
        );
    }

    #[test]
    fn parse_rclone_line_json_stats_with_unknown_totals() {
        let line = r#"{"level":"notice","msg":"early tick","stats":{"bytes":0,"totalBytes":0,"speed":0,"eta":null,"transfers":0,"totalTransfers":0}}"#;
        let ParsedRcloneLine::Progress(snapshot) = parse_rclone_line(line) else {
            panic!("stats JSON line should parse as progress");
        };

        assert_eq!(snapshot.bytes_done, Some(0));
        assert_eq!(snapshot.bytes_total, None, "totalBytes=0 means unknown");
        assert_eq!(snapshot.percent, None);
        assert_eq!(snapshot.eta_secs, None);
    }

    #[test]
    fn parse_rclone_line_json_log_maps_levels() {
        for (json_level, expected) in [
            ("error", LogLevel::Error),
            ("critical", LogLevel::Error),
            ("warning", LogLevel::Warn),
            ("notice", LogLevel::Info),
            ("info", LogLevel::Info),
            ("debug", LogLevel::Debug),
            ("something-new", LogLevel::Info),
        ] {
            let line = format!(r#"{{"level":"{json_level}","msg":"the message"}}"#);
            let ParsedRcloneLine::Log(entry) = parse_rclone_line(&line) else {
                panic!("JSON line without stats should parse as log");
            };
            assert_eq!(entry.level, expected, "level {json_level}");
            assert_eq!(entry.message, "the message");
        }
    }

    #[test]
    fn parse_rclone_line_json_log_without_msg_keeps_line() {
        let line = r#"{"level":"error"}"#;
        let ParsedRcloneLine::Log(entry) = parse_rclone_line(line) else {
            panic!("JSON line without stats should parse as log");
        };
        assert_eq!(entry.level, LogLevel::Error);
        assert_eq!(entry.message, line);
    }

    #[test]
    fn parse_rclone_line_legacy_transferred_text() {
        let line = "Transferred:   1.500 MiB / 3 MiB, 50%, 512 KiB/s, ETA 3s";
        let ParsedRcloneLine::Progress(snapshot) = parse_rclone_line(line) else {
            panic!("Transferred: text should parse as progress");
        };

        assert_eq!(snapshot.bytes_done, Some(1_572_864));
        assert_eq!(snapshot.bytes_total, Some(3_145_728));
        assert_eq!(snapshot.percent, Some(50.0));
        assert_eq!(snapshot.speed_bytes_per_sec, Some(524_288.0));
        assert_eq!(snapshot.eta_secs, Some(3.0));
        assert!(
            snapshot.raw.get("line").is_none(),
            "raw must not embed the source line"
        );
    }

    #[test]
    fn parse_rclone_line_plain_text_is_log() {
        let ParsedRcloneLine::Log(entry) =
            parse_rclone_line("2026/07/26 12:00:00 ERROR : clip.mp4: Failed to copy")
        else {
            panic!("plain text should parse as log");
        };
        assert_eq!(entry.level, LogLevel::Error);

        let ParsedRcloneLine::Log(entry) = parse_rclone_line("just some output") else {
            panic!("plain text should parse as log");
        };
        assert_eq!(entry.level, LogLevel::Info);
    }

    #[tokio::test]
    async fn consume_rclone_stderr_skips_overlong_lines_and_resumes() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1024];
        input.push(b'\n');
        input.extend_from_slice(RCLONE_JSON_STATS_LINE.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(b"plain log line\n");

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
        let progress = ProgressReporter::new("job-1", progress_tx);
        let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<JobLogEntry>(8);
        let dropped = Arc::new(AtomicUsize::new(0));
        let overlong = Arc::new(AtomicUsize::new(0));

        consume_rclone_stderr(
            input.as_slice(),
            progress,
            log_tx,
            dropped.clone(),
            overlong.clone(),
        )
        .await
        .expect("reader completes after skipping the overlong line");

        let update = progress_rx.try_recv().expect("one progress snapshot");
        assert_eq!(update.snapshot.bytes_done, Some(229376));
        assert!(progress_rx.try_recv().is_err());

        let entry = log_rx.try_recv().expect("one log entry");
        assert_eq!(entry.message, "plain log line");
        assert!(log_rx.try_recv().is_err());

        assert_eq!(overlong.load(Ordering::Relaxed), 1);
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

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
