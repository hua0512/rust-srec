//! Database-backed job queue implementation.

use dashmap::DashMap;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::ports::PipelineSessionPort;
use super::processors::{ConfirmedUpload, UploadResultItem};
use super::progress::{JobProgressSnapshot, JobProgressUpdate};
use super::upload_events::UploadStatusBroadcaster;
use srec_sqlite::models::JobStatus;

use crate::manifest::PipelineInputManifest;
use crate::repositories::{JobRepository, StagedOutputRepository, UploadRecordRepository};

mod lifecycle;
mod logs_progress;
mod metadata;
mod recovery;
#[cfg(test)]
mod tests;

use logs_progress::spawn_progress_aggregator;
#[cfg(test)]
use logs_progress::*;
pub use metadata::db_model_to_job;
pub(super) use metadata::job_to_db_model;
pub(crate) use metadata::{JobStateMeta, job_state_json, parse_job_state};

pub(super) fn is_thumbnail_job_type(job_type: &str) -> bool {
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
pub(super) fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) const EXECUTION_INFO_MAX_LOGS: usize = 200;
pub(super) const PROGRESS_FLUSH_INTERVAL_MS: u64 = 250;
/// Must stay well under the frontend's upload staleness window
/// (`STALE_AFTER_MS`, 2 minutes); see [`logs_progress::heartbeat_events`].
pub(super) const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Default for [`JobQueueConfig::long_job_after`].
const LONG_JOB_AFTER: std::time::Duration = std::time::Duration::from_secs(10);
/// How often the queued-uploads count is re-checked while a client is
/// subscribed. Changes are only broadcast when the count moves.
pub(super) const UPLOAD_QUEUE_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(5);
pub(super) const LOG_PERSISTENCE_LOCK_SHARDS: usize = 64;
/// Ring-buffer bound on `job_execution_logs` rows per job: once a job
/// has this many rows, `persist_logs_to_db` trims the oldest via
/// `JobRepository::trim_execution_logs` after each insert. Counted in
/// `PersistedLogCursor::persisted_rows`, initialized from durable rows after
/// a terminal/retry path clears it. Time-based retention remains independent.
pub(super) const MAX_PERSISTED_LOG_ROWS_PER_JOB: usize = 5000;

#[derive(Debug, Default)]
pub(super) struct PersistedLogCursor {
    /// Initialized from durable rows on first use, including previous attempts.
    pub(super) persisted_rows: Option<usize>,
}

pub(super) struct LogCursorCleanup<'a> {
    pub(super) queue: &'a JobQueue,
    pub(super) job_id: &'a str,
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
pub struct AttemptToken {
    pub(super) token: CancellationToken,
    pub(super) attempt: u64,
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

pub(super) struct LongJobTrack {
    pub(super) attempt: u64,
    pub(super) started_at_ms: i64,
    pub(super) announced: bool,
}

#[derive(Clone, Copy)]
pub(super) enum LogPersistence {
    /// The caller owns a disjoint batch and removes it only after a successful write.
    Append,
    /// Entries may include clones already written by a streaming collector.
    Snapshot,
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

pub use srec_core::domain::processor::{JobLogEntry, LogLevel};

/// Extended job information for observability.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct JobExecutionInfo {
    /// Current processor name.
    pub current_processor: Option<String>,
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
    /// Per-file uploads confirmed against an active destination account by an
    /// earlier attempt of this job, used by retryable uploaders that opt in via
    /// `Processor::carries_forward_completed_uploads`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub confirmed_uploads: HashMap<String, ConfirmedUpload>,
    /// Preserve extension fields when artifact and terminal updates rewrite this document.
    #[serde(flatten)]
    pub extra_metadata: serde_json::Map<String, serde_json::Value>,
}

impl JobExecutionInfo {
    /// Create a new empty execution info.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a log entry.
    pub fn add_log(&mut self, entry: JobLogEntry) {
        self.logs.push_back(entry);
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
    pub session_start: Option<Timestamp>,
    /// Additional configuration as JSON.
    pub config: Option<String>,
    /// When the job was created.
    pub created_at: Timestamp,
    /// When the job started processing.
    pub started_at: Option<Timestamp>,
    /// When the job completed.
    pub completed_at: Option<Timestamp>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Number of retry attempts.
    pub retry_count: i32,
    /// When a scheduled automatic retry of this failed job may run.
    pub retry_after: Option<Timestamp>,
    /// Timeout for one attempt, in seconds, set by the workflow step; the
    /// worker pool's default applies when absent.
    pub timeout_secs: Option<u64>,
    /// Whether this job's workflow step has an automatic retry policy
    /// (`max_attempts > 1`). `Some(true|false)` for workflow step jobs created
    /// with this field, `None` for standalone jobs or pre-upgrade workflow rows
    /// (which derive it from the stored workflow definition at execution time).
    pub has_step_retry: Option<bool>,
    /// Epoch milliseconds when this attempt entered `PENDING` after a retry.
    /// Used so `queue_wait_secs` measures the current attempt's queue wait
    /// rather than accumulating earlier attempts and backoff intervals.
    pub enqueued_at_ms: Option<i64>,
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
            created_at: Timestamp::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            retry_after: None,
            timeout_secs: None,
            has_step_retry: None,
            enqueued_at_ms: None,
            pipeline_id: None,
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
    /// Input size from `ProcessorOutput`; recorded in the job's execution info when set.
    pub input_size_bytes: Option<u64>,
    /// Output size from `ProcessorOutput`; recorded in the job's execution info when set.
    pub output_size_bytes: Option<u64>,
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
    pub started_at: Option<Timestamp>,
    /// Latest in-memory progress snapshot, when the processor has reported one.
    pub progress: Option<JobProgressSnapshot>,
}

pub(super) struct FailedUploadContext {
    pub(super) job_type: String,
    pub(super) streamer_id: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) inputs: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureMetadata {
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
    pub(super) attempt_gate: tokio::sync::RwLock<()>,
    /// Configuration.
    pub(super) config: JobQueueConfig,
    /// Current queue depth (approximate).
    pub(super) depth: AtomicUsize,
    /// Notify when new jobs are added.
    pub(super) notify: Arc<Notify>,
    /// Job repository for database persistence.
    pub(super) job_repository: Arc<dyn JobRepository>,
    /// Narrow port for session and streamer metadata/persistence.
    pub(super) session_port: std::sync::OnceLock<Arc<dyn PipelineSessionPort>>,
    /// Upload record repository for persisting per-file upload results.
    pub(super) upload_repo: std::sync::OnceLock<Arc<dyn UploadRecordRepository>>,
    /// Temporary files staged by attempts, for listing what a crash left behind.
    pub(super) staged_output_repo: std::sync::OnceLock<Arc<dyn StagedOutputRepository>>,
    /// Fan-out for live upload status events. Shared (`Arc`) with the
    /// progress aggregator task, which publishes `Progress` events on its
    /// flush tick; installed once via `set_upload_broadcaster`.
    pub(super) upload_broadcaster: Arc<std::sync::OnceLock<UploadStatusBroadcaster>>,
    /// In-memory cache of jobs (for quick lookups).
    ///
    /// `Arc`-shared with the progress aggregator task: `DashMap::clone`
    /// deep-copies the map, so handing the aggregator a bare clone would
    /// freeze it at the (empty) construction-time contents.
    pub(super) jobs_cache: Arc<DashMap<String, Job>>,
    /// Cancellation tokens for processing jobs. `Arc`-shared with the
    /// progress aggregator, whose liveness check reads it (see `jobs_cache`).
    pub(super) cancellation_tokens: Arc<DashMap<String, AttemptToken>>,
    /// Non-upload job attempts tracked for WebSocket `JobStarted` / `JobEnded`
    /// lifecycle consistency across `long_job_after` announcement and completion.
    pub(super) long_job_tracking: Arc<DashMap<String, LongJobTrack>>,
    /// Latest progress snapshot per job (in-memory). `Arc`-shared with the
    /// progress aggregator, which writes it (see `jobs_cache`).
    pub(super) progress_cache: Arc<DashMap<String, JobProgressSnapshot>>,
    /// Progress updates for the aggregator, which caches and coalesces them.
    pub(super) progress_tx: tokio::sync::mpsc::Sender<JobProgressUpdate>,
    /// Stops the progress consumer after worker producers have settled.
    pub(super) progress_shutdown: CancellationToken,
    /// Owned so pipeline shutdown can join the aggregator, which reads the job
    /// repository for the queued-upload count, before the database pools close.
    pub(super) progress_aggregator: parking_lot::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Durable row counts used to cap logs in `job_execution_logs`.
    pub(super) persisted_log_cursor: DashMap<String, PersistedLogCursor>,
    /// Stable locks outlive cursor eviction during cancellation and terminal cleanup.
    pub(super) log_persistence_locks: [tokio::sync::Mutex<()>; LOG_PERSISTENCE_LOCK_SHARDS],
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
        let long_job_tracking: Arc<DashMap<String, LongJobTrack>> = Arc::new(DashMap::new());
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
            long_job_tracking.clone(),
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
            session_port: std::sync::OnceLock::new(),
            upload_repo: std::sync::OnceLock::new(),
            staged_output_repo: std::sync::OnceLock::new(),
            upload_broadcaster,
            jobs_cache,
            cancellation_tokens,
            long_job_tracking,
            progress_cache,
            progress_tx,
            progress_shutdown,
            progress_aggregator: parking_lot::Mutex::new(progress_aggregator),
            persisted_log_cursor: DashMap::new(),
            log_persistence_locks: std::array::from_fn(|_| tokio::sync::Mutex::new(())),
        }
    }

    /// Set the narrow session/streamer metadata port.
    /// This can only be called once.
    pub(crate) fn set_session_port(&self, port: Arc<dyn PipelineSessionPort>) {
        if self.session_port.set(port).is_err() {
            tracing::warn!("Job queue session port was already installed");
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

    /// Per-file `local_path -> remote_path` outcomes recorded as `COMPLETED`
    /// by earlier attempts of `job_id`, when upload-record persistence is wired.
    /// Install the live upload-status broadcaster. This can only be called once.
    pub(crate) fn set_upload_broadcaster(&self, broadcaster: UploadStatusBroadcaster) {
        if self.upload_broadcaster.set(broadcaster).is_err() {
            tracing::warn!("Job queue upload broadcaster was already installed");
        }
    }

    /// Get the current queue depth.
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
    }

    pub(super) fn decrement_depth(&self, by: usize) {
        if by == 0 {
            return;
        }
        let _ = self
            .depth
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_sub(by))
            });
    }

    /// The depth thresholds and other settings this queue was built with.
    pub fn config(&self) -> &JobQueueConfig {
        &self.config
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

    /// Get a notifier for new jobs.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Wake workers after a job becomes runnable.
    ///
    /// `notify_waiters` wakes every currently parked worker, so every pool
    /// (CPU, IO and upload, which share this notifier
    /// but accept different job types) re-checks the queue. It stores no permit, so `notify_one` also
    /// runs for a worker that is mid-iteration and re-enters
    /// `Notify::notified` only after this call.
    pub(super) fn wake_workers(&self) {
        self.notify.notify_waiters();
        self.notify.notify_one();
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
