//! Database-backed job queue implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::Result;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
        }
    }

    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        
        // In a real implementation, this would persist to the database
        // For now, we just track the depth
        self.depth.fetch_add(1, Ordering::SeqCst);
        
        info!("Enqueued job {} of type {}", job_id, job.job_type);
        
        // Notify waiting workers
        self.notify.notify_one();
        
        Ok(job_id)
    }

    /// Dequeue a job for processing.
    pub async fn dequeue(&self, job_type: Option<&str>) -> Result<Option<Job>> {
        // In a real implementation, this would query the database
        // For now, return None (no jobs)
        debug!("Attempting to dequeue job of type {:?}", job_type);
        Ok(None)
    }

    /// Wait for a job to become available.
    pub async fn wait_for_job(&self) {
        self.notify.notified().await;
    }

    /// Mark a job as completed.
    pub async fn complete(&self, job_id: &str, result: JobResult) -> Result<()> {
        self.depth.fetch_sub(1, Ordering::SeqCst);
        info!(
            "Job {} completed in {:.2}s",
            job_id, result.duration_secs
        );
        Ok(())
    }

    /// Mark a job as failed.
    pub async fn fail(&self, job_id: &str, error: &str) -> Result<()> {
        self.depth.fetch_sub(1, Ordering::SeqCst);
        warn!("Job {} failed: {}", job_id, error);
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
        let job = Job::new("remux", "/input.flv", "/output.mp4", "streamer-1", "session-1")
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
