use super::execution::{FailurePersistBackoff, JobFacts};
use super::lingering::{TIMED_OUT_ERROR, progress_clause};
use super::*;
use async_trait::async_trait;
use std::time::Duration;

use crate::pipeline::dag_scheduler::StepFailureKind;
use crate::pipeline::job_queue::{JobLogEntry, JobResult};
use crate::pipeline::processors::{ProcessorContext, ProcessorInput};
use crate::pipeline::progress::JobProgressSnapshot;
use crate::pipeline::{
    Job, JobFailureOutcome, JobPhase, JobStatus, ProcessorOutput, ProcessorType,
};

#[test]
fn latest_limit_wins_with_busy_and_independent_pools() {
    let cpu = WorkerPool::with_config(
        WorkerType::Cpu,
        WorkerPoolConfig {
            max_workers: 3,
            ..Default::default()
        },
    );
    let io = WorkerPool::with_config(
        WorkerType::Io,
        WorkerPoolConfig {
            max_workers: 2,
            ..Default::default()
        },
    );
    let held: Vec<_> = (0..3)
        .map(|_| cpu.concurrency.try_acquire().unwrap())
        .collect();
    cpu.set_desired_max_workers(0);
    cpu.set_desired_max_workers(2);
    cpu.set_desired_max_workers(1);
    assert_eq!(cpu.set_desired_max_workers(usize::MAX), 3);
    assert!(cpu.concurrency.try_acquire().is_none());
    drop(held);
    let held: Vec<_> = (0..3)
        .map(|_| cpu.concurrency.try_acquire().unwrap())
        .collect();
    assert!(cpu.concurrency.try_acquire().is_none());
    assert_eq!(io.desired_max_workers(), 2);
    let first_io = io.concurrency.try_acquire().unwrap();
    let second_io = io.concurrency.try_acquire().unwrap();
    io.set_desired_max_workers(0);
    drop((first_io, second_io));
    assert!(io.concurrency.try_acquire().is_none());
    assert_eq!(cpu.desired_max_workers(), 3);
    drop(held);

    let disabled = WorkerPool::with_config(
        WorkerType::Cpu,
        WorkerPoolConfig {
            max_workers: 0,
            ..Default::default()
        },
    );
    assert_eq!(disabled.set_desired_max_workers(1), 0);
    assert!(disabled.concurrency.try_acquire().is_none());
}

#[tokio::test]
async fn aborted_claim_returns_admission_capacity() {
    let concurrency = Arc::new(WorkerConcurrency::new(2));
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let task_concurrency = concurrency.clone();
    let task = tokio::spawn(async move {
        let _permit = task_concurrency.try_acquire().unwrap();
        ready_tx.send(()).unwrap();
        std::future::pending::<()>().await;
    });
    tokio::time::timeout(Duration::from_secs(1), ready_rx)
        .await
        .unwrap()
        .unwrap();
    concurrency.set_limit(0);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(concurrency.state.lock().admitted, 0);
    concurrency.set_limit(1);
    let _permit = concurrency.try_acquire().unwrap();
    assert!(concurrency.try_acquire().is_none());
}

struct GatedProcessor(tokio::sync::mpsc::UnboundedSender<tokio::sync::oneshot::Sender<()>>);

#[async_trait]
impl Processor for GatedProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }
    fn job_types(&self) -> Vec<&'static str> {
        vec!["gated"]
    }
    fn name(&self) -> &'static str {
        "gated"
    }
    async fn process(
        &self,
        _input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        self.0.send(release_tx).unwrap();
        release_rx.await.unwrap();
        Ok(ProcessorOutput::default())
    }
}

/// Bound for waits on work that normally takes milliseconds, generous for a loaded machine.
const WAIT: Duration = Duration::from_secs(30);

async fn wait_for_admissions(pool: &WorkerPool, expected: usize) {
    tokio::time::timeout(WAIT, async {
        while pool.concurrency.state.lock().admitted != expected {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

/// Reports each started job's type and waits for its release.
struct GatedUploadProcessor {
    job_type: &'static str,
    one_at_a_time: bool,
    started: tokio::sync::mpsc::UnboundedSender<(String, tokio::sync::oneshot::Sender<()>)>,
}

#[async_trait]
impl Processor for GatedUploadProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Upload
    }
    fn runs_one_at_a_time(&self) -> bool {
        self.one_at_a_time
    }
    fn job_types(&self) -> Vec<&'static str> {
        vec![self.job_type]
    }
    fn name(&self) -> &'static str {
        self.job_type
    }
    async fn process(
        &self,
        _input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        self.started
            .send((self.job_type.to_string(), release_tx))
            .unwrap();
        release_rx.await.unwrap();
        Ok(ProcessorOutput::default())
    }
}

#[tokio::test]
async fn one_at_a_time_jobs_wait_without_holding_a_worker() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let pool = WorkerPool::with_config(
        WorkerType::Upload,
        WorkerPoolConfig {
            max_workers: 2,
            poll_interval_ms: 1,
            ..Default::default()
        },
    );
    // Distinct creation times fix the claim order, so a pool that claims the
    // second serial job before the parallel one always fails below.
    let created = jiff::Timestamp::now();
    for (i, job_type) in ["serial", "serial", "parallel"].into_iter().enumerate() {
        let mut job = Job::new(job_type, vec![], vec![], "streamer", "session");
        job.created_at = created + jiff::SignedDuration::from_millis(i as i64);
        queue.enqueue(job).await.unwrap();
    }
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    pool.start(
        queue,
        vec![
            Arc::new(GatedUploadProcessor {
                job_type: "serial",
                one_at_a_time: true,
                started: started_tx.clone(),
            }),
            Arc::new(GatedUploadProcessor {
                job_type: "parallel",
                one_at_a_time: false,
                started: started_tx,
            }),
        ],
    );

    let mut running = Vec::new();
    for _ in 0..2 {
        running.push(
            tokio::time::timeout(WAIT, started_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    // With two workers, the parallel job starts only if the queued second
    // serial job is not claimed by (and blocking) the other worker.
    let mut types: Vec<_> = running.iter().map(|(t, _)| t.as_str()).collect();
    types.sort_unstable();
    assert_eq!(types, ["parallel", "serial"]);

    let serial = running.iter().position(|(t, _)| t == "serial").unwrap();
    running.remove(serial).1.send(()).unwrap();
    let (next, release) = tokio::time::timeout(WAIT, started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, "serial");
    release.send(()).unwrap();
    running.pop().unwrap().1.send(()).unwrap();

    wait_for_admissions(&pool, 0).await;
    tokio::time::timeout(WAIT, pool.stop()).await.unwrap();
}

#[tokio::test]
async fn busy_workers_apply_reduction_to_queued_jobs() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let pool = WorkerPool::with_config(
        WorkerType::Cpu,
        WorkerPoolConfig {
            max_workers: 3,
            poll_interval_ms: 1,
            ..Default::default()
        },
    );
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    for _ in 0..6 {
        queue
            .enqueue(Job::new("gated", vec![], vec![], "streamer", "session"))
            .await
            .unwrap();
    }
    pool.start(queue, vec![Arc::new(GatedProcessor(started_tx))]);
    let mut releases = Vec::new();
    for _ in 0..3 {
        releases.push(
            tokio::time::timeout(WAIT, started_rx.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    pool.set_desired_max_workers(1);
    for remaining in (1..3).rev() {
        releases.pop().unwrap().send(()).unwrap();
        wait_for_admissions(&pool, remaining).await;
        assert!(pool.concurrency.try_acquire().is_none());
        assert!(started_rx.try_recv().is_err());
    }
    releases.pop().unwrap().send(()).unwrap();
    for _ in 0..3 {
        let release = tokio::time::timeout(WAIT, started_rx.recv())
            .await
            .unwrap()
            .unwrap();
        // Idle workers poll every millisecond, so at the old limit a second queued job
        // would start well within this window.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            started_rx.try_recv().is_err(),
            "only one queued job may run at the reduced limit"
        );
        release.send(()).unwrap();
    }
    wait_for_admissions(&pool, 0).await;
    tokio::time::timeout(WAIT, pool.stop()).await.unwrap();
    assert_eq!(pool.concurrency.state.lock().admitted, 0);
}

#[tokio::test]
async fn adaptive_pool_can_resume_from_zero_and_stop() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let pool = WorkerPool::with_config(
        WorkerType::Cpu,
        WorkerPoolConfig {
            max_workers: 2,
            poll_interval_ms: 1,
            adaptive: AdaptiveWorkerPoolConfig {
                enabled: true,
                min_workers: 0,
                interval_ms: 100,
                scale_down_idle_ticks: 1,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let (event_tx, mut event_rx) = broadcast::channel(8);
    pool.start_with_dag_scheduler(
        queue.clone(),
        vec![Arc::new(NoopProcessor)],
        None,
        None,
        Some(event_tx),
    );
    assert_eq!(pool.desired_max_workers(), 0);
    queue
        .enqueue(Job::new("noop", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while !matches!(
            event_rx.recv().await.unwrap(),
            PipelineEvent::JobCompleted { .. }
        ) {}
    })
    .await
    .unwrap();
    tokio::time::timeout(WAIT, async {
        while pool.desired_max_workers() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(WAIT, pool.stop()).await.unwrap();
    assert_eq!(pool.concurrency.state.lock().admitted, 0);
}

#[tokio::test]
async fn worker_drains_a_burst_without_poll_delays_and_wakes_after_idle() {
    // Far longer than `WAIT`: a worker that waited for its polling timer, between jobs or
    // to notice new work, could not finish within it.
    const POLL_INTERVAL: Duration = Duration::from_secs(60);
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let pool = WorkerPool::with_config(
        WorkerType::Cpu,
        WorkerPoolConfig {
            max_workers: 1,
            poll_interval_ms: POLL_INTERVAL.as_millis() as u64,
            ..Default::default()
        },
    );
    let (event_tx, mut event_rx) = broadcast::channel(256);
    for _ in 0..32 {
        queue
            .enqueue(Job::new("noop", vec![], vec![], "streamer", "session"))
            .await
            .unwrap();
    }
    pool.start_with_dag_scheduler(
        queue.clone(),
        vec![Arc::new(NoopProcessor)],
        None,
        None,
        Some(event_tx),
    );
    tokio::time::timeout(WAIT, async {
        let mut completed = 0;
        while completed < 32 {
            if matches!(
                event_rx.recv().await.unwrap(),
                PipelineEvent::JobCompleted { .. }
            ) {
                completed += 1;
            }
        }
    })
    .await
    .expect("a queued burst must not wait for a polling timer between jobs");
    wait_for_admissions(&pool, 0).await;
    // Let the worker find the queue empty and park in its backoff, so that only the
    // enqueue below can wake it in time.
    tokio::time::sleep(Duration::from_millis(100)).await;
    queue
        .enqueue(Job::new("noop", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        while !matches!(
            event_rx.recv().await.unwrap(),
            PipelineEvent::JobCompleted { .. }
        ) {}
    })
    .await
    .expect("enqueue must wake an idle worker without waiting for backoff");
    tokio::time::timeout(WAIT, pool.stop()).await.unwrap();
    assert_eq!(pool.concurrency.state.lock().admitted, 0);
    assert_eq!(pool.active_count(), 0);
}

struct NoopProcessor;

#[async_trait]
impl Processor for NoopProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["noop"]
    }

    async fn process(
        &self,
        _input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        Ok(ProcessorOutput {
            duration_secs: 1.5,
            ..Default::default()
        })
    }

    fn name(&self) -> &'static str {
        "noop"
    }
}

struct FailingProcessor;

#[async_trait]
impl Processor for FailingProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["failing"]
    }

    async fn process(
        &self,
        _input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        Err(srec_core::CoreError::PipelineError("boom".to_string()))
    }

    fn name(&self) -> &'static str {
        "failing"
    }
}

fn test_pool_config() -> WorkerPoolConfig {
    WorkerPoolConfig {
        max_workers: 1,
        job_timeout_secs: 30,
        poll_interval_ms: 10,
        adaptive: AdaptiveWorkerPoolConfig::default(),
    }
}

struct ObservedProcessor {
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    token: parking_lot::Mutex<Option<CancellationToken>>,
}

#[async_trait]
impl Processor for ObservedProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["observed"]
    }

    fn name(&self) -> &'static str {
        "observed"
    }

    async fn process(
        &self,
        _input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.token.lock() = Some(ctx.cancellation_token.clone());
        self.entered.notify_one();
        std::future::pending().await
    }
}

fn observed_runner(
    job_queue: Arc<JobQueue>,
    event_tx: broadcast::Sender<PipelineEvent>,
) -> JobRunner {
    JobRunner {
        worker_type: WorkerType::Cpu,
        job_queue,
        processors: vec![],
        dag_scheduler: None,
        dag_notify_tx: None,
        event_tx: Some(event_tx),
        active_workers: Arc::new(AtomicUsize::new(0)),
        avg_runtime_ms: Arc::new(AtomicU64::new(0)),
        job_timeout: Duration::from_secs(30),
        shutdown: CancellationToken::new(),
        lingering: Arc::default(),
    }
}

struct ContextHandoffProcessor(tokio::sync::mpsc::Sender<ProcessorContext>);

#[async_trait]
impl Processor for ContextHandoffProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["handoff"]
    }

    fn name(&self) -> &'static str {
        "handoff"
    }

    async fn process(
        &self,
        _input: &ProcessorInput,
        ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        self.0.try_send(ctx.clone()).unwrap();
        std::future::pending().await
    }
}

/// Short enough that a timed-out job ends promptly, long enough that the processor has
/// started by then even on a loaded machine.
const SHORT_JOB_TIMEOUT: Duration = Duration::from_millis(200);

/// Poll `execution` together with `until` and return what `until` yields, failing if the
/// execution ends first or `until` takes longer than `WAIT`. A test must not touch the
/// database while it holds an execution unpolled in the middle of a query: the fixture
/// has a single connection.
async fn drive_until<T>(
    execution: std::pin::Pin<&mut impl Future<Output = ()>>,
    until: impl Future<Output = T>,
) -> T {
    tokio::time::timeout(WAIT, async {
        tokio::select! {
            biased;
            value = until => value,
            () = execution => panic!("the execution ended while its work was retained"),
        }
    })
    .await
    .expect("the awaited state was not reached")
}

async fn wait_for_lingering(
    lingering: &LingeringAttempts,
    min_age: Duration,
) -> Vec<StalledOperation> {
    loop {
        let operations = lingering.older_than(min_age);
        if !operations.is_empty() {
            return operations;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn user_cancellation_during_timeout_log_drain_keeps_the_recorded_timeout() {
    assert_timeout_with_retained_sink(true).await;
}

async fn assert_timeout_with_retained_sink(cancel_during_drain: bool) {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let id = queue
        .enqueue(Job::new("handoff", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    let (events, mut event_rx) = broadcast::channel(8);
    let mut runner = observed_runner(queue.clone(), events);
    runner.job_timeout = SHORT_JOB_TIMEOUT;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let processor: Arc<dyn Processor> = Arc::new(ContextHandoffProcessor(sender));
    let execution = runner.execute_job(job, &processor, attempt);
    tokio::pin!(execution);
    let retained = drive_until(execution.as_mut(), receiver.recv())
        .await
        .unwrap();
    assert!(matches!(
        event_rx.try_recv(),
        Ok(PipelineEvent::JobStarted { .. })
    ));

    // The deadline is recorded and reported while the retained sender still holds the
    // worker, which then waits for it without a query in flight.
    let failed = drive_until(execution.as_mut(), event_rx.recv())
        .await
        .unwrap();
    assert!(
        matches!(&failed, PipelineEvent::JobFailed { error, .. } if error.starts_with(TIMED_OUT_ERROR)),
        "unexpected event: {failed:?}"
    );
    assert!(
        retained.cancellation_token.is_cancelled(),
        "processor helpers must see timeout before the worker waits for their log sender"
    );
    let stored = queue.get_job(&id).await.unwrap().unwrap();
    assert_eq!(stored.status, JobStatus::Failed);
    assert!(
        stored
            .error
            .as_deref()
            .is_some_and(|error| error.starts_with(TIMED_OUT_ERROR)),
        "unexpected stored error: {:?}",
        stored.error
    );
    assert_eq!(runner.active_workers.load(Ordering::SeqCst), 1);
    if cancel_during_drain {
        assert!(queue.cancel_job(&id).await.is_err());
    }
    drop(retained);
    tokio::time::timeout(WAIT, execution).await.unwrap();
    assert_eq!(
        queue.get_job(&id).await.unwrap().unwrap().status,
        JobStatus::Failed
    );
    assert!(event_rx.try_recv().is_err());
    assert_eq!(runner.active_workers.load(Ordering::SeqCst), 0);
}

struct CompletingProcessor;

#[async_trait]
impl Processor for CompletingProcessor {
    fn processor_type(&self) -> ProcessorType {
        ProcessorType::Cpu
    }

    fn job_types(&self) -> Vec<&'static str> {
        vec!["handoff"]
    }

    fn name(&self) -> &'static str {
        "completing"
    }

    async fn process(
        &self,
        _input: &ProcessorInput,
        _ctx: &ProcessorContext,
    ) -> srec_core::CoreResult<ProcessorOutput> {
        Ok(ProcessorOutput::default())
    }
}

#[tokio::test]
async fn ended_job_is_reported_stalled_until_its_retained_work_returns() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let id = queue
        .enqueue(Job::new(
            "handoff",
            vec!["/recordings/part.flv".to_string()],
            vec![],
            "streamer",
            "session",
        ))
        .await
        .unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    let (events, mut event_rx) = broadcast::channel(8);
    let mut runner = observed_runner(queue.clone(), events);
    runner.job_timeout = SHORT_JOB_TIMEOUT;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let processor: Arc<dyn Processor> = Arc::new(ContextHandoffProcessor(sender));
    let started = tokio::time::Instant::now();
    let execution = runner.execute_job(job, &processor, attempt);
    tokio::pin!(execution);
    let retained = drive_until(execution.as_mut(), receiver.recv())
        .await
        .unwrap();

    // Listed from the timeout on, which the worker records before it waits for the
    // retained sender.
    while !matches!(
        drive_until(execution.as_mut(), event_rx.recv())
            .await
            .unwrap(),
        PipelineEvent::JobFailed { .. }
    ) {}
    let ended = runner.lingering.older_than(Duration::ZERO);
    assert_eq!(ended.len(), 1);
    // Measured from the end of the job, not from its start, and reported only once the
    // work has outlived its job by the threshold.
    let age = ended[0].stalled_for();
    assert!(
        age + SHORT_JOB_TIMEOUT <= started.elapsed(),
        "age {age:?} counts from before the job timed out"
    );
    assert!(
        runner
            .lingering
            .older_than(STALLED_OPERATION_THRESHOLD)
            .is_empty()
    );
    let threshold = Duration::from_millis(100);
    let stalled = drive_until(
        execution.as_mut(),
        wait_for_lingering(&runner.lingering, threshold),
    )
    .await;
    assert_eq!(stalled.len(), 1);
    assert_eq!(
        (
            stalled[0].job_id.as_str(),
            stalled[0].processor,
            stalled[0].input_file_name.as_deref(),
        ),
        (id.as_str(), "handoff", Some("part.flv"))
    );

    // A retry runs a successor under the same job ID. Finishing it must not clear the old
    // attempt, whose work still holds a worker.
    queue.retry_job(&id).await.unwrap();
    let (successor, successor_attempt) = queue.dequeue(None).await.unwrap().unwrap();
    let (successor_events, _successor_rx) = broadcast::channel(8);
    let mut successor_runner = observed_runner(queue.clone(), successor_events);
    successor_runner.lingering = runner.lingering.clone();
    let completing: Arc<dyn Processor> = Arc::new(CompletingProcessor);
    successor_runner
        .execute_job(successor, &completing, successor_attempt)
        .await;
    assert_eq!(
        queue.get_job(&id).await.unwrap().unwrap().status,
        JobStatus::Completed
    );
    assert_eq!(runner.lingering.older_than(Duration::ZERO).len(), 1);

    drop(retained);
    tokio::time::timeout(WAIT, execution).await.unwrap();
    assert!(runner.lingering.older_than(Duration::ZERO).is_empty());
}

#[tokio::test]
async fn forced_worker_abort_cancels_processor_and_releases_active_count() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let id = queue
        .enqueue(Job::new("handoff", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let pool = WorkerPool::with_config(WorkerType::Cpu, test_pool_config());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    pool.start(
        queue.clone(),
        vec![Arc::new(ContextHandoffProcessor(sender))],
    );
    let retained = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pool.active_count(), 1);
    pool.abort(tokio::time::Instant::now() + Duration::from_secs(2))
        .await;

    assert_eq!(
        (
            retained.cancellation_token.is_cancelled(),
            pool.active_count()
        ),
        (true, 0),
        "aborting must cancel processor helpers and release active accounting"
    );
    // Interrupted work remains recoverable; abort must not claim a durable cancellation.
    assert_eq!(
        queue.get_job(&id).await.unwrap().unwrap().status,
        JobStatus::Processing
    );
}

#[tokio::test]
async fn forced_worker_abort_does_not_detach_log_collector() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    queue
        .enqueue(Job::new("handoff", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let pool = WorkerPool::with_config(WorkerType::Cpu, test_pool_config());
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    pool.start(
        queue.clone(),
        vec![Arc::new(ContextHandoffProcessor(sender))],
    );
    let retained = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    pool.abort(tokio::time::Instant::now() + Duration::from_secs(2))
        .await;
    let weak_queue = Arc::downgrade(&queue);
    drop(queue);
    tokio::time::timeout(Duration::from_secs(1), async {
        while weak_queue.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("log collector must release its queue even while a helper retains the sink");
    drop(retained);
}

#[test]
fn failure_persist_backoff_widens_to_its_cap_and_rate_limits_warnings() {
    let mut backoff = FailurePersistBackoff::default();
    let schedule: Vec<_> = std::iter::repeat_with(|| backoff.on_failure())
        .take(8)
        .map(|(delay, warn)| (delay.as_secs(), warn))
        .collect();
    assert_eq!(
        schedule,
        [
            (1, true),
            (2, false),
            (4, false),
            (8, false),
            (16, false),
            (30, true),
            (30, true),
            (30, true),
        ]
    );
    assert_eq!(backoff.failures, 8);
}

#[tokio::test]
async fn failed_status_writes_retry_without_rerunning_and_allow_cancel_or_shutdown() {
    use crate::database::models::{
        DagExecutionStatus, DagPipelineDefinition, DagStep, DagStepStatus, PipelineStep,
        StepRetryPolicy,
    };
    use crate::database::repositories::{
        DagRepository, JobRepository, SqlxDagRepository, SqlxJobRepository,
    };
    use crate::database::test_support::SqlTrace;
    use crate::pipeline::dag_scheduler::DagRunContext;

    struct CountedFailure(Arc<AtomicUsize>);
    #[async_trait]
    impl Processor for CountedFailure {
        fn processor_type(&self) -> ProcessorType {
            ProcessorType::Cpu
        }
        fn job_types(&self) -> Vec<&'static str> {
            vec!["failing"]
        }
        fn name(&self) -> &'static str {
            "failing"
        }
        async fn process(
            &self,
            _: &ProcessorInput,
            _: &ProcessorContext,
        ) -> srec_core::CoreResult<ProcessorOutput> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(srec_core::CoreError::PipelineError(
                "processor failed".into(),
            ))
        }
    }

    for ending in ["restore", "cancel", "shutdown"] {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
        let scheduler = Arc::new(DagScheduler::new(queue.clone(), dags.clone(), jobs.clone()));
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "failed status write",
                    vec![
                        DagStep::new("A", PipelineStep::inline("failing", serde_json::json!({})))
                            .with_retry(StepRetryPolicy {
                                max_attempts: 2,
                                backoff_secs: 30,
                            }),
                    ],
                ),
                &["input.flv".into()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
        let job_id = job.id.clone();
        let step_id = job.dag_step_execution_id.clone().unwrap();
        sqlx::query("CREATE TRIGGER reject_failed BEFORE UPDATE OF status ON job WHEN NEW.status = 'FAILED' BEGIN SELECT RAISE(ABORT, 'injected failure'); END").execute(&pool).await.unwrap();
        let trace = SqlTrace::install(&pool).await;
        let (events, mut receiver) = broadcast::channel(16);
        let mut runner = observed_runner(queue.clone(), events);
        runner.dag_scheduler = Some(scheduler.clone());
        let runner = Arc::new(runner);
        let calls = Arc::new(AtomicUsize::new(0));
        let processor: Arc<dyn Processor> = Arc::new(CountedFailure(calls.clone()));
        let running = runner.clone();
        let task = tokio::spawn(async move {
            running.execute_job(job, &processor, attempt).await;
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !trace
                .statements()
                .iter()
                .any(|sql| sql.starts_with("UPDATE job SET status = ?, completed_at"))
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!task.is_finished());
        assert_eq!(
            jobs.get_job(&job_id).await.unwrap().get_status(),
            Some(JobStatus::Processing)
        );
        assert_eq!(
            dags.get_step(&step_id).await.unwrap().get_status(),
            Some(DagStepStatus::Processing)
        );
        assert_eq!(
            dags.get_dag(&created.dag_id).await.unwrap().get_status(),
            Some(DagExecutionStatus::Processing)
        );
        assert_eq!(queue.depth(), 1);
        while let Ok(event) = receiver.try_recv() {
            assert!(!matches!(event, PipelineEvent::JobFailed { .. }));
        }

        match ending {
            "restore" => {
                sqlx::query("DROP TRIGGER reject_failed")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            "cancel" => {
                scheduler.cancel_dag(&created.dag_id).await.unwrap();
            }
            _ => runner.shutdown.cancel(),
        }
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(runner.active_workers.load(Ordering::SeqCst), 0);
        let stored = jobs.get_job(&job_id).await.unwrap();
        let failed_events = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter(|event| matches!(event, PipelineEvent::JobFailed { .. }))
            .count();
        match ending {
            "restore" => {
                assert_eq!(stored.get_status(), Some(JobStatus::Failed));
                assert!(stored.retry_after.is_some());
                assert_eq!(
                    dags.get_step(&step_id).await.unwrap().get_status(),
                    Some(DagStepStatus::Processing)
                );
                assert_eq!(queue.depth(), 0);
                assert!(queue.get_cancellation_token(&job_id).await.is_none());
                assert_eq!(failed_events, 0);
            }
            "cancel" => {
                assert_eq!(stored.get_status(), Some(JobStatus::Cancelled));
                assert_eq!(
                    dags.get_dag(&created.dag_id).await.unwrap().get_status(),
                    Some(DagExecutionStatus::Cancelled)
                );
                assert_eq!(queue.depth(), 0);
                assert!(queue.get_cancellation_token(&job_id).await.is_none());
                assert_eq!(failed_events, 0);
            }
            _ => {
                assert_eq!(stored.get_status(), Some(JobStatus::Processing));
                assert_eq!(
                    dags.get_step(&step_id).await.unwrap().get_status(),
                    Some(DagStepStatus::Processing)
                );
                assert_eq!(failed_events, 0);
                sqlx::query("DROP TRIGGER reject_failed")
                    .execute(&pool)
                    .await
                    .unwrap();
                assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
                assert_eq!(queue.recover_jobs().await.unwrap(), 1);
                assert_eq!(queue.dequeue(None).await.unwrap().unwrap().0.id, job_id);
            }
        }
        queue.stop_progress_aggregator().await;
    }
}

#[tokio::test]
async fn late_failures_preserve_terminal_jobs_and_do_not_emit_events() {
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let (events, mut receiver) = broadcast::channel(8);
    let runner = observed_runner(queue.clone(), events);
    for terminal in [
        JobStatus::Cancelled,
        JobStatus::Completed,
        JobStatus::Failed,
    ] {
        let job = Job::new("noop", vec!["input.flv".into()], vec![], "", "");
        queue.enqueue(job.clone()).await.unwrap();
        let (_, attempt) = queue.dequeue(None).await.unwrap().unwrap();
        match terminal {
            JobStatus::Cancelled => {
                queue.cancel_job(&job.id).await.unwrap();
            }
            JobStatus::Completed => {
                queue
                    .complete(
                        &job.id,
                        JobResult {
                            outputs: vec!["output.mp4".into()],
                            duration_secs: 1.0,
                            metadata: None,
                            uploads: vec![],
                            logs: vec![],
                            input_size_bytes: None,
                            output_size_bytes: None,
                        },
                    )
                    .await
                    .unwrap();
            }
            JobStatus::Failed => {
                assert_eq!(
                    queue.fail(&job.id, "first failure").await.unwrap(),
                    JobFailureOutcome::Transitioned
                );
            }
            _ => unreachable!(),
        }
        let facts = JobFacts {
            id: job.id.clone(),
            job_type: job.job_type,
            processor_name: "noop",
            dag_step_execution_id: None,
            retry_count: 0,
            attempt,
        };
        runner
            .finish_failed(
                &facts,
                "late failure",
                StepFailureKind::ProcessorError,
                &[],
                None,
            )
            .await;
        assert!(receiver.try_recv().is_err());
        assert_eq!(
            queue
                .fail_execution_start(&job.id, "late startup failure")
                .await
                .unwrap(),
            JobFailureOutcome::Unchanged
        );
        let stored = queue.get_job(&job.id).await.unwrap().unwrap();
        assert_eq!(stored.status, terminal);
        if terminal == JobStatus::Failed {
            assert_eq!(stored.error.as_deref(), Some("first failure"));
        }
        if terminal == JobStatus::Completed {
            assert_eq!(stored.outputs, ["output.mp4"]);
        }
        assert_eq!(queue.depth(), 0);
        assert!(queue.get_cancellation_token(&job.id).await.is_none());
    }
    queue.stop_progress_aggregator().await;
}

#[tokio::test]
async fn rejected_attempt_metadata_never_invokes_processor_or_rewrites_history() {
    use crate::database::models::JobDbModel;
    use crate::database::repositories::{JobRepository, SqlxJobRepository};

    for stored in [
        None,
        Some("malformed"),
        Some("null"),
        Some("[]"),
        Some(r#"{"items_produced": false}"#),
    ] {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repository = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            repository.clone(),
        ));
        let mut model = JobDbModel::new_with_input("gated", "input.flv", 0, None, None, "{}");
        model.execution_info = stored.map(str::to_owned);
        repository.create_job(&model).await.unwrap();
        if stored.is_none() {
            sqlx::query("CREATE TRIGGER reject_attempt BEFORE UPDATE OF execution_info ON job BEGIN SELECT RAISE(ABORT, 'rejected attempt'); END")
                .execute(&pool).await.unwrap();
        }
        let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
        let (events, mut event_rx) = broadcast::channel(8);
        let runner = observed_runner(queue.clone(), events);
        let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
        let processor: Arc<dyn Processor> = Arc::new(GatedProcessor(started_tx));
        tokio::time::timeout(
            Duration::from_secs(5),
            runner.execute_job(job, &processor, attempt),
        )
        .await
        .unwrap();
        assert!(started_rx.try_recv().is_err());
        let failed = repository.get_job(&model.id).await.unwrap();
        assert_eq!(failed.status, JobStatus::Failed.as_str());
        assert_eq!(failed.execution_info.as_deref(), stored);
        assert!(
            failed
                .error
                .as_deref()
                .unwrap()
                .contains("Failed to persist execution start")
        );
        assert!(matches!(
            event_rx.try_recv().unwrap(),
            PipelineEvent::JobFailed { .. }
        ));
        assert!(event_rx.try_recv().is_err());
        assert!(queue.get_cancellation_token(&model.id).await.is_none());
        assert_eq!(runner.active_workers.load(Ordering::SeqCst), 0);
        pool.close().await;
    }
}

#[tokio::test]
async fn fresh_worker_persists_attempt_marker_before_processor_runs() {
    use crate::database::models::JobDbModel;
    use crate::database::repositories::{JobRepository, SqlxJobRepository};
    use crate::pipeline::job_queue::JobExecutionInfo;

    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let repository = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        repository.clone(),
    ));
    let model = JobDbModel::new_with_input("gated", "input.flv", 0, None, None, "{}");
    assert!(model.execution_info.is_none());
    repository.create_job(&model).await.unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    let (events, _) = broadcast::channel(8);
    let runner = observed_runner(queue, events);
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let processor: Arc<dyn Processor> = Arc::new(GatedProcessor(started_tx));
    let work = runner.execute_job(job, &processor, attempt);
    tokio::pin!(work);
    let release = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            () = &mut work => panic!("processor must wait for release"),
            release = started_rx.recv() => release.unwrap(),
        }
    })
    .await
    .unwrap();
    let stored = repository.get_job(&model.id).await.unwrap();
    assert_eq!(stored.status, JobStatus::Processing.as_str());
    let info: JobExecutionInfo =
        serde_json::from_str(stored.execution_info.as_deref().unwrap()).unwrap();
    assert_eq!(info.current_processor.as_deref(), Some("gated"));
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
async fn failed_retry_preserves_published_artifacts_and_prior_execution_history() {
    use crate::database::models::{JobDbModel, Pagination};
    use crate::database::repositories::{JobRepository, SqlxJobRepository};
    use crate::pipeline::job_queue::JobExecutionInfo;

    for retry_count in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let published = directory.path().join("previously-published.mp4");
        tokio::fs::write(&published, b"successful earlier artifact")
            .await
            .unwrap();
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repository = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            repository.clone(),
        ));
        let mut info = JobExecutionInfo {
            current_processor: Some("previous".to_owned()),
            input_size_bytes: Some(123),
            output_size_bytes: Some(456),
            ..Default::default()
        };
        info.items_produced
            .push(published.to_string_lossy().into_owned());
        let mut model = JobDbModel::new_with_input("failing", "input.flv", 0, None, None, "{}");
        model.retry_count = retry_count;
        model.execution_info = Some(serde_json::to_string(&info).unwrap());
        repository.create_job(&model).await.unwrap();
        let prior_log = JobLogEntry::warn("prior attempt warning");
        queue
            .append_log_entry(&model.id, std::slice::from_ref(&prior_log))
            .await
            .unwrap();
        info.logs.push_back(prior_log);
        info.log_lines_total = 1;
        info.log_warn_count = 1;
        repository
            .update_job_execution_info(&model.id, &serde_json::to_string(&info).unwrap())
            .await
            .unwrap();
        let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
        let (events, _) = broadcast::channel(8);
        let runner = observed_runner(queue.clone(), events);
        let processor: Arc<dyn Processor> = Arc::new(FailingProcessor);
        tokio::time::timeout(
            Duration::from_secs(5),
            runner.execute_job(job, &processor, attempt),
        )
        .await
        .unwrap();
        assert_eq!(
            tokio::fs::read(&published).await.unwrap(),
            b"successful earlier artifact"
        );
        let failed = repository.get_job(&model.id).await.unwrap();
        assert_eq!(failed.status, JobStatus::Failed.as_str());
        let stored: JobExecutionInfo =
            serde_json::from_str(failed.execution_info.as_deref().unwrap()).unwrap();
        assert_eq!(stored.current_processor.as_deref(), Some("failing"));
        assert_eq!(stored.items_produced, info.items_produced);
        assert_eq!(stored.input_size_bytes, Some(123));
        assert_eq!(stored.output_size_bytes, Some(456));
        assert_eq!(stored.log_lines_total, 2);
        assert_eq!(stored.log_warn_count, 1);
        assert_eq!(stored.log_error_count, 1);
        let (rows, total) = repository
            .list_execution_logs(&model.id, &Pagination::new(100, 0))
            .await
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(
            rows.iter()
                .filter(|row| row.message.as_deref() == Some("prior attempt warning"))
                .count(),
            1
        );
        pool.close().await;
    }
}

#[tokio::test]
async fn withdrawn_job_never_starts_execution() {
    use crate::database::repositories::JobRepository;

    let repo = crate::database::test_support::job_repository().await;
    let queue = Arc::new(JobQueue::with_repository(Default::default(), repo.clone()));
    let job_id = queue
        .enqueue(Job::new("observed", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    repo.delete_job(&job_id).await.unwrap();
    queue.forget_jobs(std::slice::from_ref(&job_id));

    let (event_tx, mut event_rx) = broadcast::channel(8);
    let runner = observed_runner(queue.clone(), event_tx);
    let observed = Arc::new(ObservedProcessor {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        token: parking_lot::Mutex::new(None),
    });
    let processor: Arc<dyn Processor> = observed.clone();
    tokio::time::timeout(
        Duration::from_secs(1),
        runner.execute_job(job, &processor, attempt),
    )
    .await
    .expect("withdrawn job must return before spawning execution tasks");

    assert_eq!(observed.calls.load(Ordering::SeqCst), 0);
    assert_eq!(runner.active_workers.load(Ordering::SeqCst), 0);
    assert!(matches!(
        event_rx.try_recv(),
        Err(broadcast::error::TryRecvError::Empty)
    ));
    assert!(queue.get_job(&job_id).await.unwrap().is_none());
    assert_eq!(queue.depth(), 0);
}

#[tokio::test]
async fn forgetting_started_job_cancels_retained_processor_token() {
    use crate::database::repositories::JobRepository;

    let repo = crate::database::test_support::job_repository().await;
    let queue = Arc::new(JobQueue::with_repository(Default::default(), repo.clone()));
    let job_id = queue
        .enqueue(Job::new("observed", vec![], vec![], "streamer", "session"))
        .await
        .unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    let (event_tx, mut event_rx) = broadcast::channel(8);
    let runner = observed_runner(queue.clone(), event_tx);
    let active = runner.active_workers.clone();
    let observed = Arc::new(ObservedProcessor {
        calls: AtomicUsize::new(0),
        entered: tokio::sync::Notify::new(),
        token: parking_lot::Mutex::new(None),
    });
    let processor: Arc<dyn Processor> = observed.clone();
    let execution = tokio::spawn(async move { runner.execute_job(job, &processor, attempt).await });
    tokio::time::timeout(Duration::from_secs(1), observed.entered.notified())
        .await
        .expect("processor should start");
    assert_eq!(active.load(Ordering::SeqCst), 1);
    repo.delete_job(&job_id).await.unwrap();
    queue.forget_jobs(std::slice::from_ref(&job_id));
    assert!(observed.token.lock().as_ref().unwrap().is_cancelled());
    tokio::time::timeout(Duration::from_secs(1), execution)
        .await
        .expect("worker and log collector should settle after withdrawal")
        .unwrap();

    assert_eq!(observed.calls.load(Ordering::SeqCst), 1);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert!(matches!(
        event_rx.try_recv(),
        Ok(PipelineEvent::JobStarted { .. })
    ));
    assert!(event_rx.try_recv().is_err());
    assert!(queue.get_job(&job_id).await.unwrap().is_none());
    assert_eq!(queue.depth(), 0);
}

/// The worker is the only place a job's start and terminal state are known, so it is what
/// feeds the `pipeline_started` / `pipeline_completed` notification subscriptions.
#[tokio::test]
async fn test_worker_emits_started_and_completed_events() {
    let job_queue = Arc::new(JobQueue::with_repository(
        Default::default(),
        crate::database::test_support::job_repository().await,
    ));
    let (event_tx, mut event_rx) = broadcast::channel(16);
    let pool = WorkerPool::with_config(WorkerType::Cpu, test_pool_config());

    pool.start_with_dag_scheduler(
        job_queue.clone(),
        vec![Arc::new(NoopProcessor)],
        None,
        None,
        Some(event_tx),
    );

    let job = Job::new(
        "noop",
        vec!["/input".to_string()],
        vec![],
        "streamer-1",
        "session-1",
    );
    let job_id = job_queue.enqueue(job).await.unwrap();

    let mut started: Option<(String, String)> = None;
    let mut completed: Option<(String, f64)> = None;
    tokio::time::timeout(Duration::from_secs(5), async {
        while started.is_none() || completed.is_none() {
            match event_rx.recv().await.unwrap() {
                PipelineEvent::JobStarted {
                    job_id,
                    streamer_id,
                    ..
                } => started = Some((job_id, streamer_id)),
                PipelineEvent::JobCompleted {
                    job_id,
                    duration_secs,
                    ..
                } => completed = Some((job_id, duration_secs)),
                _ => {}
            }
        }
    })
    .await
    .expect("worker emits JobStarted and JobCompleted");

    // streamer_id has to come from the job itself: DAG step jobs are enqueued through
    // enqueue_existing and never emit JobEnqueued for a listener to correlate against.
    assert_eq!(started, Some((job_id.clone(), "streamer-1".to_string())));
    assert_eq!(completed, Some((job_id, 1.5)));

    pool.stop().await;
}

/// A timed-out job's error names the phase it was stuck in and how long
/// ago its progress last moved.
#[test]
fn timeout_progress_clause_names_the_phase() {
    let mut snapshot = JobProgressSnapshot::new(crate::pipeline::ProgressKind::BaiduPcs);
    snapshot.phase = Some(JobPhase::Retrying);
    snapshot.updated_at = jiff::Timestamp::now() - jiff::SignedDuration::from_mins(59);

    let clause = progress_clause(&snapshot);
    assert!(
        clause.starts_with(", progress waiting to retry, last updated 59m "),
        "{clause}"
    );
}

#[tokio::test]
async fn non_retryable_step_failures_skip_dag_retry_and_notify_immediately() {
    use crate::database::models::{
        DagExecutionStatus, DagPipelineDefinition, DagStep, DagStepStatus, PipelineStep,
        StepRetryPolicy,
    };
    use crate::database::repositories::{
        DagRepository, JobRepository, SqlxDagRepository, SqlxJobRepository,
    };
    use crate::pipeline::dag_scheduler::DagRunContext;

    #[derive(Clone, Copy)]
    enum Mode {
        NonRetryableErr,
        NonRetryablePartialOutput,
        LegacyMissingStateFlag,
    }

    struct FailingProcessor {
        mode: Mode,
        saw_step_retry: Arc<AtomicBool>,
    }
    #[async_trait]
    impl Processor for FailingProcessor {
        fn processor_type(&self) -> ProcessorType {
            ProcessorType::Cpu
        }
        fn job_types(&self) -> Vec<&'static str> {
            vec!["non_retryable_test"]
        }
        fn name(&self) -> &'static str {
            "non_retryable_test"
        }
        async fn process(
            &self,
            _: &ProcessorInput,
            ctx: &ProcessorContext,
        ) -> srec_core::CoreResult<ProcessorOutput> {
            self.saw_step_retry
                .store(ctx.has_step_retry, Ordering::SeqCst);
            match self.mode {
                Mode::NonRetryableErr | Mode::LegacyMissingStateFlag => Err(
                    srec_core::CoreError::NonRetryablePipeline("invalid_grant".into()),
                ),
                Mode::NonRetryablePartialOutput => Ok(ProcessorOutput {
                    succeeded_inputs: vec!["ok.mp4".into()],
                    failed_inputs: vec![("bad.mp4".into(), "illegal name".into())],
                    non_retryable: true,
                    ..Default::default()
                }),
            }
        }
    }

    for mode in [
        Mode::NonRetryableErr,
        Mode::NonRetryablePartialOutput,
        Mode::LegacyMissingStateFlag,
    ] {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
        let scheduler = Arc::new(DagScheduler::new(queue.clone(), dags.clone(), jobs.clone()));
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "non-retryable",
                    vec![
                        DagStep::new(
                            "upload",
                            PipelineStep::inline("non_retryable_test", serde_json::json!({})),
                        )
                        .with_retry(StepRetryPolicy {
                            max_attempts: 3,
                            backoff_secs: 60,
                        }),
                    ],
                ),
                &["input.flv".into()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        if matches!(mode, Mode::LegacyMissingStateFlag) {
            // Simulate a pre-upgrade workflow row lacking `has_step_retry` in `job.state`.
            sqlx::query("UPDATE job SET state = '{}' WHERE id = ?")
                .bind(&created.root_job_ids[0])
                .execute(&pool)
                .await
                .unwrap();
        }
        let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
        let job_id = job.id.clone();
        let step_id = job.dag_step_execution_id.clone().unwrap();

        let (events, mut receiver) = broadcast::channel(16);
        let mut runner = observed_runner(queue.clone(), events);
        runner.dag_scheduler = Some(scheduler.clone());
        let saw_step_retry = Arc::new(AtomicBool::new(false));
        let processor: Arc<dyn Processor> = Arc::new(FailingProcessor {
            mode,
            saw_step_retry: saw_step_retry.clone(),
        });
        runner.execute_job(job, &processor, attempt).await;

        assert!(
            saw_step_retry.load(Ordering::SeqCst),
            "processor must see has_step_retry=true (including legacy workflow rows)"
        );
        let stored = jobs.get_job(&job_id).await.unwrap();
        assert_eq!(stored.get_status(), Some(JobStatus::Failed));
        assert_eq!(stored.retry_after, None);
        assert_eq!(
            dags.get_step(&step_id).await.unwrap().get_status(),
            Some(DagStepStatus::Failed)
        );
        assert_eq!(
            dags.get_dag(&created.dag_id).await.unwrap().get_status(),
            Some(DagExecutionStatus::Failed)
        );
        let failed_events = std::iter::from_fn(|| receiver.try_recv().ok())
            .filter(|event| matches!(event, PipelineEvent::JobFailed { .. }))
            .count();
        assert_eq!(failed_events, 1);

        queue.stop_progress_aggregator().await;
    }
}

#[tokio::test]
async fn baidupcs_worker_retry_persists_account_bound_carry_forward_in_sqlite() {
    use crate::database::models::{
        DagPipelineDefinition, DagStep, JobStatus, PipelineStep, StepRetryPolicy,
    };
    use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
    use crate::pipeline::dag_scheduler::{DagRunContext, DagScheduler};
    use crate::pipeline::job_queue::JobQueue;
    use crate::pipeline::processors::BaiduPcsProcessor;

    let temp_dir = tempfile::tempdir().unwrap();
    let cfg_dir = temp_dir.path().join("cfg");
    std::fs::create_dir(&cfg_dir).unwrap();
    let file_a = temp_dir.path().join("a.flv");
    let file_b = temp_dir.path().join("b.flv");
    std::fs::write(&file_a, b"video-a").unwrap();
    std::fs::write(&file_b, b"video-b").unwrap();

    // Compile a tiny cross-platform fake BaiduPCS-Go binary so `who` and
    // `upload` print exact UTF-8 markers through ProcessBaiduPcsCommandRunner.
    let fake_src = r#"
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let cfg = PathBuf::from(env::var("BAIDUPCS_GO_CONFIG_DIR").expect("config dir"));
    match args.first().map(String::as_str) {
        Some("who") => {
            let uid = fs::read_to_string(cfg.join("uid.txt")).unwrap_or_default();
            println!("当前帐号 uid: {}, 用户名: tester, 性别: 未知, 年龄: 0.0", uid.trim());
        }
        Some("upload") => {
            let fail_name = fs::read_to_string(cfg.join("fail.txt")).unwrap_or_default();
            let fail_name = fail_name.trim();
            let remote_dir = args.last().expect("remote dir");
            let mut files = Vec::new();
            let mut i = 1;
            while i + 1 < args.len() {
                if args[i] == "--policy" {
                    i += 2;
                    continue;
                }
                if args[i].starts_with("--") {
                    i += 1;
                    continue;
                }
                files.push(args[i].clone());
                i += 1;
            }
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(cfg.join("invocations.log"))
                .unwrap();
            writeln!(log, "{}", files.join("|")).unwrap();
            for (idx, path) in files.iter().enumerate() {
                let id = idx + 1;
                println!("[{id}] 加入上传队列: {path}");
                let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
                if !fail_name.is_empty() && name == fail_name {
                    println!("[{id}] 上传文件失败, 网络超时");
                } else {
                    println!("[{id}] 上传文件成功, 保存到网盘路径: {}/{}", remote_dir.trim_end_matches('/'), name);
                }
            }
            println!("上传结束, 时间: 1s, 总大小: 14B");
        }
        other => panic!("unexpected subcommand: {other:?}"),
    }
}
"#;
    let fake_binary = tokio::task::spawn_blocking({
        let dir = temp_dir.path().to_path_buf();
        move || {
            let src_path = dir.join("fake_baidupcs.rs");
            std::fs::write(&src_path, fake_src).unwrap();
            let bin_path = dir.join(format!("fake_baidupcs{}", std::env::consts::EXE_SUFFIX));
            let output = process_utils::std_command(
                std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()),
            )
            .arg("--edition=2024")
            .arg(&src_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            bin_path
        }
    })
    .await
    .unwrap();

    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
    let scheduler = Arc::new(DagScheduler::new(queue.clone(), dags.clone(), jobs.clone()));
    let (events, _rx) = broadcast::channel(32);
    let mut runner = observed_runner(queue.clone(), events);
    runner.dag_scheduler = Some(scheduler.clone());
    let processor: Arc<dyn Processor> = Arc::new(BaiduPcsProcessor::new());

    let file_a_str = file_a.to_string_lossy().into_owned();
    let file_b_str = file_b.to_string_lossy().into_owned();
    let created = scheduler
        .create_dag_pipeline(
            DagPipelineDefinition::new(
                "baidupcs-carry-e2e",
                vec![
                    DagStep::new(
                        "upload",
                        PipelineStep::inline(
                            "baidupcs",
                            serde_json::json!({
                                "binary_path": fake_binary.to_string_lossy(),
                                "config_dir": cfg_dir.to_string_lossy(),
                                "destination_root": "/rec//daily",
                                "policy": "skip",
                                "remove_source_after_upload": true,
                                "max_retries": 3,
                            }),
                        ),
                    )
                    .with_retry(StepRetryPolicy {
                        max_attempts: 5,
                        backoff_secs: 0,
                    }),
                ],
            ),
            &[file_a_str.clone(), file_b_str.clone()],
            DagRunContext::default(),
        )
        .await
        .unwrap();
    let job_id = created.root_job_ids[0].clone();

    // Attempt 1 (account 1001): a.flv uploads, b.flv fails -> confirmed_uploads persisted in SQLite.
    std::fs::write(cfg_dir.join("uid.txt"), "1001").unwrap();
    std::fs::write(cfg_dir.join("fail.txt"), "b.flv").unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    runner.execute_job(job, &processor, attempt).await;
    assert!(file_a.exists() && file_b.exists());
    let after_1 = queue.get_job(&job_id).await.unwrap().unwrap();
    assert_eq!(after_1.status, JobStatus::Failed);
    assert_eq!(
        after_1
            .execution_info
            .as_ref()
            .and_then(|info| info.confirmed_uploads.get(&file_a_str))
            .map(|c| (c.account_uid, c.remote_path.as_str())),
        Some((1001, "/rec/daily/a.flv"))
    );

    // Attempt 2 after switching to account 2002: must NOT carry forward a.flv; b.flv succeeds on 2002, a.flv fails.
    std::fs::write(cfg_dir.join("uid.txt"), "2002").unwrap();
    std::fs::write(cfg_dir.join("fail.txt"), "a.flv").unwrap();
    queue.retry_job(&job_id).await.unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    runner.execute_job(job, &processor, attempt).await;
    assert!(
        file_a.exists() && file_b.exists(),
        "switched-account partial failure must keep both local files"
    );

    // Attempt 3 still on account 2002, but `b.flv` was replaced on disk: must NOT carry forward
    // or delete the replacement; `b.flv` is re-uploaded and `a.flv` fails transiently.
    std::fs::write(&file_b, b"replaced-video-b-v2").unwrap();
    queue.retry_job(&job_id).await.unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    runner.execute_job(job, &processor, attempt).await;
    assert!(
        file_a.exists() && file_b.exists(),
        "replacement upload partial failure must keep both local files"
    );

    // Attempt 4 still on account 2002 with unchanged `b.flv`: b.flv is carried forward,
    // only a.flv is uploaded, and both local files are deleted on full success.
    std::fs::write(cfg_dir.join("fail.txt"), "").unwrap();
    queue.retry_job(&job_id).await.unwrap();
    let (job, attempt) = queue.dequeue(None).await.unwrap().unwrap();
    runner.execute_job(job, &processor, attempt).await;

    let final_job = queue.get_job(&job_id).await.unwrap().unwrap();
    assert_eq!(final_job.status, JobStatus::Completed);
    assert!(
        final_job
            .execution_info
            .as_ref()
            .is_none_or(|info| info.confirmed_uploads.is_empty()),
        "completed job should clear retry carry-forward state"
    );
    assert!(!file_a.exists() && !file_b.exists());

    let invocations: Vec<String> = std::fs::read_to_string(cfg_dir.join("invocations.log"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        invocations,
        vec![
            format!("{file_a_str}|{file_b_str}"),
            format!("{file_a_str}|{file_b_str}"),
            format!("{file_a_str}|{file_b_str}"),
            file_a_str,
        ]
    );

    queue.stop_progress_aggregator().await;
}

#[tokio::test]
async fn zero_backoff_retry_orders_retry_scheduled_before_reenqueue_without_leaking_notifications()
{
    use crate::database::models::{DagPipelineDefinition, DagStep, PipelineStep, StepRetryPolicy};
    use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
    use crate::pipeline::dag_scheduler::{DagRunContext, DagScheduler};
    use crate::pipeline::job_queue::JobQueue;
    use crate::services::notification_bridge::PipelineNotifications;

    struct FailOnceProcessor {
        attempts: AtomicUsize,
    }

    #[async_trait]
    impl Processor for FailOnceProcessor {
        fn processor_type(&self) -> ProcessorType {
            ProcessorType::Cpu
        }
        fn job_types(&self) -> Vec<&'static str> {
            vec!["flaky_zero_backoff"]
        }
        fn name(&self) -> &'static str {
            "flaky_zero_backoff"
        }
        async fn process(
            &self,
            input: &ProcessorInput,
            _ctx: &ProcessorContext,
        ) -> srec_core::CoreResult<ProcessorOutput> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(srec_core::CoreError::Other("transient boom".into()))
            } else {
                Ok(ProcessorOutput {
                    outputs: input.inputs.clone(),
                    duration_secs: 0.01,
                    ..Default::default()
                })
            }
        }
    }

    let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
        .await
        .unwrap();
    crate::database::run_migrations(&pool).await.unwrap();
    let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
    let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
    let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
    let scheduler = Arc::new(DagScheduler::new(queue.clone(), dags.clone(), jobs.clone()));
    let (event_tx, mut event_rx) = broadcast::channel(64);
    let mut runner = observed_runner(queue.clone(), event_tx.clone());
    runner.dag_scheduler = Some(scheduler.clone());

    let mut notifications = PipelineNotifications::default();
    let now = jiff::Timestamp::now();

    for round in 0..5 {
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    format!("zero-backoff-{round}"),
                    vec![
                        DagStep::new(
                            "step",
                            PipelineStep::inline("flaky_zero_backoff", serde_json::json!({})),
                        )
                        .with_retry(StepRetryPolicy {
                            max_attempts: 2,
                            backoff_secs: 0,
                        }),
                    ],
                ),
                &["clip.mp4".into()],
                DagRunContext {
                    streamer_id: Some(format!("streamer-{round}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let job_id = created.root_job_ids[0].clone();
        let processor: Arc<dyn Processor> = Arc::new(FailOnceProcessor {
            attempts: AtomicUsize::new(0),
        });

        // Concurrent re-queuer races with attempt 1's failure persistence, and
        // a second worker task immediately executes attempt 2 once woken.
        let retry_queue = queue.clone();
        let retry_tx = event_tx.clone();
        let retry_job_id = job_id.clone();
        let requeuer = tokio::spawn(async move {
            loop {
                let due = retry_queue
                    .list_due_retries(jiff::Timestamp::now())
                    .await
                    .unwrap();
                if let Some(due_job) = due.into_iter().find(|j| j.id == retry_job_id) {
                    retry_queue
                        .retry_job_with_hook(&due_job.id, || {
                            let _ = retry_tx.send(PipelineEvent::JobEnqueued {
                                job_id: due_job.id.clone(),
                                job_type: due_job.job_type.clone(),
                                streamer_id: due_job.streamer_id.clone(),
                            });
                        })
                        .await
                        .unwrap();
                    break;
                }
                tokio::task::yield_now().await;
            }
        });

        let (job_1, attempt_1) = queue.dequeue(None).await.unwrap().unwrap();

        // Spawn attempt 2's worker concurrently so it races to claim and
        // complete the retried job the instant `retry_job_with_hook` wakes it.
        let worker2_queue = queue.clone();
        let worker2_events = event_tx.clone();
        let worker2_scheduler = scheduler.clone();
        let worker2_processor = processor.clone();
        let worker2 = tokio::spawn(async move {
            let mut runner2 = observed_runner(worker2_queue.clone(), worker2_events);
            runner2.dag_scheduler = Some(worker2_scheduler);
            let notifier = worker2_queue.notifier();
            loop {
                let notified = notifier.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Some((job_2, attempt_2)) = worker2_queue.dequeue(None).await.unwrap() {
                    runner2
                        .execute_job(job_2, &worker2_processor, attempt_2)
                        .await;
                    break;
                }
                notified.await;
            }
        });

        runner.execute_job(job_1, &processor, attempt_1).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            requeuer.await.unwrap();
            worker2.await.unwrap();
        })
        .await
        .expect("zero-backoff retry and worker2 complete promptly");

        let mut tags = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            match &event {
                PipelineEvent::JobStarted { .. } => tags.push("started"),
                PipelineEvent::JobRetryScheduled { .. } => tags.push("retry_scheduled"),
                PipelineEvent::JobEnqueued { .. } => tags.push("enqueued"),
                PipelineEvent::JobCompleted { .. } => tags.push("completed"),
                PipelineEvent::JobFailed { .. } => tags.push("failed"),
                _ => {}
            }
            if let Some(notice) = notifications.map(event, now) {
                assert_ne!(notice.event_type(), "pipeline_failed");
            }
        }
        assert_eq!(
            tags,
            vec![
                "started",
                "retry_scheduled",
                "enqueued",
                "started",
                "completed"
            ],
            "round {round} must strictly order retry_scheduled -> enqueued -> started -> completed"
        );

        // Confirm no stale `job_to_streamer` entry survived after completion.
        let probe = notifications
            .map(
                PipelineEvent::JobStarted {
                    job_id,
                    job_type: "flaky_zero_backoff".into(),
                    streamer_id: String::new(),
                },
                now,
            )
            .unwrap();
        assert!(matches!(
            probe,
            crate::notification::NotificationEvent::PipelineStarted { streamer_id, .. }
                if streamer_id.is_empty()
        ));
    }

    queue.stop_progress_aggregator().await;
}
