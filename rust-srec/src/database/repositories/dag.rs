//! DAG (Directed Acyclic Graph) repository for pipeline execution.

mod writes;

use async_trait::async_trait;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

use crate::database::begin_immediate;
use crate::database::models::{
    DagExecutionDbModel, DagExecutionStats, DagStepExecutionDbModel, DagStepStatus, JobDbModel,
    ReadyStep,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};

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

    /// Update DAG execution status.
    async fn update_dag_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<()>;

    /// Increment completed steps counter for a DAG.
    async fn increment_dag_completed(&self, dag_id: &str) -> Result<()>;

    /// Increment failed steps counter for a DAG.
    async fn increment_dag_failed(&self, dag_id: &str) -> Result<()>;

    /// List DAG executions with optional status and session_id filters.
    async fn list_dags(
        &self,
        status: Option<&str>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>>;

    /// Count DAG executions with optional status and session_id filters.
    async fn count_dags(&self, status: Option<&str>, session_id: Option<&str>) -> Result<u64>;

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

    /// Update a step execution.
    async fn update_step(&self, step: &DagStepExecutionDbModel) -> Result<()>;

    /// Update step status.
    async fn update_step_status(&self, id: &str, status: &str) -> Result<()>;

    /// Update step status and job ID.
    async fn update_step_status_with_job(&self, id: &str, status: &str, job_id: &str)
    -> Result<()>;

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

    /// Atomically fail an active step and its non-terminal DAG.
    /// Returns `None` when either record was already terminal.
    async fn fail_step_and_cancel_dag(
        &self,
        step_id: &str,
        error: &str,
    ) -> Result<Option<Vec<String>>>;

    /// Atomically fail a DAG and cancel all pending/blocked steps.
    /// Returns job IDs of steps that had jobs created (for cancellation).
    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>>;

    /// Atomically cancel a DAG and cancel all pending/blocked steps.
    /// Returns job IDs of steps that had jobs created (for cancellation).
    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>>;

    /// Reset a previously-failed DAG execution for retry.
    ///
    /// This prepares the DAG state so that when retried jobs complete, downstream steps
    /// can be scheduled again:
    /// - DAG: status -> PROCESSING, clear error/completed_at, recompute failed_steps
    /// - Steps: CANCELLED -> BLOCKED, FAILED -> PROCESSING (preserves job_id)
    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()>;

    // ========================================================================
    // Query Operations
    // ========================================================================

    /// Get concatenated outputs from all specified dependency steps.
    async fn get_dependency_outputs(
        &self,
        dag_id: &str,
        step_ids: &[String],
    ) -> Result<Vec<String>>;

    /// Check if all dependencies for a step are complete.
    async fn check_all_dependencies_complete(&self, dag_id: &str, step_id: &str) -> Result<bool>;

    /// Get statistics for a DAG execution.
    async fn get_dag_stats(&self, dag_id: &str) -> Result<DagExecutionStats>;

    /// Get job IDs for all processing steps in a DAG.
    async fn get_processing_job_ids(&self, dag_id: &str) -> Result<Vec<String>>;

    /// Get pending root steps for a DAG (for initial job creation).
    async fn get_pending_root_steps(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>>;

    /// List active step executions whose attached job is durably completed.
    async fn list_processing_steps_with_completed_jobs(
        &self,
    ) -> Result<Vec<DagStepExecutionDbModel>>;

    /// List non-terminal DAGs containing a step whose job has not been materialized.
    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>>;

    /// List PROCESSING steps of non-terminal DAGs whose attached job is FAILED or
    /// CANCELLED, or whose job row is gone (`job_id` NULL): the worker's failure
    /// report never reached the scheduler, so nothing will advance or fail the DAG
    /// unless startup reconciliation does.
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

/// Which row state a step completion may transition from.
#[derive(Clone, Copy)]
enum CompletionGuard {
    /// A step whose job finished: PENDING or PROCESSING.
    AttachedJob,
    /// A ready step completed without a job: BLOCKED or PENDING with `job_id` NULL
    /// in a non-terminal DAG.
    NoJob,
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

    async fn complete_step_with_guard(
        &self,
        step_id: &str,
        outputs: &[String],
        guard: CompletionGuard,
    ) -> Result<StepCompletion> {
        let label = match guard {
            CompletionGuard::AttachedJob => "complete_step_and_check_dependents",
            CompletionGuard::NoJob => "complete_ready_step_without_job",
        };
        retry_on_sqlite_busy(label, || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();
            let outputs_json = serde_json::to_string(outputs)?;

            fn output_dedup_key(output: &str) -> String {
                if cfg!(windows) {
                    output.to_lowercase()
                } else {
                    output.to_string()
                }
            }

            fn merge_dependency_outputs(
                depends_on_step_ids: &[String],
                completed_outputs_by_step_id: &HashMap<String, Vec<String>>,
            ) -> Vec<String> {
                let mut merged = Vec::new();
                let mut seen = HashSet::<String>::new();

                for dep in depends_on_step_ids {
                    let Some(dep_outputs) = completed_outputs_by_step_id.get(dep) else {
                        continue;
                    };
                    for out in dep_outputs {
                        if seen.insert(output_dedup_key(out)) {
                            merged.push(out.clone());
                        }
                    }
                }

                merged
            }

            // The conditional transition fences duplicate notifications and cancelled steps.
            let transition = match guard {
                CompletionGuard::AttachedJob => {
                    r#"
                    UPDATE dag_step_execution
                    SET status = 'COMPLETED', outputs = ?, updated_at = ?
                    WHERE id = ?
                      AND status IN ('PENDING', 'PROCESSING')
                    RETURNING *
                    "#
                }
                CompletionGuard::NoJob => {
                    r#"
                    UPDATE dag_step_execution
                    SET status = 'COMPLETED', outputs = ?, updated_at = ?
                    WHERE id = ?
                      AND status IN ('BLOCKED', 'PENDING')
                      AND job_id IS NULL
                      AND (
                        SELECT status
                        FROM dag_execution
                        WHERE id = dag_step_execution.dag_id
                      ) NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                    RETURNING *
                    "#
                }
            };
            let completed = sqlx::query_as::<_, DagStepExecutionDbModel>(transition)
                .bind(&outputs_json)
                .bind(now)
                .bind(step_id)
                .fetch_optional(&mut *tx)
                .await?;

            let Some(completed_step) = completed else {
                // A duplicate still supplies terminal state for notification replay,
                // without incrementing counters or scheduling dependents again.
                let step = sqlx::query_as::<_, DagStepExecutionDbModel>("SELECT * FROM dag_step_execution WHERE id = ?")
                    .bind(step_id).fetch_optional(&mut *tx).await?
                    .ok_or_else(|| Error::not_found("DAG step execution", step_id))?;
                let dag = sqlx::query_as::<_, DagExecutionDbModel>("SELECT * FROM dag_execution WHERE id = ?")
                    .bind(&step.dag_id).fetch_one(&mut *tx).await?;
                tx.commit().await?;
                return Ok(StepCompletion { step, dag, ready_steps: Vec::new() });
            };

            // Keep the DAG snapshot from its counter update within this transaction.
            let mut dag = sqlx::query_as::<_, DagExecutionDbModel>(
                "UPDATE dag_execution SET completed_steps = completed_steps + 1, updated_at = ? WHERE id = ? RETURNING *",
            )
            .bind(now)
            .bind(&completed_step.dag_id)
            .fetch_one(&mut *tx)
            .await?;

            // Dependency IDs are JSON arrays; inspect only blocked direct dependents.
            let blocked_dependents = sqlx::query_as::<_, DagStepExecutionDbModel>(
                r#"
                SELECT dse.* FROM dag_step_execution dse
                WHERE dse.dag_id = ?
                  AND dse.status = 'BLOCKED'
                  AND EXISTS (
                      SELECT 1 FROM json_each(dse.depends_on_step_ids)
                      WHERE json_each.value = ?
                  )
                "#,
            )
            .bind(&completed_step.dag_id)
            .bind(&completed_step.step_id)
                .fetch_all(&mut *tx)
                .await?;

            // Read statuses and outputs once for all fan-in readiness decisions,
            // under the same write transaction as the completed-step transition.
            let step_rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
                r#"
                SELECT step_id, status, outputs
                FROM dag_step_execution
                WHERE dag_id = ?
                "#,
            )
            .bind(&completed_step.dag_id)
            .fetch_all(&mut *tx)
            .await?;

            let mut status_by_step_id: HashMap<String, String> =
                HashMap::with_capacity(step_rows.len());
            let mut completed_outputs_by_step_id: HashMap<String, Vec<String>> =
                HashMap::with_capacity(step_rows.len());

            for (step_id, status, outputs) in step_rows {
                status_by_step_id.insert(step_id.clone(), status.clone());
                if status == "COMPLETED" {
                    let parsed = outputs
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<Vec<String>>(raw).ok())
                        .unwrap_or_default();
                    completed_outputs_by_step_id.insert(step_id, parsed);
                }
            }

            let mut ready_steps = Vec::new();

            for dependent in blocked_dependents {
                let depends_on: Vec<String> =
                    serde_json::from_str(&dependent.depends_on_step_ids).unwrap_or_default();

                let all_deps_complete = depends_on.iter().all(|dep| {
                    status_by_step_id
                        .get(dep)
                        .map(|s| s == "COMPLETED")
                        .unwrap_or(false)
                });

                if all_deps_complete {
                    // All dependencies complete - mark as PENDING
                    sqlx::query(
                        "UPDATE dag_step_execution SET status = 'PENDING', updated_at = ? WHERE id = ?",
                    )
                    .bind(now)
                    .bind(&dependent.id)
                    .execute(&mut *tx)
                    .await?;

                    // Collect merged inputs from all dependencies (fan-in), respecting dependency order.
                    let merged_inputs = merge_dependency_outputs(
                        &depends_on,
                        &completed_outputs_by_step_id,
                    );

                    // Update the step record with PENDING status
                    let mut updated_step = dependent.clone();
                    updated_step.status = DagStepStatus::Pending.as_str().to_string();

                    ready_steps.push(ReadyStep {
                        step: updated_step,
                        merged_inputs,
                    });
                }
            }

            // Settle the last step and parent DAG together before releasing the transaction.
            if dag.completed_steps + dag.failed_steps >= dag.total_steps {
                // DAG is complete
                let final_status = if dag.failed_steps > 0 {
                    "FAILED"
                } else {
                    "COMPLETED"
                };
                dag = sqlx::query_as::<_, DagExecutionDbModel>(
                    "UPDATE dag_execution SET status = ?, completed_at = ?, updated_at = ? WHERE id = ? RETURNING *",
                )
                .bind(final_status)
                .bind(now)
                .bind(now)
                .bind(&completed_step.dag_id)
                .fetch_one(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok(StepCompletion { step: completed_step, dag, ready_steps })
        })
        .await
    }
}

#[async_trait]
impl DagRepository for SqlxDagRepository {
    // ========================================================================
    // DAG Execution CRUD
    // ========================================================================

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
        retry_on_sqlite_busy("publish_dag", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            writes::insert_dag(&mut tx, dag).await?;

            for step in steps {
                let published_status = if step.job_id.is_some() {
                    DagStepStatus::Pending.as_str()
                } else {
                    step.status.as_str()
                };
                writes::insert_step(
                    &mut tx,
                    step,
                    writes::InsertState::Unattached {
                        status: published_status,
                    },
                )
                .await?;
            }

            for job in root_jobs {
                super::job::writes::insert_job(&mut tx, job).await?;

                let Some(step_id) = job.dag_step_execution_id.as_deref() else {
                    return Err(Error::Validation(format!(
                        "Root DAG job {} has no step execution ID",
                        job.id
                    )));
                };
                let attached = sqlx::query(
                    r#"
                    UPDATE dag_step_execution
                    SET status = 'PROCESSING', job_id = ?, updated_at = ?
                    WHERE id = ? AND dag_id = ? AND job_id IS NULL AND status = 'PENDING'
                    "#,
                )
                .bind(&job.id)
                .bind(dag.updated_at)
                .bind(step_id)
                .bind(&dag.id)
                .execute(&mut *tx)
                .await?;
                if attached.rows_affected() != 1 {
                    return Err(Error::InvalidStateTransition {
                        from: format!("unpublished root step {}", step_id),
                        to: format!("PROCESSING(job_id={})", job.id),
                    });
                }
            }

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn get_dag(&self, id: &str) -> Result<DagExecutionDbModel> {
        sqlx::query_as::<_, DagExecutionDbModel>("SELECT * FROM dag_execution WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("DAG execution", id))
    }

    async fn update_dag_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<()> {
        retry_on_sqlite_busy("update_dag_status", || async {
            let now = crate::database::time::now_ms();
            let completed_at = if status == "COMPLETED" || status == "FAILED" {
                Some(now)
            } else {
                None
            };

            // A terminal DAG is final: neither a late `PROCESSING` write from
            // `create_dag_pipeline_with_hook` nor a second terminal verdict may overwrite the
            // `completed_at` and `error` recorded by whichever transition got there first.
            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = ?, updated_at = ?, completed_at = COALESCE(?, completed_at), error = ?
                WHERE id = ?
                  AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(status)
            .bind(now)
            .bind(completed_at)
            .bind(error)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn increment_dag_completed(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("increment_dag_completed", || async {
            let now = crate::database::time::now_ms();
            sqlx::query(
                "UPDATE dag_execution SET completed_steps = completed_steps + 1, updated_at = ? WHERE id = ?",
            )
            .bind(now)
            .bind(dag_id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn increment_dag_failed(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("increment_dag_failed", || async {
            let now = crate::database::time::now_ms();
            sqlx::query(
                "UPDATE dag_execution SET failed_steps = failed_steps + 1, updated_at = ? WHERE id = ?",
            )
            .bind(now)
            .bind(dag_id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn list_dags(
        &self,
        status: Option<&str>,
        session_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<DagExecutionDbModel>> {
        // Build dynamic WHERE clause
        let mut conditions: Vec<String> = Vec::new();
        if status.is_some() {
            conditions.push("status = ?".to_string());
        }
        if session_id.is_some() {
            conditions.push("session_id = ?".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT * FROM dag_execution {} ORDER BY created_at DESC LIMIT ? OFFSET ?",
            where_clause
        );

        let mut query = sqlx::query_as::<_, DagExecutionDbModel>(sqlx::AssertSqlSafe(sql));
        if let Some(status) = status {
            query = query.bind(status);
        }
        if let Some(session_id) = session_id {
            query = query.bind(session_id);
        }
        query = query.bind(limit).bind(offset);

        let dags = query.fetch_all(&self.pool).await?;
        Ok(dags)
    }

    async fn count_dags(&self, status: Option<&str>, session_id: Option<&str>) -> Result<u64> {
        // Build dynamic WHERE clause
        let mut conditions: Vec<String> = Vec::new();
        if status.is_some() {
            conditions.push("status = ?".to_string());
        }
        if session_id.is_some() {
            conditions.push("session_id = ?".to_string());
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let sql = format!("SELECT COUNT(*) FROM dag_execution {}", where_clause);

        let mut query = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql));
        if let Some(status) = status {
            query = query.bind(status);
        }
        if let Some(session_id) = session_id {
            query = query.bind(session_id);
        }

        let count = query.fetch_one(&self.pool).await?;
        Ok(count as u64)
    }

    async fn delete_dag(&self, id: &str) -> Result<()> {
        // CASCADE will delete associated steps
        sqlx::query("DELETE FROM dag_execution WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    // ========================================================================
    // DAG Step Execution CRUD
    // ========================================================================

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
        sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("DAG step execution", id))
    }

    async fn get_step_by_dag_and_step_id(
        &self,
        dag_id: &str,
        step_id: &str,
    ) -> Result<DagStepExecutionDbModel> {
        sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE dag_id = ? AND step_id = ?",
        )
        .bind(dag_id)
        .bind(step_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("DAG step", format!("{}/{}", dag_id, step_id)))
    }

    async fn get_steps_by_dag(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            "SELECT * FROM dag_step_execution WHERE dag_id = ? ORDER BY created_at",
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    async fn update_step(&self, step: &DagStepExecutionDbModel) -> Result<()> {
        retry_on_sqlite_busy("update_step", || async {
            let now = crate::database::time::now_ms();
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET job_id = ?, status = ?, outputs = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&step.job_id)
            .bind(&step.status)
            .bind(&step.outputs)
            .bind(now)
            .bind(&step.id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn update_step_status(&self, id: &str, status: &str) -> Result<()> {
        retry_on_sqlite_busy("update_step_status", || async {
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE dag_step_execution SET status = ?, updated_at = ? WHERE id = ?")
                .bind(status)
                .bind(now)
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn update_step_status_with_job(
        &self,
        id: &str,
        status: &str,
        job_id: &str,
    ) -> Result<()> {
        retry_on_sqlite_busy("update_step_status_with_job", || async {
            let now = crate::database::time::now_ms();
            let updated = sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = ?, job_id = ?, updated_at = ?
                WHERE id = ?
                  AND job_id IS NULL
                  AND status IN ('PENDING', 'BLOCKED')
                  AND (
                    SELECT status
                    FROM dag_execution
                    WHERE id = dag_step_execution.dag_id
                  ) NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(status)
            .bind(job_id)
            .bind(now)
            .bind(id)
            .execute(&self.write_pool)
            .await?;

            if updated.rows_affected() == 0 {
                #[derive(sqlx::FromRow)]
                struct StepStatusRow {
                    status: String,
                    job_id: Option<String>,
                    dag_id: String,
                }

                let step = sqlx::query_as::<_, StepStatusRow>(
                    "SELECT status, job_id, dag_id FROM dag_step_execution WHERE id = ?",
                )
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;

                let Some(step) = step else {
                    return Err(Error::not_found("DAG step execution", id));
                };

                let dag_status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM dag_execution WHERE id = ?")
                        .bind(&step.dag_id)
                        .fetch_optional(&self.pool)
                        .await?;

                return Err(Error::InvalidStateTransition {
                    from: format!(
                        "step_status={}, step_job_id={}, dag_status={}",
                        step.status,
                        step.job_id.as_deref().unwrap_or("NULL"),
                        dag_status.as_deref().unwrap_or("UNKNOWN")
                    ),
                    to: format!("{}(job_id={})", status, job_id),
                });
            }
            Ok(())
        })
        .await
    }

    async fn create_job_for_step(&self, step_id: &str, job: &JobDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_job_for_step", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            super::job::writes::insert_job(&mut tx, job).await?;

            let now = crate::database::time::now_ms();
            let attached = sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'PROCESSING', job_id = ?, updated_at = ?
                WHERE id = ?
                  AND job_id IS NULL
                  AND status IN ('PENDING', 'BLOCKED')
                  AND (
                    SELECT status
                    FROM dag_execution
                    WHERE id = dag_step_execution.dag_id
                  ) NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(&job.id)
            .bind(now)
            .bind(step_id)
            .execute(&mut *tx)
            .await?;

            if attached.rows_affected() != 1 {
                return Err(Error::InvalidStateTransition {
                    from: format!("unmaterialized active step {}", step_id),
                    to: format!("PROCESSING(job_id={})", job.id),
                });
            }

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    // ========================================================================
    // Core DAG Operations (Atomic)
    // ========================================================================

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
        retry_on_sqlite_busy("fail_step_and_cancel_dag", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();

            let step = sqlx::query_as::<_, DagStepExecutionDbModel>(
                "SELECT * FROM dag_step_execution WHERE id = ?",
            )
            .bind(step_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| Error::not_found("DAG step execution", step_id))?;

            let failed = sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'FAILED', updated_at = ?
                WHERE id = ?
                  AND status IN ('PENDING', 'PROCESSING')
                  AND EXISTS (
                    SELECT 1
                    FROM dag_execution
                    WHERE id = dag_step_execution.dag_id
                      AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                  )
                "#,
            )
            .bind(now)
            .bind(step_id)
            .execute(&mut *tx)
            .await?;

            if failed.rows_affected() == 0 {
                tx.commit().await?;
                return Ok(None);
            }

            let processing_job_ids: Vec<String> = sqlx::query_scalar(
                r#"
                SELECT job_id FROM dag_step_execution
                WHERE dag_id = ? AND status = 'PROCESSING' AND job_id IS NOT NULL
                "#,
            )
            .bind(&step.dag_id)
            .fetch_all(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'CANCELLED', updated_at = ?
                WHERE dag_id = ? AND status IN ('BLOCKED', 'PENDING', 'PROCESSING')
                "#,
            )
            .bind(now)
            .bind(&step.dag_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = 'FAILED',
                    failed_steps = failed_steps + 1,
                    completed_at = ?,
                    updated_at = ?,
                    error = ?
                WHERE id = ? AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(now)
            .bind(now)
            .bind(error)
            .bind(&step.dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(Some(processing_job_ids))
        })
        .await
    }

    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        retry_on_sqlite_busy("fail_dag_and_cancel_steps", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();

            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM dag_execution WHERE id = ?")
                    .bind(dag_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if status
                .as_deref()
                .is_some_and(|status| matches!(status, "COMPLETED" | "FAILED" | "CANCELLED"))
            {
                tx.commit().await?;
                return Ok(Vec::new());
            }

            // 1. Get job IDs of processing steps (for cancellation)
            let processing_job_ids: Vec<String> = sqlx::query_scalar(
                r#"
                SELECT job_id FROM dag_step_execution
                WHERE dag_id = ? AND status = 'PROCESSING' AND job_id IS NOT NULL
                "#,
            )
            .bind(dag_id)
            .fetch_all(&mut *tx)
            .await?;

            // 2. Cancel all BLOCKED and PENDING steps
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'CANCELLED', updated_at = ?
                WHERE dag_id = ? AND status IN ('BLOCKED', 'PENDING')
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            // 2b. Mark in-flight steps as CANCELLED too (fail-fast). We keep the job_id for
            // observability and for best-effort cancellation by the caller.
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'CANCELLED', updated_at = ?
                WHERE dag_id = ? AND status = 'PROCESSING'
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            // 3. Mark the DAG as FAILED
            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = 'FAILED', completed_at = ?, updated_at = ?, error = ?
                WHERE id = ? AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(now)
            .bind(now)
            .bind(error)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(processing_job_ids)
        })
        .await
    }

    async fn cancel_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        retry_on_sqlite_busy("cancel_dag_and_cancel_steps", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();

            let status: Option<String> =
                sqlx::query_scalar("SELECT status FROM dag_execution WHERE id = ?")
                    .bind(dag_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if status
                .as_deref()
                .is_some_and(|status| matches!(status, "COMPLETED" | "FAILED" | "CANCELLED"))
            {
                tx.commit().await?;
                return Ok(Vec::new());
            }

            let processing_job_ids: Vec<String> = sqlx::query_scalar(
                r#"
                SELECT job_id FROM dag_step_execution
                WHERE dag_id = ? AND status = 'PROCESSING' AND job_id IS NOT NULL
                "#,
            )
            .bind(dag_id)
            .fetch_all(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'CANCELLED', updated_at = ?
                WHERE dag_id = ? AND status IN ('BLOCKED', 'PENDING')
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'CANCELLED', updated_at = ?
                WHERE dag_id = ? AND status = 'PROCESSING'
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = 'CANCELLED', completed_at = ?, updated_at = ?, error = ?
                WHERE id = ? AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(now)
            .bind(now)
            .bind(error)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(processing_job_ids)
        })
        .await
    }

    async fn reset_dag_for_retry(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("reset_dag_for_retry", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();

            // Un-cancel downstream work so it can be scheduled again.
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = CASE WHEN job_id IS NULL THEN 'BLOCKED' ELSE 'PROCESSING' END,
                    outputs = NULL,
                    updated_at = ?
                WHERE dag_id = ? AND status = 'CANCELLED'
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            // Mark failed steps as active again (their jobs will be retried separately).
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'PROCESSING', outputs = NULL, updated_at = ?
                WHERE dag_id = ? AND status = 'FAILED'
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            // Reset DAG execution record back to non-terminal state.
            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = 'PROCESSING',
                    completed_at = NULL,
                    updated_at = ?,
                    error = NULL,
                    failed_steps = (
                        SELECT COUNT(*)
                        FROM dag_step_execution
                        WHERE dag_id = ? AND status = 'FAILED'
                    )
                WHERE id = ?
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    // ========================================================================
    // Query Operations
    // ========================================================================

    async fn get_dependency_outputs(
        &self,
        dag_id: &str,
        step_ids: &[String],
    ) -> Result<Vec<String>> {
        if step_ids.is_empty() {
            return Ok(Vec::new());
        }

        let step_ids_json = serde_json::to_string(step_ids)?;

        let outputs_rows: Vec<Option<String>> = sqlx::query_scalar(
            r#"
            SELECT outputs FROM dag_step_execution
            WHERE dag_id = ?
              AND step_id IN (SELECT value FROM json_each(?))
              AND status = 'COMPLETED'
            "#,
        )
        .bind(dag_id)
        .bind(&step_ids_json)
        .fetch_all(&self.pool)
        .await?;

        let merged: Vec<String> = outputs_rows
            .into_iter()
            .flatten()
            .flat_map(|s| serde_json::from_str::<Vec<String>>(&s).unwrap_or_default())
            .collect();

        Ok(merged)
    }

    async fn check_all_dependencies_complete(&self, dag_id: &str, step_id: &str) -> Result<bool> {
        // Get the step's dependencies
        let step = self.get_step_by_dag_and_step_id(dag_id, step_id).await?;

        let incomplete_count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM dag_step_execution
            WHERE dag_id = ?
              AND step_id IN (SELECT value FROM json_each(?))
              AND status != 'COMPLETED'
            "#,
        )
        .bind(dag_id)
        .bind(&step.depends_on_step_ids)
        .fetch_one(&self.pool)
        .await?;

        Ok(incomplete_count == 0)
    }

    async fn get_dag_stats(&self, dag_id: &str) -> Result<DagExecutionStats> {
        #[derive(sqlx::FromRow)]
        struct StatusCount {
            status: String,
            count: i64,
        }

        let counts: Vec<StatusCount> = sqlx::query_as(
            r#"
            SELECT status, COUNT(*) as count
            FROM dag_step_execution
            WHERE dag_id = ?
            GROUP BY status
            "#,
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;

        let mut stats = DagExecutionStats::default();
        for StatusCount { status, count } in counts {
            match status.as_str() {
                "BLOCKED" => stats.blocked = count as u64,
                "PENDING" => stats.pending = count as u64,
                "PROCESSING" => stats.processing = count as u64,
                "COMPLETED" => stats.completed = count as u64,
                "FAILED" => stats.failed = count as u64,
                "CANCELLED" => stats.cancelled = count as u64,
                _ => {}
            }
        }

        Ok(stats)
    }

    async fn get_processing_job_ids(&self, dag_id: &str) -> Result<Vec<String>> {
        let job_ids: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT job_id FROM dag_step_execution
            WHERE dag_id = ? AND status = 'PROCESSING' AND job_id IS NOT NULL
            "#,
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(job_ids)
    }

    async fn get_pending_root_steps(&self, dag_id: &str) -> Result<Vec<DagStepExecutionDbModel>> {
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            r#"
            SELECT * FROM dag_step_execution
            WHERE dag_id = ?
              AND status = 'PENDING'
              AND depends_on_step_ids = '[]'
            ORDER BY created_at
            "#,
        )
        .bind(dag_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(steps)
    }

    async fn list_processing_steps_with_completed_jobs(
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

    async fn list_processing_steps_with_failed_jobs(&self) -> Result<Vec<DagStepExecutionDbModel>> {
        let steps = sqlx::query_as::<_, DagStepExecutionDbModel>(
            r#"
            SELECT step.*
            FROM dag_step_execution AS step
            JOIN dag_execution AS dag ON dag.id = step.dag_id
            LEFT JOIN job ON job.id = step.job_id
            WHERE step.status = 'PROCESSING'
              AND (step.job_id IS NULL OR job.status IN ('FAILED', 'CANCELLED'))
              AND dag.status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
            ORDER BY step.created_at, step.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(steps)
    }

    async fn list_dag_ids_with_unmaterialized_steps(&self) -> Result<Vec<String>> {
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
            ORDER BY dag.created_at, dag.id
            "#,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(dag_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::{
        DagExecutionStatus, DagPipelineDefinition, DagStep, JobDbModel, PipelineStep,
    };
    use crate::database::repositories::{JobRepository as _, SqlxJobRepository};
    use crate::database::{init_pool_with_size, run_migrations};

    async fn setup_test_pool() -> SqlitePool {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        pool
    }

    async fn create_job(pool: &SqlitePool, id: &str) {
        let mut job = JobDbModel::new("test-job", "{}");
        job.id = id.to_string();
        SqlxJobRepository::new(pool.clone(), pool.clone())
            .create_job(&job)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_create_and_get_dag() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );

        let dag = DagExecutionDbModel::new(&dag_def, Some("streamer-1".to_string()), None);
        let dag_id = dag.id.clone();

        repo.create_dag(&dag).await.unwrap();

        let retrieved = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(retrieved.id, dag_id);
        assert_eq!(retrieved.total_steps, 1);
    }

    /// The manifest is published in the same transaction as the steps and root
    /// jobs and comes back verbatim, so a restart or retry never has to look for
    /// pairing information outside the DAG row.
    #[tokio::test]
    async fn publish_dag_round_trips_the_input_manifest() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
        let dag_def = DagPipelineDefinition::new(
            "manifest-dag",
            vec![DagStep::new("A", PipelineStep::preset("remux"))],
        );
        let mut dag = DagExecutionDbModel::new(&dag_def, None, Some("session".to_string()));
        dag.status = DagExecutionStatus::Processing.as_str().to_string();
        let manifest = serde_json::json!({
            "version": 1,
            "session_id": "session",
            "streamer_id": "streamer",
            "scope": { "segment": { "index": 3 } },
            "segments": [{ "segment_index": 3, "video": ["/rec/3.mp4"], "danmu": ["/rec/3.xml"] }],
        })
        .to_string();
        dag.input_manifest = Some(manifest.clone());
        let mut step = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
        let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
        step.status = DagStepStatus::Processing.as_str().to_string();
        step.job_id = Some(job.id.clone());
        job.pipeline_id = Some(dag.id.clone());
        job.dag_step_execution_id = Some(step.id.clone());

        repo.publish_dag(&dag, &[step], &[job]).await.unwrap();

        let stored = repo.get_dag(&dag.id).await.unwrap();
        assert_eq!(stored.input_manifest.as_deref(), Some(manifest.as_str()));
        let listed = repo.list_dags(None, Some("session"), 10, 0).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].input_manifest, stored.input_manifest);

        let mut plain = DagExecutionDbModel::new(&dag_def, None, None);
        plain.status = DagExecutionStatus::Processing.as_str().to_string();
        repo.create_dag(&plain).await.unwrap();
        assert_eq!(repo.get_dag(&plain.id).await.unwrap().input_manifest, None);
    }

    /// The no-job completion transitions only a ready step that has no job in a
    /// non-terminal DAG; anything else is reported like a duplicate completion.
    #[tokio::test]
    async fn complete_ready_step_without_job_guards_its_transition() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());
        let dag_def = DagPipelineDefinition::new(
            "no-op",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("delete"),
                    vec!["A".to_string()],
                ),
            ],
        );
        let mut dag = DagExecutionDbModel::new(&dag_def, None, None);
        dag.status = DagExecutionStatus::Processing.as_str().to_string();
        repo.create_dag(&dag).await.unwrap();
        let step_a = DagStepExecutionDbModel::new(&dag.id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag.id, "B", &["A".to_string()]);
        repo.create_steps(&[step_a.clone(), step_b.clone()])
            .await
            .unwrap();
        let mut job = JobDbModel::new_pipeline_step("remux", "[]", "[]", 0, None, None);
        job.dag_step_execution_id = Some(step_a.id.clone());
        repo.create_job_for_step(&step_a.id, &job).await.unwrap();

        // A step with a job keeps its status.
        let untouched = repo
            .complete_ready_step_without_job(&step_a.id, &[])
            .await
            .unwrap();
        assert_eq!(untouched.step.status, "PROCESSING");
        assert!(untouched.ready_steps.is_empty());
        assert_eq!(untouched.dag.completed_steps, 0);

        repo.complete_step_and_check_dependents(&step_a.id, &[])
            .await
            .unwrap();
        let completed = repo
            .complete_ready_step_without_job(&step_b.id, &[])
            .await
            .unwrap();
        assert_eq!(completed.step.status, "COMPLETED");
        assert_eq!(completed.step.get_outputs(), Vec::<String>::new());
        assert_eq!(completed.dag.completed_steps, 2);
        assert_eq!(completed.dag.status, "COMPLETED");

        let replay = repo
            .complete_ready_step_without_job(&step_b.id, &["late.mp4".to_string()])
            .await
            .unwrap();
        assert_eq!(replay.step.get_outputs(), Vec::<String>::new());
        assert_eq!(replay.dag.completed_steps, 2);
    }

    #[tokio::test]
    async fn test_create_and_get_steps() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);

        repo.create_steps(&[step_a, step_b]).await.unwrap();

        let steps = repo.get_steps_by_dag(&dag_id).await.unwrap();
        assert_eq!(steps.len(), 2);

        let step_a = repo
            .get_step_by_dag_and_step_id(&dag_id, "A")
            .await
            .unwrap();
        assert!(step_a.is_root());
        assert_eq!(step_a.status, "PENDING"); // Root starts as PENDING

        let step_b = repo
            .get_step_by_dag_and_step_id(&dag_id, "B")
            .await
            .unwrap();
        assert!(!step_b.is_root());
        assert_eq!(step_b.status, "BLOCKED"); // Non-root starts as BLOCKED
    }

    #[tokio::test]
    async fn test_complete_step_and_check_dependents() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: A -> B
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_a_id = step_a.id.clone();
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);

        repo.create_steps(&[step_a, step_b]).await.unwrap();

        // Complete step A
        let completion = repo
            .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
            .await
            .unwrap();
        assert_eq!(completion.step.id, step_a_id);
        assert_eq!(completion.step.status, "COMPLETED");
        assert_eq!(completion.step.get_outputs(), vec!["/output/a.mp4"]);
        assert_eq!(completion.dag.completed_steps, 1);
        assert_eq!(
            serde_json::to_value(&completion.dag).unwrap(),
            serde_json::to_value(repo.get_dag(&dag_id).await.unwrap()).unwrap()
        );
        let ready_steps = completion.ready_steps;

        // Step B should now be ready
        assert_eq!(ready_steps.len(), 1);
        assert_eq!(ready_steps[0].step.step_id, "B");
        assert_eq!(ready_steps[0].merged_inputs, vec!["/output/a.mp4"]);
    }

    #[tokio::test]
    async fn test_fan_in_complete() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: [A, B] -> C (fan-in)
        let dag_def = DagPipelineDefinition::new(
            "fan-in-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("B", PipelineStep::preset("thumbnail")),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string(), "B".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_a_id = step_a.id.clone();
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &[]);
        let step_b_id = step_b.id.clone();
        let step_c =
            DagStepExecutionDbModel::new(&dag_id, "C", &["A".to_string(), "B".to_string()]);

        repo.create_steps(&[step_a, step_b, step_c]).await.unwrap();

        // Complete step A - C should NOT be ready yet
        let ready = repo
            .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
            .await
            .unwrap()
            .ready_steps;
        assert!(ready.is_empty());

        // Complete step B - NOW C should be ready with merged inputs
        let ready = repo
            .complete_step_and_check_dependents(&step_b_id, &["/output/b.jpg".to_string()])
            .await
            .unwrap()
            .ready_steps;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].step.step_id, "C");
        assert_eq!(
            ready[0].merged_inputs,
            vec!["/output/a.mp4".to_string(), "/output/b.jpg".to_string()]
        );
    }

    #[tokio::test]
    async fn test_complete_step_is_idempotent() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: A -> B
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_a_id = step_a.id.clone();
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);

        repo.create_steps(&[step_a, step_b]).await.unwrap();

        let ready = repo
            .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
            .await
            .unwrap()
            .ready_steps;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].step.step_id, "B");

        // Duplicate completion should be a no-op (no double increment, no re-ready).
        let ready2 = repo
            .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
            .await
            .unwrap()
            .ready_steps;
        assert!(ready2.is_empty());

        let dag = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(dag.completed_steps, 1);

        let step_b = repo
            .get_step_by_dag_and_step_id(&dag_id, "B")
            .await
            .unwrap();
        assert_eq!(step_b.status, "PENDING");
    }

    #[tokio::test]
    async fn test_fail_dag_and_cancel_steps() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: A -> B -> C
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);
        let step_c = DagStepExecutionDbModel::new(&dag_id, "C", &["B".to_string()]);

        repo.create_steps(&[step_a, step_b, step_c]).await.unwrap();

        // Fail the DAG
        repo.fail_dag_and_cancel_steps(&dag_id, "Step A failed")
            .await
            .unwrap();

        // Check DAG status
        let dag = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(dag.status, "FAILED");
        assert_eq!(dag.error, Some("Step A failed".to_string()));

        // Check step statuses - B and C should be cancelled
        let step_b = repo
            .get_step_by_dag_and_step_id(&dag_id, "B")
            .await
            .unwrap();
        assert_eq!(step_b.status, "CANCELLED");

        let step_c = repo
            .get_step_by_dag_and_step_id(&dag_id, "C")
            .await
            .unwrap();
        assert_eq!(step_c.status, "CANCELLED");
    }

    #[tokio::test]
    async fn test_fail_dag_cancels_processing_steps() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: A -> B
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);
        let step_b_id = step_b.id.clone();

        repo.create_steps(&[step_a, step_b]).await.unwrap();

        // Simulate an in-flight job for step B.
        create_job(&pool, "job-b").await;
        repo.update_step_status_with_job(&step_b_id, "PROCESSING", "job-b")
            .await
            .unwrap();

        let cancelled = repo
            .fail_dag_and_cancel_steps(&dag_id, "fail-fast")
            .await
            .unwrap();
        assert_eq!(cancelled, vec!["job-b".to_string()]);

        let step_b = repo
            .get_step_by_dag_and_step_id(&dag_id, "B")
            .await
            .unwrap();
        assert_eq!(step_b.status, "CANCELLED");

        let dag = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(dag.status, "FAILED");
    }

    #[tokio::test]
    async fn test_cancel_dag_and_cancel_steps_marks_parent_cancelled() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);
        let step_b_id = step_b.id.clone();

        repo.create_steps(&[step_a, step_b]).await.unwrap();
        create_job(&pool, "job-b").await;
        repo.update_step_status_with_job(&step_b_id, "PROCESSING", "job-b")
            .await
            .unwrap();

        let cancelled = repo
            .cancel_dag_and_cancel_steps(&dag_id, "Cancelled by user")
            .await
            .unwrap();
        assert_eq!(cancelled, vec!["job-b".to_string()]);

        let dag = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(dag.status, "CANCELLED");
        assert_eq!(dag.error, Some("Cancelled by user".to_string()));

        let step_b = repo.get_step(&step_b_id).await.unwrap();
        assert_eq!(step_b.status, "CANCELLED");
    }

    #[tokio::test]
    async fn test_reset_dag_for_retry_unblocks_downstream() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool.clone(), pool.clone());

        // Create DAG: A -> B
        let dag_def = DagPipelineDefinition::new(
            "test-dag",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
            ],
        );

        let dag = DagExecutionDbModel::new(&dag_def, None, None);
        let dag_id = dag.id.clone();
        repo.create_dag(&dag).await.unwrap();

        let step_a = DagStepExecutionDbModel::new(&dag_id, "A", &[]);
        let step_b = DagStepExecutionDbModel::new(&dag_id, "B", &["A".to_string()]);
        let step_a_id = step_a.id.clone();
        let step_b_id = step_b.id.clone();

        repo.create_steps(&[step_a, step_b]).await.unwrap();

        // Mimic a failed DAG: A failed, B cancelled by fail-fast.
        create_job(&pool, "job-a").await;
        repo.update_step_status_with_job(&step_a_id, "PROCESSING", "job-a")
            .await
            .unwrap();
        repo.update_step_status(&step_a_id, "FAILED").await.unwrap();
        repo.increment_dag_failed(&dag_id).await.unwrap();
        repo.fail_dag_and_cancel_steps(&dag_id, "Step A failed")
            .await
            .unwrap();

        let b = repo.get_step(&step_b_id).await.unwrap();
        assert_eq!(b.status, "CANCELLED");

        // Retry prep should restore downstream to BLOCKED so completion can re-trigger it.
        repo.reset_dag_for_retry(&dag_id).await.unwrap();

        let dag = repo.get_dag(&dag_id).await.unwrap();
        assert_eq!(dag.status, "PROCESSING");
        assert!(dag.completed_at.is_none());
        assert!(dag.error.is_none());
        assert_eq!(dag.failed_steps, 0);

        let a = repo.get_step(&step_a_id).await.unwrap();
        assert_eq!(a.status, "PROCESSING");

        let b = repo.get_step(&step_b_id).await.unwrap();
        assert_eq!(b.status, "BLOCKED");

        // Completing A should now discover B as ready.
        let ready = repo
            .complete_step_and_check_dependents(&step_a_id, &["/out/a.mp4".to_string()])
            .await
            .unwrap()
            .ready_steps;
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].step.step_id, "B");
        assert_eq!(ready[0].merged_inputs, vec!["/out/a.mp4".to_string()]);
    }
}
