//! DAG Scheduler for orchestrating DAG pipeline execution.
//!
//! The DagScheduler is responsible for:
//! - Creating jobs for ready DAG steps
//! - Handling job completion and triggering downstream steps (fan-in)
//! - Implementing fail-fast behavior on job failure
//! - Tracking DAG execution progress

use std::sync::Arc;
use tracing::{debug, error, info, warn};

use crate::database::models::{
    DagExecutionDbModel, DagPipelineDefinition, DagStepExecutionDbModel, DagStepStatus, JobDbModel,
    PipelineStep, ReadyStep,
};
use crate::database::repositories::{DagRepository, JobRepository};
use crate::pipeline::{Job, JobQueue, JobStatus};
use crate::{Error, Result};

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
        streamer_id: Option<String>,
        session_id: Option<String>,
    ) -> Result<DagCreationResult> {
        // 1. Validate DAG structure
        dag_definition
            .validate()
            .map_err(|e| Error::Validation(e.into()))?;

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
                )
                .await?;

            root_job_ids.push(job_id);
        }

        Ok(DagCreationResult {
            dag_id,
            root_job_ids,
            total_steps: dag_definition.steps.len(),
        })
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
        outputs: Vec<String>,
    ) -> Result<Vec<String>> {
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
            .complete_step_and_check_dependents(dag_step_execution_id, &outputs)
            .await?;

        if ready_steps.is_empty() {
            debug!(
                dag_id = %step.dag_id,
                step_id = %step.step_id,
                "No dependent steps are ready yet"
            );
            return Ok(Vec::new());
        }

        // Get DAG definition to resolve step configs
        let dag = self.dag_repository.get_dag(&step.dag_id).await?;
        let dag_def = dag
            .get_dag_definition()
            .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;

        // Create jobs for ready steps
        let mut new_job_ids = Vec::with_capacity(ready_steps.len());

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

            let job_id = self
                .create_step_job(
                    &step.dag_id,
                    &ready_step.id,
                    dag_step,
                    merged_inputs,
                    dag.streamer_id.clone(),
                    dag.session_id.clone(),
                )
                .await?;

            new_job_ids.push(job_id);
        }

        // Check if DAG is complete
        let updated_dag = self.dag_repository.get_dag(&step.dag_id).await?;
        if updated_dag.is_complete() {
            if updated_dag.is_failed() {
                info!(
                    dag_id = %step.dag_id,
                    completed = %updated_dag.completed_steps,
                    failed = %updated_dag.failed_steps,
                    "DAG execution completed with failures"
                );
            } else {
                info!(
                    dag_id = %step.dag_id,
                    completed = %updated_dag.completed_steps,
                    "DAG execution completed successfully"
                );
            }
        }

        Ok(new_job_ids)
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
    pub async fn on_job_failed(&self, dag_step_execution_id: &str, error: &str) -> Result<u64> {
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

        Ok(cancelled_count)
    }

    /// Create a job for a DAG step.
    async fn create_step_job(
        &self,
        dag_id: &str,
        step_execution_id: &str,
        dag_step: &crate::database::models::DagStep,
        inputs: Vec<String>,
        streamer_id: Option<String>,
        session_id: Option<String>,
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
        let inputs_json = serde_json::to_string(&inputs).unwrap_or_else(|_| "[]".to_string());

        let mut job_db = JobDbModel::new_pipeline_step(
            &processor,
            inputs_json,
            "[]".to_string(),
            0, // priority
            streamer_id,
            session_id,
            Some(dag_id.to_string()), // Use DAG ID as pipeline_id for grouping
            None,                     // No next_job_type for DAG (handled by scheduler)
            None,                     // No remaining_steps for DAG
        );
        job_db.config = config;
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
            config: Some(job_db.config.clone()),
            created_at: chrono::Utc::now(),
            started_at: None,
            completed_at: None,
            error: None,
            retry_count: 0,
            next_job_type: None,
            remaining_steps: None,
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

    /// List DAG executions with optional status filter.
    pub async fn list_dags(
        &self,
        status: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        self.dag_repository.list_dags(status, limit, offset).await
    }

    /// Count DAG executions with optional status filter.
    pub async fn count_dags(&self, status: Option<&str>) -> Result<u64> {
        self.dag_repository.count_dags(status).await
    }

    /// Get statistics for a DAG execution.
    pub async fn get_dag_stats(
        &self,
        dag_id: &str,
    ) -> Result<crate::database::models::dag::DagExecutionStats> {
        self.dag_repository.get_dag_stats(dag_id).await
    }
}

#[cfg(test)]
mod tests {
    // Tests would go here but require mocking the repositories and job queue
    // which is complex. Integration tests would be more appropriate.
}
