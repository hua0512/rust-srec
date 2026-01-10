//! DAG Scheduler for orchestrating DAG pipeline execution.
//!
//! The DagScheduler is responsible for:
//! - Creating jobs for ready DAG steps
//! - Handling job completion and triggering downstream steps (fan-in)
//! - Implementing fail-fast behavior on job failure
//! - Tracking DAG execution progress

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::database::models::{
    DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStepExecutionDbModel,
    DagStepStatus, JobDbModel, PipelineStep, ReadyStep,
};
use crate::database::repositories::{DagRepository, JobRepository};
use crate::pipeline::{Job, JobQueue, JobStatus};
use crate::{Error, Result};

type BeforeRootJobsHook = Box<dyn FnOnce(&str) + Send>;

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
    #[allow(clippy::too_many_arguments)]
    pub async fn create_dag_pipeline(
        &self,
        dag_definition: DagPipelineDefinition,
        input_paths: &[String],
        streamer_id: Option<String>,
        session_id: Option<String>,
        streamer_name: Option<String>,
        session_title: Option<String>,
        platform: Option<String>,
    ) -> Result<DagCreationResult> {
        self.create_dag_pipeline_with_hook(
            dag_definition,
            input_paths,
            streamer_id,
            session_id,
            streamer_name,
            session_title,
            platform,
            None,
        )
        .await
    }

    /// Create a new DAG pipeline execution with an optional hook called after step execution
    /// records are created but before any root jobs are enqueued.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_dag_pipeline_with_hook(
        &self,
        dag_definition: DagPipelineDefinition,
        input_paths: &[String],
        streamer_id: Option<String>,
        session_id: Option<String>,
        streamer_name: Option<String>,
        session_title: Option<String>,
        platform: Option<String>,
        before_root_jobs: Option<BeforeRootJobsHook>,
    ) -> Result<DagCreationResult> {
        // 1. Validate DAG structure
        dag_definition.validate()?;

        // 2. Create DAG execution record
        let dag_exec =
            DagExecutionDbModel::new(&dag_definition, streamer_id.clone(), session_id.clone());
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
                    streamer_id.clone(),
                    session_id.clone(),
                    streamer_name.clone(),
                    session_title.clone(),
                    platform.clone(),
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
            let _ = self.job_queue.cancel_job(job_id).await;
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
                        dag.streamer_id.clone(),
                        dag.session_id.clone(),
                        streamer_name.clone(),
                        session_title.clone(),
                        platform.clone(),
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

        let updated_dag = self.dag_repository.get_dag(&step.dag_id).await.ok();
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
    #[allow(clippy::too_many_arguments)]
    async fn create_step_job(
        &self,
        dag_id: &str,
        step_execution_id: &str,
        dag_step: &crate::database::models::DagStep,
        inputs: Vec<String>,
        streamer_id: Option<String>,
        session_id: Option<String>,
        streamer_name: Option<String>,
        session_title: Option<String>,
        platform: Option<String>,
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
            streamer_id.clone(),
            session_id.clone(),
        );
        job_db.config = config;
        job_db.state = serde_json::json!({
            "streamer_name": streamer_name.clone(),
            "session_title": session_title.clone(),
            "platform": platform.clone(),
        })
        .to_string();
        job_db.dag_step_execution_id = Some(step_execution_id.to_string());

        let job_id = job_db.id.clone();

        // Create the job in the repository
        self.job_repository.create_job(&job_db).await?;

        // Update step execution with job ID and PROCESSING status
        self.dag_repository
            .update_step_status_with_job(
                step_execution_id,
                DagStepStatus::Processing.as_str(),
                &job_id,
            )
            .await?;

        // Create in-memory Job and enqueue
        let job = Job {
            id: job_db.id.clone(),
            job_type: processor,
            inputs,
            outputs: Vec::new(),
            priority: 0,
            status: JobStatus::Pending,
            streamer_id: job_db.streamer_id.clone().unwrap_or_default(),
            session_id: job_db.session_id.clone().unwrap_or_default(),
            streamer_name,
            session_title,
            platform,
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

    /// Cancel a DAG execution.
    pub async fn cancel_dag(&self, dag_id: &str) -> Result<u64> {
        let dag = self.dag_repository.get_dag(dag_id).await?;

        if dag.get_status().map(|s| s.is_terminal()).unwrap_or(false) {
            return Err(Error::Validation(format!(
                "DAG {} is already in terminal state",
                dag_id
            )));
        }

        let cancelled = self
            .dag_repository
            .fail_dag_and_cancel_steps(dag_id, "Cancelled by user")
            .await?;

        // Cancel any processing jobs
        for job_id in &cancelled {
            let _ = self.job_queue.cancel_job(job_id).await;
        }

        Ok(cancelled.len() as u64)
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
        self.enqueue_now_ready_steps(dag_id, None, None, None)
            .await?;

        Ok(())
    }

    async fn enqueue_now_ready_steps(
        &self,
        dag_id: &str,
        streamer_name: Option<&str>,
        session_title: Option<&str>,
        platform: Option<&str>,
    ) -> Result<Vec<String>> {
        let streamer_name = streamer_name.map(ToString::to_string);
        let session_title = session_title.map(ToString::to_string);
        let platform = platform.map(ToString::to_string);

        let dag = self.dag_repository.get_dag(dag_id).await?;
        let dag_def = dag
            .get_dag_definition()
            .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
        let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;

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
                    dag.streamer_id.clone(),
                    dag.session_id.clone(),
                    streamer_name.clone(),
                    session_title.clone(),
                    platform.clone(),
                )
                .await?;
            new_job_ids.push(job_id);
        }

        Ok(new_job_ids)
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
    use crate::database::models::{DagPipelineDefinition, DagStep, PipelineStep};

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
}
