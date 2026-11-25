//! DAG step completion, failure propagation, retry planning, and cancellation.

use std::collections::{HashMap, HashSet};
use std::future::Future;

use jiff::Timestamp;
use tracing::{debug, error, info, warn};

use super::{
    DagCompletionInfo, DagJobCompletedUpdate, DagJobFailedUpdate, DagScheduler, FailedStepAttempt,
    LeafOutputs, PlannedRetry, StepFailureOutcome, StepRetryState,
};
use crate::database::models::{
    DagExecutionDbModel, DagExecutionStatus, DagPipelineDefinition, DagStepExecutionDbModel,
};
use crate::pipeline::JobFailureOutcome;
use crate::pipeline::job_queue::JobStateMeta;
use crate::{Error, Result};

impl DagScheduler {
    pub(crate) fn collect_leaf_outputs_from_step_executions(
        def: &DagPipelineDefinition,
        step_execs: &[DagStepExecutionDbModel],
    ) -> LeafOutputs {
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
        let mut complete = true;

        for leaf in def.leaf_steps() {
            let Some(exec) = exec_by_step_id.get(leaf.id.as_str()) else {
                complete = false;
                continue;
            };
            let step_outputs = match exec.outputs.as_deref() {
                None => Vec::new(),
                Some(raw) => match serde_json::from_str::<Vec<String>>(raw) {
                    Ok(outputs) => outputs,
                    Err(_) => {
                        complete = false;
                        // Keep the model's contextual diagnostic and empty fallback.
                        exec.get_outputs()
                    }
                },
            };
            for output in step_outputs {
                if seen.insert(crate::utils::fs::path_dedup_key(&output)) {
                    outputs.push(output);
                }
            }
        }

        LeafOutputs {
            paths: outputs,
            complete,
        }
    }

    pub(super) async fn collect_leaf_outputs(
        &self,
        dag: &DagExecutionDbModel,
    ) -> Result<Vec<String>> {
        let Some(def) = dag.get_dag_definition() else {
            return Ok(Vec::new());
        };

        if def.leaf_steps().is_empty() {
            return Ok(Vec::new());
        }

        let step_execs = self.dag_repository.get_steps_by_dag(&dag.id).await?;
        Ok(Self::collect_leaf_outputs_from_step_executions(&def, &step_execs).paths)
    }

    /// Terminal notifications share one best-effort artifact projection. The
    /// caller supplies success according to the transition it is reporting.
    pub(super) async fn completion_info(
        &self,
        dag: &DagExecutionDbModel,
        succeeded: bool,
    ) -> Option<DagCompletionInfo> {
        if !dag.get_status().is_some_and(|status| status.is_terminal()) {
            return None;
        }
        let leaf_outputs = self
            .collect_leaf_outputs(dag)
            .await
            .unwrap_or_else(|error| {
                warn!(
                    dag_id = %dag.id,
                    error = %error,
                    "Failed to collect DAG leaf outputs; reporting completion without them"
                );
                Vec::new()
            });
        Some(DagCompletionInfo {
            dag_id: dag.id.clone(),
            streamer_id: dag.streamer_id.clone(),
            session_id: dag.session_id.clone(),
            succeeded,
            leaf_outputs,
        })
    }

    pub(super) async fn fail_dag_internal(
        &self,
        dag_id: &str,
        error: &str,
    ) -> Result<Option<DagCompletionInfo>> {
        let cancelled = self
            .dag_repository
            .fail_dag_and_cancel_steps(dag_id, error)
            .await?;
        for job_id in &cancelled {
            if let Err(cancel_error) = self.job_queue.cancel_job(job_id).await {
                warn!(
                    job_id,
                    error = %cancel_error,
                    "Failed to cancel job while failing DAG"
                );
            }
        }

        let dag = self.dag_repository.get_dag(dag_id).await?;
        Ok(self
            .completion_info(
                &dag,
                dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await)
    }

    /// Fail a DAG that is not terminal, cancelling its remaining steps and jobs.
    /// Returns the completion to forward when the DAG became terminal through
    /// this call; a DAG that was already terminal is left as it is and yields
    /// no completion, so its earlier outcome is not replayed.
    pub async fn fail_dag(&self, dag_id: &str, error: &str) -> Result<Option<DagCompletionInfo>> {
        let dag = self.dag_repository.get_dag(dag_id).await?;
        if dag.get_status().is_some_and(|status| status.is_terminal()) {
            return Ok(None);
        }
        self.fail_dag_internal(dag_id, error).await
    }

    /// Fail the DAG of a step whose workflow cannot advance.
    async fn fail_dag_for_step(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<Option<DagCompletionInfo>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;
        self.fail_dag_internal(&step.dag_id, error).await
    }

    /// Advance the workflow after a step's job completed: the worker's entry
    /// point for a job outcome, wrapping [`Self::on_job_completed`]. When the
    /// workflow cannot advance from the result, it is failed with that error
    /// so it settles instead of waiting for a completion that will not come
    /// again; the returned update then carries the failure's completion.
    pub async fn on_job_attempt_completed(
        &self,
        dag_step_execution_id: &str,
        outputs: &[String],
        streamer_name: Option<&str>,
        session_title: Option<&str>,
        platform: Option<&str>,
        session_start: Option<Timestamp>,
    ) -> Result<DagJobCompletedUpdate> {
        match self
            .on_job_completed(
                dag_step_execution_id,
                outputs,
                streamer_name,
                session_title,
                platform,
                session_start,
            )
            .await
        {
            Ok(update) => Ok(update),
            Err(error) => {
                error!(
                    dag_step_execution_id,
                    %error,
                    "Failed to advance the workflow after a step completed; failing it"
                );
                let completion = self
                    .fail_dag_for_step(
                        dag_step_execution_id,
                        &format!("DAG scheduler error: {error}"),
                    )
                    .await?;
                Ok(DagJobCompletedUpdate {
                    new_job_ids: Vec::new(),
                    completion,
                })
            }
        }
    }

    /// Apply a failed attempt of a step's job to its workflow: the worker's
    /// entry point for a job outcome.
    ///
    /// Decides first whether the step's retry policy grants another attempt,
    /// then has `persist_failure` record the attempt on the job row in one
    /// write: the error together with the planned retry, if any, so neither a
    /// reader nor a restart sees the failure without its retry. With the row
    /// recorded, a planned retry is left to the manager's sweeper and the step
    /// stays PROCESSING; otherwise the failure reaches the step and its
    /// dependents through [`Self::on_job_failed`].
    /// A job failure that could not be persisted leaves the step active: the worker
    /// retains ownership until persistence succeeds or shutdown interrupts it.
    pub async fn on_job_attempt_failed<F, Fut>(
        &self,
        dag_step_execution_id: &str,
        attempt: FailedStepAttempt<'_>,
        persist_failure: F,
    ) -> Result<StepFailureOutcome>
    where
        F: FnOnce(Option<PlannedRetry>) -> Fut,
        Fut: Future<Output = Result<JobFailureOutcome>>,
    {
        let planned = if attempt.kind.retryable() {
            self.plan_step_retry(dag_step_execution_id, attempt.retry_count)
                .await
                .unwrap_or_else(|error| {
                    warn!(
                        job_id = %attempt.job_id,
                        %error,
                        "Could not plan a step retry; failing the step"
                    );
                    None
                })
        } else {
            None
        };

        let recorded = persist_failure(planned).await?;
        if recorded == JobFailureOutcome::Unchanged {
            return Ok(StepFailureOutcome::Ignored);
        }
        if let Some(planned) = planned {
            info!(
                job_id = %attempt.job_id,
                dag_step_execution_id,
                next_attempt = planned.next_attempt,
                max_attempts = planned.max_attempts,
                retry_after = %planned.retry_after,
                "Step failed; automatic retry scheduled"
            );
            return Ok(StepFailureOutcome::Retried(planned));
        }

        Ok(
            match self
                .apply_step_failure(dag_step_execution_id, attempt.error)
                .await?
            {
                Some(update) => StepFailureOutcome::Failed(update),
                None => StepFailureOutcome::Ignored,
            },
        )
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
        session_start: Option<Timestamp>,
    ) -> Result<DagJobCompletedUpdate> {
        let streamer_name = streamer_name.map(ToString::to_string);
        let session_title = session_title.map(ToString::to_string);
        let platform = platform.map(ToString::to_string);

        let crate::database::repositories::StepCompletion {
            step,
            dag,
            ready_steps,
        } = self
            .dag_repository
            .complete_step_and_check_dependents(dag_step_execution_id, outputs)
            .await?;
        info!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            outputs_count = %outputs.len(),
            "DAG step completed"
        );

        let mut new_job_ids = Vec::new();

        if !ready_steps.is_empty() {
            // Definition and metadata come from the same transaction as readiness.
            let dag_def = dag
                .get_dag_definition()
                .ok_or_else(|| Error::Validation("Failed to parse DAG definition".into()))?;
            let meta = JobStateMeta {
                streamer_name,
                session_title,
                platform,
                session_start,
                manifest: None,
                timeout_secs: None,
                has_step_retry: None,
                enqueued_at_ms: None,
            };

            match self
                .materialize_ready_steps(dag, &dag_def, ready_steps, &meta)
                .await
            {
                Ok((job_ids, _)) => new_job_ids = job_ids,
                Err((job_ids, e)) => {
                    let err_msg = format!("Failed to create downstream DAG job: {}", e);
                    warn!(
                        dag_id = %step.dag_id,
                        error = %err_msg,
                        "Failing DAG due to scheduler error"
                    );
                    let completion = self.fail_dag_internal(&step.dag_id, &err_msg).await?;
                    return Ok(DagJobCompletedUpdate {
                        new_job_ids: job_ids,
                        completion,
                    });
                }
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
        let completion = self
            .completion_info(
                &updated_dag,
                updated_dag.get_status() == Some(DagExecutionStatus::Completed),
            )
            .await;

        Ok(DagJobCompletedUpdate {
            new_job_ids,
            completion,
        })
    }

    /// Handle job failure for a DAG step.
    ///
    /// Fail-fast is scoped to the failed step's descendants:
    /// 1. Marks the step as failed and records the error on the DAG
    /// 2. Cancels the steps that depend on it, directly or indirectly
    /// 3. Signals cancellation for the cancelled steps' jobs
    /// 4. Finalizes the DAG as FAILED once no step is active; independent
    ///    branches keep running and the last one to settle finalizes it
    ///
    /// Returns the count of cancelled jobs and, when the DAG became terminal
    /// through this failure, its completion. A failure reported for a step
    /// that has already settled changes nothing and returns an empty update.
    pub async fn on_job_failed(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<DagJobFailedUpdate> {
        Ok(self
            .apply_step_failure(dag_step_execution_id, error)
            .await?
            .unwrap_or(DagJobFailedUpdate {
                cancelled_count: 0,
                completion: None,
            }))
    }

    /// [`Self::on_job_failed`], returning `None` for a step that had already
    /// settled.
    pub(super) async fn apply_step_failure(
        &self,
        dag_step_execution_id: &str,
        error: &str,
    ) -> Result<Option<DagJobFailedUpdate>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;

        let failure_error = format!("Step '{}' failed: {}", step.step_id, error);
        let Some(processing_job_ids) = self
            .dag_repository
            .fail_step_and_cancel_dag(dag_step_execution_id, &failure_error)
            .await?
        else {
            debug!(
                dag_id = %step.dag_id,
                step_id = %step.step_id,
                "Ignoring failure for terminal DAG step"
            );
            return Ok(None);
        };

        error!(
            dag_id = %step.dag_id,
            step_id = %step.step_id,
            error = %error,
            "DAG step failed; cancelling the steps that depend on it"
        );

        // Cancel the jobs of the cancelled dependents.
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

        let updated_dag = match self.dag_repository.get_dag(&step.dag_id).await {
            Ok(dag) => Some(dag),
            Err(error) => {
                warn!(dag_id = %step.dag_id, %error, "Failed to reload failed DAG");
                None
            }
        };
        let completion = match &updated_dag {
            Some(dag) => self.completion_info(dag, false).await,
            None => None,
        };
        if completion.is_some() {
            info!(
                dag_id = %step.dag_id,
                cancelled_jobs = %cancelled_count,
                "DAG failed; no step is still running"
            );
        } else {
            info!(
                dag_id = %step.dag_id,
                cancelled_jobs = %cancelled_count,
                "DAG step failed; independent branches keep running until they settle"
            );
        }

        Ok(Some(DagJobFailedUpdate {
            cancelled_count,
            completion,
        }))
    }

    /// Whether the failed job of `dag_step_execution_id` gets another attempt
    /// under its step's retry policy, and when. `retry_count` is the number of
    /// retries the job has already had. `None` when the step has no policy,
    /// the budget is spent, or the step or its DAG is no longer active, in
    /// which case the failure is applied to the workflow as usual.
    pub async fn plan_step_retry(
        &self,
        dag_step_execution_id: &str,
        retry_count: i32,
    ) -> Result<Option<PlannedRetry>> {
        let step = self.dag_repository.get_step(dag_step_execution_id).await?;
        if step.get_status() != Some(crate::database::models::DagStepStatus::Processing) {
            return Ok(None);
        }
        let dag = self.dag_repository.get_dag(&step.dag_id).await?;
        if dag.get_status().is_some_and(|status| status.is_terminal()) {
            return Ok(None);
        }
        let Some(policy) = dag
            .get_dag_definition()
            .and_then(|definition| definition.get_step(&step.step_id)?.retry.clone())
        else {
            return Ok(None);
        };
        let attempts_made = u32::try_from(retry_count)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        if attempts_made >= policy.max_attempts {
            return Ok(None);
        }
        let next_attempt = attempts_made.saturating_add(1);
        let wait = jiff::SignedDuration::try_from(policy.backoff_before(next_attempt))
            .unwrap_or_else(|_| {
                jiff::SignedDuration::from_secs(
                    crate::database::models::StepRetryPolicy::MAX_BACKOFF_SECS as i64,
                )
            });
        Ok(Some(PlannedRetry {
            retry_after: Timestamp::now() + wait,
            next_attempt,
            max_attempts: policy.max_attempts,
        }))
    }

    /// Whether `job_id` is still the attempt `dag_step_execution_id` is waiting
    /// on in a DAG that has not settled, so a scheduled retry of it is still
    /// wanted. A step that was cancelled, failed by recovery or deleted in the
    /// meantime answers `false`.
    pub async fn step_awaits_job(&self, dag_step_execution_id: &str, job_id: &str) -> Result<bool> {
        Ok(self
            .step_retry_state(dag_step_execution_id, job_id)
            .await?
            .awaits_job)
    }

    /// The retry budget of step `dag_step_execution_id` and whether it still
    /// awaits `job_id`, as [`Self::step_awaits_job`] decides.
    pub async fn step_retry_state(
        &self,
        dag_step_execution_id: &str,
        job_id: &str,
    ) -> Result<StepRetryState> {
        let step = match self.dag_repository.get_step(dag_step_execution_id).await {
            Ok(step) => step,
            Err(srec_sqlite::DbError::NotFound { .. }) => return Ok(StepRetryState::default()),
            Err(error) => return Err(error.into()),
        };
        let dag = match self.dag_repository.get_dag(&step.dag_id).await {
            Ok(dag) => dag,
            Err(srec_sqlite::DbError::NotFound { .. }) => return Ok(StepRetryState::default()),
            Err(error) => return Err(error.into()),
        };
        let max_attempts = dag
            .get_dag_definition()
            .and_then(|definition| definition.get_step(&step.step_id)?.retry.clone())
            .map(|policy| policy.max_attempts);
        Ok(StepRetryState {
            max_attempts,
            awaits_job: step.awaits_job(job_id, dag.get_status()),
        })
    }

    /// Cancel a DAG execution.
    ///
    /// The update counts the running steps the cancel stopped, a step whose job
    /// waits for an automatic retry included: such a FAILED job cannot be
    /// cancelled itself, but cancelling its step drops the retry.
    pub async fn cancel_dag_with_completion(&self, dag_id: &str) -> Result<DagJobFailedUpdate> {
        let dag = self.dag_repository.get_dag(dag_id).await?;

        if dag.get_status().map(|s| s.is_terminal()).unwrap_or(false) {
            return Err(Error::DagAlreadyTerminal {
                dag_id: dag_id.to_string(),
            });
        }

        let cancelled = self
            .dag_repository
            .cancel_dag_and_cancel_steps(dag_id, "Cancelled by user")
            .await?;

        for job_id in &cancelled {
            match self.job_queue.cancel_job(job_id).await {
                // A job that already settled, such as one waiting for its
                // retry, has nothing left to stop.
                Ok(_) | Err(Error::InvalidStateTransition { .. }) => {}
                Err(error) => warn!(dag_id, %job_id, %error, "Failed to cancel a step job"),
            }
        }

        let updated_dag = match self.dag_repository.get_dag(dag_id).await {
            Ok(dag) => Some(dag),
            Err(error) => {
                warn!(dag_id, %error, "Failed to reload cancelled DAG");
                None
            }
        };
        let completion = match updated_dag {
            Some(dag) => self.completion_info(&dag, false).await,
            None => None,
        };

        Ok(DagJobFailedUpdate {
            cancelled_count: cancelled.len() as u64,
            completion,
        })
    }

    /// Cancel a DAG execution.
    pub async fn cancel_dag(&self, dag_id: &str) -> Result<u64> {
        Ok(self
            .cancel_dag_with_completion(dag_id)
            .await?
            .cancelled_count)
    }
}
