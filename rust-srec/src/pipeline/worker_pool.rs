//! Worker pool implementation for pipeline processing.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::dag_scheduler::{
    DagCompletionInfo, DagJobCompletedUpdate, DagJobFailedUpdate, DagScheduler,
};
use super::job_queue::{JobExecutionInfo, JobQueue, JobResult};
use super::processors::{JobLogSink, Processor, ProcessorContext, ProcessorInput};

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

/// Adaptive worker scaling configuration.
///
/// When enabled, the pool dynamically adjusts its effective concurrency between
/// `min_workers` and `max_workers` based on backlog and observed runtimes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveWorkerPoolConfig {
    /// Enable adaptive scaling.
    pub enabled: bool,
    /// Minimum workers to keep available.
    pub min_workers: usize,
    /// Controller interval in milliseconds.
    pub interval_ms: u64,
    /// Target time to drain backlog (seconds).
    pub target_drain_secs: u64,
    /// Maximum workers to add per interval.
    pub max_step_up: usize,
    /// Maximum workers to remove per interval.
    pub max_step_down: usize,
    /// Only scale down after this many consecutive idle intervals.
    pub scale_down_idle_ticks: u32,
    /// Fallback runtime (milliseconds) until we have observations.
    pub default_avg_runtime_ms: u64,
}

impl Default for AdaptiveWorkerPoolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_workers: 1,
            interval_ms: 1000,
            target_drain_secs: 30,
            max_step_up: 2,
            max_step_down: 1,
            scale_down_idle_ticks: 5,
            default_avg_runtime_ms: 1000,
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
    /// Adaptive worker scaling configuration.
    #[serde(default)]
    pub adaptive: AdaptiveWorkerPoolConfig,
}

impl Default for WorkerPoolConfig {
    fn default() -> Self {
        Self {
            max_workers: 4,
            job_timeout_secs: 3600, // 1 hour
            poll_interval_ms: 100,
            adaptive: AdaptiveWorkerPoolConfig::default(),
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
    active_workers: Arc<AtomicUsize>,
    /// Desired max concurrency for this pool (<= config.max_workers).
    desired_workers: Arc<AtomicUsize>,
    /// Reserved permits to reduce effective concurrency.
    reserved_permits: Arc<parking_lot::Mutex<Vec<OwnedSemaphorePermit>>>,
    /// EWMA of observed runtime (milliseconds) for this pool.
    avg_runtime_ms: Arc<AtomicU64>,
    /// Cancellation token.
    cancellation_token: CancellationToken,
    /// Task set for workers.
    tasks: parking_lot::Mutex<Option<JoinSet<()>>>,
}

fn set_desired_with_handles(
    semaphore: &Arc<Semaphore>,
    reserved_permits: &parking_lot::Mutex<Vec<OwnedSemaphorePermit>>,
    desired_workers: &AtomicUsize,
    max_workers: usize,
    desired: usize,
) -> usize {
    let desired = desired.min(max_workers);
    desired_workers.store(desired, Ordering::SeqCst);

    let target_reserved = max_workers.saturating_sub(desired);
    let mut reserved = reserved_permits.lock();

    while reserved.len() > target_reserved {
        reserved.pop();
    }

    while reserved.len() < target_reserved {
        match semaphore.clone().try_acquire_owned() {
            Ok(p) => reserved.push(p),
            Err(_) => break,
        }
    }

    desired
}

fn update_avg_runtime_ms(avg_runtime_ms: &AtomicU64, sample_ms: u64) {
    // EWMA with alpha=0.2 in integer space: new = old + (sample-old)/5
    let sample_ms = sample_ms.max(1);
    let old = avg_runtime_ms.load(Ordering::Relaxed);
    let next = if old == 0 {
        sample_ms
    } else {
        let delta = sample_ms as i64 - old as i64;
        (old as i64 + delta / 5).max(1) as u64
    };
    avg_runtime_ms.store(next, Ordering::Relaxed);
}

impl WorkerPool {
    /// Create a new worker pool.
    pub fn new(worker_type: WorkerType) -> Self {
        Self::with_config(worker_type, WorkerPoolConfig::default())
    }

    /// Create a new worker pool with custom configuration.
    pub fn with_config(worker_type: WorkerType, config: WorkerPoolConfig) -> Self {
        let max_workers = config.max_workers;
        Self {
            worker_type,
            semaphore: Arc::new(Semaphore::new(max_workers)),
            config,
            active_workers: Arc::new(AtomicUsize::new(0)),
            desired_workers: Arc::new(AtomicUsize::new(max_workers)),
            reserved_permits: Arc::new(parking_lot::Mutex::new(Vec::new())),
            avg_runtime_ms: Arc::new(AtomicU64::new(0)),
            cancellation_token: CancellationToken::new(),
            tasks: parking_lot::Mutex::new(Some(JoinSet::new())),
        }
    }

    /// Get the desired effective concurrency for this pool.
    pub fn desired_max_workers(&self) -> usize {
        self.desired_workers.load(Ordering::SeqCst)
    }

    /// Get the configured maximum worker count for this pool.
    ///
    /// Note: this is the physical pool size created at `start*()` time and is not currently
    /// resizable without restarting the pool.
    pub fn max_workers(&self) -> usize {
        self.config.max_workers
    }

    /// Set the desired effective concurrency for this pool (clamped to `config.max_workers`).
    pub fn set_desired_max_workers(&self, desired: usize) -> usize {
        set_desired_with_handles(
            &self.semaphore,
            &self.reserved_permits,
            &self.desired_workers,
            self.config.max_workers,
            desired,
        )
    }

    /// Start the worker pool.
    pub fn start(&self, job_queue: Arc<JobQueue>, processors: Vec<Arc<dyn Processor>>) {
        self.start_with_dag_scheduler(job_queue, processors, None, None);
    }

    /// Start the worker pool with optional DAG scheduler support.
    pub fn start_with_dag_scheduler(
        &self,
        job_queue: Arc<JobQueue>,
        processors: Vec<Arc<dyn Processor>>,
        dag_scheduler: Option<Arc<DagScheduler>>,
        dag_notify_tx: Option<tokio::sync::mpsc::Sender<DagCompletionInfo>>,
    ) {
        let worker_type = self.worker_type;
        let semaphore = self.semaphore.clone();
        let cancellation_token = self.cancellation_token.clone();
        let poll_interval = std::time::Duration::from_millis(self.config.poll_interval_ms.max(1));
        let max_poll_interval = std::time::Duration::from_millis(
            self.config
                .poll_interval_ms
                .max(1)
                .saturating_mul(50)
                .max(1000),
        );
        let job_timeout = std::time::Duration::from_secs(self.config.job_timeout_secs);
        let active_workers = self.active_workers.clone();
        let avg_runtime_ms = self.avg_runtime_ms.clone();

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

        if self.config.adaptive.enabled {
            let initial = self
                .config
                .adaptive
                .min_workers
                .min(self.config.max_workers);
            self.set_desired_max_workers(initial);
            info!(
                "Adaptive scaling enabled for {} pool: desired_workers={}/{}",
                worker_type,
                self.desired_max_workers(),
                self.config.max_workers
            );
        }

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
                let active_workers = active_workers.clone();
                let avg_runtime_ms = avg_runtime_ms.clone();
                let dag_scheduler = dag_scheduler.clone();
                let dag_notify_tx = dag_notify_tx.clone();

                join_set.spawn(async move {
                    debug!("{} worker {} started", worker_type, i);
                    let mut current_poll_interval = poll_interval;

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
                            _ = tokio::time::sleep(current_poll_interval) => {
                                // Poll timeout (fallback / missed notify)
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
                                let next_ms = (current_poll_interval.as_millis() as u64)
                                    .saturating_mul(2)
                                    .min(max_poll_interval.as_millis() as u64);
                                current_poll_interval =
                                    std::time::Duration::from_millis(next_ms.max(1));
                                drop(permit);
                                continue;
                            }
                            Err(e) => {
                                error!("Error dequeuing job: {}", e);
                                let next_ms = (current_poll_interval.as_millis() as u64)
                                    .saturating_mul(2)
                                    .min(max_poll_interval.as_millis() as u64);
                                current_poll_interval =
                                    std::time::Duration::from_millis(next_ms.max(1));
                                drop(permit);
                                continue;
                            }
                        };
                        current_poll_interval = poll_interval;

                        // Find a processor for this job
                        let processor = processors.iter().find(|p| p.can_process(&job.job_type));

                        if let Some(processor) = processor {
                            let job_id = job.id.clone();
                            let job_type = job.job_type.clone();

                            debug!(
                                "{} worker {} processing job {} ({})",
                                worker_type, i, job_id, job_type
                            );

                            // Handle multi-input jobs for processors without batch support.
                            //
                            // IMPORTANT: Splitting a DAG step job into multiple jobs would corrupt DAG semantics
                            // (one step would be "completed" multiple times). For DAG jobs, fail fast instead.
                            if job.inputs.len() > 1 && !processor.supports_batch_input() {
                                if let Some(dag_step_id) = job.dag_step_execution_id.as_deref() {
                                    let reason = format!(
                                        "DAG step job has {} inputs but processor '{}' does not support batch inputs",
                                        job.inputs.len(),
                                        processor.name()
                                    );
                                    error!(job_id = %job_id, dag_step_execution_id = %dag_step_id, "{}", reason);

                                    // Mark job failed (best-effort logging).
                                    let _ = job_queue
                                        .fail_with_cleanup_and_step_info(
                                            &job_id,
                                            &reason,
                                            Some(processor.name()),
                                            job.execution_info.as_ref().and_then(|i| i.current_step),
                                            job.execution_info.as_ref().and_then(|i| i.total_steps),
                                        )
                                        .await;

                                    // Fail the DAG execution (fail-fast) and notify completion listeners.
                                    if let Some(scheduler) = &dag_scheduler {
                                        match scheduler.on_job_failed(dag_step_id, &reason).await {
                                            Ok(DagJobFailedUpdate { completion, .. }) => {
                                                if let Some(completion) = completion
                                                    && let Some(tx) = &dag_notify_tx
                                                    && let Err(e) = tx.send(completion).await
                                                {
                                                    warn!(
                                                        error = %e,
                                                        "Failed to send DAG completion notification"
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                error!(
                                                    dag_step_execution_id = %dag_step_id,
                                                    error = %e,
                                                    "Failed to fail DAG for non-batch processor"
                                                );
                                            }
                                        }
                                    }

                                    drop(permit);
                                    continue;
                                }

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

                            active_workers.fetch_add(1, Ordering::SeqCst);
                            let started = std::time::Instant::now();

                            // Record execution start info
                            let exec_info =
                                JobExecutionInfo::new().with_processor(processor.name());
                            if let Err(e) =
                                job_queue.update_execution_info(&job_id, exec_info).await
                            {
                                warn!("Failed to update execution info for job {}: {}", job_id, e);
                            }

                            // Process the job with timeout
                            let mut job = job;
                            let dag_step_execution_id = job.dag_step_execution_id.take();
                            let current_step = job
                                .execution_info
                                .as_ref()
                                .and_then(|i| i.current_step);
                            let total_steps = job.execution_info.as_ref().and_then(|i| i.total_steps);

                            let input = ProcessorInput {
                                inputs: std::mem::take(&mut job.inputs),
                                outputs: std::mem::take(&mut job.outputs),
                                config: job.config.take(),
                                streamer_id: std::mem::take(&mut job.streamer_id),
                                session_id: std::mem::take(&mut job.session_id),
                                streamer_name: job.streamer_name.take(),
                                session_title: job.session_title.take(),
                                platform: job.platform.take(),
                                created_at: job.created_at,
                            };

                            let (log_tx, mut log_rx) = tokio::sync::mpsc::channel(1024);
                            let log_dropped = Arc::new(AtomicUsize::new(0));
                            let log_sink = JobLogSink::new(log_tx, log_dropped.clone());
                            let job_queue_clone = job_queue.clone();
                            let job_id_clone = job_id.clone();

                            // Spawn log collector task
                            let log_collector = tokio::spawn(async move {
                                const FLUSH_INTERVAL_MS: u64 = 200;
                                const MAX_BATCH_SIZE: usize = 1000;
                                const MAX_BUFFERED_LOGS: usize = 4000;

                                let mut flush_timer =
                                    tokio::time::interval(std::time::Duration::from_millis(
                                        FLUSH_INTERVAL_MS,
                                    ));
                                flush_timer
                                    .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

                                let mut buffer: VecDeque<super::job_queue::JobLogEntry> =
                                    VecDeque::with_capacity(MAX_BATCH_SIZE);
                                let mut dropped_due_to_backpressure: usize = 0;

                                let mut backoff = std::time::Duration::ZERO;
                                let mut next_flush_allowed = tokio::time::Instant::now();

                                async fn flush(
                                    job_queue: &super::job_queue::JobQueue,
                                    job_id: &str,
                                    buffer: &mut VecDeque<super::job_queue::JobLogEntry>,
                                    backoff: &mut std::time::Duration,
                                    next_flush_allowed: &mut tokio::time::Instant,
                                    force: bool,
                                ) {
                                    if buffer.is_empty() {
                                        return;
                                    }
                                    if !force && tokio::time::Instant::now() < *next_flush_allowed {
                                        return;
                                    }

                                    let slice = buffer.make_contiguous();
                                    match job_queue.append_log_entry(job_id, slice).await {
                                        Ok(()) => {
                                            buffer.clear();
                                            *backoff = std::time::Duration::ZERO;
                                            *next_flush_allowed = tokio::time::Instant::now();
                                        }
                                        Err(e) => {
                                            warn!(
                                                "Failed to append streaming logs for job {}: {}",
                                                job_id, e
                                            );
                                            *backoff = if backoff.is_zero() {
                                                std::time::Duration::from_millis(200)
                                            } else {
                                                (*backoff * 2).min(std::time::Duration::from_secs(5))
                                            };
                                            *next_flush_allowed = tokio::time::Instant::now() + *backoff;
                                        }
                                    }
                                }

                                loop {
                                    tokio::select! {
                                        entry = log_rx.recv() => {
                                            match entry {
                                                Some(entry) => {
                                                    buffer.push_back(entry);

                                                    if buffer.len() > MAX_BUFFERED_LOGS {
                                                        while buffer.len() > MAX_BUFFERED_LOGS {
                                                            let _ = buffer.pop_front();
                                                            dropped_due_to_backpressure = dropped_due_to_backpressure.saturating_add(1);
                                                        }
                                                    }

                                                    if buffer.len() >= MAX_BATCH_SIZE {
                                                        flush(
                                                            &job_queue_clone,
                                                            &job_id_clone,
                                                            &mut buffer,
                                                            &mut backoff,
                                                            &mut next_flush_allowed,
                                                            false,
                                                        )
                                                        .await;
                                                    }
                                                }
                                                None => {
                                                    let producer_dropped = log_dropped.load(Ordering::Relaxed);
                                                    if dropped_due_to_backpressure > 0 {
                                                        buffer.push_back(super::job_queue::JobLogEntry::warn(format!(
                                                            "Dropped {} log lines due to DB backpressure (buffer_cap={})",
                                                            dropped_due_to_backpressure,
                                                            MAX_BUFFERED_LOGS,
                                                        )));
                                                    }
                                                    if producer_dropped > 0 {
                                                        buffer.push_back(super::job_queue::JobLogEntry::warn(format!(
                                                            "Dropped {} log lines due to log channel backpressure (capacity={})",
                                                            producer_dropped,
                                                            1024,
                                                        )));
                                                    }
                                                    flush(
                                                        &job_queue_clone,
                                                        &job_id_clone,
                                                        &mut buffer,
                                                        &mut backoff,
                                                        &mut next_flush_allowed,
                                                        true,
                                                    )
                                                    .await;
                                                    break;
                                                }
                                            }
                                        }
                                        _ = flush_timer.tick() => {
                                            flush(
                                                &job_queue_clone,
                                                &job_id_clone,
                                                &mut buffer,
                                                &mut backoff,
                                                &mut next_flush_allowed,
                                                false,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            });

                            let job_cancellation_token =
                                match job_queue.get_cancellation_token(&job_id).await {
                                    Some(token) => token,
                                    None => {
                                        warn!(
                                            job_id = %job_id,
                                            "Missing cancellation token for processing job"
                                        );
                                        CancellationToken::new()
                                    }
                                };

                            let ctx = ProcessorContext::new(
                                job_id.clone(),
                                job_queue.progress_reporter(&job_id),
                                log_sink,
                                job_cancellation_token.clone(),
                            );

                            let result = {
                                let timed = tokio::time::timeout(
                                    job_timeout,
                                    processor.process(&input, &ctx),
                                );
                                tokio::pin!(timed);

                                tokio::select! {
                                    biased;
                                    _ = job_cancellation_token.cancelled() => None,
                                    res = &mut timed => Some(res),
                                }
                            };

                            // Drop ctx to close the log channel
                            drop(ctx);

                            // Wait for log collector to finish draining
                            let _ = log_collector.await;

                            match result {
                                None => {
                                    info!(job_id = %job_id, "Job cancelled while processing");
                                }
                                Some(Ok(Ok(output))) => {
                                    if job_cancellation_token.is_cancelled() {
                                        info!(
                                            job_id = %job_id,
                                            "Job finished after cancellation; skipping completion"
                                        );
                                    }

                                    if !job_cancellation_token.is_cancelled() {
                                    // Track partial outputs for observability
                                    if !output.items_produced.is_empty()
                                        && let Err(e) = job_queue
                                            .track_partial_outputs(&job_id, &output.items_produced)
                                            .await
                                        {
                                            warn!(
                                                "Failed to track partial outputs for job {}: {}",
                                                job_id, e
                                            );
                                        }

                                    // Check if this is a DAG job
                                    debug!(
                                        "Job {} completed, dag_step_execution_id={:?}",
                                        job_id, dag_step_execution_id
                                    );
                                    if let Some(dag_step_id) = dag_step_execution_id.as_deref()
                                        && let Some(scheduler) = &dag_scheduler
                                    {
                                        // Notify DAG scheduler to create next jobs before moving outputs into completion.
                                        match scheduler
                                            .on_job_completed(
                                                dag_step_id,
                                                &output.outputs,
                                                input.streamer_name.as_deref(),
                                                input.session_title.as_deref(),
                                                input.platform.as_deref(),
                                            )
                                            .await
                                        {
                                            Ok(DagJobCompletedUpdate { new_job_ids, completion }) => {
                                                if !new_job_ids.is_empty() {
                                                    info!(
                                                        "DAG step {} completed, created {} downstream jobs",
                                                        dag_step_id,
                                                        new_job_ids.len()
                                                    );
                                                }
                                                if let Some(completion) = completion
                                                    && let Some(tx) = &dag_notify_tx
                                                    && let Err(e) = tx.send(completion).await
                                                {
                                                    warn!(
                                                        error = %e,
                                                        "Failed to send DAG completion notification"
                                                    );
                                                }
                                            }
                                            Err(e) => {
                                                error!(
                                                    "Failed to handle DAG job completion for {}: {}",
                                                    dag_step_id, e
                                                );
                                                if let Ok(completion) = scheduler
                                                    .fail_dag_for_step(
                                                        dag_step_id,
                                                        &format!("DAG scheduler error: {}", e),
                                                    )
                                                    .await
                                                    && let Some(completion) = completion
                                                    && let Some(tx) = &dag_notify_tx
                                                    && let Err(e) = tx.send(completion).await
                                                {
                                                    warn!(
                                                        error = %e,
                                                        "Failed to send DAG completion notification"
                                                    );
                                                }
                                            }
                                        }
                                    }

                                    // Complete the job normally.
                                    let _ = job_queue
                                        .complete(
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
                                }
                                Some(Ok(Err(e))) => {
                                    if job_cancellation_token.is_cancelled() {
                                        info!(
                                            job_id = %job_id,
                                            "Job failed after cancellation; skipping failure handling"
                                        );
                                    }

                                    if !job_cancellation_token.is_cancelled() {
                                    // Check if this is a DAG job for fail-fast handling
                                    let error = e.to_string();
                                    if let Some(dag_step_id) = dag_step_execution_id.as_deref() {
                                        // First mark job as failed
                                        let partial_outputs = job_queue
                                            .fail_with_cleanup_and_step_info(
                                                &job_id,
                                                &error,
                                                Some(processor.name()),
                                                current_step,
                                                total_steps,
                                            )
                                            .await;
                                        if let Ok(outputs) = partial_outputs
                                            && !outputs.is_empty() {
                                                cleanup_partial_outputs(&outputs).await;
                                            }

                                        // Then notify DAG scheduler for fail-fast
                                        if let Some(scheduler) = &dag_scheduler {
                                            match scheduler
                                                .on_job_failed(dag_step_id, &error)
                                                .await
                                            {
                                                Ok(DagJobFailedUpdate { cancelled_count, completion }) => {
                                                    info!(
                                                        "DAG step {} failed, cancelled {} jobs (fail-fast)",
                                                        dag_step_id, cancelled_count
                                                    );
                                                    if let Some(completion) = completion
                                                        && let Some(tx) = &dag_notify_tx
                                                        && let Err(e) = tx.send(completion).await
                                                    {
                                                        warn!(
                                                            error = %e,
                                                            "Failed to send DAG completion notification"
                                                        );
                                                    }
                                                }
                                                Err(err) => {
                                                    error!(
                                                        "Failed to handle DAG job failure for {}: {}",
                                                        dag_step_id, err
                                                    );
                                                }
                                            }
                                        }
                                    } else {
                                        // Regular pipeline job failure
                                        // Clean up partial outputs on failure
                                        // Record step info for observability
                                        let partial_outputs = job_queue
                                            .fail_with_cleanup_and_step_info(
                                                &job_id,
                                                &error,
                                                Some(processor.name()),
                                                current_step,
                                                total_steps,
                                            )
                                            .await;
                                        if let Ok(outputs) = partial_outputs
                                            && !outputs.is_empty() {
                                                cleanup_partial_outputs(&outputs).await;
                                        }
                                    }
                                    }
                                }
                                Some(Err(_)) => {
                                    if job_cancellation_token.is_cancelled() {
                                        info!(
                                            job_id = %job_id,
                                            "Job timed out after cancellation; skipping timeout handling"
                                        );
                                    }

                                    if !job_cancellation_token.is_cancelled() {
                                    job_cancellation_token.cancel();

                                    // Check if this is a DAG job for fail-fast handling
                                    if let Some(dag_step_id) = dag_step_execution_id.as_deref() {
                                        // First mark job as failed
                                        let partial_outputs = job_queue
                                            .fail_with_cleanup_and_step_info(
                                                &job_id,
                                                "Job timed out",
                                                Some(processor.name()),
                                                current_step,
                                                total_steps,
                                            )
                                            .await;
                                        if let Ok(outputs) = partial_outputs
                                            && !outputs.is_empty() {
                                                cleanup_partial_outputs(&outputs).await;
                                            }

                                        // Then notify DAG scheduler for fail-fast
                                        if let Some(scheduler) = &dag_scheduler {
                                            match scheduler
                                                .on_job_failed(dag_step_id, "Job timed out")
                                                .await
                                            {
                                                Ok(DagJobFailedUpdate { cancelled_count, completion }) => {
                                                    info!(
                                                        "DAG step {} timed out, cancelled {} jobs (fail-fast)",
                                                        dag_step_id, cancelled_count
                                                    );
                                                    if let Some(completion) = completion
                                                        && let Some(tx) = &dag_notify_tx
                                                        && let Err(e) = tx.send(completion).await
                                                    {
                                                        warn!(
                                                            error = %e,
                                                            "Failed to send DAG completion notification"
                                                        );
                                                    }
                                                }
                                                Err(err) => {
                                                    error!(
                                                        "Failed to handle DAG job timeout for {}: {}",
                                                        dag_step_id, err
                                                    );
                                                }
                                            }
                                        }
                                    } else {
                                        // Regular pipeline job timeout
                                        // Clean up partial outputs on timeout
                                        // Record step info for observability
                                        let partial_outputs = job_queue
                                            .fail_with_cleanup_and_step_info(
                                                &job_id,
                                                "Job timed out",
                                                Some(processor.name()),
                                                current_step,
                                                total_steps,
                                            )
                                            .await;
                                        if let Ok(outputs) = partial_outputs
                                            && !outputs.is_empty() {
                                                cleanup_partial_outputs(&outputs).await;
                                            }
                                    }
                                    }
                                }
                            }

                            if job_cancellation_token.is_cancelled() {
                                job_queue.finalize_interrupted_job(&job_id);
                            }

                            active_workers.fetch_sub(1, Ordering::SeqCst);
                            update_avg_runtime_ms(
                                &avg_runtime_ms,
                                started.elapsed().as_millis() as u64,
                            );
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

            if self.config.adaptive.enabled {
                let adaptive = self.config.adaptive.clone();
                let max_workers = self.config.max_workers;
                let desired_workers = self.desired_workers.clone();
                let reserved_permits = self.reserved_permits.clone();
                let semaphore = self.semaphore.clone();
                let job_queue = job_queue.clone();
                let supported_job_types = supported_job_types.clone();
                let cancellation_token = self.cancellation_token.clone();
                let active_workers = self.active_workers.clone();
                let avg_runtime_ms = self.avg_runtime_ms.clone();

                join_set.spawn(async move {
                    let interval =
                        std::time::Duration::from_millis(adaptive.interval_ms.max(100));
                    let mut idle_ticks: u32 = 0;

                    loop {
                        tokio::select! {
                            _ = cancellation_token.cancelled() => break,
                            _ = tokio::time::sleep(interval) => {}
                        }

                        let filter_types = if supported_job_types.is_empty() {
                            None
                        } else {
                            Some(supported_job_types.as_slice())
                        };

                        let pending = match job_queue.count_pending_jobs(filter_types).await {
                            Ok(v) => v,
                            Err(e) => {
                                warn!("Adaptive {} scaler failed to count pending jobs: {}", worker_type, e);
                                continue;
                            }
                        };

                        let active = active_workers.load(Ordering::SeqCst) as u64;
                        let backlog = pending.saturating_add(active);

                        if pending == 0 && active == 0 {
                            idle_ticks = idle_ticks.saturating_add(1);
                        } else {
                            idle_ticks = 0;
                        }

                        let avg_ms = avg_runtime_ms
                            .load(Ordering::Relaxed)
                            .max(adaptive.default_avg_runtime_ms.max(1));
                        let target_drain_secs = adaptive.target_drain_secs.max(1) as f64;

                        let mut target = if backlog == 0 {
                            0usize
                        } else {
                            (((backlog as f64) * (avg_ms as f64) / 1000.0) / target_drain_secs)
                                .ceil()
                                .max(0.0) as usize
                        };
                        target = target.clamp(adaptive.min_workers.min(max_workers), max_workers);

                        let current = desired_workers.load(Ordering::SeqCst);
                        let mut next = current;

                        if target > current {
                            let step = adaptive.max_step_up.max(1);
                            next = (current + step).min(target);
                        } else if target < current && idle_ticks >= adaptive.scale_down_idle_ticks.max(1) {
                            let step = adaptive.max_step_down.max(1);
                            next = current.saturating_sub(step).max(target);
                        }

                        if next != current {
                            let applied = set_desired_with_handles(
                                &semaphore,
                                &reserved_permits,
                                &desired_workers,
                                max_workers,
                                next,
                            );
                            debug!(
                                "Adaptive {} pool concurrency: pending={}, active={}, avg_ms={}, desired={}/{} (target={})",
                                worker_type, pending, active, avg_ms, applied, max_workers, target
                            );
                        }
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
async fn cleanup_partial_outputs(outputs: &[String]) {
    for output in outputs {
        let path = std::path::Path::new(output);
        match tokio::fs::remove_file(path).await {
            Ok(()) => {
                info!("Cleaned up partial output: {}", output);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!("Failed to clean up partial output {}: {}", output, e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::time::Duration;
    use tempfile::TempDir;

    use crate::pipeline::{Job, JobStatus, ProcessorOutput, ProcessorType};

    struct SleepProcessor;

    #[async_trait]
    impl Processor for SleepProcessor {
        fn processor_type(&self) -> ProcessorType {
            ProcessorType::Cpu
        }

        fn job_types(&self) -> Vec<&'static str> {
            vec!["sleep"]
        }

        async fn process(
            &self,
            _input: &ProcessorInput,
            _ctx: &ProcessorContext,
        ) -> crate::Result<ProcessorOutput> {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            Ok(ProcessorOutput::default())
        }

        fn name(&self) -> &'static str {
            "sleep"
        }
    }

    struct TimeoutPublishProcessor;

    #[async_trait]
    impl Processor for TimeoutPublishProcessor {
        fn processor_type(&self) -> ProcessorType {
            ProcessorType::Cpu
        }

        fn job_types(&self) -> Vec<&'static str> {
            vec!["timeout-publish"]
        }

        async fn process(
            &self,
            input: &ProcessorInput,
            ctx: &ProcessorContext,
        ) -> crate::Result<ProcessorOutput> {
            let output_path =
                input.outputs.first().cloned().ok_or_else(|| {
                    crate::Error::PipelineError("missing output path".to_string())
                })?;

            let token = ctx.cancellation_token.clone();
            let output_path_for_blocking = output_path.clone();
            tokio::task::spawn_blocking(move || {
                std::thread::sleep(Duration::from_millis(1500));
                if token.is_cancelled() {
                    return Ok::<_, std::io::Error>(());
                }
                std::fs::write(&output_path_for_blocking, b"published")?;
                Ok(())
            })
            .await
            .map_err(|e| crate::Error::Other(format!("blocking task panicked: {}", e)))??;

            Ok(ProcessorOutput {
                outputs: vec![output_path.clone()],
                items_produced: vec![output_path],
                ..Default::default()
            })
        }

        fn name(&self) -> &'static str {
            "timeout-publish"
        }
    }

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

    #[tokio::test]
    async fn test_cancel_processing_job_releases_worker() {
        let job_queue = Arc::new(JobQueue::new());
        let pool = WorkerPool::with_config(
            WorkerType::Cpu,
            WorkerPoolConfig {
                max_workers: 1,
                job_timeout_secs: 3600,
                poll_interval_ms: 10,
                adaptive: AdaptiveWorkerPoolConfig::default(),
            },
        );

        pool.start(job_queue.clone(), vec![Arc::new(SleepProcessor)]);

        let job = Job::new(
            "sleep",
            vec!["/input".to_string()],
            vec![],
            "streamer-1",
            "session-1",
        );
        let job_id = job_queue.enqueue(job).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(job) = job_queue.get_job(&job_id).await.unwrap()
                    && job.status == JobStatus::Processing
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("job should start processing");

        job_queue.cancel_job(&job_id).await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if pool.active_count() == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("worker should stop processing cancelled job");

        let job = job_queue.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Interrupted);

        pool.stop().await;
    }

    #[tokio::test]
    async fn test_timeout_cancels_job_token_prevents_publish() {
        let dir = TempDir::new().unwrap();
        let output_path = dir.path().join("published.txt");
        let output_str = output_path.to_string_lossy().to_string();

        let job_queue = Arc::new(JobQueue::new());
        let pool = WorkerPool::with_config(
            WorkerType::Cpu,
            WorkerPoolConfig {
                max_workers: 1,
                job_timeout_secs: 1,
                poll_interval_ms: 10,
                adaptive: AdaptiveWorkerPoolConfig::default(),
            },
        );

        pool.start(job_queue.clone(), vec![Arc::new(TimeoutPublishProcessor)]);

        let job = Job::new(
            "timeout-publish",
            vec!["/input".to_string()],
            vec![output_str.clone()],
            "streamer-1",
            "session-1",
        );
        let job_id = job_queue.enqueue(job).await.unwrap();

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(job) = job_queue.get_job(&job_id).await.unwrap()
                    && job.status == JobStatus::Failed
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("job should time out and fail");

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!output_path.exists());

        pool.stop().await;
    }
}
