//! DAG Scheduler for orchestrating DAG pipeline execution.
//!
//! The DagScheduler is responsible for:
//! - Creating jobs for ready DAG steps
//! - Handling job completion and triggering downstream steps (fan-in)
//! - Cancelling the steps downstream of a failed job while other branches settle
//! - Tracking DAG execution progress

use jiff::Timestamp;
use std::sync::Arc;

use crate::Error;
use crate::database::models::{DagExecutionDbModel, DagSegmentSource};
use crate::database::repositories::{DagRepository, JobRepository};
use crate::pipeline::job_queue::JobStateMeta;
use crate::pipeline::manifest::PipelineInputManifest;
use crate::pipeline::{Job, JobQueue};
use crate::utils::json::{self, JsonContext};

pub(crate) type PublicationRollback = Box<dyn FnOnce() + Send>;
pub(crate) type BeforeRootJobsHook = Box<dyn FnOnce(&str) -> PublicationRollback + Send>;

/// Optional metadata stored on the DAG execution row.
#[derive(Debug, Clone, Default)]
pub struct DagExecutionMetadata {
    pub segment_index: Option<u32>,
    pub segment_source: Option<DagSegmentSource>,
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
    pub session_start: Option<Timestamp>,
    /// Copied into every step job; taken from `DagExecutionMetadata` at
    /// publication and from the DAG row afterwards.
    pub manifest: Option<Arc<PipelineInputManifest>>,
}

impl DagRunContext {
    /// Context for steps materialized after publication. The manifest comes from
    /// the DAG row rather than a sibling job so downstream steps do not depend on
    /// another job's state surviving.
    pub(super) fn for_downstream(dag: &DagExecutionDbModel, meta: JobStateMeta) -> Self {
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

pub(crate) struct LeafOutputs {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedRetry {
    /// When the manager's retry sweeper may re-queue the job.
    pub retry_after: Timestamp,
    /// Number of the attempt that will run next; the first run is attempt 1.
    pub next_attempt: u32,
    /// The step's attempt budget, including the first run.
    pub max_attempts: u32,
}

/// A workflow step's retry budget and whether it still waits on a given job.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepRetryState {
    /// Attempts the step's retry policy allows, counting the first run; `None`
    /// when the step has no policy.
    pub max_attempts: Option<u32>,
    /// Whether the step is still running the job in a DAG that has not
    /// settled, so the job's scheduled retry will run.
    pub awaits_job: bool,
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
    /// The fault lies with the database that refused the write, not with the
    /// job, so the failure is applied at once.
    ExecutionStart,
    /// The processor rejected the job as permanently invalid or unrecoverable
    /// (for example malformed configuration, missing initial input, invalid
    /// credentials, or files permanently rejected by the destination).
    NonRetryable,
}

impl StepFailureKind {
    pub(super) fn retryable(self) -> bool {
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
pub(super) enum RetryAction {
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
mod completion;
mod creation;
mod queries;
mod recovery;
#[cfg(test)]
mod tests;

pub struct DagScheduler {
    pub(super) job_queue: Arc<JobQueue>,
    pub(super) dag_repository: Arc<dyn DagRepository>,
    pub(super) job_repository: Arc<dyn JobRepository>,
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
}
