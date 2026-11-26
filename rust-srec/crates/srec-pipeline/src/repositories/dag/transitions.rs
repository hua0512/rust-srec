//! Transactional DAG publication, step state transitions, and settlement.

use std::collections::{HashMap, HashSet, VecDeque};

use super::{SqlxDagRepository, StepCompletion, writes};
use srec_core::CoreError;
use srec_sqlite::begin_immediate;
use srec_sqlite::models::{
    DagExecutionDbModel, DagExecutionStatus, DagStepExecutionDbModel, DagStepStatus, JobDbModel,
    ReadyStep,
};
use srec_sqlite::retry::retry_on_sqlite_busy;
use srec_sqlite::{DbError as Error, DbResult as Result};

/// Which row state a step completion may transition from.
#[derive(Clone, Copy)]
pub(super) enum CompletionGuard {
    /// A step whose job finished: PENDING or PROCESSING.
    AttachedJob,
    /// A ready step completed without a job: BLOCKED or PENDING with `job_id` NULL
    /// in a non-terminal DAG.
    NoJob,
}

impl SqlxDagRepository {
    pub(super) async fn publish_dag_tx(
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
                super::super::job::writes::insert_job(&mut tx, job).await?;

                let Some(step_id) = job.dag_step_execution_id.as_deref() else {
                    return Err(Error::validation(format!(
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
                    return Err(Error::Core(CoreError::InvalidStateTransition {
                        from: format!("unpublished root step {}", step_id),
                        to: format!("PROCESSING(job_id={})", job.id),
                    }));
                }
            }

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    pub(super) async fn create_job_for_step_tx(
        &self,
        step_id: &str,
        job: &JobDbModel,
    ) -> Result<()> {
        retry_on_sqlite_busy("create_job_for_step", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;

            super::super::job::writes::insert_job(&mut tx, job).await?;

            let now = srec_sqlite::time::now_ms();
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
                return Err(Error::Core(CoreError::InvalidStateTransition {
                    from: format!("unmaterialized active step {}", step_id),
                    to: format!("PROCESSING(job_id={})", job.id),
                }));
            }

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    pub(super) async fn complete_step_with_guard(
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
            let now = srec_sqlite::time::now_ms();
            let outputs_json = serde_json::to_string(outputs)?;

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
                let step = sqlx::query_as::<_, DagStepExecutionDbModel>(
                    "SELECT * FROM dag_step_execution WHERE id = ?",
                )
                .bind(step_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| Error::not_found("DAG step execution", step_id))?;
                let dag = sqlx::query_as::<_, DagExecutionDbModel>(
                    "SELECT * FROM dag_execution WHERE id = ?",
                )
                .bind(&step.dag_id)
                .fetch_one(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(StepCompletion {
                    step,
                    dag,
                    ready_steps: Vec::new(),
                });
            };

            // Keep the DAG snapshot from its counter update within this transaction.
            let mut dag = sqlx::query_as::<_, DagExecutionDbModel>(
                "UPDATE dag_execution SET completed_steps = completed_steps + 1, updated_at = ? WHERE id = ? RETURNING *",
            )
            .bind(now)
            .bind(&completed_step.dag_id)
            .fetch_one(&mut *tx)
            .await?;

            // Only the completed step's BLOCKED direct dependents can have become
            // ready through this completion; anything else that is ready and
            // jobless is left to retry fan-out and startup recovery. Readiness
            // is read under the same write transaction as the transition.
            let rows = sqlx::query_as::<_, DagStepExecutionDbModel>(
                "SELECT * FROM dag_step_execution WHERE dag_id = ? ORDER BY created_at",
            )
            .bind(&completed_step.dag_id)
            .fetch_all(&mut *tx)
            .await?;
            let mut ready_steps = ready_steps(&rows, |row, depends_on| {
                row.status == DagStepStatus::Blocked.as_str()
                    && depends_on.contains(&completed_step.step_id)
            });
            for ready in &mut ready_steps {
                sqlx::query(
                    "UPDATE dag_step_execution SET status = 'PENDING', updated_at = ? WHERE id = ?",
                )
                .bind(now)
                .bind(&ready.step.id)
                .execute(&mut *tx)
                .await?;
                ready.step.status = DagStepStatus::Pending.as_str().to_string();
            }

            // Settle the parent DAG in the same transaction once every step has
            // settled; the cancelled dependents of a failed step count as settled.
            if let Some(finalized) =
                finalize_settled_dag(&mut tx, &completed_step.dag_id, now).await?
            {
                dag = finalized;
            }

            tx.commit().await?;
            Ok(StepCompletion {
                step: completed_step,
                dag,
                ready_steps,
            })
        })
        .await
    }

    pub(super) async fn fail_step_and_cancel_dag_tx(
        &self,
        step_id: &str,
        error: &str,
    ) -> Result<Option<Vec<String>>> {
        retry_on_sqlite_busy("fail_step_and_cancel_dag", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = srec_sqlite::time::now_ms();

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

            // Only the steps downstream of the failed one have lost their inputs.
            // A branch that does not depend on it keeps running and settles on
            // its own; the DAG is finalized when the last active step does.
            let rows = sqlx::query_as::<_, DagStepExecutionDbModel>(
                "SELECT * FROM dag_step_execution WHERE dag_id = ?",
            )
            .bind(&step.dag_id)
            .fetch_all(&mut *tx)
            .await?;
            let dependents = transitive_dependents(&step.step_id, &rows);
            let mut cancelled_job_ids = Vec::new();
            for row in rows
                .iter()
                .filter(|row| dependents.contains(&row.step_id))
                .filter(|row| matches!(row.status.as_str(), "BLOCKED" | "PENDING" | "PROCESSING"))
            {
                sqlx::query(
                    "UPDATE dag_step_execution SET status = 'CANCELLED', updated_at = ? WHERE id = ?",
                )
                .bind(now)
                .bind(&row.id)
                .execute(&mut *tx)
                .await?;
                if row.status == "PROCESSING"
                    && let Some(job_id) = &row.job_id
                {
                    cancelled_job_ids.push(job_id.clone());
                }
            }

            // The first failure's message becomes the DAG's error and later
            // failures of a draining DAG keep it; the status is settled below.
            sqlx::query(
                r#"
                UPDATE dag_execution
                SET failed_steps = failed_steps + 1,
                    updated_at = ?,
                    error = COALESCE(error, ?)
                WHERE id = ?
                "#,
            )
            .bind(now)
            .bind(error)
            .bind(&step.dag_id)
            .execute(&mut *tx)
            .await?;

            finalize_settled_dag(&mut tx, &step.dag_id, now).await?;

            tx.commit().await?;
            Ok(Some(cancelled_job_ids))
        })
        .await
    }

    /// Settle a non-terminal DAG as `status` and cancel every step that has
    /// not settled, running ones included. Running steps keep their job_id for
    /// observability; the returned job IDs let the caller cancel those jobs.
    pub(super) async fn terminate_dag(
        &self,
        dag_id: &str,
        error: &str,
        status: DagExecutionStatus,
        label: &'static str,
    ) -> Result<Vec<String>> {
        retry_on_sqlite_busy(label, || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = srec_sqlite::time::now_ms();

            let current: Option<String> =
                sqlx::query_scalar("SELECT status FROM dag_execution WHERE id = ?")
                    .bind(dag_id)
                    .fetch_optional(&mut *tx)
                    .await?;
            if current
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
                WHERE dag_id = ? AND status IN ('BLOCKED', 'PENDING', 'PROCESSING')
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                r#"
                UPDATE dag_execution
                SET status = ?, completed_at = ?, updated_at = ?, error = ?
                WHERE id = ? AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
                "#,
            )
            .bind(status.as_str())
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

    pub(super) async fn reset_dag_for_retry_tx(&self, dag_id: &str) -> Result<()> {
        retry_on_sqlite_busy("reset_dag_for_retry", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = srec_sqlite::time::now_ms();

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
                    failed_steps = 0
                WHERE id = ?
                "#,
            )
            .bind(now)
            .bind(dag_id)
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(())
        })
        .await
    }
}

/// Readiness of some of a DAG's steps, from all of its rows. A candidate is
/// ready when every step it depends on has COMPLETED; its inputs are those
/// dependencies' outputs in dependency order, each path once. `candidate`
/// sees each row with its parsed dependency list. The completion transaction
/// and [`super::DagRepository::list_ready_steps`] both decide readiness here, so a
/// step materialized by retry fan-out or recovery gets the inputs a live
/// completion would have given it.
pub(super) fn ready_steps(
    rows: &[DagStepExecutionDbModel],
    candidate: impl Fn(&DagStepExecutionDbModel, &[String]) -> bool,
) -> Vec<ReadyStep> {
    let completed_outputs: HashMap<&str, Vec<String>> = rows
        .iter()
        .filter(|row| row.status == DagStepStatus::Completed.as_str())
        .map(|row| (row.step_id.as_str(), row.get_outputs()))
        .collect();
    rows.iter()
        .filter_map(|row| {
            let depends_on = row.get_depends_on();
            if !candidate(row, &depends_on) {
                return None;
            }
            let mut merged_inputs = Vec::new();
            let mut seen = HashSet::<String>::new();
            for dependency in &depends_on {
                // A dependency that has not COMPLETED, or does not exist, keeps
                // the step waiting.
                let outputs = completed_outputs.get(dependency.as_str())?;
                for output in outputs {
                    if seen.insert(srec_core::utils::fs::path_dedup_key(output)) {
                        merged_inputs.push(output.clone());
                    }
                }
            }
            Some(ReadyStep {
                step: row.clone(),
                merged_inputs,
            })
        })
        .collect()
}

/// Finalize a DAG whose steps have all settled, inside the completion or
/// failure transaction that settled the last one. When no step is BLOCKED,
/// PENDING or PROCESSING, the DAG becomes FAILED if any step failed or was
/// cancelled and COMPLETED otherwise. Returns `None` for a DAG that still has
/// an active step or is already terminal, which is left as it is.
async fn finalize_settled_dag(
    tx: &mut sqlx::SqliteConnection,
    dag_id: &str,
    now: i64,
) -> Result<Option<DagExecutionDbModel>> {
    let finalized = sqlx::query_as::<_, DagExecutionDbModel>(
        r#"
        UPDATE dag_execution
        SET status = CASE
                WHEN EXISTS (
                    SELECT 1 FROM dag_step_execution
                    WHERE dag_id = dag_execution.id
                      AND status IN ('FAILED', 'CANCELLED')
                ) THEN 'FAILED'
                ELSE 'COMPLETED'
            END,
            completed_at = ?,
            updated_at = ?
        WHERE id = ?
          AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED')
          AND NOT EXISTS (
            SELECT 1 FROM dag_step_execution
            WHERE dag_id = dag_execution.id
              AND status IN ('BLOCKED', 'PENDING', 'PROCESSING')
          )
        RETURNING *
        "#,
    )
    .bind(now)
    .bind(now)
    .bind(dag_id)
    .fetch_optional(&mut *tx)
    .await?;
    Ok(finalized)
}

/// Step ids that depend on `origin`, directly or through other steps, from the
/// rows' stored dependency lists.
fn transitive_dependents(origin: &str, steps: &[DagStepExecutionDbModel]) -> HashSet<String> {
    let mut dependents_of: HashMap<String, Vec<&str>> = HashMap::new();
    for step in steps {
        for dependency in step.get_depends_on() {
            dependents_of
                .entry(dependency)
                .or_default()
                .push(step.step_id.as_str());
        }
    }
    let mut result = HashSet::new();
    let mut queue = VecDeque::from([origin]);
    while let Some(current) = queue.pop_front() {
        for &dependent in dependents_of.get(current).into_iter().flatten() {
            if result.insert(dependent.to_string()) {
                queue.push_back(dependent);
            }
        }
    }
    result
}
