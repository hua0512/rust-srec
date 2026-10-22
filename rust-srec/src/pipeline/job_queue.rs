//! Database-backed job queue implementation.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::FutureExt;
use futures::future::{BoxFuture, OptionFuture};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::processors::{UploadItemStatus, UploadResultItem};
use super::progress::{JobProgressSnapshot, JobProgressUpdate, ProgressKind, ProgressReporter};
use super::upload_events::{UploadStatusBroadcaster, UploadStatusEvent, UploadTerminalStatus};
use crate::database::models::job::LogEntry as DbLogEntry;
use crate::database::models::{
    JobDbModel, JobExecutionLogDbModel, JobFilters, JobStatus, MediaFileType, MediaOutputDbModel,
    Pagination, TitleEntry, UploadRecordDbModel,
};
use crate::database::repositories::{
    JobRepository, SessionRepository, StagedOutputRepository, StreamerRepository,
    UploadRecordRepository,
};
use crate::pipeline::manifest::PipelineInputManifest;
use crate::pipeline::processors::utils as processor_utils;
use crate::utils::json::{self, JsonContext};
use crate::{Error, Result};

fn is_thumbnail_job_type(job_type: &str) -> bool {
    // JobDbModel.job_type is a free-form string. In practice we use:
    // - "thumbnail" for the direct thumbnail processor
    // - "thumbnail_<preset>" (e.g. thumbnail_native/thumbnail_hd) for preset-driven DAG steps
    let jt = job_type.to_ascii_lowercase();
    jt == "thumbnail" || jt.starts_with("thumbnail_")
}

/// Upload kind for a job type, or `None` when the job is not an upload.
///
/// Same convention as `is_thumbnail_job_type`: preset-driven DAG steps use
/// `<processor>_<preset>` job types, so prefixes match too. The returned
/// value is persisted as `upload_records.uploader` and broadcast as
/// `UploadStatusEvent::Started.uploader`.
pub(crate) fn upload_kind_for_job_type(job_type: &str) -> Option<&'static str> {
    let jt = job_type.to_ascii_lowercase();
    // "upload" is RcloneProcessor's legacy job-type alias.
    if jt == "rclone" || jt.starts_with("rclone_") || jt == "upload" || jt.starts_with("upload_") {
        return Some("rclone");
    }
    if jt == "baidupcs" || jt.starts_with("baidupcs_") {
        return Some("baidupcs");
    }
    None
}

/// Empty-string-as-absent conversion for `Job.streamer_id` / `Job.session_id`
/// (the `Job` struct uses `String` where the DB row has `Option<String>`).
fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// `UploadStatusEvent::Started` for `job`, or `None` when it is not an
/// upload job.
fn upload_started_event(job: &Job) -> Option<UploadStatusEvent> {
    let uploader = upload_kind_for_job_type(&job.job_type)?;
    Some(UploadStatusEvent::Started {
        job_id: job.id.clone(),
        streamer_id: non_empty(&job.streamer_id),
        streamer_name: job.streamer_name.clone(),
        session_id: non_empty(&job.session_id),
        uploader,
        files_total: job.inputs.len() as u32,
        started_at_ms: job.started_at.unwrap_or_else(Utc::now).timestamp_millis(),
    })
}

/// True when `job` is not an upload and has been processing for at least
/// `long_job_after` (see [`JobQueueConfig::long_job_after`]) at `now`. Only
/// such jobs reach the WebSocket, which keeps per-segment thumbnails and quick
/// renames off it.
fn is_long_job(job: &Job, now: DateTime<Utc>, long_job_after: std::time::Duration) -> bool {
    upload_kind_for_job_type(&job.job_type).is_none()
        && job.started_at.is_some_and(|started| {
            (now - started)
                .to_std()
                .is_ok_and(|elapsed| elapsed >= long_job_after)
        })
}

fn long_job_started_event(job: &Job) -> UploadStatusEvent {
    UploadStatusEvent::JobStarted {
        job_id: job.id.clone(),
        streamer_id: non_empty(&job.streamer_id),
        streamer_name: job.streamer_name.clone(),
        job_type: job.job_type.clone(),
        started_at_ms: job.started_at.unwrap_or_else(Utc::now).timestamp_millis(),
    }
}

/// `JobProgress` for a long job, `Progress` for an upload.
fn progress_event(
    long_job: bool,
    job_id: String,
    streamer_id: Option<String>,
    snapshot: JobProgressSnapshot,
) -> UploadStatusEvent {
    if long_job {
        UploadStatusEvent::JobProgress {
            job_id,
            streamer_id,
            snapshot,
        }
    } else {
        UploadStatusEvent::Progress {
            job_id,
            streamer_id,
            snapshot,
        }
    }
}

/// Re-announces every live upload job and long job (see [`is_long_job`]) at
/// `now`. Uploaders can go silent for minutes (BaiduPCS-Go prints nothing
/// while it prepares a file), and clients drop jobs they have not heard
/// about for a while so that a lost terminal event cannot leave one on
/// screen forever. Repeating the latest progress, or the start event before
/// there is any, keeps a silent job alive without inventing new numbers.
fn heartbeat_events(
    jobs_cache: &DashMap<String, Job>,
    cancellation_tokens: &DashMap<String, AttemptToken>,
    progress_cache: &DashMap<String, JobProgressSnapshot>,
    now: DateTime<Utc>,
    long_job_after: std::time::Duration,
) -> Vec<UploadStatusEvent> {
    // Collected before touching jobs_cache: cancel_pipeline locks jobs_cache
    // and then cancellation_tokens, so holding a token guard here while
    // locking jobs_cache could deadlock against it.
    let live_job_ids: Vec<String> = cancellation_tokens
        .iter()
        .filter(|entry| !entry.value().is_cancelled())
        .map(|entry| entry.key().clone())
        .collect();
    let mut events = Vec::new();
    for job_id in live_job_ids {
        let Some(job) = jobs_cache.get(&job_id) else {
            continue;
        };
        if job.status != JobStatus::Processing {
            continue;
        }
        let long_job = is_long_job(&job, now, long_job_after);
        let started = match upload_started_event(&job) {
            Some(started) => started,
            None if long_job => long_job_started_event(&job),
            None => continue,
        };
        match progress_cache.get(&job.id) {
            Some(snapshot) => events.push(progress_event(
                long_job,
                job.id.clone(),
                non_empty(&job.streamer_id),
                snapshot.clone(),
            )),
            None => events.push(started),
        }
    }
    events
}

/// Upload jobs waiting for a worker. Uses the same job-type rule as
/// [`upload_kind_for_job_type`], which SQL cannot express, so the database
/// only groups pending jobs by type.
async fn count_pending_uploads(repo: &dyn JobRepository) -> Result<u32> {
    let count: u64 = repo
        .count_pending_jobs_by_type()
        .await?
        .into_iter()
        .filter(|(job_type, _)| upload_kind_for_job_type(job_type).is_some())
        .map(|(_, count)| count)
        .sum();
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

const EXECUTION_INFO_MAX_LOGS: usize = 200;
const PROGRESS_FLUSH_INTERVAL_MS: u64 = 250;
/// Must stay well under the frontend's upload staleness window
/// (`STALE_AFTER_MS`, 2 minutes); see [`heartbeat_events`].
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Default for [`JobQueueConfig::long_job_after`].
const LONG_JOB_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
/// How often the queued-uploads count is re-checked while a client is
/// subscribed. Changes are only broadcast when the count moves.
const UPLOAD_QUEUE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
const LOG_PERSISTENCE_LOCK_SHARDS: usize = 64;
/// Ring-buffer bound on `job_execution_logs` rows per job: once a job
/// has this many rows, `persist_logs_to_db` trims the oldest via
/// `JobRepository::trim_execution_logs` after each insert. Counted in
/// `PersistedLogCursor::persisted_rows`, initialized from durable rows after
/// a terminal/retry path clears it. Time-based retention remains independent.
const MAX_PERSISTED_LOG_ROWS_PER_JOB: usize = 5000;

#[derive(Debug, Default)]
struct PersistedLogCursor {
    /// Initialized from durable rows on first use, including previous attempts.
    persisted_rows: Option<usize>,
}

struct LogCursorCleanup<'a> {
    queue: &'a JobQueue,
    job_id: &'a str,
}

impl Drop for LogCursorCleanup<'_> {
    fn drop(&mut self) {
        if !self.queue.jobs_cache.contains_key(self.job_id)
            && !self.queue.cancellation_tokens.contains_key(self.job_id)
        {
            self.queue.persisted_log_cursor.remove(self.job_id);
        }
    }
}

/// Cancellation for one execution attempt. A retry dequeues a new attempt under
/// the same job ID, so a worker still finishing an older attempt identifies its
/// own tracking by attempt rather than by job ID.
#[derive(Clone)]
pub(crate) struct AttemptToken {
    token: CancellationToken,
    attempt: u64,
}

impl Default for AttemptToken {
    fn default() -> Self {
        static NEXT_ATTEMPT: AtomicU64 = AtomicU64::new(0);
        Self {
            token: CancellationToken::new(),
            attempt: NEXT_ATTEMPT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl AttemptToken {
    /// Identity of this attempt, unique for the process lifetime.
    pub(crate) fn id(&self) -> u64 {
        self.attempt
    }
}

impl std::ops::Deref for AttemptToken {
    type Target = CancellationToken;

    fn deref(&self) -> &CancellationToken {
        &self.token
    }
}

#[derive(Clone, Copy)]
enum LogPersistence {
    /// The caller owns a disjoint batch and removes it only after a successful write.
    Append,
    /// Entries may include clones already written by a streaming collector.
    Snapshot,
}

/// Oldest rows to delete so `already + incoming` stays within `cap`.
fn excess_log_rows(already: usize, incoming: usize, cap: usize) -> usize {
    already.saturating_add(incoming).saturating_sub(cap)
}

fn extend_logs_capped(exec_info: &mut JobExecutionInfo, new_logs: &[JobLogEntry]) {
    if new_logs.is_empty() {
        return;
    }

    let tail = if new_logs.len() > EXECUTION_INFO_MAX_LOGS {
        &new_logs[new_logs.len() - EXECUTION_INFO_MAX_LOGS..]
    } else {
        new_logs
    };

    exec_info.logs.extend(tail.iter().cloned());
    while exec_info.logs.len() > EXECUTION_INFO_MAX_LOGS {
        exec_info.logs.pop_front();
    }
}

fn cap_logs_in_place(logs: &mut VecDeque<JobLogEntry>, cap: usize) {
    // VecDeque::pop_front is O(1) vs Vec::drain which is O(n)
    while logs.len() > cap {
        logs.pop_front();
    }
}

fn update_log_summary(exec_info: &mut JobExecutionInfo, new_logs: &[JobLogEntry]) {
    exec_info.log_lines_total = exec_info
        .log_lines_total
        .saturating_add(new_logs.len() as u64);

    for log in new_logs {
        match log.level {
            LogLevel::Warn => {
                exec_info.log_warn_count = exec_info.log_warn_count.saturating_add(1);
            }
            LogLevel::Error => {
                exec_info.log_error_count = exec_info.log_error_count.saturating_add(1);
            }
            _ => {}
        }
    }
}

/// Configuration for the job queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobQueueConfig {
    /// Warning threshold for queue depth.
    pub warning_threshold: usize,
    /// Critical threshold for queue depth.
    pub critical_threshold: usize,
    /// Poll interval in milliseconds.
    pub poll_interval_ms: u64,
    /// How long a job other than an upload runs before it reaches the
    /// WebSocket.
    pub long_job_after: std::time::Duration,
}

impl Default for JobQueueConfig {
    fn default() -> Self {
        Self {
            warning_threshold: 100,
            critical_threshold: 500,
            poll_interval_ms: 100,
            long_job_after: LONG_JOB_AFTER,
        }
    }
}

/// Status of queue depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueDepthStatus {
    /// Queue depth is normal.
    Normal,
    /// Queue depth is at warning level.
    Warning,
    /// Queue depth is at critical level.
    Critical,
}

/// Log level for job execution logs.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug level log.
    Debug,
    /// Info level log.
    #[default]
    Info,
    /// Warning level log.
    Warn,
    /// Error level log.
    Error,
}

impl LogLevel {
    /// Uppercase name stored in the `job_execution_logs.level` column and returned by the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

/// A single log entry for job execution.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Timestamp of the log entry.
    pub timestamp: DateTime<Utc>,
    /// Log level.
    pub level: LogLevel,
    /// Log message.
    pub message: String,
    // Shared by clones in streamed and returned logs; equality of timestamps or text does
    // not identify an occurrence. Owned by the entries so retained snapshots stay bounded
    // without a queue-wide history, even after their database rows have been trimmed.
    #[serde(skip)]
    persisted_for: Arc<parking_lot::Mutex<HashSet<Arc<str>>>>,
}

impl JobLogEntry {
    /// Create a new log entry with the current timestamp.
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            message: message.into(),
            persisted_for: Arc::default(),
        }
    }

    fn mark_persisted(&self, job_id: Arc<str>) {
        self.persisted_for.lock().insert(job_id);
    }

    /// Create an info log entry.
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Info, message)
    }

    /// Create a warning log entry.
    pub fn warn(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Warn, message)
    }

    /// Create an error log entry.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Error, message)
    }

    /// Create a debug log entry.
    pub fn debug(message: impl Into<String>) -> Self {
        Self::new(LogLevel::Debug, message)
    }
}

/// Per-step duration tracking for pipeline jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDuration {
    /// Step number (1-indexed).
    pub step: u32,
    /// Processor/job type name.
    pub processor: String,
    /// Duration in seconds.
    pub duration_secs: f64,
    /// Start timestamp.
    pub started_at: DateTime<Utc>,
    /// End timestamp.
    pub completed_at: DateTime<Utc>,
}

/// Extended job information for observability.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JobExecutionInfo {
    /// Current processor name.
    pub current_processor: Option<String>,
    /// Current step number (1-indexed).
    pub current_step: Option<u32>,
    /// Total steps in pipeline.
    pub total_steps: Option<u32>,
    /// Intermediate artifacts produced.
    pub items_produced: Vec<String>,
    /// Input file size in bytes.
    pub input_size_bytes: Option<u64>,
    /// Output file size in bytes.
    pub output_size_bytes: Option<u64>,
    /// Detailed execution logs (VecDeque for O(1) pop_front when capping).
    pub logs: VecDeque<JobLogEntry>,
    /// Total number of log lines captured for this job (across all steps).
    #[serde(default)]
    pub log_lines_total: u64,
    /// Number of WARN lines captured.
    #[serde(default)]
    pub log_warn_count: u64,
    /// Number of ERROR lines captured.
    #[serde(default)]
    pub log_error_count: u64,
    /// Per-step duration tracking for pipeline jobs.
    #[serde(default)]
    pub step_durations: Vec<StepDuration>,
    /// Preserve extension fields when artifact and terminal updates rewrite this document.
    #[serde(flatten)]
    pub extra_metadata: serde_json::Map<String, serde_json::Value>,
}

impl JobExecutionInfo {
    /// Create a new empty execution info.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the current processor.
    pub fn with_processor(mut self, processor: impl Into<String>) -> Self {
        self.current_processor = Some(processor.into());
        self
    }

    /// Set the current step.
    pub fn with_step(mut self, step: u32, total: u32) -> Self {
        self.current_step = Some(step);
        self.total_steps = Some(total);
        self
    }

    /// Add a log entry.
    pub fn add_log(&mut self, entry: JobLogEntry) {
        self.logs.push_back(entry);
    }

    /// Add an info log.
    pub fn log_info(&mut self, message: impl Into<String>) {
        self.add_log(JobLogEntry::info(message));
    }

    /// Add an error log.
    pub fn log_error(&mut self, message: impl Into<String>) {
        self.add_log(JobLogEntry::error(message));
    }

    /// Add an intermediate artifact.
    pub fn add_item_produced(&mut self, path: impl Into<String>) {
        self.items_produced.push(path.into());
    }

    /// Set input size.
    pub fn with_input_size(mut self, size: u64) -> Self {
        self.input_size_bytes = Some(size);
        self
    }

    /// Set output size.
    pub fn with_output_size(mut self, size: u64) -> Self {
        self.output_size_bytes = Some(size);
        self
    }

    /// Record a completed step's duration.
    pub fn record_step_duration(
        &mut self,
        step: u32,
        processor: impl Into<String>,
        duration_secs: f64,
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
    ) {
        self.step_durations.push(StepDuration {
            step,
            processor: processor.into(),
            duration_secs,
            started_at,
            completed_at,
        });
    }

    /// Get total duration of all recorded steps.
    pub fn total_step_duration(&self) -> f64 {
        self.step_durations.iter().map(|s| s.duration_secs).sum()
    }
}

/// A job in the queue.
#[derive(Debug, Clone)]
pub struct Job {
    /// Unique job ID.
    pub id: String,
    /// Job type (e.g., "remux", "upload", "thumbnail").
    pub job_type: String,
    /// Input file paths.
    pub inputs: Vec<String>,
    /// Output file paths.
    pub outputs: Vec<String>,
    /// Job priority (higher = more urgent).
    pub priority: i32,
    /// Current status.
    pub status: JobStatus,
    /// Streamer ID this job belongs to.
    pub streamer_id: String,
    /// Session ID this job belongs to.
    pub session_id: String,
    /// Human-readable streamer name.
    pub streamer_name: Option<String>,
    /// Session/stream title.
    pub session_title: Option<String>,
    /// Platform name (e.g., "Twitch", "Huya").
    pub platform: Option<String>,
    /// live_sessions.start_time of the session this job belongs to; None for jobs whose session_id does not reference a real session row.
    pub session_start: Option<DateTime<Utc>>,
    /// Additional configuration as JSON.
    pub config: Option<String>,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job started processing.
    pub started_at: Option<DateTime<Utc>>,
    /// When the job completed.
    pub completed_at: Option<DateTime<Utc>>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Number of retry attempts.
    pub retry_count: i32,
    /// When a scheduled automatic retry of this failed job may run.
    pub retry_after: Option<DateTime<Utc>>,
    /// Timeout for one attempt, in seconds, set by the workflow step; the
    /// worker pool's default applies when absent.
    pub timeout_secs: Option<u64>,
    // Pipeline chain fields
    /// Pipeline ID to group related jobs (first job's ID).
    pub pipeline_id: Option<String>,
    /// Execution information for observability.
    pub execution_info: Option<JobExecutionInfo>,
    /// Processing duration in seconds (from processor output).
    pub duration_secs: Option<f64>,
    /// Time spent waiting in queue before processing started (seconds).
    pub queue_wait_secs: Option<f64>,
    /// DAG step execution ID (if this job is part of a DAG pipeline).
    pub dag_step_execution_id: Option<String>,
    /// Per-segment video/danmu pairing copied from the DAG row of a
    /// paired-segment or session-complete pipeline; persisted in `state`.
    pub manifest: Option<Arc<PipelineInputManifest>>,
}

impl Job {
    /// Create a new job.
    pub fn new(
        job_type: impl Into<String>,
        inputs: Vec<String>,
        outputs: Vec<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            inputs,
            outputs,
            priority: 0,
            status: JobStatus::Pending,
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            streamer_name: None,
            session_title: None,
            platform: None,
            session_start: None,
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            retry_after: None,
            timeout_secs: None,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
            manifest: None,
        }
    }

    /// Create a new pipeline step job with pipeline ID.
    pub fn new_pipeline_step(
        job_type: impl Into<String>,
        inputs: Vec<String>,
        outputs: Vec<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        pipeline_id: Option<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            inputs,
            outputs,
            priority: 0,
            status: JobStatus::Pending,
            streamer_id: streamer_id.into(),
            session_id: session_id.into(),
            streamer_name: None,
            session_title: None,
            platform: None,
            session_start: None,
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            retry_after: None,
            timeout_secs: None,
            pipeline_id,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
            manifest: None,
        }
    }

    /// Set the job priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set the job configuration.
    pub fn with_config(mut self, config: impl Into<String>) -> Self {
        self.config = Some(config.into());
        self
    }

    /// Set the pipeline ID.
    pub fn with_pipeline_id(mut self, pipeline_id: impl Into<String>) -> Self {
        self.pipeline_id = Some(pipeline_id.into());
        self
    }

    /// Set the DAG step execution ID.
    pub fn with_dag_step_execution_id(mut self, dag_step_execution_id: impl Into<String>) -> Self {
        self.dag_step_execution_id = Some(dag_step_execution_id.into());
        self
    }

    /// Set the streamer name.
    pub fn with_streamer_name(mut self, name: impl Into<String>) -> Self {
        self.streamer_name = Some(name.into());
        self
    }

    /// Set the session title.
    pub fn with_session_title(mut self, title: impl Into<String>) -> Self {
        self.session_title = Some(title.into());
        self
    }

    /// Set the platform.
    pub fn with_platform(mut self, platform: impl Into<String>) -> Self {
        self.platform = Some(platform.into());
        self
    }

    /// Set the session start timestamp.
    pub fn with_session_start(mut self, session_start: DateTime<Utc>) -> Self {
        self.session_start = Some(session_start);
        self
    }

    /// Check if this job is part of a DAG pipeline.
    pub fn is_dag_job(&self) -> bool {
        self.dag_step_execution_id.is_some()
    }
}

/// Result of a completed job.
#[derive(Debug, Clone)]
pub struct JobResult {
    /// Output paths or result data.
    pub outputs: Vec<String>,
    /// Duration of processing in seconds.
    pub duration_secs: f64,
    /// Additional metadata.
    pub metadata: Option<String>,
    /// Per-file upload results (from `ProcessorOutput::uploads`); persisted
    /// to `upload_records` by `JobQueue::complete` for upload job types.
    pub uploads: Vec<UploadResultItem>,
    /// Execution logs.
    pub logs: Vec<JobLogEntry>,
}

/// One in-flight job, as reported by [`JobQueue::list_active_jobs`]. Feeds
/// the WebSocket `DownloadSnapshot.uploads` and `jobs` slices so reconnecting
/// clients recover jobs that started before they subscribed.
#[derive(Debug, Clone)]
pub struct ActiveJobInfo {
    pub job_id: String,
    pub streamer_id: Option<String>,
    pub streamer_name: Option<String>,
    pub session_id: Option<String>,
    /// Upload kind; `None` for other jobs, which are listed once they have
    /// been processing for 10 seconds.
    pub uploader: Option<&'static str>,
    pub job_type: String,
    pub files_total: u32,
    pub started_at: Option<DateTime<Utc>>,
    /// Latest in-memory progress snapshot, when the processor has reported one.
    pub progress: Option<JobProgressSnapshot>,
}

/// Where a job failed, recorded in its execution info when known.
#[derive(Default, Clone, Copy)]
struct FailedStep<'a> {
    processor_name: Option<&'a str>,
    step_number: Option<u32>,
    total_steps: Option<u32>,
}

struct FailedUploadContext {
    job_type: String,
    streamer_id: Option<String>,
    session_id: Option<String>,
    inputs: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FailureMetadata {
    Preserve,
    DatabaseLog,
}

/// Whether this call changed an active job to FAILED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobFailureOutcome {
    Transitioned,
    /// The conditional update lost to another terminal transition (or deletion).
    Unchanged,
}

/// The job queue service.
pub struct JobQueue {
    /// Workers hold a read lease through execution and cleanup. Retention takes an exclusive lease.
    pub(crate) output_files_gate: Arc<tokio::sync::RwLock<()>>,
    /// Dequeues share a lease through claiming and publishing tracking, and failures
    /// through recording and cleaning up their attempt. Retry and attempt finalization
    /// take an exclusive lease so cleanup cannot cross that handoff.
    attempt_gate: tokio::sync::RwLock<()>,
    /// Configuration.
    config: JobQueueConfig,
    /// Current queue depth (approximate).
    depth: AtomicUsize,
    /// Notify when new jobs are added.
    notify: Arc<Notify>,
    /// Job repository for database persistence.
    job_repository: Arc<dyn JobRepository>,
    /// Session repository for persisting media outputs (e.g., thumbnails).
    session_repo: std::sync::OnceLock<Arc<dyn SessionRepository>>,
    /// Streamer repository for looking up streamer metadata (e.g., name).
    streamer_repo: std::sync::OnceLock<Arc<dyn StreamerRepository>>,
    /// Upload record repository for persisting per-file upload results.
    upload_repo: std::sync::OnceLock<Arc<dyn UploadRecordRepository>>,
    /// Temporary files staged by attempts, for listing what a crash left behind.
    staged_output_repo: std::sync::OnceLock<Arc<dyn StagedOutputRepository>>,
    /// Fan-out for live upload status events. Shared (`Arc`) with the
    /// progress aggregator task, which publishes `Progress` events on its
    /// flush tick; installed once via `set_upload_broadcaster`.
    upload_broadcaster: Arc<std::sync::OnceLock<UploadStatusBroadcaster>>,
    /// In-memory cache of jobs (for quick lookups).
    ///
    /// `Arc`-shared with the progress aggregator task: `DashMap::clone`
    /// deep-copies the map, so handing the aggregator a bare clone would
    /// freeze it at the (empty) construction-time contents.
    jobs_cache: Arc<DashMap<String, Job>>,
    /// Cancellation tokens for processing jobs. `Arc`-shared with the
    /// progress aggregator, whose liveness check reads it (see `jobs_cache`).
    cancellation_tokens: Arc<DashMap<String, AttemptToken>>,
    /// Latest progress snapshot per job (in-memory). `Arc`-shared with the
    /// progress aggregator, which writes it (see `jobs_cache`).
    progress_cache: Arc<DashMap<String, JobProgressSnapshot>>,
    /// Progress updates for the aggregator, which caches and coalesces them.
    progress_tx: tokio::sync::mpsc::Sender<JobProgressUpdate>,
    /// Stops the progress consumer after worker producers have settled.
    progress_shutdown: CancellationToken,
    /// Owned so pipeline shutdown can join the aggregator, which reads the job
    /// repository for the queued-upload count, before the database pools close.
    progress_aggregator: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Durable row counts used to cap logs in `job_execution_logs`.
    persisted_log_cursor: DashMap<String, PersistedLogCursor>,
    /// Stable locks outlive cursor eviction during cancellation and terminal cleanup.
    log_persistence_locks: [tokio::sync::Mutex<()>; LOG_PERSISTENCE_LOCK_SHARDS],
}

impl JobQueue {
    /// Create a job queue that persists its jobs through `repository`.
    pub fn with_repository(config: JobQueueConfig, repository: Arc<dyn JobRepository>) -> Self {
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<JobProgressUpdate>(1024);
        // These maps are Arc-shared with the aggregator task: DashMap::clone
        // deep-copies contents, so a bare clone would permanently detach the
        // aggregator's view from the queue's.
        let jobs_cache: Arc<DashMap<String, Job>> = Arc::new(DashMap::new());
        let cancellation_tokens: Arc<DashMap<String, AttemptToken>> = Arc::new(DashMap::new());
        let progress_cache: Arc<DashMap<String, JobProgressSnapshot>> = Arc::new(DashMap::new());
        let upload_broadcaster: Arc<std::sync::OnceLock<UploadStatusBroadcaster>> =
            Arc::new(std::sync::OnceLock::new());
        let progress_shutdown = CancellationToken::new();
        let progress_aggregator = spawn_progress_aggregator(
            repository.clone(),
            config.long_job_after,
            progress_rx,
            jobs_cache.clone(),
            cancellation_tokens.clone(),
            progress_cache.clone(),
            upload_broadcaster.clone(),
            progress_shutdown.clone(),
        );

        Self {
            config,
            output_files_gate: Arc::new(tokio::sync::RwLock::new(())),
            attempt_gate: tokio::sync::RwLock::new(()),
            depth: AtomicUsize::new(0),
            notify: Arc::new(Notify::new()),
            job_repository: repository,
            session_repo: std::sync::OnceLock::new(),
            streamer_repo: std::sync::OnceLock::new(),
            upload_repo: std::sync::OnceLock::new(),
            staged_output_repo: std::sync::OnceLock::new(),
            upload_broadcaster,
            jobs_cache,
            cancellation_tokens,
            progress_cache,
            progress_tx,
            progress_shutdown,
            progress_aggregator: parking_lot::Mutex::new(progress_aggregator),
            persisted_log_cursor: DashMap::new(),
            log_persistence_locks: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) async fn stop_progress_aggregator(&self) {
        self.progress_shutdown.cancel();
        let task = self.progress_aggregator.lock().take();
        if let Some(task) = task
            && let Err(error) = task.await
        {
            warn!(%error, "Job progress aggregator failed while stopping");
        }
    }

    #[cfg(test)]
    pub(crate) fn progress_aggregator_is_running(&self) -> bool {
        self.progress_aggregator
            .lock()
            .as_ref()
            .is_some_and(|task| !task.is_finished())
    }

    /// Set the session repository for persisting media outputs (e.g., thumbnails).
    /// This can only be called once.
    pub(crate) fn set_session_repo(&self, repo: Arc<dyn SessionRepository>) {
        if self.session_repo.set(repo).is_err() {
            tracing::warn!("Job queue session repository was already installed");
        }
    }

    /// Set the streamer repository for looking up streamer metadata.
    /// This can only be called once.
    pub(crate) fn set_streamer_repo(&self, repo: Arc<dyn StreamerRepository>) {
        if self.streamer_repo.set(repo).is_err() {
            tracing::warn!("Job queue streamer repository was already installed");
        }
    }

    /// Set the upload record repository for persisting per-file upload results.
    /// This can only be called once.
    pub(crate) fn set_upload_repo(&self, repo: Arc<dyn UploadRecordRepository>) {
        if self.upload_repo.set(repo).is_err() {
            tracing::warn!("Job queue upload record repository was already installed");
        }
    }

    /// Set the repository that records staged temporary files. This can only be called once.
    pub(crate) fn set_staged_output_repo(&self, repo: Arc<dyn StagedOutputRepository>) {
        if self.staged_output_repo.set(repo).is_err() {
            tracing::warn!("Job queue staged output repository was already installed");
        }
    }

    pub(crate) fn staged_output_repo(&self) -> Option<Arc<dyn StagedOutputRepository>> {
        self.staged_output_repo.get().cloned()
    }

    /// A recorder for a new attempt of `job_id`; inert without a repository.
    pub(crate) fn staged_output_recorder(&self, job_id: &str) -> super::StagedOutputRecorder {
        self.staged_output_repo
            .get()
            .map(|repo| super::StagedOutputRecorder::for_attempt(repo.clone(), job_id))
            .unwrap_or_default()
    }

    /// Install the live upload-status broadcaster. This can only be called once.
    pub(crate) fn set_upload_broadcaster(&self, broadcaster: UploadStatusBroadcaster) {
        if self.upload_broadcaster.set(broadcaster).is_err() {
            tracing::warn!("Job queue upload broadcaster was already installed");
        }
    }

    fn emit_upload_event(&self, event: UploadStatusEvent) {
        if let Some(broadcaster) = self.upload_broadcaster.get() {
            broadcaster.send(event);
        }
    }

    /// Publish `Started` for an upload job. Any other job is announced as
    /// `JobStarted` after [`JobQueueConfig::long_job_after`], followed by its latest progress,
    /// and only if `attempt` is still running then. Called from `dequeue`
    /// right after the claim succeeds.
    fn announce_started(&self, job: &Job, attempt: u64) {
        if let Some(event) = upload_started_event(job) {
            self.emit_upload_event(event);
            return;
        }
        let jobs_cache = self.jobs_cache.clone();
        let cancellation_tokens = self.cancellation_tokens.clone();
        let progress_cache = self.progress_cache.clone();
        let upload_broadcaster = self.upload_broadcaster.clone();
        let job_id = job.id.clone();
        let long_job_after = self.config.long_job_after;
        tokio::spawn(async move {
            tokio::time::sleep(long_job_after).await;
            // The token guard is released before jobs_cache is locked; see
            // heartbeat_events for the lock order this avoids.
            let live = cancellation_tokens
                .get(&job_id)
                .is_some_and(|token| token.id() == attempt && !token.is_cancelled());
            if live
                && let Some(broadcaster) = upload_broadcaster.get()
                && broadcaster.has_subscribers()
                && let Some((started, streamer_id)) = jobs_cache
                    .get(&job_id)
                    .filter(|job| job.status == JobStatus::Processing)
                    .map(|job| (long_job_started_event(&job), non_empty(&job.streamer_id)))
            {
                broadcaster.send(started);
                // Progress from before this point stayed off the socket, and
                // the job may report nothing more for a while (FFmpeg prints
                // no progress during the MP4 muxer's second pass), so its
                // latest snapshot goes out now rather than at the next
                // heartbeat.
                let snapshot = progress_cache.get(&job_id).map(|snapshot| snapshot.clone());
                if let Some(snapshot) = snapshot {
                    broadcaster.send(UploadStatusEvent::JobProgress {
                        job_id,
                        streamer_id,
                        snapshot,
                    });
                }
            }
        });
    }

    /// Publish `Terminal { Cancelled }` when the job is an upload job.
    /// Cancellation produces no per-file records (the job's CANCELLED status
    /// is the durable truth), so the file counters are zero by construction.
    fn emit_upload_cancelled(&self, job_id: &str, job_type: &str, streamer_id: &str) {
        if upload_kind_for_job_type(job_type).is_none() {
            return;
        }
        self.emit_upload_event(UploadStatusEvent::Terminal {
            job_id: job_id.to_string(),
            streamer_id: non_empty(streamer_id),
            status: UploadTerminalStatus::Cancelled,
            files_succeeded: 0,
            files_failed: 0,
            files_skipped: 0,
            error: None,
        });
    }

    /// Publish `JobEnded` when `job` ran long enough to have been announced
    /// (see [`is_long_job`]); shorter jobs never reach the socket.
    fn emit_long_job_ended(&self, job: &Job) {
        if is_long_job(job, Utc::now(), self.config.long_job_after)
            && let Some(started_at) = job.started_at
        {
            self.emit_upload_event(UploadStatusEvent::JobEnded {
                job_id: job.id.clone(),
                started_at_ms: started_at.timestamp_millis(),
            });
        }
    }

    /// Persist per-file upload results to `upload_records`.
    ///
    /// Best-effort, same policy as `persist_thumbnail_output`: a bookkeeping
    /// failure is logged and swallowed so it can never fail the job whose
    /// transfer already succeeded. No-op for non-upload job types and when
    /// no upload record repository is installed.
    pub(crate) async fn persist_upload_records(
        &self,
        job_id: &str,
        job_type: &str,
        streamer_id: Option<&str>,
        session_id: Option<&str>,
        items: &[UploadResultItem],
    ) {
        let Some(uploader) = upload_kind_for_job_type(job_type) else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let Some(repo) = self.upload_repo.get() else {
            return;
        };

        let now = crate::database::time::now_ms();
        let records: Vec<UploadRecordDbModel> = items
            .iter()
            .map(|item| UploadRecordDbModel {
                id: uuid::Uuid::new_v4().to_string(),
                job_id: Some(job_id.to_string()),
                streamer_id: streamer_id.filter(|s| !s.is_empty()).map(str::to_string),
                session_id: session_id.filter(|s| !s.is_empty()).map(str::to_string),
                uploader: uploader.to_string(),
                local_path: item.local_path.clone(),
                remote_path: item.remote_path.clone(),
                status: item.status.as_str().to_string(),
                size_bytes: item.size_bytes.map(|s| s as i64),
                error: item.error.clone(),
                created_at: now,
                updated_at: now,
                completed_at: matches!(item.status, UploadItemStatus::Completed).then_some(now),
            })
            .collect();

        if let Err(e) = repo.upsert_records(&records).await {
            warn!(job_id, error = %e, "Failed to persist upload records");
        }
    }

    /// In-flight upload jobs, and other jobs processing for
    /// [`JobQueueConfig::long_job_after`] or more, with their latest progress
    /// snapshots. Feeds the WebSocket
    /// `DownloadSnapshot`.
    pub async fn list_active_jobs(&self) -> Result<Vec<ActiveJobInfo>> {
        let now = Utc::now();
        let mut infos = Vec::new();
        for db_job in self
            .job_repository
            .list_jobs_by_status(JobStatus::Processing)
            .await?
        {
            infos.extend(self.active_job_info(&db_model_to_job(&db_job), now));
        }
        Ok(infos)
    }

    /// Upload jobs waiting for a worker.
    pub async fn count_pending_uploads(&self) -> Result<u32> {
        count_pending_uploads(self.job_repository.as_ref()).await
    }

    fn active_job_info(&self, job: &Job, now: DateTime<Utc>) -> Option<ActiveJobInfo> {
        let uploader = upload_kind_for_job_type(&job.job_type);
        if uploader.is_none() && !is_long_job(job, now, self.config.long_job_after) {
            return None;
        }
        Some(ActiveJobInfo {
            job_id: job.id.clone(),
            streamer_id: non_empty(&job.streamer_id),
            streamer_name: job.streamer_name.clone(),
            session_id: non_empty(&job.session_id),
            uploader,
            job_type: job.job_type.clone(),
            files_total: job.inputs.len() as u32,
            started_at: job.started_at,
            progress: self.progress_cache.get(&job.id).map(|s| s.clone()),
        })
    }

    /// Persist thumbnail output to media_outputs table.
    async fn persist_thumbnail_output(&self, session_id: &str, output_path: &str) {
        let Some(repo) = self.session_repo.get() else {
            return;
        };

        // Avoid inserting obviously-non-thumbnail outputs.
        // The ThumbnailProcessor may return passthrough outputs for unsupported inputs.
        let Some(ext) = processor_utils::get_extension(output_path) else {
            return;
        };
        if !processor_utils::is_image(&ext) {
            return;
        }

        // Best-effort dedupe: repeated retries/completions should not create duplicate rows.
        // (Schema does not enforce uniqueness on file_path.)
        if let Ok(existing) = repo.get_media_outputs_for_session(session_id).await {
            let output_key = if cfg!(windows) {
                output_path.to_ascii_lowercase()
            } else {
                output_path.to_string()
            };

            if existing.iter().any(|row| {
                row.file_type == MediaFileType::Thumbnail.as_str() && {
                    let row_key = if cfg!(windows) {
                        row.file_path.to_ascii_lowercase()
                    } else {
                        row.file_path.clone()
                    };
                    row_key == output_key
                }
            }) {
                return;
            }
        }

        // Get file size
        let size_bytes = tokio::fs::metadata(output_path)
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);

        let output = MediaOutputDbModel::new(
            session_id,
            output_path,
            MediaFileType::Thumbnail,
            size_bytes,
        );

        if let Err(e) = repo.create_media_output(&output).await {
            warn!(
                "Failed to persist thumbnail output for session {}: {}",
                session_id, e
            );
        } else {
            info!(
                "Persisted thumbnail output for session {}: {}",
                session_id, output_path
            );
        }
    }

    /// Persist a disjoint batch, including repeated entries. Callers must not resubmit a
    /// successful batch; after an error the same batch can be retried.
    pub async fn append_log_entry(&self, job_id: &str, logs: &[JobLogEntry]) -> Result<()> {
        self.persist_logs_to_db(job_id, logs, LogPersistence::Append)
            .await?;
        Ok(())
    }

    /// Persist the logs a processor returned with its result and fold them into
    /// the job's execution summary. Entries the streaming collector already wrote
    /// are not duplicated. Used for a result the pool turns into a failure, where
    /// `complete_if_processing` never runs.
    pub(crate) async fn record_result_logs(
        &self,
        job_id: &str,
        logs: &[JobLogEntry],
    ) -> Result<()> {
        let repo = &self.job_repository;
        let new_logs = self
            .persist_logs_to_db(job_id, logs, LogPersistence::Snapshot)
            .await?;
        if new_logs.is_empty() {
            return Ok(());
        }
        let stored = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            stored.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );
        update_log_summary(&mut exec_info, &new_logs);
        extend_logs_capped(&mut exec_info, &new_logs);
        let execution_info = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &execution_info)
            .await
    }

    fn log_persistence_lock(&self, job_id: &str) -> &tokio::sync::Mutex<()> {
        let mut hasher = DefaultHasher::new();
        job_id.hash(&mut hasher);
        &self.log_persistence_locks[hasher.finish() as usize % LOG_PERSISTENCE_LOCK_SHARDS]
    }

    async fn persist_logs_to_db(
        &self,
        job_id: &str,
        logs: &[JobLogEntry],
        mode: LogPersistence,
    ) -> Result<Vec<JobLogEntry>> {
        let repo = &self.job_repository;
        if logs.is_empty() {
            return Ok(vec![]);
        }

        // Serialize snapshot deduplication and the row cap across database IO. Concurrent
        // snapshots can otherwise both observe an unpersisted occurrence before insertion.
        // The lock must survive removal of the cursor by cancellation or terminal cleanup.
        let _guard = self.log_persistence_lock(job_id).lock().await;
        let _cleanup = LogCursorCleanup {
            queue: self,
            job_id,
        };
        let previous_rows = self
            .persisted_log_cursor
            .entry(job_id.to_owned())
            .or_default()
            .persisted_rows;
        let already_persisted = match previous_rows {
            Some(rows) => rows,
            None => {
                let (_, rows) = repo
                    .list_execution_logs(job_id, &Pagination::new(0, 0))
                    .await?;
                let rows = usize::try_from(rows).unwrap_or(usize::MAX);
                if let Some(mut cursor) = self.persisted_log_cursor.get_mut(job_id) {
                    cursor.persisted_rows = Some(rows);
                }
                rows
            }
        };

        let new_logs: Vec<JobLogEntry> = logs
            .iter()
            .filter(|entry| {
                matches!(mode, LogPersistence::Append)
                    || !entry.persisted_for.lock().contains(job_id)
            })
            .cloned()
            .collect();
        let mut persisted_rows = already_persisted;
        if !new_logs.is_empty() {
            let db_logs: Vec<JobExecutionLogDbModel> = new_logs
                .iter()
                .map(|entry| JobExecutionLogDbModel {
                    id: uuid::Uuid::new_v4().to_string(),
                    job_id: job_id.to_string(),
                    entry: json::to_string_or_fallback(
                        entry,
                        r#"{"level":"error","message":"log serialize failed"}"#,
                        JsonContext::JobField {
                            job_id,
                            field: "execution_log_entry",
                        },
                        "Failed to serialize execution log entry; using fallback",
                    ),
                    created_at: entry.timestamp.timestamp_millis(),
                    level: Some(entry.level.as_str().to_string()),
                    message: Some(entry.message.clone()),
                })
                .collect();
            repo.add_execution_logs(&db_logs).await?;
            persisted_rows = persisted_rows.saturating_add(new_logs.len());
            let owner: Arc<str> = Arc::from(job_id);
            for entry in &new_logs {
                entry.mark_persisted(owner.clone());
            }

            // Insert first, trim after: the newest lines (a failing run's
            // tail) always land, and the oldest rows leave once the run
            // exceeds MAX_PERSISTED_LOG_ROWS_PER_JOB. Trim failure is
            // logged, not propagated — the log write itself succeeded.
            let excess = excess_log_rows(
                already_persisted,
                new_logs.len(),
                MAX_PERSISTED_LOG_ROWS_PER_JOB,
            );
            if excess > 0 {
                match repo.trim_execution_logs(job_id, excess).await {
                    Ok(()) => persisted_rows = persisted_rows.saturating_sub(excess),
                    Err(error) => {
                        warn!(job_id, %error, "Failed to trim oldest execution log rows");
                    }
                }
            }
        }

        // Keep excess rows in the count after failed trimming, so the next write retries
        // all outstanding excess. Do not resurrect a cursor removed during the IO above.
        if let Some(mut cursor) = self.persisted_log_cursor.get_mut(job_id) {
            cursor.persisted_rows = Some(persisted_rows);
        }
        Ok(new_logs)
    }

    /// Create a progress reporter for a job.
    pub fn progress_reporter(&self, job_id: &str) -> ProgressReporter {
        ProgressReporter::new(job_id.to_string(), self.progress_tx.clone())
    }

    /// The latest progress snapshot of a running job.
    ///
    /// Snapshots are kept in memory only: a job has none until it first
    /// reports, including after a restart, and loses it when it ends.
    pub fn get_job_progress(&self, job_id: &str) -> Option<JobProgressSnapshot> {
        self.progress_cache
            .get(job_id)
            .map(|snapshot| snapshot.clone())
    }

    /// [`Self::get_job_progress`] with `raw` set to null.
    ///
    /// Only running jobs have an entry, but a failing job keeps its entry for a
    /// moment after FAILED is written, so callers should only ask about jobs
    /// whose stored status is PROCESSING. `raw` is dropped because it is too
    /// large to repeat for every workflow step.
    pub fn cached_progress(&self, job_id: &str) -> Option<JobProgressSnapshot> {
        let snapshot = self.progress_cache.get(job_id)?;
        Some(JobProgressSnapshot {
            current_file: snapshot.current_file.clone(),
            raw: serde_json::Value::Null,
            ..*snapshot
        })
    }

    /// Resolve missing metadata (streamer_name, session_title, session_start) for a job.
    ///
    /// This is useful for jobs recovered from the database where metadata
    /// is not persisted. It looks up the streamer and session repositories.
    /// Anything resolved is written back to the job row's `state` column so
    /// the values survive later deletion of the backing streamer/session rows
    /// (a retry re-reads the job from the DB via `db_model_to_job`).
    pub async fn resolve_job_metadata(&self, job: &mut Job) {
        let mut backfilled = false;

        // Only resolve if we have a streamer_id and name is missing
        if job.streamer_name.is_none()
            && !job.streamer_id.is_empty()
            && let Some(streamer_repo) = self.streamer_repo.get()
        {
            match streamer_repo.get_streamer(&job.streamer_id).await {
                Ok(streamer) => {
                    tracing::debug!(
                        job_id = %job.id,
                        streamer_id = %job.streamer_id,
                        streamer_name = %streamer.name,
                        "Resolved streamer_name from repository"
                    );
                    job.streamer_name = Some(streamer.name);
                    backfilled = true;
                }
                Err(e) => {
                    tracing::debug!(
                        job_id = %job.id,
                        streamer_id = %job.streamer_id,
                        error = %e,
                        "Failed to lookup streamer_name"
                    );
                }
            }
        }

        // Only resolve if we have a session_id and session metadata is missing
        if (job.session_title.is_none() || job.session_start.is_none())
            && !job.session_id.is_empty()
            && let Some(session_repo) = self.session_repo.get()
        {
            match session_repo.get_session(&job.session_id).await {
                Ok(session) => {
                    if job.session_start.is_none() {
                        job.session_start =
                            Some(crate::database::time::ms_to_datetime(session.start_time));
                        backfilled = true;
                    }

                    // Parse titles JSON and get the most recent one
                    if job.session_title.is_none()
                        && let Some(titles_json) = session.titles
                        && let Ok(entries) = serde_json::from_str::<Vec<TitleEntry>>(&titles_json)
                        && let Some(last_entry) = entries.last()
                    {
                        tracing::debug!(
                            job_id = %job.id,
                            session_id = %job.session_id,
                            session_title = %last_entry.title,
                            "Resolved session_title from repository"
                        );
                        job.session_title = Some(last_entry.title.clone());
                        backfilled = true;
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        job_id = %job.id,
                        session_id = %job.session_id,
                        error = %e,
                        "Failed to lookup session_title"
                    );
                }
            }
        }

        // Best-effort durability: without this write the streamer/session
        // rows must still exist at every future dequeue of this job.
        if backfilled {
            let state = job_state_json(job);
            if let Err(e) = self.job_repository.update_job_state(&job.id, &state).await {
                tracing::debug!(
                    job_id = %job.id,
                    error = %e,
                    "Failed to persist resolved job metadata"
                );
            }
        }
    }

    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();

        self.job_repository
            .create_job(&job_to_db_model(&job))
            .await?;

        // Add to in-memory cache
        self.jobs_cache.insert(job_id.clone(), job);

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued job {} of type {}", job_id, job_type);

        self.wake_workers();

        Ok(job_id)
    }

    /// Enqueue an existing job (already persisted to database).
    /// This adds the job to the in-memory cache and notifies workers.
    /// Used by DagScheduler when creating jobs for DAG steps.
    pub async fn enqueue_existing(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();

        // Add to in-memory cache (job is already in database)
        self.jobs_cache.insert(job_id.clone(), job);

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued existing job {} of type {}", job_id, job_type);

        self.wake_workers();

        Ok(job_id)
    }

    /// Dequeue a job for processing.
    pub async fn dequeue(&self, job_types: Option<&[String]>) -> Result<Option<Job>> {
        let _attempt_lease = self.attempt_gate.read().await;
        // Note: This is called frequently by worker pools, so we use trace level
        // to avoid log spam. Use debug level only when a job is actually dequeued.

        let Some(db_job) = self
            .job_repository
            .claim_next_pending_job(job_types)
            .await?
        else {
            return Ok(None);
        };
        let mut job = db_model_to_job(&db_job);
        job.status = JobStatus::Processing;
        if job.started_at.is_none() {
            job.started_at = Some(Utc::now());
        }

        // Resolve missing metadata (streamer_name, session_title)
        // These are not stored in the database, so we look them up from repositories
        self.resolve_job_metadata(&mut job).await;

        // Update cache
        self.jobs_cache.insert(job.id.clone(), job.clone());

        // Create cancellation token for this job (do not overwrite an existing token,
        // e.g. if a cancellation raced with dequeue).
        let attempt = self
            .cancellation_tokens
            .entry(job.id.clone())
            .or_default()
            .id();

        self.announce_started(&job, attempt);
        Ok(Some(job))
    }

    /// Count pending jobs, optionally filtered by job types.
    pub async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64> {
        self.job_repository.count_pending_jobs(job_types).await
    }

    /// Wait for a job to become available.
    pub async fn wait_for_job(&self) {
        self.notify.notified().await;
    }

    /// Mark a job as completed.
    pub async fn complete(&self, job_id: &str, result: JobResult) -> Result<()> {
        self.complete_if_processing(job_id, result).await?;
        Ok(())
    }

    /// Mark a job as completed and report whether it left an active state.
    pub(crate) async fn complete_if_processing(
        &self,
        job_id: &str,
        result: JobResult,
    ) -> Result<bool> {
        // Capture outputs for persistence before they are moved into cache/DB models.
        let outputs_for_persist = result.outputs.clone();
        let repo = &self.job_repository;
        let mut db_job = repo.get_job(job_id).await?;

        let completed_job_type = db_job.job_type.clone();
        let completed_session_id = db_job.session_id.clone();
        let completed_streamer_id = db_job.streamer_id.clone();

        if db_job.get_status() != Some(JobStatus::Processing) {
            self.finalize_cancelled_job(job_id);
            return Ok(false);
        }

        db_job.mark_completed();
        if !result.outputs.is_empty() {
            db_job.set_outputs(&result.outputs);
        }
        db_job.duration_secs = Some(result.duration_secs);

        // Calculate queue wait time (DB stores epoch millis)
        if let (created_ms, Some(started_ms)) = (db_job.created_at, db_job.started_at) {
            let wait_ms = started_ms.saturating_sub(created_ms);
            db_job.queue_wait_secs = Some((wait_ms as f64 / 1000.0).max(0.0));
        }

        let updated = repo
            .update_job_if_status(&db_job, JobStatus::Processing)
            .await?;
        if updated == 0 {
            self.finalize_cancelled_job(job_id);
            return Ok(false);
        }

        // The COMPLETED row above is the recovery boundary for DAG jobs. Execution logs and
        // their summary are auxiliary observability writes and cannot make a processor rerun.
        if !result.logs.is_empty() {
            match self
                .persist_logs_to_db(job_id, &result.logs, LogPersistence::Snapshot)
                .await
            {
                Ok(new_logs) => {
                    let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
                        db_job.execution_info.as_deref(),
                        JsonContext::JobField {
                            job_id: &db_job.id,
                            field: "execution_info",
                        },
                        "Invalid execution_info JSON; resetting to defaults",
                    );
                    update_log_summary(&mut exec_info, &new_logs);
                    extend_logs_capped(&mut exec_info, &new_logs);
                    match serde_json::to_string(&exec_info) {
                        Ok(execution_info) => {
                            if let Err(error) = repo
                                .update_job_execution_info(job_id, &execution_info)
                                .await
                            {
                                warn!(
                                    job_id = %job_id,
                                    error = %error,
                                    "Failed to persist completed job log summary"
                                );
                            }
                        }
                        Err(error) => {
                            warn!(
                                job_id = %job_id,
                                error = %error,
                                "Failed to serialize completed job log summary"
                            );
                        }
                    }
                }
                Err(error) => {
                    warn!(
                        job_id = %job_id,
                        error = %error,
                        "Failed to persist completed job execution logs"
                    );
                }
            }
        }

        // Persist thumbnail outputs to the session media_outputs table.
        //
        // This covers:
        // - Direct thumbnail jobs (job_type == "thumbnail")
        // - Preset-driven DAG steps (job_type == "thumbnail_<preset>")
        //
        // Segment persistence is handled by the download manager event path.
        if is_thumbnail_job_type(&completed_job_type)
            && let Some(session_id) = completed_session_id.as_deref()
        {
            for output_path in &outputs_for_persist {
                self.persist_thumbnail_output(session_id, output_path).await;
            }
        }

        // Persist per-file upload results and broadcast the terminal event
        // for upload job types (persist_upload_records itself no-ops for
        // everything else).
        if upload_kind_for_job_type(&completed_job_type).is_some() {
            self.persist_upload_records(
                job_id,
                &completed_job_type,
                completed_streamer_id.as_deref(),
                completed_session_id.as_deref(),
                &result.uploads,
            )
            .await;

            let (mut succeeded, mut failed, mut skipped) = (0u32, 0u32, 0u32);
            for item in &result.uploads {
                match item.status {
                    UploadItemStatus::Completed => succeeded += 1,
                    UploadItemStatus::Failed => failed += 1,
                    UploadItemStatus::Skipped => skipped += 1,
                }
            }
            self.emit_upload_event(UploadStatusEvent::Terminal {
                job_id: job_id.to_string(),
                streamer_id: completed_streamer_id.clone(),
                status: UploadTerminalStatus::Completed,
                files_succeeded: succeeded,
                files_failed: failed,
                files_skipped: skipped,
                error: None,
            });
        }
        if let Some((_, job)) = self.jobs_cache.remove(job_id) {
            self.emit_long_job_ended(&job);
        }

        // Cleanup in-memory tracking for this job.
        let _ = self.cancellation_tokens.remove(job_id);
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);

        self.decrement_depth(1);
        info!("Job {} completed in {:.2}s", job_id, result.duration_secs);

        Ok(true)
    }
    /// Mark a job as failed.
    pub async fn fail(&self, job_id: &str, error: &str) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                error,
                None,
                FailedStep::default(),
                FailureMetadata::DatabaseLog,
                &[],
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(job_id, error, "Job failed");
        }
        Ok(outcome)
    }

    /// Fail before processor invocation without rewriting unavailable or invalid metadata.
    pub async fn fail_execution_start(
        &self,
        job_id: &str,
        error: &str,
    ) -> Result<JobFailureOutcome> {
        self.fail_internal(
            job_id,
            error,
            None,
            FailedStep::default(),
            FailureMetadata::Preserve,
            &[],
        )
        .await
    }

    /// Mark a job as failed with step information for observability.
    /// Records the error message, failing step, and processor name in execution_info.
    /// `retry_after` is when the job's automatic retry is due, if one is planned.
    /// `uploads` carries the per-file results an upload processor reported for
    /// a partially failed batch; inputs it does not cover are recorded as failed
    /// with the job's error.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate part of the recorded failure"
    )]
    pub async fn fail_with_step_info(
        &self,
        job_id: &str,
        error: &str,
        retry_after: Option<DateTime<Utc>>,
        processor_name: Option<&str>,
        step_number: Option<u32>,
        total_steps: Option<u32>,
        uploads: &[UploadResultItem],
    ) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                error,
                retry_after,
                FailedStep {
                    processor_name,
                    step_number,
                    total_steps,
                },
                FailureMetadata::DatabaseLog,
                uploads,
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(
                job_id,
                step_number, total_steps, processor_name, error, "Job failed at pipeline step"
            );
        }
        Ok(outcome)
    }

    /// Get a job by ID.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        let repo = &self.job_repository;
        match repo.get_job(id).await {
            Ok(db_job) => {
                let job = db_model_to_job(&db_job);
                if let Some(info) = &job.execution_info
                    && !info.logs.is_empty()
                {
                    let (_, rows) = repo.list_execution_logs(id, &Pagination::new(0, 0)).await?;
                    if rows > 0 {
                        let owner: Arc<str> = Arc::from(id);
                        for entry in &info.logs {
                            entry.mark_persisted(owner.clone());
                        }
                    }
                    // An empty log table may be a legacy snapshot-only job. Its
                    // entries must be copied on the next snapshot update, before
                    // new rows make list_job_logs stop using its legacy fallback.
                }
                Ok(Some(job))
            }
            Err(Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List job execution logs (paged) from `job_execution_logs`, falling back to
    /// the logs that legacy rows kept only in `execution_info`.
    pub async fn list_job_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobLogEntry>, u64)> {
        let repo = &self.job_repository;
        let (rows, total) = repo.list_execution_logs(job_id, pagination).await?;

        let logs = rows
            .into_iter()
            .map(|row| {
                let timestamp = crate::database::time::ms_to_datetime(row.created_at);

                if row.level.is_some() || row.message.is_some() {
                    let level = row
                        .level
                        .as_deref()
                        .map(|l| match l.to_ascii_uppercase().as_str() {
                            "DEBUG" => LogLevel::Debug,
                            "WARN" | "WARNING" => LogLevel::Warn,
                            "ERROR" => LogLevel::Error,
                            _ => LogLevel::Info,
                        })
                        .unwrap_or(LogLevel::Info);

                    return JobLogEntry {
                        timestamp,
                        level,
                        message: row.message.unwrap_or(row.entry),
                        persisted_for: Arc::default(),
                    };
                }

                if let Ok(entry) = serde_json::from_str::<JobLogEntry>(&row.entry) {
                    return entry;
                }

                if let Ok(entry) = serde_json::from_str::<DbLogEntry>(&row.entry) {
                    let level = match entry.level.to_ascii_uppercase().as_str() {
                        "DEBUG" => LogLevel::Debug,
                        "WARN" | "WARNING" => LogLevel::Warn,
                        "ERROR" => LogLevel::Error,
                        _ => LogLevel::Info,
                    };

                    return JobLogEntry {
                        timestamp,
                        level,
                        message: entry.message,
                        persisted_for: Arc::default(),
                    };
                }

                JobLogEntry {
                    timestamp,
                    level: LogLevel::Info,
                    message: row.entry,
                    persisted_for: Arc::default(),
                }
            })
            .inspect(|entry| entry.mark_persisted(Arc::from(job_id)))
            .collect();

        if total > 0 {
            return Ok((logs, total));
        }

        // Fallback for legacy rows that only stored logs in `job.execution_info`.
        let exec_info = repo.get_job_execution_info(job_id).await?;
        if let Some(exec_info) = exec_info
            && let Ok(parsed) = serde_json::from_str::<JobExecutionInfo>(&exec_info)
        {
            let total = parsed.logs.len() as u64;
            let start = pagination.offset as usize;
            let limit = pagination.limit as usize;
            let page: Vec<_> = parsed
                .logs
                .iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect();
            return Ok((page, total));
        }

        Ok((vec![], 0))
    }

    /// List jobs with filters and pagination.
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        let (db_jobs, total) = self
            .job_repository
            .list_jobs_filtered(filters, pagination)
            .await?;
        let jobs = db_jobs.iter().map(db_model_to_job).collect();
        Ok((jobs, total))
    }

    /// List jobs with filters and pagination, without running a total `COUNT(*)`.
    pub async fn list_jobs_page(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<Job>> {
        let db_jobs = self
            .job_repository
            .list_jobs_page_filtered(filters, pagination)
            .await?;
        Ok(db_jobs.iter().map(db_model_to_job).collect())
    }

    /// Failed jobs whose scheduled retry is due at `now`, earliest first.
    pub async fn list_due_retries(&self, now: DateTime<Utc>) -> Result<Vec<Job>> {
        Ok(self
            .job_repository
            .list_jobs_due_for_retry(now.timestamp_millis())
            .await?
            .iter()
            .map(db_model_to_job)
            .collect())
    }

    /// Drop the scheduled retry of `job_id`; the job stays failed.
    pub async fn clear_scheduled_retry(&self, job_id: &str) -> Result<()> {
        self.job_repository.clear_job_retry(job_id).await?;
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.retry_after = None;
        }
        Ok(())
    }

    /// Retry a failed or cancelled job.
    /// Returns error if job is not in a retryable terminal status.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        let attempt_lease = self.attempt_gate.write().await;
        self.job_repository.reset_job_for_retry(id).await?;

        let _ = self.cancellation_tokens.remove(id);
        let _ = self.persisted_log_cursor.remove(id);
        self.progress_cache.remove(id);
        self.jobs_cache.remove(id);

        self.depth.fetch_add(1, Ordering::SeqCst);
        drop(attempt_lease);
        self.wake_workers();

        let updated_job = db_model_to_job(&self.job_repository.get_job(id).await?);
        info!("Job {} retried (attempt {})", id, updated_job.retry_count);
        Ok(updated_job)
    }

    /// Cancel a job.
    /// Returns the cancelled job, or error for Completed/Failed jobs.
    pub async fn cancel_job(&self, id: &str) -> Result<Job> {
        // Get the job
        let job = self
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        // Validate job is not in terminal status
        if job.status == JobStatus::Completed || job.status == JobStatus::Failed {
            return Err(Error::InvalidStateTransition {
                from: job.status.as_str().to_string(),
                to: "CANCELLED".to_string(),
            });
        }

        let is_processing = job.status == JobStatus::Processing;

        // Signal cancellation for processing jobs (ensure a token exists, since cancellation can
        // race with the dequeue path that creates tokens).
        if is_processing {
            let token = self.cancellation_tokens.entry(id.to_string()).or_default();
            token.cancel();
        } else if job.status == JobStatus::Cancelled
            && let Some(token) = self.cancellation_tokens.get(id)
        {
            token.cancel();
        }

        let updated = self.job_repository.mark_job_cancelled(id).await?;

        // Update cache and get the updated job
        let cancelled_job = {
            if let Some(mut cached_job) = self.jobs_cache.get_mut(id) {
                cached_job.status = JobStatus::Cancelled;
                cached_job.completed_at = Some(Utc::now());
                cached_job.clone()
            } else {
                // Job not in cache, return original with updated status
                let mut updated = job.clone();
                updated.status = JobStatus::Cancelled;
                updated.completed_at = Some(Utc::now());
                updated
            }
        };

        let keep_tracking = is_processing || self.cancellation_tokens.contains_key(id);
        if !keep_tracking {
            let _ = self.cancellation_tokens.remove(id);
            let _ = self.persisted_log_cursor.remove(id);
            self.progress_cache.remove(id);
        }

        self.jobs_cache.remove(id);

        if updated > 0 {
            self.decrement_depth(1);

            // job was fetched before the transition, so its type/streamer
            // fields are intact even though the cache entry is gone now.
            self.emit_upload_cancelled(id, &job.job_type, &job.streamer_id);
            self.emit_long_job_ended(&job);
        }

        info!("Job {} cancelled", id);
        Ok(cancelled_job)
    }

    /// Delete a job.
    /// Removes from database and cache.
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        // Try to get from cache first to check status
        if let Some(job) = self.jobs_cache.get(id) {
            match job.status {
                JobStatus::Pending | JobStatus::Processing => {
                    return Err(Error::InvalidStateTransition {
                        from: job.status.as_str().to_string(),
                        to: "DELETED".to_string(),
                    });
                }
                _ => {} // Terminal states are fine for deletion
            }
        } else {
            // If not in cache, check DB
            match self.job_repository.get_job(id).await {
                Ok(job) => {
                    let status = match job.get_status() {
                        Some(s) => s,
                        None => {
                            return Err(Error::Other(format!(
                                "Invalid job status for job {}: {}",
                                id, job.status
                            )));
                        }
                    };

                    match status {
                        JobStatus::Pending | JobStatus::Processing => {
                            return Err(Error::InvalidStateTransition {
                                from: status.as_str().to_string(),
                                to: "DELETED".to_string(),
                            });
                        }
                        _ => {}
                    }
                }
                Err(Error::NotFound { .. }) => {
                    // Job doesn't exist, so deletion is trivially successful
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }

        // Proceed with deletion

        // 1. Delete from database
        self.job_repository.delete_job(id).await?;

        // 2. Remove from in-memory components
        self.jobs_cache.remove(id);
        self.cancellation_tokens.remove(id);
        self.persisted_log_cursor.remove(id);
        self.progress_cache.remove(id);

        info!("Job {} deleted", id);
        Ok(())
    }

    /// Cancel all jobs in a pipeline.
    /// Returns the list of cancelled jobs.
    pub async fn cancel_pipeline(&self, pipeline_id: &str) -> Result<Vec<Job>> {
        let repo = &self.job_repository;
        // Get all jobs in the pipeline before cancelling
        let db_jobs = repo.get_jobs_by_pipeline(pipeline_id).await?;

        let count = repo.cancel_jobs_by_pipeline(pipeline_id).await?;
        info!(
            "Cancelled {} jobs in pipeline {} (database)",
            count, pipeline_id
        );

        // Convert to Job and collect cancelled ones
        let mut cancelled_jobs = Vec::new();
        for db_job in db_jobs {
            if matches!(
                db_job.get_status(),
                Some(JobStatus::Pending | JobStatus::Processing)
            ) {
                let mut job = db_model_to_job(&db_job);
                job.status = JobStatus::Cancelled;
                job.completed_at = Some(Utc::now());
                cancelled_jobs.push(job);
            }
        }

        // Update cache for all jobs in pipeline
        let mut ids: Vec<String> = Vec::new();
        for entry in self.jobs_cache.iter() {
            let job = entry.value();
            if job.pipeline_id.as_deref() == Some(pipeline_id)
                && (job.status == JobStatus::Pending || job.status == JobStatus::Processing)
            {
                ids.push(entry.key().clone());
            }
        }

        for id in &ids {
            let mut keep_tracking = self.cancellation_tokens.contains_key(id);
            if let Some(job) = self.jobs_cache.get(id) {
                if job.pipeline_id.as_deref() != Some(pipeline_id) {
                    continue;
                }

                if job.status == JobStatus::Processing {
                    let token = self.cancellation_tokens.entry(id.to_string()).or_default();
                    token.cancel();
                    keep_tracking = true;
                }
            }

            if !keep_tracking {
                let _ = self.cancellation_tokens.remove(id);
                let _ = self.persisted_log_cursor.remove(id);
                self.progress_cache.remove(id);
            }

            self.jobs_cache.remove(id);
        }

        self.decrement_depth(count as usize);

        for job in &cancelled_jobs {
            self.emit_upload_cancelled(&job.id, &job.job_type, &job.streamer_id);
            self.emit_long_job_ended(job);
        }

        info!(
            "Cancelled {} jobs in pipeline {}",
            cancelled_jobs.len(),
            pipeline_id
        );
        Ok(cancelled_jobs)
    }

    /// Get the cancellation token for a job.
    pub async fn get_cancellation_token(&self, job_id: &str) -> Option<CancellationToken> {
        self.cancellation_tokens
            .get(job_id)
            .map(|t| t.token.clone())
    }

    /// The current attempt of a dequeued job, for the worker that must later
    /// finalize exactly that attempt.
    pub(crate) fn attempt_token(&self, job_id: &str) -> Option<AttemptToken> {
        self.cancellation_tokens.get(job_id).map(|t| t.clone())
    }

    /// Drop every in-memory trace of jobs whose rows have been deleted.
    ///
    /// `DagScheduler::delete_dag` removes job rows through
    /// `JobRepository::delete_jobs_by_pipeline`, so no terminal transition will ever run for
    /// them and nothing else clears what `enqueue`, `dequeue` and `progress_reporter` left
    /// behind. Cancelling the token stops a worker that is still inside `Processor::process`
    /// for a deleted job, and any entry still in an active state is removed from `depth`,
    /// which feeds `depth_status` and the throttle controller.
    pub(crate) fn forget_jobs(&self, job_ids: &[String]) {
        let mut still_active = 0usize;

        for job_id in job_ids {
            if let Some((_, job)) = self.jobs_cache.remove(job_id)
                && matches!(job.status, JobStatus::Pending | JobStatus::Processing)
            {
                still_active += 1;
            }
            if let Some((_, token)) = self.cancellation_tokens.remove(job_id) {
                token.cancel();
            }
            let _ = self.persisted_log_cursor.remove(job_id);
            self.progress_cache.remove(job_id);
        }

        if still_active > 0 {
            self.decrement_depth(still_active);
        }
    }

    pub fn finalize_cancelled_job(&self, job_id: &str) {
        let _ = self.cancellation_tokens.remove(job_id);
        self.clear_attempt_tracking(job_id);
    }

    /// Finalize `attempt` unless a retry has already dequeued a successor under
    /// the same job ID. The successor then owns the job's token, log cursor,
    /// progress and cache entry, which a late cleanup must not remove.
    pub(crate) async fn finalize_attempt(&self, job_id: &str, attempt: &AttemptToken) {
        let _attempt_lease = self.attempt_gate.write().await;
        let removed = self
            .cancellation_tokens
            .remove_if(job_id, |_, current| current.attempt == attempt.attempt);
        if removed.is_none() && self.cancellation_tokens.contains_key(job_id) {
            return;
        }
        self.clear_attempt_tracking(job_id);
    }

    fn clear_attempt_tracking(&self, job_id: &str) {
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);
        self.jobs_cache.remove(job_id);
    }

    /// Recover jobs from database on startup.
    /// Loads pending jobs and resets processing jobs to pending.
    /// Cancelled jobs remain terminal until explicitly retried.
    pub async fn recover_jobs(&self) -> Result<usize> {
        let repo = &self.job_repository;

        // Reset processing jobs to pending (they were interrupted by shutdown)
        let reset_processing = repo.reset_processing_jobs().await?;
        if reset_processing > 0 {
            info!("Reset {} processing jobs to pending", reset_processing);
        }

        // No worker has started, so every staged file still recorded belongs to
        // an attempt of an earlier process that never finalized. Listing those
        // is best effort and must not hold back job recovery.
        if let Some(staged) = self.staged_output_repo.get() {
            match staged
                .mark_interrupted(crate::database::time::now_ms())
                .await
            {
                Ok(0) => {}
                Ok(interrupted) => warn!(
                    interrupted,
                    "Interrupted jobs may have left partial temporary files; they are listed for review"
                ),
                Err(error) => warn!(%error, "Failed to mark staged outputs of interrupted jobs"),
            }
        }

        // Load pending jobs into cache
        let filters = JobFilters {
            status: Some(JobStatus::Pending),
            ..Default::default()
        };
        let pagination = Pagination::new(10000, 0); // Load all pending jobs

        let (db_jobs, total) = repo.list_jobs_filtered(&filters, &pagination).await?;

        self.jobs_cache.clear();
        self.cancellation_tokens.clear();
        self.persisted_log_cursor.clear();
        self.progress_cache.clear();
        for db_job in &db_jobs {
            let job = db_model_to_job(db_job);
            self.jobs_cache.insert(job.id.clone(), job);
        }

        // Update depth counter
        self.depth.store(total as usize, Ordering::SeqCst);

        info!("Recovered {} pending jobs from database", total);
        Ok(total as usize)
    }

    /// Read the current processing count without scanning completed job history.
    pub async fn count_processing_jobs(&self) -> Result<u64> {
        self.job_repository
            .count_jobs(&JobFilters {
                status: Some(crate::database::models::JobStatus::Processing),
                ..Default::default()
            })
            .await
    }

    /// Get job statistics.
    pub async fn get_stats(&self) -> Result<JobStats> {
        let counts = self.job_repository.get_job_counts_by_status().await?;
        let avg_processing_time = self.job_repository.get_avg_processing_time().await?;

        Ok(JobStats {
            pending: counts.pending,
            processing: counts.processing,
            completed: counts.completed,
            failed: counts.failed,
            cancelled: counts.cancelled,
            avg_processing_time_secs: avg_processing_time,
        })
    }

    // ========================================================================
    // Fan-out and Multi-input Support Methods
    // ========================================================================

    /// Split a multi-input job into separate jobs for single-input processors.
    /// Creates one job per input file, all sharing the same pipeline context.
    /// Returns the IDs of the newly created jobs.
    pub async fn split_job_for_single_input(&self, job: &Job) -> Result<Vec<String>> {
        if job.inputs.len() <= 1 {
            // No splitting needed
            return Ok(vec![job.id.clone()]);
        }

        let mut created_job_ids = Vec::new();

        for input in job.inputs.iter() {
            // Create a new job for each input
            let mut split_job = Job::new_pipeline_step(
                job.job_type.clone(),
                vec![input.clone()],
                vec![], // Outputs will be determined by processor
                job.streamer_id.clone(),
                job.session_id.clone(),
                job.pipeline_id.clone(),
            )
            .with_priority(job.priority)
            .with_config(job.config.clone().unwrap_or_else(|| "{}".to_string()));

            if let Some(name) = job.streamer_name.as_ref() {
                split_job = split_job.with_streamer_name(name.clone());
            }
            if let Some(title) = job.session_title.as_ref() {
                split_job = split_job.with_session_title(title.clone());
            }
            if let Some(platform) = job.platform.as_ref() {
                split_job = split_job.with_platform(platform.clone());
            }
            if let Some(session_start) = job.session_start {
                split_job = split_job.with_session_start(session_start);
            }
            split_job.manifest = job.manifest.clone();

            match self.enqueue(split_job).await {
                Ok(job_id) => created_job_ids.push(job_id),
                Err(error) => {
                    self.rollback_split_jobs(&created_job_ids).await;
                    return Err(error);
                }
            }
        }

        // Mark the original job as completed (it was split)
        let repo = &self.job_repository;
        let result = async {
            let mut db_job = repo.get_job(&job.id).await?;
            db_job.mark_completed();
            db_job.set_outputs(&[]); // No outputs, job was split
            repo.update_job(&db_job).await
        }
        .await;
        if let Err(error) = result {
            self.rollback_split_jobs(&created_job_ids).await;
            return Err(error);
        }

        // Remove cancellation token for original job
        let _ = self.cancellation_tokens.remove(&job.id);
        let _ = self.persisted_log_cursor.remove(&job.id);
        self.progress_cache.remove(&job.id);
        self.jobs_cache.remove(&job.id);

        // Adjust queue depth to account for the original job completing.
        // The split jobs are enqueued and already increment depth.
        self.decrement_depth(1);

        info!(
            "Split job {} into {} jobs for single-input processing",
            job.id,
            created_job_ids.len()
        );

        Ok(created_job_ids)
    }

    async fn rollback_split_jobs(&self, job_ids: &[String]) {
        for job_id in job_ids {
            if let Err(error) = self.cancel_job(job_id).await {
                warn!(
                    %job_id,
                    %error,
                    "Failed to cancel a split job after fan-out failed"
                );
            }
        }
    }

    /// Record produced artifacts for observability. These paths do not convey
    /// temporary-file ownership and must never authorize generic failure cleanup.
    pub async fn record_produced_items(&self, job_id: &str, outputs: &[String]) -> Result<()> {
        let repo = &self.job_repository;
        let exec_info_str = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            exec_info_str.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );

        // Retain produced artifacts across attempts.
        exec_info.items_produced.extend(outputs.iter().cloned());

        let exec_info_json = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &exec_info_json)
            .await?;

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            let exec_info = job
                .execution_info
                .get_or_insert_with(JobExecutionInfo::default);
            exec_info.items_produced.extend(outputs.iter().cloned());
        }

        Ok(())
    }

    /// Patch the current processor before its log collector starts. Preserve all
    /// stored metadata, including unknown fields, without replaying embedded logs.
    pub async fn begin_execution(&self, job_id: &str, processor: &str) -> Result<()> {
        let repo = &self.job_repository;
        let stored = repo.get_job_execution_info(job_id).await?;
        let mut info = match stored {
            Some(raw) if !raw.is_empty() => serde_json::from_str::<JobExecutionInfo>(&raw)?,
            // Legacy writers also used an empty string for absent metadata.
            // Nonempty malformed documents remain errors rather than losing history.
            _ => JobExecutionInfo::default(),
        };
        info.current_processor = Some(processor.to_owned());
        repo.update_job_execution_info(job_id, &serde_json::to_string(&info)?)
            .await?;
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.execution_info
                .get_or_insert_with(JobExecutionInfo::default)
                .current_processor = Some(processor.to_owned());
        }
        Ok(())
    }

    /// Replace execution info with an explicit snapshot, persisting new log entries.
    pub async fn update_execution_info(
        &self,
        job_id: &str,
        exec_info: JobExecutionInfo,
    ) -> Result<()> {
        let mut exec_info = exec_info;

        if !exec_info.logs.is_empty() {
            // make_contiguous() allows VecDeque to be used as a slice
            let new_logs = self
                .persist_logs_to_db(
                    job_id,
                    exec_info.logs.make_contiguous(),
                    LogPersistence::Snapshot,
                )
                .await?;
            update_log_summary(&mut exec_info, &new_logs);
            cap_logs_in_place(&mut exec_info.logs, EXECUTION_INFO_MAX_LOGS);
        }

        let exec_info_json = serde_json::to_string(&exec_info)?;
        self.job_repository
            .update_job_execution_info(job_id, &exec_info_json)
            .await?;

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.execution_info = Some(exec_info);
        }

        Ok(())
    }

    /// Get the current queue depth.
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
    }

    fn decrement_depth(&self, by: usize) {
        if by == 0 {
            return;
        }
        let _ = self
            .depth
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_sub(by))
            });
    }

    /// Get the queue depth status.
    pub fn depth_status(&self) -> QueueDepthStatus {
        let depth = self.depth();
        if depth >= self.config.critical_threshold {
            QueueDepthStatus::Critical
        } else if depth >= self.config.warning_threshold {
            QueueDepthStatus::Warning
        } else {
            QueueDepthStatus::Normal
        }
    }

    /// Check if the queue is at warning level.
    pub fn is_warning(&self) -> bool {
        self.depth() >= self.config.warning_threshold
    }

    /// Check if the queue is at critical level.
    pub fn is_critical(&self) -> bool {
        self.depth() >= self.config.critical_threshold
    }

    /// Get a notifier for new jobs.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Wake workers after a job becomes runnable.
    ///
    /// `notify_waiters` wakes every currently parked worker, so every pool
    /// (CPU, IO and the one-worker serial IO pool, which share this notifier
    /// but accept different job types) re-checks the queue. It stores no permit, so `notify_one` also
    /// runs for a worker that is mid-iteration and re-enters
    /// `Notify::notified` only after this call.
    fn wake_workers(&self) {
        self.notify.notify_waiters();
        self.notify.notify_one();
    }

    async fn persist_failure_metadata(
        &self,
        repo: &dyn JobRepository,
        job_id: &str,
        step: FailedStep<'_>,
        log_entry: &JobLogEntry,
    ) -> Result<()> {
        let FailedStep {
            processor_name,
            step_number,
            total_steps,
        } = step;
        let exec_info_str = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            exec_info_str.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );

        if let Some(name) = processor_name {
            exec_info.current_processor = Some(name.to_string());
        }
        if let Some(step) = step_number {
            exec_info.current_step = Some(step);
        }
        if let Some(total) = total_steps {
            exec_info.total_steps = Some(total);
        }

        extend_logs_capped(&mut exec_info, std::slice::from_ref(log_entry));
        update_log_summary(&mut exec_info, std::slice::from_ref(log_entry));

        self.persist_logs_to_db(
            job_id,
            std::slice::from_ref(log_entry),
            LogPersistence::Append,
        )
        .await?;

        let exec_info_json = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &exec_info_json)
            .await?;
        Ok(())
    }

    async fn fail_internal(
        &self,
        job_id: &str,
        error: &str,
        retry_after: Option<DateTime<Utc>>,
        step: FailedStep<'_>,
        metadata: FailureMetadata,
        uploads: &[UploadResultItem],
    ) -> Result<JobFailureOutcome> {
        let log_entry = JobLogEntry::error(format!("Job failed: {}", error));

        // Capture everything needed to persist failed per-file results and
        // emit the terminal event before cache cleanup evicts the job.
        let failed_upload_ctx = self.jobs_cache.get(job_id).and_then(|job| {
            upload_kind_for_job_type(&job.job_type).map(|_| FailedUploadContext {
                job_type: job.job_type.clone(),
                streamer_id: non_empty(&job.streamer_id),
                session_id: non_empty(&job.session_id),
                inputs: job.inputs.clone(),
            })
        });

        // The failure write can make a retry due at once. `retry_job` needs the
        // exclusive lease, so it cannot hand the job to a successor attempt
        // before the cleanup below has removed this attempt's tracking.
        let _attempt_lease = self.attempt_gate.read().await;

        let updated = self
            .job_repository
            .mark_job_failed(
                job_id,
                error,
                retry_after.map(|retry_after| retry_after.timestamp_millis()),
            )
            .await?;
        if updated == 0 {
            self.finalize_cancelled_job(job_id);
            return Ok(JobFailureOutcome::Unchanged);
        }

        // FAILED is already durable. Auxiliary metadata errors must not suppress a
        // retry or skip terminal cleanup after this transition has won.
        if metadata != FailureMetadata::Preserve
            && let Err(error) = self
                .persist_failure_metadata(self.job_repository.as_ref(), job_id, step, &log_entry)
                .await
        {
            warn!(job_id, %error, "Failed to persist failed job execution metadata");
        }

        // Persist per-file rows only after this failure transition wins.
        // A concurrent cancellation that wins first returns above without
        // leaving FAILED upload records on a CANCELLED job. Results the
        // processor reported for individual files are kept as reported, so a
        // file that did transfer before the batch failed is not recorded as
        // failed; every other input failed with the job's error.
        let mut file_counts = (0u32, 0u32, 0u32);
        if let Some(context) = &failed_upload_ctx {
            let reported: HashMap<&str, &UploadResultItem> = uploads
                .iter()
                .map(|item| (item.local_path.as_str(), item))
                .collect();
            let items: Vec<UploadResultItem> = context
                .inputs
                .iter()
                .map(|path| {
                    reported
                        .get(path.as_str())
                        .map(|item| (*item).clone())
                        .unwrap_or_else(|| UploadResultItem {
                            local_path: path.clone(),
                            remote_path: None,
                            size_bytes: None,
                            status: UploadItemStatus::Failed,
                            error: Some(error.to_string()),
                        })
                })
                .collect();
            for item in &items {
                match item.status {
                    UploadItemStatus::Completed => file_counts.0 += 1,
                    UploadItemStatus::Failed => file_counts.1 += 1,
                    UploadItemStatus::Skipped => file_counts.2 += 1,
                }
            }
            self.persist_upload_records(
                job_id,
                &context.job_type,
                context.streamer_id.as_deref(),
                context.session_id.as_deref(),
                &items,
            )
            .await;
        }
        if let Some((_, job)) = self.jobs_cache.remove(job_id) {
            self.emit_long_job_ended(&job);
        }

        // Remove cancellation token
        let _ = self.cancellation_tokens.remove(job_id);
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);

        self.decrement_depth(1);

        if let Some(context) = failed_upload_ctx {
            self.emit_upload_event(UploadStatusEvent::Terminal {
                job_id: job_id.to_string(),
                streamer_id: context.streamer_id,
                status: UploadTerminalStatus::Failed,
                files_succeeded: file_counts.0,
                files_failed: file_counts.1,
                files_skipped: file_counts.2,
                error: Some(error.to_string()),
            });
        }
        Ok(JobFailureOutcome::Transitioned)
    }
}

impl Drop for JobQueue {
    fn drop(&mut self) {
        self.progress_shutdown.cancel();
        if let Some(task) = self.progress_aggregator.get_mut().take() {
            task.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_progress_aggregator(
    repo: Arc<dyn JobRepository>,
    long_job_after: std::time::Duration,
    mut rx: tokio::sync::mpsc::Receiver<JobProgressUpdate>,
    jobs_cache: Arc<DashMap<String, Job>>,
    cancellation_tokens: Arc<DashMap<String, AttemptToken>>,
    progress_cache: Arc<DashMap<String, JobProgressSnapshot>>,
    upload_broadcaster: Arc<std::sync::OnceLock<UploadStatusBroadcaster>>,
    shutdown: CancellationToken,
) -> Option<tokio::task::JoinHandle<()>> {
    if tokio::runtime::Handle::try_current().is_err() {
        // Some unit tests construct JobQueue outside a Tokio runtime; they run
        // without progress tracking.
        return None;
    }

    Some(tokio::spawn(async move {
        let mut pending: HashMap<String, JobProgressSnapshot> = HashMap::new();
        let flush_every = std::time::Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS);
        let mut tick = tokio::time::interval(flush_every);
        let mut heartbeat = tokio::time::interval_at(
            tokio::time::Instant::now() + HEARTBEAT_INTERVAL,
            HEARTBEAT_INTERVAL,
        );
        let mut queue_poll = tokio::time::interval(UPLOAD_QUEUE_POLL_INTERVAL);
        // Last count broadcast; `None` forces a send on the first poll that
        // has a subscriber.
        let mut last_pending_uploads: Option<u32> = None;
        // The count can wait on the database for a connection, so it runs as
        // its own branch rather than inside a handler: meanwhile the loop
        // keeps draining progress, flushing and heartbeating, and shutdown
        // still stops it.
        let mut queue_count: Option<BoxFuture<'_, Result<u32>>> = None;

        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                _ = heartbeat.tick() => {
                    if let Some(broadcaster) = upload_broadcaster.get()
                        && broadcaster.has_subscribers()
                    {
                        for event in heartbeat_events(
                            &jobs_cache,
                            &cancellation_tokens,
                            &progress_cache,
                            Utc::now(),
                            long_job_after,
                        ) {
                            broadcaster.send(event);
                        }
                    }
                }
                _ = queue_poll.tick() => {
                    let Some(broadcaster) = upload_broadcaster.get() else {
                        continue;
                    };
                    if !broadcaster.has_subscribers() {
                        // New subscribers get the count from their snapshot;
                        // re-sync on the next poll that has one. A count still
                        // in flight would set the remembered value for nobody.
                        last_pending_uploads = None;
                        queue_count = None;
                        continue;
                    }
                    if queue_count.is_none() {
                        queue_count =
                            Some(count_pending_uploads(repo.as_ref()).boxed());
                    }
                }
                Some(result) = OptionFuture::from(queue_count.as_mut()) => {
                    queue_count = None;
                    match result {
                        Ok(pending) if last_pending_uploads != Some(pending) => {
                            last_pending_uploads = Some(pending);
                            if let Some(broadcaster) = upload_broadcaster.get() {
                                broadcaster.send(UploadStatusEvent::Queue { pending });
                            }
                        }
                        Ok(_) => {}
                        Err(error) => warn!(%error, "Failed to count pending uploads"),
                    }
                }
                _ = tick.tick() => {
                    if pending.is_empty() {
                        continue;
                    }
                    let Some(broadcaster) = upload_broadcaster.get() else {
                        continue;
                    };
                    let now = Utc::now();
                    for (job_id, snapshot) in pending.drain() {
                        // A job can reach a terminal state between receive and
                        // this flush tick, and its terminal WS event is already
                        // out, so a progress event now would outlive it.
                        // cancel_job cancels the token before the worker
                        // finalizes, hence the is_cancelled check on top of
                        // plain presence.
                        let job_is_live = cancellation_tokens
                            .get(&job_id)
                            .is_some_and(|token| !token.is_cancelled());
                        if !job_is_live {
                            continue;
                        }

                        // Snapshots of upload processors and long jobs (see
                        // is_long_job) double as live WS events. Publishing on
                        // the flush tick reuses this loop's coalescing instead
                        // of adding a second throttle.
                        let upload = matches!(
                            snapshot.kind,
                            ProgressKind::Rclone | ProgressKind::BaiduPcs
                        );
                        let (long_job, streamer_id) = jobs_cache
                            .get(&job_id)
                            .map(|job| {
                                (
                                    is_long_job(&job, now, long_job_after),
                                    non_empty(&job.streamer_id),
                                )
                            })
                            .unwrap_or_default();
                        if upload || long_job {
                            broadcaster.send(progress_event(
                                long_job,
                                job_id,
                                streamer_id,
                                snapshot,
                            ));
                        }
                    }
                }
                update = rx.recv() => {
                    let Some(update) = update else { break; };
                    // Only retain progress for actively-processing jobs. This prevents
                    // late progress messages (after completion/cancellation) from reintroducing
                    // entries into the in-memory cache. The token is also checked for
                    // cancellation: cancel_job cancels it but leaves it in the map until the
                    // worker calls finalize_attempt.
                    let job_is_live = cancellation_tokens
                        .get(&update.job_id)
                        .is_some_and(|token| !token.is_cancelled());
                    if !job_is_live {
                        continue;
                    }
                    // pending only feeds the WS broadcast, so skip the clone
                    // while nobody is listening.
                    if upload_broadcaster
                        .get()
                        .is_some_and(|broadcaster| broadcaster.has_subscribers())
                    {
                        pending.insert(update.job_id.clone(), update.snapshot.clone());
                    }
                    progress_cache.insert(update.job_id, update.snapshot);
                }
            }
        }
    }))
}

/// Job statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JobStats {
    /// Number of pending jobs.
    pub pending: u64,
    /// Number of processing jobs.
    pub processing: u64,
    /// Number of completed jobs.
    pub completed: u64,
    /// Number of failed jobs.
    pub failed: u64,
    pub cancelled: u64,
    /// Average processing time in seconds.
    pub avg_processing_time_secs: Option<f64>,
}

/// Placeholder metadata carried in the job `state` JSON column.
#[derive(Debug, Clone, Default)]
pub(crate) struct JobStateMeta {
    pub(crate) streamer_name: Option<String>,
    pub(crate) session_title: Option<String>,
    pub(crate) platform: Option<String>,
    pub(crate) session_start: Option<DateTime<Utc>>,
    pub(crate) manifest: Option<Arc<PipelineInputManifest>>,
    /// Per-attempt timeout of a workflow step job; not a placeholder source.
    pub(crate) timeout_secs: Option<u64>,
}

impl JobStateMeta {
    /// True when at least one placeholder field carries a value. The manifest
    /// is not a placeholder source: DAG recovery reads it from the DAG row.
    pub(crate) fn has_any(&self) -> bool {
        self.streamer_name.is_some()
            || self.session_title.is_some()
            || self.platform.is_some()
            || self.session_start.is_some()
    }
}

/// Serialize a job's placeholder metadata into the `state` column JSON.
///
/// Single source of the state shape, together with its inverse
/// [`parse_job_state`]: used by [`job_to_db_model`] at persist time, by
/// [`JobQueue::resolve_job_metadata`] when writing back-filled values, and
/// by `DagScheduler::create_step_job`.
pub(crate) fn job_state_json(job: &Job) -> String {
    if job.streamer_name.is_some()
        || job.session_title.is_some()
        || job.platform.is_some()
        || job.session_start.is_some()
        || job.manifest.is_some()
        || job.timeout_secs.is_some()
    {
        let mut state = serde_json::json!({
            "streamer_name": job.streamer_name.clone(),
            "session_title": job.session_title.clone(),
            "platform": job.platform.clone(),
            "session_start_ms": job.session_start.as_ref().map(|dt| dt.timestamp_millis()),
        });
        if let Some(timeout_secs) = job.timeout_secs {
            state["timeout_secs"] = serde_json::json!(timeout_secs);
        }
        if let Some(manifest) = job.manifest.as_deref() {
            state["manifest"] = match serde_json::to_value(manifest) {
                Ok(value) => value,
                Err(error) => {
                    warn!(job_id = %job.id, %error, "Failed to serialize job manifest; storing null");
                    serde_json::Value::Null
                }
            };
        }
        state.to_string()
    } else {
        "{}".to_string()
    }
}

/// Parse the placeholder metadata out of a job `state` JSON string.
///
/// Inverse of [`job_state_json`]; malformed JSON or missing keys yield
/// all-`None`. Used by [`db_model_to_job`] and
/// `DagScheduler::recover_placeholder_metadata`.
pub(crate) fn parse_job_state(state: &str) -> JobStateMeta {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(state) else {
        return JobStateMeta::default();
    };
    let Some(obj) = value.as_object() else {
        return JobStateMeta::default();
    };

    let get_str = |key: &str| {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(ToString::to_string)
    };

    JobStateMeta {
        streamer_name: get_str("streamer_name"),
        session_title: get_str("session_title"),
        platform: get_str("platform"),
        session_start: obj
            .get("session_start_ms")
            .and_then(|v| v.as_i64())
            .map(crate::database::time::ms_to_datetime),
        manifest: obj
            .get("manifest")
            .filter(|value| !value.is_null())
            .and_then(|value| {
                serde_json::from_value::<PipelineInputManifest>(value.clone())
                    .inspect_err(|error| warn!(%error, "Invalid manifest in job state; ignoring"))
                    .ok()
            })
            .map(Arc::new),
        timeout_secs: obj.get("timeout_secs").and_then(|v| v.as_u64()),
    }
}

/// Convert a Job to JobDbModel.
fn job_to_db_model(job: &Job) -> JobDbModel {
    let inputs_json = json::to_string_or_fallback(
        &job.inputs,
        "[]",
        JsonContext::JobField {
            job_id: &job.id,
            field: "inputs",
        },
        "Failed to serialize job inputs; storing empty list",
    );
    let outputs_json = json::to_string_or_fallback(
        &job.outputs,
        "[]",
        JsonContext::JobField {
            job_id: &job.id,
            field: "outputs",
        },
        "Failed to serialize job outputs; storing empty list",
    );

    let state = job_state_json(job);

    // Serialize execution_info to JSON
    let execution_info_json = job.execution_info.as_ref().and_then(|info| {
        json::to_string_option_or_warn(
            info,
            JsonContext::JobField {
                job_id: &job.id,
                field: "execution_info",
            },
            "Failed to serialize job execution_info; omitting",
        )
    });

    JobDbModel {
        id: job.id.clone(),
        job_type: job.job_type.clone(),
        status: job.status.as_str().to_string(),
        config: job.config.clone().unwrap_or_else(|| "{}".to_string()),
        state,
        created_at: job.created_at.timestamp_millis(),
        updated_at: crate::database::time::now_ms(),
        input: Some(inputs_json),
        outputs: Some(outputs_json),
        priority: job.priority,
        streamer_id: Some(job.streamer_id.clone()),
        session_id: Some(job.session_id.clone()),
        started_at: job.started_at.map(|dt| dt.timestamp_millis()),
        completed_at: job.completed_at.map(|dt| dt.timestamp_millis()),
        error: job.error.clone(),
        retry_count: job.retry_count,
        pipeline_id: job.pipeline_id.clone(),
        execution_info: execution_info_json,
        duration_secs: job.duration_secs,
        queue_wait_secs: job.queue_wait_secs,
        dag_step_execution_id: job.dag_step_execution_id.clone(),
        retry_after: job.retry_after.map(|at| at.timestamp_millis()),
    }
}

/// Convert a JobDbModel to Job.
fn db_model_to_job(db_job: &JobDbModel) -> Job {
    let status = db_job.get_status().unwrap_or(JobStatus::Pending);

    let created_at = crate::database::time::ms_to_datetime(db_job.created_at);

    let started_at = db_job.started_at.map(crate::database::time::ms_to_datetime);

    let completed_at = db_job
        .completed_at
        .map(crate::database::time::ms_to_datetime);

    // Parse inputs JSON array
    // If it fails (legacy data), treat as single path wrapped in vec
    let input_str = db_job.input.clone().unwrap_or_default();
    let inputs = if input_str.starts_with('[') {
        serde_json::from_str::<Vec<String>>(&input_str).unwrap_or_else(|_| {
            if input_str.is_empty() {
                vec![]
            } else {
                vec![input_str]
            }
        })
    } else if input_str.is_empty() {
        vec![]
    } else {
        vec![input_str]
    };

    // Parse outputs JSON array
    let output_str = db_job.outputs.clone().unwrap_or_default();
    let outputs = if output_str.starts_with('[') {
        serde_json::from_str::<Vec<String>>(&output_str).unwrap_or_else(|_| vec![])
    } else if output_str.is_empty() {
        vec![]
    } else {
        vec![output_str]
    };

    let JobStateMeta {
        streamer_name,
        session_title,
        platform,
        session_start,
        manifest,
        timeout_secs,
    } = parse_job_state(&db_job.state);

    Job {
        id: db_job.id.clone(),
        job_type: db_job.job_type.clone(),
        inputs,
        outputs,
        priority: db_job.priority,
        status,
        streamer_id: db_job.streamer_id.clone().unwrap_or_default(),
        session_id: db_job.session_id.clone().unwrap_or_default(),
        config: if db_job.config == "{}" {
            None
        } else {
            Some(db_job.config.clone())
        },
        created_at,
        started_at,
        completed_at,
        error: db_job.error.clone(),
        retry_count: db_job.retry_count,
        retry_after: db_job
            .retry_after
            .map(crate::database::time::ms_to_datetime),
        timeout_secs,
        pipeline_id: db_job.pipeline_id.clone(),
        // Parse execution_info JSON
        execution_info: db_job
            .execution_info
            .as_ref()
            .and_then(|s| serde_json::from_str::<JobExecutionInfo>(s).ok()),
        duration_secs: db_job.duration_secs,
        queue_wait_secs: db_job.queue_wait_secs,
        dag_step_execution_id: db_job.dag_step_execution_id.clone(),
        streamer_name,
        session_title,
        platform,
        session_start,
        manifest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::JobPhase;

    /// The manifest travels in the `state` column with the placeholder metadata
    /// and comes back through `db_model_to_job`; rows written before the column
    /// existed, or with a corrupt value, yield no manifest.
    #[test]
    fn job_state_round_trips_the_manifest() {
        use crate::pipeline::manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};
        let manifest = Arc::new(PipelineInputManifest::new(
            "session",
            "streamer",
            ManifestScope::Session,
            vec![ManifestSegment {
                segment_index: 0,
                video: vec!["/rec/0.mp4".to_string()],
                danmu: vec!["/rec/0.xml".to_string()],
            }],
        ));
        let mut job = Job::new("remux", vec!["/rec/0.mp4".to_string()], vec![], "s", "sess");
        job.manifest = Some(manifest.clone());

        let state = job_state_json(&job);
        let parsed = parse_job_state(&state);
        assert_eq!(parsed.manifest.as_deref(), Some(manifest.as_ref()));
        assert!(
            !parsed.has_any(),
            "a manifest alone is not placeholder metadata"
        );
        assert_eq!(
            db_model_to_job(&job_to_db_model(&job)).manifest,
            job.manifest
        );
        let named = job
            .clone()
            .with_streamer_name("StreamerName")
            .with_session_title("SessionTitle")
            .with_platform("Twitch");
        let restored = db_model_to_job(&job_to_db_model(&named));
        assert_eq!(restored.manifest, job.manifest);
        assert_eq!(restored.platform.as_deref(), Some("Twitch"));
        assert_eq!(restored.streamer_name.as_deref(), Some("StreamerName"));
        assert_eq!(restored.session_title.as_deref(), Some("SessionTitle"));

        assert!(parse_job_state("{}").manifest.is_none());
        assert!(
            parse_job_state(r#"{"streamer_name":"S","manifest":null}"#)
                .manifest
                .is_none()
        );
        assert!(
            parse_job_state(r#"{"manifest":{"version":"nope"}}"#)
                .manifest
                .is_none()
        );
        job.manifest = None;
        assert_eq!(job_state_json(&job), "{}");
    }

    /// Per-file results an upload processor reported for a partially failed
    /// batch are kept as reported; only inputs without a result are failed with
    /// the job's error.
    #[tokio::test]
    async fn failed_upload_job_keeps_reported_per_file_results() {
        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );
        let broadcaster =
            UploadStatusBroadcaster::new(std::sync::Arc::new(|_: &UploadStatusEvent| {
                bytes::Bytes::new()
            }));
        queue.set_upload_broadcaster(broadcaster.clone());
        let mut rx = broadcaster.subscribe();
        let job_id = queue
            .enqueue(Job::new(
                "rclone",
                vec![
                    "/rec/a.mp4".to_string(),
                    "/rec/b.mp4".to_string(),
                    "/rec/c.mp4".to_string(),
                ],
                vec![],
                "streamer",
                "session",
            ))
            .await
            .unwrap();
        queue.dequeue(None).await.unwrap().unwrap();

        queue
            .fail_with_step_info(
                &job_id,
                "1 of 3 inputs failed: /rec/c.mp4: boom",
                None,
                Some("rclone"),
                None,
                None,
                &[
                    UploadResultItem {
                        local_path: "/rec/a.mp4".to_string(),
                        remote_path: Some("remote:/a.mp4".to_string()),
                        size_bytes: Some(1),
                        status: UploadItemStatus::Completed,
                        error: None,
                    },
                    UploadResultItem {
                        local_path: "/rec/b.mp4".to_string(),
                        remote_path: Some("remote:/b.mp4".to_string()),
                        size_bytes: Some(1),
                        status: UploadItemStatus::Skipped,
                        error: Some("exists".to_string()),
                    },
                ],
            )
            .await
            .unwrap();

        let terminal = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let envelope = rx.recv().await.expect("broadcast channel stays open");
                if let UploadStatusEvent::Terminal {
                    files_succeeded,
                    files_failed,
                    files_skipped,
                    status,
                    ..
                } = envelope.event.as_ref()
                {
                    break (*files_succeeded, *files_failed, *files_skipped, *status);
                }
            }
        })
        .await
        .expect("a terminal upload event is emitted");
        assert_eq!(terminal, (1, 1, 1, UploadTerminalStatus::Failed));
    }

    #[tokio::test]
    async fn baidupcs_progress_reports_are_broadcast_as_upload_progress() {
        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );
        let broadcaster =
            UploadStatusBroadcaster::new(std::sync::Arc::new(|_: &UploadStatusEvent| {
                bytes::Bytes::new()
            }));
        queue.set_upload_broadcaster(broadcaster.clone());
        let mut rx = broadcaster.subscribe();

        // The aggregator publishes only for live jobs; a processing job owns
        // a cancellation token (created on dequeue), which is all the
        // liveness check reads.
        queue
            .cancellation_tokens
            .insert("job-1".to_string(), AttemptToken::default());

        let mut snapshot = JobProgressSnapshot::new(ProgressKind::BaiduPcs);
        snapshot.percent = Some(42.0);
        snapshot.bytes_done = Some(4200);
        queue.progress_reporter("job-1").report(snapshot);

        // The queued-uploads count is published independently of progress.
        let envelope = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let envelope = rx.recv().await.expect("broadcast channel stays open");
                if !matches!(envelope.event.as_ref(), UploadStatusEvent::Queue { .. }) {
                    break envelope;
                }
            }
        })
        .await
        .expect("aggregator publishes within one flush interval");
        match envelope.event.as_ref() {
            UploadStatusEvent::Progress {
                job_id, snapshot, ..
            } => {
                assert_eq!(job_id, "job-1");
                assert_eq!(snapshot.percent, Some(42.0));
                assert_eq!(snapshot.bytes_done, Some(4200));
            }
            other => panic!("expected Progress event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn terminal_cursor_cleanup_does_not_split_log_write_lock() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repo = Arc::new(crate::database::repositories::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let queue = JobQueue::with_repository(JobQueueConfig::default(), repo.clone());
        let job_id = queue
            .enqueue(Job::new("remux", vec![], vec![], "streamer", "session"))
            .await
            .unwrap();
        let old: Vec<_> = (0..MAX_PERSISTED_LOG_ROWS_PER_JOB - 1)
            .map(|i| JobLogEntry::info(format!("old-{i}")))
            .collect();
        queue.append_log_entry(&job_id, &old).await.unwrap();

        // Hold the same permit as an in-flight insert. Polling each writer once proves
        // both remain blocked, even though terminal cleanup evicts the count between them.
        let guard = queue.log_persistence_lock(&job_id).lock().await;
        let first_batch = [JobLogEntry::info("first late batch")];
        let second_batch = [JobLogEntry::info("second late batch")];
        let first = queue.append_log_entry(&job_id, &first_batch);
        tokio::pin!(first);
        assert!(futures::poll!(first.as_mut()).is_pending());
        queue.finalize_cancelled_job(&job_id);
        assert!(!queue.persisted_log_cursor.contains_key(&job_id));
        let second = queue.append_log_entry(&job_id, &second_batch);
        tokio::pin!(second);
        assert!(futures::poll!(second.as_mut()).is_pending());
        drop(guard);
        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(first, second)
        })
        .await
        .unwrap();
        first.unwrap();
        second.unwrap();
        let (logs, count) = repo
            .list_execution_logs(&job_id, &Pagination::new(5000, 0))
            .await
            .unwrap();
        assert_eq!(count, MAX_PERSISTED_LOG_ROWS_PER_JOB as u64);
        assert!(
            logs.iter()
                .any(|entry| entry.message.as_deref() == Some("first late batch"))
        );
        assert!(
            logs.iter()
                .any(|entry| entry.message.as_deref() == Some("second late batch"))
        );
        assert!(!queue.persisted_log_cursor.contains_key(&job_id));
        sqlx::query(
            "CREATE TRIGGER reject_late_log BEFORE INSERT ON job_execution_logs
             BEGIN SELECT RAISE(FAIL, 'injected late failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            queue
                .append_log_entry(&job_id, &[JobLogEntry::info("failed late batch")])
                .await
                .is_err()
        );
        assert!(!queue.persisted_log_cursor.contains_key(&job_id));
    }

    /// resolve_job_metadata must write back-filled values to the job row's
    /// state column so a later dequeue does not depend on the live_sessions
    /// row still existing.
    #[tokio::test]
    async fn test_resolve_job_metadata_persists_backfilled_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("job_queue_test.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
        let pool = crate::database::init_pool(&db_url).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        // Satisfy the live_sessions -> streamers -> platform_config FK chain.
        sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES ('p1', 'test')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state) \
             VALUES ('streamer-1', 'Streamer', 'https://example.com/s1', 'p1', 'NOT_LIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let session_start_ms: i64 = 1_704_152_700_000;
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time) \
             VALUES ('session-1', 'streamer-1', ?)",
        )
        .bind(session_start_ms)
        .execute(&pool)
        .await
        .unwrap();

        let job_repo: Arc<dyn JobRepository> = Arc::new(
            crate::database::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
        );
        let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo.clone());
        queue.set_session_repo(Arc::new(
            crate::database::repositories::session::SqlxSessionRepository::new(
                pool.clone(),
                pool.clone(),
            ),
        ));

        let mut job = Job::new(
            "rclone",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let manifest = Arc::new(crate::pipeline::manifest::PipelineInputManifest::new(
            "session-1",
            "streamer-1",
            crate::pipeline::manifest::ManifestScope::Session,
            Vec::new(),
        ));
        job.manifest = Some(manifest.clone());
        queue.enqueue(job.clone()).await.unwrap();
        assert!(
            parse_job_state(&job_repo.get_job(&job.id).await.unwrap().state)
                .session_start
                .is_none()
        );

        queue.resolve_job_metadata(&mut job).await;
        assert_eq!(
            job.session_start.map(|dt| dt.timestamp_millis()),
            Some(session_start_ms)
        );

        // The resolved value must now be in the DB row, not just in memory, and
        // rewriting the state must keep the manifest that was stored with the job.
        let db_job = job_repo.get_job(&job.id).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(&db_job.state).unwrap();
        assert_eq!(
            state.get("session_start_ms").and_then(|v| v.as_i64()),
            Some(session_start_ms)
        );
        assert_eq!(
            parse_job_state(&db_job.state).manifest.as_deref(),
            Some(manifest.as_ref())
        );
    }

    /// A failed parent finalization must not leave runnable split jobs:
    /// split_job_for_single_input rolls back already-created jobs through
    /// cancel_job when marking the original job completed fails.
    #[tokio::test]
    async fn split_rollback_cancels_created_jobs_when_parent_finalization_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("job_queue_split_rollback.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
        let pool = crate::database::init_pool(&db_url).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        // Satisfy the live_sessions -> streamers -> platform_config FK chain.
        sqlx::query("INSERT INTO platform_config (id, platform_name) VALUES ('p1', 'test')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state) \
             VALUES ('streamer-1', 'Streamer', 'https://example.com/s1', 'p1', 'NOT_LIVE')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time) \
             VALUES ('session-1', 'streamer-1', 1704152700000)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let job_repo: Arc<dyn JobRepository> = Arc::new(
            crate::database::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
        );
        let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo.clone());

        // The parent is never enqueued, so repo.get_job(&job.id) in the
        // finalization step fails after both split jobs were created.
        let job = Job::new(
            "remux",
            vec!["/input-a.flv".to_string(), "/input-b.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );

        let result = queue.split_job_for_single_input(&job).await;
        assert!(result.is_err());

        let pending = job_repo
            .list_jobs_by_status(JobStatus::Pending)
            .await
            .unwrap();
        assert!(pending.is_empty(), "split jobs must not stay runnable");
        let cancelled = job_repo
            .list_jobs_by_status(JobStatus::Cancelled)
            .await
            .unwrap();
        assert_eq!(cancelled.len(), 2);
    }

    #[test]
    fn test_upload_kind_for_job_type() {
        // Bare processor names and their "<type>_<preset>" DAG-step variants.
        assert_eq!(upload_kind_for_job_type("rclone"), Some("rclone"));
        assert_eq!(upload_kind_for_job_type("RCLONE"), Some("rclone"));
        assert_eq!(upload_kind_for_job_type("rclone_default"), Some("rclone"));
        assert_eq!(upload_kind_for_job_type("upload"), Some("rclone"));
        assert_eq!(upload_kind_for_job_type("upload_gdrive"), Some("rclone"));
        assert_eq!(upload_kind_for_job_type("baidupcs"), Some("baidupcs"));
        assert_eq!(upload_kind_for_job_type("BaiduPCS"), Some("baidupcs"));
        assert_eq!(
            upload_kind_for_job_type("baidupcs_default"),
            Some("baidupcs")
        );
        assert_eq!(upload_kind_for_job_type("remux"), None);
        assert_eq!(upload_kind_for_job_type("thumbnail"), None);
        // "uploader" would prefix-match "upload_" only with the underscore.
        assert_eq!(upload_kind_for_job_type("uploader"), None);
        assert_eq!(upload_kind_for_job_type("baidu"), None);
    }

    /// `complete()` must persist `JobResult.uploads` to upload_records for
    /// upload job types and leave the table untouched for everything else.
    #[tokio::test]
    async fn test_complete_persists_upload_records_for_upload_jobs() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        let job_repo: Arc<dyn JobRepository> = Arc::new(
            crate::database::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
        );
        let upload_repo = Arc::new(
            crate::database::repositories::upload_record::SqlxUploadRecordRepository::new(
                pool.clone(),
                pool.clone(),
            ),
        );
        let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo);
        queue.set_upload_repo(upload_repo.clone());

        let uploads = vec![UploadResultItem {
            local_path: "/videos/a.mp4".to_string(),
            remote_path: Some("remote:bucket/a.mp4".to_string()),
            size_bytes: Some(2048),
            status: UploadItemStatus::Completed,
            error: None,
        }];

        // Upload job type → rows persisted.
        let rclone_job = Job::new("rclone", vec!["/videos/a.mp4".to_string()], vec![], "", "");
        let rclone_id = queue.enqueue(rclone_job).await.unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        queue
            .complete(
                &rclone_id,
                JobResult {
                    outputs: vec![],
                    duration_secs: 1.0,
                    metadata: None,
                    uploads: uploads.clone(),
                    logs: vec![],
                },
            )
            .await
            .unwrap();
        let rows = upload_repo.list_by_job(&rclone_id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].uploader, "rclone");
        assert_eq!(rows[0].status, "COMPLETED");
        assert_eq!(rows[0].remote_path.as_deref(), Some("remote:bucket/a.mp4"));
        assert!(rows[0].completed_at.is_some());

        // Non-upload job type → uploads on the result are ignored.
        let remux_job = Job::new("remux", vec!["/videos/a.mp4".to_string()], vec![], "", "");
        let remux_id = queue.enqueue(remux_job).await.unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        queue
            .complete(
                &remux_id,
                JobResult {
                    outputs: vec![],
                    duration_secs: 1.0,
                    metadata: None,
                    uploads,
                    logs: vec![],
                },
            )
            .await
            .unwrap();
        assert!(upload_repo.list_by_job(&remux_id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_failed_upload_records_follow_winning_terminal_transition() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        let job_repo: Arc<dyn JobRepository> = Arc::new(
            crate::database::repositories::job::SqlxJobRepository::new(pool.clone(), pool.clone()),
        );
        let upload_repo = Arc::new(
            crate::database::repositories::upload_record::SqlxUploadRecordRepository::new(
                pool.clone(),
                pool.clone(),
            ),
        );
        let queue = JobQueue::with_repository(JobQueueConfig::default(), job_repo);
        queue.set_upload_repo(upload_repo.clone());

        let failed_job = Job::new(
            "rclone",
            vec!["/videos/failed.mp4".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let failed_id = queue.enqueue(failed_job).await.unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        queue.fail(&failed_id, "connection reset").await.unwrap();

        let rows = upload_repo.list_by_job(&failed_id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "FAILED");
        assert_eq!(rows[0].error.as_deref(), Some("connection reset"));

        let cancelled_job = Job::new(
            "rclone",
            vec!["/videos/cancelled.mp4".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let cancelled_id = queue.enqueue(cancelled_job).await.unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        queue.cancel_job(&cancelled_id).await.unwrap();
        queue
            .fail(&cancelled_id, "late processor error")
            .await
            .unwrap();

        assert!(
            upload_repo
                .list_by_job(&cancelled_id)
                .await
                .unwrap()
                .is_empty(),
            "a cancellation that wins the transition must not leave failed upload rows"
        );
    }

    /// A snapshot only means anything while its job runs: it is dropped when
    /// the job ends, and a report that reaches the aggregator after that does
    /// not bring it back.
    #[tokio::test]
    async fn terminal_transitions_drop_progress_and_late_reports_stay_dropped() {
        async fn wait_until(mut done: impl FnMut() -> bool) {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                while !done() {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("the aggregator handles the report");
        }

        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );

        let mut job_ids = Vec::new();
        for _ in 0..4 {
            let job = Job::new("remux", vec!["/in.mp4".to_string()], vec![], "", "");
            job_ids.push(queue.enqueue(job).await.unwrap());
        }
        for _ in &job_ids {
            queue.dequeue(None).await.unwrap().unwrap();
        }
        for job_id in &job_ids {
            queue
                .progress_reporter(job_id)
                .report(JobProgressSnapshot::new(ProgressKind::Ffmpeg));
        }
        wait_until(|| {
            job_ids
                .iter()
                .all(|job_id| queue.get_job_progress(job_id).is_some())
        })
        .await;

        let [completed, failed, cancelled, live] = job_ids.as_slice() else {
            unreachable!("four jobs were enqueued");
        };
        queue
            .complete(
                completed,
                JobResult {
                    outputs: vec![],
                    duration_secs: 1.0,
                    metadata: None,
                    uploads: vec![],
                    logs: vec![],
                },
            )
            .await
            .unwrap();
        queue.fail(failed, "boom").await.unwrap();
        // A cancelled job keeps its tracking until its worker ends the attempt.
        let cancelled_attempt = queue.attempt_token(cancelled).unwrap();
        queue.cancel_job(cancelled).await.unwrap();
        queue.finalize_attempt(cancelled, &cancelled_attempt).await;
        let ended = [completed, failed, cancelled];
        for job_id in ended {
            assert!(queue.get_job_progress(job_id).is_none(), "{job_id}");
        }

        for job_id in ended {
            queue
                .progress_reporter(job_id)
                .report(JobProgressSnapshot::new(ProgressKind::Ffmpeg));
        }
        // The channel is FIFO: once this later report is cached, the late
        // ones before it have been handled.
        let mut probe = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        probe.percent = Some(50.0);
        queue.progress_reporter(live).report(probe);
        wait_until(|| {
            queue
                .get_job_progress(live)
                .is_some_and(|snapshot| snapshot.percent == Some(50.0))
        })
        .await;
        for job_id in ended {
            assert!(queue.get_job_progress(job_id).is_none(), "{job_id}");
        }
    }

    #[tokio::test]
    async fn late_attempt_finalization_waits_for_successor_claim_and_keeps_its_tracking() {
        use std::time::Duration;

        use crate::database::repositories::{SqlxJobRepository, SqlxUploadRecordRepository};

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let queue = JobQueue::with_repository(
            JobQueueConfig::default(),
            Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
        );
        queue.stop_progress_aggregator().await;
        let uploads = Arc::new(SqlxUploadRecordRepository::new(pool.clone(), pool.clone()));
        queue.set_upload_repo(uploads.clone());
        let broadcaster =
            UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
        queue.set_upload_broadcaster(broadcaster.clone());
        let job_id = queue
            .enqueue(Job::new(
                "rclone",
                vec!["/input.mp4".to_string()],
                vec![],
                "",
                "",
            ))
            .await
            .unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        let stale = queue.attempt_token(&job_id).unwrap();
        queue.cancel_job(&job_id).await.unwrap();
        queue.retry_job(&job_id).await.unwrap();

        // Pause a real claim before its cache/token publication. A late finalizer
        // must wait for that whole handoff rather than observe partially installed tracking.
        let connection = pool.acquire().await.unwrap();
        let mut dequeue = std::pin::pin!(queue.dequeue(None));
        assert!(futures::poll!(dequeue.as_mut()).is_pending());
        let mut finalization = std::pin::pin!(queue.finalize_attempt(&job_id, &stale));
        assert!(futures::poll!(finalization.as_mut()).is_pending());
        drop(connection);

        let successor = tokio::time::timeout(Duration::from_secs(3), dequeue)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(successor.id, job_id);
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
        snapshot.percent = Some(42.0);
        queue.progress_cache.insert(job_id.clone(), snapshot);
        tokio::time::timeout(Duration::from_secs(3), finalization)
            .await
            .unwrap();
        assert!(!queue.attempt_token(&job_id).unwrap().is_cancelled());
        assert_eq!(queue.get_job_progress(&job_id).unwrap().percent, Some(42.0));

        let mut events = broadcaster.subscribe();
        queue.fail(&job_id, "successor failed").await.unwrap();
        let rows = uploads.list_by_job(&job_id).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].error.as_deref(), Some("successor failed"));
        assert!(matches!(
            events.try_recv().unwrap().event.as_ref(),
            UploadStatusEvent::Terminal {
                status: UploadTerminalStatus::Failed,
                files_failed: 1,
                ..
            }
        ));
    }

    /// A failure whose retry is already due keeps the retry waiting until the
    /// failed attempt's cleanup is done, so that cleanup cannot remove the
    /// tracking of the attempt the retry starts.
    #[tokio::test]
    async fn due_retry_waits_for_the_failed_attempt_cleanup() {
        use std::time::Duration;

        use crate::database::repositories::SqlxJobRepository;

        /// Pauses the per-file upload write, which `fail_internal` makes after
        /// the failure is durable and before it cleans up the attempt.
        #[derive(Default)]
        struct PausedUploads {
            entered: Notify,
            release: Notify,
        }

        #[async_trait::async_trait]
        impl UploadRecordRepository for PausedUploads {
            async fn upsert_records(&self, _records: &[UploadRecordDbModel]) -> Result<()> {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(())
            }

            async fn list_by_job(&self, _job_id: &str) -> Result<Vec<UploadRecordDbModel>> {
                Ok(Vec::new())
            }

            async fn list_by_session_local_paths(
                &self,
                _output_keys: &[(String, String)],
            ) -> Result<Vec<UploadRecordDbModel>> {
                Ok(Vec::new())
            }
        }

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let queue = JobQueue::with_repository(
            JobQueueConfig::default(),
            Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone())),
        );
        queue.stop_progress_aggregator().await;
        let uploads = Arc::new(PausedUploads::default());
        queue.set_upload_repo(uploads.clone());
        let job_id = queue
            .enqueue(Job::new(
                "rclone",
                vec!["/input.mp4".to_string()],
                vec![],
                "",
                "",
            ))
            .await
            .unwrap();
        queue.dequeue(None).await.unwrap().unwrap();
        let failed = queue.attempt_token(&job_id).unwrap();

        let mut failure = std::pin::pin!(queue.fail_with_step_info(
            &job_id,
            "boom",
            Some(Utc::now()),
            Some("rclone"),
            None,
            None,
            &[],
        ));
        tokio::select! {
            outcome = &mut failure => panic!("the upload write did not pause: {outcome:?}"),
            () = uploads.entered.notified() => {}
        }

        let mut successor = std::pin::pin!(async {
            queue.retry_job(&job_id).await.unwrap();
            queue.dequeue(None).await.unwrap().unwrap();
            queue.attempt_token(&job_id).unwrap()
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut successor)
                .await
                .is_err(),
            "the retry started while the failed attempt was still cleaning up"
        );

        uploads.release.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), failure)
                .await
                .unwrap()
                .unwrap(),
            JobFailureOutcome::Transitioned
        );
        let successor = tokio::time::timeout(Duration::from_secs(3), successor)
            .await
            .unwrap();
        assert_ne!(successor.id(), failed.id());
        assert_eq!(
            queue.attempt_token(&job_id).map(|token| token.id()),
            Some(successor.id())
        );
    }

    // ========================================================================
    // Fan-out and Multi-input Support Tests
    // ========================================================================

    /// Test that split jobs preserve pipeline context.
    #[tokio::test]
    async fn test_split_job_preserves_pipeline_context() {
        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );

        let job = Job::new(
            "remux",
            vec![
                "/input1.flv".to_string(),
                "/input2.flv".to_string(),
                "/input3.flv".to_string(),
            ],
            vec![],
            "streamer-1",
            "session-1",
        )
        .with_pipeline_id("pipeline-1".to_string())
        .with_streamer_name("StreamerName".to_string())
        .with_session_title("SessionTitle".to_string())
        .with_platform("Twitch".to_string());

        let original_id = job.id.clone();
        queue.enqueue(job.clone()).await.unwrap();

        let split_ids = queue.split_job_for_single_input(&job).await.unwrap();

        // Should create 3 new jobs
        assert_eq!(split_ids.len(), 3);

        // Original job should be marked as completed
        let original = queue.get_job(&original_id).await.unwrap().unwrap();
        assert_eq!(original.status, JobStatus::Completed);

        // Verify each split job has single input and preserves pipeline context
        for split_id in split_ids.iter() {
            let split_job = queue.get_job(split_id).await.unwrap().unwrap();
            assert_eq!(split_job.inputs.len(), 1);
            assert_eq!(split_job.job_type, "remux");
            assert_eq!(split_job.pipeline_id, Some("pipeline-1".to_string()));
            assert_eq!(split_job.streamer_name.as_deref(), Some("StreamerName"));
            assert_eq!(split_job.session_title.as_deref(), Some("SessionTitle"));
            assert_eq!(split_job.platform.as_deref(), Some("Twitch"));
        }
    }

    // ========================================================================
    // Pipeline Recovery Tests
    // ========================================================================

    #[tokio::test]
    async fn recovery_lists_files_staged_by_unfinished_attempts_only() {
        use crate::database::repositories::{SqlxStagedOutputRepository, StagedOutputRepository};

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repo = Arc::new(crate::database::repositories::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let staged = Arc::new(SqlxStagedOutputRepository::new(pool.clone(), pool));
        let queue = JobQueue::with_repository(JobQueueConfig::default(), repo);
        queue.set_staged_output_repo(staged.clone());

        // One attempt finishes normally; the other never does, as in a crash.
        let finished = queue.staged_output_recorder("job-done");
        finished
            .record(
                std::path::Path::new("/out/a.tmp-1.mp4"),
                std::path::Path::new("/out/a.mp4"),
            )
            .await;
        finished.finish().await;
        queue
            .staged_output_recorder("job-crashed")
            .record(
                std::path::Path::new("/out/b.tmp-2.mp4"),
                std::path::Path::new("/out/b.mp4"),
            )
            .await;

        queue.recover_jobs().await.unwrap();

        let listed = staged.list_interrupted(None, 10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].job_id, "job-crashed");
        assert_eq!(listed[0].path, "/out/b.tmp-2.mp4");
    }

    /// Silent uploads and long jobs are kept alive by repeating what the
    /// client already knows: the latest progress, or the start event before
    /// any progress exists. Other jobs, and cancelling ones, stay quiet.
    #[tokio::test]
    async fn heartbeat_reannounces_live_uploads_and_long_jobs() {
        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );
        let mut ids = Vec::new();
        for job_type in ["rclone", "baidupcs_main", "rclone", "remux", "remux"] {
            let job = Job::new(job_type, vec!["/in.flv".to_string()], vec![], "s", "");
            ids.push(queue.enqueue(job).await.unwrap());
            queue.dequeue(None).await.unwrap().unwrap();
        }
        let [with_progress, silent, cancelling, short, long] = ids.as_slice() else {
            unreachable!()
        };
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
        snapshot.percent = Some(42.0);
        queue
            .progress_cache
            .insert(with_progress.clone(), snapshot.clone());
        for id in [short, long] {
            queue
                .progress_cache
                .insert(id.clone(), JobProgressSnapshot::new(ProgressKind::Ffmpeg));
        }
        queue.cancellation_tokens.get(cancelling).unwrap().cancel();
        let now = Utc::now();
        queue.jobs_cache.get_mut(long).unwrap().started_at = Some(now - LONG_JOB_AFTER);

        let events = heartbeat_events(
            &queue.jobs_cache,
            &queue.cancellation_tokens,
            &queue.progress_cache,
            now,
            LONG_JOB_AFTER,
        );

        assert_eq!(events.len(), 3, "{events:?}");
        for event in &events {
            match event {
                UploadStatusEvent::Progress {
                    job_id, snapshot, ..
                } => {
                    assert_eq!(job_id, with_progress);
                    assert_eq!(snapshot.percent, Some(42.0));
                }
                UploadStatusEvent::Started {
                    job_id, uploader, ..
                } => {
                    assert_eq!(job_id, silent);
                    assert_eq!(*uploader, "baidupcs");
                }
                UploadStatusEvent::JobProgress { job_id, .. } => assert_eq!(job_id, long),
                other => panic!("unexpected heartbeat event {other:?}"),
            }
        }
    }

    /// Jobs other than uploads reach the socket only after
    /// `JobQueueConfig::long_job_after` of processing: announced with the
    /// progress they already have, then their progress, then their end, and
    /// listed in the connect snapshot. Shorter jobs stay off it entirely.
    #[tokio::test]
    async fn long_jobs_reach_the_socket_after_the_long_job_delay() {
        // Far longer than the few database calls a short job makes here.
        const LONG_JOB_AFTER: std::time::Duration = std::time::Duration::from_millis(500);
        async fn next_event(
            rx: &mut tokio::sync::broadcast::Receiver<crate::pipeline::UploadBroadcastEnvelope>,
        ) -> Arc<UploadStatusEvent> {
            // Well under HEARTBEAT_INTERVAL, so a heartbeat cannot stand in
            // for an event that has to come sooner.
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    let envelope = rx.recv().await.expect("broadcast channel stays open");
                    // The queued-uploads count is published independently.
                    if !matches!(*envelope.event, UploadStatusEvent::Queue { .. }) {
                        return envelope.event;
                    }
                }
            })
            .await
            .expect("an event is published")
        }

        let queue = JobQueue::with_repository(
            JobQueueConfig {
                long_job_after: LONG_JOB_AFTER,
                ..Default::default()
            },
            crate::database::test_support::job_repository().await,
        );
        let broadcaster =
            UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
        queue.set_upload_broadcaster(broadcaster.clone());
        let mut rx = broadcaster.subscribe();
        let dequeued_at = tokio::time::Instant::now();
        let mut ids = Vec::new();
        for job_type in ["thumbnail", "remux"] {
            let job = Job::new(job_type, vec![], vec![], "s", "");
            ids.push(queue.enqueue(job).await.unwrap());
            queue.dequeue(None).await.unwrap().unwrap();
        }
        let [short, long] = ids.as_slice() else {
            unreachable!()
        };
        assert!(queue.list_active_jobs().await.unwrap().is_empty());
        // Cached before the announcement, with nothing reported after it, as
        // while FFmpeg finalizes an MP4.
        let mut finalizing = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        finalizing.phase = Some(JobPhase::Finalizing);
        queue.progress_cache.insert(long.clone(), finalizing);

        queue.fail(short, "boom").await.unwrap();
        let started_at_ms = match &*next_event(&mut rx).await {
            UploadStatusEvent::JobStarted {
                job_id,
                job_type,
                started_at_ms,
                ..
            } => {
                assert_eq!((job_id, job_type.as_str()), (long, "remux"));
                *started_at_ms
            }
            other => panic!("expected JobStarted, got {other:?}"),
        };
        assert!(dequeued_at.elapsed() >= LONG_JOB_AFTER);
        match &*next_event(&mut rx).await {
            UploadStatusEvent::JobProgress {
                job_id, snapshot, ..
            } => assert_eq!((job_id, snapshot.phase), (long, Some(JobPhase::Finalizing))),
            other => panic!("expected the cached JobProgress, got {other:?}"),
        }
        // With the announcement, not from the heartbeat.
        assert!(dequeued_at.elapsed() < HEARTBEAT_INTERVAL);
        let active = queue.list_active_jobs().await.unwrap();
        assert_eq!(
            active.iter().map(|job| &job.job_id).collect::<Vec<_>>(),
            [long]
        );

        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        snapshot.percent = Some(42.0);
        queue.progress_reporter(long).report(snapshot);
        match &*next_event(&mut rx).await {
            UploadStatusEvent::JobProgress {
                job_id, snapshot, ..
            } => assert_eq!((job_id, snapshot.percent), (long, Some(42.0))),
            other => panic!("expected JobProgress, got {other:?}"),
        }

        queue.fail(long, "boom").await.unwrap();
        match &*next_event(&mut rx).await {
            UploadStatusEvent::JobEnded {
                job_id,
                started_at_ms: ended_started_at_ms,
            } => assert_eq!((job_id, *ended_started_at_ms), (long, started_at_ms)),
            other => panic!("expected JobEnded, got {other:?}"),
        }
    }

    /// The pending count applies the upload job-type rule (including preset
    /// prefixes) to rows SQL grouped by exact type.
    #[tokio::test]
    async fn pending_upload_count_matches_upload_job_types() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repo = Arc::new(crate::database::repositories::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let queue = JobQueue::with_repository(JobQueueConfig::default(), repo);
        // Highest priority, so it is the one dequeued below.
        queue
            .enqueue(Job::new("rclone", vec![], vec![], "s", "").with_priority(10))
            .await
            .unwrap();
        for job_type in [
            "rclone",
            "rclone_offsite",
            "upload",
            "baidupcs_main",
            "remux",
            "thumbnail",
        ] {
            queue
                .enqueue(Job::new(job_type, vec![], vec![], "s", ""))
                .await
                .unwrap();
        }
        let dequeued = queue.dequeue(None).await.unwrap().unwrap();
        assert_eq!(dequeued.priority, 10);

        assert_eq!(queue.count_pending_uploads().await.unwrap(), 4);
    }

    /// Live progress is published while the queued-uploads count waits for a
    /// database connection, and the count follows once one is free.
    #[tokio::test]
    async fn progress_flows_while_the_queued_upload_count_waits_for_the_database() {
        use std::time::Duration;

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        // Taken before the aggregator first runs: its first queue poll is
        // immediate and has to wait for this connection.
        let connection = pool.acquire().await.unwrap();
        let queue = JobQueue::with_repository(
            JobQueueConfig::default(),
            Arc::new(crate::database::repositories::SqlxJobRepository::new(
                pool.clone(),
                pool.clone(),
            )),
        );
        let broadcaster =
            UploadStatusBroadcaster::new(Arc::new(|_: &UploadStatusEvent| bytes::Bytes::new()));
        queue.set_upload_broadcaster(broadcaster.clone());
        let mut rx = broadcaster.subscribe();
        queue
            .cancellation_tokens
            .insert("job-1".to_string(), AttemptToken::default());
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Rclone);
        snapshot.percent = Some(42.0);
        queue.progress_reporter("job-1").report(snapshot);

        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("progress is published while the count waits")
            .expect("broadcast channel stays open");
        match envelope.event.as_ref() {
            UploadStatusEvent::Progress {
                job_id, snapshot, ..
            } => assert_eq!((job_id.as_str(), snapshot.percent), ("job-1", Some(42.0))),
            other => panic!("expected Progress, got {other:?}"),
        }

        drop(connection);
        let envelope = tokio::time::timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("the count is published once the database is free")
            .expect("broadcast channel stays open");
        assert!(matches!(
            envelope.event.as_ref(),
            UploadStatusEvent::Queue { pending: 0 }
        ));
    }

    /// DAG list and detail payloads show each running step's progress; the
    /// raw ffmpeg/rclone JSON is too large to repeat for every step.
    #[tokio::test]
    async fn cached_progress_copies_the_snapshot_without_raw() {
        let queue = JobQueue::with_repository(
            Default::default(),
            crate::database::test_support::job_repository().await,
        );
        let mut snapshot = JobProgressSnapshot::new(ProgressKind::Ffmpeg);
        snapshot.percent = Some(42.5);
        snapshot.current_file = Some("seg.flv".to_string());
        snapshot.raw = serde_json::json!({"path": "/abs/file"});
        queue.progress_cache.insert("job-1".to_string(), snapshot);

        let cached = queue.cached_progress("job-1").unwrap();
        assert_eq!(
            (cached.percent, cached.current_file.as_deref()),
            (Some(42.5), Some("seg.flv"))
        );
        assert_eq!(cached.raw, serde_json::Value::Null);
        assert!(queue.cached_progress("unknown").is_none());
    }
}
