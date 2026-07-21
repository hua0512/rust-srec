//! DAG Scheduler for orchestrating DAG pipeline execution.
//!
//! The DagScheduler is responsible for:
//! - Creating jobs for ready DAG steps
//! - Handling job completion and triggering downstream steps (fan-in)
//! - Implementing fail-fast behavior on job failure
//! - Tracking DAG execution progress

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::database::models::{
    DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStepExecutionDbModel,
    DagStepStatus, JobDbModel, PipelineStep, ReadyStep,
};
use crate::database::repositories::{DagRepository, JobRepository};
use crate::pipeline::job_queue::{JobStateMeta, job_state_json, parse_job_state};
use crate::pipeline::{Job, JobQueue, JobStatus};
use crate::{Error, Result};

type BeforeRootJobsHook = Box<dyn FnOnce(&str) + Send>;

/// Optional metadata associated with a DAG execution.
#[derive(Debug, Clone, Default)]
pub struct DagExecutionMetadata {
    pub segment_index: Option<u32>,
    pub segment_source: Option<String>,
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

    fn output_dedup_key(output: &str) -> String {
        if cfg!(windows) {
            output.to_lowercase()
        } else {
            output.to_string()
        }
    }

    fn collect_leaf_outputs_from_step_executions(
        def: &DagPipelineDefinition,
        step_execs: &[DagStepExecutionDbModel],
    ) -> Vec<String> {
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

        for leaf in def.leaf_steps() {
            let Some(exec) = exec_by_step_id.get(leaf.id.as_str()) else {
                continue;
            };
            for output in exec.get_outputs() {
                if seen.insert(Self::output_dedup_key(&output)) {
                    outputs.push(output);
                }
            }
        }

        outputs
    }

    async fn collect_leaf_outputs(&self, dag: &DagExecutionDbModel) -> Result<Vec<String>> {
        let Some(def) = dag.get_dag_definition() else {
            return Ok(Vec::new());
        };

        if def.leaf_steps().is_empty() {
            return Ok(Vec::new());
        }

        let step_execs = self.dag_repository.get_steps_by_dag(&dag.id).await?;
        Ok(Self::collect_leaf_outputs_from_step_executions(
            &def,
            &step_execs,
        ))
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

    /// Create a new DAG pipeline execution with an optional hook called after step execution
    /// records are created but before any root jobs are enqueued.
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

        // 2. Create DAG execution record
        let mut dag_exec = DagExecutionDbModel::new(
            &dag_definition,
            context.streamer_id.clone(),
            context.session_id.clone(),
        );
        if let Some(meta) = metadata {
            dag_exec.segment_index = meta.segment_index.map(i64::from);
            dag_exec.segment_source = meta.segment_source;
        }
        let dag_id = dag_exec.id.clone();

        self.dag_repository.create_dag(&dag_exec).await?;

        info!(
            dag_id = %dag_id,
            total_steps = %dag_definition.steps.len(),
            "Created DAG pipeline execution"
        );

        // 3. Create step execution records for all steps
        let mut step_executions = Vec::with_capacity(dag_definition.steps.len());

        for dag_step in &dag_definition.steps {
            let step_exec =
                DagStepExecutionDbModel::new(&dag_id, &dag_step.id, &dag_step.depends_on);
            step_executions.push(step_exec);
        }

        self.dag_repository.create_steps(&step_executions).await?;

        if let Some(hook) = before_root_jobs {
            hook(&dag_id);
        }

        // 4. Create jobs for root steps
        let root_steps = dag_definition.root_steps();
        let mut root_job_ids = Vec::with_capacity(root_steps.len());

        for root_step in root_steps {
            let step_exec = step_executions
                .iter()
                .find(|s| s.step_id == root_step.id)
                .ok_or_else(|| Error::Validation("Step execution not found".into()))?;

            let job_id = self
                .create_step_job(
                    &dag_id,
                    &step_exec.id,
                    root_step,
                    input_paths.to_vec(),
                    &context,
                )
                .await?;

            root_job_ids.push(job_id);
        }

        // 5. Update DAG status to PROCESSING now that jobs are queued
        self.dag_repository
            .update_dag_status(&dag_id, DagExecutionStatus::Processing.as_str(), None)
            .await?;

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
        let status = dag.get_status();
        if !status.map(|s| s.is_terminal()).unwrap_or(false) {
            return Ok(None);
        }

        let leaf_outputs = self.collect_leaf_outputs(&dag).await.unwrap_or_default();
        let succeeded = status == Some(DagExecutionStatus::Completed);

        Ok(Some(DagCompletionInfo {
            dag_id: dag.id.clone(),
            streamer_id: dag.streamer_id.clone(),
            session_id: dag.session_id.clone(),
            succeeded,
            leaf_outputs,
        }))
    }

    /// Fail the DAG execution for a given step execution ID (used when the scheduler can't advance).
    pub async fn fail_dag_for_step(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<Option<DagCompletionInfo>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;
        self.fail_dag_internal(&step.dag_id, error).await
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

        // Get step info for logging
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;

        info!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            outputs_count = %outputs.len(),
            "DAG step completed"
        );

        // Atomically complete step and find ready dependents
        let ready_steps = self
            .dag_repository
            .complete_step_and_check_dependents(dag_step_execution_id, outputs)
            .await?;

        let mut new_job_ids = Vec::new();

        if !ready_steps.is_empty() {
            // Get DAG definition to resolve step configs
            let dag = self.dag_repository.get_dag(&step.dag_id).await?;
            let dag_def = dag
                .get_dag_definition()
                .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;

            new_job_ids = Vec::with_capacity(ready_steps.len());

            // Create jobs for ready steps
            for ReadyStep {
                step: ready_step,
                merged_inputs,
            } in ready_steps
            {
                let dag_step = dag_def.get_step(&ready_step.step_id).ok_or_else(|| {
                    Error::Validation(format!(
                        "Step '{}' not found in DAG definition",
                        ready_step.step_id
                    ))
                })?;

                info!(
                    dag_id = %step.dag_id,
                    step_id = %ready_step.step_id,
                    inputs_count = %merged_inputs.len(),
                    "Creating job for ready step (fan-in merge)"
                );

                let job_id = match self
                    .create_step_job(
                        &step.dag_id,
                        &ready_step.id,
                        dag_step,
                        merged_inputs,
                        &DagRunContext {
                            streamer_id: dag.streamer_id.clone(),
                            session_id: dag.session_id.clone(),
                            streamer_name: streamer_name.clone(),
                            session_title: session_title.clone(),
                            platform: platform.clone(),
                            session_start,
                        },
                    )
                    .await
                {
                    Ok(job_id) => job_id,
                    Err(e) => {
                        let err_msg = format!(
                            "Failed to create downstream DAG job for step {}: {}",
                            ready_step.step_id, e
                        );
                        warn!(
                            dag_id = %step.dag_id,
                            error = %err_msg,
                            "Failing DAG due to scheduler error"
                        );
                        let completion = self.fail_dag_internal(&step.dag_id, &err_msg).await?;
                        return Ok(DagJobCompletedUpdate {
                            new_job_ids,
                            completion,
                        });
                    }
                };

                new_job_ids.push(job_id);
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
        let completion = if updated_dag
            .get_status()
            .map(|s| s.is_terminal())
            .unwrap_or(false)
        {
            let leaf_outputs = self
                .collect_leaf_outputs(&updated_dag)
                .await
                .unwrap_or_default();
            let succeeded = updated_dag.get_status() == Some(DagExecutionStatus::Completed);
            Some(DagCompletionInfo {
                dag_id: updated_dag.id.clone(),
                streamer_id: updated_dag.streamer_id.clone(),
                session_id: updated_dag.session_id.clone(),
                succeeded,
                leaf_outputs,
            })
        } else {
            None
        };

        Ok(DagJobCompletedUpdate {
            new_job_ids,
            completion,
        })
    }

    /// Handle job failure for a DAG step.
    ///
    /// Implements fail-fast behavior:
    /// 1. Marks the step as failed
    /// 2. Cancels all pending/blocked steps in the DAG
    /// 3. Signals cancellation for any processing jobs
    /// 4. Marks the DAG as failed
    ///
    /// Returns the count of cancelled items.
    pub async fn on_job_failed(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<DagJobFailedUpdate> {
        // Get step info
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;

        error!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            error = %error,
            "DAG step failed, implementing fail-fast"
        );

        // Mark step as failed
        self.dag_repository
            .update_step_status(dag_step_execution_id, DagStepStatus::Failed.as_str())
            .await?;

        // Increment failed counter
        self.dag_repository
            .increment_dag_failed(&step.dag_id)
            .await?;

        // Fail DAG and cancel all pending/blocked steps, get processing job IDs
        let processing_job_ids = self
            .dag_repository
            .fail_dag_and_cancel_steps(
                &step.dag_id,
                &format!("Step '{}' failed: {}", step.step_id, error),
            )
            .await?;

        // Cancel processing jobs
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

        info!(
            dag_id = %step.dag_id,
            cancelled_jobs = %cancelled_count,
            "DAG failed, cancelled pending work"
        );

        let updated_dag = match self.dag_repository.get_dag(&step.dag_id).await {
            Ok(dag) => Some(dag),
            Err(error) => {
                warn!(dag_id = %step.dag_id, %error, "Failed to reload failed DAG");
                None
            }
        };
        let completion = if let Some(dag) = updated_dag
            && dag.get_status().map(|s| s.is_terminal()).unwrap_or(false)
        {
            let leaf_outputs = self.collect_leaf_outputs(&dag).await.unwrap_or_default();
            Some(DagCompletionInfo {
                dag_id: dag.id.clone(),
                streamer_id: dag.streamer_id.clone(),
                session_id: dag.session_id.clone(),
                succeeded: false,
                leaf_outputs,
            })
        } else {
            None
        };

        Ok(DagJobFailedUpdate {
            cancelled_count,
            completion,
        })
    }

    /// Create a job for a DAG step.
    async fn create_step_job(
        &self,
        dag_id: &str,
        step_execution_id: &str,
        dag_step: &crate::database::models::DagStep,
        inputs: Vec<String>,
        context: &DagRunContext,
    ) -> Result<String> {
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
            pipeline_id: Some(dag_id.to_string()),
            execution_info: None,
            duration_secs: None,
            queue_wait_secs: None,
            dag_step_execution_id: Some(step_execution_id.to_string()),
        };
        job_db.state = job_state_json(&job);

        let job_id = job_db.id.clone();

        // Create the job in the repository
        self.job_repository.create_job(&job_db).await?;

        // Update step execution with job ID and PROCESSING status
        if let Err(e) = self
            .dag_repository
            .update_step_status_with_job(
                step_execution_id,
                DagStepStatus::Processing.as_str(),
                &job_id,
            )
            .await
        {
            // Best-effort: avoid leaking an orphaned job if we failed to attach it to the DAG step.
            if let Err(delete_err) = self.job_repository.delete_job(&job_id).await {
                warn!(
                    job_id = %job_id,
                    error = %delete_err,
                    "Failed to delete orphaned DAG job after step update failure"
                );
            }
            return Err(e);
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

        Ok(job_id)
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
            return Err(Error::Validation(format!(
                "DAG {} is already in terminal state",
                dag_id
            )));
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
        let completion = if let Some(dag) = updated_dag
            && dag.get_status().map(|s| s.is_terminal()).unwrap_or(false)
        {
            let leaf_outputs = self.collect_leaf_outputs(&dag).await.unwrap_or_default();
            Some(DagCompletionInfo {
                dag_id: dag.id.clone(),
                streamer_id: dag.streamer_id.clone(),
                session_id: dag.session_id.clone(),
                succeeded: false,
                leaf_outputs,
            })
        } else {
            None
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

    /// Reset a failed DAG execution so it can be retried.
    ///
    /// This restores cancelled downstream steps back to `BLOCKED`, clears the DAG terminal state,
    /// and marks failed steps as active again so retried jobs can fan-out to subsequent steps.
    pub async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
        self.dag_repository.reset_dag_for_retry(dag_id).await?;

        // Some cancelled steps may already have all dependencies completed (e.g. a parallel
        // branch when fail-fast triggers). Since no new completion events will occur for those
        // dependencies, proactively enqueue any now-ready steps.
        self.enqueue_now_ready_steps(dag_id).await?;

        Ok(())
    }

    async fn enqueue_now_ready_steps(&self, dag_id: &str) -> Result<Vec<String>> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        let dag_def = dag
            .get_dag_definition()
            .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
        let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;

        // The retry entry point (reset_dag_for_retry) only knows the dag_id,
        // so the placeholder metadata for jobs created here is recovered from
        // a sibling step's job row: create_step_job persisted streamer_name /
        // session_title / platform / session_start_ms in every job's state
        // JSON. Relying on the dequeue-time backfill
        // (JobQueue::resolve_job_metadata) instead would lose session_start
        // whenever the live_sessions row was deleted before the retry.
        let JobStateMeta {
            streamer_name,
            session_title,
            platform,
            session_start,
        } = self.recover_placeholder_metadata(&steps).await;

        let mut status_by_step_id = HashMap::<String, String>::with_capacity(steps.len());
        let mut outputs_by_step_id = HashMap::<String, Vec<String>>::with_capacity(steps.len());

        for step in &steps {
            status_by_step_id.insert(step.step_id.clone(), step.status.clone());
            if step.status == DagStepStatus::Completed.as_str() {
                outputs_by_step_id.insert(step.step_id.clone(), step.get_outputs());
            }
        }

        let mut new_job_ids = Vec::new();

        for step_exec in steps {
            if step_exec.status != DagStepStatus::Blocked.as_str() || step_exec.job_id.is_some() {
                continue;
            }

            let depends_on = step_exec.get_depends_on();
            if depends_on.is_empty() {
                // Root steps should already have a job created at DAG creation time.
                continue;
            }

            let all_deps_complete = depends_on.iter().all(|dep| {
                status_by_step_id
                    .get(dep)
                    .map(|s| s == DagStepStatus::Completed.as_str())
                    .unwrap_or(false)
            });
            if !all_deps_complete {
                continue;
            }

            let merged_inputs = {
                let mut merged = Vec::new();
                let mut seen = HashSet::<String>::new();
                for dep in &depends_on {
                    let Some(dep_outputs) = outputs_by_step_id.get(dep) else {
                        continue;
                    };
                    for out in dep_outputs {
                        if seen.insert(Self::output_dedup_key(out)) {
                            merged.push(out.clone());
                        }
                    }
                }
                merged
            };

            let dag_step = dag_def.get_step(&step_exec.step_id).ok_or_else(|| {
                Error::Validation(format!(
                    "Step '{}' not found in DAG definition",
                    step_exec.step_id
                ))
            })?;

            let job_id = self
                .create_step_job(
                    dag_id,
                    &step_exec.id,
                    dag_step,
                    merged_inputs,
                    &DagRunContext {
                        streamer_id: dag.streamer_id.clone(),
                        session_id: dag.session_id.clone(),
                        streamer_name: streamer_name.clone(),
                        session_title: session_title.clone(),
                        platform: platform.clone(),
                        session_start,
                    },
                )
                .await?;
            new_job_ids.push(job_id);
        }

        Ok(new_job_ids)
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

            // A sibling created before this feature (or by an earlier retry)
            // may carry an all-null state; keep scanning for one with values.
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
    pub async fn delete_dag(&self, dag_id: &str) -> Result<()> {
        // Verify DAG exists first
        self.dag_repository.get_dag(dag_id).await?;

        // Delete all associated jobs and their logs
        self.job_repository.delete_jobs_by_pipeline(dag_id).await?;

        // Delete the DAG (CASCADE deletes steps)
        self.dag_repository.delete_dag(dag_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            unimplemented!("not needed for these tests")
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
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                },
                DagStep {
                    id: "C".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
                },
            ],
        );

        let mut exec_b = DagStepExecutionDbModel::new("dag1", "B", &["A".to_string()]);
        exec_b.set_outputs(&["x".to_string(), "y".to_string()]);
        let mut exec_c = DagStepExecutionDbModel::new("dag1", "C", &["A".to_string()]);
        exec_c.set_outputs(&["y".to_string(), "z".to_string()]);

        // Simulate non-deterministic row order from DB (C then B).
        let step_execs = vec![exec_c, exec_b];

        let out = DagScheduler::collect_leaf_outputs_from_step_executions(&def, &step_execs);
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
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
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
                },
                DagStep {
                    id: "B".to_string(),
                    step: PipelineStep::inline("noop", serde_json::json!({})),
                    depends_on: vec!["A".to_string()],
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
            .unwrap();
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
