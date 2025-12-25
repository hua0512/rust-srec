//! Database-backed job queue implementation.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::progress::{JobProgressSnapshot, JobProgressUpdate, ProgressReporter};
use crate::database::models::JobExecutionProgressDbModel;
use crate::database::models::job::LogEntry as DbLogEntry;
use crate::database::models::{
    JobDbModel, JobExecutionLogDbModel, JobFilters, JobStatus as DbJobStatus, MediaFileType,
    MediaOutputDbModel, Pagination, TitleEntry,
};
use crate::database::repositories::{JobRepository, SessionRepository, StreamerRepository};
use crate::utils::json::{self, JsonContext};
use crate::{Error, Result};

const EXECUTION_INFO_MAX_LOGS: usize = 200;
const PROGRESS_FLUSH_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, Copy)]
struct PersistedLogCursor {
    last_sig: u64,
    last_ts_ms: i64,
}

fn log_level_to_db(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn log_signature(entry: &JobLogEntry) -> u64 {
    let mut hasher = DefaultHasher::new();
    entry.timestamp.timestamp_millis().hash(&mut hasher);
    log_level_to_db(entry.level).hash(&mut hasher);
    entry.message.hash(&mut hasher);
    hasher.finish()
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
}

impl Default for JobQueueConfig {
    fn default() -> Self {
        Self {
            warning_threshold: 100,
            critical_threshold: 500,
            poll_interval_ms: 100,
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

/// Job status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    /// Job is waiting to be processed.
    Pending,
    /// Job is currently being processed.
    Processing,
    /// Job completed successfully.
    Completed,
    /// Job failed.
    Failed,
    /// Job was interrupted.
    Interrupted,
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

/// A single log entry for job execution.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobLogEntry {
    /// Timestamp of the log entry.
    pub timestamp: DateTime<Utc>,
    /// Log level.
    pub level: LogLevel,
    /// Log message.
    pub message: String,
}

impl JobLogEntry {
    /// Create a new log entry with the current timestamp.
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            message: message.into(),
        }
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
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
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
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            pipeline_id,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: None,
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
    /// Execution logs.
    pub logs: Vec<JobLogEntry>,
}

/// The job queue service.
pub struct JobQueue {
    /// Configuration.
    config: JobQueueConfig,
    /// Current queue depth (approximate).
    depth: AtomicUsize,
    /// Notify when new jobs are added.
    notify: Arc<Notify>,
    /// Job repository for database persistence.
    job_repository: Option<Arc<dyn JobRepository>>,
    /// Session repository for persisting media outputs (e.g., thumbnails).
    session_repo: std::sync::OnceLock<Arc<dyn SessionRepository>>,
    /// Streamer repository for looking up streamer metadata (e.g., name).
    streamer_repo: std::sync::OnceLock<Arc<dyn StreamerRepository>>,
    /// In-memory cache of jobs (for quick lookups).
    jobs_cache: DashMap<String, Job>,
    /// Cancellation tokens for processing jobs.
    cancellation_tokens: DashMap<String, CancellationToken>,
    /// Latest progress snapshot per job (in-memory).
    progress_cache: DashMap<String, JobProgressSnapshot>,
    /// Progress update sender for async persistence/coalescing.
    progress_tx: tokio::sync::mpsc::Sender<JobProgressUpdate>,
    /// Cursor used to dedupe/append logs into `job_execution_logs`.
    persisted_log_cursor: DashMap<String, PersistedLogCursor>,
}

impl JobQueue {
    /// Create a new job queue.
    pub fn new() -> Self {
        Self::with_config(JobQueueConfig::default())
    }

    /// Create a new job queue with custom configuration.
    pub fn with_config(config: JobQueueConfig) -> Self {
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<JobProgressUpdate>(1024);
        let progress_cache: DashMap<String, JobProgressSnapshot> = DashMap::new();
        spawn_progress_aggregator(None, progress_rx, progress_cache.clone());

        Self {
            config,
            depth: AtomicUsize::new(0),
            notify: Arc::new(Notify::new()),
            job_repository: None,
            session_repo: std::sync::OnceLock::new(),
            streamer_repo: std::sync::OnceLock::new(),
            jobs_cache: DashMap::new(),
            cancellation_tokens: DashMap::new(),
            progress_cache,
            progress_tx,
            persisted_log_cursor: DashMap::new(),
        }
    }

    /// Create a new job queue with a job repository for database persistence.
    pub fn with_repository(config: JobQueueConfig, repository: Arc<dyn JobRepository>) -> Self {
        let (progress_tx, progress_rx) = tokio::sync::mpsc::channel::<JobProgressUpdate>(1024);
        let progress_cache: DashMap<String, JobProgressSnapshot> = DashMap::new();
        spawn_progress_aggregator(
            Some(repository.clone()),
            progress_rx,
            progress_cache.clone(),
        );

        Self {
            config,
            depth: AtomicUsize::new(0),
            notify: Arc::new(Notify::new()),
            job_repository: Some(repository),
            session_repo: std::sync::OnceLock::new(),
            streamer_repo: std::sync::OnceLock::new(),
            jobs_cache: DashMap::new(),
            cancellation_tokens: DashMap::new(),
            progress_cache,
            progress_tx,
            persisted_log_cursor: DashMap::new(),
        }
    }

    /// Set the job repository for database persistence.
    pub fn set_repository(&mut self, repository: Arc<dyn JobRepository>) {
        self.job_repository = Some(repository);
    }

    /// Set the session repository for persisting media outputs (e.g., thumbnails).
    /// This can only be called once.
    pub fn set_session_repo(&self, repo: Arc<dyn SessionRepository>) {
        let _ = self.session_repo.set(repo);
    }

    /// Set the streamer repository for looking up streamer metadata.
    /// This can only be called once.
    pub fn set_streamer_repo(&self, repo: Arc<dyn StreamerRepository>) {
        let _ = self.streamer_repo.set(repo);
    }

    /// Persist thumbnail output to media_outputs table.
    async fn _persist_thumbnail_output(&self, session_id: &str, output_path: &str) {
        let Some(repo) = self.session_repo.get() else {
            return;
        };

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

    pub async fn append_log_entry(&self, job_id: &str, logs: &[JobLogEntry]) -> Result<()> {
        let _ = self.persist_logs_to_db(job_id, logs).await?;
        Ok(())
    }

    async fn persist_logs_to_db(
        &self,
        job_id: &str,
        logs: &[JobLogEntry],
    ) -> Result<Vec<JobLogEntry>> {
        let Some(repo) = &self.job_repository else {
            return Ok(vec![]);
        };
        if logs.is_empty() {
            return Ok(vec![]);
        }

        let cursor = self.persisted_log_cursor.get(job_id).map(|c| *c);

        let mut start_index = 0usize;
        if let Some(cursor) = cursor {
            if let Some(pos) = logs
                .iter()
                .rposition(|e| log_signature(e) == cursor.last_sig)
            {
                start_index = pos.saturating_add(1);
            } else if let Some(pos) = logs
                .iter()
                .position(|e| e.timestamp.timestamp_millis() > cursor.last_ts_ms)
            {
                start_index = pos;
            } else {
                start_index = logs.len();
            }
        }

        let new_logs: Vec<JobLogEntry> = logs[start_index..].to_vec();
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
                    created_at: entry.timestamp.to_rfc3339(),
                    level: Some(log_level_to_db(entry.level).to_string()),
                    message: Some(entry.message.clone()),
                })
                .collect();
            repo.add_execution_logs(&db_logs).await?;
        }

        if let Some(last) = logs.last() {
            self.persisted_log_cursor.insert(
                job_id.to_string(),
                PersistedLogCursor {
                    last_sig: log_signature(last),
                    last_ts_ms: last.timestamp.timestamp_millis(),
                },
            );
        }

        Ok(new_logs)
    }

    /// Create a progress reporter for a job.
    pub fn progress_reporter(&self, job_id: &str) -> ProgressReporter {
        ProgressReporter::new(job_id.to_string(), self.progress_tx.clone())
    }

    /// Get the latest progress snapshot for a job.
    pub async fn get_job_progress(&self, job_id: &str) -> Result<Option<JobProgressSnapshot>> {
        if let Some(snapshot) = self.progress_cache.get(job_id) {
            return Ok(Some(snapshot.clone()));
        }

        let Some(repo) = &self.job_repository else {
            return Ok(None);
        };

        let Some(row) = repo.get_job_execution_progress(job_id).await? else {
            return Ok(None);
        };

        let snapshot = serde_json::from_str::<JobProgressSnapshot>(&row.progress)
            .map_err(|e| Error::Other(format!("Failed to parse job progress JSON: {}", e)))?;

        self.progress_cache
            .insert(job_id.to_string(), snapshot.clone());
        Ok(Some(snapshot))
    }

    /// Resolve missing metadata (streamer_name, session_title) for a job.
    ///
    /// This is useful for jobs recovered from the database where metadata
    /// is not persisted. It looks up the streamer and session repositories.
    pub async fn resolve_job_metadata(&self, job: &mut Job) {
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

        // Only resolve if we have a session_id and title is missing
        if job.session_title.is_none()
            && !job.session_id.is_empty()
            && let Some(session_repo) = self.session_repo.get()
        {
            match session_repo.get_session(&job.session_id).await {
                Ok(session) => {
                    // Parse titles JSON and get the most recent one
                    if let Some(titles_json) = session.titles
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
    }

    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();

        // Persist to database if repository is available
        if let Some(repo) = &self.job_repository {
            let db_model = job_to_db_model(&job);
            repo.create_job(&db_model).await?;
        }

        // Add to in-memory cache
        self.jobs_cache.insert(job_id.clone(), job);

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued job {} of type {}", job_id, job_type);

        // Notify waiting workers
        self.notify.notify_one();

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

        // Notify waiting workers
        self.notify.notify_one();

        Ok(job_id)
    }

    /// Dequeue a job for processing.
    pub async fn dequeue(&self, job_types: Option<&[String]>) -> Result<Option<Job>> {
        // Note: This is called frequently by worker pools, so we use trace level
        // to avoid log spam. Use debug level only when a job is actually dequeued.

        // Try to get from database if repository is available
        if let Some(repo) = &self.job_repository {
            if let Some(db_job) = repo.claim_next_pending_job(job_types).await? {
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

                // Create cancellation token for this job
                self.cancellation_tokens
                    .insert(job.id.clone(), CancellationToken::new());

                return Ok(Some(job));
            }
        } else {
            // Fallback to in-memory cache
            let mut selected_id: Option<String> = None;
            for entry in self.jobs_cache.iter() {
                let job = entry.value();
                if job.status != JobStatus::Pending {
                    continue;
                }
                if let Some(types) = job_types
                    && !types.iter().any(|t| t == &job.job_type)
                {
                    continue;
                }
                selected_id = Some(entry.key().clone());
                break;
            }

            if let Some(job_id) = selected_id
                && let Some(mut job_ref) = self.jobs_cache.get_mut(&job_id)
            {
                if job_ref.status != JobStatus::Pending {
                    return Ok(None);
                }
                job_ref.status = JobStatus::Processing;
                job_ref.started_at = Some(Utc::now());
                let job = job_ref.clone();
                drop(job_ref);

                self.cancellation_tokens
                    .insert(job_id, CancellationToken::new());
                return Ok(Some(job));
            }
        }

        Ok(None)
    }

    /// Count pending jobs, optionally filtered by job types.
    pub async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64> {
        if let Some(repo) = &self.job_repository {
            return repo.count_pending_jobs(job_types).await;
        }

        let mut count: u64 = 0;
        for entry in self.jobs_cache.iter() {
            let job = entry.value();
            if job.status != JobStatus::Pending {
                continue;
            }
            if let Some(types) = job_types
                && !types.iter().any(|t| t == &job.job_type)
            {
                continue;
            }
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    /// Wait for a job to become available.
    pub async fn wait_for_job(&self) {
        self.notify.notified().await;
    }

    /// Mark a job as completed.
    pub async fn complete(&self, job_id: &str, result: JobResult) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(job_id).await?;
            db_job.mark_completed();
            if !result.outputs.is_empty() {
                db_job.set_outputs(&result.outputs);
            }
            // Persist the processing duration
            db_job.duration_secs = Some(result.duration_secs);

            // Persist detailed logs to job_execution_logs and keep only a capped summary in execution_info.
            if !result.logs.is_empty() {
                let new_logs = self.persist_logs_to_db(job_id, &result.logs).await?;

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
                db_job.execution_info = Some(serde_json::to_string(&exec_info)?);
            }

            // Calculate queue wait time
            if let Some(created) = db_job.created_at.parse::<chrono::DateTime<Utc>>().ok()
                && let Some(started) = &db_job.started_at
                && let Ok(started_dt) = started.parse::<chrono::DateTime<Utc>>()
            {
                let wait_secs = (started_dt - created).num_milliseconds() as f64 / 1000.0;
                db_job.queue_wait_secs = Some(wait_secs.max(0.0));
            }
            repo.update_job(&db_job).await?;
        }

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.status = JobStatus::Completed;
            job.completed_at = Some(Utc::now());
            job.outputs = result.outputs;
            job.duration_secs = Some(result.duration_secs);

            // Update cached execution info if logs are present
            if !result.logs.is_empty() {
                let mut exec_info = job.execution_info.clone().unwrap_or_default();
                update_log_summary(&mut exec_info, &result.logs);
                extend_logs_capped(&mut exec_info, &result.logs);
                job.execution_info = Some(exec_info);
            }

            // Calculate queue wait time
            if let (Some(created), Some(started)) = (Some(job.created_at), job.started_at) {
                let wait_secs = (started - created).num_milliseconds() as f64 / 1000.0;
                job.queue_wait_secs = Some(wait_secs.max(0.0));
            }
        }

        // Remove cancellation token
        let _ = self.cancellation_tokens.remove(job_id);
        let _ = self.persisted_log_cursor.remove(job_id);

        self.depth.fetch_sub(1, Ordering::SeqCst);
        info!("Job {} completed in {:.2}s", job_id, result.duration_secs);
        Ok(())
    }
    /// Mark a job as failed.
    pub async fn fail(&self, job_id: &str, error: &str) -> Result<()> {
        self.fail_internal(job_id, error, None, None, None, false)
            .await?;
        warn!("Job {} failed: {}", job_id, error);
        Ok(())
    }

    /// Mark a job as failed with step information for observability.
    /// Records the error message, failing step, and processor name in execution_info.
    pub async fn fail_with_step_info(
        &self,
        job_id: &str,
        error: &str,
        processor_name: Option<&str>,
        step_number: Option<u32>,
        total_steps: Option<u32>,
    ) -> Result<()> {
        self.fail_internal(
            job_id,
            error,
            processor_name,
            step_number,
            total_steps,
            true,
        )
        .await?;
        warn!(
            "Job {} failed at step {:?}/{:?} (processor: {:?}): {}",
            job_id, step_number, total_steps, processor_name, error
        );
        Ok(())
    }

    /// Get a job by ID.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        // Try database first if repository is available
        if let Some(repo) = &self.job_repository {
            match repo.get_job(id).await {
                Ok(db_job) => return Ok(Some(db_model_to_job(&db_job))),
                Err(Error::NotFound { .. }) => return Ok(None),
                Err(e) => return Err(e),
            }
        }

        // Fallback to cache
        Ok(self.jobs_cache.get(id).map(|job| job.clone()))
    }

    /// List job execution logs (paged). When a database repository is configured, this reads
    /// from `job_execution_logs`; otherwise it paginates the cached `execution_info.logs`.
    pub async fn list_job_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobLogEntry>, u64)> {
        if let Some(repo) = &self.job_repository {
            let (rows, total) = repo.list_execution_logs(job_id, pagination).await?;

            let logs = rows
                .into_iter()
                .map(|row| {
                    let timestamp = chrono::DateTime::parse_from_rfc3339(&row.created_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

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
                        };
                    }

                    JobLogEntry {
                        timestamp,
                        level: LogLevel::Info,
                        message: row.entry,
                    }
                })
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

            return Ok((vec![], 0));
        }

        let logs = self
            .jobs_cache
            .get(job_id)
            .and_then(|job| job.execution_info.clone())
            .map(|info| info.logs)
            .unwrap_or_default();

        let total = logs.len() as u64;
        let start = pagination.offset as usize;
        let limit = pagination.limit as usize;
        let page: Vec<_> = logs.iter().skip(start).take(limit).cloned().collect();

        Ok((page, total))
    }

    /// List jobs with filters and pagination.
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        if let Some(repo) = &self.job_repository {
            let (db_jobs, total) = repo.list_jobs_filtered(filters, pagination).await?;
            let jobs = db_jobs.iter().map(db_model_to_job).collect();
            return Ok((jobs, total));
        }

        // Fallback to cache (basic filtering)
        let ids = self.filter_cached_job_ids(filters);
        let total = ids.len() as u64;

        // Apply pagination
        let start = pagination.offset as usize;
        let end = std::cmp::min(start + pagination.limit as usize, ids.len());
        let jobs = if start < ids.len() {
            ids[start..end]
                .iter()
                .filter_map(|id| self.jobs_cache.get(id).map(|job| job.clone()))
                .collect()
        } else {
            vec![]
        };

        Ok((jobs, total))
    }

    /// List jobs with filters and pagination, without running a total `COUNT(*)`.
    pub async fn list_jobs_page(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<Job>> {
        if let Some(repo) = &self.job_repository {
            let db_jobs = repo.list_jobs_page_filtered(filters, pagination).await?;
            let jobs = db_jobs.iter().map(db_model_to_job).collect();
            return Ok(jobs);
        }

        // Fallback to cache (basic filtering)
        let ids = self.filter_cached_job_ids(filters);
        let start = pagination.offset as usize;
        let end = std::cmp::min(start + pagination.limit as usize, ids.len());
        let jobs = if start < ids.len() {
            ids[start..end]
                .iter()
                .filter_map(|id| self.jobs_cache.get(id).map(|job| job.clone()))
                .collect()
        } else {
            vec![]
        };

        Ok(jobs)
    }

    /// Retry a failed job.
    /// Returns error if job is not in Failed status.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        // Get the job
        let job = self
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        // Validate job is in Failed status
        if job.status != JobStatus::Failed {
            return Err(Error::InvalidStateTransition {
                from: format!("{:?}", job.status),
                to: "Pending".to_string(),
            });
        }

        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            repo.reset_job_for_retry(id).await?;
        }

        // Update cache
        let updated_job = {
            if let Some(mut cached_job) = self.jobs_cache.get_mut(id) {
                cached_job.status = JobStatus::Pending;
                cached_job.started_at = None;
                cached_job.completed_at = None;
                cached_job.error = None;
                cached_job.retry_count += 1;
                cached_job.clone()
            } else {
                // Create from scratch if not in cache
                let mut new_job = job.clone();
                new_job.status = JobStatus::Pending;
                new_job.started_at = None;
                new_job.completed_at = None;
                new_job.error = None;
                new_job.retry_count += 1;
                self.jobs_cache.insert(id.to_string(), new_job.clone());
                new_job
            }
        };

        self.depth.fetch_add(1, Ordering::SeqCst);
        self.notify.notify_one();

        info!("Job {} retried (attempt {})", id, updated_job.retry_count);
        Ok(updated_job)
    }

    /// Cancel a job.
    /// For Pending jobs: removes from queue and marks as Interrupted.
    /// For Processing jobs: signals cancellation and marks as Interrupted.
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
                from: format!("{:?}", job.status),
                to: "Interrupted".to_string(),
            });
        }

        // Signal cancellation for processing jobs
        if job.status == JobStatus::Processing
            && let Some(token) = self.cancellation_tokens.get(id)
        {
            token.cancel();
        }

        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            repo.mark_job_interrupted(id).await?;
        }

        // Update cache and get the updated job
        let cancelled_job = {
            if let Some(mut cached_job) = self.jobs_cache.get_mut(id) {
                cached_job.status = JobStatus::Interrupted;
                cached_job.completed_at = Some(Utc::now());
                cached_job.clone()
            } else {
                // Job not in cache, return original with updated status
                let mut updated = job.clone();
                updated.status = JobStatus::Interrupted;
                updated.completed_at = Some(Utc::now());
                updated
            }
        };

        // Remove cancellation token
        let _ = self.cancellation_tokens.remove(id);
        let _ = self.persisted_log_cursor.remove(id);

        // Decrement depth only for pending jobs (processing jobs already counted)
        if job.status == JobStatus::Pending {
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }

        info!("Job {} cancelled", id);
        Ok(cancelled_job)
    }

    /// Delete a job.
    /// Only allows deleting jobs in terminal states (Completed, Failed, Interrupted).
    /// Removes from database and cache.
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        // Try to get from cache first to check status
        if let Some(job) = self.jobs_cache.get(id) {
            match job.status {
                JobStatus::Pending | JobStatus::Processing => {
                    return Err(Error::InvalidStateTransition {
                        from: format!("{:?}", job.status),
                        to: "Deleted".to_string(),
                    });
                }
                _ => {} // Terminal states are fine for deletion
            }
        } else if let Some(repo) = &self.job_repository {
            // If not in cache, check DB
            match repo.get_job(id).await {
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
                        DbJobStatus::Pending | DbJobStatus::Processing => {
                            return Err(Error::InvalidStateTransition {
                                from: format!("{:?}", status),
                                to: "Deleted".to_string(),
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

        // 1. Delete from database if available
        if let Some(repo) = &self.job_repository {
            repo.delete_job(id).await?;
        }

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
        let mut cancelled_jobs = Vec::new();

        // If we have a repository, cancel in database first
        if let Some(repo) = &self.job_repository {
            // Get all jobs in the pipeline before cancelling
            let db_jobs = repo.get_jobs_by_pipeline(pipeline_id).await?;

            // Cancel in database
            let count = repo.cancel_jobs_by_pipeline(pipeline_id).await?;
            info!(
                "Cancelled {} jobs in pipeline {} (database)",
                count, pipeline_id
            );

            // Convert to Job and collect cancelled ones
            for db_job in db_jobs {
                if db_job.status == "PENDING" || db_job.status == "PROCESSING" {
                    let mut job = db_model_to_job(&db_job);
                    job.status = JobStatus::Interrupted;
                    job.completed_at = Some(Utc::now());
                    cancelled_jobs.push(job);
                }
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

        let mut depth_reduction = 0usize;
        for id in &ids {
            if let Some(mut job) = self.jobs_cache.get_mut(id) {
                if job.pipeline_id.as_deref() != Some(pipeline_id) {
                    continue;
                }

                if job.status == JobStatus::Pending {
                    depth_reduction += 1;
                }

                if job.status == JobStatus::Processing
                    && let Some(token) = self.cancellation_tokens.get(id)
                {
                    token.cancel();
                }

                job.status = JobStatus::Interrupted;
                job.completed_at = Some(Utc::now());

                if self.job_repository.is_none() {
                    cancelled_jobs.push(job.clone());
                }
            }

            let _ = self.cancellation_tokens.remove(id);
            let _ = self.persisted_log_cursor.remove(id);
        }

        if depth_reduction > 0 {
            self.depth.fetch_sub(depth_reduction, Ordering::SeqCst);
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
        self.cancellation_tokens.get(job_id).map(|t| t.clone())
    }

    /// Recover jobs from database on startup.
    /// Loads pending jobs and resets processing jobs to pending.
    pub async fn recover_jobs(&self) -> Result<usize> {
        let Some(repo) = &self.job_repository else {
            return Ok(0);
        };

        // Reset interrupted jobs to pending
        let reset_interrupted = repo.reset_interrupted_jobs().await?;
        if reset_interrupted > 0 {
            info!("Reset {} interrupted jobs to pending", reset_interrupted);
        }

        // Reset processing jobs to pending (they were interrupted by shutdown)
        let reset_processing = repo.reset_processing_jobs().await?;
        if reset_processing > 0 {
            info!("Reset {} processing jobs to pending", reset_processing);
        }

        // Load pending jobs into cache
        let filters = JobFilters {
            status: Some(DbJobStatus::Pending),
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

    /// Get job statistics.
    pub async fn get_stats(&self) -> Result<JobStats> {
        if let Some(repo) = &self.job_repository {
            let counts = repo.get_job_counts_by_status().await?;
            let avg_processing_time = repo.get_avg_processing_time().await?;

            return Ok(JobStats {
                pending: counts.pending,
                processing: counts.processing,
                completed: counts.completed,
                failed: counts.failed,
                interrupted: counts.interrupted,
                avg_processing_time_secs: avg_processing_time,
            });
        }

        // Fallback to cache
        let mut stats = JobStats::default();

        for job in self.jobs_cache.iter().map(|e| e.value().clone()) {
            match job.status {
                JobStatus::Pending => stats.pending += 1,
                JobStatus::Processing => stats.processing += 1,
                JobStatus::Completed => stats.completed += 1,
                JobStatus::Failed => stats.failed += 1,
                JobStatus::Interrupted => stats.interrupted += 1,
            }
        }

        Ok(stats)
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
            let split_job = Job::new_pipeline_step(
                job.job_type.clone(),
                vec![input.clone()],
                vec![], // Outputs will be determined by processor
                job.streamer_id.clone(),
                job.session_id.clone(),
                job.pipeline_id.clone(),
            )
            .with_priority(job.priority)
            .with_config(job.config.clone().unwrap_or_else(|| "{}".to_string()));

            let job_id = self.enqueue(split_job).await?;
            created_job_ids.push(job_id);
        }

        // Mark the original job as completed (it was split)
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(&job.id).await?;
            db_job.mark_completed();
            db_job.set_outputs(&[]); // No outputs, job was split
            repo.update_job(&db_job).await?;
        }

        // Update cache for original job
        if let Some(mut original) = self.jobs_cache.get_mut(&job.id) {
            original.status = JobStatus::Completed;
            original.completed_at = Some(Utc::now());
        }

        // Remove cancellation token for original job
        let _ = self.cancellation_tokens.remove(&job.id);
        let _ = self.persisted_log_cursor.remove(&job.id);

        // Adjust queue depth to account for the original job completing.
        // The split jobs are enqueued and already increment depth.
        self.depth.fetch_sub(1, Ordering::SeqCst);

        info!(
            "Split job {} into {} jobs for single-input processing",
            job.id,
            created_job_ids.len()
        );

        Ok(created_job_ids)
    }

    /// Track partial outputs for a job (used for cleanup on failure).
    /// Updates the job's execution_info with items_produced.
    pub async fn track_partial_outputs(&self, job_id: &str, outputs: &[String]) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let exec_info_str = repo.get_job_execution_info(job_id).await?;
            let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
                exec_info_str.as_deref(),
                JsonContext::JobField {
                    job_id,
                    field: "execution_info",
                },
                "Invalid execution_info JSON; resetting to defaults",
            );

            // Add the partial outputs
            exec_info.items_produced.extend(outputs.iter().cloned());

            let exec_info_json = serde_json::to_string(&exec_info)?;
            repo.update_job_execution_info(job_id, &exec_info_json)
                .await?;
        }

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            let exec_info = job
                .execution_info
                .get_or_insert_with(JobExecutionInfo::default);
            exec_info.items_produced.extend(outputs.iter().cloned());
        }

        Ok(())
    }

    /// Get partial outputs for a job (for cleanup on failure).
    pub async fn get_partial_outputs(&self, job_id: &str) -> Result<Vec<String>> {
        // Try cache first
        if let Some(job) = self.jobs_cache.get(job_id)
            && let Some(ref exec_info) = job.execution_info
        {
            return Ok(exec_info.items_produced.clone());
        }

        // Try database
        if let Some(repo) = &self.job_repository {
            let db_job = repo.get_job(job_id).await?;
            if let Some(exec_info_str) = &db_job.execution_info
                && let Ok(exec_info) = serde_json::from_str::<JobExecutionInfo>(exec_info_str)
            {
                return Ok(exec_info.items_produced);
            }
        }

        Ok(vec![])
    }

    /// Fail a job and clean up partial outputs.
    pub async fn fail_with_cleanup(&self, job_id: &str, error: &str) -> Result<Vec<String>> {
        self.fail_with_cleanup_and_step_info(job_id, error, None, None, None)
            .await
    }

    /// Fail a job with step info and clean up partial outputs.
    /// Records the error message, failing step, and processor name in execution_info.
    pub async fn fail_with_cleanup_and_step_info(
        &self,
        job_id: &str,
        error: &str,
        processor_name: Option<&str>,
        step_number: Option<u32>,
        total_steps: Option<u32>,
    ) -> Result<Vec<String>> {
        // Get partial outputs before failing
        let partial_outputs = self.get_partial_outputs(job_id).await?;

        // Mark job as failed with step info
        self.fail_with_step_info(job_id, error, processor_name, step_number, total_steps)
            .await?;

        // Return partial outputs for cleanup by caller
        Ok(partial_outputs)
    }

    /// Update execution info for a job.
    pub async fn update_execution_info(
        &self,
        job_id: &str,
        exec_info: JobExecutionInfo,
    ) -> Result<()> {
        let mut exec_info = exec_info;

        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            if !exec_info.logs.is_empty() {
                // make_contiguous() allows VecDeque to be used as a slice
                let new_logs = self
                    .persist_logs_to_db(job_id, exec_info.logs.make_contiguous())
                    .await?;
                update_log_summary(&mut exec_info, &new_logs);
                cap_logs_in_place(&mut exec_info.logs, EXECUTION_INFO_MAX_LOGS);
            }

            let exec_info_json = serde_json::to_string(&exec_info)?;
            repo.update_job_execution_info(job_id, &exec_info_json)
                .await?;
        }

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            if !exec_info.logs.is_empty() {
                cap_logs_in_place(&mut exec_info.logs, EXECUTION_INFO_MAX_LOGS);
            }
            job.execution_info = Some(exec_info);
        }

        Ok(())
    }

    /// Get the current queue depth.
    pub fn depth(&self) -> usize {
        self.depth.load(Ordering::SeqCst)
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

    async fn fail_internal(
        &self,
        job_id: &str,
        error: &str,
        processor_name: Option<&str>,
        step_number: Option<u32>,
        total_steps: Option<u32>,
        update_cache_logs: bool,
    ) -> Result<()> {
        let log_entry = JobLogEntry::error(format!("Job failed: {}", error));

        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            repo.mark_job_failed(job_id, error).await?;

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

            extend_logs_capped(&mut exec_info, std::slice::from_ref(&log_entry));
            update_log_summary(&mut exec_info, std::slice::from_ref(&log_entry));

            let _ = self
                .persist_logs_to_db(job_id, std::slice::from_ref(&log_entry))
                .await?;

            let exec_info_json = serde_json::to_string(&exec_info)?;
            repo.update_job_execution_info(job_id, &exec_info_json)
                .await?;
        }

        // Update cache
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.status = JobStatus::Failed;
            job.completed_at = Some(Utc::now());
            job.error = Some(error.to_string());

            if update_cache_logs {
                let exec_info = job
                    .execution_info
                    .get_or_insert_with(JobExecutionInfo::default);
                if let Some(name) = processor_name {
                    exec_info.current_processor = Some(name.to_string());
                }
                if let Some(step) = step_number {
                    exec_info.current_step = Some(step);
                }
                if let Some(total) = total_steps {
                    exec_info.total_steps = Some(total);
                }
                extend_logs_capped(exec_info, std::slice::from_ref(&log_entry));
                update_log_summary(exec_info, std::slice::from_ref(&log_entry));
            }
        }

        // Remove cancellation token
        let _ = self.cancellation_tokens.remove(job_id);
        let _ = self.persisted_log_cursor.remove(job_id);

        self.depth.fetch_sub(1, Ordering::SeqCst);
        Ok(())
    }

    fn filter_cached_job_ids(&self, filters: &JobFilters) -> Vec<String> {
        let mut items: Vec<(i32, chrono::DateTime<Utc>, String)> = Vec::new();

        for entry in self.jobs_cache.iter() {
            let job = entry.value();

            if let Some(status) = &filters.status {
                let status_enum = match status {
                    DbJobStatus::Pending => JobStatus::Pending,
                    DbJobStatus::Processing => JobStatus::Processing,
                    DbJobStatus::Completed => JobStatus::Completed,
                    DbJobStatus::Failed => JobStatus::Failed,
                    DbJobStatus::Interrupted => JobStatus::Interrupted,
                };
                if job.status != status_enum {
                    continue;
                }
            }
            if let Some(streamer_id) = &filters.streamer_id
                && &job.streamer_id != streamer_id
            {
                continue;
            }

            if let Some(session_id) = &filters.session_id
                && &job.session_id != session_id
            {
                continue;
            }

            items.push((job.priority, job.created_at, job.id.clone()));
        }

        // Match DB ordering: priority DESC, created_at DESC, id ASC for stability.
        items.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        items.into_iter().map(|(_, _, id)| id).collect()
    }
}

fn spawn_progress_aggregator(
    repo: Option<Arc<dyn JobRepository>>,
    mut rx: tokio::sync::mpsc::Receiver<JobProgressUpdate>,
    progress_cache: DashMap<String, JobProgressSnapshot>,
) {
    if tokio::runtime::Handle::try_current().is_err() {
        // Some unit tests construct JobQueue outside a Tokio runtime. Progress persistence
        // is best-effort and can be disabled in those contexts.
        return;
    }

    tokio::spawn(async move {
        let mut pending: HashMap<String, JobProgressSnapshot> = HashMap::new();
        let flush_every = std::time::Duration::from_millis(PROGRESS_FLUSH_INTERVAL_MS);
        let mut tick = tokio::time::interval(flush_every);

        loop {
            tokio::select! {
                _ = tick.tick() => {
                    if pending.is_empty() {
                        continue;
                    }
                    let Some(repo) = &repo else {
                        pending.clear();
                        continue;
                    };
                    for (job_id, snapshot) in pending.drain() {
                        let progress = match serde_json::to_string(&snapshot) {
                            Ok(s) => s,
                            Err(_) => continue,
                        };
                        let row = JobExecutionProgressDbModel {
                            job_id: job_id.clone(),
                            kind: format!("{:?}", snapshot.kind).to_ascii_lowercase(),
                            progress,
                            updated_at: snapshot.updated_at.to_rfc3339(),
                        };
                        let _ = repo.upsert_job_execution_progress(&row).await;
                    }
                }
                update = rx.recv() => {
                    let Some(update) = update else { break; };
                    progress_cache.insert(update.job_id.clone(), update.snapshot.clone());
                    pending.insert(update.job_id, update.snapshot);
                }
            }
        }
    });
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
    /// Number of interrupted jobs.
    pub interrupted: u64,
    /// Average processing time in seconds.
    pub avg_processing_time_secs: Option<f64>,
}

/// Convert a Job to JobDbModel.
fn job_to_db_model(job: &Job) -> JobDbModel {
    let status = match job.status {
        JobStatus::Pending => DbJobStatus::Pending,
        JobStatus::Processing => DbJobStatus::Processing,
        JobStatus::Completed => DbJobStatus::Completed,
        JobStatus::Failed => DbJobStatus::Failed,
        JobStatus::Interrupted => DbJobStatus::Interrupted,
    };

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
        status: status.as_str().to_string(),
        config: job.config.clone().unwrap_or_else(|| "{}".to_string()),
        state: "{}".to_string(),
        created_at: job.created_at.to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        input: Some(inputs_json),
        outputs: Some(outputs_json),
        priority: job.priority,
        streamer_id: Some(job.streamer_id.clone()),
        session_id: Some(job.session_id.clone()),
        started_at: job.started_at.map(|dt| dt.to_rfc3339()),
        completed_at: job.completed_at.map(|dt| dt.to_rfc3339()),
        error: job.error.clone(),
        retry_count: job.retry_count,
        pipeline_id: job.pipeline_id.clone(),
        execution_info: execution_info_json,
        duration_secs: job.duration_secs,
        queue_wait_secs: job.queue_wait_secs,
        dag_step_execution_id: job.dag_step_execution_id.clone(),
    }
}

/// Convert a JobDbModel to Job.
fn db_model_to_job(db_job: &JobDbModel) -> Job {
    let status = match DbJobStatus::parse(&db_job.status) {
        Some(DbJobStatus::Pending) => JobStatus::Pending,
        Some(DbJobStatus::Processing) => JobStatus::Processing,
        Some(DbJobStatus::Completed) => JobStatus::Completed,
        Some(DbJobStatus::Failed) => JobStatus::Failed,
        Some(DbJobStatus::Interrupted) => JobStatus::Interrupted,
        None => JobStatus::Pending,
    };

    let created_at = chrono::DateTime::parse_from_rfc3339(&db_job.created_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let started_at = db_job.started_at.as_ref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .ok()
    });

    let completed_at = db_job.completed_at.as_ref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .ok()
    });

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
        pipeline_id: db_job.pipeline_id.clone(),
        // Parse execution_info JSON
        execution_info: db_job
            .execution_info
            .as_ref()
            .and_then(|s| serde_json::from_str::<JobExecutionInfo>(s).ok()),
        duration_secs: db_job.duration_secs,
        queue_wait_secs: db_job.queue_wait_secs,
        dag_step_execution_id: db_job.dag_step_execution_id.clone(),
        streamer_name: None,
        session_title: None,
        platform: None,
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_queue_config_default() {
        let config = JobQueueConfig::default();
        assert_eq!(config.warning_threshold, 100);
        assert_eq!(config.critical_threshold, 500);
    }

    #[test]
    fn test_job_creation() {
        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec!["/output.mp4".to_string()],
            "streamer-1",
            "session-1",
        )
        .with_priority(10);

        assert_eq!(job.job_type, "remux");
        assert_eq!(job.priority, 10);
        assert_eq!(job.status, JobStatus::Pending);
    }

    #[test]
    fn test_queue_depth_status() {
        let config = JobQueueConfig {
            warning_threshold: 10,
            critical_threshold: 20,
            poll_interval_ms: 100,
        };
        let queue = JobQueue::with_config(config);

        assert_eq!(queue.depth_status(), QueueDepthStatus::Normal);
    }

    #[tokio::test]
    async fn test_enqueue_dequeue() {
        let queue = JobQueue::new();

        let job = Job::new(
            "test",
            vec!["input".to_string()],
            vec!["output".to_string()],
            "streamer",
            "session",
        );
        let job_id = queue.enqueue(job).await.unwrap();

        assert!(!job_id.is_empty());
        assert_eq!(queue.depth(), 1);
    }

    // ========================================================================
    // Fan-out and Multi-input Support Tests
    // ========================================================================
    #[tokio::test]
    async fn test_split_job_single_input_no_split() {
        // A job with single input should not be split
        let queue = JobQueue::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job.clone()).await.unwrap();

        let split_ids = queue.split_job_for_single_input(&job).await.unwrap();

        // Should return the original job ID (no split needed)
        assert_eq!(split_ids.len(), 1);
        assert_eq!(split_ids[0], job_id);
    }

    /// Test that split jobs preserve pipeline context.
    #[tokio::test]
    async fn test_split_job_preserves_pipeline_context() {
        let queue = JobQueue::new();

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
        .with_pipeline_id("pipeline-1".to_string());

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
        }
    }

    #[tokio::test]
    async fn test_track_partial_outputs() {
        let queue = JobQueue::new();

        let job = Job::new(
            "compress",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Track some partial outputs
        let partial = vec![
            "/tmp/partial1.mp4".to_string(),
            "/tmp/partial2.mp4".to_string(),
        ];
        queue
            .track_partial_outputs(&job_id, &partial)
            .await
            .unwrap();

        // Verify partial outputs are tracked
        let tracked = queue.get_partial_outputs(&job_id).await.unwrap();
        assert_eq!(tracked.len(), 2);
        assert!(tracked.contains(&"/tmp/partial1.mp4".to_string()));
        assert!(tracked.contains(&"/tmp/partial2.mp4".to_string()));
    }

    #[tokio::test]
    async fn test_fail_with_cleanup_returns_partial_outputs() {
        let queue = JobQueue::new();

        let job = Job::new(
            "compress",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Track some partial outputs
        let partial = vec!["/tmp/partial.mp4".to_string()];
        queue
            .track_partial_outputs(&job_id, &partial)
            .await
            .unwrap();

        // Fail the job and get partial outputs for cleanup
        let outputs = queue
            .fail_with_cleanup(&job_id, "Test error")
            .await
            .unwrap();

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], "/tmp/partial.mp4");

        // Verify job is failed
        let failed_job = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(failed_job.status, JobStatus::Failed);
        assert_eq!(failed_job.error, Some("Test error".to_string()));
    }

    #[tokio::test]
    async fn test_update_execution_info() {
        let queue = JobQueue::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Update execution info
        let exec_info = JobExecutionInfo::new()
            .with_processor("RemuxProcessor")
            .with_step(1, 3)
            .with_input_size(1024);

        queue
            .update_execution_info(&job_id, exec_info)
            .await
            .unwrap();

        // Verify execution info is updated
        let updated_job = queue.get_job(&job_id).await.unwrap().unwrap();
        let info = updated_job.execution_info.unwrap();
        assert_eq!(info.current_processor, Some("RemuxProcessor".to_string()));
        assert_eq!(info.current_step, Some(1));
        assert_eq!(info.total_steps, Some(3));
        assert_eq!(info.input_size_bytes, Some(1024));
    }

    #[test]
    fn test_job_execution_info_builder() {
        let mut info = JobExecutionInfo::new()
            .with_processor("TestProcessor")
            .with_step(2, 5)
            .with_input_size(2048)
            .with_output_size(1024);

        info.add_item_produced("/tmp/item1.mp4");
        info.log_info("Processing started");
        info.log_error("Something went wrong");

        assert_eq!(info.current_processor, Some("TestProcessor".to_string()));
        assert_eq!(info.current_step, Some(2));
        assert_eq!(info.total_steps, Some(5));
        assert_eq!(info.input_size_bytes, Some(2048));
        assert_eq!(info.output_size_bytes, Some(1024));
        assert_eq!(info.items_produced.len(), 1);
        assert_eq!(info.logs.len(), 2);
    }

    // ========================================================================
    // Job Failure Handling Tests
    // ========================================================================

    /// Test that fail_with_step_info records the failing step and processor.
    #[tokio::test]
    async fn test_fail_with_step_info_records_failure_details() {
        let queue = JobQueue::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Fail the job with step info
        queue
            .fail_with_step_info(
                &job_id,
                "FFmpeg error: invalid input",
                Some("RemuxProcessor"),
                Some(1),
                Some(3),
            )
            .await
            .unwrap();

        // Verify job is failed with correct info
        let failed_job = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(failed_job.status, JobStatus::Failed);
        assert_eq!(
            failed_job.error,
            Some("FFmpeg error: invalid input".to_string())
        );

        // Verify execution info contains failure details
        let exec_info = failed_job.execution_info.unwrap();
        assert_eq!(
            exec_info.current_processor,
            Some("RemuxProcessor".to_string())
        );
        assert_eq!(exec_info.current_step, Some(1));
        assert_eq!(exec_info.total_steps, Some(3));

        // Verify error log was added
        assert!(!exec_info.logs.is_empty());
        let last_log = exec_info.logs.back().unwrap();
        assert_eq!(last_log.level, LogLevel::Error);
        assert!(last_log.message.contains("FFmpeg error"));
    }

    /// Test that fail_with_cleanup_and_step_info combines cleanup and step info.

    #[tokio::test]
    async fn test_fail_with_cleanup_and_step_info() {
        let queue = JobQueue::new();

        let job = Job::new(
            "compress",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Track some partial outputs
        let partial = vec!["/tmp/partial.mp4".to_string()];
        queue
            .track_partial_outputs(&job_id, &partial)
            .await
            .unwrap();

        // Fail with cleanup and step info
        let outputs = queue
            .fail_with_cleanup_and_step_info(
                &job_id,
                "Compression failed",
                Some("CompressionProcessor"),
                Some(2),
                Some(4),
            )
            .await
            .unwrap();

        // Verify partial outputs are returned for cleanup
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], "/tmp/partial.mp4");

        // Verify job is failed with step info
        let failed_job = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(failed_job.status, JobStatus::Failed);
        assert_eq!(failed_job.error, Some("Compression failed".to_string()));

        let exec_info = failed_job.execution_info.unwrap();
        assert_eq!(
            exec_info.current_processor,
            Some("CompressionProcessor".to_string())
        );
        assert_eq!(exec_info.current_step, Some(2));
        assert_eq!(exec_info.total_steps, Some(4));
    }

    // ========================================================================
    // Pipeline Recovery Tests
    // ========================================================================

    /// Test that recover_jobs without repository returns 0.
    /// This verifies the fallback behavior when no database is configured.

    #[tokio::test]
    async fn test_recover_jobs_without_repository() {
        let queue = JobQueue::new();

        // Enqueue some jobs
        let job1 = Job::new(
            "remux",
            vec!["/input1.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job2 = Job::new(
            "upload",
            vec!["/input2.flv".to_string()],
            vec![],
            "streamer-2",
            "session-2",
        );
        queue.enqueue(job1).await.unwrap();
        queue.enqueue(job2).await.unwrap();

        // Without repository, recover_jobs returns 0
        let recovered = queue.recover_jobs().await.unwrap();
        assert_eq!(recovered, 0);

        // But in-memory jobs are still there
        assert_eq!(queue.depth(), 2);
    }

    /// Test deletion of jobs.

    #[tokio::test]
    async fn test_delete_job() {
        let queue = JobQueue::new();

        let job = Job::new(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // 1. Try to delete pending job - should fail
        let result = queue.delete_job(&job_id).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InvalidStateTransition { from, to } => {
                assert_eq!(from, "Pending");
                assert_eq!(to, "Deleted");
            }
            _ => panic!("Expected InvalidStateTransition error"),
        }

        // 2. Cancel the job
        queue.cancel_job(&job_id).await.unwrap();

        // Verify status is interrupted
        let job = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Interrupted);

        // 3. Delete the job - should succeed
        queue.delete_job(&job_id).await.unwrap();

        // 4. Verify job is gone from cache
        let job = queue.get_job(&job_id).await.unwrap();
        assert!(job.is_none());
    }
}
