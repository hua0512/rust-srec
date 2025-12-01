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
use crate::Result;
use crate::downloader::DownloadManagerEvent;

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
}
