//! Pipeline Manager implementation.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::job_queue::{Job, JobQueue, JobQueueConfig, QueueDepthStatus};
use super::processors::{
    ExecuteCommandProcessor, Processor, RemuxProcessor, ThumbnailProcessor, UploadProcessor,
};
use super::worker_pool::{WorkerPool, WorkerPoolConfig, WorkerType};
use crate::database::models::{JobFilters, Pagination};
use crate::database::repositories::JobRepository;
use crate::downloader::DownloadManagerEvent;
use crate::Result;

/// Configuration for the Pipeline Manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineManagerConfig {
    /// Job queue configuration.
    pub job_queue: JobQueueConfig,
    /// CPU worker pool configuration.
    pub cpu_pool: WorkerPoolConfig,
    /// IO worker pool configuration.
    pub io_pool: WorkerPoolConfig,
    /// Whether to enable download throttling on backpressure.
    pub enable_throttling: bool,
}

impl Default for PipelineManagerConfig {
    fn default() -> Self {
        Self {
            job_queue: JobQueueConfig::default(),
            cpu_pool: WorkerPoolConfig {
                max_workers: 2,
                ..Default::default()
            },
            io_pool: WorkerPoolConfig {
                max_workers: 4,
                ..Default::default()
            },
            enable_throttling: false,
        }
    }
}

/// Events emitted by the Pipeline Manager.
#[derive(Debug, Clone)]
pub enum PipelineEvent {
    /// Job enqueued.
    JobEnqueued {
        job_id: String,
        job_type: String,
        streamer_id: String,
    },
    /// Job started processing.
    JobStarted { job_id: String, job_type: String },
    /// Job completed successfully.
    JobCompleted {
        job_id: String,
        job_type: String,
        duration_secs: f64,
    },
    /// Job failed.
    JobFailed {
        job_id: String,
        job_type: String,
        error: String,
    },
    /// Queue depth warning.
    QueueWarning { depth: usize },
    /// Queue depth critical.
    QueueCritical { depth: usize },
}

/// The Pipeline Manager service.
pub struct PipelineManager {
    /// Configuration.
    config: PipelineManagerConfig,
    /// Job queue.
    job_queue: Arc<JobQueue>,
    /// CPU worker pool.
    cpu_pool: WorkerPool,
    /// IO worker pool.
    io_pool: WorkerPool,
    /// Processors.
    processors: Vec<Arc<dyn Processor>>,
    /// Event broadcaster.
    event_tx: broadcast::Sender<PipelineEvent>,
    /// Cancellation token.
    cancellation_token: CancellationToken,
}

impl PipelineManager {
    /// Create a new Pipeline Manager.
    pub fn new() -> Self {
        Self::with_config(PipelineManagerConfig::default())
    }

    /// Create a new Pipeline Manager with custom configuration.
    pub fn with_config(config: PipelineManagerConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_config(config.job_queue.clone()));

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(UploadProcessor::new()),
            Arc::new(ExecuteCommandProcessor::new()),
            Arc::new(ThumbnailProcessor::new()),
        ];

        Self {
            cpu_pool: WorkerPool::with_config(WorkerType::Cpu, config.cpu_pool.clone()),
            io_pool: WorkerPool::with_config(WorkerType::Io, config.io_pool.clone()),
            config,
            job_queue,
            processors,
            event_tx,
            cancellation_token: CancellationToken::new(),
        }
    }

    /// Create a new Pipeline Manager with custom configuration and job repository.
    /// This enables database persistence and job recovery on startup.
    /// Requirements: 6.1, 6.3
    pub fn with_repository(config: PipelineManagerConfig, repository: Arc<dyn JobRepository>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let job_queue = Arc::new(JobQueue::with_repository(config.job_queue.clone(), repository));

        // Create default processors
        let processors: Vec<Arc<dyn Processor>> = vec![
            Arc::new(RemuxProcessor::new()),
            Arc::new(UploadProcessor::new()),
            Arc::new(ExecuteCommandProcessor::new()),
            Arc::new(ThumbnailProcessor::new()),
        ];

        Self {
            cpu_pool: WorkerPool::with_config(WorkerType::Cpu, config.cpu_pool.clone()),
            io_pool: WorkerPool::with_config(WorkerType::Io, config.io_pool.clone()),
            config,
            job_queue,
            processors,
            event_tx,
            cancellation_token: CancellationToken::new(),
        }
    }

    /// Set the job repository for database persistence.
    /// This enables job recovery on startup and persistent job tracking.
    /// Requirements: 6.1, 6.3
    /// 
    /// Note: This method is deprecated. Use `with_repository` constructor instead
    /// for full functionality. Setting the repository after construction has
    /// limited effect due to the immutable nature of the internal JobQueue.
    #[deprecated(note = "Use with_repository constructor instead")]
    pub fn set_job_repository(&self, _repository: Arc<dyn JobRepository>) {
        // Note: In a production system, you'd want to either:
        // 1. Pass the repository at construction time (preferred - use with_repository)
        // 2. Use Arc<RwLock<JobQueue>> for interior mutability
        // 3. Use a different pattern for dependency injection
        
        // For now, we log a warning that this should be done at construction time
        warn!("set_job_repository called after construction - use with_repository constructor for full functionality");
    }

    /// Recover jobs from database on startup.
    /// Resets PROCESSING jobs to PENDING for re-execution.
    /// For sequential pipelines, no special handling is needed since only one job
    /// per pipeline exists at a time.
    /// Requirements: 6.3, 7.4
    pub async fn recover_jobs(&self) -> Result<usize> {
        info!("Recovering jobs from database...");
        let recovered = self.job_queue.recover_jobs().await?;
        if recovered > 0 {
            info!("Recovered {} jobs from database", recovered);
        } else {
            debug!("No jobs to recover from database");
        }
        Ok(recovered)
    }

    /// Start the pipeline manager.
    pub fn start(&self) {
        info!("Starting Pipeline Manager");

        // Get CPU and IO processors
        let cpu_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == super::processors::ProcessorType::Cpu)
            .cloned()
            .collect();

        let io_processors: Vec<Arc<dyn Processor>> = self
            .processors
            .iter()
            .filter(|p| p.processor_type() == super::processors::ProcessorType::Io)
            .cloned()
            .collect();

        // Start worker pools
        self.cpu_pool.start(self.job_queue.clone(), cpu_processors);
        self.io_pool.start(self.job_queue.clone(), io_processors);

        info!("Pipeline Manager started");
    }

    /// Stop the pipeline manager.
    pub async fn stop(&self) {
        info!("Stopping Pipeline Manager");
        self.cancellation_token.cancel();

        // Stop worker pools
        self.cpu_pool.stop().await;
        self.io_pool.stop().await;

        info!("Pipeline Manager stopped");
    }

    /// Subscribe to pipeline events.
    pub fn subscribe(&self) -> broadcast::Receiver<PipelineEvent> {
        self.event_tx.subscribe()
    }

    /// Enqueue a job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();
        let streamer_id = job.streamer_id.clone();

        self.job_queue.enqueue(job).await?;

        // Emit event
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job_id.clone(),
            job_type,
            streamer_id,
        });

        // Check queue depth
        self.check_queue_depth();

        Ok(job_id)
    }

    /// Create a remux job for a downloaded segment.
    pub async fn create_remux_job(
        &self,
        input_path: &str,
        output_path: &str,
        streamer_id: &str,
        session_id: &str,
    ) -> Result<String> {
        let job = Job::new("remux", input_path, output_path, streamer_id, session_id);
        self.enqueue(job).await
    }

    /// Create an upload job.
    pub async fn create_upload_job(
        &self,
        input_path: &str,
        destination: &str,
        streamer_id: &str,
        session_id: &str,
    ) -> Result<String> {
        let job = Job::new("upload", input_path, destination, streamer_id, session_id);
        self.enqueue(job).await
    }

    /// Create a thumbnail job.
    pub async fn create_thumbnail_job(
        &self,
        input_path: &str,
        output_path: &str,
        streamer_id: &str,
        session_id: &str,
        config: Option<&str>,
    ) -> Result<String> {
        let mut job = Job::new(
            "thumbnail",
            input_path,
            output_path,
            streamer_id,
            session_id,
        );
        if let Some(cfg) = config {
            job = job.with_config(cfg);
        }
        self.enqueue(job).await
    }

    /// Create a new pipeline with sequential job execution.
    /// Only the first job is created immediately; subsequent jobs are created
    /// atomically when each job completes.
    /// 
    /// Returns the pipeline_id (which is the first job's ID) for tracking.
    /// 
    /// Requirements: 6.1, 7.1, 7.5
    pub async fn create_pipeline(
        &self,
        session_id: &str,
        streamer_id: &str,
        input_path: &str,
        steps: Option<Vec<String>>,
    ) -> Result<PipelineCreationResult> {
        // Use default steps if not provided
        let steps = steps.unwrap_or_else(|| {
            vec![
                "remux".to_string(),
                "upload".to_string(),
                "thumbnail".to_string(),
            ]
        });

        if steps.is_empty() {
            return Err(crate::Error::Validation("Pipeline must have at least one step".to_string()));
        }

        // Get the first step
        let first_step = &steps[0];

        // Calculate next_job_type and remaining_steps for the first job
        let (next_job_type, remaining_steps) = if steps.len() > 1 {
            let next = steps.get(1).cloned();
            let remaining: Vec<String> = steps.iter().skip(2).cloned().collect();
            (next, if remaining.is_empty() { None } else { Some(remaining) })
        } else {
            (None, None)
        };

        // Create the first job with pipeline chain information
        let first_job = Job::new_pipeline_step(
            first_step.clone(),
            input_path,
            input_path, // Output path will be determined by the processor
            streamer_id,
            session_id,
            None, // pipeline_id will be set to this job's ID
            next_job_type,
            remaining_steps,
        );

        // The pipeline_id is the first job's ID
        let pipeline_id = first_job.id.clone();

        // Set the pipeline_id on the first job
        let first_job = first_job.with_pipeline_id(pipeline_id.clone());

        // Enqueue the first job
        let job_id = self.enqueue(first_job.clone()).await?;

        info!(
            "Created pipeline {} with {} steps for session {}",
            pipeline_id,
            steps.len(),
            session_id
        );

        Ok(PipelineCreationResult {
            pipeline_id,
            first_job_id: job_id,
            first_job_type: first_step.clone(),
            total_steps: steps.len(),
            steps,
        })
    }

    /// Handle download manager events.
    pub async fn handle_download_event(&self, event: DownloadManagerEvent) {
        match event {
            DownloadManagerEvent::SegmentCompleted {
                streamer_id,
                segment_path,
                ..
            } => {
                debug!("Segment completed for {}: {}", streamer_id, segment_path);
                // Could create jobs for real-time processing here
            }
            DownloadManagerEvent::DownloadCompleted {
                streamer_id,
                session_id,
                ..
            } => {
                info!(
                    "Download completed for streamer {} session {}",
                    streamer_id, session_id
                );
                // Could create post-processing jobs here
            }
            _ => {}
        }
    }

    /// Listen for download events and create jobs.
    pub fn listen_for_downloads(&self, mut rx: mpsc::Receiver<DownloadManagerEvent>) {
        let _job_queue = self.job_queue.clone();
        let _event_tx = self.event_tx.clone();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event listener shutting down");
                        break;
                    }
                    event = rx.recv() => {
                        match event {
                            Some(DownloadManagerEvent::DownloadCompleted {
                                streamer_id,
                                session_id,
                                ..
                            }) => {
                                info!(
                                    "Creating post-processing jobs for {} / {}",
                                    streamer_id, session_id
                                );
                                // Jobs would be created based on pipeline configuration
                            }
                            Some(_) => {}
                            None => break,
                        }
                    }
                }
            }
        });
    }

    /// Check queue depth and emit warnings.
    fn check_queue_depth(&self) {
        let depth = self.job_queue.depth();
        let status = self.job_queue.depth_status();

        match status {
            QueueDepthStatus::Critical => {
                warn!("Queue depth critical: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueCritical { depth });
            }
            QueueDepthStatus::Warning => {
                warn!("Queue depth warning: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueWarning { depth });
            }
            QueueDepthStatus::Normal => {}
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.job_queue.depth()
    }

    /// Get the queue depth status.
    pub fn queue_status(&self) -> QueueDepthStatus {
        self.job_queue.depth_status()
    }

    /// Check if throttling should be enabled.
    pub fn should_throttle(&self) -> bool {
        self.config.enable_throttling && self.job_queue.is_critical()
    }

    // ========================================================================
    // Query and Management Methods (Requirements 1.1-1.5, 2.1-2.5, 3.1-3.3)
    // ========================================================================

    /// List jobs with filters and pagination.
    /// Delegates to JobQueue/JobRepository.
    /// Requirements: 1.1, 1.3, 1.4, 1.5
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        self.job_queue.list_jobs(filters, pagination).await
    }

    /// Get a job by ID.
    /// Retrieves job from repository.
    /// Requirements: 1.2
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.job_queue.get_job(id).await
    }

    /// Retry a failed job.
    /// Delegates to JobQueue.
    /// Requirements: 2.1, 2.2
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        let job = self.job_queue.retry_job(id).await?;

        // Emit event for the retried job
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            streamer_id: job.streamer_id.clone(),
        });

        // Check queue depth after retry
        self.check_queue_depth();

        Ok(job)
    }

    /// Cancel a job.
    /// For Pending jobs: removes from queue and marks as Interrupted.
    /// For Processing jobs: signals cancellation and marks as Interrupted.
    /// Returns error for Completed/Failed jobs.
    /// Delegates to JobQueue.
    /// Requirements: 2.3, 2.4, 2.5
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
        self.job_queue.cancel_job(id).await
    }

    /// Get comprehensive pipeline statistics.
    /// Returns counts by status (pending, processing, completed, failed)
    /// and average processing time.
    /// Requirements: 3.1, 3.2, 3.3
    pub async fn get_stats(&self) -> Result<PipelineStats> {
        let job_stats = self.job_queue.get_stats().await?;

        Ok(PipelineStats {
            pending: job_stats.pending,
            processing: job_stats.processing,
            completed: job_stats.completed,
            failed: job_stats.failed,
            interrupted: job_stats.interrupted,
            avg_processing_time_secs: job_stats.avg_processing_time_secs,
            queue_depth: self.queue_depth(),
            queue_status: self.queue_status(),
        })
    }
}

/// Comprehensive pipeline statistics.
/// Requirements: 3.1, 3.2, 3.3
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStats {
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
    /// Average processing time in seconds for completed jobs.
    pub avg_processing_time_secs: Option<f64>,
    /// Current queue depth.
    pub queue_depth: usize,
    /// Current queue status.
    pub queue_status: QueueDepthStatus,
}

/// Result of creating a new pipeline.
/// Requirements: 6.1, 7.1
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineCreationResult {
    /// Pipeline ID (same as first job's ID).
    pub pipeline_id: String,
    /// ID of the first job in the pipeline.
    pub first_job_id: String,
    /// Type of the first job.
    pub first_job_type: String,
    /// Total number of steps in the pipeline.
    pub total_steps: usize,
    /// List of all steps in the pipeline.
    pub steps: Vec<String>,
}

impl Default for PipelineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_manager_config_default() {
        let config = PipelineManagerConfig::default();
        assert_eq!(config.cpu_pool.max_workers, 2);
        assert_eq!(config.io_pool.max_workers, 4);
        assert!(!config.enable_throttling);
    }

    #[test]
    fn test_pipeline_manager_creation() {
        let manager = PipelineManager::new();
        assert_eq!(manager.queue_depth(), 0);
        assert_eq!(manager.queue_status(), QueueDepthStatus::Normal);
    }

    #[tokio::test]
    async fn test_enqueue_job() {
        let manager = PipelineManager::new();

        let job = Job::new(
            "remux",
            "/input.flv",
            "/output.mp4",
            "streamer-1",
            "session-1",
        );
        let job_id = manager.enqueue(job).await.unwrap();

        assert!(!job_id.is_empty());
        assert_eq!(manager.queue_depth(), 1);
    }

    #[tokio::test]
    async fn test_create_remux_job() {
        let manager = PipelineManager::new();

        let job_id = manager
            .create_remux_job("/input.flv", "/output.mp4", "streamer-1", "session-1")
            .await
            .unwrap();

        assert!(!job_id.is_empty());
    }

    #[tokio::test]
    async fn test_list_jobs() {
        use crate::database::models::{JobFilters, Pagination};

        let manager = PipelineManager::new();

        // Enqueue some jobs
        let job1 = Job::new("remux", "/input1.flv", "/output1.mp4", "streamer-1", "session-1");
        let job2 = Job::new("upload", "/input2.flv", "/output2.mp4", "streamer-2", "session-2");
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        // List all jobs
        let filters = JobFilters::default();
        let pagination = Pagination::new(10, 0);
        let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

        assert_eq!(total, 2);
        assert_eq!(jobs.len(), 2);
    }

    #[tokio::test]
    async fn test_list_jobs_with_filter() {
        use crate::database::models::{JobFilters, Pagination};

        let manager = PipelineManager::new();

        // Enqueue jobs for different streamers
        let job1 = Job::new("remux", "/input1.flv", "/output1.mp4", "streamer-1", "session-1");
        let job2 = Job::new("upload", "/input2.flv", "/output2.mp4", "streamer-2", "session-2");
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        // Filter by streamer_id
        let filters = JobFilters::new().with_streamer_id("streamer-1");
        let pagination = Pagination::new(10, 0);
        let (jobs, total) = manager.list_jobs(&filters, &pagination).await.unwrap();

        assert_eq!(total, 1);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].streamer_id, "streamer-1");
    }

    #[tokio::test]
    async fn test_get_job() {
        let manager = PipelineManager::new();

        let job = Job::new("remux", "/input.flv", "/output.mp4", "streamer-1", "session-1");
        let job_id = job.id.clone();
        manager.enqueue(job).await.unwrap();

        // Get existing job
        let retrieved = manager.get_job(&job_id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, job_id);

        // Get non-existing job
        let not_found = manager.get_job("non-existent-id").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_get_stats() {
        let manager = PipelineManager::new();

        // Enqueue some jobs
        let job1 = Job::new("remux", "/input1.flv", "/output1.mp4", "streamer-1", "session-1");
        let job2 = Job::new("upload", "/input2.flv", "/output2.mp4", "streamer-2", "session-2");
        manager.enqueue(job1).await.unwrap();
        manager.enqueue(job2).await.unwrap();

        let stats = manager.get_stats().await.unwrap();

        assert_eq!(stats.pending, 2);
        assert_eq!(stats.processing, 0);
        assert_eq!(stats.completed, 0);
        assert_eq!(stats.failed, 0);
        assert_eq!(stats.queue_depth, 2);
        assert_eq!(stats.queue_status, QueueDepthStatus::Normal);
    }

    #[tokio::test]
    async fn test_cancel_pending_job() {
        use crate::pipeline::JobStatus;

        let manager = PipelineManager::new();

        let job = Job::new("remux", "/input.flv", "/output.mp4", "streamer-1", "session-1");
        let job_id = job.id.clone();
        manager.enqueue(job).await.unwrap();

        // Cancel the pending job
        manager.cancel_job(&job_id).await.unwrap();

        // Verify job is now interrupted
        let cancelled = manager.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, JobStatus::Interrupted);
    }

    #[tokio::test]
    async fn test_create_pipeline_default_steps() {
        let manager = PipelineManager::new();

        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", None)
            .await
            .unwrap();

        assert!(!result.pipeline_id.is_empty());
        assert_eq!(result.first_job_id, result.pipeline_id);
        assert_eq!(result.first_job_type, "remux");
        assert_eq!(result.total_steps, 3);
        assert_eq!(result.steps, vec!["remux", "upload", "thumbnail"]);

        // Verify the first job was created
        let job = manager.get_job(&result.first_job_id).await.unwrap().unwrap();
        assert_eq!(job.job_type, "remux");
        assert_eq!(job.next_job_type, Some("upload".to_string()));
        assert_eq!(job.remaining_steps, Some(vec!["thumbnail".to_string()]));
        assert_eq!(job.pipeline_id, Some(result.pipeline_id.clone()));
    }

    #[tokio::test]
    async fn test_create_pipeline_custom_steps() {
        let manager = PipelineManager::new();

        let custom_steps = vec!["remux".to_string(), "upload".to_string()];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(custom_steps))
            .await
            .unwrap();

        assert_eq!(result.total_steps, 2);
        assert_eq!(result.steps, vec!["remux", "upload"]);
        assert_eq!(result.first_job_type, "remux");

        // Verify the first job has correct chain info
        let job = manager.get_job(&result.first_job_id).await.unwrap().unwrap();
        assert_eq!(job.next_job_type, Some("upload".to_string()));
        assert_eq!(job.remaining_steps, None); // No steps after upload
    }

    #[tokio::test]
    async fn test_create_pipeline_single_step() {
        let manager = PipelineManager::new();

        let single_step = vec!["remux".to_string()];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(single_step))
            .await
            .unwrap();

        assert_eq!(result.total_steps, 1);
        assert_eq!(result.first_job_type, "remux");

        // Verify the first job has no next job
        let job = manager.get_job(&result.first_job_id).await.unwrap().unwrap();
        assert_eq!(job.next_job_type, None);
        assert_eq!(job.remaining_steps, None);
    }

    #[tokio::test]
    async fn test_create_pipeline_empty_steps_error() {
        let manager = PipelineManager::new();

        let empty_steps: Vec<String> = vec![];
        let result = manager
            .create_pipeline("session-1", "streamer-1", "/input.flv", Some(empty_steps))
            .await;

        assert!(result.is_err());
    }
}
