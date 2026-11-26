//! Worker pool implementation for pipeline processing.

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::dag_scheduler::{DagCompletionInfo, DagScheduler};
use super::job_queue::JobQueue;
use super::manager::PipelineEvent;
use super::processors::Processor;

mod execution;
mod lingering;
#[cfg(test)]
mod tests;

use execution::JobRunner;
use lingering::LingeringAttempts;
pub(crate) use lingering::STALLED_OPERATION_THRESHOLD;
pub use lingering::StalledOperation;

/// How long a job already in `Processor::process` may keep running after the pool's
/// cancellation token fires before its own token is cancelled too.
///
/// Sized to sit inside the cooperative window `ServiceContainer::shutdown_until` allows, so a
/// short job (thumbnail, metadata, delete) still finishes and records its result, while a long
/// one (remux, upload) stops cooperatively instead of holding `WorkerPool::stop` until the
/// caller gives up and calls `WorkerPool::abort`.
pub(super) const SHUTDOWN_DRAIN_GRACE: std::time::Duration = std::time::Duration::from_secs(10);

/// Replaces [`SHUTDOWN_DRAIN_GRACE`] when set.
#[cfg(any(test, feature = "test-support"))]
static SHUTDOWN_DRAIN_GRACE_OVERRIDE: parking_lot::RwLock<Option<std::time::Duration>> =
    parking_lot::RwLock::new(None);

/// Give a running job `grace` instead of [`SHUTDOWN_DRAIN_GRACE`] to finish after a pool
/// shutdown starts, so an end-to-end test can interrupt a job with a restart without waiting
/// the production 10 seconds. Applies to shutdowns that start afterwards.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub fn override_shutdown_drain_grace(grace: std::time::Duration) {
    *SHUTDOWN_DRAIN_GRACE_OVERRIDE.write() = Some(grace);
}

/// [`SHUTDOWN_DRAIN_GRACE`], or the duration `override_shutdown_drain_grace` set.
pub(super) fn shutdown_drain_grace() -> std::time::Duration {
    #[cfg(any(test, feature = "test-support"))]
    if let Some(grace) = *SHUTDOWN_DRAIN_GRACE_OVERRIDE.read() {
        return grace;
    }
    SHUTDOWN_DRAIN_GRACE
}

/// Capacity of the per-job channel between `JobLogSink` and its collector task. Overflow is
/// counted by the sink and reported in the job's log rather than blocking the processor.
pub(super) const LOG_CHANNEL_CAPACITY: usize = 1024;

/// Error recorded when no registered processor accepts a job's type.
pub(super) const NO_PROCESSOR_ERROR: &str = "No processor found";

/// Broadcast a job lifecycle event, if the pool was started with a sender.
///
/// Send errors mean no subscriber is currently attached (the notification service is the only
/// consumer, and it is optional), which is not a job failure.
pub(super) fn emit_event(
    event_tx: Option<&broadcast::Sender<PipelineEvent>>,
    event: PipelineEvent,
) {
    if let Some(tx) = event_tx {
        let _ = tx.send(event);
    }
}

/// Type of worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkerType {
    /// CPU-bound worker (remux, thumbnail).
    Cpu,
    /// IO-bound worker (file operations).
    Io,
    /// Worker that runs [`ProcessorType::Upload`] jobs.
    ///
    /// [`ProcessorType::Upload`]: crate::ProcessorType::Upload
    Upload,
}

impl std::fmt::Display for WorkerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerType::Cpu => write!(f, "CPU"),
            WorkerType::Io => write!(f, "IO"),
            WorkerType::Upload => write!(f, "Upload"),
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
    pub(super) worker_type: WorkerType,
    /// Configuration.
    pub(super) config: WorkerPoolConfig,
    /// Admission limit, including workers currently claiming a job.
    pub(super) concurrency: Arc<WorkerConcurrency>,
    /// Active worker count.
    pub(super) active_workers: Arc<AtomicUsize>,
    /// Cancellation token.
    pub(super) cancellation_token: CancellationToken,
    /// Task set for workers.
    pub(super) tasks: parking_lot::Mutex<Option<JoinSet<()>>>,
    /// Attempts whose job ended while their blocking work still holds a worker.
    lingering: Arc<LingeringAttempts>,
}

pub(super) struct WorkerConcurrency {
    pub(super) max_workers: usize,
    pub(super) state: parking_lot::Mutex<AdmissionState>,
}

pub(super) struct AdmissionState {
    pub(super) limit: usize,
    pub(super) admitted: usize,
}

impl WorkerConcurrency {
    fn new(max_workers: usize) -> Self {
        Self {
            max_workers,
            state: parking_lot::Mutex::new(AdmissionState {
                limit: max_workers,
                admitted: 0,
            }),
        }
    }

    pub(super) fn limit(&self) -> usize {
        self.state.lock().limit
    }

    pub(super) fn set_limit(&self, desired: usize) -> usize {
        let mut state = self.state.lock();
        state.limit = desired.min(self.max_workers);
        state.limit
    }

    pub(super) fn try_acquire(self: &Arc<Self>) -> Option<WorkerPermit> {
        // Serialize admission with limit changes. A decrease can leave already-admitted
        // jobs above the limit, but no new claim starts until enough of them finish.
        let mut state = self.state.lock();
        if state.admitted >= state.limit {
            return None;
        }
        state.admitted += 1;
        Some(WorkerPermit {
            concurrency: Arc::clone(self),
        })
    }
}

pub(super) struct WorkerPermit {
    concurrency: Arc<WorkerConcurrency>,
}

/// The pool-wide turn for jobs of processors that run one job at a time.
#[derive(Default)]
struct OneAtATime {
    held: AtomicBool,
    /// A worker found the turn held and claimed without those job types.
    missed: AtomicBool,
}

/// A held [`OneAtATime`] turn. Released on drop, so an aborted or panicking
/// worker cannot keep it.
struct OneAtATimeTurn {
    state: Arc<OneAtATime>,
    notifier: Arc<Notify>,
}

impl OneAtATimeTurn {
    fn try_take(state: &Arc<OneAtATime>, notifier: &Arc<Notify>) -> Option<Self> {
        // SeqCst on `held` and `missed` here and in `pass_on`: either this
        // worker sees the turn released, or `pass_on` sees the miss and wakes it.
        loop {
            if state
                .held
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(Self {
                    state: Arc::clone(state),
                    notifier: Arc::clone(notifier),
                });
            }
            state.missed.store(true, Ordering::SeqCst);
            if state.held.load(Ordering::SeqCst) {
                return None;
            }
        }
    }

    /// Release a turn the claimed job does not need. A worker that missed it
    /// may have parked with backoff while a one-at-a-time job waits, so wake
    /// the workers instead of leaving that job to its next poll. Only a
    /// successful claim passes a turn on, so this cannot wake workers in a loop.
    fn pass_on(self) {
        let state = Arc::clone(&self.state);
        let notifier = Arc::clone(&self.notifier);
        drop(self);
        if state.missed.swap(false, Ordering::SeqCst) {
            notifier.notify_waiters();
        }
    }
}

impl Drop for OneAtATimeTurn {
    fn drop(&mut self) {
        self.state.held.store(false, Ordering::SeqCst);
    }
}

/// Active accounting must settle even when a worker is aborted or panics.
pub(super) struct ActiveWorkerGuard<'a>(pub(super) &'a AtomicUsize);

impl Drop for ActiveWorkerGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        self.concurrency.state.lock().admitted -= 1;
    }
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
            concurrency: Arc::new(WorkerConcurrency::new(max_workers)),
            config,
            active_workers: Arc::new(AtomicUsize::new(0)),
            cancellation_token: CancellationToken::new(),
            tasks: parking_lot::Mutex::new(Some(JoinSet::new())),
            lingering: Arc::default(),
        }
    }

    /// Get the current admission limit. Jobs admitted before a decrease may temporarily
    /// exceed this limit while they finish.
    pub fn desired_max_workers(&self) -> usize {
        self.concurrency.limit()
    }

    /// Get the configured maximum worker count for this pool.
    ///
    /// Note: this is the physical pool size created at `start*()` time and is not currently
    /// resizable without restarting the pool.
    pub fn max_workers(&self) -> usize {
        self.config.max_workers
    }

    /// Set the admission limit (clamped to `config.max_workers`) without interrupting jobs.
    pub fn set_desired_max_workers(&self, desired: usize) -> usize {
        self.concurrency.set_limit(desired)
    }

    /// Start the worker pool.
    pub fn start(&self, job_queue: Arc<JobQueue>, processors: Vec<Arc<dyn Processor>>) {
        self.start_with_dag_scheduler(job_queue, processors, None, None, None);
    }

    /// Start the worker pool with optional DAG scheduler support.
    ///
    /// `event_tx` receives the `PipelineEvent::Job{Started,Completed,Failed}` transitions this
    /// pool observes; workers are the only place they are known, since a job's terminal state
    /// is decided here rather than by `PipelineManager`.
    pub fn start_with_dag_scheduler(
        &self,
        job_queue: Arc<JobQueue>,
        processors: Vec<Arc<dyn Processor>>,
        dag_scheduler: Option<Arc<DagScheduler>>,
        dag_notify_tx: Option<tokio::sync::mpsc::Sender<DagCompletionInfo>>,
        event_tx: Option<broadcast::Sender<PipelineEvent>>,
    ) {
        let worker_type = self.worker_type;
        let concurrency = self.concurrency.clone();
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
        let lingering = self.lingering.clone();

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
        // Job types of processors that run one job at a time. A worker may claim
        // them only while it holds `one_at_a_time`; otherwise it claims from
        // `other_job_types`. Claiming in one queue keeps those jobs in queue
        // order with the rest of the pool's work, and a job waiting for its turn
        // holds no worker.
        let one_at_a_time_job_types: Arc<Vec<String>> = Arc::new(
            processors
                .iter()
                .filter(|p| p.runs_one_at_a_time())
                .flat_map(|p| p.job_types())
                .map(str::to_string)
                .collect(),
        );
        let other_job_types: Arc<Vec<String>> = Arc::new(
            supported_job_types
                .iter()
                .filter(|t| !one_at_a_time_job_types.contains(t))
                .cloned()
                .collect(),
        );
        let one_at_a_time = Arc::new(OneAtATime::default());

        // Spawn worker tasks
        let mut tasks = self.tasks.lock();
        if let Some(ref mut join_set) = *tasks {
            for i in 0..self.config.max_workers {
                let concurrency = concurrency.clone();
                let cancellation_token = cancellation_token.clone();
                let job_queue = job_queue.clone();
                let processors = processors.clone();
                let notifier = job_queue.notifier();
                let supported_job_types = supported_job_types.clone();
                let one_at_a_time_job_types = one_at_a_time_job_types.clone();
                let other_job_types = other_job_types.clone();
                let one_at_a_time = one_at_a_time.clone();
                let active_workers = active_workers.clone();
                let dag_scheduler = dag_scheduler.clone();
                let dag_notify_tx = dag_notify_tx.clone();
                let event_tx = event_tx.clone();

                let runner = JobRunner {
                    worker_type,
                    job_queue: job_queue.clone(),
                    processors,
                    dag_scheduler,
                    dag_notify_tx,
                    event_tx,
                    active_workers,
                    job_timeout,
                    failure_persist_delay: execution::FailurePersistBackoff::INITIAL_DELAY,
                    shutdown: cancellation_token.clone(),
                    lingering: lingering.clone(),
                };

                join_set.spawn(async move {
                    debug!("{} worker {} started", worker_type, i);
                    let mut current_poll_interval = poll_interval;

                    loop {
                        // Register before inspecting the queue so an enqueue between an
                        // empty claim and parking cannot lose its broadcast wakeup.
                        let notified = notifier.notified();
                        tokio::pin!(notified);
                        notified.as_mut().enable();
                        if cancellation_token.is_cancelled() {
                            debug!("{} worker {} shutting down", worker_type, i);
                            break;
                        }

                        if let Some(permit) = concurrency.try_acquire() {
                            let turn = if one_at_a_time_job_types.is_empty() {
                                None
                            } else {
                                OneAtATimeTurn::try_take(&one_at_a_time, &notifier)
                            };
                            let filter_types = if supported_job_types.is_empty() {
                                None
                            } else if turn.is_some() {
                                Some(supported_job_types.as_slice())
                            } else {
                                Some(other_job_types.as_slice())
                            };
                            let claimed = if filter_types.is_some_and(<[String]>::is_empty) {
                                None
                            } else {
                                match job_queue.dequeue(filter_types).await {
                                    Ok(claimed) => claimed,
                                    Err(e) => {
                                        error!("Error dequeuing job: {}", e);
                                        None
                                    }
                                }
                            };

                            if let Some((job, attempt)) = claimed {
                                // Keep the turn only for the job that needs it.
                                let turn = match turn {
                                    Some(turn)
                                        if !one_at_a_time_job_types.contains(&job.job_type) =>
                                    {
                                        turn.pass_on();
                                        None
                                    }
                                    turn => turn,
                                };
                                current_poll_interval = poll_interval;
                                debug!(
                                    "{} worker {} processing job {} ({})",
                                    worker_type, i, job.id, job.job_type
                                );
                                runner.run(job, attempt).await;
                                drop(turn);
                                drop(permit);
                                // A cache-backed queue and a short processor may both
                                // finish without yielding. Keep other tasks responsive
                                // while draining a backlog without a timer per job.
                                tokio::task::yield_now().await;
                                continue;
                            }
                            let next_ms = (current_poll_interval.as_millis() as u64)
                                .saturating_mul(2)
                                .min(max_poll_interval.as_millis() as u64);
                            current_poll_interval =
                                std::time::Duration::from_millis(next_ms.max(1));
                            drop(permit);
                        }

                        // Back off only when no work or admission capacity is available.
                        tokio::select! {
                            _ = cancellation_token.cancelled() => break,
                            _ = &mut notified => {},
                            _ = tokio::time::sleep(current_poll_interval) => {},
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

    /// Abort the worker tasks instead of waiting for their current job.
    ///
    /// [`Self::stop`] joins workers, so a job that runs up to
    /// `WorkerPoolConfig::job_timeout_secs` holds it. Aborting drops the job
    /// future, which kills any process the processor spawned through
    /// `processors::utils::run_command_with_logs` (process-tree containment); the join
    /// that follows proves those drops ran. Returns 0 when a concurrent
    /// [`Self::stop`] already owns the join set, in which case dropping that
    /// call's join set is what aborts the workers.
    ///
    /// Returns the number of worker tasks that were aborted.
    pub(crate) async fn abort(&self, deadline: tokio::time::Instant) -> usize {
        self.cancellation_token.cancel();

        let join_set = {
            let mut tasks = self.tasks.lock();
            tasks.take()
        };
        let Some(mut join_set) = join_set else {
            return 0;
        };

        let aborted = join_set.len();
        join_set.abort_all();
        while !join_set.is_empty() {
            match tokio::time::timeout_at(deadline, join_set.join_next()).await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        "{} worker pool did not settle {} aborted tasks before the reap deadline",
                        self.worker_type,
                        join_set.len()
                    );
                    break;
                }
            }
        }

        info!("{} worker pool aborted", self.worker_type);
        aborted
    }

    /// Get the number of active workers.
    pub fn active_count(&self) -> usize {
        self.active_workers.load(Ordering::SeqCst)
    }

    /// Get the worker type.
    pub fn worker_type(&self) -> WorkerType {
        self.worker_type
    }

    /// Attempts whose job ended at least `min_age` ago while blocking work they started still
    /// holds a worker.
    pub fn stalled_operations(&self, min_age: std::time::Duration) -> Vec<StalledOperation> {
        self.lingering.older_than(min_age)
    }
}
