//! Per-job execution runner, log collector, and failure persistence backoff.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use srec_core::CoreError;
use srec_sqlite::DbError;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::{debug, error, info, warn};

use super::autoscaling::update_avg_runtime_ms;
use super::lingering::{
    LingeringAttempts, LingeringGuard, STALLED_OPERATION_THRESHOLD, StalledOperation, timeout_error,
};
use super::{
    ActiveWorkerGuard, LOG_CHANNEL_CAPACITY, NO_PROCESSOR_ERROR, SHUTDOWN_DRAIN_GRACE, WorkerType,
    emit_event,
};
use crate::dag_scheduler::{
    DagCompletionInfo, DagJobCompletedUpdate, DagJobFailedUpdate, DagScheduler, FailedStepAttempt,
    PlannedRetry, StepFailureKind, StepFailureOutcome,
};
use crate::job_queue::{AttemptToken, Job, JobFailureOutcome, JobLogEntry, JobQueue, JobResult};
use crate::manager::PipelineEvent;
use crate::processors::{
    ConfirmedUpload, JobLogSink, Processor, ProcessorContext, ProcessorInput, ProcessorOutput,
    UploadResultItem,
};

/// Everything a worker task needs to run the jobs it claims.
///
/// One is built per worker in [`super::WorkerPool::start_with_dag_scheduler`]. The worker loop owns
/// the claim side — permit, `JobQueue::dequeue`, poll backoff — and hands each claimed job to
/// [`JobRunner::run`], which owns everything from picking a processor to recording a terminal
/// state.
pub(super) struct JobRunner {
    pub(super) worker_type: WorkerType,
    pub(super) job_queue: Arc<JobQueue>,
    pub(super) processors: Vec<Arc<dyn Processor>>,
    pub(super) dag_scheduler: Option<Arc<DagScheduler>>,
    pub(super) dag_notify_tx: Option<tokio::sync::mpsc::Sender<DagCompletionInfo>>,
    pub(super) event_tx: Option<broadcast::Sender<PipelineEvent>>,
    pub(super) active_workers: Arc<AtomicUsize>,
    pub(super) avg_runtime_ms: Arc<AtomicU64>,
    pub(super) job_timeout: std::time::Duration,
    /// The pool's shutdown token. Distinct from the per-job token that reaches the processor
    /// through `ProcessorContext`; see [`JobRunner::execute`].
    pub(super) shutdown: CancellationToken,
    pub(super) lingering: Arc<LingeringAttempts>,
}

/// What `Processor::process` produced, once the cancellation and timeout races around it have
/// been settled.
enum JobOutcome {
    /// The job's own token fired: a user cancel, DAG fail-fast, or the shutdown drain.
    Cancelled,
    Completed(Box<ProcessorOutput>),
    Failed(crate::PipelineError),
    TimedOut,
}

/// The job fields the terminal handlers still need after `ProcessorInput` has taken ownership
/// of the rest of the job.
pub(super) struct JobFacts {
    pub(super) id: String,
    pub(super) job_type: String,
    pub(super) processor_name: &'static str,
    pub(super) dag_step_execution_id: Option<String>,
    /// Retries this attempt already sits on top of; decides whether a step's
    /// retry budget has another attempt left.
    pub(super) retry_count: i32,
    /// The attempt every terminal write is fenced to.
    pub(super) attempt: AttemptToken,
}

impl JobRunner {
    fn emit(&self, event: PipelineEvent) {
        emit_event(self.event_tx.as_ref(), event);
    }

    /// Keep ownership of the attempt until its failure is durable. Repeating this write
    /// never invokes the processor again; shutdown leaves an interrupted job for recovery.
    async fn persist_failure_with_retry<F, Fut>(
        &self,
        job_id: &str,
        mut persist: F,
    ) -> crate::PipelineResult<JobFailureOutcome>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = crate::PipelineResult<JobFailureOutcome>>,
    {
        let mut backoff = FailurePersistBackoff::default();
        loop {
            match persist().await {
                Ok(outcome) => {
                    if backoff.failures > 0 {
                        info!(
                            job_id,
                            failed_attempts = backoff.failures,
                            "Persisted job failure after retrying"
                        );
                    }
                    return Ok(outcome);
                }
                Err(
                    error @ (crate::PipelineError::Db(DbError::DatabaseSqlx(_))
                    | crate::PipelineError::Core(CoreError::Database(_))),
                ) => {
                    let (delay, warn) = backoff.on_failure();
                    if warn {
                        warn!(job_id, failed_attempts = backoff.failures, retry_in = ?delay, %error, "Retrying job failure persistence without rerunning the processor");
                    } else {
                        debug!(job_id, failed_attempts = backoff.failures, retry_in = ?delay, %error, "Retrying job failure persistence without rerunning the processor");
                    }
                    tokio::select! {
                        biased;
                        _ = self.shutdown.cancelled() => return Err(error),
                        _ = tokio::time::sleep(delay) => {},
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Route `job` to a processor that accepts its type and run it to a terminal state.
    ///
    /// Reaching the no-processor arm means `JobQueue::dequeue`'s type filter and
    /// `Processor::can_process` disagree, since the filter is built from the same
    /// `job_types()`.
    pub(super) async fn run(&self, job: Job, attempt: AttemptToken) {
        let Some(processor) = self
            .processors
            .iter()
            .find(|p| p.can_process(&job.job_type))
            .cloned()
        else {
            warn!(
                "No processor found for job type: '{}'. Available processors: {:?}",
                job.job_type,
                self.processors.iter().map(|p| p.name()).collect::<Vec<_>>()
            );
            match self
                .persist_failure_with_retry(&job.id, || {
                    self.job_queue
                        .fail_attempt(&job.id, &attempt, NO_PROCESSOR_ERROR)
                })
                .await
            {
                Ok(JobFailureOutcome::Transitioned) => self.emit(PipelineEvent::JobFailed {
                    job_id: job.id,
                    job_type: job.job_type,
                    error: NO_PROCESSOR_ERROR.to_string(),
                }),
                Ok(JobFailureOutcome::Unchanged) => {}
                Err(error) => error!(job_id = %job.id, %error, "Failed to mark job as failed"),
            }
            return;
        };

        if self.settle_multi_input(&job, &processor, &attempt).await {
            return;
        }

        self.execute_job(job, &processor, attempt).await;
    }

    /// Handle a multi-input job routed to a processor that takes one input at a time.
    ///
    /// Returns `true` when the job has been dealt with here and must not be executed: either
    /// it was split into single-input jobs, or — for a DAG step, where splitting would let one
    /// step complete several times — it was failed and its DAG failed with it.
    async fn settle_multi_input(
        &self,
        job: &Job,
        processor: &Arc<dyn Processor>,
        attempt: &AttemptToken,
    ) -> bool {
        if job.inputs.len() <= 1 || processor.supports_batch_input() {
            return false;
        }

        if let Some(dag_step_id) = job.dag_step_execution_id.as_deref() {
            let reason = format!(
                "DAG step job has {} inputs but processor '{}' does not support batch inputs",
                job.inputs.len(),
                processor.name()
            );
            error!(job_id = %job.id, dag_step_execution_id = %dag_step_id, "{}", reason);

            let facts = JobFacts {
                id: job.id.clone(),
                job_type: job.job_type.clone(),
                processor_name: processor.name(),
                dag_step_execution_id: Some(dag_step_id.to_string()),
                retry_count: job.retry_count,
                attempt: attempt.clone(),
            };
            self.finish_failed(&facts, &reason, StepFailureKind::NonBatchInput, &[], None)
                .await;

            return true;
        }

        info!(
            "Splitting job {} with {} inputs for single-input processor {}",
            job.id,
            job.inputs.len(),
            processor.name()
        );
        match self.job_queue.split_job_for_single_input(job).await {
            Ok(split_ids) => {
                info!(
                    "Split job {} into {} jobs: {:?}",
                    job.id,
                    split_ids.len(),
                    split_ids
                );
            }
            Err(e) => {
                error!("Failed to split job {}: {}", job.id, e);
                let reason = format!("Failed to split job: {e}");
                if let Err(fail_error) = self
                    .persist_failure_with_retry(&job.id, || {
                        self.job_queue.fail_attempt(&job.id, attempt, &reason)
                    })
                    .await
                {
                    error!(
                        job_id = %job.id,
                        error = %fail_error,
                        "Failed to persist job split failure"
                    );
                }
            }
        }

        true
    }

    /// Run one job through its processor and record the terminal state it reached.
    pub(super) async fn execute_job(
        &self,
        mut job: Job,
        processor: &Arc<dyn Processor>,
        job_token: AttemptToken,
    ) {
        let _output_lease = self.job_queue.output_files_gate.read().await;
        // The queue stops tracking an attempt once the job is deleted or a
        // retry has replaced it; neither leaves anything for this worker to run.
        if self
            .job_queue
            .attempt_token(&job.id)
            .is_none_or(|current| current.id() != job_token.id())
        {
            info!(job_id = %job.id, "Skipping withdrawn job");
            return;
        }
        // Stop helpers when this attempt is dropped. Keep timeout/abort cancellation
        // separate from the queue's token, which represents an explicit job cancellation.
        let processor_token = job_token.child_token();
        let _cancel_processor = processor_token.clone().drop_guard();

        // Start metadata is a patch: prior attempts retain their diagnostics and
        // produced-item history, and historical log snapshots are not replayed.
        if let Err(e) = self
            .job_queue
            .begin_execution(&job.id, processor.name())
            .await
        {
            let reason = format!("Failed to persist execution start: {e}");
            error!(job_id = %job.id, error = %e, "Skipping processor without durable attempt metadata");
            if job_token.is_cancelled() {
                self.job_queue.finalize_attempt(&job.id, &job_token).await;
                return;
            }
            let job_id = job.id.clone();
            let job_type = job.job_type.clone();
            let reason = reason.as_str();
            let attempt_ref = &job_token;
            // `ExecutionStart` is never retried, so no retry is planned.
            let persist = |_planned: Option<PlannedRetry>| async move {
                let outcome = self
                    .persist_failure_with_retry(&job_id, || {
                        self.job_queue
                            .fail_execution_start_attempt(&job_id, attempt_ref, reason)
                    })
                    .await?;
                if outcome == JobFailureOutcome::Transitioned {
                    self.emit(PipelineEvent::JobFailed {
                        job_id,
                        job_type,
                        error: reason.to_string(),
                    });
                }
                Ok(outcome)
            };
            match (job.dag_step_execution_id.as_deref(), &self.dag_scheduler) {
                (Some(dag_step_id), Some(scheduler)) => {
                    let attempt = FailedStepAttempt {
                        job_id: &job.id,
                        retry_count: job.retry_count,
                        error: reason,
                        kind: StepFailureKind::ExecutionStart,
                    };
                    let outcome = scheduler
                        .on_job_attempt_failed(dag_step_id, attempt, persist)
                        .await;
                    self.report_step_failure(dag_step_id, StepFailureKind::ExecutionStart, outcome)
                        .await;
                }
                _ => {
                    if let Err(error) = persist(None).await {
                        error!(job_id = %job.id, %error, "Failed to persist execution-start failure");
                    }
                }
            }
            return;
        }

        self.active_workers.fetch_add(1, Ordering::SeqCst);
        let _active_worker = ActiveWorkerGuard(&self.active_workers);
        let started = std::time::Instant::now();

        // True when this job has been attempted before, which is what makes a processor's
        // resume path legitimate (see CopyMoveProcessor's absent-source handling).
        // `retry_count` covers an explicit `JobQueue::retry_job`; `current_processor` covers a
        // job a crash left PROCESSING that `JobQueue::recover_jobs` reset to PENDING, which
        // leaves `retry_count` alone but keeps the execution_info the earlier attempt wrote
        // through the execution-start metadata patch above.
        let is_retry = job.retry_count > 0
            || job
                .execution_info
                .as_ref()
                .is_some_and(|info| info.current_processor.is_some());

        // A workflow step may set its own timeout; the pool default covers the rest.
        let step_timeout = job.timeout_secs.map(std::time::Duration::from_secs);
        let job_timeout = step_timeout.unwrap_or(self.job_timeout);

        let facts = JobFacts {
            id: job.id.clone(),
            job_type: job.job_type.clone(),
            processor_name: processor.name(),
            dag_step_execution_id: job.dag_step_execution_id.take(),
            retry_count: job.retry_count,
            attempt: job_token.clone(),
        };

        let input = ProcessorInput {
            inputs: std::mem::take(&mut job.inputs),
            outputs: std::mem::take(&mut job.outputs),
            config: job.config.take(),
            streamer_id: std::mem::take(&mut job.streamer_id),
            session_id: std::mem::take(&mut job.session_id),
            streamer_name: job.streamer_name.take(),
            session_title: job.session_title.take(),
            platform: job.platform.take(),
            session_start: job.session_start.take(),
            created_at: job.created_at,
            manifest: job.manifest.take(),
        };

        self.emit(PipelineEvent::JobStarted {
            job_id: facts.id.clone(),
            job_type: facts.job_type.clone(),
            streamer_id: input.streamer_id.clone(),
        });

        let (log_tx, log_rx) = tokio::sync::mpsc::channel(LOG_CHANNEL_CAPACITY);
        let log_dropped = Arc::new(AtomicUsize::new(0));
        let log_sink = JobLogSink::new(log_tx, log_dropped.clone());
        let log_collector = spawn_log_collector(
            self.job_queue.clone(),
            facts.id.clone(),
            log_rx,
            log_dropped,
        );

        let has_step_retry = match (
            job.has_step_retry,
            facts.dag_step_execution_id.as_deref(),
            &self.dag_scheduler,
        ) {
            (Some(value), _, _) => value,
            // Legacy workflow rows persisted before `has_step_retry` existed
            // fall back to the stored workflow definition once per attempt.
            (None, Some(step_id), Some(scheduler)) => scheduler
                .step_retry_state(step_id, &facts.id)
                .await
                .ok()
                .and_then(|state| state.max_attempts)
                .is_some_and(|max| max > 1),
            _ => false,
        };
        let prior_completed_uploads = if is_retry && processor.carries_forward_completed_uploads() {
            job.execution_info
                .map(|info| info.confirmed_uploads)
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        let staged_outputs = self.job_queue.staged_output_recorder(&facts.id);
        let ctx = ProcessorContext::new(
            facts.id.clone(),
            self.job_queue.progress_reporter(&facts.id),
            log_sink,
            processor_token.clone(),
        )
        .with_retry(is_retry)
        .with_step_retry(has_step_retry)
        .with_step_timeout(step_timeout)
        .with_staged_outputs(staged_outputs.clone())
        .with_prior_completed_uploads(prior_completed_uploads);

        let outcome = self
            .execute(
                processor,
                &input,
                &ctx,
                &facts.id,
                &processor_token,
                job_timeout,
            )
            .await;

        // Drop ctx to close the log channel
        drop(ctx);

        // The processor future of a cancelled or timed-out job is gone, but blocking work it
        // started cannot be interrupted and may still hold a log sender, and with it this
        // worker, until the file system returns.
        let lingering =
            matches!(outcome, JobOutcome::Cancelled | JobOutcome::TimedOut).then(|| {
                self.lingering.register(
                    job_token.id(),
                    StalledOperation::ended(&facts, &input, self.worker_type),
                )
            });

        // Blocking work that a deadline cannot interrupt may still own a log sender. The
        // timeout is recorded at the deadline; this worker keeps its slot and output lease
        // until the collector drains, then finalizes only this attempt, because a scheduled
        // or manual retry can start a successor under the same job ID in the meantime.
        let timeout_recorded = matches!(outcome, JobOutcome::TimedOut) && !job_token.is_cancelled();
        if timeout_recorded {
            let progress = self.job_queue.get_job_progress(&facts.id);
            let reason = timeout_error(job_timeout, &facts, &input, progress.as_ref());
            self.finish_failed(&facts, &reason, StepFailureKind::Timeout, &[], None)
                .await;
        }

        self.drain_log_collector(log_collector, &facts, lingering)
            .await;
        // Blocking copies hold a log sender, so the drain outlasts them; other
        // staged files were published or handed to their guards' cleanup when
        // the processor returned. A crash before this point keeps the rows.
        staged_outputs.finish().await;

        // A cancellation that landed while the processor was finishing wins: its CANCELLED row
        // is the durable truth, so nothing here may write a competing terminal state.
        let cancelled = job_token.is_cancelled();
        match outcome {
            JobOutcome::Cancelled => {
                // No upload_records synthesis here: the job's CANCELLED status is the durable
                // truth, and cancel_job already broadcast the Terminal{Cancelled} upload event.
                info!(job_id = %facts.id, "Job cancelled while processing");
            }
            JobOutcome::Completed(_) if cancelled => {
                info!(
                    job_id = %facts.id,
                    "Job finished after cancellation; skipping completion"
                );
            }
            JobOutcome::Completed(output) => {
                self.finish_completed(&facts, *output, &input).await;
            }
            JobOutcome::Failed(_) if cancelled => {
                info!(
                    job_id = %facts.id,
                    "Job failed after cancellation; skipping failure handling"
                );
            }
            JobOutcome::Failed(e) => {
                let kind = if e.is_non_retryable_step_error() {
                    StepFailureKind::NonRetryable
                } else {
                    StepFailureKind::ProcessorError
                };
                self.finish_failed(&facts, &e.to_string(), kind, &[], None)
                    .await;
            }
            JobOutcome::TimedOut if timeout_recorded => {}
            JobOutcome::TimedOut => {
                info!(
                    job_id = %facts.id,
                    "Job timed out after cancellation; skipping timeout handling"
                );
            }
        }

        if processor_token.is_cancelled() {
            self.job_queue.finalize_attempt(&facts.id, &job_token).await;
        }

        update_avg_runtime_ms(&self.avg_runtime_ms, started.elapsed().as_millis() as u64);
    }

    /// Wait for the log collector, which finishes once the last log sender is gone, including
    /// one held by blocking work that outlived its ended job. Such an attempt stays listed
    /// through `lingering` until then and is logged once it passes
    /// `STALLED_OPERATION_THRESHOLD`.
    async fn drain_log_collector(
        &self,
        mut log_collector: AbortOnDropHandle<()>,
        facts: &JobFacts,
        lingering: Option<LingeringGuard>,
    ) {
        let drained = match &lingering {
            Some(_) => {
                match tokio::time::timeout(STALLED_OPERATION_THRESHOLD, &mut log_collector).await {
                    Ok(drained) => drained,
                    Err(_) => {
                        warn!(
                            job_id = %facts.id,
                            processor = facts.processor_name,
                            worker_type = %self.worker_type,
                            stalled_secs = STALLED_OPERATION_THRESHOLD.as_secs(),
                            "Job ended but blocking work it started has not returned; its worker \
                             stays reserved until the file system responds"
                        );
                        log_collector.await
                    }
                }
            }
            None => log_collector.await,
        };
        drop(lingering);
        if let Err(join_error) = drained {
            error!(
                job_id = %facts.id,
                error = %join_error,
                "Job log collector task failed"
            );
        }
    }

    /// Await `Processor::process` against the job's own cancellation, the pool's shutdown, and
    /// `job_timeout` (the step's own timeout, or the pool default).
    ///
    /// On shutdown a job gets `SHUTDOWN_DRAIN_GRACE` to finish on its own; past that its own
    /// token is cancelled, which is the only signal that reaches the processor, since
    /// `ProcessorContext` carries that token and not the pool's. Without it the worker stays
    /// inside `process` until `job_timeout` and `WorkerPool::stop` waits there with it, so
    /// shutdown could only end by falling back to `WorkerPool::abort`. A job stopped this way
    /// keeps its PROCESSING row, which `JobQueue::recover_jobs` resets to PENDING to be run
    /// again on the next start.
    async fn execute(
        &self,
        processor: &Arc<dyn Processor>,
        input: &ProcessorInput,
        ctx: &ProcessorContext,
        job_id: &str,
        job_token: &CancellationToken,
        job_timeout: std::time::Duration,
    ) -> JobOutcome {
        let timed = tokio::time::timeout(job_timeout, processor.process(input, ctx));
        tokio::pin!(timed);

        let shutdown_drain = {
            let shutdown = self.shutdown.clone();
            async move {
                shutdown.cancelled().await;
                tokio::time::sleep(SHUTDOWN_DRAIN_GRACE).await;
            }
        };

        tokio::select! {
            biased;
            _ = job_token.cancelled() => JobOutcome::Cancelled,
            _ = shutdown_drain => {
                info!(
                    job_id = %job_id,
                    "Stopping unfinished job for {} worker pool shutdown",
                    self.worker_type
                );
                job_token.cancel();
                JobOutcome::Cancelled
            }
            res = &mut timed => match res {
                Ok(Ok(output)) => JobOutcome::Completed(Box::new(output)),
                Ok(Err(e)) => JobOutcome::Failed(e.into()),
                Err(_) => {
                    // Helpers can retain a log sender until cancellation. Signal them
                    // before execute_job waits for the log collector to drain.
                    job_token.cancel();
                    JobOutcome::TimedOut
                },
            },
        }
    }

    /// Persist a successful job and let its DAG advance.
    async fn finish_completed(
        &self,
        facts: &JobFacts,
        output: ProcessorOutput,
        input: &ProcessorInput,
    ) {
        // Produced artifacts are observability data, not permission to delete files.
        if !output.items_produced.is_empty()
            && let Err(e) = self
                .job_queue
                .record_produced_items(&facts.id, &output.items_produced)
                .await
        {
            warn!(
                "Failed to record produced items for job {}: {}",
                facts.id, e
            );
        }

        // A processor that could not handle every input has not succeeded, even
        // when it published the rest: a downstream `delete` or `move` would act on
        // an incomplete set. The outputs it did publish stay on disk and in the
        // produced-item history so a retry can resume past them.
        if !output.failed_inputs.is_empty() {
            let details = output
                .failed_inputs
                .iter()
                .map(|(path, reason)| format!("{path}: {reason}"))
                .collect::<Vec<_>>()
                .join("; ");
            let error = format!(
                "{} of {} inputs failed: {details}",
                output.failed_inputs.len(),
                input.inputs.len()
            );
            if !output.logs.is_empty()
                && let Err(e) = self
                    .job_queue
                    .record_result_logs(&facts.id, &output.logs)
                    .await
            {
                warn!(job_id = %facts.id, error = %e, "Failed to record logs of a partially failed job");
            }
            let kind = if output.non_retryable {
                StepFailureKind::NonRetryable
            } else {
                StepFailureKind::ProcessorError
            };
            self.finish_failed(
                facts,
                &error,
                kind,
                &output.uploads,
                Some(&output.confirmed_uploads),
            )
            .await;
            return;
        }

        let completed_outputs = output.outputs.clone();
        let completed_duration = output.duration_secs;
        let result = JobResult {
            outputs: output.outputs,
            duration_secs: output.duration_secs,
            metadata: output.metadata,
            uploads: output.uploads,
            logs: output.logs,
            input_size_bytes: output.input_size_bytes,
            output_size_bytes: output.output_size_bytes,
        };
        let persisted = self
            .job_queue
            .complete_attempt(&facts.id, &facts.attempt, result)
            .await;

        match persisted {
            Ok(true) => {}
            Ok(false) => {
                debug!(job_id = %facts.id, "Job completion lost an active-state race");
                return;
            }
            Err(e) => {
                error!(
                    job_id = %facts.id,
                    error = %e,
                    "Failed to persist completed pipeline job"
                );
                return;
            }
        }

        self.emit(PipelineEvent::JobCompleted {
            job_id: facts.id.clone(),
            job_type: facts.job_type.clone(),
            duration_secs: completed_duration,
        });

        // `JobQueue::complete_attempt` persists outputs before
        // `DagScheduler::on_job_completed` derives graph advancement from them.
        debug!(
            "Job {} completed, dag_step_execution_id={:?}",
            facts.id, facts.dag_step_execution_id
        );

        let Some(dag_step_id) = facts.dag_step_execution_id.as_deref() else {
            return;
        };
        let Some(scheduler) = &self.dag_scheduler else {
            return;
        };

        match scheduler
            .on_job_attempt_completed(
                dag_step_id,
                &completed_outputs,
                input.streamer_name.as_deref(),
                input.session_title.as_deref(),
                input.platform.as_deref(),
                input.session_start,
            )
            .await
        {
            Ok(DagJobCompletedUpdate {
                new_job_ids,
                completion,
            }) => {
                if !new_job_ids.is_empty() {
                    info!(
                        "DAG step {} completed, created {} downstream jobs",
                        dag_step_id,
                        new_job_ids.len()
                    );
                }
                self.notify_dag_completion(completion).await;
            }
            Err(error) => error!(
                dag_step_execution_id = %dag_step_id,
                %error,
                "Failed to settle the workflow after a step completed"
            ),
        }
    }

    /// Record a failed or timed-out attempt. For a workflow step the scheduler
    /// decides whether the step gets another attempt or fails, and the job row
    /// is written with the retry it plans; the worker itself only persists and
    /// announces. The row keeps the plain error; a failure event is emitted
    /// only when no further automatic retry is planned. Processors own cleanup
    /// of their staged outputs; persisted artifact paths may belong to earlier
    /// attempts. `uploads` are the per-file results of a partially failed
    /// upload batch.
    pub(super) async fn finish_failed(
        &self,
        facts: &JobFacts,
        error: &str,
        kind: StepFailureKind,
        uploads: &[UploadResultItem],
        confirmed_uploads: Option<&HashMap<String, ConfirmedUpload>>,
    ) {
        let persist = |planned: Option<PlannedRetry>| async move {
            let retry_after = planned.map(|planned| planned.retry_after);
            // Emit the pipeline event inside `fail_attempt_with_step_info`
            // while the queue still holds `attempt_gate`, so an immediate
            // zero-backoff retry cannot emit `JobEnqueued` before this
            // attempt's `JobRetryScheduled` (or `JobFailed`) reaches listeners.
            let event = if planned.is_some() {
                PipelineEvent::JobRetryScheduled {
                    job_id: facts.id.clone(),
                }
            } else {
                PipelineEvent::JobFailed {
                    job_id: facts.id.clone(),
                    job_type: facts.job_type.clone(),
                    error: error.to_string(),
                }
            };
            let outcome = self
                .persist_failure_with_retry(&facts.id, || {
                    let event = &event;
                    self.job_queue.fail_attempt_with_step_info(
                        &facts.id,
                        &facts.attempt,
                        error,
                        retry_after,
                        Some(facts.processor_name),
                        uploads,
                        confirmed_uploads,
                        move || {
                            self.emit(event.clone());
                        },
                    )
                })
                .await?;
            Ok(outcome)
        };

        match (facts.dag_step_execution_id.as_deref(), &self.dag_scheduler) {
            (Some(dag_step_id), Some(scheduler)) => {
                let attempt = FailedStepAttempt {
                    job_id: &facts.id,
                    retry_count: facts.retry_count,
                    error,
                    kind,
                };
                let outcome = scheduler
                    .on_job_attempt_failed(dag_step_id, attempt, persist)
                    .await;
                self.report_step_failure(dag_step_id, kind, outcome).await;
            }
            _ => {
                if let Err(error) = persist(None).await {
                    error!(job_id = %facts.id, %error, "Failed to persist pipeline job failure");
                }
            }
        }
    }

    /// Log what the scheduler did with a failed step attempt and forward the
    /// workflow's completion when the failure settled it.
    async fn report_step_failure(
        &self,
        dag_step_id: &str,
        kind: StepFailureKind,
        outcome: crate::PipelineResult<StepFailureOutcome>,
    ) {
        match outcome {
            Ok(StepFailureOutcome::Retried(_)) => {}
            Ok(StepFailureOutcome::Failed(DagJobFailedUpdate {
                cancelled_count,
                completion,
            })) => {
                match kind {
                    StepFailureKind::Timeout => info!(
                        "DAG step {} timed out, cancelled {} jobs (fail-fast)",
                        dag_step_id, cancelled_count
                    ),
                    StepFailureKind::ProcessorError
                    | StepFailureKind::NonBatchInput
                    | StepFailureKind::ExecutionStart
                    | StepFailureKind::NonRetryable => info!(
                        "DAG step {} failed, cancelled {} jobs (fail-fast)",
                        dag_step_id, cancelled_count
                    ),
                }
                self.notify_dag_completion(completion).await;
            }
            Ok(StepFailureOutcome::Ignored) => debug!(
                dag_step_execution_id = %dag_step_id,
                "Ignoring the failure of a step that had already settled"
            ),
            Err(error) => error!(
                dag_step_execution_id = %dag_step_id,
                %error,
                "Failed to apply a step failure to its workflow"
            ),
        }
    }

    /// Forward a terminal DAG state to `PipelineManager::handle_dag_completion`.
    async fn notify_dag_completion(&self, completion: Option<DagCompletionInfo>) {
        if let Some(completion) = completion
            && let Some(tx) = &self.dag_notify_tx
            && let Err(e) = tx.send(completion).await
        {
            warn!(
                error = %e,
                "Failed to send DAG completion notification"
            );
        }
    }
}

/// Drain a job's streaming log entries into `job_execution_logs`.
///
/// Runs as its own task so a slow or failing database write never blocks
/// `Processor::process`. `execute_job` drops the `ProcessorContext` — and with it the sink's
/// sender — before awaiting the returned handle, which is what ends the loop and forces the
/// final flush.
pub(super) fn spawn_log_collector(
    job_queue: Arc<JobQueue>,
    job_id: String,
    mut log_rx: tokio::sync::mpsc::Receiver<JobLogEntry>,
    log_dropped: Arc<AtomicUsize>,
) -> AbortOnDropHandle<()> {
    const FLUSH_INTERVAL_MS: u64 = 200;
    const MAX_BATCH_SIZE: usize = 1000;
    const MAX_BUFFERED_LOGS: usize = 4000;

    AbortOnDropHandle::new(tokio::spawn(async move {
        let mut flush_timer =
            tokio::time::interval(std::time::Duration::from_millis(FLUSH_INTERVAL_MS));
        flush_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut buffer: VecDeque<JobLogEntry> = VecDeque::with_capacity(MAX_BATCH_SIZE);
        let mut dropped_due_to_backpressure: usize = 0;
        let mut backoff = LogFlushBackoff::default();

        loop {
            tokio::select! {
                entry = log_rx.recv() => {
                    let Some(entry) = entry else {
                        // The sink is gone: report what was dropped and flush unconditionally.
                        let producer_dropped = log_dropped.load(Ordering::Relaxed);
                        if dropped_due_to_backpressure > 0 {
                            buffer.push_back(JobLogEntry::warn(format!(
                                "Dropped {dropped_due_to_backpressure} log lines due to DB backpressure (buffer_cap={MAX_BUFFERED_LOGS})",
                            )));
                        }
                        if producer_dropped > 0 {
                            buffer.push_back(JobLogEntry::warn(format!(
                                "Dropped {producer_dropped} log lines due to log channel backpressure (capacity={LOG_CHANNEL_CAPACITY})",
                            )));
                        }
                        backoff.flush(&job_queue, &job_id, &mut buffer, true).await;
                        break;
                    };

                    buffer.push_back(entry);
                    while buffer.len() > MAX_BUFFERED_LOGS {
                        let _ = buffer.pop_front();
                        dropped_due_to_backpressure = dropped_due_to_backpressure.saturating_add(1);
                    }

                    if buffer.len() >= MAX_BATCH_SIZE {
                        backoff.flush(&job_queue, &job_id, &mut buffer, false).await;
                    }
                }
                _ = flush_timer.tick() => {
                    backoff.flush(&job_queue, &job_id, &mut buffer, false).await;
                }
            }
        }
    }))
}

/// Retry pacing for `JobQueue::append_log_entry`, so a database that is refusing writes is
/// retried with a widening gap instead of once per buffered batch.
struct LogFlushBackoff {
    delay: std::time::Duration,
    next_flush_allowed: tokio::time::Instant,
}

impl Default for LogFlushBackoff {
    fn default() -> Self {
        Self {
            delay: std::time::Duration::ZERO,
            next_flush_allowed: tokio::time::Instant::now(),
        }
    }
}

impl LogFlushBackoff {
    const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(5);
    const INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(200);

    /// Write `buffer` out, unless a previous failure asked for a wait that has not elapsed.
    /// `force` ignores that wait, for the final flush once the job's sink is gone.
    async fn flush(
        &mut self,
        job_queue: &JobQueue,
        job_id: &str,
        buffer: &mut VecDeque<JobLogEntry>,
        force: bool,
    ) {
        if buffer.is_empty() {
            return;
        }
        if !force && tokio::time::Instant::now() < self.next_flush_allowed {
            return;
        }

        match job_queue
            .append_log_entry(job_id, buffer.make_contiguous())
            .await
        {
            Ok(()) => {
                buffer.clear();
                self.delay = std::time::Duration::ZERO;
                self.next_flush_allowed = tokio::time::Instant::now();
            }
            Err(e) => {
                warn!("Failed to append streaming logs for job {}: {}", job_id, e);
                self.delay = if self.delay.is_zero() {
                    Self::INITIAL_DELAY
                } else {
                    (self.delay * 2).min(Self::MAX_DELAY)
                };
                self.next_flush_allowed = tokio::time::Instant::now() + self.delay;
            }
        }
    }
}

/// Retry pacing for `JobRunner::persist_failure_with_retry`, which keeps retrying until the
/// failure is durable or the pool shuts down. A lasting database error (typically a full disk)
/// is retried with a widening gap and warned about on the first failure, then once per capped
/// interval.
#[derive(Default)]
pub(super) struct FailurePersistBackoff {
    pub(super) delay: std::time::Duration,
    pub(super) failures: u32,
}

impl FailurePersistBackoff {
    pub(super) const INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
    pub(super) const MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

    /// Record a failed attempt: how long to wait before the next one, and whether to warn.
    pub(super) fn on_failure(&mut self) -> (std::time::Duration, bool) {
        self.failures = self.failures.saturating_add(1);
        self.delay = if self.delay.is_zero() {
            Self::INITIAL_DELAY
        } else {
            (self.delay * 2).min(Self::MAX_DELAY)
        };
        (
            self.delay,
            self.failures == 1 || self.delay == Self::MAX_DELAY,
        )
    }
}
