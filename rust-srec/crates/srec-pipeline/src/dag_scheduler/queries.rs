//! Read queries and deletion for DAG executions and step records.

use tracing::warn;

use super::DagScheduler;
use crate::JobProgressSnapshot;
use crate::repositories::DagListFilter;
use crate::{PipelineError as Error, PipelineResult as Result};
use srec_sqlite::models::{
    DagExecutionDbModel, DagExecutionListSummary, DagExecutionStatus, DagStepExecutionDbModel,
    DagStepWithJob,
};

impl DagScheduler {
    /// Get DAG execution status.
    pub async fn get_dag_status(&self, dag_id: &str) -> Result<DagExecutionDbModel> {
        Ok(self.dag_repository.get_dag(dag_id).await?)
    }

    /// Get all step executions for a DAG.
    pub async fn get_dag_steps(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        Ok(self.dag_repository.get_steps_by_dag(dag_id).await?)
    }

    /// Get all step executions for a DAG, each with the status and timing of its job.
    pub async fn get_dag_steps_with_jobs(&self, dag_id: &str) -> Result<Vec<DagStepWithJob>> {
        Ok(self
            .dag_repository
            .get_steps_with_jobs_by_dag(dag_id)
            .await?)
    }

    /// The running job's latest progress, as [`crate::JobQueue::cached_progress`] reports it.
    pub fn cached_job_progress(&self, job_id: &str) -> Option<JobProgressSnapshot> {
        self.job_queue.cached_progress(job_id)
    }

    /// Get a single DAG step execution by ID.
    pub async fn get_step_execution(
        &self,
        dag_step_execution_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        Ok(self.dag_repository.get_step(dag_step_execution_id).await?)
    }

    /// List DAG executions with optional status and session_id filters.
    pub async fn list_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        Ok(self
            .dag_repository
            .list_dags(status, session_id, limit, offset)
            .await?)
    }

    /// Count DAG executions with optional status and session_id filters.
    pub async fn count_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
    ) -> Result<u64> {
        Ok(self.dag_repository.count_dags(status, session_id).await?)
    }

    /// Count DAG executions matching `filter`.
    pub async fn count_dags_by_display_status(&self, filter: &DagListFilter<'_>) -> Result<u64> {
        Ok(self
            .dag_repository
            .count_dags_by_display_status(filter)
            .await?)
    }

    /// List display summaries without loading execution manifests.
    pub async fn list_dag_summaries(
        &self,
        filter: &DagListFilter<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>> {
        Ok(self
            .dag_repository
            .list_dag_summaries(filter, limit, offset)
            .await?)
    }

    /// Get a DAG execution's display status.
    pub async fn get_dag_display_status(&self, dag_id: &str) -> Result<DagExecutionStatus> {
        Ok(self.dag_repository.get_dag_display_status(dag_id).await?)
    }

    /// Get a DAG execution and its display status from one read.
    pub async fn get_dag_with_display_status(
        &self,
        dag_id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)> {
        Ok(self
            .dag_repository
            .get_dag_with_display_status(dag_id)
            .await?)
    }

    /// Count all DAG executions by display status.
    pub async fn get_dag_display_counts(&self) -> Result<srec_sqlite::models::DagDisplayCounts> {
        Ok(self.dag_repository.get_dag_display_counts().await?)
    }

    /// Get statistics for a DAG execution.
    pub async fn get_dag_stats(
        &self,
        dag_id: &str,
    ) -> Result<srec_sqlite::models::dag::DagExecutionStats> {
        Ok(self.dag_repository.get_dag_stats(dag_id).await?)
    }

    /// The running steps of the given DAGs, as `(dag_id, job_id)` pairs.
    pub async fn list_running_step_jobs(&self, dag_ids: &[&str]) -> Result<Vec<(String, String)>> {
        Ok(self.dag_repository.list_running_step_jobs(dag_ids).await?)
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
        Ok(self.dag_repository.delete_dag(dag_id).await?)
    }
}
