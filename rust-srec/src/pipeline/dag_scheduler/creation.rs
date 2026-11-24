//! DAG pipeline creation and step-job materialization.

use std::collections::VecDeque;
use std::sync::Arc;

use tracing::{info, warn};

use super::{
    BeforeRootJobsHook, DagCreationResult, DagExecutionMetadata, DagRunContext, DagScheduler,
};
use crate::database::models::{
    DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStepExecutionDbModel,
    DagStepStatus, JobDbModel, PipelineStep, ReadyStep,
};
use crate::pipeline::job_queue::{JobStateMeta, job_state_json};
use crate::pipeline::{Job, JobStatus};
use crate::{Error, Result};

impl DagScheduler {
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
            dag_exec.segment_source = meta
                .segment_source
                .map(|source| source.as_str().to_string());
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

    pub(super) fn build_step_job(
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
            created_at: jiff::Timestamp::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            retry_after: None,
            timeout_secs: dag_step.timeout_secs,
            has_step_retry: Some(dag_step.retry.as_ref().is_some_and(|p| p.max_attempts > 1)),
            enqueued_at_ms: None,
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
    pub(super) async fn materialize_ready_steps(
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
    pub(super) fn runs_without_inputs(step: &PipelineStep) -> bool {
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
    pub(super) async fn create_step_job(
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
}
