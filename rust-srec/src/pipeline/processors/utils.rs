//! Utility functions for processors.

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use process_utils::ContainedChild;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, warn};

use super::traits::{JobLogSink, ProcessorContext};
use super::upload_runner::{
    BAIDUPCS_BAR_END, UploadTracker, consume_baidupcs_stream, consume_rclone_stderr,
};
use crate::pipeline::job_queue::{JobLogEntry, LogLevel};
use crate::pipeline::{JobPhase, JobProgressSnapshot, ProgressKind, ProgressReporter};

const LOG_CHANNEL_CAPACITY: usize = 1024;
const MAX_LOG_ENTRIES: usize = 2000;

/// Retry delays stay representable and bounded even for extreme user settings.
pub(super) fn retry_delay(base_ms: u64, attempt: u32) -> std::time::Duration {
    std::time::Duration::from_millis(
        base_ms
            .saturating_mul(2u64.saturating_pow(attempt))
            .min(30_000),
    )
}

/// Waits out the delay after attempt `failed` (1-based) of the processor's
/// main work in its own retry loop, as [`wait_out_retry`] does. From the wait
/// on, reports also carry attempt `failed + 1` of at most `attempts`.
pub(super) async fn wait_to_retry(
    ctx: &ProcessorContext,
    kind: ProgressKind,
    failed: u32,
    attempts: u32,
    failure: &str,
) {
    ctx.progress.set_attempt(failed + 1, attempts);
    wait_out_retry(ctx, kind, failed, failure).await;
}

/// Waits out the delay after attempt `failed` (1-based) in a processor's own
/// retry loop. The job log notes `failure` and the wait. Progress reports the
/// retrying phase for the length of the wait, without the failed attempt's
/// bytes, speed or time left, then a plain snapshot so that the next attempt
/// does not read as retrying while it sets up (such as rclone before its
/// first stats line).
///
/// It sets no attempt number, because the reporter keeps one for the rest of
/// the job run: retries of a preparatory step, such as creating remote
/// folders, would otherwise number the main work's first attempt.
pub(super) async fn wait_out_retry(
    ctx: &ProcessorContext,
    kind: ProgressKind,
    failed: u32,
    failure: &str,
) {
    let delay = retry_delay(1000, failed);
    ctx.warn(format!("{failure}; retrying in {} s", delay.as_secs()));
    let mut snapshot = JobProgressSnapshot::new(kind);
    snapshot.phase = Some(JobPhase::Retrying);
    ctx.progress.report(snapshot);
    tokio::time::sleep(delay).await;
    ctx.progress.report(JobProgressSnapshot::new(kind));
}

pub(super) async fn try_exists(path: &Path) -> crate::Result<bool> {
    tokio::fs::try_exists(path)
        .await
        .map_err(|error| crate::Error::io_path("inspect processor path", path, error))
}
/// Hard per-line byte cap for child stdout/stderr readers (`CappedLines`).
/// Bounds the line buffer against streams that never emit a newline —
/// e.g. rclone's `--progress` display redraws with control codes instead
/// of `\n`, which would otherwise grow a single String for the entire
/// transfer. Generous enough for any real log or `--use-json-log` stats
/// line (a few KiB even with a full `transferring` array).
pub(super) const MAX_LINE_BYTES: usize = 64 * 1024;

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

/// Parse a processor config that must be valid when present, so a malformed
/// config fails the job instead of silently running with defaults.
/// `label` names the processor in the error, e.g. "rclone".
pub(super) fn parse_config_strict<T: DeserializeOwned + Default>(
    raw: Option<&str>,
    label: &str,
) -> crate::Result<T> {
    match raw {
        Some(raw) => serde_json::from_str(raw)
            .map_err(|e| crate::Error::Validation(format!("Invalid {label} config JSON: {e}"))),
        None => Ok(T::default()),
    }
}

/// Fail with a validation error naming the first input that does not exist.
pub(super) async fn ensure_inputs_exist(inputs: &[String]) -> crate::Result<()> {
    for input_path in inputs {
        if !try_exists(Path::new(input_path)).await? {
            return Err(crate::Error::Validation(format!(
                "Input file does not exist: {input_path}"
            )));
        }
    }
    Ok(())
}

/// Outcome of [`remove_sources`].
#[derive(Debug, Default)]
pub(super) struct RemovalSummary {
    /// Paths that were removed or were already absent.
    pub removed: HashSet<String>,
    /// The subset of `removed` that was already absent.
    pub absent: HashSet<String>,
    /// The logged message for each path that could not be removed.
    pub failed: Vec<String>,
}

/// Best-effort removal of source files after their outputs were published.
/// A path that is already absent counts as removed. Each failure is logged
/// once to tracing and once to the job log, as "Failed to remove {what} {path}".
pub(super) async fn remove_sources<'a>(
    paths: impl IntoIterator<Item = &'a String>,
    what: &str,
    ctx: &ProcessorContext,
) -> RemovalSummary {
    let mut summary = RemovalSummary::default();
    for path in paths {
        match tokio::fs::remove_file(path).await {
            Ok(()) => {
                summary.removed.insert(path.clone());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                summary.removed.insert(path.clone());
                summary.absent.insert(path.clone());
            }
            Err(error) => {
                warn!(path = %path, %error, "Failed to remove {what}");
                let msg = format!("Failed to remove {what} {path}: {error}");
                ctx.warn(msg.clone());
                summary.failed.push(msg);
            }
        }
    }
    summary
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

impl CommandOutput {
    /// Captured non-Info messages joined with newlines, in their original order.
    pub(super) fn diagnostic_text(&self) -> String {
        self.logs
            .iter()
            .filter(|entry| entry.level != LogLevel::Info)
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Last captured error, excluding later warnings or informational output.
    /// Callers choose their own fallback when no error line was captured.
    pub fn last_error_message(&self) -> Option<&str> {
        self.logs
            .iter()
            .rfind(|entry| entry.level == LogLevel::Error)
            .map(|entry| entry.message.as_str())
    }
}

async fn wait_for_reader_task(
    stream_name: &'static str,
    handle: Option<AbortOnDropHandle<std::io::Result<()>>>,
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

/// Line reader over a child process stream holding at most `MAX_LINE_BYTES`
/// of an unterminated line: once a line crosses the cap its buffered prefix
/// is dropped, input is discarded up to the next newline, and reading
/// resumes with the following line. Each skipped line is counted in
/// `overlong_count` so callers can surface one summary warning next to the
/// `dropped_count` backpressure summary. (A codec-based reader is unsuitable
/// here: `FramedRead` treats a decode error as terminal, so a single
/// overlong line would silently end the stream instead of being skipped.)
pub(super) struct CappedLines<R> {
    reader: BufReader<R>,
    partial: Vec<u8>,
    skipping_to_newline: bool,
    overlong_count: Arc<AtomicUsize>,
    /// Treat `\r` as a line terminator alongside `\n`. Needed for
    /// BaiduPCS-Go, whose upload progress bar redraws via bare `\r`
    /// (`run_baidupcs_with_logs`): without CR splitting an entire
    /// transfer's redraws accumulate into one overlong "line" that gets
    /// skipped wholesale. `\r\n` then yields an empty line for the `\n`,
    /// which callers must skip. A segment ending in [`BAIDUPCS_BAR_END`] is
    /// also yielded as soon as it is read, without waiting for a terminator.
    split_cr: bool,
}

impl<R: AsyncRead + Unpin> CappedLines<R> {
    pub(super) fn new(reader: R, overlong_count: Arc<AtomicUsize>) -> Self {
        Self {
            reader: BufReader::new(reader),
            partial: Vec::new(),
            skipping_to_newline: false,
            overlong_count,
            split_cr: false,
        }
    }

    pub(super) fn with_cr_splitting(reader: R, overlong_count: Arc<AtomicUsize>) -> Self {
        Self {
            split_cr: true,
            ..Self::new(reader, overlong_count)
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
    pub(super) async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        let split_cr = self.split_cr;
        loop {
            let chunk = self.reader.fill_buf().await?;
            if chunk.is_empty() {
                if self.partial.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(self.take_line()));
            }

            match chunk
                .iter()
                .position(|&byte| byte == b'\n' || (split_cr && byte == b'\r'))
            {
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
                    // Only BaiduPCS-Go's next output ends such a bar, and that
                    // may be the next bar 3 s later.
                    if self.split_cr && self.partial.ends_with(BAIDUPCS_BAR_END.as_bytes()) {
                        return Ok(Some(self.take_line()));
                    }
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

#[derive(Clone)]
pub(super) enum CommandMode {
    Plain,
    Ffmpeg {
        progress: ProgressReporter,
        /// Shared by the run's stdout and stderr readers.
        state: Arc<Mutex<FfmpegProgressState>>,
    },
    Rclone(ProgressReporter),
    /// Follows an `upload` invocation's progress when present.
    BaiduPcs(Option<Arc<UploadTracker>>),
}

#[derive(Clone)]
struct ReaderContext {
    mode: CommandMode,
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped_count: Arc<AtomicUsize>,
    overlong_count: Arc<AtomicUsize>,
    log_sink: Option<JobLogSink>,
}

async fn consume_command_stream<R: AsyncRead + Unpin>(
    reader: R,
    stdout: bool,
    context: ReaderContext,
) -> std::io::Result<()> {
    if let CommandMode::Rclone(progress) = &context.mode {
        return consume_rclone_stderr(
            reader,
            progress.clone(),
            context.tx,
            context.dropped_count,
            context.overlong_count,
        )
        .await;
    }
    if let CommandMode::BaiduPcs(tracker) = &context.mode {
        return consume_baidupcs_stream(
            reader,
            tracker.clone(),
            context.log_sink,
            context.tx,
            context.dropped_count,
            context.overlong_count,
        )
        .await;
    }
    let mut lines = CappedLines::new(reader, context.overlong_count);
    if stdout && let CommandMode::Ffmpeg { progress, state } = &context.mode {
        while let Some(line) = lines.next_line().await? {
            // Reporting under the state lock delivers the two readers'
            // snapshots in the order they read the run's state.
            let mut state = state.lock();
            if let Some(snapshot) = parse_ffmpeg_kv_line(&line, &mut state) {
                progress.report(snapshot);
            }
        }
        return Ok(());
    }
    while let Some(line) = lines.next_line().await? {
        let level = if stdout {
            LogLevel::Info
        } else if let CommandMode::Ffmpeg { progress, state } = &context.mode {
            let mut state = state.lock();
            if let Some(snapshot) = parse_ffmpeg_stderr_line(&line, &mut state) {
                progress.report(snapshot);
            }
            determine_ffmpeg_log_level(&line)
        } else if line.starts_with("[error]")
            || line.contains("Error ")
            || line.contains("error:")
            || line.contains("failed")
            || line.contains("Invalid ")
        {
            LogLevel::Error
        } else {
            LogLevel::Info
        };
        if matches!(&context.mode, CommandMode::Plain) {
            debug!(stdout, %line, "Command output");
        }
        if context.tx.try_send(JobLogEntry::new(level, line)).is_err() {
            context.dropped_count.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Own the process tree and its pipe readers for the lifetime of the command.
/// Worker cancellation and deadlines drop this future; containment then stops
/// descendants and the reader guards close pipes without leaving detached tasks.
/// Dropping only requests termination; callers that must know the process has
/// exited use [`run_logged_command_until`].
pub(super) async fn run_logged_command(
    command: &mut Command,
    mode: CommandMode,
    log_sink: Option<JobLogSink>,
) -> crate::Result<CommandOutput> {
    run_logged_command_until(command, mode, log_sink, None).await
}

/// How long a stopped command's process tree gets to be reaped before
/// [`run_logged_command_until`] logs that it is still waiting.
const STOPPED_COMMAND_REAP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// [`run_logged_command`] that, once `stop` is cancelled, terminates the
/// process tree and returns only after the direct child has been reaped, for
/// callers that must not start another process before then. A child stuck in
/// the kernel (for example on a stalled network mount) is waited for as long
/// as it takes.
pub(super) async fn run_logged_command_until(
    command: &mut Command,
    mode: CommandMode,
    log_sink: Option<JobLogSink>,
    stop: Option<&tokio_util::sync::CancellationToken>,
) -> crate::Result<CommandOutput> {
    let start = std::time::Instant::now();
    command.stdin(Stdio::null());
    command.stdout(if matches!(&mode, CommandMode::Rclone(_)) {
        Stdio::null()
    } else {
        Stdio::piped()
    });
    command.stderr(Stdio::piped());
    let mut child = ContainedChild::spawn(command)
        .map_err(|error| crate::Error::Other(format!("Failed to spawn command: {error}")))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(LOG_CHANNEL_CAPACITY);
    let dropped_count = Arc::new(AtomicUsize::new(0));
    let overlong_count = Arc::new(AtomicUsize::new(0));
    let context = ReaderContext {
        mode,
        tx,
        dropped_count: dropped_count.clone(),
        overlong_count: overlong_count.clone(),
        log_sink: log_sink.clone(),
    };
    let stdout_handle = child.take_stdout().map(|stdout| {
        AbortOnDropHandle::new(tokio::spawn(consume_command_stream(
            stdout,
            true,
            context.clone(),
        )))
    });
    let stderr_handle = child.take_stderr().map(|stderr| {
        AbortOnDropHandle::new(tokio::spawn(consume_command_stream(
            stderr,
            false,
            context.clone(),
        )))
    });
    drop(context);

    let mut logs = VecDeque::new();
    let mut truncated_count = 0usize;
    let mut status = None;
    let mut stopped = false;
    let mut wait_fut = Box::pin(child.wait());
    loop {
        tokio::select! {
            () = async {
                match stop {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending().await,
                }
            }, if status.is_none() => {
                stopped = true;
                break;
            }
            result = &mut wait_fut, if status.is_none() => {
                status = Some(result.map_err(|error| crate::Error::Other(format!("Failed to wait for command: {error}")))?);
            }
            entry = rx.recv() => {
                match entry {
                    Some(entry) => {
                        if let Some(sink) = &log_sink { sink.try_send(entry.clone()); }
                        push_log_with_cap(&mut logs, entry, MAX_LOG_ENTRIES, &mut truncated_count);
                    }
                    None => {
                        if status.is_none() {
                            status = Some((&mut wait_fut).await.map_err(|error| crate::Error::Other(format!("Failed to wait for command: {error}")))?);
                        }
                        break;
                    }
                }
            }
        }
    }
    if stopped {
        drop(wait_fut);
        drop((stdout_handle, stderr_handle, log_sink));
        let deadline = tokio::time::Instant::now() + STOPPED_COMMAND_REAP_TIMEOUT;
        if let Err(error) = child.terminate_tree_until(deadline).await {
            warn!(%error, "Stopped command has not exited yet; waiting for it");
            if let Err(error) = child.wait().await {
                warn!(%error, "Failed to wait for stopped command");
            }
        }
        return Err(crate::Error::Other("Command stopped".to_owned()));
    }
    let (stdout_result, stderr_result) = tokio::join!(
        wait_for_reader_task("command stdout", stdout_handle),
        wait_for_reader_task("command stderr", stderr_handle),
    );
    stdout_result?;
    stderr_result?;

    let status =
        status.ok_or_else(|| crate::Error::Other("process exit status missing".to_owned()))?;
    let dropped = dropped_count.load(Ordering::Relaxed);
    if dropped > 0 {
        push_log_with_cap(
            &mut logs,
            JobLogEntry::warn(format!(
                "Dropped {dropped} log lines due to backpressure (capacity={LOG_CHANNEL_CAPACITY})"
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
                "Truncated {truncated_count} older log entries (kept last {MAX_LOG_ENTRIES} entries)"
            )),
            MAX_LOG_ENTRIES,
            &mut truncated_count,
        );
    }
    Ok(CommandOutput {
        status,
        duration: start.elapsed().as_secs_f64(),
        logs: logs.into_iter().collect(),
    })
}

/// Run a command and capture bounded stdout/stderr logs.
pub async fn run_command_with_logs(
    command: &mut Command,
    log_sink: Option<JobLogSink>,
) -> crate::Result<CommandOutput> {
    run_logged_command(command, CommandMode::Plain, log_sink).await
}

/// What one FFmpeg run has reported on stdout and stderr.
#[derive(Default)]
pub(super) struct FfmpegProgressState {
    /// Range of input time in seconds the run writes when it cuts its input.
    trim: Option<Range<f64>>,
    /// The first input's length in seconds from ffmpeg's stderr report;
    /// `Some(None)` once it reported an unknown length.
    input_secs: Option<Option<f64>>,
    out_time_ms: Option<u64>,
    total_size: Option<u64>,
    speed_x: Option<f64>,
    phase: Option<JobPhase>,
    /// Set by the final `progress=end` report.
    ended: bool,
    raw: serde_json::Map<String, serde_json::Value>,
}

impl FfmpegProgressState {
    /// Media length the run writes, when known.
    fn duration_ms(&self) -> Option<u64> {
        output_length_ms(self.input_secs.flatten(), self.trim.as_ref())
    }

    fn snapshot(&self) -> JobProgressSnapshot {
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        snapshot.out_time_ms = self.out_time_ms;
        snapshot.bytes_done = self.total_size;
        snapshot.duration_ms = self.duration_ms();
        snapshot.phase = self.phase;
        // Speed and time left describe writing frames, which a finalizing run
        // has finished.
        let speed = self.speed_x.filter(|_| self.phase.is_none());
        snapshot.speed_x = speed;
        if let Some(total) = snapshot.duration_ms {
            let remaining = if self.ended {
                0
            } else {
                total.saturating_sub(snapshot.out_time_ms.unwrap_or(0))
            };
            // Only the end marker reaches 100: an estimated input length
            // (MPEG-TS) can be overshot, and a faststart rewrite runs after
            // the last frame is written.
            snapshot.percent = Some(if self.ended {
                100.0
            } else {
                ((total - remaining) as f64 / total as f64 * 100.0).min(99.0) as f32
            });
            snapshot.eta_secs = speed
                .filter(|speed| *speed > 0.0)
                .map(|speed| remaining as f64 / 1000.0 / speed);
        }
        snapshot.raw = serde_json::Value::Object(self.raw.clone());
        snapshot
    }
}

fn parse_speed_x(s: &str) -> Option<f64> {
    let s = s.trim().trim_end_matches('x');
    s.parse::<f64>().ok()
}

/// Reads an ffmpeg stderr line, returning a snapshot when it changes the
/// run's phase.
///
/// The input report line (`  Duration: 01:02:03.45, start: …`) gives the
/// length; only the first input's report counts, and its `Duration: N/A`
/// records an unknown length. The MP4 muxer logs `Starting second pass`
/// after it has written every packet and before it rewrites the file to move
/// its index (faststart) or insert one (global sidx), which can take a while
/// for a long recording.
fn parse_ffmpeg_stderr_line(
    line: &str,
    state: &mut FfmpegProgressState,
) -> Option<JobProgressSnapshot> {
    if let Some(value) = line.trim_start().strip_prefix("Duration: ") {
        if state.input_secs.is_none() {
            let clock = value.split(',').next().unwrap_or_default().trim();
            state.input_secs = Some(clock.split(':').try_fold(0.0, |secs, part| {
                Some(secs * 60.0 + part.parse::<f64>().ok()?)
            }));
        }
        return None;
    }
    if state.ended || !line.contains("Starting second pass") {
        return None;
    }
    state.phase = Some(JobPhase::Finalizing);
    state.out_time_ms = state.out_time_ms.max(state.duration_ms());
    Some(state.snapshot())
}

/// Milliseconds of media a run writes: the input's length, or the `trim`
/// range of input time clipped to that length.
fn output_length_ms(input_secs: Option<f64>, trim: Option<&Range<f64>>) -> Option<u64> {
    let secs = match trim {
        Some(trim) => input_secs.map_or(trim.end, |input| input.min(trim.end)) - trim.start,
        None => input_secs?,
    };
    let ms = (secs * 1000.0) as u64;
    (secs.is_finite() && ms > 0).then_some(ms)
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
        // Both keys carry microseconds (FFmpeg's legacy out_time_ms name too).
        // FFmpeg prints N/A while the encoder flushes and can step back once
        // under load; neither may move the reported position backwards.
        "out_time_ms" | "out_time_us" => {
            if let Ok(us) = value.parse::<u64>() {
                state.out_time_ms = state.out_time_ms.max(Some(us / 1000));
            }
        }
        "total_size" => {
            if let Ok(size) = value.parse::<u64>() {
                state.total_size = Some(size);
            }
        }
        "speed" => {
            if let Some(speed) = parse_speed_x(value) {
                state.speed_x = Some(speed);
            }
        }
        "progress" => {
            if value == "end" {
                state.ended = true;
                state.phase = None;
            }
            return Some(state.snapshot());
        }
        _ => {}
    }

    None
}

pub(super) fn parse_size_to_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let unit_start = s
        .find(|ch: char| ch.is_ascii_alphabetic())
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(unit_start);
    let number = number.trim();
    let unit = unit.trim();
    let unit = if unit.is_empty() { "B" } else { unit };
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

pub(super) fn parse_speed_to_bytes_per_sec(s: &str) -> Option<f64> {
    let s = s.trim();
    let s = s.strip_suffix("/s").unwrap_or(s);
    let bytes = parse_size_to_bytes(s)? as f64;
    Some(bytes)
}

pub(super) fn parse_eta_to_secs(s: &str) -> Option<f64> {
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

/// Determine log level from an FFmpeg stderr line.
fn determine_ffmpeg_log_level(line: &str) -> LogLevel {
    let lower = line.to_lowercase();
    if lower.contains("error")
        || lower.starts_with("fatal")
        || lower.contains("failed")
        // ffmpeg logs these at error level without saying so; processors pass
        // an input with no usable stream through when they see them.
        || lower.contains("does not contain any stream")
        || lower.contains("nothing was written into output file")
    {
        LogLevel::Error
    } else if lower.contains("warning") || lower.contains("warn") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

/// Run an ffmpeg-style command that emits `-progress pipe:1` key=value lines on stdout.
/// This parses progress snapshots and emits them via `progress` while capturing only stderr logs.
/// Percent and ETA need the input length from ffmpeg's stderr report, printed at
/// `-loglevel info`. `trim` is the range of input time in seconds the command writes
/// when it cuts its input.
pub async fn run_ffmpeg_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<JobLogSink>,
    trim: Option<Range<f64>>,
) -> crate::Result<CommandOutput> {
    let mode = CommandMode::Ffmpeg {
        progress: progress.clone(),
        state: Arc::new(Mutex::new(FfmpegProgressState {
            trim,
            ..Default::default()
        })),
    };
    run_logged_command(command, mode, log_sink).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_diagnostics_preserve_levels_order_and_fallback_choice() {
        let mut output = CommandOutput {
            status: super::super::test_utils::test_exit_status(false),
            duration: 0.0,
            logs: vec![JobLogEntry::warn("warning only")],
        };
        assert_eq!(output.last_error_message(), None);
        assert_eq!(output.diagnostic_text(), "warning only");
        output.logs.extend([
            JobLogEntry::error("first failure"),
            JobLogEntry::error("final failure"),
            JobLogEntry::info("closing output"),
            JobLogEntry::debug("debug context"),
        ]);
        assert_eq!(output.last_error_message(), Some("final failure"));
        assert_eq!(
            output.diagnostic_text(),
            "warning only\nfirst failure\nfinal failure\ndebug context"
        );
        output.logs.push(JobLogEntry::error(""));
        assert_eq!(output.last_error_message(), Some(""));
        assert!(output.diagnostic_text().ends_with("debug context\n"));
        output.logs = vec![JobLogEntry::info("info only")];
        assert_eq!(output.diagnostic_text(), "");
    }

    #[test]
    fn ffmpeg_stderr_lines_are_classified_by_level() {
        for (line, level) in [
            ("Error: something went wrong", LogLevel::Error),
            ("[error] broken frame", LogLevel::Error),
            ("Fatal error occurred", LogLevel::Error),
            ("Conversion failed!", LogLevel::Error),
            ("Warning: buffer underrun", LogLevel::Warn),
            ("[warn] something mild", LogLevel::Warn),
            ("Input #0, mov,mp4,m4a,3gp,3g2,mj2, from...", LogLevel::Info),
            (
                "  Stream #0:0(und): Video: h264 (High) (avc1 / 0x31637661)",
                LogLevel::Info,
            ),
        ] {
            assert_eq!(determine_ffmpeg_log_level(line), level, "{line}");
        }
    }

    /// Processors look for these lines in `diagnostic_text`, which leaves out
    /// Info lines, to pass an input with no usable stream through. Both are
    /// verbatim from ffmpeg 9.0.2.
    #[test]
    fn ffmpeg_no_stream_lines_are_errors() {
        for line in [
            "[out#0/adts @ 0x55562415fbc0] Output file does not contain any stream",
            "[out#0/image2 @ 0x55b745880c80] Nothing was written into output file, because at least one of its streams received no packets.",
        ] {
            assert_eq!(determine_ffmpeg_log_level(line), LogLevel::Error, "{line}");
        }
    }

    #[test]
    fn processor_retry_delay_saturates_and_caps_extreme_configuration() {
        assert_eq!(retry_delay(100, 0), std::time::Duration::from_millis(100));
        assert_eq!(retry_delay(100, 3), std::time::Duration::from_millis(800));
        for (base, attempt) in [(u64::MAX, 0), (100, 64), (1000, u32::MAX)] {
            assert_eq!(
                retry_delay(base, attempt),
                std::time::Duration::from_secs(30)
            );
        }
        assert_eq!(retry_delay(0, u32::MAX), std::time::Duration::ZERO);
    }

    #[test]
    fn ffmpeg_progress_position_is_in_milliseconds_and_never_moves_back() {
        for key in ["out_time_ms", "out_time_us"] {
            let mut state = FfmpegProgressState::default();
            parse_ffmpeg_kv_line(&format!("{key}=1234567"), &mut state);
            let progress = parse_ffmpeg_kv_line("progress=continue", &mut state).unwrap();
            assert_eq!(progress.out_time_ms, Some(1234));
            assert_eq!(progress.raw[key], "1234567");
        }
        let mut state = FfmpegProgressState::default();
        parse_ffmpeg_kv_line("out_time_us=2000000", &mut state);
        parse_ffmpeg_kv_line("out_time_ms=1000000", &mut state);
        assert_eq!(
            parse_ffmpeg_kv_line("progress=end", &mut state)
                .unwrap()
                .out_time_ms,
            Some(2000)
        );
        parse_ffmpeg_kv_line("out_time_us=N/A", &mut state);
        parse_ffmpeg_kv_line("out_time_ms=-1", &mut state);
        assert_eq!(
            parse_ffmpeg_kv_line("progress=end", &mut state)
                .unwrap()
                .out_time_ms,
            Some(2000)
        );
    }

    #[test]
    fn ffmpeg_progress_percent_and_eta_stay_below_100_until_end() {
        let mut state = FfmpegProgressState {
            input_secs: Some(Some(60.0)),
            ..Default::default()
        };
        parse_ffmpeg_kv_line("out_time_us=15000000", &mut state);
        parse_ffmpeg_kv_line("speed=1.5x", &mut state);
        let running = parse_ffmpeg_kv_line("progress=continue", &mut state).unwrap();
        assert_eq!(
            (
                running.percent,
                running.eta_secs,
                running.speed_x,
                running.duration_ms
            ),
            (Some(25.0), Some(30.0), Some(1.5), Some(60_000))
        );

        parse_ffmpeg_kv_line("out_time_us=61000000", &mut state);
        let past_length = parse_ffmpeg_kv_line("progress=continue", &mut state).unwrap();
        assert_eq!(past_length.percent, Some(99.0));
        let ended = parse_ffmpeg_kv_line("progress=end", &mut state).unwrap();
        assert_eq!((ended.percent, ended.eta_secs), (Some(100.0), Some(0.0)));
    }

    #[test]
    fn ffmpeg_finalizing_holds_99_percent_until_end() {
        const SECOND_PASS: &str = "[mp4 @ 0x5564] Starting second pass: moving the moov atom to the beginning of the file";
        let writing = || {
            let mut state = FfmpegProgressState::default();
            parse_ffmpeg_stderr_line(
                "  Duration: 00:01:00.00, start: 0.000000, bitrate: 2000 kb/s",
                &mut state,
            );
            for line in ["out_time_us=59000000", "speed=2x", "progress=continue"] {
                parse_ffmpeg_kv_line(line, &mut state);
            }
            state
        };

        let mut state = writing();
        let finalizing = parse_ffmpeg_stderr_line(SECOND_PASS, &mut state).unwrap();
        assert_eq!(
            (
                finalizing.phase,
                finalizing.out_time_ms,
                finalizing.percent,
                finalizing.eta_secs,
                finalizing.speed_x
            ),
            (
                Some(JobPhase::Finalizing),
                Some(60_000),
                Some(99.0),
                None,
                None
            )
        );
        let later = parse_ffmpeg_kv_line("progress=continue", &mut state).unwrap();
        assert_eq!((later.phase, later.percent), (finalizing.phase, Some(99.0)));
        let ended = parse_ffmpeg_kv_line("progress=end", &mut state).unwrap();
        assert_eq!((ended.phase, ended.percent), (None, Some(100.0)));

        // stdout and stderr are read separately, so the second-pass line can
        // be read after the end.
        let mut state = writing();
        parse_ffmpeg_kv_line("progress=end", &mut state);
        assert!(parse_ffmpeg_stderr_line(SECOND_PASS, &mut state).is_none());
        let ended = state.snapshot();
        assert_eq!((ended.phase, ended.percent), (None, Some(100.0)));
    }

    #[test]
    fn ffmpeg_length_comes_from_the_first_input_report_and_trimming() {
        let mut state = FfmpegProgressState::default();
        // Matroska's per-stream DURATION tag is metadata, not the input report.
        parse_ffmpeg_stderr_line("      DURATION        : 00:10:00.040000000", &mut state);
        assert_eq!(state.input_secs, None);
        parse_ffmpeg_stderr_line(
            "  Duration: 01:02:03.50, start: 0.000000, bitrate: 2000 kb/s",
            &mut state,
        );
        parse_ffmpeg_stderr_line(
            "  Duration: 00:00:05.00, start: 0.000000, bitrate: 128 kb/s",
            &mut state,
        );
        assert_eq!(state.input_secs, Some(Some(3723.5)));
        let mut unknown = FfmpegProgressState::default();
        parse_ffmpeg_stderr_line(
            "  Duration: N/A, start: 0.000000, bitrate: N/A",
            &mut unknown,
        );
        assert_eq!(unknown.input_secs, Some(None));

        for (input, trim, expected) in [
            (Some(100.0), None, Some(100_000)),
            (Some(100.0), Some(10.0..f64::INFINITY), Some(90_000)),
            (Some(100.0), Some(10.0..40.0), Some(30_000)),
            (Some(20.0), Some(10.0..40.0), Some(10_000)),
            (None, Some(10.0..40.0), Some(30_000)),
            (None, Some(10.0..f64::INFINITY), None),
        ] {
            assert_eq!(
                output_length_ms(input, trim.as_ref()),
                expected,
                "input {input:?}, trim {trim:?}"
            );
        }
    }

    const HELPER_MODE: &str = "RUST_SREC_PROCESSOR_COMMAND_HELPER";
    const HEARTBEAT_PATH: &str = "RUST_SREC_PROCESSOR_COMMAND_HEARTBEAT";

    fn helper_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["processor_command_helper", "--nocapture"])
            .env(HELPER_MODE, mode);
        command
    }

    fn command_modes() -> [CommandMode; 4] {
        [
            CommandMode::Plain,
            CommandMode::Ffmpeg {
                progress: ProgressReporter::noop("test"),
                state: Arc::default(),
            },
            CommandMode::Rclone(ProgressReporter::noop("test")),
            CommandMode::BaiduPcs(None),
        ]
    }

    #[test]
    #[expect(
        clippy::zombie_processes,
        reason = "The tree-exit fixture deliberately leaves its descendant for the containing process to terminate"
    )]
    fn processor_command_helper() {
        use std::io::{Read, Write};
        let Ok(mode) = std::env::var(HELPER_MODE) else {
            return;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        match mode.as_str() {
            "stdin" => {
                assert_eq!(std::io::stdin().read(&mut [0u8; 1]).unwrap(), 0);
            }
            "exit-7" => std::process::exit(7),
            "logs" => {
                let stdout = std::thread::spawn(|| {
                    let mut stdout = std::io::stdout().lock();
                    stdout
                        .write_all(&vec![b'x'; MAX_LINE_BYTES + 1024])
                        .unwrap();
                    writeln!(stdout).unwrap();
                    for i in 0..5000 {
                        writeln!(stdout, "stdout {i}").unwrap();
                    }
                });
                let mut stderr = std::io::stderr().lock();
                stderr
                    .write_all(&vec![b'x'; MAX_LINE_BYTES + 1024])
                    .unwrap();
                writeln!(stderr).unwrap();
                for i in 0..5000 {
                    writeln!(stderr, "stderr {i}").unwrap();
                }
                writeln!(stderr, "[error] final diagnostic").unwrap();
                stdout.join().unwrap();
            }
            "tree-parent" | "tree-exit" => {
                let heartbeat = std::env::var_os(HEARTBEAT_PATH).unwrap();
                let mut command = process_utils::std_command(std::env::current_exe().unwrap());
                command
                    .args(["processor_command_helper", "--nocapture"])
                    .env(HELPER_MODE, "tree-leaf")
                    .env(HEARTBEAT_PATH, &heartbeat)
                    .stdin(Stdio::null())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit());
                let mut leaf = command.spawn().unwrap();
                while std::time::Instant::now() < deadline {
                    if mode == "tree-exit"
                        && std::fs::metadata(&heartbeat).is_ok_and(|m| m.len() > 0)
                    {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                if let Err(error) = leaf.kill() {
                    eprintln!("helper cleanup failed: {error}");
                }
                leaf.wait().unwrap();
            }
            "tree-leaf" => {
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(std::env::var_os(HEARTBEAT_PATH).unwrap())
                    .unwrap();
                while std::time::Instant::now() < deadline {
                    file.write_all(b"x").unwrap();
                    file.flush().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            _ => panic!("unknown helper mode"),
        }
    }

    async fn wait_for_heartbeat(path: &Path) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if tokio::fs::metadata(path).await.is_ok_and(|m| m.len() > 0) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("descendant must start");
    }

    async fn assert_heartbeat_stopped(path: &Path) {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let before = tokio::fs::metadata(path).await.unwrap().len();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        assert_eq!(
            tokio::fs::metadata(path).await.unwrap().len(),
            before,
            "descendant must stop after command cleanup"
        );
    }

    #[tokio::test]
    async fn command_modes_close_stdin_preserve_exit_status_and_bound_logs() {
        for mode in command_modes() {
            for helper in ["stdin", "exit-7", "logs"] {
                let mut command = helper_command(helper);
                let output = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    run_logged_command(&mut command, mode.clone(), None),
                )
                .await
                .expect("pipes must not deadlock")
                .unwrap();
                assert_eq!(
                    output.status.code(),
                    Some(if helper == "exit-7" { 7 } else { 0 })
                );
                assert!(output.logs.len() <= MAX_LOG_ENTRIES);
                if helper == "logs" {
                    assert!(
                        output
                            .logs
                            .iter()
                            .any(|entry| entry.message.contains("Skipped")
                                && entry.message.contains("output lines longer"))
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn command_cancellation_and_timeout_stop_descendants_in_every_mode() {
        for mode in command_modes() {
            for timeout in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let heartbeat = dir.path().join("heartbeat");
                let mut command = helper_command("tree-parent");
                command.env(HEARTBEAT_PATH, &heartbeat);
                let mut work = Box::pin(run_logged_command(&mut command, mode.clone(), None));
                tokio::select! {
                    result = &mut work => panic!("parent must still be running: {:?}", result.map(|output| output.status)),
                    () = wait_for_heartbeat(&heartbeat) => {},
                }
                if timeout {
                    assert!(
                        tokio::time::timeout(std::time::Duration::from_millis(30), work)
                            .await
                            .is_err()
                    );
                } else {
                    drop(work);
                }
                assert_heartbeat_stopped(&heartbeat).await;
            }
        }
    }

    #[tokio::test]
    async fn exited_parent_does_not_leave_descendants_holding_pipes_open() {
        for mode in command_modes() {
            let dir = tempfile::tempdir().unwrap();
            let heartbeat = dir.path().join("heartbeat");
            let mut command = helper_command("tree-exit");
            command.env(HEARTBEAT_PATH, &heartbeat);
            let output = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                run_logged_command(&mut command, mode, None),
            )
            .await
            .expect("exited descendants must not hold pipes open")
            .unwrap();
            assert!(output.status.success());
            assert_heartbeat_stopped(&heartbeat).await;
        }
    }
}
