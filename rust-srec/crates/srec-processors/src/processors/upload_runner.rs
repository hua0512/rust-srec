//! Shared upload command execution, progress parsing, and outcome tracking for
//! `RcloneProcessor`, `RshipProcessor`, and `BaiduPcsProcessor`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::MissedTickBehavior;

use super::traits::JobLogSink;
use super::utils::{
    CappedLines, CommandMode, CommandOutput, RcloneRunState, RshipRunState, parse_eta_to_secs,
    parse_size_to_bytes, parse_speed_to_bytes_per_sec, run_logged_command,
    run_logged_command_until,
};
use srec_core::domain::processor::{
    JobLogEntry, JobPhase, JobProgressSnapshot, LogLevel, ProgressKind, ProgressReporter,
    progress_file_name,
};

// Output markers printed by BaiduPCS-Go (hardcoded constants in its
// source, not localized). `parse_upload_outcomes` resolves per-file
// statuses from these because the CLI's exit code carries no signal.
/// `[<id>] 加入上传队列: <local path>` — establishes the task-id→file map.
pub(super) const MARKER_QUEUED: &str = "加入上传队列: ";
/// `[<id>] 准备上传: <local path>` — same mapping, printed at task start.
pub(super) const MARKER_PREPARING: &str = "准备上传: ";
/// Suffix carrying the final remote path on both success variants.
pub(super) const MARKER_REMOTE_PATH: &str = "保存到网盘路径: ";
/// `[<id>] 上传文件成功, 保存到网盘路径: <remote>`.
pub(super) const MARKER_UPLOAD_SUCCESS: &str = "上传文件成功";
/// `[<id>] 秒传成功, 保存到网盘路径: <remote>` (rapid upload).
pub(super) const MARKER_RAPID_SUCCESS: &str = "秒传成功";
/// Appears in both terminal failures and retry notices; a line is only
/// terminal when it lacks [`MARKER_RETRY`].
pub(super) const MARKER_UPLOAD_FAILED: &str = "上传文件失败";
/// `[<id>] 上传文件失败, 重试 <n>/<max>` — the CLI's own retry, not terminal.
pub(super) const MARKER_RETRY: &str = "重试";
/// Benign skip: the remote file already exists (`目标文件已存在, 跳过...`).
pub(super) const MARKER_EXISTS: &str = "已存在";
/// Benign skip under `--policy rsync`: `目标大小未发生改变, 跳过`.
pub(super) const MARKER_UNCHANGED: &str = "大小未发生改变";
/// `[0] <path> 文件路径含有非法字符，已跳过!` — the file is never uploaded,
/// so this "skip" is a permanent failure.
pub(super) const MARKER_ILLEGAL: &str = "非法字符";
/// `上传结束, 时间: <t>, 总大小: <s>` — the run reached its summary.
pub(super) const MARKER_FINISHED: &str = "上传结束";
/// `以下文件上传失败:` — header of the terminal failure table whose rows
/// carry task IDs and local paths.
pub(super) const MARKER_FAILURE_TABLE: &str = "以下文件上传失败";
/// `未检测到上传的文件.` — nothing was queued (e.g. every path was invalid).
pub(super) const MARKER_NO_TASKS: &str = "未检测到上传的文件";

/// Per-file outcomes extracted from one `upload` run's captured logs.
#[derive(Debug, Default)]
pub(super) struct UploadOutcomes {
    /// The run printed its `上传结束` summary.
    pub(super) finished: bool,
    /// Input → remote path from a success marker (`None` when the marker
    /// carried no readable path).
    pub(super) completed: HashMap<String, Option<String>>,
    /// Input → benign-skip reason line (already exists / size unchanged).
    pub(super) skipped: HashMap<String, String>,
    /// Input → terminal error line for this run that another attempt may
    /// resolve (a transfer error, a failure-table row).
    pub(super) failed: HashMap<String, String>,
    /// Input → error line for a file BaiduPCS-Go will never upload with this
    /// configuration: an illegal file name, a file over the size limit, an
    /// unreadable file or an exhausted quota. Retrying (and re-logging in
    /// before it) cannot change the outcome.
    pub(super) permanent: HashMap<String, String>,
    /// Success/benign-skip markers whose task ID never mapped to an input.
    pub(super) orphan_successes: usize,
    /// Failure markers whose task ID never mapped to an input.
    pub(super) orphan_failures: usize,
    /// The `以下文件上传失败` table header appeared.
    pub(super) saw_failure_table: bool,
}

/// `[<id>] rest` → `(id, rest)`. IDs are numeric task counters; anything
/// else inside the brackets is not a task prefix.
pub(super) fn parse_task_prefix(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix('[')?;
    let close = rest.find(']')?;
    let id = rest[..close].trim();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((id, rest[close + 1..].trim_start()))
}

/// Match a path printed by BaiduPCS-Go back to one of the job's inputs:
/// exact, then separator-normalized, then by file name when that is
/// unambiguous (the CLI may clean or absolutize the path it echoes).
fn match_input<'a>(inputs: &'a [String], printed: &str) -> Option<&'a String> {
    if let Some(exact) = inputs.iter().find(|input| input.as_str() == printed) {
        return Some(exact);
    }
    let normalized = printed.replace('\\', "/");
    if let Some(matched) = inputs
        .iter()
        .find(|input| input.replace('\\', "/") == normalized)
    {
        return Some(matched);
    }
    let printed_name = Path::new(printed).file_name()?;
    let mut candidates = inputs
        .iter()
        .filter(|input| Path::new(input.as_str()).file_name() == Some(printed_name));
    let first = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// Match an input embedded somewhere in `line` (failure-table rows carry a
/// task ID column next to the path, illegal-filename lines embed the path
/// mid-sentence). Falls back to an unambiguous file name.
fn match_input_by_containment<'a>(inputs: &'a [String], line: &str) -> Option<&'a String> {
    if let Some(direct) = inputs.iter().find(|input| line.contains(input.as_str())) {
        return Some(direct);
    }
    let mut candidates = inputs.iter().filter(|input| {
        Path::new(input.as_str())
            .file_name()
            .map(|name| line.contains(&*name.to_string_lossy()))
            .unwrap_or(false)
    });
    let first = candidates.next()?;
    if candidates.next().is_some() {
        None
    } else {
        Some(first)
    }
}

/// What a task line of `upload` output says about the task's file.
#[derive(Debug)]
enum TaskLine<'a> {
    /// `加入上传队列` or `准备上传`: task `id` is the file at `path`.
    /// `started` for the latter, printed when the task begins.
    Named {
        id: &'a str,
        path: &'a str,
        started: bool,
    },
    /// The file of task `id` is in the Netdisk, uploaded or by rapid upload,
    /// at `remote` when the line carries a readable path.
    Uploaded {
        id: &'a str,
        remote: Option<&'a str>,
    },
    /// The Netdisk already has the file of task `id`: a benign skip.
    Skipped { id: &'a str, reason: &'a str },
    /// BaiduPCS-Go never uploads the file of task `id` with this
    /// configuration: a file over the size limit, an unreadable file or an
    /// exhausted quota.
    Rejected { id: &'a str, reason: &'a str },
    /// The file whose path `reason` embeds has an illegal name and is never
    /// uploaded. The line carries a literal `[0]` prefix, not a task ID.
    IllegalName { reason: &'a str },
    /// The upload of task `id` failed in this run; another attempt may
    /// succeed.
    Failed { id: &'a str, reason: &'a str },
}

/// Classify a task line of `upload` output. Both [`parse_upload_outcomes`]
/// and the live [`UploadTracker`] read lines through it, so progress counts
/// a file as ended exactly when its outcome is resolved.
///
/// Marker precedence guards against the CLI's overloaded wording:
/// `跳过秒传失败, 开始秒传` matches neither the skip test (`, 跳过` /
/// `已跳过`) nor the failure test (which requires [`MARKER_UPLOAD_FAILED`]
/// without [`MARKER_RETRY`]), and `文件路径含有非法字符，已跳过` is a
/// permanent failure despite its skip wording.
fn parse_task_line(line: &str) -> Option<TaskLine<'_>> {
    let (id, rest) = parse_task_prefix(line)?;
    if let Some(path) = rest.strip_prefix(MARKER_QUEUED) {
        return Some(TaskLine::Named {
            id,
            path: path.trim(),
            started: false,
        });
    }
    if let Some(path) = rest.strip_prefix(MARKER_PREPARING) {
        return Some(TaskLine::Named {
            id,
            path: path.trim(),
            started: true,
        });
    }
    if rest.contains(MARKER_UPLOAD_SUCCESS) || rest.contains(MARKER_RAPID_SUCCESS) {
        let remote = rest
            .split(MARKER_REMOTE_PATH)
            .nth(1)
            .map(str::trim)
            .filter(|path| !path.is_empty());
        return Some(TaskLine::Uploaded { id, remote });
    }
    if rest.contains(", 跳过") || rest.contains("已跳过") {
        return Some(if rest.contains(MARKER_ILLEGAL) {
            TaskLine::IllegalName { reason: rest }
        } else if rest.contains(MARKER_EXISTS) || rest.contains(MARKER_UNCHANGED) {
            TaskLine::Skipped { id, reason: rest }
        } else {
            TaskLine::Rejected { id, reason: rest }
        });
    }
    if rest.contains(MARKER_UPLOAD_FAILED) && !rest.contains(MARKER_RETRY) {
        return Some(TaskLine::Failed { id, reason: rest });
    }
    None
}

/// Resolve per-file outcomes from the captured log lines of one `upload`
/// run over `inputs`.
///
/// Failures the CLI reports as skips of the file itself land in
/// `permanent`; transfer failures land in `failed`. Unrecognized lines are
/// ignored; inputs without any marker stay unresolved and the caller decides
/// between the orphan-success escape hatch and failing the attempt.
pub(super) fn parse_upload_outcomes(logs: &[JobLogEntry], inputs: &[String]) -> UploadOutcomes {
    let mut outcomes = UploadOutcomes::default();
    let mut task_to_input: HashMap<String, String> = HashMap::new();
    let mut in_failure_table = false;

    for entry in logs {
        let line = entry.message.trim();
        if line.is_empty() {
            continue;
        }

        if line.contains(MARKER_FAILURE_TABLE) {
            outcomes.saw_failure_table = true;
            in_failure_table = true;
            continue;
        }
        if line.contains(MARKER_FINISHED) {
            outcomes.finished = true;
            continue;
        }
        if line.contains(MARKER_NO_TASKS) {
            continue;
        }

        if let Some(task_line) = parse_task_line(line) {
            match task_line {
                TaskLine::Named { id, path, .. } => {
                    if let Some(input) = match_input(inputs, path) {
                        task_to_input.insert(id.to_string(), input.clone());
                    }
                }
                TaskLine::Uploaded { id, remote } => match task_to_input.get(id) {
                    Some(input) => {
                        outcomes
                            .completed
                            .insert(input.clone(), remote.map(str::to_string));
                    }
                    None => outcomes.orphan_successes += 1,
                },
                TaskLine::Skipped { id, reason } => match task_to_input.get(id) {
                    Some(input) => {
                        outcomes.skipped.insert(input.clone(), reason.to_string());
                    }
                    // A benign skip is a terminal success-like outcome, so an
                    // unmapped one feeds the same escape hatch as an unmapped
                    // success.
                    None => outcomes.orphan_successes += 1,
                },
                TaskLine::Rejected { id, reason } => match task_to_input.get(id) {
                    Some(input) => {
                        outcomes.permanent.insert(input.clone(), reason.to_string());
                    }
                    None => outcomes.orphan_failures += 1,
                },
                TaskLine::IllegalName { reason } => {
                    match match_input_by_containment(inputs, reason) {
                        Some(input) => {
                            outcomes.permanent.insert(input.clone(), reason.to_string());
                        }
                        None => outcomes.orphan_failures += 1,
                    }
                }
                TaskLine::Failed { id, reason } => match task_to_input.get(id) {
                    Some(input) => {
                        outcomes.failed.insert(input.clone(), reason.to_string());
                    }
                    None => outcomes.orphan_failures += 1,
                },
            }
            continue;
        }

        if in_failure_table && let Some(input) = match_input_by_containment(inputs, line) {
            outcomes
                .failed
                .entry(input.clone())
                .or_insert_with(|| format!("listed in the BaiduPCS-Go failure table: {line}"));
        }
    }

    outcomes
}

/// How often an `upload` invocation reports its progress at most.
const UPLOAD_REPORT_INTERVAL: Duration = Duration::from_secs(1);

/// Files and bytes of a job's upload, counted for its progress.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct UploadCounts {
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
}

impl UploadCounts {
    /// Counts over `inputs`, the files pending when the job started: those in
    /// `uploaded` or `skipped` are done, and those in `rejected`, which
    /// BaiduPCS-Go never uploads, are out of the totals. Other entries, such
    /// as inputs an earlier run of the job uploaded and removed, are not
    /// counted, so `files_done` never exceeds `files_total`.
    pub(super) fn of_job(
        inputs: &[String],
        sizes: &HashMap<String, u64>,
        uploaded: &HashMap<String, String>,
        skipped: &HashMap<String, String>,
        rejected: &HashMap<String, String>,
    ) -> Self {
        let mut counts = Self::default();
        for input in inputs {
            let size = sizes.get(input).copied().unwrap_or(0);
            counts.files_total += 1;
            counts.bytes_total += size;
            if uploaded.contains_key(input) || skipped.contains_key(input) {
                counts.end(size, true);
            } else if rejected.contains_key(input) {
                counts.end(size, false);
            }
        }
        counts
    }

    /// Count a file of `size` bytes as ended: done when it is in the Netdisk,
    /// otherwise out of the totals, so the percent can still reach 100.
    fn end(&mut self, size: u64, in_netdisk: bool) {
        if in_netdisk {
            self.files_done += 1;
            self.bytes_done += size;
        } else {
            self.files_total = self.files_total.saturating_sub(1);
            self.bytes_total = self.bytes_total.saturating_sub(size);
        }
    }

    pub(super) fn snapshot(&self) -> JobProgressSnapshot {
        let bytes_done = self.bytes_done.min(self.bytes_total);
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::BaiduPcs);
        snapshot.bytes_done = Some(bytes_done);
        snapshot.bytes_total = Some(self.bytes_total);
        snapshot.percent = (self.bytes_total > 0)
            .then(|| (bytes_done as f64 / self.bytes_total as f64 * 100.0) as f32);
        snapshot.files_done = Some(self.files_done);
        snapshot.files_total = Some(self.files_total);
        snapshot
    }
}

/// A task of one `upload` invocation that has started and not ended.
struct TaskInFlight {
    id: String,
    bytes_done: u64,
    speed_bytes_per_sec: Option<f64>,
    /// A bar arrived. Before that BaiduPCS-Go checks the Netdisk and hashes
    /// the file without progress output.
    sending: bool,
}

#[derive(Default)]
pub(super) struct TrackerState {
    /// The job's counts, including the files this invocation ended.
    counts: UploadCounts,
    /// Task ID → input, from the queue and start lines.
    names: HashMap<String, String>,
    in_flight: Vec<TaskInFlight>,
    /// Tasks that ended; a bar redrawn after the end must not restart one.
    ended: HashSet<String>,
    /// Something changed since the last report.
    changed: bool,
}

impl TrackerState {
    fn task(&mut self, id: &str) -> &mut TaskInFlight {
        let index = match self.in_flight.iter().position(|task| task.id == id) {
            Some(index) => index,
            None => {
                self.in_flight.push(TaskInFlight {
                    id: id.to_string(),
                    bytes_done: 0,
                    speed_bytes_per_sec: None,
                    sending: false,
                });
                self.in_flight.len() - 1
            }
        };
        &mut self.in_flight[index]
    }

    /// Task `id` ended; returns its input.
    fn end_task(&mut self, id: &str) -> Option<String> {
        self.in_flight.retain(|task| task.id != id);
        self.ended.insert(id.to_string());
        self.changed = true;
        self.names.remove(id)
    }

    pub(super) fn snapshot(&self) -> JobProgressSnapshot {
        let mut counts = self.counts;
        counts.bytes_done += self
            .in_flight
            .iter()
            .map(|task| task.bytes_done)
            .sum::<u64>();
        let mut snapshot = counts.snapshot();
        // BaiduPCS-Go uploads several files at once (four by default), each
        // with its own bar.
        snapshot.speed_bytes_per_sec = self
            .in_flight
            .iter()
            .filter_map(|task| task.speed_bytes_per_sec)
            .reduce(|sum, speed| sum + speed);
        snapshot.eta_secs = snapshot
            .speed_bytes_per_sec
            .filter(|speed| *speed > 0.0)
            .map(|speed| counts.bytes_total.saturating_sub(counts.bytes_done) as f64 / speed);
        // BaiduPCS-Go checks the Netdisk and hashes each file before sending
        // it, without progress output: minutes for a large recording on a
        // slow disk.
        if !self.in_flight.is_empty() && !self.in_flight.iter().any(|task| task.sending) {
            snapshot.phase = Some(JobPhase::Preparing);
        }
        // Naming one of several files in flight would read as the file that
        // "File N of M" counts.
        snapshot.current_file = match self.in_flight.as_slice() {
            [task] => self.names.get(&task.id).and_then(progress_file_name),
            _ => None,
        };
        snapshot
    }
}

/// Job-wide progress of one `upload` invocation, fed by the readers of its
/// stdout and stderr.
pub(super) struct UploadTracker {
    reporter: ProgressReporter,
    /// The invocation's inputs, which its task lines name.
    inputs: Vec<String>,
    sizes: HashMap<String, u64>,
    pub(super) state: Mutex<TrackerState>,
}

impl UploadTracker {
    /// `counts` are the job's counts before the invocation starts.
    pub(super) fn new(
        reporter: ProgressReporter,
        counts: UploadCounts,
        inputs: &[String],
        sizes: &HashMap<String, u64>,
    ) -> Self {
        Self {
            reporter,
            inputs: inputs.to_vec(),
            sizes: inputs
                .iter()
                .filter_map(|input| Some((input.clone(), *sizes.get(input)?)))
                .collect(),
            state: Mutex::new(TrackerState {
                counts,
                ..TrackerState::default()
            }),
        }
    }

    fn size(&self, input: &str) -> u64 {
        self.sizes.get(input).copied().unwrap_or(0)
    }

    /// A progress bar redraw of task `id`.
    pub(super) fn record_bar(&self, id: &str, bytes_done: u64, speed_bytes_per_sec: Option<f64>) {
        let mut state = self.state.lock();
        if state.ended.contains(id) {
            return;
        }
        let task = state.task(id);
        task.bytes_done = bytes_done;
        task.speed_bytes_per_sec = speed_bytes_per_sec;
        task.sending = true;
        state.changed = true;
    }

    /// An output line other than a progress bar.
    pub(super) fn record_line(&self, line: &str) {
        let Some(task_line) = parse_task_line(line.trim()) else {
            return;
        };
        let mut state = self.state.lock();
        match task_line {
            TaskLine::Named { id, path, started } => {
                if let Some(input) = match_input(&self.inputs, path) {
                    state.names.insert(id.to_string(), input.clone());
                }
                if started && !state.ended.contains(id) {
                    state.task(id);
                    state.changed = true;
                }
            }
            TaskLine::Uploaded { id, .. } | TaskLine::Skipped { id, .. } => {
                if let Some(input) = state.end_task(id) {
                    state.counts.end(self.size(&input), true);
                }
            }
            TaskLine::Rejected { id, .. } => {
                if let Some(input) = state.end_task(id) {
                    state.counts.end(self.size(&input), false);
                }
            }
            TaskLine::IllegalName { reason } => {
                if let Some(input) = match_input_by_containment(&self.inputs, reason) {
                    state.counts.end(self.size(input), false);
                    state.changed = true;
                }
            }
            // Another attempt may upload the file, so it stays pending.
            TaskLine::Failed { id, .. } => {
                state.end_task(id);
            }
        }
    }

    /// Run `work`, the invocation, reporting what changed at most once per
    /// [`UPLOAD_REPORT_INTERVAL`], and once more when it ends.
    pub(super) async fn report_while<T>(&self, work: impl Future<Output = T>) -> T {
        let mut ticks = tokio::time::interval_at(
            tokio::time::Instant::now() + UPLOAD_REPORT_INTERVAL,
            UPLOAD_REPORT_INTERVAL,
        );
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        tokio::pin!(work);
        let output = loop {
            tokio::select! {
                output = &mut work => break output,
                _ = ticks.tick() => {
                    let snapshot = {
                        let mut state = self.state.lock();
                        std::mem::take(&mut state.changed).then(|| state.snapshot())
                    };
                    if let Some(snapshot) = snapshot {
                        self.reporter.report(snapshot);
                    }
                }
            }
        };
        let snapshot = {
            let mut state = self.state.lock();
            // No task runs once the invocation's output ends.
            state.in_flight.clear();
            state.snapshot()
        };
        self.reporter.report(snapshot);
        output
    }
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
    // Parsed fields only — never the source line. Every report is kept in
    // progress_cache and copied on each progress read and WebSocket broadcast,
    // so raw must stay small and fixed-shape.
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
/// module consumes are declared; serde drops the rest at parse time.
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
    /// Files in flight; rclone leaves the key out when there are none.
    #[serde(default)]
    transferring: Vec<RcloneTransfer>,
    #[serde(default)]
    errors: Option<u64>,
    #[serde(default, rename = "lastError")]
    last_error: Option<String>,
    #[serde(default, rename = "fatalError")]
    fatal_error: Option<bool>,
}

/// A file in flight.
#[derive(Deserialize)]
struct RcloneTransfer {
    /// Path relative to the root of the transfer.
    #[serde(default)]
    name: Option<String>,
    /// Bytes of the file sent so far.
    #[serde(default)]
    bytes: Option<u64>,
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
    // Counted across the whole `--files-from` batch, so they describe the
    // job rather than the file in flight.
    snapshot.files_done = stats.transfers;
    snapshot.files_total = stats.total_transfers.filter(|total| *total > 0);
    // Naming one of several files in flight would read as the file that
    // "File N of M" counts.
    snapshot.current_file = match stats.transferring.as_slice() {
        [transfer] => transfer.name.as_deref().and_then(progress_file_name),
        _ => None,
    };
    // rclone's byte and transfer counts stay put while it prepares files and
    // while the destination confirms them. Before its first transfer it lists
    // the destination and compares the files already there, which can mean
    // reading a whole file to compare checksums; a file in flight may be
    // hashed before its first byte, for destinations that store a checksum
    // with the upload such as S3 multipart. Once every byte is sent, a
    // server that first stores the files elsewhere can take minutes to
    // confirm them.
    snapshot.phase = if stats.transferring.is_empty() {
        (stats.bytes.unwrap_or(0) == 0 && stats.transfers.unwrap_or(0) == 0)
            .then_some(JobPhase::Preparing)
    } else if stats
        .transferring
        .iter()
        .all(|transfer| transfer.bytes.unwrap_or(0) == 0)
    {
        Some(JobPhase::Preparing)
    } else {
        (snapshot.bytes_total.is_some() && snapshot.bytes_done == snapshot.bytes_total)
            .then_some(JobPhase::Finalizing)
    };
    // File counters only — see the raw-size note on parse_rclone_stats_line.
    snapshot.raw = serde_json::json!({
        "transfers": stats.transfers,
        "totalTransfers": stats.total_transfers,
    });
    snapshot
}

struct RcloneProgressDiagnostics {
    errors: u64,
    last_error: Option<String>,
    fatal_error: bool,
}

// Stack-only return value immediately matched and consumed by
// `consume_rclone_stderr` (never stored in collections); avoid boxing
// `JobProgressSnapshot` on every 1-second stats tick.
#[allow(clippy::large_enum_variant)]
enum ParsedRcloneLine {
    Progress {
        snapshot: JobProgressSnapshot,
        diagnostics: Option<RcloneProgressDiagnostics>,
    },
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
        if let Some(mut stats) = parsed.stats {
            let last_error = stats.last_error.take().and_then(|msg| {
                let trimmed = msg.trim();
                if trimmed.is_empty() {
                    None
                } else if trimmed.len() == msg.len() {
                    Some(msg)
                } else {
                    Some(trimmed.to_owned())
                }
            });
            let diagnostics = RcloneProgressDiagnostics {
                errors: stats.errors.unwrap_or(0),
                last_error,
                fatal_error: stats.fatal_error.unwrap_or(false),
            };
            return ParsedRcloneLine::Progress {
                snapshot: snapshot_from_json_stats(stats),
                diagnostics: Some(diagnostics),
            };
        }
        let level = map_rclone_json_level(parsed.level.as_deref());
        let message = match parsed.msg.map(|msg| msg.trim().to_string()) {
            Some(msg) if !msg.is_empty() => msg,
            _ => trimmed.to_string(),
        };
        return ParsedRcloneLine::Log(JobLogEntry::new(level, message));
    }

    if let Some(snapshot) = parse_rclone_stats_line(line) {
        return ParsedRcloneLine::Progress {
            snapshot,
            diagnostics: None,
        };
    }
    ParsedRcloneLine::Log(JobLogEntry::new(determine_rclone_log_level(line), line))
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

/// Reads rclone stderr to EOF, routing each line through
/// [`parse_rclone_line`]: stats entries go to `progress`, error transitions
/// inside JSON stats emit deduplicated warning entries and record `fatalError`
/// and `lastError` in `state`, and everything else goes to the bounded log
/// channel (`dropped_count` tracks `try_send` overflow). Overlong lines are
/// skipped and counted by [`CappedLines`].
pub(super) async fn consume_rclone_stderr<R: AsyncRead + Unpin>(
    reader: R,
    progress: ProgressReporter,
    state: Arc<Mutex<RcloneRunState>>,
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped_count: Arc<AtomicUsize>,
    overlong_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut lines = CappedLines::new(reader, overlong_count);
    let mut last_error_count = 0u64;
    let mut last_error_message: Option<String> = None;
    let mut saw_fatal_error = false;
    while let Some(line) = lines.next_line().await? {
        match parse_rclone_line(&line) {
            ParsedRcloneLine::Progress {
                snapshot,
                diagnostics,
            } => {
                progress.report(snapshot);
                if let Some(diag) = diagnostics {
                    // When a preset overrides `--retries > 1`, rclone resets
                    // `stats.errors` to 0 at the start of the next internal
                    // pass. Reset both count and last message whenever the
                    // counter drops so a new pass is surfaced even if no zero
                    // tick was observed before its first error.
                    if diag.errors < last_error_count {
                        last_error_count = 0;
                        last_error_message = None;
                    }
                    let fatal_flipped = diag.fatal_error && !saw_fatal_error;
                    let count_increased = diag.errors > last_error_count;
                    let message_changed =
                        diag.last_error.is_some() && diag.last_error != last_error_message;
                    if fatal_flipped || message_changed {
                        let mut run_state = state.lock();
                        if fatal_flipped {
                            saw_fatal_error = true;
                            run_state.fatal_error = true;
                        }
                        if message_changed && let Some(msg) = &diag.last_error {
                            run_state.last_stats_error = Some(msg.clone());
                        }
                    }
                    if count_increased || message_changed {
                        last_error_count = diag.errors;
                        let message = if let Some(msg) = diag.last_error {
                            let formatted = if last_error_count > 0 {
                                format!("rclone transfer error (#{last_error_count}): {msg}")
                            } else {
                                format!("rclone transfer error: {msg}")
                            };
                            last_error_message = Some(msg);
                            formatted
                        } else {
                            format!("rclone reported {last_error_count} transfer error(s)")
                        };
                        if tx.try_send(JobLogEntry::warn(message)).is_err() {
                            dropped_count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
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
pub(super) async fn run_rclone_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<JobLogSink>,
) -> srec_core::CoreResult<CommandOutput> {
    run_logged_command(
        command,
        CommandMode::Rclone {
            progress: progress.clone(),
            state: Arc::default(),
        },
        log_sink,
    )
    .await
}

/// Hard ceiling on captured `--json` stdout from an `rship` process.
/// `rship` already caps `files` in its `RunReport` at 10,000 entries, so valid
/// summaries fit comfortably within a few megabytes.
const MAX_RSHIP_STDOUT_BYTES: usize = 8 * 1024 * 1024;

/// Output of an `rship` invocation: captured stderr logs plus the `--json`
/// summary written to stdout when the report was not truncated.
pub(super) struct RshipCommandOutput {
    pub(super) output: CommandOutput,
    pub(super) stdout_json: Option<String>,
}

/// One `--json-progress` JSONL event emitted on `rship` stderr (`progress` or
/// `error`/`conflict`).
#[derive(Deserialize)]
struct RshipProgressLine {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    bytes_done: Option<u64>,
    #[serde(default)]
    bytes_total: Option<u64>,
    #[serde(default)]
    speed_bps: Option<u64>,
    #[serde(default)]
    eta_secs: Option<u64>,
    #[serde(default)]
    files_done: Option<u64>,
    #[serde(default)]
    files_total: Option<u64>,
    #[serde(default)]
    files_planned: Option<u64>,
    #[serde(default)]
    files_skipped: Option<u64>,
    #[serde(default)]
    active_transfers: Option<usize>,
    #[serde(default)]
    current_file: Option<String>,
    #[serde(default)]
    active_files: Vec<RshipActiveFile>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default, alias = "error")]
    message: Option<String>,
}

#[derive(Deserialize)]
struct RshipActiveFile {
    #[serde(default)]
    path: Option<String>,
}

#[allow(clippy::large_enum_variant)]
enum ParsedRshipLine {
    Progress(JobProgressSnapshot),
    Log(JobLogEntry),
}

fn snapshot_from_rship_progress(p: RshipProgressLine) -> JobProgressSnapshot {
    let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rship);
    let bytes_done = p.bytes_done;
    let bytes_total = p.bytes_total.filter(|&total| total > 0);
    snapshot.bytes_done = bytes_done;
    snapshot.bytes_total = bytes_total;

    let planned_done = p.files_done.unwrap_or(0);
    let files_finished = planned_done.saturating_add(p.files_skipped.unwrap_or(0));
    let files_total = p.files_total.filter(|&total| total > 0);
    snapshot.files_done = Some(files_finished);
    snapshot.files_total = files_total;

    let phase_str = p.phase.as_deref().unwrap_or_default();
    snapshot.percent = match (bytes_done, bytes_total) {
        (Some(done), Some(total)) => {
            Some((done as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32)
        }
        _ if phase_str == "completed" => Some(100.0),
        _ => match files_total {
            Some(total) if files_finished > 0 => {
                Some((files_finished as f64 / total as f64 * 100.0).clamp(0.0, 100.0) as f32)
            }
            _ => None,
        },
    };

    snapshot.speed_bytes_per_sec = p.speed_bps.map(|bps| bps as f64);
    snapshot.eta_secs = p.eta_secs.map(|eta| eta as f64);

    let active_count = p.active_transfers.unwrap_or(p.active_files.len());
    snapshot.current_file = if active_count == 1 {
        p.active_files
            .first()
            .and_then(|item| item.path.as_deref())
            .or(p.current_file.as_deref())
            .and_then(progress_file_name)
    } else {
        None
    };

    snapshot.phase = match phase_str {
        "preparing" | "observing" | "creating_dirs" => Some(JobPhase::Preparing),
        "finalizing" | "deleting" => Some(JobPhase::Finalizing),
        "transferring" => {
            if bytes_done.unwrap_or(0) == 0 && planned_done == 0 {
                Some(JobPhase::Preparing)
            } else if bytes_total.is_some() && bytes_done == bytes_total && active_count > 0 {
                Some(JobPhase::Finalizing)
            } else {
                None
            }
        }
        _ => None,
    };

    snapshot.raw = serde_json::json!({
        "files_done": p.files_done,
        "files_planned": p.files_planned,
        "files_skipped": p.files_skipped,
        "files_total": p.files_total,
    });
    snapshot
}

fn determine_rship_log_level(line: &str) -> LogLevel {
    let trimmed = line.trim_start();
    if trimmed.starts_with('✗')
        || trimmed.starts_with("├─")
        || trimmed.starts_with("└─")
        || trimmed.starts_with('│')
        || trimmed.starts_with("error:")
        || trimmed.contains(" ERROR ")
        || trimmed.starts_with("ERROR ")
    {
        LogLevel::Error
    } else if trimmed.starts_with('▲')
        || trimmed.starts_with("conflict ")
        || trimmed.starts_with("hint:")
        || trimmed.contains(" WARN ")
        || trimmed.starts_with("WARN ")
    {
        LogLevel::Warn
    } else if trimmed.contains(" DEBUG ")
        || trimmed.starts_with("DEBUG ")
        || trimmed.contains(" TRACE ")
        || trimmed.starts_with("TRACE ")
    {
        LogLevel::Debug
    } else {
        LogLevel::Info
    }
}

fn parse_rship_stderr_line(line: &str) -> ParsedRshipLine {
    let trimmed = line.trim();
    if trimmed.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<RshipProgressLine>(trimmed)
    {
        match parsed.event.as_deref() {
            Some("progress") => {
                return ParsedRshipLine::Progress(snapshot_from_rship_progress(parsed));
            }
            Some("error") => {
                let text = match (parsed.path.as_deref(), parsed.message.as_deref()) {
                    (Some(path), Some(msg)) => format!("rship {path}: {msg}"),
                    (None, Some(msg)) => format!("rship: {msg}"),
                    _ => trimmed.to_owned(),
                };
                return ParsedRshipLine::Log(JobLogEntry::error(text));
            }
            Some("conflict" | "warn" | "warning") => {
                let text = match (parsed.path.as_deref(), parsed.message.as_deref()) {
                    (Some(path), Some(msg)) => format!("rship {path}: {msg}"),
                    (None, Some(msg)) => format!("rship: {msg}"),
                    _ => trimmed.to_owned(),
                };
                return ParsedRshipLine::Log(JobLogEntry::warn(text));
            }
            _ => {}
        }
    }
    ParsedRshipLine::Log(JobLogEntry::new(
        determine_rship_log_level(trimmed),
        trimmed,
    ))
}

/// Reads `rship` stdout into `state.stdout` up to [`MAX_RSHIP_STDOUT_BYTES`]
/// and drains any remainder so the child process never blocks on a full pipe.
pub(super) async fn consume_rship_stdout<R: AsyncRead + Unpin>(
    mut reader: R,
    state: Arc<Mutex<RshipRunState>>,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        if !truncated {
            if buf.len() + n <= MAX_RSHIP_STDOUT_BYTES {
                buf.extend_from_slice(&chunk[..n]);
            } else {
                truncated = true;
                buf.clear();
            }
        }
    }
    let mut guard = state.lock();
    guard.stdout_truncated = truncated;
    if !truncated && !buf.is_empty() {
        guard.stdout = String::from_utf8_lossy(&buf).into_owned();
    }
    Ok(())
}

/// Reads `rship` stderr to EOF: `--json-progress` lines become progress
/// snapshots, and diagnostic lines go to the bounded job-log channel.
pub(super) async fn consume_rship_stderr<R: AsyncRead + Unpin>(
    reader: R,
    progress: ProgressReporter,
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped_count: Arc<AtomicUsize>,
    overlong_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut lines = CappedLines::new(reader, overlong_count);
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        match parse_rship_stderr_line(&line) {
            ParsedRshipLine::Progress(snapshot) => {
                progress.report(snapshot);
            }
            ParsedRshipLine::Log(entry) => {
                if tx.try_send(entry).is_err() {
                    dropped_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    Ok(())
}

/// Run an `rship` command configured with `--json --json-progress --stats 1s`,
/// streaming JSONL progress from stderr, capturing the final JSON report from
/// stdout, and reaping the child process when `stop` is cancelled so `rship`'s
/// exclusive `ProcessLock` is released before any subsequent attempt starts.
pub(super) async fn run_rship_with_progress(
    command: &mut Command,
    progress: &ProgressReporter,
    log_sink: Option<JobLogSink>,
    stop: &tokio_util::sync::CancellationToken,
) -> srec_core::CoreResult<RshipCommandOutput> {
    let state = Arc::new(Mutex::new(RshipRunState::default()));
    let output = run_logged_command_until(
        command,
        CommandMode::Rship {
            progress: progress.clone(),
            state: state.clone(),
        },
        log_sink,
        Some(stop),
    )
    .await?;
    let stdout_json = {
        let mut guard = state.lock();
        if guard.stdout_truncated || guard.stdout.trim().is_empty() {
            None
        } else {
            Some(std::mem::take(&mut guard.stdout))
        }
    };
    Ok(RshipCommandOutput {
        output,
        stdout_json,
    })
}

/// Minimum seconds between progress-bar lines forwarded to the live log
/// sink by [`run_baidupcs_with_logs`]: BaiduPCS-Go redraws each file's bar
/// every 3 seconds and uploads several files at once (four by default).
const BAIDUPCS_NOISE_INTERVAL_SECS: u64 = 5;

/// True for BaiduPCS-Go upload progress-bar redraws
/// (`[<id>] ↑ <done>/<total> <speed>/s in <elapsed> ...`). Outcome marker
/// lines (加入上传队列 / 上传文件成功 / 跳过 / 上传结束 / failure table)
/// never contain the `↑` glyph, so shape alone identifies the bar.
fn is_baidupcs_progress_noise(line: &str) -> bool {
    line.contains('↑') && line.contains("/s")
}

/// End of the bar BaiduPCS-Go prints when it uploads one file at a time.
/// That bar starts with `\r` and has no newline after it, so BaiduPCS-Go's
/// next output follows these dots.
pub(super) const BAIDUPCS_BAR_END: &str = "............";

/// One redraw of a BaiduPCS-Go upload task's progress bar.
struct BaiduPcsBar<'a> {
    task_id: &'a str,
    bytes_done: u64,
    speed_bytes_per_sec: Option<f64>,
}

fn parse_baidupcs_progress_line(line: &str) -> Option<BaiduPcsBar<'_>> {
    let (task_id, bar) = parse_task_prefix(line)?;
    let mut parts = bar.strip_prefix('↑')?.split_whitespace();
    let (done, _total) = parts.next()?.split_once('/')?;
    Some(BaiduPcsBar {
        task_id,
        bytes_done: parse_size_to_bytes(done)?,
        speed_bytes_per_sec: parts.next().and_then(parse_speed_to_bytes_per_sec),
    })
}

/// Log level for a BaiduPCS-Go output line. Cosmetic for the job-log UI
/// only; `BaiduPcsProcessor` re-parses the captured lines with precise
/// markers to resolve per-file outcomes, so mislevelled lines (e.g. the
/// benign `跳过秒传失败, 开始秒传` note) cannot flip an upload status.
fn determine_baidupcs_log_level(line: &str) -> LogLevel {
    // The CLI's own per-file retry notice (`[<id>] 上传文件失败, 重试 <n>/<max>`)
    // is not a failure yet.
    if line.contains("失败") && line.contains("重试") {
        LogLevel::Warn
    } else if line.contains("失败") || line.contains("错误") || line.contains("ERROR") {
        LogLevel::Error
    } else if line.contains("警告") || line.contains("WARNING") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

/// Reads one BaiduPCS-Go output stream to EOF through a CR-splitting
/// [`CappedLines`], feeding every line to `tracker`. Progress-bar redraws go
/// straight to `noise_sink` at most once per [`BAIDUPCS_NOISE_INTERVAL_SECS`]
/// and are never captured; empty segments produced by `\r\n` are dropped,
/// and every other line is sent to the bounded log channel for capture.
pub(super) async fn consume_baidupcs_stream<R: AsyncRead + Unpin>(
    reader: R,
    tracker: Option<Arc<UploadTracker>>,
    noise_sink: Option<JobLogSink>,
    tx: tokio::sync::mpsc::Sender<JobLogEntry>,
    dropped_count: Arc<AtomicUsize>,
    overlong_count: Arc<AtomicUsize>,
) -> std::io::Result<()> {
    let mut lines = CappedLines::with_cr_splitting(reader, overlong_count);
    let mut last_noise: Option<std::time::Instant> = None;
    while let Some(segment) = lines.next_line().await? {
        // A line printed right after a bar ending in `BAIDUPCS_BAR_END`, such
        // as a failure, shares the bar's segment when both arrive in one read.
        let (bar, line) = match segment.split_once(BAIDUPCS_BAR_END) {
            Some((bar, rest)) if is_baidupcs_progress_noise(bar) => (Some(bar.trim()), rest.trim()),
            _ if is_baidupcs_progress_noise(&segment) => (Some(segment.trim()), ""),
            _ => (None, segment.trim()),
        };
        if let Some(bar) = bar {
            if let Some(tracker) = &tracker
                && let Some(bar) = parse_baidupcs_progress_line(bar)
            {
                tracker.record_bar(bar.task_id, bar.bytes_done, bar.speed_bytes_per_sec);
            }
            let now = std::time::Instant::now();
            if last_noise.is_none_or(|prev| {
                now.duration_since(prev).as_secs() >= BAIDUPCS_NOISE_INTERVAL_SECS
            }) {
                last_noise = Some(now);
                if let Some(sink) = &noise_sink {
                    sink.try_send(JobLogEntry::info(bar));
                }
            }
        }
        if line.is_empty() {
            continue;
        }
        if let Some(tracker) = &tracker {
            tracker.record_line(line);
        }
        let level = determine_baidupcs_log_level(line);
        if tx.try_send(JobLogEntry::new(level, line)).is_err() {
            dropped_count.fetch_add(1, Ordering::Relaxed);
        }
    }
    Ok(())
}

/// Run a BaiduPCS-Go command, capturing stdout and stderr as logs.
/// Both streams are split on `\r` as well as `\n` because the upload
/// progress bar redraws with bare `\r`; bar redraws are throttled to the
/// live `log_sink` and excluded from `CommandOutput::logs`, so hours of
/// redraws cannot evict the per-file outcome markers that
/// `BaiduPcsProcessor` resolves upload statuses from. Both streams of an
/// `upload` invocation feed its `tracker`.
pub(super) async fn run_baidupcs_with_logs(
    command: &mut Command,
    tracker: Option<Arc<UploadTracker>>,
    log_sink: Option<JobLogSink>,
    stop: &tokio_util::sync::CancellationToken,
) -> srec_core::CoreResult<CommandOutput> {
    run_logged_command_until(
        command,
        CommandMode::BaiduPcs(tracker),
        log_sink,
        Some(stop),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::super::utils::MAX_LINE_BYTES;
    use super::*;

    // Captured from rclone v1.72.1 stderr under the flag set built by
    // RcloneProcessor (`--use-json-log --stats 1s --stats-log-level NOTICE`).
    const RCLONE_JSON_STATS_LINE: &str = r#"{"time":"2026-07-26T14:06:28.7986513+02:00","level":"notice","msg":"50.059 MiB / 100 MiB, 50%, 25.090 MiB/s, ETA 1s","stats":{"bytes":229376,"checks":0,"deletedDirs":0,"deletes":0,"elapsedTime":1.999943,"errors":0,"eta":2,"fatalError":false,"listed":1,"renames":0,"retryError":false,"speed":131072,"totalBytes":508811,"totalChecks":0,"totalTransfers":1,"transferTime":1.999943,"transferring":[{"bytes":229376,"eta":2,"group":"global_stats","name":"clip.mp4","percentage":45,"size":508811,"speed":114720.5,"speedAvg":131072}],"transfers":0},"source":"slog/logger.go:256"}"#;

    #[test]
    fn parse_rclone_line_json_stats_becomes_progress() {
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(RCLONE_JSON_STATS_LINE)
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
        assert_eq!(
            (
                snapshot.files_done,
                snapshot.files_total,
                snapshot.current_file.as_deref()
            ),
            (Some(0), Some(1), Some("clip.mp4"))
        );
        // Only the file counters survive into raw; `transferring` and the
        // human msg must not.
        assert_eq!(
            snapshot.raw,
            serde_json::json!({"transfers": 0, "totalTransfers": 1})
        );
    }

    #[test]
    fn parse_rclone_line_names_the_file_in_flight_without_its_directories() {
        let line = r#"{"level":"notice","stats":{"bytes":10,"totalBytes":40,"transferring":[{"name":"day/sub/clip2.mp4","bytes":10}],"transfers":1,"totalTransfers":2}}"#;
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(line) else {
            panic!("stats JSON line should parse as progress");
        };
        assert_eq!(snapshot.current_file.as_deref(), Some("clip2.mp4"));

        let line = r#"{"level":"notice","stats":{"bytes":10,"totalBytes":40,"transferring":[{"name":"clip2.mp4","bytes":6},{"name":"clip3.mp4","bytes":4}],"transfers":0,"totalTransfers":2}}"#;
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(line) else {
            panic!("stats JSON line with two transfers in flight should parse as progress");
        };
        assert_eq!(snapshot.current_file, None);

        // rclone leaves `transferring` out when nothing is in flight, as on
        // its final line.
        let line = r#"{"level":"notice","stats":{"bytes":40,"totalBytes":40,"speed":20,"eta":0,"transfers":2,"totalTransfers":2}}"#;
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(line) else {
            panic!("stats JSON line without transfers in flight should parse as progress");
        };
        assert_eq!(
            (snapshot.percent, snapshot.current_file),
            (Some(100.0), None)
        );
    }

    #[test]
    fn parse_rclone_line_json_stats_with_unknown_totals() {
        let line = r#"{"level":"notice","msg":"early tick","stats":{"bytes":0,"totalBytes":0,"speed":0,"eta":null,"transfers":0,"totalTransfers":0}}"#;
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(line) else {
            panic!("stats JSON line should parse as progress");
        };

        assert_eq!(snapshot.bytes_done, Some(0));
        assert_eq!(snapshot.bytes_total, None, "totalBytes=0 means unknown");
        assert_eq!(snapshot.percent, None);
        assert_eq!(snapshot.eta_secs, None);
        assert_eq!(snapshot.files_total, None, "totalTransfers=0 means unknown");
    }

    #[test]
    fn parse_rclone_line_json_stats_phases() {
        let phase = |stats: &str| {
            let line = format!(r#"{{"level":"notice","stats":{stats}}}"#);
            let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(&line) else {
                panic!("stats JSON line should parse as progress");
            };
            snapshot.phase
        };
        // Listing the destination, comparing a file already there, and files
        // in flight before their first byte, at the start and after another
        // file was sent.
        for stats in [
            r#"{"bytes":0,"totalBytes":0,"checks":0,"totalChecks":0,"transfers":0,"totalTransfers":0}"#,
            r#"{"bytes":0,"totalBytes":0,"checks":0,"totalChecks":1,"checking":["rec.flv"],"transfers":0,"totalTransfers":0}"#,
            r#"{"bytes":0,"totalBytes":67108864,"transfers":0,"totalTransfers":1,"transferring":[{"name":"rec.flv","bytes":0}]}"#,
            r#"{"bytes":40,"totalBytes":80,"transfers":1,"totalTransfers":2,"transferring":[{"name":"b.flv","bytes":0}]}"#,
        ] {
            assert_eq!(phase(stats), Some(JobPhase::Preparing), "{stats}");
        }
        // Files sending, one of them before its first byte, and a batch
        // comparing its remaining files after sending one.
        for stats in [
            r#"{"bytes":1077248,"totalBytes":67108864,"transfers":0,"totalTransfers":1,"transferring":[{"name":"rec.flv","bytes":1077248}]}"#,
            r#"{"bytes":10,"totalBytes":80,"transfers":0,"totalTransfers":2,"transferring":[{"name":"a.flv","bytes":10},{"name":"b.flv","bytes":0}]}"#,
            r#"{"bytes":40,"totalBytes":40,"checks":0,"totalChecks":1,"checking":["b.flv"],"transfers":1,"totalTransfers":1}"#,
        ] {
            assert_eq!(phase(stats), None, "{stats}");
        }
        // Every byte sent, waiting for the destination to confirm the file.
        assert_eq!(
            phase(
                r#"{"bytes":67108864,"totalBytes":67108864,"transfers":0,"totalTransfers":1,"transferring":[{"name":"rec.flv","bytes":67108864}]}"#
            ),
            Some(JobPhase::Finalizing)
        );
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
    fn parse_rclone_line_legacy_transferred_text() {
        let line = "Transferred:   1.500 MiB / 3 MiB, 50%, 512 KiB/s, ETA 3s";
        let ParsedRcloneLine::Progress { snapshot, .. } = parse_rclone_line(line) else {
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

    #[tokio::test]
    async fn consume_rclone_stderr_skips_overlong_lines_and_resumes() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 1024];
        input.push(b'\n');
        input.extend_from_slice(RCLONE_JSON_STATS_LINE.as_bytes());
        input.push(b'\n');
        input.extend_from_slice(b"plain log line\n");

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
        let progress = ProgressReporter::new("job-1", progress_tx);
        let state = Arc::new(Mutex::new(RcloneRunState::default()));
        let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<JobLogEntry>(8);
        let dropped = Arc::new(AtomicUsize::new(0));
        let overlong = Arc::new(AtomicUsize::new(0));

        consume_rclone_stderr(
            input.as_slice(),
            progress,
            state.clone(),
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

    #[tokio::test]
    async fn consume_rclone_stderr_surfaces_stats_error_transitions_and_fatal_flag() {
        let stream = concat!(
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":0,"fatalError":false}}"#,
            "\n",
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":1,"lastError":"googleapi: Error 403: rateLimitExceeded","fatalError":false}}"#,
            "\n",
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":1,"lastError":"googleapi: Error 403: rateLimitExceeded","fatalError":false}}"#,
            "\n",
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":2,"lastError":"googleapi: Error 403: rateLimitExceeded","fatalError":false}}"#,
            "\n",
            // Next pass starts directly at `errors: 1` with the same `lastError`
            // (no intervening `errors: 0` tick observed); must still surface.
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":1,"lastError":"googleapi: Error 403: rateLimitExceeded","fatalError":false}}"#,
            "\n",
            // Next pass starts via an explicit `errors: 0` tick, then hits fatal.
            r#"{"level":"notice","stats":{"bytes":0,"totalBytes":1000,"errors":0,"fatalError":false}}"#,
            "\n",
            r#"{"level":"notice","stats":{"bytes":100,"totalBytes":1000,"errors":1,"lastError":"googleapi: Error 403: userRateLimitExceeded","fatalError":true}}"#,
            "\n",
        );

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::channel(8);
        let progress = ProgressReporter::new("job-2", progress_tx);
        let state = Arc::new(Mutex::new(RcloneRunState::default()));
        let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<JobLogEntry>(8);

        consume_rclone_stderr(
            stream.as_bytes(),
            progress,
            state.clone(),
            log_tx,
            Arc::default(),
            Arc::default(),
        )
        .await
        .expect("stream drains");

        let mut logs = Vec::new();
        while let Ok(entry) = log_rx.try_recv() {
            logs.push((entry.level, entry.message));
        }

        assert_eq!(
            logs,
            vec![
                (
                    LogLevel::Warn,
                    "rclone transfer error (#1): googleapi: Error 403: rateLimitExceeded"
                        .to_string(),
                ),
                (
                    LogLevel::Warn,
                    "rclone transfer error (#2): googleapi: Error 403: rateLimitExceeded"
                        .to_string(),
                ),
                (
                    LogLevel::Warn,
                    "rclone transfer error (#1): googleapi: Error 403: rateLimitExceeded"
                        .to_string(),
                ),
                (
                    LogLevel::Warn,
                    "rclone transfer error (#1): googleapi: Error 403: userRateLimitExceeded"
                        .to_string(),
                ),
            ]
        );
        let run_state = state.lock();
        assert!(run_state.fatal_error);
        assert_eq!(
            run_state.last_stats_error.as_deref(),
            Some("googleapi: Error 403: userRateLimitExceeded")
        );
    }

    #[tokio::test]
    async fn consume_baidupcs_stream_throttles_bar_and_keeps_markers() {
        // BaiduPCS-Go's output while uploading one file: each bar starts with
        // `\r` and ends with dots, and the retry notice follows them directly.
        let input = "[1] 加入上传队列: D:\\rec\\a.flv\n\
                     \r[1] ↑ 1.00MB/9.00MB 1.00MB/s in 3s ............\
                     \r[1] ↑ 2.00MB/9.00MB 1.00MB/s in 6s ............\
                     [1] 上传文件失败, 网络请求错误, 重试 1/3\n\
                     \r[1] ↑ 3.00MB/9.00MB 1.00MB/s in 3s ............\n\
                     [1] 上传文件成功, 保存到网盘路径: /rec/a.flv\n"
            .as_bytes();

        let (log_tx, mut log_rx) = tokio::sync::mpsc::channel::<JobLogEntry>(16);
        let dropped = Arc::new(AtomicUsize::new(0));
        let overlong = Arc::new(AtomicUsize::new(0));

        consume_baidupcs_stream(input, None, None, log_tx, dropped.clone(), overlong.clone())
            .await
            .expect("stream drains");

        let mut messages = Vec::new();
        while let Ok(entry) = log_rx.try_recv() {
            messages.push(entry.message);
        }

        // Bar redraws never reach the captured-log channel; markers and the
        // retry notice do.
        assert_eq!(
            messages,
            vec![
                "[1] 加入上传队列: D:\\rec\\a.flv".to_string(),
                "[1] 上传文件失败, 网络请求错误, 重试 1/3".to_string(),
                "[1] 上传文件成功, 保存到网盘路径: /rec/a.flv".to_string(),
            ]
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn baidupcs_tracker_reports_job_wide_progress_once_a_second() {
        use tokio::io::AsyncWriteExt;

        const MIB: u64 = 1024 * 1024;
        let inputs = ["/rec/a.flv".to_string(), "/rec/b.flv".to_string()];
        let sizes = std::collections::HashMap::from([
            (inputs[0].clone(), 100 * MIB),
            (inputs[1].clone(), 300 * MIB),
        ]);
        let none = std::collections::HashMap::new();
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel(8);
        let tracker = Arc::new(UploadTracker::new(
            ProgressReporter::new("job", progress_tx),
            UploadCounts::of_job(&inputs, &sizes, &none, &none, &none),
            &inputs,
            &sizes,
        ));

        // The dots end each bar, so the last one written is read as it
        // arrives. Two tasks share the stream here only to exercise the
        // job-wide sums.
        let start = tokio::time::Instant::now();
        let script = [
            (
                250,
                "[1] 加入上传队列: /rec/a.flv\n[2] 加入上传队列: /rec/b.flv\n\
                 [1] 准备上传: /rec/a.flv\n[2] 准备上传: /rec/b.flv\n\
                 \r[1] ↑ 10.00MB/100.00MB 5.00MB/s in 2s ............\
                 \r[2] ↑ 30.00MB/300.00MB 10.00MB/s in 3s ............\
                 \r[1] ↑ 20.00MB/100.00MB 5.00MB/s in 4s ............\
                 \r[2] ↑ 60.00MB/300.00MB 10.00MB/s in 6s ............",
            ),
            (
                1250,
                "\r[1] ↑ 100.00MB/100.00MB 5.00MB/s in 20s ............\
                 \r[2] ↑ 120.00MB/300.00MB 12.00MB/s in 8s ............\
                 \n[1] 上传文件成功, 保存到网盘路径: /rec/a.flv\n\
                 \r[2] ↑ 150.00MB/300.00MB 12.00MB/s in 10s ............",
            ),
            (
                2250,
                "\n[2] 秒传成功, 保存到网盘路径: /rec/b.flv\n\
                 \n上传结束, 时间: 3s, 总大小: 400.00MB\n",
            ),
        ];
        let (mut output, reader) = tokio::io::duplex(4096);
        let write = async move {
            for (at_ms, text) in script {
                tokio::time::sleep_until(start + std::time::Duration::from_millis(at_ms)).await;
                output.write_all(text.as_bytes()).await.unwrap();
            }
        };
        let (log_tx, _log_rx) = tokio::sync::mpsc::channel(16);
        let read = consume_baidupcs_stream(
            reader,
            Some(tracker.clone()),
            None,
            log_tx,
            Arc::default(),
            Arc::default(),
        );
        let (read, ()) = tokio::join!(tracker.report_while(read), write);
        read.unwrap();

        let reports: Vec<_> = std::iter::from_fn(|| progress_rx.try_recv().ok())
            .map(|update| {
                let s = update.snapshot;
                (
                    (s.files_done, s.files_total),
                    (s.bytes_done, s.bytes_total, s.percent),
                    (s.speed_bytes_per_sec, s.eta_secs),
                    s.current_file,
                )
            })
            .collect();
        let mib_per_sec = |speed: f64| Some(speed * MIB as f64);
        assert_eq!(
            reports,
            [
                // One report for the first second's burst of bars, with both
                // files' bytes and speeds; two files in flight, so none is
                // named.
                (
                    (Some(0), Some(2)),
                    (Some(80 * MIB), Some(400 * MIB), Some(20.0)),
                    (mib_per_sec(15.0), Some(320.0 / 15.0)),
                    None,
                ),
                (
                    (Some(1), Some(2)),
                    (Some(250 * MIB), Some(400 * MIB), Some(62.5)),
                    (mib_per_sec(12.0), Some(150.0 / 12.0)),
                    Some("b.flv".to_string()),
                ),
                // When the output ends.
                (
                    (Some(2), Some(2)),
                    (Some(400 * MIB), Some(400 * MIB), Some(100.0)),
                    (None, None),
                    None,
                ),
            ]
        );
    }

    /// rclone prints plain-text errors before it switches to JSON logging;
    /// they reach the job log at their own level, and anything else as info.
    #[test]
    fn parse_rclone_line_plain_text_is_log() {
        for (line, level) in [
            (
                "2026/07/26 12:00:00 ERROR : clip.mp4: Failed to copy",
                LogLevel::Error,
            ),
            ("just some output", LogLevel::Info),
        ] {
            let ParsedRcloneLine::Log(entry) = parse_rclone_line(line) else {
                panic!("plain text should parse as log: {line}");
            };
            assert_eq!(entry.level, level, "{line}");
        }
    }

    /// BaiduPCS-Go progress bars are dropped from the job log; an upload
    /// failure is an error unless a retry follows, and success is info.
    #[test]
    fn baidupcs_noise_and_level_classification() {
        assert!(is_baidupcs_progress_noise(
            "[1] ↑ 12.00MB/120.00MB 3.50MB/s in 3s ............"
        ));
        assert!(!is_baidupcs_progress_noise(
            "[1] 上传文件成功, 保存到网盘路径: /rec/a.flv"
        ));
        assert!(!is_baidupcs_progress_noise("[1] 加入上传队列: /rec/a.flv"));

        for (line, level) in [
            ("[1] 上传文件失败, 上传文件错误", LogLevel::Error),
            ("[1] 上传文件失败, 上传文件错误, 重试 1/3", LogLevel::Warn),
            ("警告: 遍历错误: xx", LogLevel::Error),
            ("[1] 上传文件成功, 保存到网盘路径: /a", LogLevel::Info),
        ] {
            assert_eq!(determine_baidupcs_log_level(line), level, "{line}");
        }
    }

    #[tokio::test]
    async fn rship_stderr_progress_and_stdout_report_handling() {
        // 1. Preparing / transferring / finalizing / completed progress ticks + current_file rules.
        let ParsedRshipLine::Progress(preparing) = parse_rship_stderr_line(
            r#"{"event":"progress","phase":"observing","bytes_done":0,"bytes_total":0,"files_done":0,"files_total":0,"files_skipped":0,"active_transfers":0,"active_files":[]}"#,
        ) else {
            panic!("observing JSONL tick must parse as progress");
        };
        assert_eq!(preparing.kind, ProgressKind::Rship);
        assert_eq!(preparing.phase, Some(JobPhase::Preparing));
        assert_eq!(preparing.bytes_total, None);
        assert_eq!(preparing.files_total, None);
        assert_eq!(preparing.percent, None);

        let ParsedRshipLine::Progress(single_in_flight) = parse_rship_stderr_line(
            r#"{"event":"progress","phase":"transferring","bytes_done":52428800,"bytes_total":104857600,"speed_bps":10485760,"eta_secs":5,"files_done":1,"files_skipped":2,"files_total":4,"active_transfers":1,"active_files":[{"path":"nested/dir/seg04.mp4"}]}"#,
        ) else {
            panic!("transferring JSONL tick must parse as progress");
        };
        assert_eq!(single_in_flight.phase, None);
        assert_eq!(single_in_flight.percent, Some(50.0));
        assert_eq!(single_in_flight.files_done, Some(3));
        assert_eq!(single_in_flight.files_total, Some(4));
        assert_eq!(single_in_flight.speed_bytes_per_sec, Some(10_485_760.0));
        assert_eq!(single_in_flight.eta_secs, Some(5.0));
        assert_eq!(single_in_flight.current_file.as_deref(), Some("seg04.mp4"));

        let ParsedRshipLine::Progress(multi_in_flight) = parse_rship_stderr_line(
            r#"{"event":"progress","phase":"transferring","bytes_done":100,"bytes_total":100,"files_done":0,"files_total":2,"active_transfers":2,"active_files":[{"path":"a.mp4"},{"path":"b.mp4"}]}"#,
        ) else {
            panic!("finalizing-bytes JSONL tick must parse as progress");
        };
        assert_eq!(multi_in_flight.current_file, None);
        assert_eq!(multi_in_flight.phase, Some(JobPhase::Finalizing));

        // 2. Structured JSONL `error` events and plain diagnostic lines preserve severity.
        for (line, expected) in [
            (
                r#"{"event":"error","path":"seg04.mp4","error":"connection reset"}"#,
                LogLevel::Error,
            ),
            (
                "✗ error: remote `missing` is not configured",
                LogLevel::Error,
            ),
            ("  └─ Protocol: invalid proxy URL", LogLevel::Error),
            ("▲ rate limit backoff on gdrive", LogLevel::Warn),
            ("conflict a.mp4: modified on both sides", LogLevel::Warn),
            (
                "2026-07-15T12:00:00Z DEBUG rship_core: stat",
                LogLevel::Debug,
            ),
        ] {
            let ParsedRshipLine::Log(entry) = parse_rship_stderr_line(line) else {
                panic!("diagnostic line must parse as log: {line}");
            };
            assert_eq!(entry.level, expected, "{line}");
        }

        // 3. Stdout JSON capture keeps normal summaries and drains oversized streams without OOM.
        let state = Arc::new(Mutex::new(RshipRunState::default()));
        consume_rship_stdout(br#"{"errors":[],"files":[]}"#.as_slice(), state.clone())
            .await
            .unwrap();
        assert!(!state.lock().stdout_truncated);
        assert_eq!(state.lock().stdout, r#"{"errors":[],"files":[]}"#);

        let oversized = vec![b'a'; MAX_RSHIP_STDOUT_BYTES + 64];
        let capped_state = Arc::new(Mutex::new(RshipRunState::default()));
        consume_rship_stdout(oversized.as_slice(), capped_state.clone())
            .await
            .unwrap();
        assert!(capped_state.lock().stdout_truncated);
        assert!(capped_state.lock().stdout.is_empty());
    }
}
