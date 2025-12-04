//! Worker pool implementation for pipeline processing.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::job_queue::{JobQueue, JobResult};
use super::processors::{Processor, ProcessorInput};

/// Type of worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkerType {
    /// CPU-bound worker (remux, thumbnail).
    Cpu,
    /// IO-bound worker (upload, file operations).
    Io,
}

impl std::fmt::Display for WorkerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerType::Cpu => write!(f, "CPU"),
            WorkerType::Io => write!(f, "IO"),
        }
    }
}

/// Configuration for a worker pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerPoolConfig {
    /// Maximum concurrent workers.
    pub max_workers: usize,
    /// Job timeout in seconds.
    ///
    /// If a job exceeds this duration, the `process` future will be cancelled.
    /// Processors must be cancel-safe to handle this gracefully.
    pub job_timeout_secs: u64,
    /// Poll interval in milliseconds.
    pub poll_interval_ms: u64,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            max_workers: 4,
            job_timeout_secs: 3600, // 1 hour
            poll_interval_ms: 100,
        }
    }
}

/// A worker pool for processing jobs.
pub struct WorkerPool {
    /// Worker type.
    worker_type: WorkerType,
    /// Configuration.
    config: WorkerPoolConfig,
    /// Semaphore for concurrency control.
    semaphore: Arc<Semaphore>,
    /// Active worker count.
    active_workers: AtomicUsize,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Task set for workers.
    tasks: parking_lot::Mutex<Option<JoinSet<()>>>,
}

impl WorkerPool {
    /// Create a new worker pool.
    pub fn new(worker_type: WorkerType) -> Self {
        Self::with_config(worker_type, WorkerPoolConfig::default())
    }

    /// Create a new worker pool with custom configuration.
    pub fn with_config(worker_type: WorkerType, config: WorkerPoolConfig) -> Self {
        Self {
            worker_type,
            semaphore: Arc::new(Semaphore::new(config.max_workers)),
            config,
            active_workers: AtomicUsize::new(0),
            cancellation_token: CancellationToken::new(),
            tasks: parking_lot::Mutex::new(Some(JoinSet::new())),
        }
    }

    /// Start the worker pool.
    pub fn start(&self, job_queue: Arc<JobQueue>, processors: Vec<Arc<dyn Processor>>) {
        let worker_type = self.worker_type;
        let semaphore = self.semaphore.clone();
        let cancellation_token = self.cancellation_token.clone();
        let poll_interval = std::time::Duration::from_millis(self.config.poll_interval_ms);
        let job_timeout = std::time::Duration::from_secs(self.config.job_timeout_secs);
        let _active_workers = &self.active_workers;

        info!(
            "Starting {} worker pool with {} max workers",
            worker_type, self.config.max_workers
        );

        // Spawn worker tasks
        let mut tasks = self.tasks.lock();
        if let Some(ref mut join_set) = *tasks {
            for i in 0..self.config.max_workers {
                let semaphore = semaphore.clone();
                let cancellation_token = cancellation_token.clone();
                let job_queue = job_queue.clone();
                let processors = processors.clone();
                let notifier = job_queue.notifier();

                join_set.spawn(async move {
                    debug!("{} worker {} started", worker_type, i);

                    loop {
                        // Check for cancellation
                        if cancellation_token.is_cancelled() {
                            debug!("{} worker {} shutting down", worker_type, i);
                            break;
                        }

                        // Wait for a job or timeout
                        tokio::select! {
                            _ = cancellation_token.cancelled() => {
                                break;
                            }
                            _ = notifier.notified() => {
                                // New job available
                            }
                            _ = tokio::time::sleep(poll_interval) => {
                                // Poll timeout
                            }
                        }

                        // Try to acquire a permit
                        let permit = match semaphore.clone().try_acquire_owned() {
                            Ok(p) => p,
                            Err(_) => continue, // No permits available
                        };

                        // Try to dequeue a job
                        let job = match job_queue.dequeue(None).await {
                            Ok(Some(job)) => job,
                            Ok(None) => {
                                drop(permit);
                                continue;
                            }
                            Err(e) => {
                                error!("Error dequeuing job: {}", e);
                                drop(permit);
                                continue;
                            }
                        };

                        // Find a processor for this job
                        let processor = processors.iter().find(|p| p.can_process(&job.job_type));

                        if let Some(processor) = processor {
                            let job_id = job.id.clone();
                            let job_type = job.job_type.clone();

                            debug!(
                                "{} worker {} processing job {} ({})",
                                worker_type, i, job_id, job_type
                            );

                            // Process the job with timeout
                            let input = ProcessorInput {
                                input_path: job.input.clone(),
                                output_path: job.output.clone(),
                                config: job.config.clone(),
                                streamer_id: job.streamer_id.clone(),
                                session_id: job.session_id.clone(),
                            };

                            let result =
                                tokio::time::timeout(job_timeout, processor.process(&input)).await;

                            match result {
                                Ok(Ok(output)) => {
                                    let _ = job_queue
                                        .complete(
                                            &job_id,
                                            JobResult {
                                                output: output.output_path,
                                                duration_secs: output.duration_secs,
                                                metadata: output.metadata,
                                            },
                                        )
                                        .await;
                                }
                                Ok(Err(e)) => {
                                    let _ = job_queue.fail(&job_id, &e.to_string()).await;
                                }
                                Err(_) => {
                                    let _ = job_queue.fail(&job_id, "Job timed out").await;
                                }
                            }
                        } else {
                            warn!("No processor found for job type: {}", job.job_type);
                            let _ = job_queue.fail(&job.id, "No processor found").await;
                        }

                        drop(permit);
                    }
                });
            }
        }
    }

    /// Stop the worker pool.
    pub async fn stop(&self) {
        info!("Stopping {} worker pool", self.worker_type);
        self.cancellation_token.cancel();

        // Take the join set out of the mutex before awaiting
        let join_set = {
            let mut tasks = self.tasks.lock();
            tasks.take()
        };

        // Wait for all workers to finish (outside the lock)
        if let Some(mut join_set) = join_set {
            while join_set.join_next().await.is_some() {}
        }

        info!("{} worker pool stopped", self.worker_type);
    }

    /// Get the number of active workers.
    pub fn active_count(&self) -> usize {
        self.active_workers.load(Ordering::SeqCst)
    }

    /// Get the worker type.
    pub fn worker_type(&self) -> WorkerType {
        self.worker_type
    }

    /// Check if the pool is running.
    pub fn is_running(&self) -> bool {
        !self.cancellation_token.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_pool_config_default() {
        let config = WorkerPoolConfig::default();
        assert_eq!(config.max_workers, 4);
        assert_eq!(config.job_timeout_secs, 3600);
    }

    #[test]
    fn test_worker_type_display() {
        assert_eq!(format!("{}", WorkerType::Cpu), "CPU");
        assert_eq!(format!("{}", WorkerType::Io), "IO");
    }

    #[test]
    fn test_worker_pool_creation() {
        let pool = WorkerPool::new(WorkerType::Cpu);
        assert_eq!(pool.worker_type(), WorkerType::Cpu);
        assert!(pool.is_running());
    }
}
