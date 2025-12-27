//! DAG (Directed Acyclic Graph) repository for pipeline execution.

use async_trait::async_trait;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

use crate::database::models::{
    DagExecutionDbModel, DagExecutionStats, DagStepExecutionDbModel, DagStepStatus, ReadyStep,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};

/// DAG repository trait for pipeline execution management.
#[async_trait]
pub trait DagRepository: Send + Sync {
    // ========================================================================
    // DAG Execution CRUD
    // ========================================================================

    /// Create a new DAG execution record.
    async fn create_dag(&self, dag: &DagExecutionDbModel) -> Result<()>;

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

    // ========================================================================
    // Core DAG Operations (Atomic)
    // ========================================================================

    /// Atomically complete a step and check for ready dependents.
    /// Returns steps that are now ready to run (all dependencies complete).
    async fn complete_step_and_check_dependents(
        &self,
        step_id: &str,
        outputs: &[String],
    ) -> Result<Vec<ReadyStep>>;

    /// Atomically fail a DAG and cancel all pending/blocked steps.
    /// Returns job IDs of steps that had jobs created (for cancellation).
    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>>;

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
}

/// SQLx implementation of DagRepository.
pub struct SqlxDagRepository {
    pool: SqlitePool,
}

impl SqlxDagRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl DagRepository for SqlxDagRepository {
    // ========================================================================
    // DAG Execution CRUD
    // ========================================================================

    async fn create_dag(&self, dag: &DagExecutionDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dag_execution (
                id, dag_definition, status, streamer_id, session_id,
                created_at, updated_at, completed_at, error,
                total_steps, completed_steps, failed_steps
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&dag.id)
        .bind(&dag.dag_definition)
        .bind(&dag.status)
        .bind(&dag.streamer_id)
        .bind(&dag.session_id)
        .bind(&dag.created_at)
        .bind(&dag.updated_at)
        .bind(&dag.completed_at)
        .bind(&dag.error)
        .bind(dag.total_steps)
        .bind(dag.completed_steps)
        .bind(dag.failed_steps)
        .execute(&self.pool)
        .await?;
        Ok(())
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
            let now = chrono::Utc::now().to_rfc3339();
            let completed_at = if status == "COMPLETED" || status == "FAILED" {
                Some(now.clone())
            } else {
                None
            };

            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = ?, updated_at = ?, completed_at = COALESCE(?, completed_at), error = ?
                WHERE id = ?
                "#,
            )
            .bind(status)
            .bind(&now)
            .bind(&completed_at)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn increment_dag_completed(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("increment_dag_completed", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE dag_execution SET completed_steps = completed_steps + 1, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(dag_id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn increment_dag_failed(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("increment_dag_failed", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE dag_execution SET failed_steps = failed_steps + 1, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(dag_id)
            .execute(&self.pool)
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

        let mut query = sqlx::query_as::<_, DagExecutionDbModel>(&sql);
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

        let mut query = sqlx::query_scalar::<_, i64>(&sql);
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
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // ========================================================================
    // DAG Step Execution CRUD
    // ========================================================================

    async fn create_step(&self, step: &DagStepExecutionDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO dag_step_execution (
                id, dag_id, step_id, job_id, status,
                depends_on_step_ids, outputs, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&step.id)
        .bind(&step.dag_id)
        .bind(&step.step_id)
        .bind(&step.job_id)
        .bind(&step.status)
        .bind(&step.depends_on_step_ids)
        .bind(&step.outputs)
        .bind(&step.created_at)
        .bind(&step.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_steps(&self, steps: &[DagStepExecutionDbModel]) -> Result<()> {
        if steps.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        for step in steps {
            sqlx::query(
                r#"
                INSERT INTO dag_step_execution (
                    id, dag_id, step_id, job_id, status,
                    depends_on_step_ids, outputs, created_at, updated_at
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&step.id)
            .bind(&step.dag_id)
            .bind(&step.step_id)
            .bind(&step.job_id)
            .bind(&step.status)
            .bind(&step.depends_on_step_ids)
            .bind(&step.outputs)
            .bind(&step.created_at)
            .bind(&step.updated_at)
            .execute(&mut *tx)
            .await?;
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
            let now = chrono::Utc::now().to_rfc3339();
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
            .bind(&now)
            .bind(&step.id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn update_step_status(&self, id: &str, status: &str) -> Result<()> {
        retry_on_sqlite_busy("update_step_status", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query("UPDATE dag_step_execution SET status = ?, updated_at = ? WHERE id = ?")
                .bind(status)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
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
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE dag_step_execution SET status = ?, job_id = ?, updated_at = ? WHERE id = ?",
            )
            .bind(status)
            .bind(job_id)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
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
    ) -> Result<Vec<ReadyStep>> {
        retry_on_sqlite_busy("complete_step_and_check_dependents", || async {
            let mut tx = self.pool.begin().await?;
            let now = chrono::Utc::now().to_rfc3339();
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

            // 1. Mark step as completed with outputs
            sqlx::query(
                r#"
                UPDATE dag_step_execution
                SET status = 'COMPLETED', outputs = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(&outputs_json)
            .bind(&now)
            .bind(step_id)
            .execute(&mut *tx)
            .await?;

            // 2. Get the completed step info
            let completed_step = sqlx::query_as::<_, DagStepExecutionDbModel>(
                "SELECT * FROM dag_step_execution WHERE id = ?",
            )
            .bind(step_id)
            .fetch_one(&mut *tx)
            .await?;

            // 3. Increment completed count on DAG
            sqlx::query(
                "UPDATE dag_execution SET completed_steps = completed_steps + 1, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&completed_step.dag_id)
            .execute(&mut *tx)
            .await?;

            // 4. Find blocked steps that depend on this step
            // SQLite: Use json_each to check if step_id is in depends_on_step_ids
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

            // 5. For each dependent, check if ALL its dependencies are complete.
            //
            // Performance: avoid per-dependent SQL queries. Instead, load the DAG step statuses
            // and completed outputs once and evaluate readiness in-memory.
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
                    .bind(&now)
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

            // 6. Check if DAG is complete
            let dag = sqlx::query_as::<_, DagExecutionDbModel>(
                "SELECT * FROM dag_execution WHERE id = ?",
            )
            .bind(&completed_step.dag_id)
            .fetch_one(&mut *tx)
            .await?;

            if dag.completed_steps + dag.failed_steps >= dag.total_steps {
                // DAG is complete
                let final_status = if dag.failed_steps > 0 {
                    "FAILED"
                } else {
                    "COMPLETED"
                };
                sqlx::query(
                    "UPDATE dag_execution SET status = ?, completed_at = ?, updated_at = ? WHERE id = ?",
                )
                .bind(final_status)
                .bind(&now)
                .bind(&now)
                .bind(&completed_step.dag_id)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok(ready_steps)
        })
        .await
    }

    async fn fail_dag_and_cancel_steps(&self, dag_id: &str, error: &str) -> Result<Vec<String>> {
        retry_on_sqlite_busy("fail_dag_and_cancel_steps", || async {
            let mut tx = self.pool.begin().await?;
            let now = chrono::Utc::now().to_rfc3339();

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
            .bind(&now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            // 3. Mark the DAG as FAILED
            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = 'FAILED', completed_at = ?, updated_at = ?, error = ?
                WHERE id = ?
                "#,
            )
            .bind(&now)
            .bind(&now)
            .bind(error)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(processing_job_ids)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::{DagPipelineDefinition, DagStep, PipelineStep};

    async fn setup_test_pool() -> SqlitePool {
        let pool = SqlitePool::connect(":memory:").await.unwrap();

        // Create DAG tables
        sqlx::query(
            r#"
            CREATE TABLE dag_execution (
                id TEXT PRIMARY KEY,
                dag_definition TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'PENDING',
                streamer_id TEXT,
                session_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT,
                error TEXT,
                total_steps INTEGER NOT NULL,
                completed_steps INTEGER NOT NULL DEFAULT 0,
                failed_steps INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            CREATE TABLE dag_step_execution (
                id TEXT PRIMARY KEY,
                dag_id TEXT NOT NULL,
                step_id TEXT NOT NULL,
                job_id TEXT,
                status TEXT NOT NULL DEFAULT 'BLOCKED',
                depends_on_step_ids TEXT NOT NULL DEFAULT '[]',
                outputs TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (dag_id) REFERENCES dag_execution(id) ON DELETE CASCADE,
                UNIQUE (dag_id, step_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_create_and_get_dag() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool);

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

    #[tokio::test]
    async fn test_create_and_get_steps() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool);

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
        let repo = SqlxDagRepository::new(pool);

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
        let ready_steps = repo
            .complete_step_and_check_dependents(&step_a_id, &["/output/a.mp4".to_string()])
            .await
            .unwrap();

        // Step B should now be ready
        assert_eq!(ready_steps.len(), 1);
        assert_eq!(ready_steps[0].step.step_id, "B");
        assert_eq!(ready_steps[0].merged_inputs, vec!["/output/a.mp4"]);
    }

    #[tokio::test]
    async fn test_fan_in_complete() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool);

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
            .unwrap();
        assert!(ready.is_empty());

        // Complete step B - NOW C should be ready with merged inputs
        let ready = repo
            .complete_step_and_check_dependents(&step_b_id, &["/output/b.jpg".to_string()])
            .await
            .unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].step.step_id, "C");
        assert_eq!(
            ready[0].merged_inputs,
            vec!["/output/a.mp4".to_string(), "/output/b.jpg".to_string()]
        );
    }

    #[tokio::test]
    async fn test_fail_dag_and_cancel_steps() {
        let pool = setup_test_pool().await;
        let repo = SqlxDagRepository::new(pool);

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
}
