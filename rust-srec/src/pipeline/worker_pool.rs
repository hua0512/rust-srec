//! Worker pool implementation for pipeline processing.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::job_queue::{JobExecutionInfo, JobQueue, JobResult};
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

        // Collect supported job types for valid filtering
        let supported_job_types: Vec<String> = processors
            .iter()
            .flat_map(|p| p.job_types())
            .map(|s| s.to_string())
            .collect();
        let supported_job_types = Arc::new(supported_job_types);

        // Spawn worker tasks
        let mut tasks = self.tasks.lock();
        if let Some(ref mut join_set) = *tasks {
            for i in 0..self.config.max_workers {
                let semaphore = semaphore.clone();
                let cancellation_token = cancellation_token.clone();
                let job_queue = job_queue.clone();
                let processors = processors.clone();
                let notifier = job_queue.notifier();
                let supported_job_types = supported_job_types.clone();

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
                        let filter_types = if supported_job_types.is_empty() {
                            None
                        } else {
                            Some(supported_job_types.as_slice()) // Vec Derefs to slice
                        };

                        let job = match job_queue.dequeue(filter_types).await {
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

                            // Handle fan-out: split multi-input jobs for single-input processors
                            // Requirements: 11.2, 11.3
                            if job.inputs.len() > 1 && !processor.supports_batch_input() {
                                info!(
                                    "Splitting job {} with {} inputs for single-input processor {}",
                                    job_id,
                                    job.inputs.len(),
                                    processor.name()
                                );
                                match job_queue.split_job_for_single_input(&job).await {
                                    Ok(split_ids) => {
                                        info!(
                                            "Split job {} into {} jobs: {:?}",
                                            job_id,
                                            split_ids.len(),
                                            split_ids
                                        );
                                    }
                                    Err(e) => {
                                        error!("Failed to split job {}: {}", job_id, e);
                                        let _ = job_queue
                                            .fail(&job_id, &format!("Failed to split job: {}", e))
                                            .await;
                                    }
                                }
                                drop(permit);
                                continue;
                            }

                            // Record execution start info
                            // Requirements: 6.1
                            let exec_info =
                                JobExecutionInfo::new().with_processor(processor.name());
                            if let Err(e) =
                                job_queue.update_execution_info(&job_id, exec_info).await
                            {
                                warn!("Failed to update execution info for job {}: {}", job_id, e);
                            }

                            // Process the job with timeout
                            let input = ProcessorInput {
                                inputs: job.inputs.clone(),
                                outputs: job.outputs.clone(),
                                config: job.config.clone(),
                                streamer_id: job.streamer_id.clone(),
                                session_id: job.session_id.clone(),
                            };

                            let result =
                                tokio::time::timeout(job_timeout, processor.process(&input)).await;

                            match result {
                                Ok(Ok(output)) => {
                                    // Track partial outputs for observability
                                    // Requirements: 9.1, 9.2
                                    if !output.items_produced.is_empty() {
                                        if let Err(e) = job_queue
                                            .track_partial_outputs(&job_id, &output.items_produced)
                                            .await
                                        {
                                            warn!(
                                                "Failed to track partial outputs for job {}: {}",
                                                job_id, e
                                            );
                                        }
                                    }

                                    // Complete job and pass all outputs to next step
                                    // Requirements: 9.1, 9.2
                                    let _ = job_queue
                                        .complete_with_next(
                                            &job_id,
                                            JobResult {
                                                outputs: output.outputs,
                                                duration_secs: output.duration_secs,
                                                metadata: output.metadata,
                                                logs: output.logs,
                                            },
                                        )
                                        .await;
                                }
                                Ok(Err(e)) => {
                                    // Clean up partial outputs on failure
                                    // Record step info for observability
                                    // Requirements: 6.4, 9.5, 10.3
                                    let partial_outputs = job_queue
                                        .fail_with_cleanup_and_step_info(
                                            &job_id,
                                            &e.to_string(),
                                            Some(processor.name()),
                                            job.execution_info
                                                .as_ref()
                                                .and_then(|i| i.current_step),
                                            job.execution_info.as_ref().and_then(|i| i.total_steps),
                                        )
                                        .await;
                                    if let Ok(outputs) = partial_outputs {
                                        if !outputs.is_empty() {
                                            cleanup_partial_outputs(&outputs).await;
                                        }
                                    }
                                }
                                Err(_) => {
                                    // Clean up partial outputs on timeout
                                    // Record step info for observability
                                    // Requirements: 6.4, 9.5, 10.3
                                    let partial_outputs = job_queue
                                        .fail_with_cleanup_and_step_info(
                                            &job_id,
                                            "Job timed out",
                                            Some(processor.name()),
                                            job.execution_info
                                                .as_ref()
                                                .and_then(|i| i.current_step),
                                            job.execution_info.as_ref().and_then(|i| i.total_steps),
                                        )
                                        .await;
                                    if let Ok(outputs) = partial_outputs {
                                        if !outputs.is_empty() {
                                            cleanup_partial_outputs(&outputs).await;
                                        }
                                    }
                                }
                            }
                        } else {
                            warn!(
                                "No processor found for job type: '{}'. Available processors: {:?}",
                                job.job_type,
                                processors.iter().map(|p| p.name()).collect::<Vec<_>>()
                            );
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

/// Clean up partial outputs created by a failed job.
/// Requirements: 9.5
async fn cleanup_partial_outputs(outputs: &[String]) {
    for output in outputs {
        let path = std::path::Path::new(output);
        if path.exists() {
            match tokio::fs::remove_file(path).await {
                Ok(_) => {
                    info!("Cleaned up partial output: {}", output);
                }
                Err(e) => {
                    warn!("Failed to clean up partial output {}: {}", output, e);
                }
            }
        }
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
