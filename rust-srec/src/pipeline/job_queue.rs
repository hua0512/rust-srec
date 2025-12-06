//! Database-backed job queue implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

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

/// A job in the queue.
#[derive(Debug, Clone)]
pub struct Job {
    /// Unique job ID.
    pub id: String,
    /// Job type (e.g., "remux", "upload", "thumbnail").
    pub job_type: String,
    /// Input file path or data.
    pub input: String,
    /// Output file path or destination.
    pub output: String,
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
    pub remaining_steps: Option<Vec<String>>,
    /// Pipeline ID to group related jobs (first job's ID).
    pub pipeline_id: Option<String>,
}

impl Job {
    /// Create a new job.
    pub fn new(
        job_type: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            input: input.into(),
            output: output.into(),
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
        }
    }

    /// Create a new pipeline step job with chain information.
    pub fn new_pipeline_step(
        job_type: impl Into<String>,
        input: impl Into<String>,
        output: impl Into<String>,
        streamer_id: impl Into<String>,
        session_id: impl Into<String>,
        pipeline_id: Option<String>,
        next_job_type: Option<String>,
        remaining_steps: Option<Vec<String>>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            job_type: job_type.into(),
            input: input.into(),
            output: output.into(),
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
    pub fn with_remaining_steps(mut self, steps: Vec<String>) -> Self {
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
    /// Output path or result data.
    pub output: String,
    /// Duration of processing in seconds.
    pub duration_secs: f64,
    /// Additional metadata.
    pub metadata: Option<String>,
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
    pub async fn dequeue(&self, job_type: Option<&str>) -> Result<Option<Job>> {
        debug!("Attempting to dequeue job of type {:?}", job_type);

        // Try to get from database if repository is available
        if let Some(repo) = &self.job_repository {
            let filters = JobFilters {
                status: Some(DbJobStatus::Pending),
                job_type: job_type.map(|s| s.to_string()),
                ..Default::default()
            };
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
                    if let Some(jt) = job_type {
                        if job.job_type != jt {
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
            if !result.output.is_empty() {
                db_job.add_output(&result.output);
            }
            repo.update_job(&db_job).await?;
        }

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Completed;
                job.completed_at = Some(Utc::now());
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

        // Build the next job if there's a next_job_type defined
        let next_job = if let Some(next_type) = &current_job.next_job_type {
            // Get remaining steps after the next job
            let remaining = current_job.get_remaining_steps();
            let (next_next_type, next_remaining) = if remaining.is_empty() {
                (None, None)
            } else {
                let next_next = remaining.first().cloned();
                let rest: Vec<String> = remaining.into_iter().skip(1).collect();
                (next_next, if rest.is_empty() { None } else { Some(rest) })
            };

            // Create the next job
            // Output of current job becomes input of next job
            let next_job = JobDbModel::new_pipeline_step(
                next_type.clone(),
                result.output.clone(), // Output of current becomes input of next
                result.output.clone(), // Keep same output path pattern for now
                current_job.priority,
                current_job.streamer_id.clone(),
                current_job.session_id.clone(),
                current_job.pipeline_id.clone(),
                next_next_type,
                next_remaining,
            );

            Some(next_job)
        } else {
            None
        };

        // Perform atomic completion and next job creation
        let next_job_id = repo
            .complete_job_and_create_next(job_id, &result.output, next_job.as_ref())
            .await?;

        // Update cache for completed job
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(job_id) {
                job.status = JobStatus::Completed;
                job.completed_at = Some(Utc::now());
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
    /// Returns error for Completed/Failed jobs.
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
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

        // Update cache
        {
            let mut cache = self.jobs_cache.write().await;
            if let Some(job) = cache.get_mut(id) {
                job.status = JobStatus::Interrupted;
                job.completed_at = Some(Utc::now());
            }
        }

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
        Ok(())
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

    JobDbModel {
        id: job.id.clone(),
        job_type: job.job_type.clone(),
        status: status.as_str().to_string(),
        config: job.config.clone().unwrap_or_else(|| "{}".to_string()),
        state: "{}".to_string(),
        created_at: job.created_at.to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
        input: Some(job.input.clone()),
        outputs: Some(format!("[\"{}\"]", job.output)),
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

    // Parse outputs JSON array to get the first output
    let output = db_job
        .outputs
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
        .and_then(|v| v.into_iter().next())
        .unwrap_or_default();

    // Parse remaining_steps JSON array
    let remaining_steps = db_job
        .remaining_steps
        .as_ref()
        .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok());

    Job {
        id: db_job.id.clone(),
        job_type: db_job.job_type.clone(),
        input: db_job.input.clone().unwrap_or_default(),
        output,
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
            "/input.flv",
            "/output.mp4",
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

        let job = Job::new("test", "input", "output", "streamer", "session");
        let job_id = queue.enqueue(job).await.unwrap();

        assert!(!job_id.is_empty());
        assert_eq!(queue.depth(), 1);
    }
}
