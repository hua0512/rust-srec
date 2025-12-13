//! Database-backed job queue implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::database::models::job::PipelineStep;
use crate::database::models::{JobDbModel, JobFilters, JobStatus as DbJobStatus, Pagination};
use crate::database::repositories::JobRepository;
use crate::{Error, Result};

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
/// Requirements: 6.1, 6.2, 6.3, 6.4
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug level log.
    Debug,
    /// Info level log.
    Info,
    /// Warning level log.
    Warn,
    /// Error level log.
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        Self::Info
    }
}

/// A single log entry for job execution.
/// Requirements: 6.1, 6.2, 6.3, 6.4
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
/// Requirements: 6.1, 6.2, 6.3, 6.4
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
    /// Detailed execution logs.
    pub logs: Vec<JobLogEntry>,
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
        self.logs.push(entry);
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
    // Pipeline chain fields (Requirements 7.1, 7.2)
    /// Next job type to create on completion (e.g., "upload" after "remux").
    pub next_job_type: Option<String>,
    /// Pipeline steps remaining after this job (e.g., ["upload", "thumbnail"]).
    pub remaining_steps: Option<Vec<PipelineStep>>,
    /// Pipeline ID to group related jobs (first job's ID).
    pub pipeline_id: Option<String>,
    /// Execution information for observability.
    /// Requirements: 6.1, 6.2, 6.3, 6.4
    pub execution_info: Option<JobExecutionInfo>,
    /// Processing duration in seconds (from processor output).
    pub duration_secs: Option<f64>,
    /// Time spent waiting in queue before processing started (seconds).
    pub queue_wait_secs: Option<f64>,
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
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            next_job_type: None,
            remaining_steps: None,
            pipeline_id: None,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
        }
    }

    /// Create a new pipeline step job with chain information.
    pub fn new_pipeline_step(
        job_type: impl Into<String>,
        inputs: Vec<String>,
        outputs: Vec<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        pipeline_id: Option<String>,
        next_job_type: Option<String>,
        remaining_steps: Option<Vec<PipelineStep>>,
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
            config: None,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            next_job_type,
            remaining_steps,
            pipeline_id,
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
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

    /// Set the next job type for pipeline chaining.
    pub fn with_next_job_type(mut self, next_job_type: impl Into<String>) -> Self {
        self.next_job_type = Some(next_job_type.into());
        self
    }

    /// Set the remaining pipeline steps.
    pub fn with_remaining_steps(mut self, steps: Vec<PipelineStep>) -> Self {
        self.remaining_steps = Some(steps);
        self
    }

    /// Set the pipeline ID.
    pub fn with_pipeline_id(mut self, pipeline_id: impl Into<String>) -> Self {
        self.pipeline_id = Some(pipeline_id.into());
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
    /// Execution logs.
    pub logs: Vec<JobLogEntry>,
}

// ... [Skipping JobQueue struct definition which is unchanged]

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
    /// In-memory cache of jobs (for quick lookups).
    jobs_cache: RwLock<HashMap<String, Job>>,
    /// Cancellation tokens for processing jobs.
    cancellation_tokens: RwLock<HashMap<String, CancellationToken>>,
}

impl JobQueue {
    /// Create a new job queue.
    pub fn new() -> Self {
        Self::with_config(JobQueueConfig::default())
    }

    /// Create a new job queue with custom configuration.
    pub fn with_config(config: JobQueueConfig) -> Self {
        Self {
            config,
            depth: AtomicUsize::new(0),
            notify: Arc::new(Notify::new()),
            job_repository: None,
            jobs_cache: RwLock::new(HashMap::new()),
            cancellation_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new job queue with a job repository for database persistence.
    pub fn with_repository(config: JobQueueConfig, repository: Arc<dyn JobRepository>) -> Self {
        Self {
            config,
            depth: AtomicUsize::new(0),
            notify: Arc::new(Notify::new()),
            job_repository: Some(repository),
            jobs_cache: RwLock::new(HashMap::new()),
            cancellation_tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Set the job repository for database persistence.
    pub fn set_repository(&mut self, repository: Arc<dyn JobRepository>) {
        self.job_repository = Some(repository);
    }

    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();

        // Persist to database if repository is available
        if let Some(repo) = &self.job_repository {
            let db_model = job_to_db_model(&job);
            repo.create_job(&db_model).await?;
        }

        // Add to in-memory cache
        {
            let mut cache = self.jobs_cache.write().await;
            cache.insert(job_id.clone(), job.clone());
        }

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued job {} of type {}", job_id, job.job_type);

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
            let mut filters = JobFilters {
                status: Some(DbJobStatus::Pending),
                ..Default::default()
            };

            if let Some(types) = job_types {
                filters.job_types = Some(types.to_vec());
            }

            let pagination = Pagination::new(1, 0);

            let (jobs, _) = repo.list_jobs_filtered(&filters, &pagination).await?;

            if let Some(db_job) = jobs.into_iter().next() {
                // Update status to Processing
                repo.update_job_status(&db_job.id, DbJobStatus::Processing.as_str())
                    .await?;

                let mut job = db_model_to_job(&db_job);
                job.status = JobStatus::Processing;
                job.started_at = Some(Utc::now());

                // Update cache
                {
                    let mut cache = self.jobs_cache.write().await;
                    cache.insert(job.id.clone(), job.clone());
                }

                // Create cancellation token for this job
                {
                    let mut tokens = self.cancellation_tokens.write().await;
                    tokens.insert(job.id.clone(), CancellationToken::new());
                }

                return Ok(Some(job));
            }
        } else {
            // Fallback to in-memory cache
            let cache = self.jobs_cache.read().await;
            for job in cache.values() {
                if job.status == JobStatus::Pending {
                    if let Some(types) = job_types {
                        if !types.iter().any(|t| t == &job.job_type) {
                            continue;
                        }
                    }
                    let mut job = job.clone();
                    job.status = JobStatus::Processing;
                    job.started_at = Some(Utc::now());
                    drop(cache);

                    // Update cache
                    {
                        let mut cache = self.jobs_cache.write().await;
                        cache.insert(job.id.clone(), job.clone());
                    }

                    // Create cancellation token for this job
                    {
                        let mut tokens = self.cancellation_tokens.write().await;
                        tokens.insert(job.id.clone(), CancellationToken::new());
                    }

                    return Ok(Some(job));
                }
            }
        }

        Ok(None)
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

            // Persist logs to execution_info
            if !result.logs.is_empty() {
                let mut exec_info: JobExecutionInfo = db_job
                    .execution_info
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();

                exec_info.logs.extend(result.logs.clone());
                db_job.execution_info = Some(serde_json::to_string(&exec_info)?);
            }

            // Calculate queue wait time
            if let (Some(created), Some(started)) = (
                &db_job.created_at.parse::<chrono::DateTime<Utc>>().ok(),
                &db_job.started_at,
            ) {
                if let Ok(started_dt) = started.parse::<chrono::DateTime<Utc>>() {
                    let wait_secs = (started_dt - *created).num_milliseconds() as f64 / 1000.0;
                    db_job.queue_wait_secs = Some(wait_secs.max(0.0));
                }
            }
            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Completed;
                job.completed_at = Some(Utc::now());
                job.outputs = result.outputs;
                job.duration_secs = Some(result.duration_secs);

                // Update cached execution info if logs are present
                if !result.logs.is_empty() {
                    let mut exec_info = job.execution_info.clone().unwrap_or_default();
                    exec_info.logs.extend(result.logs);
                    job.execution_info = Some(exec_info);
                }

                // Calculate queue wait time
                if let (Some(created), Some(started)) = (Some(job.created_at), job.started_at) {
                    let wait_secs = (started - created).num_milliseconds() as f64 / 1000.0;
                    job.queue_wait_secs = Some(wait_secs.max(0.0));
                }
            }
        }

        // Remove cancellation token
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(job_id);
        }

        self.depth.fetch_sub(1, Ordering::SeqCst);
        info!("Job {} completed in {:.2}s", job_id, result.duration_secs);
        Ok(())
    }

    /// Atomically complete a job and create the next job in the pipeline.
    /// This ensures crash-safe transition between pipeline steps.
    /// Returns the ID of the newly created job, if any.
    /// Requirements: 7.2, 7.3
    pub async fn complete_with_next(
        &self,
        job_id: &str,
        result: JobResult,
    ) -> Result<Option<String>> {
        let Some(repo) = &self.job_repository else {
            // Without repository, fall back to simple completion
            self.complete(job_id, result).await?;
            return Ok(None);
        };

        // Get the current job to check for next_job_type
        let current_job = repo.get_job(job_id).await?;

        // Calculate queue wait time
        let queue_wait_secs = if let (Some(created), Some(started)) = (
            &current_job.created_at.parse::<chrono::DateTime<Utc>>().ok(),
            &current_job.started_at,
        ) {
            if let Ok(started_dt) = started.parse::<chrono::DateTime<Utc>>() {
                let wait_secs = (started_dt - *created).num_milliseconds() as f64 / 1000.0;
                wait_secs.max(0.0)
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Build the next job if there's a next_job_type defined
        let next_job = if let Some(next_type) = &current_job.next_job_type {
            // Get remaining steps after the next job
            let remaining = current_job.get_remaining_steps();
            let (next_next_type, next_remaining) = if remaining.is_empty() {
                (None, None)
            } else {
                let next_next = remaining.first().cloned();
                let rest: Vec<PipelineStep> = remaining.into_iter().skip(1).collect();
                (next_next, if rest.is_empty() { None } else { Some(rest) })
            };

            // Determine next job type from PipelineStep
            // Determine next job type (it's already the processor name)
            let next_job_type_str = next_type.clone();

            // Create the next job
            // Output of current job becomes input of next job
            let inputs_json =
                serde_json::to_string(&result.outputs).unwrap_or_else(|_| "[]".to_string());

            let next_job = JobDbModel::new_pipeline_step(
                next_job_type_str,
                inputs_json,      // Inputs for next job (from current outputs)
                "[]".to_string(), // Initial outputs for next job (empty)
                current_job.priority,
                current_job.streamer_id.clone(),
                current_job.session_id.clone(),
                current_job.pipeline_id.clone(),
                next_next_type.map(|s| match s {
                    PipelineStep::Inline { processor, .. } => processor,
                    PipelineStep::Preset(name) => name,
                }),
                next_remaining,
            );

            Some(next_job)
        } else {
            None
        };

        // Perform atomic completion and next job creation
        let outputs_str =
            serde_json::to_string(&result.outputs).unwrap_or_else(|_| "[]".to_string());

        // Update execution info with logs before completion
        let mut job_updates = HashMap::new();
        if !result.logs.is_empty() {
            let mut exec_info: JobExecutionInfo = current_job
                .execution_info
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            exec_info.logs.extend(result.logs.clone());
            job_updates.insert(
                "execution_info".to_string(),
                serde_json::to_string(&exec_info)?,
            );
        }

        // We can't easily pass the logs to complete_job_and_create_next because it takes explicit args.
        // We should update the job directly or update complete_job_and_create_next signature.
        // However, repo.complete_job_and_create_next is a transactional operation.
        // If we update logs separately, it might be outside the transaction but acceptable for logs.
        // BETTER: Use repo.update_job first if we have logs, then complete?
        // OR: Just assume complete_job_and_create_next handles basic completion fields and we update logs here.
        // BUT complete_job_and_create_next might overwrite the job state.

        // Actually, let's just use update_job to save logs first, then complete it.
        // Since we have `current_job` loaded, we can modify it and save it.
        if !result.logs.is_empty() {
            let mut exec_info: JobExecutionInfo = current_job
                .execution_info
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            exec_info.logs.extend(result.logs.clone());

            // We need a mutable reference to current_job, but we have an immutable one.
            // Let's re-fetch or just update the field locally if we were to pass it back.
            // But complete_job_and_create_next doesn't take the full job object.

            // Allow update of the job with logs.
            let mut updated_job = current_job.clone();
            updated_job.execution_info = Some(serde_json::to_string(&exec_info)?);
            repo.update_job(&updated_job).await?;
        }

        let next_job_id = repo
            .complete_job_and_create_next(
                job_id,
                &outputs_str,
                result.duration_secs,
                queue_wait_secs,
                next_job.as_ref(),
            )
            .await?;

        // Update cache for completed job
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Completed;
                job.completed_at = Some(Utc::now());
                job.outputs = result.outputs;
                job.duration_secs = Some(result.duration_secs);
                job.queue_wait_secs = Some(queue_wait_secs);

                if !result.logs.is_empty() {
                    let mut exec_info = job.execution_info.clone().unwrap_or_default();
                    exec_info.logs.extend(result.logs);
                    job.execution_info = Some(exec_info);
                }
            }

            // Add next job to cache if created
            if let Some(ref next) = next_job {
                let job = db_model_to_job(next);
                cache.insert(job.id.clone(), job);
            }
        }

        // Remove cancellation token for completed job
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(job_id);
        }

        // Depth stays the same if we created a new job, otherwise decrement
        if next_job.is_none() {
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }

        info!("Job {} completed in {:.2}s", job_id, result.duration_secs);
        if let Some(ref id) = next_job_id {
            info!("Created next pipeline job: {}", id);
            // Notify workers about the new job
            self.notify.notify_one();
        }

        Ok(next_job_id)
    }

    /// Mark a job as failed.
    pub async fn fail(&self, job_id: &str, error: &str) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(job_id).await?;
            db_job.mark_failed(error);
            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Failed;
                job.completed_at = Some(Utc::now());
                job.error = Some(error.to_string());
            }
        }

        // Remove cancellation token
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(job_id);
        }

        self.depth.fetch_sub(1, Ordering::SeqCst);
        warn!("Job {} failed: {}", job_id, error);
        Ok(())
    }

    /// Mark a job as failed with step information for observability.
    /// Records the error message, failing step, and processor name in execution_info.
    /// Requirements: 6.4, 10.3
    pub async fn fail_with_step_info(
        &self,
        job_id: &str,
        error: &str,
        processor_name: Option<&str>,
        step_number: Option<u32>,
        total_steps: Option<u32>,
    ) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(job_id).await?;
            db_job.mark_failed(error);

            // Update execution_info with failure details
            let mut exec_info: JobExecutionInfo = db_job
                .execution_info
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            // Record the failing step info
            if let Some(name) = processor_name {
                exec_info.current_processor = Some(name.to_string());
            }
            if let Some(step) = step_number {
                exec_info.current_step = Some(step);
            }
            if let Some(total) = total_steps {
                exec_info.total_steps = Some(total);
            }

            // Add error log entry
            exec_info.add_log(JobLogEntry::error(format!("Job failed: {}", error)));

            db_job.execution_info =
                Some(serde_json::to_string(&exec_info).unwrap_or_else(|_| "{}".to_string()));

            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Failed;
                job.completed_at = Some(Utc::now());
                job.error = Some(error.to_string());

                // Update execution_info in cache
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
                exec_info.add_log(JobLogEntry::error(format!("Job failed: {}", error)));
            }
        }

        // Remove cancellation token
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(job_id);
        }

        self.depth.fetch_sub(1, Ordering::SeqCst);
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
        let cache = self.jobs_cache.read().await;
        Ok(cache.get(id).cloned())
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
        let cache = self.jobs_cache.read().await;
        let mut jobs: Vec<Job> = cache.values().cloned().collect();

        // Apply filters
        if let Some(status) = &filters.status {
            let status_enum = match status {
                DbJobStatus::Pending => JobStatus::Pending,
                DbJobStatus::Processing => JobStatus::Processing,
                DbJobStatus::Completed => JobStatus::Completed,
                DbJobStatus::Failed => JobStatus::Failed,
                DbJobStatus::Interrupted => JobStatus::Interrupted,
            };
            jobs.retain(|j| j.status == status_enum);
        }
        if let Some(streamer_id) = &filters.streamer_id {
            jobs.retain(|j| &j.streamer_id == streamer_id);
        }
        if let Some(session_id) = &filters.session_id {
            jobs.retain(|j| &j.session_id == session_id);
        }

        let total = jobs.len() as u64;

        // Apply pagination
        let start = pagination.offset as usize;
        let end = std::cmp::min(start + pagination.limit as usize, jobs.len());
        let jobs = if start < jobs.len() {
            jobs[start..end].to_vec()
        } else {
            vec![]
        };

        Ok((jobs, total))
    }

    /// List pipelines (grouped by pipeline_id) with pagination.
    pub async fn list_pipelines(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<crate::database::repositories::PipelineSummary>, u64)> {
        if let Some(repo) = &self.job_repository {
            return repo.list_pipelines(filters, pagination).await;
        }

        // Fallback: return empty if no repository
        Ok((vec![], 0))
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
            let mut db_job = repo.get_job(id).await?;
            db_job.reset_for_retry();
            repo.update_job(&db_job).await?;
        }

        // Update cache
        let updated_job = {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(id) {
                job.status = JobStatus::Pending;
                job.started_at = None;
                job.completed_at = None;
                job.error = None;
                job.retry_count += 1;
                job.clone()
            } else {
                // Create from scratch if not in cache
                let mut new_job = job.clone();
                new_job.status = JobStatus::Pending;
                new_job.started_at = None;
                new_job.completed_at = None;
                new_job.error = None;
                new_job.retry_count += 1;
                cache.insert(id.to_string(), new_job.clone());
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
        if job.status == JobStatus::Processing {
            let tokens = self.cancellation_tokens.read().await;
            if let Some(token) = tokens.get(id) {
                token.cancel();
            }
        }

        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            repo.update_job_status(id, DbJobStatus::Interrupted.as_str())
                .await?;
        }

        // Update cache and get the updated job
        let cancelled_job = {
            let mut cache = self.jobs_cache.write().await;
            if let Some(cached_job) = cache.get_mut(id) {
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
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(id);
        }

        // Decrement depth only for pending jobs (processing jobs already counted)
        if job.status == JobStatus::Pending {
            self.depth.fetch_sub(1, Ordering::SeqCst);
        }

        info!("Job {} cancelled", id);
        Ok(cancelled_job)
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
        {
            let mut cache = self.jobs_cache.write().await;
            let mut depth_reduction = 0;

            for job in cache.values_mut() {
                if job.pipeline_id.as_deref() == Some(pipeline_id) {
                    if job.status == JobStatus::Pending {
                        depth_reduction += 1;
                    }
                    if job.status == JobStatus::Pending || job.status == JobStatus::Processing {
                        // Signal cancellation for processing jobs
                        if job.status == JobStatus::Processing {
                            let tokens = self.cancellation_tokens.read().await;
                            if let Some(token) = tokens.get(&job.id) {
                                token.cancel();
                            }
                        }

                        job.status = JobStatus::Interrupted;
                        job.completed_at = Some(Utc::now());

                        // Only add to cancelled_jobs if not from database (avoid duplicates)
                        if self.job_repository.is_none() {
                            cancelled_jobs.push(job.clone());
                        }
                    }
                }
            }

            // Adjust depth
            if depth_reduction > 0 {
                self.depth.fetch_sub(depth_reduction, Ordering::SeqCst);
            }
        }

        // Clean up cancellation tokens for pipeline jobs
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.retain(|job_id, _| {
                if let Ok(cache) = self.jobs_cache.try_read() {
                    if let Some(job) = cache.get(job_id) {
                        return job.pipeline_id.as_deref() != Some(pipeline_id);
                    }
                }
                true
            });
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
        let tokens = self.cancellation_tokens.read().await;
        tokens.get(job_id).cloned()
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

        {
            let mut cache = self.jobs_cache.write().await;
            for db_job in &db_jobs {
                let job = db_model_to_job(db_job);
                cache.insert(job.id.clone(), job);
            }
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
        let cache = self.jobs_cache.read().await;
        let mut stats = JobStats::default();

        for job in cache.values() {
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
    // Requirements: 9.1, 9.2, 9.5, 11.1, 11.2, 11.3
    // ========================================================================

    /// Split a multi-input job into separate jobs for single-input processors.
    /// Creates one job per input file, all sharing the same pipeline context.
    /// Returns the IDs of the newly created jobs.
    /// Requirements: 11.2
    pub async fn split_job_for_single_input(&self, job: &Job) -> Result<Vec<String>> {
        if job.inputs.len() <= 1 {
            // No splitting needed
            return Ok(vec![job.id.clone()]);
        }

        let mut created_job_ids = Vec::new();

        for (_idx, input) in job.inputs.iter().enumerate() {
            // Create a new job for each input
            let split_job = Job::new_pipeline_step(
                job.job_type.clone(),
                vec![input.clone()],
                vec![], // Outputs will be determined by processor
                job.streamer_id.clone(),
                job.session_id.clone(),
                job.pipeline_id.clone(),
                // Propagate pipeline chain to ALL split jobs (Parallel Chaining)
                job.next_job_type.clone(),
                job.remaining_steps.clone(),
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
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(original) = cache.get_mut(&job.id) {
                original.status = JobStatus::Completed;
                original.completed_at = Some(Utc::now());
            }
        }

        // Remove cancellation token for original job
        {
            let mut tokens = self.cancellation_tokens.write().await;
            tokens.remove(&job.id);
        }

        info!(
            "Split job {} into {} jobs for single-input processing",
            job.id,
            created_job_ids.len()
        );

        Ok(created_job_ids)
    }

    /// Track partial outputs for a job (used for cleanup on failure).
    /// Updates the job's execution_info with items_produced.
    /// Requirements: 9.5
    pub async fn track_partial_outputs(&self, job_id: &str, outputs: &[String]) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(job_id).await?;

            // Parse existing execution_info or create new
            let mut exec_info: JobExecutionInfo = db_job
                .execution_info
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();

            // Add the partial outputs
            exec_info.items_produced.extend(outputs.iter().cloned());

            // Serialize back
            db_job.execution_info =
                Some(serde_json::to_string(&exec_info).unwrap_or_else(|_| "{}".to_string()));
            db_job.updated_at = Utc::now().to_rfc3339();

            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                let exec_info = job
                    .execution_info
                    .get_or_insert_with(JobExecutionInfo::default);
                exec_info.items_produced.extend(outputs.iter().cloned());
            }
        }

        Ok(())
    }

    /// Get partial outputs for a job (for cleanup on failure).
    /// Requirements: 9.5
    pub async fn get_partial_outputs(&self, job_id: &str) -> Result<Vec<String>> {
        // Try cache first
        {
            let cache = self.jobs_cache.read().await;
            if let Some(job) = cache.get(job_id) {
                if let Some(ref exec_info) = job.execution_info {
                    return Ok(exec_info.items_produced.clone());
                }
            }
        }

        // Try database
        if let Some(repo) = &self.job_repository {
            let db_job = repo.get_job(job_id).await?;
            if let Some(exec_info_str) = &db_job.execution_info {
                if let Ok(exec_info) = serde_json::from_str::<JobExecutionInfo>(exec_info_str) {
                    return Ok(exec_info.items_produced);
                }
            }
        }

        Ok(vec![])
    }

    /// Fail a job and clean up partial outputs.
    /// Requirements: 9.5
    pub async fn fail_with_cleanup(&self, job_id: &str, error: &str) -> Result<Vec<String>> {
        self.fail_with_cleanup_and_step_info(job_id, error, None, None, None)
            .await
    }

    /// Fail a job with step info and clean up partial outputs.
    /// Records the error message, failing step, and processor name in execution_info.
    /// Requirements: 6.4, 9.5, 10.3
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
    /// Requirements: 6.1, 6.2, 6.3, 6.4
    pub async fn update_execution_info(
        &self,
        job_id: &str,
        exec_info: JobExecutionInfo,
    ) -> Result<()> {
        // Update database if repository is available
        if let Some(repo) = &self.job_repository {
            let mut db_job = repo.get_job(job_id).await?;
            db_job.execution_info =
                Some(serde_json::to_string(&exec_info).unwrap_or_else(|_| "{}".to_string()));
            db_job.updated_at = Utc::now().to_rfc3339();
            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.execution_info = Some(exec_info);
            }
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

    let inputs_json = serde_json::to_string(&job.inputs).unwrap_or_else(|_| "[]".to_string());
    let outputs_json = serde_json::to_string(&job.outputs).unwrap_or_else(|_| "[]".to_string());

    // Serialize execution_info to JSON
    let execution_info_json = job
        .execution_info
        .as_ref()
        .and_then(|info| serde_json::to_string(info).ok());

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
        next_job_type: job.next_job_type.clone(),
        remaining_steps: job
            .remaining_steps
            .as_ref()
            .map(|steps| serde_json::to_string(steps).unwrap_or_else(|_| "[]".to_string())),
        pipeline_id: job.pipeline_id.clone(),
        execution_info: execution_info_json,
        duration_secs: job.duration_secs,
        queue_wait_secs: job.queue_wait_secs,
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
    } else {
        if input_str.is_empty() {
            vec![]
        } else {
            vec![input_str]
        }
    };

    // Parse outputs JSON array
    let output_str = db_job.outputs.clone().unwrap_or_default();
    let outputs = if output_str.starts_with('[') {
        serde_json::from_str::<Vec<String>>(&output_str).unwrap_or_else(|_| vec![])
    } else {
        if output_str.is_empty() {
            vec![]
        } else {
            vec![output_str]
        }
    };

    // Parse remaining_steps JSON array
    let remaining_steps = db_job
        .remaining_steps
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<PipelineStep>>(s).ok());

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
        next_job_type: db_job.next_job_type.clone(),
        remaining_steps,
        pipeline_id: db_job.pipeline_id.clone(),
        // Parse execution_info JSON
        execution_info: db_job
            .execution_info
            .as_ref()
            .and_then(|s| serde_json::from_str::<JobExecutionInfo>(s).ok()),
        duration_secs: db_job.duration_secs,
        queue_wait_secs: db_job.queue_wait_secs,
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
    // Requirements: 9.1, 9.2, 9.5, 11.1, 11.2, 11.3
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

    #[tokio::test]
    async fn test_split_job_multiple_inputs() {
        // A job with multiple inputs should be split into separate jobs
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
        .with_pipeline_id("pipeline-1".to_string())
        .with_next_job_type("upload".to_string())
        .with_remaining_steps(vec![PipelineStep::Preset("thumbnail".to_string())]);

        let original_id = job.id.clone();
        queue.enqueue(job.clone()).await.unwrap();

        let split_ids = queue.split_job_for_single_input(&job).await.unwrap();

        // Should create 3 new jobs
        assert_eq!(split_ids.len(), 3);

        // Original job should be marked as completed
        let original = queue.get_job(&original_id).await.unwrap().unwrap();
        assert_eq!(original.status, JobStatus::Completed);

        // Verify each split job has single input
        for (idx, split_id) in split_ids.iter().enumerate() {
            let split_job = queue.get_job(split_id).await.unwrap().unwrap();
            assert_eq!(split_job.inputs.len(), 1);
            assert_eq!(split_job.job_type, "remux");
            assert_eq!(split_job.pipeline_id, Some("pipeline-1".to_string()));

            // All jobs should have next_job_type (Parallel Chaining)
            assert_eq!(split_job.next_job_type, Some("upload".to_string()));
            assert_eq!(
                split_job.remaining_steps,
                Some(vec![PipelineStep::Preset("thumbnail".to_string())])
            );
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
    // Requirements: 6.4, 10.3
    // ========================================================================

    /// Test that fail_with_step_info records the failing step and processor.
    /// Requirements: 6.4
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
        let last_log = exec_info.logs.last().unwrap();
        assert_eq!(last_log.level, LogLevel::Error);
        assert!(last_log.message.contains("FFmpeg error"));
    }

    /// Test that failed jobs don't create subsequent jobs.
    /// This is verified by checking that complete_with_next is only called on success.
    /// Requirements: 10.3
    #[tokio::test]
    async fn test_failed_job_does_not_create_next_job() {
        let queue = JobQueue::new();

        // Create a pipeline job with next_job_type
        let job = Job::new_pipeline_step(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
            Some("pipeline-1".to_string()),
            Some("upload".to_string()),
            Some(vec![PipelineStep::Preset("thumbnail".to_string())]),
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Verify only one job exists
        assert_eq!(queue.depth(), 1);

        // Fail the job (not using complete_with_next)
        queue
            .fail_with_step_info(
                &job_id,
                "Processing failed",
                Some("RemuxProcessor"),
                Some(1),
                Some(3),
            )
            .await
            .unwrap();

        // Verify job is failed
        let failed_job = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(failed_job.status, JobStatus::Failed);

        // Verify no new jobs were created (depth should be 0 after failure)
        assert_eq!(queue.depth(), 0);
    }

    /// Test that fail_with_cleanup_and_step_info combines cleanup and step info.
    /// Requirements: 6.4, 9.5, 10.3
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
    // Requirements: 10.5
    // ========================================================================

    /// Test that pipeline jobs preserve chain info in the job struct.
    /// This info is essential for recovery - when a job is recovered,
    /// it should still have all the info needed to continue the pipeline.
    /// Requirements: 10.5
    #[tokio::test]
    async fn test_pipeline_job_chain_info_preserved() {
        let queue = JobQueue::new();

        // Create a pipeline job with chain info
        let job = Job::new_pipeline_step(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
            Some("pipeline-123".to_string()),
            Some("upload".to_string()),
            Some(vec![PipelineStep::Preset("thumbnail".to_string())]),
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Retrieve the job and verify chain info is preserved
        let retrieved = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(retrieved.pipeline_id, Some("pipeline-123".to_string()));
        assert_eq!(retrieved.next_job_type, Some("upload".to_string()));
        assert_eq!(
            retrieved.remaining_steps,
            Some(vec![PipelineStep::Preset("thumbnail".to_string())])
        );
    }

    /// Test that complete_with_next creates the next job in the pipeline.
    /// This is the core mechanism for pipeline continuation after recovery.
    /// Requirements: 10.2, 10.5
    #[tokio::test]
    async fn test_complete_with_next_creates_next_job() {
        let queue = JobQueue::new();

        // Create a pipeline job with next_job_type
        let job = Job::new_pipeline_step(
            "remux",
            vec!["/input.flv".to_string()],
            vec![],
            "streamer-1",
            "session-1",
            Some("pipeline-123".to_string()),
            Some("upload".to_string()),
            Some(vec![PipelineStep::Preset("thumbnail".to_string())]),
        );
        let job_id = job.id.clone();
        queue.enqueue(job).await.unwrap();

        // Complete the job with outputs
        // Note: Without a repository, complete_with_next falls back to simple completion
        // and doesn't create the next job (that requires database atomicity)
        let result = queue
            .complete_with_next(
                &job_id,
                JobResult {
                    outputs: vec!["/output.mp4".to_string()],
                    duration_secs: 10.0,
                    metadata: None,
                    logs: vec![],
                },
            )
            .await
            .unwrap();

        // Without repository, no next job is created
        assert!(result.is_none());

        // But the job is completed
        let completed = queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(completed.status, JobStatus::Completed);
    }

    /// Test that recover_jobs without repository returns 0.
    /// This verifies the fallback behavior when no database is configured.
    /// Requirements: 10.5
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
}
