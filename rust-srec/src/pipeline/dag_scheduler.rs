//! DAG Scheduler for orchestrating DAG pipeline execution.
//!
//! The DagScheduler is responsible for:
//! - Creating jobs for ready DAG steps
//! - Handling job completion and triggering downstream steps (fan-in)
//! - Cancelling the steps downstream of a failed job while other branches settle
//! - Tracking DAG execution progress

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::database::models::{
    DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStepExecutionDbModel,
    DagStepStatus, JobDbModel, PipelineStep, ReadyStep,
};
use crate::database::repositories::{DagRepository, JobRepository};
use crate::pipeline::job_queue::{JobStateMeta, job_state_json, parse_job_state};
use crate::pipeline::manifest::PipelineInputManifest;
use crate::pipeline::{Job, JobFailureOutcome, JobQueue, JobStatus};
use crate::utils::json::{self, JsonContext};
use crate::{Error, Result};

pub(crate) type PublicationRollback = Box<dyn FnOnce() + Send>;
pub(crate) type BeforeRootJobsHook = Box<dyn FnOnce(&str) -> PublicationRollback + Send>;

/// Optional metadata stored on the DAG execution row.
#[derive(Debug, Clone, Default)]
pub struct DagExecutionMetadata {
    pub segment_index: Option<u32>,
    pub segment_source: Option<String>,
    /// Per-segment video/danmu pairing of a paired-segment or session-complete
    /// pipeline; also copied into every step job.
    pub manifest: Option<PipelineInputManifest>,
}

/// Session and streamer metadata propagated to every job in a DAG run.
#[derive(Debug, Clone, Default)]
pub struct DagRunContext {
    pub streamer_id: Option<String>,
    pub session_id: Option<String>,
    pub streamer_name: Option<String>,
    pub session_title: Option<String>,
    pub platform: Option<String>,
    pub session_start: Option<DateTime<Utc>>,
    /// Copied into every step job; taken from `DagExecutionMetadata` at
    /// publication and from the DAG row afterwards.
    pub manifest: Option<Arc<PipelineInputManifest>>,
}

impl DagRunContext {
    /// Context for steps materialized after publication. The manifest comes from
    /// the DAG row rather than a sibling job so downstream steps do not depend on
    /// another job's state surviving.
    fn for_downstream(dag: &DagExecutionDbModel, meta: JobStateMeta) -> Self {
        Self {
            streamer_id: dag.streamer_id.clone(),
            session_id: dag.session_id.clone(),
            streamer_name: meta.streamer_name,
            session_title: meta.session_title,
            platform: meta.platform,
            session_start: meta.session_start,
            manifest: input_manifest(dag),
        }
    }
}

/// Parse the manifest stored on a DAG row; a malformed value is logged and
/// treated as absent so the DAG can still advance with stem-based pairing.
pub(crate) fn input_manifest(dag: &DagExecutionDbModel) -> Option<Arc<PipelineInputManifest>> {
    json::parse_optional::<PipelineInputManifest>(
        dag.input_manifest.as_deref(),
        JsonContext::DagExecutionField {
            dag_execution_id: &dag.id,
            field: "input_manifest",
        },
        "Invalid input_manifest JSON; pairing falls back to file stems",
    )
    .map(Arc::new)
}

/// Notification emitted when a DAG reaches a terminal state.
#[derive(Debug, Clone)]
pub struct DagCompletionInfo {
    pub dag_id: String,
    pub streamer_id: Option<String>,
    pub session_id: Option<String>,
    pub succeeded: bool,
    /// Outputs from leaf steps (best-effort; may be empty for delete/move DAGs).
    pub leaf_outputs: Vec<String>,
}

pub(super) struct LeafOutputs {
    pub paths: Vec<String>,
    /// False when a leaf row is missing or its stored output array is malformed.
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct DagJobCompletedUpdate {
    pub new_job_ids: Vec<String>,
    pub completion: Option<DagCompletionInfo>,
}

#[derive(Debug, Clone)]
pub struct DagJobFailedUpdate {
    pub cancelled_count: u64,
    pub completion: Option<DagCompletionInfo>,
}

/// An automatic retry planned for a failed step job under the step's
/// [`crate::database::models::StepRetryPolicy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRetry {
    /// When the manager's retry sweeper may re-queue the job.
    pub retry_after: DateTime<Utc>,
    /// Number of the attempt that will run next; the first run is attempt 1.
    pub next_attempt: u32,
    /// The step's attempt budget, including the first run.
    pub max_attempts: u32,
}

impl PlannedRetry {
    /// The job's error message while it waits for this retry: the job list
    /// shows the job as failed until then, so the message says what comes next.
    fn describe(&self, error: &str) -> String {
        format!(
            "{error}; attempt {} of {} failed, retrying at {}",
            self.next_attempt - 1,
            self.max_attempts,
            self.retry_after
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
    }
}

/// Why a step job attempt failed, as the worker observed it. Decides whether
/// the step's retry policy applies: another attempt is only worth scheduling
/// when the same job could succeed next time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepFailureKind {
    /// `Processor::process` returned an error, or handled only part of a batch.
    ProcessorError,
    /// The attempt outlived its timeout.
    Timeout,
    /// The job's inputs cannot be handled by its processor at all: a
    /// multi-input job routed to a processor without batch support. The same
    /// inputs would be rejected again.
    NonBatchInput,
    /// The attempt's start could not be persisted, so the processor never ran.
    /// Recording a retry needs the job-row write that just failed, so the
    /// failure is applied at once.
    ExecutionStart,
}

impl StepFailureKind {
    fn retryable(self) -> bool {
        matches!(self, Self::ProcessorError | Self::Timeout)
    }
}

/// A failed attempt of a step job, reported by the worker that ran it.
#[derive(Debug, Clone, Copy)]
pub struct FailedStepAttempt<'a> {
    pub job_id: &'a str,
    /// Retries the attempt already sat on top of; the first run has none.
    pub retry_count: i32,
    pub error: &'a str,
    pub kind: StepFailureKind,
}

/// What became of a failed step job attempt.
#[derive(Debug)]
pub enum StepFailureOutcome {
    /// Another attempt is scheduled under the step's retry policy: the step
    /// stays PROCESSING and the workflow keeps running.
    Retried(PlannedRetry),
    /// The failure reached the workflow: the step is FAILED and the steps
    /// that depend on it are cancelled.
    Failed(DagJobFailedUpdate),
    /// The step had already settled; nothing changed.
    Ignored,
}

/// What retrying a workflow's failed and cancelled branches did.
#[derive(Debug)]
pub struct DagRetryUpdate {
    /// Steps that were FAILED or CANCELLED when the retry began.
    pub retryable_steps: usize,
    /// Of those, the steps the retry restarted or settled: no longer FAILED
    /// or CANCELLED afterwards.
    pub retried_steps: usize,
    /// Steps whose job had completed while the earlier run was being
    /// cancelled; their result was replayed instead of re-running the job.
    pub reconciled_steps: usize,
    /// Jobs the retry restarted, for the caller to announce.
    pub restarted_jobs: Vec<Job>,
    /// Every job the retry restarted or created, in the order it did so.
    pub job_ids: Vec<String>,
    /// Terminal outcomes the retry itself reached, in order: a no-op step or
    /// a replayed result can settle the whole workflow.
    pub completions: Vec<DagCompletionInfo>,
}

/// Why [`DagScheduler::retry_dag`] did not retry a workflow.
#[derive(Debug)]
pub enum DagRetryError {
    /// The retry was refused before any row changed: the DAG is not terminal,
    /// nothing in it is retryable, or a job it would restart is missing or
    /// still active. The DAG is as it was.
    Rejected(Error),
    /// Restarting broke down after the rows were reset. The DAG was failed
    /// again with the retry error so it is terminal, and retryable, instead
    /// of stranded in PROCESSING; `completion` is that failure's, for the
    /// caller to forward like any other.
    Refailed {
        error: Error,
        completion: Option<Box<DagCompletionInfo>>,
    },
}

impl From<Error> for DagRetryError {
    fn from(error: Error) -> Self {
        Self::Rejected(error)
    }
}

/// What [`DagScheduler::retry_dag`] does with a retryable step's job.
enum RetryAction {
    /// The job failed or was cancelled: it runs again.
    Restart(Job),
    /// The job completed while the earlier run was being cancelled: its
    /// result is replayed so the step and its dependents advance.
    Replay(Job),
}

/// Result of creating a DAG pipeline.
#[derive(Debug, Clone)]
pub struct DagCreationResult {
    /// ID of the created DAG execution.
    pub dag_id: String,
    /// IDs of the root jobs (first jobs to run).
    pub root_job_ids: Vec<String>,
    /// Total number of steps in the DAG.
    pub total_steps: usize,
}

/// DAG Scheduler for orchestrating DAG pipeline execution.
pub struct DagScheduler {
    job_queue: Arc<JobQueue>,
    dag_repository: Arc<dyn DagRepository>,
    job_repository: Arc<dyn JobRepository>,
}

impl DagScheduler {
    /// Create a new DagScheduler.
    pub fn new(
        job_queue: Arc<JobQueue>,
        dag_repository: Arc<dyn DagRepository>,
        job_repository: Arc<dyn JobRepository>,
    ) -> Self {
        Self {
            job_queue,
            dag_repository,
            job_repository,
        }
    }

    pub(super) fn collect_leaf_outputs_from_step_executions(
        def: &DagPipelineDefinition,
        step_execs: &[DagStepExecutionDbModel],
    ) -> LeafOutputs {
        // `get_steps_by_dag` does not guarantee ordering, but output order matters for
        // downstream uses (e.g. concat). Collect leaf outputs in the leaf-step order
        // defined by the DAG definition and de-duplicate while preserving order.
        let mut exec_by_step_id: HashMap<&str, &DagStepExecutionDbModel> =
            HashMap::with_capacity(step_execs.len());
        for exec in step_execs {
            exec_by_step_id.insert(exec.step_id.as_str(), exec);
        }

        let mut seen = HashSet::<String>::new();
        let mut outputs = Vec::new();
        let mut complete = true;

        for leaf in def.leaf_steps() {
            let Some(exec) = exec_by_step_id.get(leaf.id.as_str()) else {
                complete = false;
                continue;
            };
            let step_outputs = match exec.outputs.as_deref() {
                None => Vec::new(),
                Some(raw) => match serde_json::from_str::<Vec<String>>(raw) {
                    Ok(outputs) => outputs,
                    Err(_) => {
                        complete = false;
                        // Keep the model's contextual diagnostic and empty fallback.
                        exec.get_outputs()
                    }
                },
            };
            for output in step_outputs {
                if seen.insert(crate::utils::fs::path_dedup_key(&output)) {
                    outputs.push(output);
                }
            }
        }

        LeafOutputs {
            paths: outputs,
            complete,
        }
    }

    async fn collect_leaf_outputs(&self, dag: &DagExecutionDbModel) -> Result<Vec<String>> {
        let Some(def) = dag.get_dag_definition() else {
            return Ok(Vec::new());
        };

        if def.leaf_steps().is_empty() {
            return Ok(Vec::new());
        }

        let step_execs = self.dag_repository.get_steps_by_dag(&dag.id).await?;
        Ok(Self::collect_leaf_outputs_from_step_executions(&def, &step_execs).paths)
    }

    /// Terminal notifications share one best-effort artifact projection. The
    /// caller supplies success according to the transition it is reporting.
    async fn completion_info(
        &self,
        dag: &DagExecutionDbModel,
        succeeded: bool,
    ) -> Option<DagCompletionInfo> {
        if !dag.get_status().is_some_and(|status| status.is_terminal()) {
            return None;
        }
        Some(DagCompletionInfo {
            dag_id: dag.id.clone(),
            streamer_id: dag.streamer_id.clone(),
            session_id: dag.session_id.clone(),
            succeeded,
            leaf_outputs: self.collect_leaf_outputs(dag).await.unwrap_or_default(),
        })
    }

    /// Create a new DAG pipeline execution.
    ///
    /// This creates:
    /// 1. A DAG execution record
    /// 2. Step execution records for all steps
    /// 3. Jobs for all root steps (steps with no dependencies)
    pub async fn create_dag_pipeline(
        &self,
        dag_definition: DagPipelineDefinition,
        input_paths: &[String],
        context: DagRunContext,
    ) -> Result<DagCreationResult> {
        self.create_dag_pipeline_with_hook(dag_definition, input_paths, context, None, None)
            .await
    }

    /// Create a new DAG pipeline execution with an optional hook called before the atomic
    /// publication makes its root jobs claimable. The hook returns a callback to undo its
    /// registration if publication fails. A successfully published DAG retains registration,
    /// including when subsequent in-memory enqueueing fails.
    pub async fn create_dag_pipeline_with_hook(
        &self,
        dag_definition: DagPipelineDefinition,
        input_paths: &[String],
        context: DagRunContext,
        metadata: Option<DagExecutionMetadata>,
        before_root_jobs: Option<BeforeRootJobsHook>,
    ) -> Result<DagCreationResult> {
        // 1. Validate DAG structure
        dag_definition.validate()?;

        // 2. Build the DAG execution and all records required for publication.
        let mut context = context;
        let mut dag_exec = DagExecutionDbModel::new(
            &dag_definition,
            context.streamer_id.clone(),
            context.session_id.clone(),
        );
        dag_exec.status = DagExecutionStatus::Processing.as_str().to_string();
        if let Some(meta) = metadata {
            dag_exec.segment_index = meta.segment_index.map(i64::from);
            dag_exec.segment_source = meta.segment_source;
            if let Some(manifest) = meta.manifest {
                dag_exec.input_manifest = Some(serde_json::to_string(&manifest)?);
                context.manifest = Some(Arc::new(manifest));
            }
        }
        let dag_id = dag_exec.id.clone();
        let mut step_executions = Vec::with_capacity(dag_definition.steps.len());

        for dag_step in &dag_definition.steps {
            let step_exec =
                DagStepExecutionDbModel::new(&dag_id, &dag_step.id, &dag_step.depends_on);
            step_executions.push(step_exec);
        }

        let root_steps = dag_definition.root_steps();
        let mut root_job_ids = Vec::with_capacity(root_steps.len());
        let mut root_job_models = Vec::with_capacity(root_steps.len());
        let mut root_jobs = Vec::with_capacity(root_steps.len());

        for root_step in root_steps {
            let step_exec = step_executions
                .iter_mut()
                .find(|step| step.step_id == root_step.id)
                .ok_or_else(|| Error::Validation("Step execution not found".into()))?;
            let (job_db, job) = Self::build_step_job(
                &dag_id,
                &step_exec.id,
                root_step,
                input_paths.to_vec(),
                &context,
            )?;
            step_exec.job_id = Some(job_db.id.clone());
            step_exec.status = DagStepStatus::Processing.as_str().to_string();
            root_job_ids.push(job_db.id.clone());
            root_job_models.push(job_db);
            root_jobs.push(job);
        }

        let rollback = before_root_jobs.map(|hook| hook(&dag_id));

        if let Err(error) = self
            .dag_repository
            .publish_dag(&dag_exec, &step_executions, &root_job_models)
            .await
        {
            if let Some(rollback) = rollback {
                rollback();
            }
            return Err(error);
        }
        // Do not roll back on cancellation of an in-flight publication: commit may already
        // have succeeded without the caller observing it. Only a definite error rolls back.
        drop(rollback);

        info!(
            dag_id = %dag_id,
            total_steps = %dag_definition.steps.len(),
            "Published DAG pipeline execution"
        );

        for job in root_jobs {
            self.job_queue.enqueue_existing(job).await?;
        }

        Ok(DagCreationResult {
            dag_id,
            root_job_ids,
            total_steps: dag_definition.steps.len(),
        })
    }

    async fn fail_dag_internal(
        &self,
        dag_id: &str,
        error: &str,
    ) -> Result<Option<DagCompletionInfo>> {
        let cancelled = self
            .dag_repository
            .fail_dag_and_cancel_steps(dag_id, error)
            .await?;
        for job_id in &cancelled {
            if let Err(cancel_error) = self.job_queue.cancel_job(job_id).await {
                warn!(
                    job_id,
                    error = %cancel_error,
                    "Failed to cancel job while failing DAG"
                );
            }
        }

        let dag = self.dag_repository.get_dag(dag_id).await?;
        Ok(self
            .completion_info(
                &dag,
                dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await)
    }

    /// Fail a DAG that is not terminal, cancelling its remaining steps and jobs.
    /// Returns the completion to forward when the DAG became terminal through
    /// this call; a DAG that was already terminal is left as it is and yields
    /// no completion, so its earlier outcome is not replayed.
    pub async fn fail_dag(&self, dag_id: &str, error: &str) -> Result<Option<DagCompletionInfo>> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        if dag.get_status().is_some_and(|status| status.is_terminal()) {
            return Ok(None);
        }
        self.fail_dag_internal(dag_id, error).await
    }

    /// Fail the DAG of a step whose workflow cannot advance.
    async fn fail_dag_for_step(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<Option<DagCompletionInfo>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;
        self.fail_dag_internal(&step.dag_id, error).await
    }

    /// Advance the workflow after a step's job completed: the worker's entry
    /// point for a job outcome, wrapping [`Self::on_job_completed`]. When the
    /// workflow cannot advance from the result, it is failed with that error
    /// so it settles instead of waiting for a completion that will not come
    /// again; the returned update then carries the failure's completion.
    pub async fn on_job_attempt_completed(
        &self,
        dag_step_execution_id: &str,
        outputs: &[String],
        streamer_name: Option<&str>,
        session_title: Option<&str>,
        platform: Option<&str>,
        session_start: Option<DateTime<Utc>>,
    ) -> Result<DagJobCompletedUpdate> {
        match self
            .on_job_completed(
                dag_step_execution_id,
                outputs,
                streamer_name,
                session_title,
                platform,
                session_start,
            )
            .await
        {
            Ok(update) => Ok(update),
            Err(error) => {
                error!(
                    dag_step_execution_id,
                    %error,
                    "Failed to advance the workflow after a step completed; failing it"
                );
                let completion = self
                    .fail_dag_for_step(
                        dag_step_execution_id,
                        &format!("DAG scheduler error: {error}"),
                    )
                    .await?;
                Ok(DagJobCompletedUpdate {
                    new_job_ids: Vec::new(),
                    completion,
                })
            }
        }
    }

    /// Apply a failed attempt of a step's job to its workflow: the worker's
    /// entry point for a job outcome.
    ///
    /// Decides first whether the step's retry policy grants another attempt,
    /// then has `persist_failure` record the attempt on the job row with the
    /// message the job list should show: the error, extended with the retry
    /// time when one is planned. With the row recorded, a planned retry is
    /// stored for the manager's sweeper and the step stays PROCESSING;
    /// otherwise the failure reaches the step and its dependents through
    /// [`Self::on_job_failed`]. A retry that cannot be recorded, or whose job
    /// row could not be written, falls back to failing the step: nothing else
    /// would advance it.
    pub async fn on_job_attempt_failed<F, Fut>(
        &self,
        dag_step_execution_id: &str,
        attempt: FailedStepAttempt<'_>,
        persist_failure: F,
    ) -> Result<StepFailureOutcome>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<JobFailureOutcome>>,
    {
        let planned = if attempt.kind.retryable() {
            self.plan_step_retry(dag_step_execution_id, attempt.retry_count)
                .await
                .unwrap_or_else(|error| {
                    warn!(
                        job_id = %attempt.job_id,
                        %error,
                        "Could not plan a step retry; failing the step"
                    );
                    None
                })
        } else {
            None
        };
        let message = match &planned {
            Some(planned) => planned.describe(attempt.error),
            None => attempt.error.to_string(),
        };

        let recorded = persist_failure(message.clone()).await;
        if matches!(recorded, Ok(JobFailureOutcome::Unchanged)) {
            return Ok(StepFailureOutcome::Ignored);
        }
        if let Err(error) = &recorded {
            error!(job_id = %attempt.job_id, %error, "Failed to persist pipeline job failure");
        }
        if let (Some(planned), Ok(JobFailureOutcome::Transitioned)) = (planned, &recorded) {
            match self
                .job_queue
                .schedule_retry(attempt.job_id, planned.retry_after)
                .await
            {
                Ok(()) => {
                    info!(
                        job_id = %attempt.job_id,
                        dag_step_execution_id,
                        next_attempt = planned.next_attempt,
                        max_attempts = planned.max_attempts,
                        retry_after = %planned.retry_after,
                        "Step failed; automatic retry scheduled"
                    );
                    return Ok(StepFailureOutcome::Retried(planned));
                }
                Err(error) => warn!(
                    job_id = %attempt.job_id,
                    %error,
                    "Could not schedule the step retry; failing the step"
                ),
            }
        }

        Ok(
            match self
                .apply_step_failure(dag_step_execution_id, &message)
                .await?
            {
                Some(update) => StepFailureOutcome::Failed(update),
                None => StepFailureOutcome::Ignored,
            },
        )
    }

    /// Handle job completion for a DAG step.
    ///
    /// This:
    /// 1. Marks the step as completed with outputs
    /// 2. Checks if any dependent steps are now ready (all deps complete)
    /// 3. Creates jobs for ready steps with merged inputs (fan-in)
    /// 4. Checks if the DAG is complete
    ///
    /// Returns the IDs of newly created jobs.
    pub async fn on_job_completed(
        &self,
        dag_step_execution_id: &str,
        outputs: &[String],
        streamer_name: Option<&str>,
        session_title: Option<&str>,
        platform: Option<&str>,
        session_start: Option<DateTime<Utc>>,
    ) -> Result<DagJobCompletedUpdate> {
        let streamer_name = streamer_name.map(ToString::to_string);
        let session_title = session_title.map(ToString::to_string);
        let platform = platform.map(ToString::to_string);

        let crate::database::repositories::StepCompletion {
            step,
            dag,
            ready_steps,
        } = self
            .dag_repository
            .complete_step_and_check_dependents(dag_step_execution_id, outputs)
            .await?;
        info!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            outputs_count = %outputs.len(),
            "DAG step completed"
        );

        let mut new_job_ids = Vec::new();

        if !ready_steps.is_empty() {
            // Definition and metadata come from the same transaction as readiness.
            let dag_def = dag
                .get_dag_definition()
                .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
            let meta = JobStateMeta {
                streamer_name,
                session_title,
                platform,
                session_start,
                manifest: None,
                timeout_secs: None,
            };

            match self
                .materialize_ready_steps(dag, &dag_def, ready_steps, &meta)
                .await
            {
                Ok((job_ids, _)) => new_job_ids = job_ids,
                Err((job_ids, e)) => {
                    let err_msg = format!("Failed to create downstream DAG job: {}", e);
                    warn!(
                        dag_id = %step.dag_id,
                        error = %err_msg,
                        "Failing DAG due to scheduler error"
                    );
                    let completion = self.fail_dag_internal(&step.dag_id, &err_msg).await?;
                    return Ok(DagJobCompletedUpdate {
                        new_job_ids: job_ids,
                        completion,
                    });
                }
            }
        } else {
            debug!(
                dag_id = %step.dag_id,
                step_id = %step.step_id,
                "No dependent steps are ready yet"
            );
        }

        // Check if DAG reached a terminal state.
        let updated_dag = self.dag_repository.get_dag(&step.dag_id).await?;
        let completion = self
            .completion_info(
                &updated_dag,
                updated_dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await;

        Ok(DagJobCompletedUpdate {
            new_job_ids,
            completion,
        })
    }

    /// Handle job failure for a DAG step.
    ///
    /// Fail-fast is scoped to the failed step's descendants:
    /// 1. Marks the step as failed and records the error on the DAG
    /// 2. Cancels the steps that depend on it, directly or indirectly
    /// 3. Signals cancellation for the cancelled steps' jobs
    /// 4. Finalizes the DAG as FAILED once no step is active; independent
    ///    branches keep running and the last one to settle finalizes it
    ///
    /// Returns the count of cancelled jobs and, when the DAG became terminal
    /// through this failure, its completion. A failure reported for a step
    /// that has already settled changes nothing and returns an empty update.
    pub async fn on_job_failed(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<DagJobFailedUpdate> {
        Ok(self
            .apply_step_failure(dag_step_execution_id, error)
            .await?
            .unwrap_or(DagJobFailedUpdate {
                cancelled_count: 0,
                completion: None,
            }))
    }

    /// [`Self::on_job_failed`], returning `None` for a step that had already
    /// settled.
    async fn apply_step_failure(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<Option<DagJobFailedUpdate>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;

        let failure_error = format!("Step '{}' failed: {}", step.step_id, error);
        let Some(processing_job_ids) = self
            .dag_repository
            .fail_step_and_cancel_dag(dag_step_execution_id, &failure_error)
            .await?
        else {
            debug!(
                dag_id = %step.dag_id,
                step_id = %step.step_id,
                "Ignoring failure for terminal DAG step"
            );
            return Ok(None);
        };

        error!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            error = %error,
            "DAG step failed; cancelling the steps that depend on it"
        );

        // Cancel the jobs of the cancelled dependents.
        let mut cancelled_count = 0u64;
        for job_id in &processing_job_ids {
            if let Err(e) = self.job_queue.cancel_job(job_id).await {
                warn!(
                    job_id = %job_id,
                    error = %e,
                    "Failed to cancel processing job"
                );
            } else {
                cancelled_count += 1;
            }
        }

        let updated_dag = match self.dag_repository.get_dag(&step.dag_id).await {
            Ok(dag) => Some(dag),
            Err(error) => {
                warn!(dag_id = %step.dag_id, %error, "Failed to reload failed DAG");
                None
            }
        };
        let completion = match &updated_dag {
            Some(dag) => self.completion_info(dag, false).await,
            None => None,
        };
        if completion.is_some() {
            info!(
                dag_id = %step.dag_id,
                cancelled_jobs = %cancelled_count,
                "DAG failed; no step is still running"
            );
        } else {
            info!(
                dag_id = %step.dag_id,
                cancelled_jobs = %cancelled_count,
                "DAG step failed; independent branches keep running until they settle"
            );
        }

        Ok(Some(DagJobFailedUpdate {
            cancelled_count,
            completion,
        }))
    }

    /// Whether the failed job of `dag_step_execution_id` gets another attempt
    /// under its step's retry policy, and when. `retry_count` is the number of
    /// retries the job has already had. `None` when the step has no policy,
    /// the budget is spent, or the step or its DAG is no longer active, in
    /// which case the failure is applied to the workflow as usual.
    pub async fn plan_step_retry(
        &self,
        dag_step_execution_id: &str,
        retry_count: i32,
    ) -> Result<Option<PlannedRetry>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;
        if step.get_status() != Some(crate::database::models::DagStepStatus::Processing) {
            return Ok(None);
        }
        let dag = self.dag_repository.get_dag(&step.dag_id).await?;
        if dag.get_status().is_some_and(|status| status.is_terminal()) {
            return Ok(None);
        }
        let Some(policy) = dag
            .get_dag_definition()
            .and_then(|definition| definition.get_step(&step.step_id)?.retry.clone())
        else {
            return Ok(None);
        };
        let attempts_made = u32::try_from(retry_count)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        if attempts_made >= policy.max_attempts {
            return Ok(None);
        }
        let next_attempt = attempts_made.saturating_add(1);
        let wait =
            chrono::Duration::from_std(policy.backoff_before(next_attempt)).unwrap_or_else(|_| {
                chrono::Duration::seconds(
                    crate::database::models::StepRetryPolicy::MAX_BACKOFF_SECS as i64,
                )
            });
        Ok(Some(PlannedRetry {
            retry_after: Utc::now() + wait,
            next_attempt,
            max_attempts: policy.max_attempts,
        }))
    }

    /// Whether `job_id` is still the attempt `dag_step_execution_id` is waiting
    /// on in a DAG that has not settled, so a scheduled retry of it is still
    /// wanted. A step that was cancelled, failed by recovery or deleted in the
    /// meantime answers `false`.
    pub async fn step_awaits_job(&self, dag_step_execution_id: &str, job_id: &str) -> Result<bool> {
        let step = match self.dag_repository.get_step(dag_step_execution_id).await {
            Ok(step) => step,
            Err(Error::NotFound { .. }) => return Ok(false),
            Err(error) => return Err(error),
        };
        if step.get_status() != Some(crate::database::models::DagStepStatus::Processing)
            || step.job_id.as_deref() != Some(job_id)
        {
            return Ok(false);
        }
        let dag = match self.dag_repository.get_dag(&step.dag_id).await {
            Ok(dag) => dag,
            Err(Error::NotFound { .. }) => return Ok(false),
            Err(error) => return Err(error),
        };
        Ok(!dag.get_status().is_some_and(|status| status.is_terminal()))
    }

    fn build_step_job(
        dag_id: &str,
        step_execution_id: &str,
        dag_step: &crate::database::models::DagStep,
        inputs: Vec<String>,
        context: &DagRunContext,
    ) -> Result<(JobDbModel, Job)> {
        // Get processor and config from the step
        let (processor, config) = match &dag_step.step {
            PipelineStep::Inline { processor, config } => (processor.clone(), config.to_string()),
            PipelineStep::Preset { name } => {
                // For presets, we need the manager to resolve them
                // For now, use the preset name as the job type
                (name.clone(), "{}".to_string())
            }
            PipelineStep::Workflow { name } => {
                return Err(Error::Validation(format!(
                    "Workflow '{}' should be resolved before DAG creation",
                    name
                )));
            }
        };

        // Create the job
        let inputs_json = serde_json::to_string(&inputs)?;

        let mut job_db = JobDbModel::new_pipeline_step(
            &processor,
            inputs_json,
            "[]".to_string(),
            0, // priority
            context.streamer_id.clone(),
            context.session_id.clone(),
        );
        job_db.config = config;
        job_db.dag_step_execution_id = Some(step_execution_id.to_string());
        // delete_jobs_by_pipeline / cancel_jobs_by_pipeline and the JobFilters
        // pipeline_id filter all match on this column, so it must be set on the
        // inserted row, not only on the in-memory Job built below.
        job_db.pipeline_id = Some(dag_id.to_string());

        // Build the in-memory Job before persisting so job_state_json is the
        // single source of the state shape (its inverse parse_job_state is
        // what db_model_to_job and recover_placeholder_metadata read back).
        let job = Job {
            id: job_db.id.clone(),
            job_type: processor,
            inputs,
            outputs: Vec::new(),
            priority: 0,
            status: JobStatus::Pending,
            streamer_id: job_db.streamer_id.clone().unwrap_or_default(),
            session_id: job_db.session_id.clone().unwrap_or_default(),
            streamer_name: context.streamer_name.clone(),
            session_title: context.session_title.clone(),
            platform: context.platform.clone(),
            session_start: context.session_start,
            config: Some(job_db.config.clone()),
            created_at: chrono::Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            retry_after: None,
            timeout_secs: dag_step.timeout_secs,
            pipeline_id: Some(dag_id.to_string()),
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: Some(step_execution_id.to_string()),
            manifest: context.manifest.clone(),
        };
        job_db.state = job_state_json(&job);

        Ok((job_db, job))
    }

    /// Materialize `ready` steps in order: a step whose dependencies produced no
    /// outputs completes as a no-op with empty outputs, which may make further
    /// steps ready; every other step gets a job. On error the job ids created
    /// so far are returned with it so the caller can report them.
    async fn materialize_ready_steps(
        &self,
        dag: DagExecutionDbModel,
        dag_def: &DagPipelineDefinition,
        ready: Vec<ReadyStep>,
        meta: &JobStateMeta,
    ) -> std::result::Result<(Vec<String>, DagExecutionDbModel), (Vec<String>, Error)> {
        let mut dag = dag;
        let mut queue = VecDeque::from(ready);
        let mut new_job_ids = Vec::new();
        while let Some(ReadyStep {
            step: ready_step,
            merged_inputs,
        }) = queue.pop_front()
        {
            let result: Result<()> = async {
                let dag_step = dag_def.get_step(&ready_step.step_id).ok_or_else(|| {
                    Error::Validation(format!(
                        "Step '{}' not found in DAG definition",
                        ready_step.step_id
                    ))
                })?;

                if merged_inputs.is_empty() && !Self::runs_without_inputs(&dag_step.step) {
                    // Processors such as `delete` reject an empty input list, and a
                    // dependency that produced nothing (an `rclone` move, a `delete`)
                    // is a normal outcome, not a failure of this step.
                    info!(
                        dag_id = %dag.id,
                        step_id = %ready_step.step_id,
                        "Dependencies produced no outputs; completing step as a no-op"
                    );
                    let completion = self
                        .dag_repository
                        .complete_ready_step_without_job(&ready_step.id, &[])
                        .await?;
                    dag = completion.dag;
                    queue.extend(completion.ready_steps);
                    return Ok(());
                }

                info!(
                    dag_id = %dag.id,
                    step_id = %ready_step.step_id,
                    inputs_count = %merged_inputs.len(),
                    "Creating job for ready step (fan-in merge)"
                );
                if let Some(job_id) = self
                    .create_step_job(
                        &dag.id,
                        &ready_step.id,
                        dag_step,
                        merged_inputs,
                        &DagRunContext::for_downstream(&dag, meta.clone()),
                    )
                    .await?
                {
                    new_job_ids.push(job_id);
                }
                Ok(())
            }
            .await;
            if let Err(error) = result {
                return Err((new_job_ids, error));
            }
        }
        Ok((new_job_ids, dag))
    }

    /// Whether the step's processor runs a job even with no inputs. An `execute`
    /// step after an upload or delete is a common way to run a notification or
    /// clean-up script, and its command does not need `{input}`.
    fn runs_without_inputs(step: &PipelineStep) -> bool {
        let processor = match step {
            PipelineStep::Inline { processor, .. } => processor.as_str(),
            PipelineStep::Preset { name } => name.as_str(),
            PipelineStep::Workflow { .. } => return false,
        };
        matches!(processor, "execute" | "command")
    }

    /// Create a job for a DAG step.
    ///
    /// Returns `Ok(None)` when the step already has a job: retry fan-out and
    /// startup recovery compute readiness from rows a live completion may have
    /// advanced in the meantime, and that step then needs nothing from this call.
    async fn create_step_job(
        &self,
        dag_id: &str,
        step_execution_id: &str,
        dag_step: &crate::database::models::DagStep,
        inputs: Vec<String>,
        context: &DagRunContext,
    ) -> Result<Option<String>> {
        let (job_db, job) =
            Self::build_step_job(dag_id, step_execution_id, dag_step, inputs, context)?;
        let job_id = job_db.id.clone();

        if let Err(error) = self
            .dag_repository
            .create_job_for_step(step_execution_id, &job_db)
            .await
        {
            let materialized_elsewhere = matches!(error, Error::InvalidStateTransition { .. })
                && self
                    .dag_repository
                    .get_step(step_execution_id)
                    .await
                    .is_ok_and(|step| step.job_id.is_some());
            if materialized_elsewhere {
                warn!(
                    dag_id = %dag_id,
                    step_id = %dag_step.id,
                    "DAG step already has a job; skipping duplicate materialization"
                );
                return Ok(None);
            }
            return Err(error);
        }

        // Add to job queue cache and notify workers
        self.job_queue.enqueue_existing(job).await?;

        info!(
            dag_id = %dag_id,
            step_id = %dag_step.id,
            job_id = %job_id,
            processor = %job_db.job_type,
            "Created and enqueued job for DAG step"
        );

        Ok(Some(job_id))
    }

    /// Get DAG execution status.
    pub async fn get_dag_status(&self, dag_id: &str) -> Result<DagExecutionDbModel> {
        self.dag_repository.get_dag(dag_id).await
    }

    /// Get all step executions for a DAG.
    pub async fn get_dag_steps(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        self.dag_repository.get_steps_by_dag(dag_id).await
    }

    /// Get a single DAG step execution by ID.
    pub async fn get_step_execution(
        &self,
        dag_step_execution_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        self.dag_repository.get_step(dag_step_execution_id).await
    }

    /// Cancel a DAG execution.
    pub async fn cancel_dag_with_completion(&self, dag_id: &str) -> Result<DagJobFailedUpdate> {
        let dag = self.dag_repository.get_dag(dag_id).await?;

        if dag.get_status().map(|s| s.is_terminal()).unwrap_or(false) {
            return Err(Error::DagAlreadyTerminal {
                dag_id: dag_id.to_string(),
            });
        }

        let cancelled = self
            .dag_repository
            .cancel_dag_and_cancel_steps(dag_id, "Cancelled by user")
            .await?;

        // Cancel any processing jobs
        let mut cancelled_count = 0u64;
        for job_id in &cancelled {
            if self.job_queue.cancel_job(job_id).await.is_ok() {
                cancelled_count += 1;
            }
        }

        let updated_dag = match self.dag_repository.get_dag(dag_id).await {
            Ok(dag) => Some(dag),
            Err(error) => {
                warn!(dag_id, %error, "Failed to reload cancelled DAG");
                None
            }
        };
        let completion = match updated_dag {
            Some(dag) => self.completion_info(&dag, false).await,
            None => None,
        };

        Ok(DagJobFailedUpdate {
            cancelled_count,
            completion,
        })
    }

    /// Cancel a DAG execution.
    pub async fn cancel_dag(&self, dag_id: &str) -> Result<u64> {
        Ok(self
            .cancel_dag_with_completion(dag_id)
            .await?
            .cancelled_count)
    }

    /// Retry every failed or cancelled branch of a terminal workflow.
    ///
    /// Every FAILED or CANCELLED step is retried: one with a job has that job
    /// restarted; one without (its job creation failed, or it was cancelled
    /// before it was materialized) is materialized again once the reset
    /// unblocks it. A step whose job completed while the run was being
    /// cancelled has that result replayed instead. Every job is checked
    /// before any row changes, so a missing or still active job rejects the
    /// retry while the DAG is untouched and still retryable.
    ///
    /// `after_reset` runs once the rows are reset and before any job is
    /// restarted or materialized: a restarted job can settle the workflow
    /// again at once, and the caller may need to stop treating the earlier
    /// outcome as current before that can happen.
    pub async fn retry_dag(
        &self,
        dag_id: &str,
        after_reset: impl FnOnce(),
    ) -> std::result::Result<DagRetryUpdate, DagRetryError> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        if !matches!(
            dag.get_status(),
            Some(DagExecutionStatus::Failed | DagExecutionStatus::Cancelled)
        ) {
            return Err(
                Error::Validation("DAG is not in FAILED or CANCELLED status".to_string()).into(),
            );
        }

        let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;
        let retryable_steps: Vec<&DagStepExecutionDbModel> = steps
            .iter()
            .filter(|step| {
                matches!(
                    step.get_status(),
                    Some(DagStepStatus::Failed | DagStepStatus::Cancelled)
                )
            })
            .collect();
        if retryable_steps.is_empty() {
            return Err(Error::Validation(
                "No failed or cancelled steps found to retry".to_string(),
            )
            .into());
        }

        let mut actions = Vec::with_capacity(retryable_steps.len());
        for step in &retryable_steps {
            let Some(job_id) = step.job_id.as_deref() else {
                continue;
            };
            let job = self.job_queue.get_job(job_id).await?.ok_or_else(|| {
                Error::Validation(format!(
                    "Job {} of step '{}' no longer exists; the workflow cannot be retried",
                    job_id, step.step_id
                ))
            })?;
            let action = match job.status {
                JobStatus::Failed | JobStatus::Cancelled => RetryAction::Restart(job),
                JobStatus::Completed => RetryAction::Replay(job),
                JobStatus::Pending | JobStatus::Processing => {
                    return Err(Error::Validation(format!(
                        "Job {} of step '{}' is {} and cannot be retried",
                        job_id,
                        step.step_id,
                        job.status.as_str()
                    ))
                    .into());
                }
            };
            actions.push((*step, action));
        }

        // From here on every failure re-fails the DAG with the retry error, so
        // a partially restarted DAG never sits in PROCESSING with nothing to
        // advance it.
        if let Err(error) = self.dag_repository.reset_dag_for_retry(dag_id).await {
            return Err(self.refail_after_retry(dag_id, error).await);
        }
        after_reset();

        // A cancelled step whose dependencies had all completed (a parallel
        // branch cut short by fail-fast) gets no further completion event, so
        // it is materialized here; a no-op step can settle the DAG at once.
        let mut completions = Vec::new();
        let mut job_ids = match self.enqueue_now_ready_steps(dag_id).await {
            Ok(update) => {
                completions.extend(update.completion);
                update.new_job_ids
            }
            Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
        };

        let mut restarted_jobs = Vec::new();
        let mut reconciled_steps = 0usize;
        for (step, action) in actions {
            match action {
                RetryAction::Restart(job) => match self.job_queue.retry_job(&job.id).await {
                    Ok(job) => {
                        job_ids.push(job.id.clone());
                        restarted_jobs.push(job);
                    }
                    // Two retries of the same DAG can pass the terminal check
                    // before either reset commits; a job the other one already
                    // restarted needs nothing more from this call.
                    Err(error) if self.job_already_restarted(&job.id).await => {
                        debug!(
                            dag_id,
                            job_id = %job.id,
                            %error,
                            "Job was restarted by a concurrent retry"
                        );
                        job_ids.push(job.id);
                    }
                    Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
                },
                RetryAction::Replay(job) => {
                    match self
                        .on_job_completed(
                            &step.id,
                            &job.outputs,
                            job.streamer_name.as_deref(),
                            job.session_title.as_deref(),
                            job.platform.as_deref(),
                            job.session_start,
                        )
                        .await
                    {
                        Ok(update) => {
                            reconciled_steps += 1;
                            job_ids.extend(update.new_job_ids);
                            completions.extend(update.completion);
                        }
                        Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
                    }
                }
            }
        }

        // Steps settled by the retry itself (restarted, materialized, or
        // completed as no-ops) are those that are no longer FAILED or CANCELLED.
        let retryable_ids: HashSet<&str> = retryable_steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        // The reset alone makes every retryable step active again, so a
        // lower count means a restarted job already failed again; the retry
        // itself has taken effect either way, so an unreadable row set only
        // costs the exact count.
        let retried_steps = match self.dag_repository.get_steps_by_dag(dag_id).await {
            Ok(steps) => steps
                .iter()
                .filter(|step| {
                    retryable_ids.contains(step.id.as_str())
                        && !matches!(
                            step.get_status(),
                            Some(DagStepStatus::Failed | DagStepStatus::Cancelled)
                        )
                })
                .count(),
            Err(error) => {
                warn!(dag_id, %error, "Could not count the retried steps after the retry");
                retryable_steps.len()
            }
        };

        Ok(DagRetryUpdate {
            retryable_steps: retryable_steps.len(),
            retried_steps,
            reconciled_steps,
            restarted_jobs,
            job_ids,
            completions,
        })
    }

    /// Whether `job_id` is already queued or running, as after a concurrent retry.
    async fn job_already_restarted(&self, job_id: &str) -> bool {
        matches!(
            self.job_queue.get_job(job_id).await,
            Ok(Some(job)) if matches!(job.status, JobStatus::Pending | JobStatus::Processing)
        )
    }

    /// Fail a DAG whose retry broke down after its rows were reset, so it is
    /// terminal (and retryable) again instead of stranded in PROCESSING.
    async fn refail_after_retry(&self, dag_id: &str, error: Error) -> DagRetryError {
        let message = format!("Retry failed: {error}");
        let completion = match self.fail_dag(dag_id, &message).await {
            Ok(completion) => completion,
            Err(fail_error) => {
                warn!(
                    dag_id,
                    error = %fail_error,
                    "Failed to re-fail DAG after an unsuccessful retry"
                );
                None
            }
        };
        DagRetryError::Refailed {
            error,
            completion: completion.map(Box::new),
        }
    }

    /// Reconcile durable job results and unmaterialized ready steps before workers resume.
    pub async fn recover_dag_jobs(&self) -> Result<usize> {
        self.recover_dag_jobs_with_status()
            .await
            .map(|(count, _)| count)
    }

    pub(crate) async fn recover_dag_jobs_with_status(&self) -> Result<(usize, bool)> {
        let mut complete = true;
        let completed_steps = self
            .dag_repository
            .list_processing_steps_with_completed_jobs()
            .await?;
        let mut materialized_jobs = 0usize;

        // One unreadable row must not strand the rest of the reconciliation: `on_job_completed`
        // rejects a DAG whose `dag_definition` no longer parses, and that DAG's neighbours are
        // still recoverable. Each failure is isolated to its own step or DAG.
        for step in completed_steps {
            let Some(job_id) = step.job_id.as_deref() else {
                complete = false;
                continue;
            };
            let job = match self.job_repository.get_job(job_id).await {
                Ok(job) => job,
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %step.dag_id,
                        step_id = %step.step_id,
                        job_id = %job_id,
                        error = %e,
                        "Skipping DAG step reconciliation for unreadable job"
                    );
                    continue;
                }
            };
            if !serde_json::from_str::<serde_json::Value>(&job.state)
                .is_ok_and(|value| value.is_object())
                || job
                    .outputs
                    .as_deref()
                    .is_some_and(|raw| serde_json::from_str::<Vec<String>>(raw).is_err())
            {
                complete = false;
            }
            let metadata = parse_job_state(&job.state);
            match self
                .on_job_completed(
                    &step.id,
                    &job.get_outputs(),
                    metadata.streamer_name.as_deref(),
                    metadata.session_title.as_deref(),
                    metadata.platform.as_deref(),
                    metadata.session_start,
                )
                .await
            {
                Ok(update) => {
                    materialized_jobs = materialized_jobs.saturating_add(update.new_job_ids.len());
                }
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %step.dag_id,
                        step_id = %step.step_id,
                        error = %e,
                        "Failed to advance DAG step from its durable job result"
                    );
                }
            }
        }

        // A worker that failed its job but died before `on_job_failed` ran leaves the step
        // PROCESSING forever: nothing else reports on that job, and retry rejects a DAG
        // that is not terminal. Fail those DAGs now so they become retryable.
        for step in self
            .dag_repository
            .list_processing_steps_with_failed_jobs()
            .await?
        {
            let reason = match step.job_id.as_deref() {
                None => "the step's job row no longer exists".to_string(),
                Some(job_id) => match self.job_repository.get_job(job_id).await {
                    Ok(job) => format!(
                        "job {} is {}{}",
                        job_id,
                        job.status,
                        job.error
                            .as_deref()
                            .map(|error| format!(": {error}"))
                            .unwrap_or_default()
                    ),
                    Err(e) => {
                        complete = false;
                        warn!(
                            dag_id = %step.dag_id,
                            step_id = %step.step_id,
                            job_id = %job_id,
                            error = %e,
                            "Skipping stranded DAG step reconciliation for unreadable job"
                        );
                        continue;
                    }
                },
            };
            if let Err(e) = self.on_job_failed(&step.id, &reason).await {
                complete = false;
                warn!(
                    dag_id = %step.dag_id,
                    step_id = %step.step_id,
                    error = %e,
                    "Failed to fail DAG step whose job had already failed"
                );
            } else {
                warn!(
                    dag_id = %step.dag_id,
                    step_id = %step.step_id,
                    reason = %reason,
                    "Failed stranded DAG step during startup reconciliation"
                );
            }
        }

        for dag_id in self
            .dag_repository
            .list_dag_ids_with_unmaterialized_steps()
            .await?
        {
            match self.enqueue_now_ready_steps(&dag_id).await {
                Ok(update) => {
                    materialized_jobs = materialized_jobs.saturating_add(update.new_job_ids.len());
                }
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %dag_id,
                        error = %e,
                        "Failed to materialize jobs for ready DAG steps"
                    );
                }
            }
        }

        Ok((materialized_jobs, complete))
    }

    /// Materialize the ready steps of a DAG that no completion will reach:
    /// after a retry reset and during startup recovery.
    async fn enqueue_now_ready_steps(&self, dag_id: &str) -> Result<DagJobCompletedUpdate> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        let dag_def = dag
            .get_dag_definition()
            .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
        let ready = self.dag_repository.list_ready_steps(dag_id).await?;

        // The retry entry point only knows the dag_id, so the placeholder
        // metadata for jobs created here is recovered from a sibling step's
        // job row: create_step_job persisted streamer_name / session_title /
        // platform / session_start_ms in every job's state JSON. Relying on
        // the dequeue-time backfill (JobQueue::resolve_job_metadata) instead
        // would lose session_start whenever the live_sessions row was deleted
        // before the retry.
        let placeholder_meta = if ready.is_empty() {
            JobStateMeta::default()
        } else {
            let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;
            self.recover_placeholder_metadata(&steps).await
        };

        let (new_job_ids, dag) = self
            .materialize_ready_steps(dag, &dag_def, ready, &placeholder_meta)
            .await
            .map_err(|(_, error)| error)?;
        let completion = self
            .completion_info(
                &dag,
                dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await;

        Ok(DagJobCompletedUpdate {
            new_job_ids,
            completion,
        })
    }

    /// Recover the placeholder metadata persisted by [`Self::create_step_job`]
    /// in the job `state` JSON from the first sibling step that has a job row
    /// with any of those values set. Returns all-`None` when no step has a
    /// job yet or none carries metadata.
    async fn recover_placeholder_metadata(
        &self,
        steps: &[DagStepExecutionDbModel],
    ) -> JobStateMeta {
        for step in steps {
            let Some(job_id) = step.job_id.as_deref() else {
                continue;
            };
            let Ok(job) = self.job_repository.get_job(job_id).await else {
                continue;
            };

            // Siblings can carry an all-null state; only a row with stored
            // values can supply placeholder metadata for recovered steps.
            let meta = parse_job_state(&job.state);
            if meta.has_any() {
                return meta;
            }
        }

        JobStateMeta::default()
    }

    /// List DAG executions with optional status and session_id filters.
    pub async fn list_dags(
        &self,
        status: Option<&str>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        self.dag_repository
            .list_dags(status, session_id, limit, offset)
            .await
    }

    /// Count DAG executions with optional status and session_id filters.
    pub async fn count_dags(&self, status: Option<&str>, session_id: Option<&str>) -> Result<u64> {
        self.dag_repository.count_dags(status, session_id).await
    }

    /// Get statistics for a DAG execution.
    pub async fn get_dag_stats(
        &self,
        dag_id: &str,
    ) -> Result<crate::database::models::dag::DagExecutionStats> {
        self.dag_repository.get_dag_stats(dag_id).await
    }

    /// Permanently delete a DAG execution, all its steps, and associated jobs/logs.
    ///
    /// `delete_jobs_by_pipeline` removes rows regardless of status, so a job that is still
    /// pending or processing has to be stood down first: `JobQueue::cancel_job` signals the
    /// worker's cancellation token and settles the queue depth, and `JobQueue::forget_jobs`
    /// clears the caches that would otherwise keep entries for rows that no longer exist.
    pub async fn delete_dag(&self, dag_id: &str) -> Result<()> {
        // Verify DAG exists first
        self.dag_repository.get_dag(dag_id).await?;

        let job_ids: Vec<String> = self
            .job_repository
            .get_jobs_by_pipeline(dag_id)
            .await?
            .into_iter()
            .map(|job| job.id)
            .collect();

        for job_id in &job_ids {
            // A job that is already terminal, or already gone, needs no stand-down.
            if let Err(e) = self.job_queue.cancel_job(job_id).await
                && !matches!(
                    e,
                    Error::InvalidStateTransition { .. } | Error::NotFound { .. }
                )
            {
                warn!(
                    job_id = %job_id,
                    error = %e,
                    "Failed to cancel job while deleting DAG"
                );
            }
        }

        // Delete all associated jobs and their logs
        self.job_repository.delete_jobs_by_pipeline(dag_id).await?;
        self.job_queue.forget_jobs(&job_ids);

        // Delete the DAG (CASCADE deletes steps)
        self.dag_repository.delete_dag(dag_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn completion_reuses_atomic_snapshots_and_keeps_guarded_downstream_publication() {
        use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
        use crate::database::test_support::SqlTrace;
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
        let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
        let scheduler = DagScheduler::new(queue, dags.clone(), jobs.clone());
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("atomic snapshots"),
                &["input.flv".to_owned()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = dags.get_steps_by_dag(&created.dag_id).await.unwrap();
        let first = steps.iter().find(|step| step.step_id == "A").unwrap();
        let trace = SqlTrace::install(&pool).await;
        let update = scheduler
            .on_job_completed(
                &first.id,
                &["first.mp4".to_owned()],
                Some("Streamer"),
                Some("Title"),
                Some("twitch"),
                None,
            )
            .await
            .unwrap();
        assert_eq!(update.new_job_ids.len(), 1);
        assert!(update.completion.is_none());
        // One row fetch decides readiness inside the transaction; the only
        // other read is the DAG reload for the completion check.
        let statements = trace.statements();
        assert_eq!(
            statements
                .iter()
                .filter(|sql| sql.starts_with("SELECT "))
                .count(),
            2,
            "{statements:?}"
        );
        assert_eq!(
            statements
                .iter()
                .filter(|sql| sql.starts_with("SELECT * FROM dag_execution WHERE id ="))
                .count(),
            1
        );
        assert!(
            !statements
                .iter()
                .any(|sql| sql.starts_with("SELECT * FROM dag_step_execution WHERE id ="))
        );
        assert!(statements.iter().any(|sql| {
            sql.starts_with("UPDATE dag_step_execution SET status = 'PROCESSING'")
                && sql.contains("AND job_id IS NULL")
                && sql.contains("SELECT status FROM dag_execution")
        }));
        let replay = scheduler
            .on_job_completed(&first.id, &["wrong.mp4".to_owned()], None, None, None, None)
            .await
            .unwrap();
        assert!(replay.new_job_ids.is_empty());
        assert_eq!(
            dags.get_dag(&created.dag_id).await.unwrap().completed_steps,
            1
        );
        let downstream = jobs.get_job(&update.new_job_ids[0]).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(&downstream.state).unwrap();
        assert_eq!(state["streamer_name"], "Streamer");
        assert_eq!(state["session_title"], "Title");
        assert_eq!(state["platform"], "twitch");
        let second = downstream.dag_step_execution_id.unwrap();
        for output in ["final.mp4", "duplicate-must-not-replace.mp4"] {
            let update = scheduler
                .on_job_completed(&second, &[output.to_owned()], None, None, None, None)
                .await
                .unwrap();
            let completion = update.completion.unwrap();
            assert!(completion.succeeded);
            assert_eq!(completion.dag_id, created.dag_id);
            assert_eq!(completion.leaf_outputs, vec!["final.mp4"]);
        }
        assert_eq!(
            dags.get_dag(&created.dag_id).await.unwrap().completed_steps,
            2
        );
        pool.close().await;
    }

    #[test]
    fn shared_leaf_collection_reports_incomplete_data_without_losing_valid_outputs() {
        let definition = DagPipelineDefinition::new(
            "leaves",
            vec![
                DagStep::new(
                    "missing",
                    PipelineStep::inline("remux", serde_json::json!({})),
                ),
                DagStep::new(
                    "corrupt",
                    PipelineStep::inline("remux", serde_json::json!({})),
                ),
                DagStep::new("good", PipelineStep::inline("remux", serde_json::json!({}))),
            ],
        );
        let mut corrupt = DagStepExecutionDbModel::new("dag", "corrupt", &[]);
        corrupt.outputs = Some("not JSON".to_owned());
        let mut good = DagStepExecutionDbModel::new("dag", "good", &[]);
        good.set_outputs(&[
            "A.mp4".to_owned(),
            "a.mp4".to_owned(),
            "last.mp4".to_owned(),
        ]);
        let collected =
            DagScheduler::collect_leaf_outputs_from_step_executions(&definition, &[good, corrupt]);
        assert!(!collected.complete);
        // Windows and the default macOS file systems fold case, so the two
        // spellings are one file there (see `utils::fs::path_dedup_key`).
        assert_eq!(
            collected.paths,
            if cfg!(any(windows, target_os = "macos")) {
                vec!["A.mp4", "last.mp4"]
            } else {
                vec!["A.mp4", "a.mp4", "last.mp4"]
            }
        );
    }
    use crate::database::models::dag::DagStepExecutionDbModel;
    use crate::database::models::{DagPipelineDefinition, DagStep, JobStatus, PipelineStep};
    use crate::database::repositories::dag::DagRepository;
    use crate::database::repositories::job::JobRepository;
    use crate::pipeline::JobQueue;
    use std::sync::Arc;
    use tempfile::TempDir;

    async fn setup_test_pool() -> sqlx::SqlitePool {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("dag_scheduler_test.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.to_string_lossy());
        let pool = crate::database::init_pool(&db_url).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    struct NoopJobRepository;

    #[async_trait::async_trait]
    impl JobRepository for NoopJobRepository {
        async fn create_job(&self, _job: &crate::database::models::JobDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job(&self, _id: &str) -> Result<crate::database::models::JobDbModel> {
            unimplemented!("not needed for these tests")
        }

        async fn list_pending_jobs(
            &self,
            _job_type: &str,
        ) -> Result<Vec<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_by_status(
            &self,
            _status: JobStatus,
        ) -> Result<Vec<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_recent_jobs(
            &self,
            _limit: i32,
        ) -> Result<Vec<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_status(&self, _id: &str, _status: JobStatus) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn mark_job_failed(&self, _id: &str, _error: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn mark_job_cancelled(&self, _id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_job_for_retry(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn count_pending_jobs(&self, _job_types: Option<&[String]>) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn upsert_job_execution_progress(
            &self,
            _progress: &crate::database::models::JobExecutionProgressDbModel,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_execution_progress(
            &self,
            _job_id: &str,
        ) -> Result<Option<crate::database::models::JobExecutionProgressDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn claim_next_pending_job(
            &self,
            _job_types: Option<&[String]>,
        ) -> Result<Option<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_execution_info(&self, _id: &str) -> Result<Option<String>> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_execution_info(&self, _id: &str, _execution_info: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_state(&self, _id: &str, _state: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job(&self, _job: &crate::database::models::JobDbModel) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn update_job_if_status(
            &self,
            _job: &crate::database::models::JobDbModel,
            _expected_status: JobStatus,
        ) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn reset_processing_jobs(&self) -> Result<i32> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_job(&self, _id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn add_execution_log(
            &self,
            _log: &crate::database::models::JobExecutionLogDbModel,
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn add_execution_logs(
            &self,
            _logs: &[crate::database::models::JobExecutionLogDbModel],
        ) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn get_execution_logs(
            &self,
            _job_id: &str,
        ) -> Result<Vec<crate::database::models::JobExecutionLogDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn list_execution_logs(
            &self,
            _job_id: &str,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<(Vec<crate::database::models::JobExecutionLogDbModel>, u64)> {
            Ok((Vec::new(), 0))
        }

        async fn delete_execution_logs_for_job(&self, _job_id: &str) -> Result<()> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_filtered(
            &self,
            _filters: &crate::database::models::JobFilters,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<(Vec<crate::database::models::JobDbModel>, u64)> {
            unimplemented!("not needed for these tests")
        }

        async fn list_jobs_page_filtered(
            &self,
            _filters: &crate::database::models::JobFilters,
            _pagination: &crate::database::models::Pagination,
        ) -> Result<Vec<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn count_jobs(&self, _filters: &crate::database::models::JobFilters) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn get_job_counts_by_status(&self) -> Result<crate::database::models::JobCounts> {
            unimplemented!("not needed for these tests")
        }

        async fn get_avg_processing_time(&self) -> Result<Option<f64>> {
            unimplemented!("not needed for these tests")
        }

        async fn cancel_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }

        async fn get_jobs_by_pipeline(
            &self,
            _pipeline_id: &str,
        ) -> Result<Vec<crate::database::models::JobDbModel>> {
            unimplemented!("not needed for these tests")
        }

        async fn delete_jobs_by_pipeline(&self, _pipeline_id: &str) -> Result<u64> {
            unimplemented!("not needed for these tests")
        }
    }

    #[test]
    fn test_collect_leaf_outputs_is_deterministic_and_deduped() {
        // Graph: A -> B, A -> C (leaves are B, C)
        // Definition order is A, B, C so outputs should be B then C, regardless of DB row order.
        let def = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep {
                    id: "A".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec![],
                    retry: None,
                    timeout_secs: None,
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                    retry: None,
                    timeout_secs: None,
                },
                DagStep {
                    id: "C".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                    retry: None,
                    timeout_secs: None,
                },
            ],
        );

        let mut exec_b = DagStepExecutionDbModel::new("dag1", "B", &["A".to_string()]);
        exec_b.set_outputs(&["x".to_string(), "y".to_string()]);
        let mut exec_c = DagStepExecutionDbModel::new("dag1", "C", &["A".to_string()]);
        exec_c.set_outputs(&["y".to_string(), "z".to_string()]);

        // Simulate non-deterministic row order from DB (C then B).
        let step_execs = vec![exec_c, exec_b];

        let collected = DagScheduler::collect_leaf_outputs_from_step_executions(&def, &step_execs);
        assert!(collected.complete);
        let out = collected.paths;
        assert_eq!(out, vec!["x".to_string(), "y".to_string(), "z".to_string()]);
    }

    #[tokio::test]
    async fn test_dag_jobs_persist_session_start() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo, job_repo.clone());
        let session_start = chrono::DateTime::parse_from_rfc3339("2024-01-01T23:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let dag_def = DagPipelineDefinition::new(
            "session start propagation",
            vec![
                DagStep {
                    id: "A".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec![],
                    retry: None,
                    timeout_secs: None,
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                    retry: None,
                    timeout_secs: None,
                },
            ],
        );

        let created = scheduler
            .create_dag_pipeline(
                dag_def,
                &["/input.flv".to_string()],
                DagRunContext {
                    streamer_id: Some("streamer-1".to_string()),
                    session_id: Some("session-1".to_string()),
                    streamer_name: Some("Streamer".to_string()),
                    session_title: Some("Title".to_string()),
                    platform: Some("Platform".to_string()),
                    session_start: Some(session_start),
                    manifest: None,
                },
            )
            .await
            .unwrap();

        let root_job = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();
        let root_state: serde_json::Value = serde_json::from_str(&root_job.state).unwrap();
        assert_eq!(
            root_state
                .get("session_start_ms")
                .and_then(|value| value.as_i64()),
            Some(session_start.timestamp_millis())
        );

        let update = scheduler
            .on_job_completed(
                root_job.dag_step_execution_id.as_deref().unwrap(),
                &["/tmp/a.mp4".to_string()],
                Some("Streamer"),
                Some("Title"),
                Some("Platform"),
                Some(session_start),
            )
            .await
            .unwrap();
        assert_eq!(update.new_job_ids.len(), 1);

        let downstream_job = job_repo.get_job(&update.new_job_ids[0]).await.unwrap();
        let downstream_state: serde_json::Value =
            serde_json::from_str(&downstream_job.state).unwrap();
        assert_eq!(
            downstream_state
                .get("session_start_ms")
                .and_then(|value| value.as_i64()),
            Some(session_start.timestamp_millis())
        );
    }

    /// delete_dag removes job rows via JobRepository::delete_jobs_by_pipeline,
    /// which matches on the job.pipeline_id column — so create_step_job must
    /// persist pipeline_id on the inserted row, not just on the in-memory Job.
    #[tokio::test]
    async fn test_delete_dag_deletes_job_rows() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo, job_repo.clone());
        let dag_def = DagPipelineDefinition::new(
            "delete removes jobs",
            vec![DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            }],
        );

        let created = scheduler
            .create_dag_pipeline(
                dag_def,
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();

        let root_job = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();
        assert_eq!(
            root_job.pipeline_id.as_deref(),
            Some(created.dag_id.as_str())
        );

        job_repo
            .mark_job_failed(&root_job.id, "boom")
            .await
            .unwrap();

        scheduler.delete_dag(&created.dag_id).await.unwrap();

        assert!(job_repo.get_job(&root_job.id).await.is_err());
        let counts = job_repo.get_job_counts_by_status().await.unwrap();
        assert_eq!(counts.failed, 0);
    }

    /// Deleting a DAG removes job rows outright, so a job a worker is still running has to be
    /// told to stop and everything the queue holds for it has to go with the rows.
    #[tokio::test]
    async fn test_delete_dag_stands_down_active_jobs() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_queue = Arc::new(JobQueue::new());
        let scheduler = DagScheduler::new(job_queue.clone(), dag_repo, job_repo.clone());
        let dag_def = DagPipelineDefinition::new(
            "delete stands down jobs",
            vec![DagStep {
                id: "A".to_string(),
                step: PipelineStep::inline("noop", serde_json::json!({})),
                depends_on: vec![],
                retry: None,
                timeout_secs: None,
            }],
        );

        let created = scheduler
            .create_dag_pipeline(
                dag_def,
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();

        // Stands in for a worker claiming the job: dequeue is what registers its token.
        let claimed = job_queue
            .dequeue(None)
            .await
            .unwrap()
            .expect("the root job is claimable");
        assert_eq!(claimed.id, created.root_job_ids[0]);
        let token = job_queue
            .get_cancellation_token(&claimed.id)
            .await
            .expect("dequeue registers a cancellation token");
        assert!(!token.is_cancelled());
        assert_eq!(job_queue.depth(), 1);

        scheduler.delete_dag(&created.dag_id).await.unwrap();

        assert!(
            token.is_cancelled(),
            "a worker inside Processor::process must be told the job is gone"
        );
        assert_eq!(
            job_queue.depth(),
            0,
            "the deleted job still counted as queued"
        );
        assert!(job_queue.get_job(&claimed.id).await.unwrap().is_none());
        assert!(
            job_queue
                .get_cancellation_token(&claimed.id)
                .await
                .is_none()
        );
        assert!(job_repo.get_job(&claimed.id).await.is_err());
    }

    /// The retry fan-out path (reset_dag_for_retry -> enqueue_now_ready_steps)
    /// has no session context of its own; jobs it creates must recover the
    /// placeholder metadata from a sibling step's persisted job state.
    #[tokio::test]
    async fn test_retry_fanout_recovers_metadata_from_sibling_job() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo, job_repo.clone());
        let session_start = chrono::DateTime::parse_from_rfc3339("2024-01-01T23:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let dag_def = DagPipelineDefinition::new(
            "retry fanout metadata recovery",
            vec![
                DagStep {
                    id: "A".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec![],
                    retry: None,
                    timeout_secs: None,
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                    retry: None,
                    timeout_secs: None,
                },
            ],
        );

        let created = scheduler
            .create_dag_pipeline(
                dag_def,
                &["/input.flv".to_string()],
                DagRunContext {
                    streamer_id: Some("streamer-1".to_string()),
                    session_id: Some("session-1".to_string()),
                    streamer_name: Some("Streamer".to_string()),
                    session_title: Some("Title".to_string()),
                    platform: Some("Platform".to_string()),
                    session_start: Some(session_start),
                    manifest: None,
                },
            )
            .await
            .unwrap();

        // Put the DAG in the shape reset_dag_for_retry produces for a
        // fail-fast cancelled parallel branch: dependency completed with
        // outputs, dependent step BLOCKED with no job row attached.
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|s| s.step_id == "A").unwrap();
        sqlx::query("UPDATE dag_step_execution SET status = ?, outputs = ? WHERE id = ?")
            .bind(DagStepStatus::Completed.as_str())
            .bind(r#"["/tmp/a.mp4"]"#)
            .bind(&step_a.id)
            .execute(&pool)
            .await
            .unwrap();

        let new_job_ids = scheduler
            .enqueue_now_ready_steps(&created.dag_id)
            .await
            .unwrap()
            .new_job_ids;
        assert_eq!(new_job_ids.len(), 1);

        let recovered_job = job_repo.get_job(&new_job_ids[0]).await.unwrap();
        let state: serde_json::Value = serde_json::from_str(&recovered_job.state).unwrap();
        assert_eq!(
            state.get("session_start_ms").and_then(|v| v.as_i64()),
            Some(session_start.timestamp_millis())
        );
        assert_eq!(
            state.get("streamer_name").and_then(|v| v.as_str()),
            Some("Streamer")
        );
    }

    /// Every path that materializes a step job — publication, completion of a
    /// dependency, retry fan-out and startup recovery — must copy the DAG row's
    /// manifest into the job so the processor can pair videos with danmu.
    #[tokio::test]
    async fn manifest_reaches_jobs_created_by_every_materialization_path() {
        use crate::pipeline::manifest::{ManifestScope, ManifestSegment, PipelineInputManifest};
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let manifest = PipelineInputManifest::new(
            "session-1",
            "streamer-1",
            ManifestScope::Segment { index: 4 },
            vec![ManifestSegment {
                segment_index: 4,
                video: vec!["/rec/4.mp4".to_string()],
                danmu: vec!["/rec/4.xml".to_string()],
            }],
        );
        let manifest_value = serde_json::to_value(&manifest).unwrap();
        let state_manifest = |job: &JobDbModel| -> serde_json::Value {
            let state: serde_json::Value = serde_json::from_str(&job.state).unwrap();
            state["manifest"].clone()
        };

        let created = scheduler
            .create_dag_pipeline_with_hook(
                two_step_pipeline("manifest propagation"),
                &["/rec/4.mp4".to_string(), "/rec/4.xml".to_string()],
                DagRunContext {
                    streamer_id: Some("streamer-1".to_string()),
                    session_id: Some("session-1".to_string()),
                    streamer_name: Some("Streamer".to_string()),
                    ..DagRunContext::default()
                },
                Some(DagExecutionMetadata {
                    segment_index: Some(4),
                    segment_source: Some("paired".to_string()),
                    manifest: Some(manifest.clone()),
                }),
                None,
            )
            .await
            .unwrap();

        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(
            input_manifest(&dag).as_deref(),
            Some(&manifest),
            "publication stores the manifest on the DAG row"
        );
        let root = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();
        assert_eq!(state_manifest(&root), manifest_value, "root job");
        let parsed = parse_job_state(&root.state);
        assert_eq!(parsed.manifest.as_deref(), Some(&manifest));
        assert_eq!(parsed.streamer_name.as_deref(), Some("Streamer"));

        // Completion of A creates B from the DAG row, not from A's job state.
        let update = scheduler
            .on_job_completed(
                root.dag_step_execution_id.as_deref().unwrap(),
                &["/rec/4.mp4".to_string(), "/rec/4.ass".to_string()],
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(update.new_job_ids.len(), 1);
        let downstream = job_repo.get_job(&update.new_job_ids[0]).await.unwrap();
        assert_eq!(state_manifest(&downstream), manifest_value, "completion");

        // Retry fan-out: B is BLOCKED again without a job row.
        let step_b = dag_repo
            .get_step(downstream.dag_step_execution_id.as_deref().unwrap())
            .await
            .unwrap();
        sqlx::query("UPDATE dag_step_execution SET status = 'BLOCKED', job_id = NULL WHERE id = ?")
            .bind(&step_b.id)
            .execute(&pool)
            .await
            .unwrap();
        let retried = scheduler
            .enqueue_now_ready_steps(&created.dag_id)
            .await
            .unwrap()
            .new_job_ids;
        assert_eq!(retried.len(), 1);
        let retried_job = job_repo.get_job(&retried[0]).await.unwrap();
        assert_eq!(
            state_manifest(&retried_job),
            manifest_value,
            "retry fan-out"
        );

        // Startup recovery materializes a PENDING step through the same row once
        // no live job references the step any more.
        sqlx::query("UPDATE job SET status = 'CANCELLED' WHERE dag_step_execution_id = ?")
            .bind(&step_b.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE dag_step_execution SET status = 'PENDING', job_id = NULL WHERE id = ?")
            .bind(&step_b.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
        let recovered_step = dag_repo.get_step(&step_b.id).await.unwrap();
        let recovered_job = job_repo
            .get_job(recovered_step.job_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(state_manifest(&recovered_job), manifest_value, "recovery");

        // A DAG without a manifest yields jobs without one.
        let plain = scheduler
            .create_dag_pipeline(
                two_step_pipeline("no manifest"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let plain_root = job_repo.get_job(&plain.root_job_ids[0]).await.unwrap();
        assert!(parse_job_state(&plain_root.state).manifest.is_none());
        assert!(input_manifest(&dag_repo.get_dag(&plain.dag_id).await.unwrap()).is_none());
    }

    /// A step whose dependencies produced nothing runs no job: `delete` and
    /// `rclone move` produce no outputs and `delete` rejects an empty input list,
    /// so the documented `move -> delete` pattern must settle the DAG instead of
    /// failing it. Applies on live completion and on retry fan-out alike.
    #[tokio::test]
    async fn zero_input_steps_complete_as_no_ops_on_completion_and_retry() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let chain = DagPipelineDefinition::new(
            "move then delete",
            vec![
                DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({}))),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::inline("noop", serde_json::json!({})),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::inline("noop", serde_json::json!({})),
                    vec!["B".to_string()],
                ),
            ],
        );
        let created = scheduler
            .create_dag_pipeline(
                chain.clone(),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let root = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();

        let update = scheduler
            .on_job_completed(
                root.dag_step_execution_id.as_deref().unwrap(),
                &[],
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(update.new_job_ids.is_empty());
        let completion = update.completion.expect("no-op steps settle the DAG");
        assert!(completion.succeeded);
        assert!(completion.leaf_outputs.is_empty());
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Completed));
        assert_eq!(dag.completed_steps, 3);
        for step in dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap() {
            assert_eq!(
                step.get_status(),
                Some(DagStepStatus::Completed),
                "{}",
                step.step_id
            );
            assert!(step.get_outputs().is_empty());
        }
        assert_eq!(
            job_repo
                .get_jobs_by_pipeline(&created.dag_id)
                .await
                .unwrap()
                .len(),
            1,
            "only the root step ran a job"
        );

        // Retry fan-out: A completed with no outputs earlier, B was cancelled
        // before its job existed. The reset must settle B and the DAG.
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("retry no-op"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
        dag_repo
            .complete_step_and_check_dependents(&step_a.id, &[])
            .await
            .unwrap();
        sqlx::query(
            "UPDATE dag_step_execution SET status = 'CANCELLED', job_id = NULL WHERE id = ?",
        )
        .bind(&step_b.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE dag_execution SET status = 'FAILED', error = 'boom' WHERE id = ?")
            .bind(&created.dag_id)
            .execute(&pool)
            .await
            .unwrap();

        let update = scheduler.retry_dag(&created.dag_id, || {}).await.unwrap();
        assert!(update.job_ids.is_empty());
        assert_eq!(update.retried_steps, 1);
        assert!(
            update
                .completions
                .iter()
                .all(|completion| completion.succeeded)
        );
        assert_eq!(update.completions.len(), 1);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Completed));
        assert_eq!(
            dag_repo.get_step(&step_b.id).await.unwrap().get_status(),
            Some(DagStepStatus::Completed)
        );
    }

    /// An `execute` step runs its command even when the step before it produced
    /// nothing, so a script after an upload or delete is not silently skipped.
    #[tokio::test]
    async fn execute_steps_still_run_after_a_step_without_outputs() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "notify after move",
                    vec![
                        DagStep::new(
                            "move",
                            PipelineStep::inline("rclone", serde_json::json!({})),
                        ),
                        DagStep::with_dependencies(
                            "notify",
                            PipelineStep::inline(
                                "execute",
                                serde_json::json!({ "command": "notify-send done" }),
                            ),
                            vec!["move".to_string()],
                        ),
                    ],
                ),
                &["/rec/a.mp4".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let root = job_repo.get_job(&created.root_job_ids[0]).await.unwrap();

        let update = scheduler
            .on_job_completed(
                root.dag_step_execution_id.as_deref().unwrap(),
                &[],
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(update.new_job_ids.len(), 1, "the execute step got a job");
        assert!(update.completion.is_none());
        let notify = job_repo.get_job(&update.new_job_ids[0]).await.unwrap();
        assert_eq!(notify.job_type, "execute");
        assert_eq!(notify.input.as_deref(), Some("[]"));
    }

    /// Failing a DAG that is already terminal changes nothing and yields no
    /// completion, so a retry that could not even reset the rows does not replay
    /// the earlier outcome to the coordinator.
    #[tokio::test]
    async fn fail_dag_leaves_a_terminal_dag_alone() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo.clone(), job_repo);
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("terminal fail"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let first = scheduler
            .fail_dag(&created.dag_id, "first failure")
            .await
            .unwrap();
        assert!(first.is_some_and(|completion| !completion.succeeded));

        let second = scheduler
            .fail_dag(&created.dag_id, "retry failed")
            .await
            .unwrap();
        assert!(second.is_none());
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert_eq!(dag.error.as_deref(), Some("first failure"));
    }

    /// A PROCESSING step whose job row is gone can never be reported on; startup
    /// reconciliation fails it like a step whose job failed.
    #[tokio::test]
    async fn startup_reconciliation_fails_a_processing_step_without_a_job_row() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("jobless processing step"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        // Deleting the job row nulls the step's job_id through the foreign key.
        job_repo.delete_job(&created.root_job_ids[0]).await.unwrap();
        let steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        assert_eq!(step_a.get_status(), Some(DagStepStatus::Processing));
        assert!(step_a.job_id.is_none());

        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(
            dag.error
                .as_deref()
                .unwrap()
                .contains("job row no longer exists"),
            "{:?}",
            dag.error
        );
    }

    /// Retry fan-out and startup recovery decide readiness from rows a live
    /// completion may have advanced in between; a step that already has a job
    /// is skipped instead of failing the whole DAG.
    #[tokio::test]
    async fn already_materialized_step_does_not_fail_the_dag() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("materialization race"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
        // Another materialization attached a job to B before this completion
        // got to it.
        let mut elsewhere = JobDbModel::new_pipeline_step("noop", "[]", "[]", 0, None, None);
        elsewhere.pipeline_id = Some(created.dag_id.clone());
        elsewhere.dag_step_execution_id = Some(step_b.id.clone());
        job_repo.create_job(&elsewhere).await.unwrap();
        sqlx::query("UPDATE dag_step_execution SET job_id = ? WHERE id = ?")
            .bind(&elsewhere.id)
            .bind(&step_b.id)
            .execute(&pool)
            .await
            .unwrap();

        let update = scheduler
            .on_job_completed(&step_a.id, &["/a.mp4".to_string()], None, None, None, None)
            .await
            .unwrap();
        assert!(update.new_job_ids.is_empty());
        assert!(update.completion.is_none());
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
        assert_eq!(dag.error, None);
        assert_eq!(
            dag_repo.get_step(&step_b.id).await.unwrap().job_id,
            Some(elsewhere.id)
        );
    }

    /// A worker that persisted its job's failure but never reported it leaves the
    /// step PROCESSING; startup reconciliation fails the DAG so it can be retried.
    #[tokio::test]
    async fn startup_reconciliation_fails_a_step_whose_job_already_failed() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("stranded failure"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        job_repo
            .mark_job_failed(&created.root_job_ids[0], "disk full")
            .await
            .unwrap();

        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(
            dag.error.as_deref().unwrap().contains("disk full"),
            "{:?}",
            dag.error
        );
        let mut steps = dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap();
        steps.sort_by(|left, right| left.step_id.cmp(&right.step_id));
        assert_eq!(
            steps
                .iter()
                .map(|step| (step.step_id.as_str(), step.status.as_str()))
                .collect::<Vec<_>>(),
            vec![("A", "FAILED"), ("B", "CANCELLED")]
        );
        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
        assert!(
            dag_repo
                .list_processing_steps_with_failed_jobs()
                .await
                .unwrap()
                .is_empty()
        );
    }

    fn two_step_pipeline(name: &str) -> DagPipelineDefinition {
        DagPipelineDefinition::new(
            name,
            vec![
                DagStep {
                    id: "A".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec![],
                    retry: None,
                    timeout_secs: None,
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                    retry: None,
                    timeout_secs: None,
                },
            ],
        )
    }

    /// `noop` steps wired as given: `(id, dependencies)`.
    fn noop_pipeline(name: &str, steps: &[(&str, &[&str])]) -> DagPipelineDefinition {
        DagPipelineDefinition::new(
            name,
            steps
                .iter()
                .map(|(id, depends_on)| {
                    DagStep::with_dependencies(
                        *id,
                        PipelineStep::inline("noop", serde_json::json!({})),
                        depends_on.iter().map(|dep| dep.to_string()).collect(),
                    )
                })
                .collect(),
        )
    }

    /// `"<step>=<status>"` for every step of a DAG, in step-id order.
    async fn step_statuses(
        dag_repo: &crate::database::repositories::dag::SqlxDagRepository,
        dag_id: &str,
    ) -> Vec<String> {
        let mut statuses: Vec<String> = dag_repo
            .get_steps_by_dag(dag_id)
            .await
            .unwrap()
            .into_iter()
            .map(|step| format!("{}={}", step.step_id, step.status))
            .collect();
        statuses.sort();
        statuses
    }

    async fn complete(
        scheduler: &DagScheduler,
        step_execution_id: &str,
        outputs: &[&str],
    ) -> DagJobCompletedUpdate {
        let outputs: Vec<String> = outputs.iter().map(|output| output.to_string()).collect();
        scheduler
            .on_job_completed(step_execution_id, &outputs, None, None, None, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn startup_recovery_materializes_pending_jobless_step_once() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("recover pending intent"),
                &["/input.flv".to_string()],
                DagRunContext {
                    streamer_name: Some("Streamer".to_string()),
                    session_title: Some("Title".to_string()),
                    ..DagRunContext::default()
                },
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();

        let ready = dag_repo
            .complete_step_and_check_dependents(
                &step_a.id,
                &["/a.mp4".to_string(), "/shared.xml".to_string()],
            )
            .await
            .unwrap()
            .ready_steps;
        assert_eq!(ready.len(), 1);
        let pending = dag_repo.get_step(&ready[0].step.id).await.unwrap();
        assert_eq!(pending.get_status(), Some(DagStepStatus::Pending));
        assert!(pending.job_id.is_none());

        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);

        let recovered = dag_repo.get_step(&pending.id).await.unwrap();
        assert_eq!(recovered.get_status(), Some(DagStepStatus::Processing));
        let recovered_job = job_repo
            .get_job(recovered.job_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<String>>(recovered_job.input.as_deref().unwrap()).unwrap(),
            vec!["/a.mp4".to_string(), "/shared.xml".to_string()]
        );
        let state: serde_json::Value = serde_json::from_str(&recovered_job.state).unwrap();
        assert_eq!(
            state.get("streamer_name").and_then(|value| value.as_str()),
            Some("Streamer")
        );
        assert_eq!(
            job_repo
                .get_jobs_by_pipeline(&created.dag_id)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn startup_recovery_advances_reverse_linked_completed_job_without_duplicate() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let definition = two_step_pipeline("recover reverse link");
        let created = scheduler
            .create_dag_pipeline(
                definition.clone(),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
        dag_repo
            .complete_step_and_check_dependents(step_a.id.as_str(), &["/a.mp4".to_string()])
            .await
            .unwrap();

        let (mut orphan, _) = DagScheduler::build_step_job(
            &created.dag_id,
            &step_b.id,
            definition.get_step("B").unwrap(),
            vec!["/a.mp4".to_string()],
            &DagRunContext::default(),
        )
        .unwrap();
        orphan.status = JobStatus::Completed.as_str().to_string();
        orphan.set_outputs(&["/b.mp4".to_string()]);
        orphan.completed_at = Some(crate::database::time::now_ms());
        job_repo.create_job(&orphan).await.unwrap();

        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);
        assert_eq!(
            dag_repo.get_step(&step_b.id).await.unwrap().get_status(),
            Some(DagStepStatus::Completed)
        );
        assert_eq!(
            job_repo
                .get_jobs_by_pipeline(&created.dag_id)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn startup_recovery_advances_processing_step_from_completed_job_once() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("recover completed job"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let root_job_id = &created.root_job_ids[0];
        sqlx::query(
            "UPDATE job SET status = 'COMPLETED', outputs = ?, completed_at = ? WHERE id = ?",
        )
        .bind(r#"["/durable.mp4"]"#)
        .bind(crate::database::time::now_ms())
        .bind(root_job_id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 1);
        assert_eq!(scheduler.recover_dag_jobs().await.unwrap(), 0);

        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        let step_b = steps.iter().find(|step| step.step_id == "B").unwrap();
        assert_eq!(step_a.get_status(), Some(DagStepStatus::Completed));
        assert_eq!(step_a.get_outputs(), vec!["/durable.mp4".to_string()]);
        assert_eq!(step_b.get_status(), Some(DagStepStatus::Processing));
        assert!(step_b.job_id.is_some());
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.completed_steps, 1);
        assert_eq!(dag.failed_steps, 0);
        assert_eq!(
            job_repo
                .get_jobs_by_pipeline(&created.dag_id)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn late_failure_cannot_flip_completed_step_or_dag() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo.clone(), job_repo);
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "terminal failure guard",
                    vec![DagStep::new("A", PipelineStep::preset("noop"))],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let step = scheduler
            .get_dag_steps(&created.dag_id)
            .await
            .unwrap()
            .remove(0);
        scheduler
            .on_job_completed(
                &step.id,
                &["/output.mp4".to_string()],
                None,
                None,
                None,
                None,
            )
            .await
            .unwrap();

        let update = scheduler
            .on_job_failed(&step.id, "late failure")
            .await
            .unwrap();
        assert_eq!(update.cancelled_count, 0);
        assert!(update.completion.is_none());
        assert_eq!(
            dag_repo.get_step(&step.id).await.unwrap().get_status(),
            Some(DagStepStatus::Completed)
        );
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Completed));
        assert_eq!(dag.completed_steps, 1);
        assert_eq!(dag.failed_steps, 0);
    }

    /// A→{B,C}→D where B fails while C runs. Only D, which depends on B, is
    /// cancelled; C keeps its job and finishes; the DAG stays PROCESSING with
    /// the error recorded until C settles, and then fails exactly once.
    #[tokio::test]
    async fn failed_step_cancels_only_its_dependents_and_the_dag_settles_after_siblings() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                noop_pipeline(
                    "diamond",
                    &[("A", &[]), ("B", &["A"]), ("C", &["A"]), ("D", &["B", "C"])],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_id = |id: &str| {
            steps
                .iter()
                .find(|step| step.step_id == id)
                .unwrap()
                .id
                .clone()
        };

        let update = complete(&scheduler, &step_id("A"), &["/a.mp4"]).await;
        assert_eq!(update.new_job_ids.len(), 2, "B and C fan out");
        assert!(update.completion.is_none());
        assert_eq!(
            scheduler.recover_dag_jobs().await.unwrap(),
            0,
            "D waits on running steps and is not a recovery candidate"
        );

        let update = scheduler
            .on_job_failed(&step_id("B"), "boom")
            .await
            .unwrap();
        assert_eq!(update.cancelled_count, 0, "D had no job to cancel");
        assert!(update.completion.is_none(), "C is still running");
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
        assert_eq!(dag.error.as_deref(), Some("Step 'B' failed: boom"));
        assert_eq!(dag.failed_steps, 1);
        assert!(dag.completed_at.is_none());
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=COMPLETED", "B=FAILED", "C=PROCESSING", "D=CANCELLED"]
        );
        let step_c = dag_repo.get_step(&step_id("C")).await.unwrap();
        let job_c = job_repo
            .get_job(step_c.job_id.as_deref().unwrap())
            .await
            .unwrap();
        assert_eq!(
            job_c.status, "PENDING",
            "the independent branch keeps its job"
        );

        // A late completion for the cancelled dependent changes nothing.
        let update = complete(&scheduler, &step_id("D"), &["/late.mp4"]).await;
        assert!(update.new_job_ids.is_empty());
        assert!(update.completion.is_none());
        let step_d = dag_repo.get_step(&step_id("D")).await.unwrap();
        assert_eq!(step_d.get_status(), Some(DagStepStatus::Cancelled));
        assert!(step_d.get_outputs().is_empty());
        assert_eq!(
            dag_repo
                .get_dag(&created.dag_id)
                .await
                .unwrap()
                .completed_steps,
            1
        );

        // The last active step finalizes the DAG; the cancelled dependent is
        // not materialized.
        let update = complete(&scheduler, &step_id("C"), &["/c.mp4"]).await;
        assert!(update.new_job_ids.is_empty());
        let completion = update.completion.expect("C settles the DAG");
        assert!(!completion.succeeded);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(dag.completed_at.is_some());
        assert_eq!(dag.completed_steps, 2);
        assert_eq!(dag.failed_steps, 1);
        assert_eq!(dag.error.as_deref(), Some("Step 'B' failed: boom"));
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=COMPLETED", "B=FAILED", "C=COMPLETED", "D=CANCELLED"]
        );
        assert_eq!(
            job_repo
                .get_jobs_by_pipeline(&created.dag_id)
                .await
                .unwrap()
                .len(),
            3,
            "D never got a job"
        );
    }

    /// A→B→C where the root fails: nothing else is active, so B and C are
    /// cancelled and the DAG fails in the same call. A repeated failure report
    /// for the settled step is ignored.
    #[tokio::test]
    async fn failed_root_cancels_its_chain_and_fails_the_dag_at_once() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                noop_pipeline("chain", &[("A", &[]), ("B", &["A"]), ("C", &["B"])]),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let root = scheduler
            .get_dag_steps(&created.dag_id)
            .await
            .unwrap()
            .into_iter()
            .find(|step| step.step_id == "A")
            .unwrap();

        let update = scheduler.on_job_failed(&root.id, "boom").await.unwrap();
        assert_eq!(update.cancelled_count, 0, "the chain had no jobs yet");
        let completion = update.completion.expect("no other step is active");
        assert!(!completion.succeeded);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(dag.completed_at.is_some());
        assert_eq!(dag.completed_steps, 0);
        assert_eq!(dag.failed_steps, 1);
        assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=FAILED", "B=CANCELLED", "C=CANCELLED"]
        );
        for step in dag_repo.get_steps_by_dag(&created.dag_id).await.unwrap() {
            if step.step_id != "A" {
                assert!(step.job_id.is_none(), "{} never got a job", step.step_id);
            }
        }

        let update = scheduler.on_job_failed(&root.id, "again").await.unwrap();
        assert_eq!(update.cancelled_count, 0);
        assert!(update.completion.is_none());
        assert_eq!(
            dag_repo
                .get_dag(&created.dag_id)
                .await
                .unwrap()
                .failed_steps,
            1
        );
    }

    /// The plan follows the step's policy, doubling the wait per retry and
    /// stopping when the budget is spent; the step's timeout reaches its job.
    #[tokio::test]
    async fn plan_step_retry_follows_the_step_policy_and_stops_when_spent() {
        use crate::database::models::StepRetryPolicy;

        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "retry policy",
                    vec![
                        DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({})))
                            .with_retry(StepRetryPolicy {
                                max_attempts: 3,
                                backoff_secs: 10,
                            })
                            .with_timeout_secs(600),
                        DagStep::new("B", PipelineStep::inline("noop", serde_json::json!({}))),
                    ],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_id = |id: &str| {
            steps
                .iter()
                .find(|step| step.step_id == id)
                .unwrap()
                .id
                .clone()
        };
        let job_of = |id: &str| {
            steps
                .iter()
                .find(|step| step.step_id == id)
                .unwrap()
                .job_id
                .clone()
                .unwrap()
        };

        let before = Utc::now();
        let first = scheduler
            .plan_step_retry(&step_id("A"), 0)
            .await
            .unwrap()
            .expect("first retry");
        assert_eq!((first.next_attempt, first.max_attempts), (2, 3));
        let wait = first.retry_after - before;
        assert!(
            wait >= chrono::Duration::seconds(10) && wait < chrono::Duration::seconds(12),
            "{wait}"
        );
        let second = scheduler
            .plan_step_retry(&step_id("A"), 1)
            .await
            .unwrap()
            .expect("second retry");
        assert_eq!(second.next_attempt, 3);
        assert!(second.retry_after - before >= chrono::Duration::seconds(20));
        assert!(
            scheduler
                .plan_step_retry(&step_id("A"), 2)
                .await
                .unwrap()
                .is_none(),
            "budget spent"
        );
        assert!(
            scheduler
                .plan_step_retry(&step_id("B"), 0)
                .await
                .unwrap()
                .is_none(),
            "no policy"
        );

        let job_a = job_repo.get_job(&job_of("A")).await.unwrap();
        assert_eq!(
            crate::pipeline::job_queue::parse_job_state(&job_a.state).timeout_secs,
            Some(600)
        );
        let job_b = job_repo.get_job(&job_of("B")).await.unwrap();
        assert_eq!(
            crate::pipeline::job_queue::parse_job_state(&job_b.state).timeout_secs,
            None
        );

        assert!(
            scheduler
                .step_awaits_job(&step_id("A"), &job_of("A"))
                .await
                .unwrap()
        );
        assert!(
            !scheduler
                .step_awaits_job(&step_id("A"), "another-job")
                .await
                .unwrap()
        );
        assert!(
            !scheduler
                .step_awaits_job("missing-step", &job_of("A"))
                .await
                .unwrap()
        );

        // A step that failed for good plans nothing and awaits nothing.
        scheduler
            .on_job_failed(&step_id("A"), "boom")
            .await
            .unwrap();
        assert!(
            scheduler
                .plan_step_retry(&step_id("A"), 0)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !scheduler
                .step_awaits_job(&step_id("A"), &job_of("A"))
                .await
                .unwrap()
        );
    }

    /// The worker's failure entry point. A retryable failure with budget left
    /// is recorded on the job row with the retry time, scheduled, and leaves
    /// the step PROCESSING; once the budget is spent it reaches the workflow.
    /// A kind that cannot succeed again, a job row that could not be written,
    /// and a retry that could not be recorded all fail the step instead; a
    /// report for a settled step is ignored.
    #[tokio::test]
    async fn job_attempt_failure_is_retried_or_applied_by_kind_budget_and_persistence() {
        use crate::database::models::StepRetryPolicy;
        use crate::database::repositories::JobRepository as _;

        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        ));
        let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
        let retrying = |id: &str| {
            DagStep::new(id, PipelineStep::inline("noop", serde_json::json!({}))).with_retry(
                StepRetryPolicy {
                    max_attempts: 2,
                    backoff_secs: 30,
                },
            )
        };
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "attempt failures",
                    vec![
                        retrying("A"),
                        retrying("B"),
                        retrying("C"),
                        retrying("E"),
                        retrying("F"),
                        DagStep::with_dependencies(
                            "D",
                            PipelineStep::inline("noop", serde_json::json!({})),
                            vec!["A".to_string()],
                        ),
                    ],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_id = |id: &str| {
            steps
                .iter()
                .find(|step| step.step_id == id)
                .unwrap()
                .id
                .clone()
        };
        let job_of = |id: &str| {
            steps
                .iter()
                .find(|step| step.step_id == id)
                .unwrap()
                .job_id
                .clone()
                .unwrap()
        };
        let record = |job_id: String| {
            let queue = queue.clone();
            move |message: String| async move {
                queue
                    .fail_with_step_info(&job_id, &message, Some("noop"), None, None, &[])
                    .await
            }
        };
        fn attempt(job_id: &str, retry_count: i32, kind: StepFailureKind) -> FailedStepAttempt<'_> {
            FailedStepAttempt {
                job_id,
                retry_count,
                error: "boom",
                kind,
            }
        }

        // A: budget left, so the attempt is recorded and a retry scheduled.
        let job_a = job_of("A");
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("A"),
                attempt(&job_a, 0, StepFailureKind::ProcessorError),
                record(job_a.clone()),
            )
            .await
            .unwrap();
        let StepFailureOutcome::Retried(planned) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!((planned.next_attempt, planned.max_attempts), (2, 2));
        let row = job_repo.get_job(&job_a).await.unwrap();
        assert_eq!(row.status, JobStatus::Failed.as_str());
        assert_eq!(
            row.error.as_deref(),
            Some(planned.describe("boom").as_str()),
            "the job row carries the retry"
        );
        assert_eq!(
            row.retry_after,
            Some(planned.retry_after.timestamp_millis())
        );
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            [
                "A=PROCESSING",
                "B=PROCESSING",
                "C=PROCESSING",
                "D=BLOCKED",
                "E=PROCESSING",
                "F=PROCESSING"
            ]
        );
        assert!(
            dag_repo
                .get_dag(&created.dag_id)
                .await
                .unwrap()
                .error
                .is_none()
        );

        // The sweeper re-queues A; its next failure spends the budget and
        // reaches the workflow: D is cancelled while the siblings keep running.
        queue.retry_job(&job_a).await.unwrap();
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("A"),
                attempt(&job_a, 1, StepFailureKind::ProcessorError),
                record(job_a.clone()),
            )
            .await
            .unwrap();
        let StepFailureOutcome::Failed(update) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(update.cancelled_count, 0, "D had no job");
        assert!(update.completion.is_none(), "siblings still run");
        let row = job_repo.get_job(&job_a).await.unwrap();
        assert_eq!(row.error.as_deref(), Some("boom"));
        assert!(row.retry_after.is_none());
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Processing));
        assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));

        // B: the kind rules out another attempt, budget or not.
        let job_b = job_of("B");
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("B"),
                attempt(&job_b, 0, StepFailureKind::NonBatchInput),
                record(job_b.clone()),
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, StepFailureOutcome::Failed(_)),
            "{outcome:?}"
        );
        let row = job_repo.get_job(&job_b).await.unwrap();
        assert_eq!(row.error.as_deref(), Some("boom"));
        assert!(row.retry_after.is_none());
        // A repeated report for the settled step changes nothing.
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("B"),
                attempt(&job_b, 0, StepFailureKind::ProcessorError),
                record(job_b.clone()),
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, StepFailureOutcome::Ignored),
            "{outcome:?}"
        );

        // C: the job row was not written as FAILED, so the retry cannot be
        // recorded and the failure is applied instead.
        let job_c = job_of("C");
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("C"),
                attempt(&job_c, 0, StepFailureKind::Timeout),
                |_message| async { Ok(JobFailureOutcome::Transitioned) },
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, StepFailureOutcome::Failed(_)),
            "{outcome:?}"
        );
        let row = job_repo.get_job(&job_c).await.unwrap();
        assert_eq!(row.status, JobStatus::Pending.as_str());
        assert!(row.retry_after.is_none());

        // E: the job row could not be written at all.
        let job_e = job_of("E");
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("E"),
                attempt(&job_e, 0, StepFailureKind::ProcessorError),
                |_message| async { Err(Error::Validation("disk full".to_string())) },
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, StepFailureOutcome::Failed(_)),
            "{outcome:?}"
        );
        assert!(
            job_repo
                .get_job(&job_e)
                .await
                .unwrap()
                .retry_after
                .is_none()
        );

        // F: the processor never ran; the last active step settles the DAG.
        let job_f = job_of("F");
        let outcome = scheduler
            .on_job_attempt_failed(
                &step_id("F"),
                attempt(&job_f, 0, StepFailureKind::ExecutionStart),
                record(job_f.clone()),
            )
            .await
            .unwrap();
        let StepFailureOutcome::Failed(update) = outcome else {
            panic!("{outcome:?}");
        };
        let completion = update.completion.expect("no step is left active");
        assert!(!completion.succeeded);
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            [
                "A=FAILED",
                "B=FAILED",
                "C=FAILED",
                "D=CANCELLED",
                "E=FAILED",
                "F=FAILED"
            ]
        );
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert_eq!(dag.error.as_deref(), Some("Step 'A' failed: boom"));
        assert_eq!(dag.failed_steps, 5);
    }

    #[tokio::test]
    async fn cancelled_attempt_does_not_schedule_retry_or_apply_step_failure() {
        use crate::database::models::StepRetryPolicy;
        use crate::database::repositories::{SqlxDagRepository, SqlxJobRepository};
        use crate::database::test_support::SqlTrace;

        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let dags = Arc::new(SqlxDagRepository::new(pool.clone(), pool.clone()));
        let jobs = Arc::new(SqlxJobRepository::new(pool.clone(), pool.clone()));
        let queue = Arc::new(JobQueue::with_repository(Default::default(), jobs.clone()));
        let scheduler = DagScheduler::new(queue.clone(), dags.clone(), jobs.clone());
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "cancel race",
                    vec![
                        DagStep::new("A", PipelineStep::inline("noop", serde_json::json!({})))
                            .with_retry(StepRetryPolicy {
                                max_attempts: 2,
                                backoff_secs: 1,
                            }),
                    ],
                ),
                &["input.flv".into()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let job = queue.dequeue(None).await.unwrap().unwrap();
        let trace = std::sync::Mutex::new(None);
        let outcome = scheduler
            .on_job_attempt_failed(
                job.dag_step_execution_id.as_deref().unwrap(),
                FailedStepAttempt {
                    job_id: &job.id,
                    retry_count: 0,
                    error: "late failure",
                    kind: StepFailureKind::ProcessorError,
                },
                |message| {
                    let scheduler = &scheduler;
                    let queue = &queue;
                    let pool = &pool;
                    let trace = &trace;
                    let job_id = &job.id;
                    let dag_id = &created.dag_id;
                    async move {
                        // Cancellation wins after retry planning but before the conditional job update.
                        scheduler.cancel_dag(dag_id).await.unwrap();
                        let observer = SqlTrace::install(pool).await;
                        *trace.lock().unwrap() = Some(observer);
                        queue
                            .fail_with_step_info(job_id, &message, Some("noop"), None, None, &[])
                            .await
                    }
                },
            )
            .await
            .unwrap();
        assert!(matches!(outcome, StepFailureOutcome::Ignored));
        let statements = trace.lock().unwrap().take().unwrap().statements();
        assert!(
            !statements
                .iter()
                .any(|sql| sql.starts_with("UPDATE job SET retry_after")),
            "{statements:?}"
        );
        assert!(
            !statements
                .iter()
                .any(|sql| sql.contains("UPDATE dag_step_execution")),
            "{statements:?}"
        );
        assert_eq!(
            jobs.get_job(&job.id).await.unwrap().get_status(),
            Some(JobStatus::Cancelled)
        );
        assert_eq!(
            dags.get_dag(&created.dag_id).await.unwrap().get_status(),
            Some(DagExecutionStatus::Cancelled)
        );
        assert_eq!(queue.depth(), 0);
        assert!(queue.get_cancellation_token(&job.id).await.is_none());
        queue.stop_progress_aggregator().await;
    }

    #[tokio::test]
    async fn failed_attempt_observability_errors_preserve_retry_and_cleanup() {
        use crate::database::models::StepRetryPolicy;
        use crate::database::repositories::JobRepository as _;

        for fault in [
            "CREATE TRIGGER fail_observability BEFORE INSERT ON job_execution_logs
             BEGIN SELECT RAISE(ABORT, 'log write failed'); END",
            "CREATE TRIGGER fail_observability BEFORE UPDATE OF execution_info ON job
             BEGIN SELECT RAISE(ABORT, 'summary write failed'); END",
        ] {
            let pool = setup_test_pool().await;
            let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
                pool.clone(),
                pool.clone(),
            ));
            let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
                pool.clone(),
                pool.clone(),
            ));
            let queue = Arc::new(JobQueue::with_repository(
                Default::default(),
                job_repo.clone(),
            ));
            let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
            let created = scheduler
                .create_dag_pipeline(
                    DagPipelineDefinition::new(
                        "retry despite observability failure",
                        vec![
                            DagStep::new(
                                "upload",
                                PipelineStep::inline("rclone", serde_json::json!({})),
                            )
                            .with_retry(StepRetryPolicy {
                                max_attempts: 2,
                                backoff_secs: 30,
                            }),
                        ],
                    ),
                    &["/input.flv".to_string()],
                    DagRunContext::default(),
                )
                .await
                .unwrap();
            let job = queue.dequeue(None).await.unwrap().unwrap();
            assert_eq!(queue.depth(), 1);
            assert!(queue.get_cancellation_token(&job.id).await.is_some());
            sqlx::query(fault).execute(&pool).await.unwrap();

            let outcome = scheduler
                .on_job_attempt_failed(
                    job.dag_step_execution_id.as_deref().unwrap(),
                    FailedStepAttempt {
                        job_id: &job.id,
                        retry_count: 0,
                        error: "upload failed",
                        kind: StepFailureKind::ProcessorError,
                    },
                    |message| {
                        let queue = &queue;
                        let job_id = &job.id;
                        async move {
                            queue
                                .fail_with_step_info(
                                    job_id,
                                    &message,
                                    Some("rclone"),
                                    Some(1),
                                    Some(1),
                                    &[],
                                )
                                .await
                        }
                    },
                )
                .await
                .unwrap();
            let StepFailureOutcome::Retried(planned) = outcome else {
                panic!("observability failure suppressed retry: {outcome:?}");
            };
            let stored = job_repo.get_job(&job.id).await.unwrap();
            assert_eq!(stored.get_status(), Some(JobStatus::Failed));
            assert_eq!(
                stored.retry_after,
                Some(planned.retry_after.timestamp_millis())
            );
            assert_eq!(
                stored.error.as_deref(),
                Some(planned.describe("upload failed").as_str())
            );
            assert_eq!(queue.depth(), 0);
            assert!(queue.get_cancellation_token(&job.id).await.is_none());
            assert_eq!(
                step_statuses(&dag_repo, &created.dag_id).await,
                ["upload=PROCESSING"]
            );
            assert_eq!(
                dag_repo
                    .get_dag(&created.dag_id)
                    .await
                    .unwrap()
                    .get_status(),
                Some(DagExecutionStatus::Processing)
            );
            queue.stop_progress_aggregator().await;
        }
    }

    /// The worker's completion entry point fails a workflow it cannot advance
    /// from the result, so the DAG settles and the worker still receives a
    /// completion to forward.
    #[tokio::test]
    async fn job_attempt_completion_fails_the_dag_it_cannot_advance() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            job_repo.clone(),
        );
        let created = scheduler
            .create_dag_pipeline(
                two_step_pipeline("unreadable definition"),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let steps = scheduler.get_dag_steps(&created.dag_id).await.unwrap();
        let step_a = steps.iter().find(|step| step.step_id == "A").unwrap();
        // Fan-out to B needs the definition; make it unreadable.
        sqlx::query("UPDATE dag_execution SET dag_definition = '{' WHERE id = ?")
            .bind(&created.dag_id)
            .execute(&pool)
            .await
            .unwrap();

        let update = scheduler
            .on_job_attempt_completed(&step_a.id, &["/a.mp4".to_string()], None, None, None, None)
            .await
            .unwrap();
        assert!(update.new_job_ids.is_empty());
        let completion = update.completion.expect("the failure settles the DAG");
        assert!(!completion.succeeded);
        assert_eq!(completion.dag_id, created.dag_id);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(
            dag.error
                .as_deref()
                .unwrap()
                .starts_with("DAG scheduler error: "),
            "{:?}",
            dag.error
        );
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=COMPLETED", "B=CANCELLED"],
            "the step's own completion stands; the dependent it could not reach is cancelled"
        );
    }

    /// `retry_dag` runs `after_reset` once the rows are reset and before any
    /// job is restarted; a rejected retry never runs it and leaves the rows
    /// alone; a retry that breaks down after the reset has run it, re-fails
    /// the DAG and hands back that failure's completion.
    #[tokio::test]
    async fn retry_dag_runs_the_reset_hook_between_reset_and_restart() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let queue = Arc::new(JobQueue::with_repository(
            Default::default(),
            job_repo.clone(),
        ));
        let scheduler = DagScheduler::new(queue.clone(), dag_repo.clone(), job_repo.clone());
        let failed_chain = |name: &str| {
            let scheduler = &scheduler;
            let job_repo = &job_repo;
            let name = name.to_string();
            async move {
                let created = scheduler
                    .create_dag_pipeline(
                        noop_pipeline(&name, &[("A", &[]), ("B", &["A"])]),
                        &["/input.flv".to_string()],
                        DagRunContext::default(),
                    )
                    .await
                    .unwrap();
                let root = scheduler
                    .get_dag_steps(&created.dag_id)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|step| step.step_id == "A")
                    .unwrap();
                crate::database::repositories::JobRepository::mark_job_failed(
                    job_repo.as_ref(),
                    &created.root_job_ids[0],
                    "boom",
                )
                .await
                .unwrap();
                let update = scheduler.on_job_failed(&root.id, "boom").await.unwrap();
                assert!(update.completion.is_some());
                created
            }
        };

        // Rejected before any row changes: the DAG is still running.
        let running = scheduler
            .create_dag_pipeline(
                noop_pipeline("running", &[("A", &[])]),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        let hook_calls = AtomicUsize::new(0);
        let error = scheduler
            .retry_dag(&running.dag_id, || {
                hook_calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&error, DagRetryError::Rejected(Error::Validation(message)) if message.contains("not in FAILED or CANCELLED")),
            "{error:?}"
        );
        assert_eq!(hook_calls.load(Ordering::SeqCst), 0);

        // Retried: the hook sees the reset rows before the restart re-queues A.
        let created = failed_chain("retried").await;
        let depth_before = queue.depth();
        let depth_at_hook = AtomicUsize::new(usize::MAX);
        let update = scheduler
            .retry_dag(&created.dag_id, || {
                hook_calls.fetch_add(1, Ordering::SeqCst);
                depth_at_hook.store(queue.depth(), Ordering::SeqCst);
            })
            .await
            .unwrap();
        assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            depth_at_hook.load(Ordering::SeqCst),
            depth_before,
            "nothing re-queued yet"
        );
        assert_eq!(
            queue.depth(),
            depth_before + 1,
            "A was re-queued after the hook"
        );
        assert_eq!(
            (
                update.retryable_steps,
                update.retried_steps,
                update.reconciled_steps
            ),
            (2, 2, 0)
        );
        assert_eq!(update.job_ids, vec![created.root_job_ids[0].clone()]);
        assert_eq!(update.restarted_jobs.len(), 1);
        assert_eq!(update.restarted_jobs[0].id, created.root_job_ids[0]);
        assert!(update.completions.is_empty());
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=PROCESSING", "B=BLOCKED"]
        );
        assert_eq!(
            job_repo
                .get_job(&created.root_job_ids[0])
                .await
                .unwrap()
                .status,
            JobStatus::Pending.as_str()
        );

        // Refailed: the reset took effect, then fan-out could not read the definition.
        let created = failed_chain("refailed").await;
        sqlx::query("UPDATE dag_execution SET dag_definition = '{' WHERE id = ?")
            .bind(&created.dag_id)
            .execute(&pool)
            .await
            .unwrap();
        let error = scheduler
            .retry_dag(&created.dag_id, || {
                hook_calls.fetch_add(1, Ordering::SeqCst);
            })
            .await
            .unwrap_err();
        assert_eq!(hook_calls.load(Ordering::SeqCst), 2);
        let DagRetryError::Refailed { error, completion } = error else {
            panic!("{error:?}");
        };
        assert!(error.to_string().contains("DAG definition"), "{error}");
        let completion = completion.expect("the re-failed DAG settles");
        assert!(!completion.succeeded);
        assert_eq!(completion.dag_id, created.dag_id);
        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert!(
            dag.error.as_deref().unwrap().starts_with("Retry failed:"),
            "{:?}",
            dag.error
        );
        assert_eq!(
            step_statuses(&dag_repo, &created.dag_id).await,
            ["A=CANCELLED", "B=CANCELLED"],
            "no step is left PROCESSING"
        );
        assert_eq!(
            job_repo
                .get_job(&created.root_job_ids[0])
                .await
                .unwrap()
                .status,
            JobStatus::Failed.as_str(),
            "the job was never restarted"
        );
    }

    #[tokio::test]
    async fn dag_publication_rolls_back_all_rows_when_root_attachment_fails() {
        let pool = setup_test_pool().await;
        let dag_repo =
            crate::database::repositories::dag::SqlxDagRepository::new(pool.clone(), pool.clone());
        let definition = DagPipelineDefinition::new(
            "atomic publication",
            vec![DagStep::new("A", PipelineStep::preset("noop"))],
        );
        let mut dag = DagExecutionDbModel::new(&definition, None, None);
        dag.status = DagExecutionStatus::Processing.as_str().to_string();
        let mut step = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
        let mut job = JobDbModel::new_pipeline_step("noop", "[]", "[]", 0, None, None);
        step.status = DagStepStatus::Processing.as_str().to_string();
        step.job_id = Some(job.id.clone());
        job.pipeline_id = Some(dag.id.clone());
        job.dag_step_execution_id = Some("missing-step".to_string());

        assert!(
            dag_repo
                .publish_dag(&dag, &[step], &[job.clone()])
                .await
                .is_err()
        );
        assert!(dag_repo.get_dag(&dag.id).await.is_err());
        assert!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM dag_step_execution WHERE dag_id = ?"
            )
            .bind(&dag.id)
            .fetch_one(&pool)
            .await
            .unwrap()
                == 0
        );
        assert!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM job WHERE id = ?")
                .bind(&job.id)
                .fetch_one(&pool)
                .await
                .unwrap()
                == 0
        );
    }

    #[tokio::test]
    async fn terminal_dag_status_cannot_be_reopened() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool.clone(),
        ));
        let job_repo = Arc::new(crate::database::repositories::job::SqlxJobRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(Arc::new(JobQueue::new()), dag_repo.clone(), job_repo);
        let created = scheduler
            .create_dag_pipeline(
                DagPipelineDefinition::new(
                    "terminal status guard",
                    vec![DagStep::new("A", PipelineStep::preset("noop"))],
                ),
                &["/input.flv".to_string()],
                DagRunContext::default(),
            )
            .await
            .unwrap();
        dag_repo
            .fail_dag_and_cancel_steps(&created.dag_id, "root failed")
            .await
            .unwrap();
        dag_repo
            .update_dag_status(
                &created.dag_id,
                DagExecutionStatus::Processing.as_str(),
                None,
            )
            .await
            .unwrap();

        let dag = dag_repo.get_dag(&created.dag_id).await.unwrap();
        assert_eq!(dag.get_status(), Some(DagExecutionStatus::Failed));
        assert_eq!(dag.error.as_deref(), Some("root failed"));
        assert!(dag.completed_at.is_some());
    }

    #[tokio::test]
    async fn test_cancel_dag_marks_parent_cancelled() {
        let pool = setup_test_pool().await;
        let dag_repo = Arc::new(crate::database::repositories::dag::SqlxDagRepository::new(
            pool.clone(),
            pool,
        ));
        let scheduler = DagScheduler::new(
            Arc::new(JobQueue::new()),
            dag_repo.clone(),
            Arc::new(NoopJobRepository),
        );

        let dag_def = DagPipelineDefinition::new(
            "test",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );
        let dag = crate::database::models::DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        dag_repo.create_dag(&dag).await.unwrap();
        dag_repo
            .create_step(&DagStepExecutionDbModel::new(&dag_id, "A", &[]))
            .await
            .unwrap();
        dag_repo
            .update_dag_status(
                &dag_id,
                crate::database::models::DagExecutionStatus::Processing.as_str(),
                None,
            )
            .await
            .unwrap();

        let result = scheduler.cancel_dag_with_completion(&dag_id).await.unwrap();
        assert_eq!(result.cancelled_count, 0);

        let dag = dag_repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(
            dag.status,
            crate::database::models::DagExecutionStatus::Cancelled.as_str()
        );
    }
}
