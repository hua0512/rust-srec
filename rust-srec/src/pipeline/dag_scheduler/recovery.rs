//! Terminal workflow retries and startup DAG reconciliation.

use std::collections::HashSet;

use tracing::{debug, warn};

use super::{DagJobCompletedUpdate, DagRetryError, DagRetryUpdate, DagScheduler, RetryAction};
use crate::database::models::{DagExecutionStatus, DagStepExecutionDbModel, DagStepStatus};
use crate::pipeline::JobStatus;
use crate::pipeline::job_queue::{JobStateMeta, parse_job_state};
use crate::{Error, Result};

impl DagScheduler {
    /// Retry every failed or cancelled branch of a terminal workflow.
    ///
    /// Every FAILED or CANCELLED step is retried: one with a job has that job
    /// restarted; one without (its job creation failed, or it was cancelled
    /// before it was materialized) is materialized again once the reset
    /// unblocks it. A step whose job completed while the run was being
    /// cancelled has that result replayed instead. Every job is checked
    /// before any row changes, so a missing or still active job rejects the
    /// retry while the DAG is untouched and still retryable.
    ///
    /// `after_reset` runs once the rows are reset and before any job is
    /// restarted or materialized: a restarted job can settle the workflow
    /// again at once, and the caller may need to stop treating the earlier
    /// outcome as current before that can happen.
    pub async fn retry_dag(
        &self,
        dag_id: &str,
        after_reset: impl FnOnce(),
    ) -> std::result::Result<DagRetryUpdate, DagRetryError> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        if !matches!(
            dag.get_status(),
            Some(DagExecutionStatus::Failed | DagExecutionStatus::Cancelled)
        ) {
            return Err(
                Error::Validation("DAG is not in FAILED or CANCELLED status".to_string()).into(),
            );
        }

        let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;
        let retryable_steps: Vec<&DagStepExecutionDbModel> = steps
            .iter()
            .filter(|step| {
                matches!(
                    step.get_status(),
                    Some(DagStepStatus::Failed | DagStepStatus::Cancelled)
                )
            })
            .collect();
        if retryable_steps.is_empty() {
            return Err(Error::Validation(
                "No failed or cancelled steps found to retry".to_string(),
            )
            .into());
        }

        let mut actions = Vec::with_capacity(retryable_steps.len());
        for step in &retryable_steps {
            let Some(job_id) = step.job_id.as_deref() else {
                continue;
            };
            let job = self.job_queue.get_job(job_id).await?.ok_or_else(|| {
                Error::Validation(format!(
                    "Job {} of step '{}' no longer exists; the workflow cannot be retried",
                    job_id, step.step_id
                ))
            })?;
            let action = match job.status {
                JobStatus::Failed | JobStatus::Cancelled => RetryAction::Restart(job),
                JobStatus::Completed => RetryAction::Replay(job),
                JobStatus::Pending | JobStatus::Processing => {
                    return Err(Error::Validation(format!(
                        "Job {} of step '{}' is {} and cannot be retried",
                        job_id,
                        step.step_id,
                        job.status.as_str()
                    ))
                    .into());
                }
            };
            actions.push((*step, action));
        }

        // From here on every failure re-fails the DAG with the retry error, so
        // a partially restarted DAG never sits in PROCESSING with nothing to
        // advance it.
        if let Err(error) = self.dag_repository.reset_dag_for_retry(dag_id).await {
            return Err(self.refail_after_retry(dag_id, error).await);
        }
        after_reset();

        // A cancelled step whose dependencies had all completed (a parallel
        // branch cut short by fail-fast) gets no further completion event, so
        // it is materialized here; a no-op step can settle the DAG at once.
        let mut completions = Vec::new();
        let mut job_ids = match self.enqueue_now_ready_steps(dag_id).await {
            Ok(update) => {
                completions.extend(update.completion);
                update.new_job_ids
            }
            Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
        };

        let mut restarted_jobs = Vec::new();
        let mut reconciled_steps = 0usize;
        for (step, action) in actions {
            match action {
                RetryAction::Restart(job) => match self.job_queue.retry_job(&job.id).await {
                    Ok(job) => {
                        job_ids.push(job.id.clone());
                        restarted_jobs.push(job);
                    }
                    // Two retries of the same DAG can pass the terminal check
                    // before either reset commits; a job the other one already
                    // restarted needs nothing more from this call.
                    Err(error) if self.job_already_restarted(&job.id).await => {
                        debug!(
                            dag_id,
                            job_id = %job.id,
                            %error,
                            "Job was restarted by a concurrent retry"
                        );
                        job_ids.push(job.id);
                    }
                    Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
                },
                RetryAction::Replay(job) => {
                    match self
                        .on_job_completed(
                            &step.id,
                            &job.outputs,
                            job.streamer_name.as_deref(),
                            job.session_title.as_deref(),
                            job.platform.as_deref(),
                            job.session_start,
                        )
                        .await
                    {
                        Ok(update) => {
                            reconciled_steps += 1;
                            job_ids.extend(update.new_job_ids);
                            completions.extend(update.completion);
                        }
                        Err(error) => return Err(self.refail_after_retry(dag_id, error).await),
                    }
                }
            }
        }

        // Steps settled by the retry itself (restarted, materialized, or
        // completed as no-ops) are those that are no longer FAILED or CANCELLED.
        let retryable_ids: HashSet<&str> = retryable_steps
            .iter()
            .map(|step| step.id.as_str())
            .collect();
        // The reset alone makes every retryable step active again, so a
        // lower count means a restarted job already failed again; the retry
        // itself has taken effect either way, so an unreadable row set only
        // costs the exact count.
        let retried_steps = match self.dag_repository.get_steps_by_dag(dag_id).await {
            Ok(steps) => steps
                .iter()
                .filter(|step| {
                    retryable_ids.contains(step.id.as_str())
                        && !matches!(
                            step.get_status(),
                            Some(DagStepStatus::Failed | DagStepStatus::Cancelled)
                        )
                })
                .count(),
            Err(error) => {
                warn!(dag_id, %error, "Could not count the retried steps after the retry");
                retryable_steps.len()
            }
        };

        Ok(DagRetryUpdate {
            retryable_steps: retryable_steps.len(),
            retried_steps,
            reconciled_steps,
            restarted_jobs,
            job_ids,
            completions,
        })
    }

    /// Whether `job_id` is already queued or running, as after a concurrent retry.
    async fn job_already_restarted(&self, job_id: &str) -> bool {
        matches!(
            self.job_queue.get_job(job_id).await,
            Ok(Some(job)) if matches!(job.status, JobStatus::Pending | JobStatus::Processing)
        )
    }

    /// Fail a DAG whose retry broke down after its rows were reset, so it is
    /// terminal (and retryable) again instead of stranded in PROCESSING.
    async fn refail_after_retry(&self, dag_id: &str, error: Error) -> DagRetryError {
        let message = format!("Retry failed: {error}");
        let completion = match self.fail_dag(dag_id, &message).await {
            Ok(completion) => completion,
            Err(fail_error) => {
                warn!(
                    dag_id,
                    error = %fail_error,
                    "Failed to re-fail DAG after an unsuccessful retry"
                );
                None
            }
        };
        DagRetryError::Refailed {
            error,
            completion: completion.map(Box::new),
        }
    }

    /// Reconcile durable job results and unmaterialized ready steps before workers resume.
    pub async fn recover_dag_jobs(&self) -> Result<usize> {
        self.recover_dag_jobs_with_status()
            .await
            .map(|(count, _)| count)
    }

    pub(crate) async fn recover_dag_jobs_with_status(&self) -> Result<(usize, bool)> {
        let mut complete = true;
        let completed_steps = self
            .dag_repository
            .list_processing_steps_with_completed_jobs()
            .await?;
        let mut materialized_jobs = 0usize;

        // One unreadable row must not strand the rest of the reconciliation: `on_job_completed`
        // rejects a DAG whose `dag_definition` no longer parses, and that DAG's neighbours are
        // still recoverable. Each failure is isolated to its own step or DAG.
        for step in completed_steps {
            let Some(job_id) = step.job_id.as_deref() else {
                complete = false;
                continue;
            };
            let job = match self.job_repository.get_job(job_id).await {
                Ok(job) => job,
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %step.dag_id,
                        step_id = %step.step_id,
                        job_id = %job_id,
                        error = %e,
                        "Skipping DAG step reconciliation for unreadable job"
                    );
                    continue;
                }
            };
            if !serde_json::from_str::<serde_json::Value>(&job.state)
                .is_ok_and(|value| value.is_object())
                || job
                    .outputs
                    .as_deref()
                    .is_some_and(|raw| serde_json::from_str::<Vec<String>>(raw).is_err())
            {
                complete = false;
            }
            let metadata = parse_job_state(&job.state);
            match self
                .on_job_completed(
                    &step.id,
                    &job.get_outputs(),
                    metadata.streamer_name.as_deref(),
                    metadata.session_title.as_deref(),
                    metadata.platform.as_deref(),
                    metadata.session_start,
                )
                .await
            {
                Ok(update) => {
                    materialized_jobs = materialized_jobs.saturating_add(update.new_job_ids.len());
                }
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %step.dag_id,
                        step_id = %step.step_id,
                        error = %e,
                        "Failed to advance DAG step from its durable job result"
                    );
                }
            }
        }

        // A worker that failed its job but died before `on_job_failed` ran leaves the step
        // PROCESSING forever: nothing else reports on that job, and retry rejects a DAG
        // that is not terminal. Fail those DAGs now so they become retryable.
        for step in self
            .dag_repository
            .list_processing_steps_with_failed_jobs()
            .await?
        {
            let reason = match step.job_id.as_deref() {
                None => "the step's job row no longer exists".to_string(),
                Some(job_id) => match self.job_repository.get_job(job_id).await {
                    Ok(job) => format!(
                        "job {} is {}{}",
                        job_id,
                        job.status,
                        job.error
                            .as_deref()
                            .map(|error| format!(": {error}"))
                            .unwrap_or_default()
                    ),
                    Err(e) => {
                        complete = false;
                        warn!(
                            dag_id = %step.dag_id,
                            step_id = %step.step_id,
                            job_id = %job_id,
                            error = %e,
                            "Skipping stranded DAG step reconciliation for unreadable job"
                        );
                        continue;
                    }
                },
            };
            if let Err(e) = self.on_job_failed(&step.id, &reason).await {
                complete = false;
                warn!(
                    dag_id = %step.dag_id,
                    step_id = %step.step_id,
                    error = %e,
                    "Failed to fail DAG step whose job had already failed"
                );
            } else {
                warn!(
                    dag_id = %step.dag_id,
                    step_id = %step.step_id,
                    reason = %reason,
                    "Failed stranded DAG step during startup reconciliation"
                );
            }
        }

        for dag_id in self
            .dag_repository
            .list_dag_ids_with_unmaterialized_steps()
            .await?
        {
            match self.enqueue_now_ready_steps(&dag_id).await {
                Ok(update) => {
                    materialized_jobs = materialized_jobs.saturating_add(update.new_job_ids.len());
                }
                Err(e) => {
                    complete = false;
                    warn!(
                        dag_id = %dag_id,
                        error = %e,
                        "Failed to materialize jobs for ready DAG steps"
                    );
                }
            }
        }

        Ok((materialized_jobs, complete))
    }

    /// Materialize the ready steps of a DAG that no completion will reach:
    /// after a retry reset and during startup recovery.
    pub(super) async fn enqueue_now_ready_steps(
        &self,
        dag_id: &str,
    ) -> Result<DagJobCompletedUpdate> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        let dag_def = dag
            .get_dag_definition()
            .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
        let ready = self.dag_repository.list_ready_steps(dag_id).await?;

        // The retry entry point only knows the dag_id, so the placeholder
        // metadata for jobs created here is recovered from a sibling step's
        // job row: create_step_job persisted streamer_name / session_title /
        // platform / session_start_ms in every job's state JSON. Relying on
        // the dequeue-time backfill (JobQueue::resolve_job_metadata) instead
        // would lose session_start whenever the live_sessions row was deleted
        // before the retry.
        let placeholder_meta = if ready.is_empty() {
            JobStateMeta::default()
        } else {
            let steps = self.dag_repository.get_steps_by_dag(dag_id).await?;
            self.recover_placeholder_metadata(&steps).await
        };

        let (new_job_ids, dag) = self
            .materialize_ready_steps(dag, &dag_def, ready, &placeholder_meta)
            .await
            .map_err(|(_, error)| error)?;
        let completion = self
            .completion_info(
                &dag,
                dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await;

        Ok(DagJobCompletedUpdate {
            new_job_ids,
            completion,
        })
    }

    /// Recover the placeholder metadata persisted by [`Self::create_step_job`]
    /// in the job `state` JSON from the first sibling step that has a job row
    /// with any of those values set. Returns all-`None` when no step has a
    /// job yet or none carries metadata.
    pub(super) async fn recover_placeholder_metadata(
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

            // Siblings can carry an all-null state; only a row with stored
            // values can supply placeholder metadata for recovered steps.
            let meta = parse_job_state(&job.state);
            if meta.has_any() {
                return meta;
            }
        }

        JobStateMeta::default()
    }
}
