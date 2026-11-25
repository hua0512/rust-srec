//! Startup reconciliation and retry readiness queries for DAG executions.

use super::SqlxDagRepository;
use super::transitions::ready_steps;
use srec_sqlite::DbResult as Result;
use srec_sqlite::models::{DagStepExecutionDbModel, DagStepStatus, ReadyStep};

impl SqlxDagRepository {
    pub(super) async fn query_ready_steps(&self, dag_id: &str) -> Result<Vec<ReadyStep>> {
        let rows = self.query_steps_by_dag(dag_id).await?;
        Ok(ready_steps(&rows, |row, depends_on| {
            row.job_id.is_none()
                && matches!(
                    row.get_status(),
                    Some(DagStepStatus::Blocked | DagStepStatus::Pending)
                )
                && !depends_on.is_empty()
        }))
    }

    pub(super) async fn query_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        // `job_id` is projected from the joined job, not from `dag_step_execution.job_id`: a step
        // whose attachment write was lost still reaches its job through the job's own
        // `dag_step_execution_id`. Callers may read that field but must not write these rows back,
        // since the column itself is still NULL for a reverse-linked step.
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            r#"
            SELECT step.id,
                   step.dag_id,
                   step.step_id,
                   job.id AS job_id,
                   step.status,
                   step.depends_on_step_ids,
                   step.outputs,
                   step.created_at,
                   step.updated_at
            FROM dag_step_execution AS step
            JOIN dag_execution AS dag ON dag.id = step.dag_id
            JOIN job ON job.id = COALESCE(
                step.job_id,
                (
                    SELECT orphan.id
                    FROM job AS orphan
                    WHERE orphan.dag_step_execution_id = step.id
                      AND orphan.status = 'COMPLETED'
                    ORDER BY orphan.created_at, orphan.id
                    LIMIT 1
                )
            )
            WHERE step.status IN ('PENDING', 'PROCESSING')
              AND job.status = 'COMPLETED'
              AND dag.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
            ORDER BY step.created_at, step.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    pub(super) async fn query_processing_steps_with_failed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>> {
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            r#"
            SELECT step.*
            FROM dag_step_execution AS step
            JOIN dag_execution AS dag ON dag.id = step.dag_id
            LEFT JOIN job ON job.id = step.job_id
            WHERE step.status = 'PROCESSING'
              AND (
                step.job_id IS NULL
                OR job.status = 'CANCELLED'
                OR (job.status = 'FAILED' AND job.retry_after IS NULL)
              )
              AND dag.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
            ORDER BY step.created_at, step.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    pub(super) async fn query_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>> {
        let dag_ids = sqlx::query_scalar(
            r#"
            SELECT DISTINCT dag.id
            FROM dag_execution AS dag
            JOIN dag_step_execution AS step ON step.dag_id = dag.id
            WHERE dag.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
              AND step.status IN ('PENDING', 'BLOCKED')
              AND step.job_id IS NULL
              AND NOT EXISTS (
                SELECT 1
                FROM job
                WHERE job.dag_step_execution_id = step.id
                  AND job.status IN ('PENDING', 'PROCESSING', 'COMPLETED')
              )
              AND NOT EXISTS (
                SELECT 1
                FROM json_each(step.depends_on_step_ids) AS dependency
                WHERE NOT EXISTS (
                  SELECT 1
                  FROM dag_step_execution AS upstream
                  WHERE upstream.dag_id = step.dag_id
                    AND upstream.step_id = dependency.value
                    AND upstream.status = 'COMPLETED'
                )
              )
            ORDER BY dag.created_at, dag.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(dag_ids)
    }
}
