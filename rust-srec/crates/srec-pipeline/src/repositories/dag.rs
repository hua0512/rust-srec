//! DAG (Directed Acyclic Graph) repository for pipeline execution.

mod queries;
mod recovery;
#[cfg(test)]
mod tests;
mod transitions;
mod writes;

use async_trait::async_trait;
use sqlx::SqlitePool;

pub use self::queries::DagListFilter;
#[cfg(test)]
use self::queries::{DAG_AVG_DURATION_SQL, DAG_DISPLAY_COUNTS_SQL};
use self::transitions::CompletionGuard;
use srec_sqlite::DbResult as Result;
use srec_sqlite::begin_immediate;
use srec_sqlite::models::{
    DagDisplayCounts, DagExecutionDbModel, DagExecutionListSummary, DagExecutionStats,
    DagExecutionStatus, DagStepExecutionDbModel, DagStepWithJob, JobDbModel, ReadyStep,
};

/// The completion transaction's authoritative step and DAG snapshots, together
/// with newly ready dependents. Duplicate notifications return no ready steps.
#[derive(Debug, Clone)]
pub struct StepCompletion {
    pub step: DagStepExecutionDbModel,
    pub dag: DagExecutionDbModel,
    pub ready_steps: Vec<ReadyStep>,
}

/// DAG repository trait for pipeline execution management.
#[async_trait]
pub trait DagRepository: Send + Sync {
    // ========================================================================
    // DAG Execution CRUD
    // ========================================================================

    /// Create a new DAG execution record.
    async fn create_dag(&self, dag: &DagExecutionDbModel) -> Result<()>;

    /// Atomically publish a DAG, its steps, and the root jobs attached to those steps.
    async fn publish_dag(
        &self,
        dag: &DagExecutionDbModel,
        steps: &[DagStepExecutionDbModel],
        root_jobs: &[JobDbModel],
    ) -> Result<()>;

    /// Get a DAG execution by ID.
    async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel>;

    /// List DAG executions with optional status and session_id filters.
    async fn list_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>>;

    /// Count DAG executions with optional status and session_id filters.
    async fn count_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
    ) -> Result<u64>;

    /// List DAG summaries matching `filter`, newest first, each with its
    /// display status (see [`DagDisplayCounts`]).
    async fn list_dag_summaries(
        &self,
        filter: &DagListFilter<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>>;

    /// Count DAG executions matching `filter`.
    async fn count_dags_by_display_status(&self, filter: &DagListFilter<'_>) -> Result<u64>;

    /// Get a DAG execution's display status (see [`DagDisplayCounts`]).
    async fn get_dag_display_status(&self, id: &str) -> Result<DagExecutionStatus>;

    /// Get a DAG execution together with its display status, read in one query
    /// so the two describe the same moment.
    async fn get_dag_with_display_status(
        &self,
        id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)>;

    /// Count all DAG executions by display status.
    async fn get_dag_display_counts(&self) -> Result<DagDisplayCounts>;

    /// Delete a DAG execution and all its steps.
    async fn delete_dag(&self, id: &str) -> Result<()>;

    // ========================================================================
    // DAG Step Execution CRUD
    // ========================================================================

    /// Create a new step execution record.
    async fn create_step(&self, step: &DagStepExecutionDbModel) -> Result<()>;

    /// Create multiple step execution records in a transaction.
    async fn create_steps(&self, steps: &[DagStepExecutionDbModel]) -> Result<()>;

    /// Get a step execution by ID.
    async fn get_step(&self, id: &str) -> Result<DagStepExecutionDbModel>;

    /// Get a step execution by DAG ID and step ID.
    async fn get_step_by_dag_and_step_id(
        &self,
        dag_id: &str,
        step_id: &str,
    ) -> Result<DagStepExecutionDbModel>;

    /// Get all step executions for a DAG.
    async fn get_steps_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>>;

    /// Get all step executions for a DAG, each with the status and timing of
    /// its job, in creation order.
    async fn get_steps_with_jobs_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepWithJob>>;

    /// Atomically create a job and attach it to an unmaterialized DAG step.
    async fn create_job_for_step(&self, step_id: &str, job: &JobDbModel) -> Result<()>;

    // ========================================================================
    // Core DAG Operations (Atomic)
    // ========================================================================

    /// Atomically complete a step and check for ready dependents.
    /// Returns the transaction's updated records and newly ready dependents.
    async fn complete_step_and_check_dependents(
        &self,
        step_id: &str,
        outputs: &[String],
    ) -> Result<StepCompletion>;

    /// Atomically fail an active step and cancel every step that depends on it,
    /// directly or through other steps. Steps on independent branches keep
    /// running: the DAG stays PROCESSING with `error` set until no step is
    /// active, and is then finalized FAILED like any other settlement.
    /// Returns the job IDs of the cancelled dependents that had a job (for
    /// cancellation), or `None` when the step or the DAG was already terminal.
    async fn fail_step_and_cancel_dag(
        &self,
        step_id: &str,
        error: &str,
    ) -> Result<Option<Vec<String>>>;

    /// Atomically fail a whole DAG (a scheduler error, not a step result) and
    /// cancel every step that has not settled, running ones included.
    /// Returns job IDs of steps that had jobs created (for cancellation).
    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>>;

    /// Atomically cancel a DAG and every step that has not settled.
    /// Returns the job IDs of the cancelled PROCESSING steps, one per step.
    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>>;

    /// Reset a previously-failed DAG execution for retry.
    ///
    /// This prepares the DAG state so that when retried jobs complete, downstream steps
    /// can be scheduled again:
    /// - DAG: status -> PROCESSING, clear error/completed_at, failed_steps -> 0
    /// - Steps: FAILED -> PROCESSING; CANCELLED -> PROCESSING when the step has a
    ///   job, BLOCKED otherwise (job_id is preserved)
    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()>;

    // ========================================================================
    // Query Operations
    // ========================================================================

    /// Get statistics for a DAG execution.
    async fn get_dag_stats(&self, dag_id: &str) -> Result<DagExecutionStats>;

    /// The running steps of the given DAGs, as `(dag_id, job_id)` pairs: steps
    /// that are PROCESSING with a PROCESSING job. A step whose job is still
    /// queued or waits for an automatic retry is not running. Empty `dag_ids`
    /// return nothing without a query.
    async fn list_running_step_jobs(&self, dag_ids: &[&str]) -> Result<Vec<(String, String)>>;

    /// Ready steps of a DAG that have no job: BLOCKED or PENDING rows whose
    /// dependencies have all COMPLETED, with their inputs merged from those
    /// dependencies' outputs in dependency order. Root steps are not listed;
    /// publication attaches their jobs. This is the readiness
    /// [`Self::complete_step_and_check_dependents`] computes for the completed
    /// step's dependents, asked for the whole DAG: retry fan-out and startup
    /// recovery materialize these steps because no further completion will.
    async fn list_ready_steps(&self, dag_id: &str) -> Result<Vec<ReadyStep>>;

    /// List active step executions whose attached job is durably completed.
    async fn list_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>>;

    /// List non-terminal DAGs containing a ready step whose job has not been
    /// materialized: one with no job whose dependencies have all COMPLETED. A
    /// step still waiting on a running dependency needs no recovery and is not
    /// a candidate.
    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>>;

    /// List PROCESSING steps of non-terminal DAGs whose attached job is FAILED or
    /// CANCELLED, or whose job row is gone (`job_id` NULL): the worker's failure
    /// report never reached the scheduler, so nothing will advance or fail the DAG
    /// unless startup reconciliation does. A FAILED job with a scheduled
    /// automatic retry is waiting for the retry sweeper and is not listed.
    async fn list_processing_steps_with_failed_jobs(&self) -> Result<Vec<DagStepExecutionDbModel>>;

    /// Complete a ready step that has no job, recording `outputs`, and settle its
    /// dependents and DAG exactly like [`Self::complete_step_and_check_dependents`].
    ///
    /// Used for a step whose dependencies produced no outputs, which runs as a no-op
    /// instead of a job. The transition applies only while the step is BLOCKED or
    /// PENDING with no job and the DAG is not terminal; otherwise the current rows are
    /// returned with no ready steps, as for a duplicate completion.
    async fn complete_ready_step_without_job(
        &self,
        step_id: &str,
        outputs: &[String],
    ) -> Result<StepCompletion>;
}

/// SQLx implementation of DagRepository.
pub struct SqlxDagRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxDagRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl DagRepository for SqlxDagRepository {
    async fn create_dag(&self, dag: &DagExecutionDbModel) -> Result<()> {
        let mut connection = self.write_pool.acquire().await?;
        writes::insert_dag(&mut connection, dag).await?;
        Ok(())
    }

    async fn publish_dag(
        &self,
        dag: &DagExecutionDbModel,
        steps: &[DagStepExecutionDbModel],
        root_jobs: &[JobDbModel],
    ) -> Result<()> {
        self.publish_dag_tx(dag, steps, root_jobs).await
    }

    async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
        self.query_dag(id).await
    }

    async fn list_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        self.query_list_dags(status, session_id, limit, offset)
            .await
    }

    async fn count_dags(
        &self,
        status: Option<DagExecutionStatus>,
        session_id: Option<&str>,
    ) -> Result<u64> {
        self.query_count_dags(status, session_id).await
    }

    async fn list_dag_summaries(
        &self,
        filter: &DagListFilter<'_>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionListSummary>> {
        self.query_list_dag_summaries(filter, limit, offset).await
    }

    async fn count_dags_by_display_status(&self, filter: &DagListFilter<'_>) -> Result<u64> {
        self.query_count_dags_by_display_status(filter).await
    }

    async fn get_dag_display_status(&self, id: &str) -> Result<DagExecutionStatus> {
        self.query_dag_display_status(id).await
    }

    async fn get_dag_with_display_status(
        &self,
        id: &str,
    ) -> Result<(DagExecutionDbModel, DagExecutionStatus)> {
        self.query_dag_with_display_status(id).await
    }

    async fn get_dag_display_counts(&self) -> Result<DagDisplayCounts> {
        self.query_dag_display_counts().await
    }

    async fn delete_dag(&self, id: &str) -> Result<()> {
        // CASCADE will delete associated steps
        sqlx::query("DELETE FROM dag_execution WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn create_step(&self, step: &DagStepExecutionDbModel) -> Result<()> {
        let mut connection = self.write_pool.acquire().await?;
        writes::insert_step(&mut connection, step, writes::InsertState::Stored).await?;
        Ok(())
    }

    async fn create_steps(&self, steps: &[DagStepExecutionDbModel]) -> Result<()> {
        if steps.is_empty() {
            return Ok(());
        }

        let mut tx = begin_immediate(&self.write_pool).await?;

        for step in steps {
            writes::insert_step(&mut tx, step, writes::InsertState::Stored).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_step(&self, id: &str) -> Result<DagStepExecutionDbModel> {
        self.query_step(id).await
    }

    async fn get_step_by_dag_and_step_id(
        &self,
        dag_id: &str,
        step_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        self.query_step_by_dag_and_step_id(dag_id, step_id).await
    }

    async fn get_steps_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        self.query_steps_by_dag(dag_id).await
    }

    async fn get_steps_with_jobs_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepWithJob>> {
        self.query_steps_with_jobs_by_dag(dag_id).await
    }

    async fn create_job_for_step(&self, step_id: &str, job: &JobDbModel) -> Result<()> {
        self.create_job_for_step_tx(step_id, job).await
    }

    async fn complete_step_and_check_dependents(
        &self,
        step_id: &str,
        outputs: &[String],
    ) -> Result<StepCompletion> {
        self.complete_step_with_guard(step_id, outputs, CompletionGuard::AttachedJob)
            .await
    }

    async fn complete_ready_step_without_job(
        &self,
        step_id: &str,
        outputs: &[String],
    ) -> Result<StepCompletion> {
        self.complete_step_with_guard(step_id, outputs, CompletionGuard::NoJob)
            .await
    }

    async fn fail_step_and_cancel_dag(
        &self,
        step_id: &str,
        error: &str,
    ) -> Result<Option<Vec<String>>> {
        self.fail_step_and_cancel_dag_tx(step_id, error).await
    }

    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        self.terminate_dag(
            dag_id,
            error,
            DagExecutionStatus::Failed,
            "fail_dag_and_cancel_steps",
        )
        .await
    }

    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        self.terminate_dag(
            dag_id,
            error,
            DagExecutionStatus::Cancelled,
            "cancel_dag_and_cancel_steps",
        )
        .await
    }

    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
        self.reset_dag_for_retry_tx(dag_id).await
    }

    async fn get_dag_stats(&self, dag_id: &str) -> Result<DagExecutionStats> {
        self.query_dag_stats(dag_id).await
    }

    async fn list_running_step_jobs(&self, dag_ids: &[&str]) -> Result<Vec<(String, String)>> {
        self.query_running_step_jobs(dag_ids).await
    }

    async fn list_ready_steps(&self, dag_id: &str) -> Result<Vec<ReadyStep>> {
        self.query_ready_steps(dag_id).await
    }

    async fn list_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        self.query_processing_steps_with_completed_jobs().await
    }

    async fn list_processing_steps_with_failed_jobs(&self) -> Result<Vec<DagStepExecutionDbModel>> {
        self.query_processing_steps_with_failed_jobs().await
    }

    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>> {
        self.query_dag_ids_with_unmaterialized_steps().await
    }
}
