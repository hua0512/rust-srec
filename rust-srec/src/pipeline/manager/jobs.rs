use super::*;

impl<CR, SR> PipelineManager<CR, SR>
where
    CR: ConfigRepository + Send + Sync + 'static,
    SR: StreamerRepository + Send + Sync + 'static,
{
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();
        let streamer_id = job.streamer_id.clone();

        self.job_queue.enqueue(job).await?;

        // Emit event
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job_id.clone(),
            job_type,
            streamer_id,
        });

        // Check queue depth
        self.check_queue_depth();

        Ok(job_id)
    }

    /// Check queue depth and emit warnings.
    pub(super) fn check_queue_depth(&self) {
        let depth = self.job_queue.depth();
        let status = self.job_queue.depth_status();

        let status_code = match status {
            QueueDepthStatus::Normal => 0,
            QueueDepthStatus::Warning => 1,
            QueueDepthStatus::Critical => 2,
        };

        let prev = self.last_queue_status.load(Ordering::Relaxed);
        if prev == status_code {
            return;
        }
        self.last_queue_status.store(status_code, Ordering::Relaxed);

        match status {
            QueueDepthStatus::Critical => {
                warn!("Queue depth critical: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueCritical { depth });
            }
            QueueDepthStatus::Warning => {
                warn!("Queue depth warning: {} jobs", depth);
                let _ = self.event_tx.send(PipelineEvent::QueueWarning { depth });
            }
            QueueDepthStatus::Normal => {}
        }
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> usize {
        self.job_queue.depth()
    }

    /// Get the queue depth status.
    pub fn queue_status(&self) -> QueueDepthStatus {
        self.job_queue.depth_status()
    }

    /// Check if throttling should be enabled.
    pub fn should_throttle(&self) -> bool {
        self.config.throttle.enabled && self.job_queue.is_critical()
    }

    // ========================================================================
    // Query and Management Methods
    // ========================================================================

    /// List jobs with filters and pagination.
    /// Delegates to JobQueue/JobRepository.
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        self.job_queue.list_jobs(filters, pagination).await
    }

    /// List jobs with filters and pagination, without running a total `COUNT(*)`.
    pub async fn list_jobs_page(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<Job>> {
        self.job_queue.list_jobs_page(filters, pagination).await
    }

    /// List job execution logs (paged).
    pub async fn list_job_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobLogEntry>, u64)> {
        self.job_queue.list_job_logs(job_id, pagination).await
    }

    /// Get latest execution progress snapshot for a job (if available).
    pub async fn get_job_progress(&self, job_id: &str) -> Result<Option<JobProgressSnapshot>> {
        self.job_queue.get_job_progress(job_id).await
    }

    /// In-flight upload jobs with their latest progress snapshots.
    /// Feeds the WebSocket `DownloadSnapshot.uploads` slice.
    pub async fn list_active_uploads(&self) -> Result<Vec<crate::pipeline::ActiveUploadInfo>> {
        self.job_queue.list_active_uploads().await
    }

    /// Get a job by ID.
    /// Retrieves job from repository.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.job_queue.get_job(id).await
    }

    /// Retry a standalone job. Workflow jobs must be retried through `retry_dag`
    /// so every failed/cancelled branch participates in the same operation.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        let job_snapshot = self
            .job_queue
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        if job_snapshot.status != JobStatus::Failed && job_snapshot.status != JobStatus::Cancelled {
            return Err(Error::InvalidStateTransition {
                from: job_snapshot.status.as_str().to_string(),
                to: "PENDING".to_string(),
            });
        }

        if job_snapshot.dag_step_execution_id.is_some() {
            return Err(Error::Validation(
                "This job belongs to a workflow; retry the workflow so its cancelled branches restart together".to_string(),
            ));
        }
        self.retry_queued_job(id).await
    }

    async fn retry_queued_job(&self, id: &str) -> Result<Job> {
        let job = self.job_queue.retry_job(id).await?;

        // Emit event for the retried job
        let _ = self.event_tx.send(PipelineEvent::JobEnqueued {
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            streamer_id: job.streamer_id.clone(),
        });

        // Check queue depth after retry
        self.check_queue_depth();

        Ok(job)
    }

    /// Retry every failed or cancelled branch of a terminal workflow.
    /// All callers use this path so resetting step rows is paired with retrying
    /// their jobs and reconciling results that finished during cancellation.
    pub async fn retry_dag(&self, dag_id: &str) -> Result<DagRetryResult> {
        let dag_scheduler = self
            .dag_scheduler()
            .ok_or_else(|| Error::Validation("DAG scheduler not available".to_string()))?;

        // Get DAG execution
        let dag = dag_scheduler.get_dag_status(dag_id).await?;

        if dag.status != "FAILED" && dag.status != "CANCELLED" {
            return Err(Error::Validation(
                "DAG is not in FAILED or CANCELLED status".to_string(),
            ));
        }

        // Get all steps
        let steps = dag_scheduler.get_dag_steps(dag_id).await?;

        // Find retryable steps (failed steps + cancelled steps with an existing job).
        // Cancelled steps with a job_id typically represent fail-fast cancelled in-flight work.
        let retryable_steps: Vec<_> = steps
            .iter()
            .filter(|s| matches!(s.status.as_str(), "FAILED" | "CANCELLED") && s.job_id.is_some())
            .collect();

        if retryable_steps.is_empty() {
            return Err(Error::Validation(
                "No failed or cancelled steps found to retry".to_string(),
            ));
        }

        // Prepare DAG for retry so downstream steps can be scheduled again.
        dag_scheduler.reset_dag_for_retry(dag_id).await?;

        let mut job_ids = Vec::new();
        let mut reconciled_steps = 0usize;
        for step in &retryable_steps {
            let Some(job_id) = &step.job_id else {
                continue;
            };

            let job = match self.get_job(job_id).await {
                Ok(Some(job)) => job,
                Ok(None) => {
                    tracing::warn!("Failed to retry job {}: job not found", job_id);
                    continue;
                }
                Err(e) => {
                    tracing::warn!("Failed to load job {} for DAG retry: {}", job_id, e);
                    continue;
                }
            };

            match job.status {
                JobStatus::Failed | JobStatus::Cancelled => {
                    match self.retry_queued_job(job_id).await {
                        Ok(job) => job_ids.push(job.id),
                        Err(e) => tracing::warn!("Failed to retry job {}: {}", job_id, e),
                    }
                }
                JobStatus::Completed => {
                    if let Err(e) = dag_scheduler
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
                        tracing::warn!(
                            "Failed to reconcile completed job {} for DAG step {}: {}",
                            job_id,
                            step.id,
                            e
                        );
                    } else {
                        reconciled_steps += 1;
                    }
                }
                _ => {
                    tracing::debug!(
                        "Skipping DAG retry for job {} in status {:?}",
                        job_id,
                        job.status
                    );
                }
            }
        }

        let retried_steps = job_ids.len();
        let message = if retried_steps == retryable_steps.len() {
            format!("Successfully retried {} steps", retried_steps)
        } else {
            format!(
                "Retried {} of {} steps (reconciled {} already-completed steps)",
                retried_steps,
                retryable_steps.len(),
                reconciled_steps
            )
        };

        Ok(DagRetryResult {
            dag_id: dag_id.to_string(),
            retried_steps,
            job_ids,
            message,
        })
    }

    /// Cancel a job.
    /// Returns error for Completed/Failed jobs.
    /// Delegates to JobQueue.
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
        let job_snapshot = self
            .job_queue
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        if matches!(
            job_snapshot.status,
            JobStatus::Completed | JobStatus::Failed
        ) {
            return Err(Error::InvalidStateTransition {
                from: job_snapshot.status.as_str().to_string(),
                to: JobStatus::Cancelled.as_str().to_string(),
            });
        }

        let parent_dag = if let Some(step_exec_id) = job_snapshot.dag_step_execution_id.as_deref() {
            let dag_scheduler = self.dag_scheduler.as_ref().ok_or_else(|| {
                Error::Validation(
                    "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
                )
            })?;
            let dag_id = match job_snapshot.pipeline_id {
                Some(dag_id) => dag_id,
                None => dag_scheduler.get_step_execution(step_exec_id).await?.dag_id,
            };
            Some((dag_scheduler, dag_id))
        } else {
            None
        };

        // Cancelling only a step job leaves its parent DAG processing forever. Cancel the DAG
        // first so a repository failure cannot strand it after the job becomes terminal.
        if let Some((dag_scheduler, dag_id)) = parent_dag
            && let Err(cancel_error) = self.cancel_dag(&dag_id).await
        {
            let dag = dag_scheduler.get_dag_status(&dag_id).await?;
            if !dag.get_status().is_some_and(|status| status.is_terminal()) {
                return Err(cancel_error);
            }
        }

        let cancelled_job = self.job_queue.cancel_job(id).await?;

        let _ = self.event_tx.send(PipelineEvent::JobFailed {
            job_id: cancelled_job.id.clone(),
            job_type: cancelled_job.job_type.clone(),
            error: "Job cancelled".to_string(),
        });

        Ok(())
    }

    /// Delete a job.
    /// Removes from database and cache.
    /// Delegates to JobQueue.
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        self.job_queue.delete_job(id).await
    }

    /// Cancel a pipeline: the `dag_execution` row when `pipeline_id` names one, plus every pending
    /// or processing job carrying that `pipeline_id`.
    ///
    /// Returns the number of jobs cancelled. An id that matches nothing, and a DAG that is already
    /// terminal, both report zero rather than failing.
    pub async fn cancel_pipeline(&self, pipeline_id: &str) -> Result<usize> {
        // `DagScheduler::build_step_job` stores the DAG id in every step job's `pipeline_id`, so a
        // pipeline id naming a `dag_execution` row is stood down through `cancel_dag` first:
        // `JobQueue::cancel_pipeline` only touches job rows, which would leave `dag_execution` and
        // `dag_step_execution` in `PROCESSING` and skip `handle_dag_completion`.
        let dag_cancelled = self.cancel_pipeline_as_dag(pipeline_id).await?;

        // The sweep runs even after `cancel_dag`, which reaches jobs only through
        // `dag_step_execution.job_id`: a job row carrying this `pipeline_id` whose step holds no
        // `job_id` — the shape `list_processing_steps_with_completed_jobs` reverse-links through
        // `job.dag_step_execution_id` — would otherwise keep running with its cancellation token
        // unfired. Jobs `cancel_dag` cancelled are terminal by now, so `cancel_jobs_by_pipeline`
        // and the `jobs_cache` scan skip them: each job is counted and announced at most once.
        let cancelled_jobs = self.job_queue.cancel_pipeline(pipeline_id).await?;

        // Emit events for each cancelled job
        for job in &cancelled_jobs {
            let _ = self.event_tx.send(PipelineEvent::JobFailed {
                job_id: job.id.clone(),
                job_type: job.job_type.clone(),
                error: "Pipeline cancelled".to_string(),
            });
        }

        Ok(dag_cancelled.unwrap_or(0) + cancelled_jobs.len())
    }

    /// Cancel `pipeline_id` as a DAG execution, returning the number of step jobs cancelled.
    ///
    /// `None` means `pipeline_id` names no DAG that still needs standing down: no DAG scheduler is
    /// configured, `DagScheduler::get_dag_status` finds no such `dag_execution` row, the row is
    /// already terminal, or it turned terminal or was deleted between the status read and the
    /// cancel. The caller sweeps job rows by `pipeline_id` in every case, including this one.
    async fn cancel_pipeline_as_dag(&self, pipeline_id: &str) -> Result<Option<usize>> {
        let Some(dag_scheduler) = &self.dag_scheduler else {
            return Ok(None);
        };

        match dag_scheduler.get_dag_status(pipeline_id).await {
            Ok(dag) if dag.get_status().is_some_and(|status| status.is_terminal()) => Ok(None),
            Ok(_) => match self.cancel_dag(pipeline_id).await {
                Ok(cancelled) => Ok(Some(cancelled as usize)),
                // `DagScheduler::cancel_dag_with_completion` re-reads the row, so it rejects a DAG
                // that turned terminal or that `DagScheduler::delete_dag` removed in the meantime.
                // Neither leaves anything to stand down.
                Err(Error::DagAlreadyTerminal { .. } | Error::NotFound { .. }) => Ok(None),
                Err(error) => Err(error),
            },
            Err(Error::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// List available job presets.
    pub async fn list_presets(&self) -> Result<Vec<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets().await
        } else {
            Ok(vec![])
        }
    }

    /// List job presets filtered by category.
    pub async fn list_presets_by_category(
        &self,
        category: Option<&str>,
    ) -> Result<Vec<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets_by_category(category).await
        } else {
            Ok(vec![])
        }
    }

    /// List job presets with filtering, searching, and pagination.
    pub async fn list_presets_filtered(
        &self,
        filters: &crate::database::repositories::JobPresetFilters,
        pagination: &crate::database::models::Pagination,
    ) -> Result<(Vec<crate::database::models::JobPreset>, u64)> {
        if let Some(repo) = &self.preset_repo {
            repo.list_presets_filtered(filters, pagination).await
        } else {
            Ok((vec![], 0))
        }
    }

    /// List all unique preset categories.
    pub async fn list_preset_categories(&self) -> Result<Vec<String>> {
        if let Some(repo) = &self.preset_repo {
            repo.list_categories().await
        } else {
            Ok(vec![])
        }
    }

    /// Get a job preset by ID.
    pub async fn get_preset(&self, id: &str) -> Result<Option<crate::database::models::JobPreset>> {
        if let Some(repo) = &self.preset_repo {
            repo.get_preset(id).await
        } else {
            Ok(None)
        }
    }

    /// Check if a preset name exists (optionally excluding a specific ID).
    pub async fn name_exists(&self, name: &str, exclude_id: Option<&str>) -> Result<bool> {
        if let Some(repo) = &self.preset_repo {
            repo.name_exists(name, exclude_id).await
        } else {
            Ok(false)
        }
    }

    /// Create a new job preset.
    pub async fn create_preset(&self, preset: &crate::database::models::JobPreset) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.create_preset(preset).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Update an existing job preset.
    pub async fn update_preset(&self, preset: &crate::database::models::JobPreset) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.update_preset(preset).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Delete a job preset.
    pub async fn delete_preset(&self, id: &str) -> Result<()> {
        if let Some(repo) = &self.preset_repo {
            repo.delete_preset(id).await
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Clone an existing job preset with a new name.
    ///
    /// Creates a copy of the preset with a new ID and name.
    /// The new name must be unique.
    pub async fn clone_preset(
        &self,
        source_id: &str,
        new_name: String,
    ) -> Result<crate::database::models::JobPreset> {
        if let Some(repo) = &self.preset_repo {
            // Get the source preset
            let source =
                repo.get_preset(source_id)
                    .await?
                    .ok_or_else(|| crate::Error::NotFound {
                        entity_type: "Preset".to_string(),
                        id: source_id.to_string(),
                    })?;

            // Check if the new name already exists
            if repo.name_exists(&new_name, None).await? {
                return Err(crate::Error::Validation(format!(
                    "A preset with name '{}' already exists",
                    new_name
                )));
            }

            // Create the cloned preset with a new ID
            let cloned = crate::database::models::JobPreset {
                id: uuid::Uuid::new_v4().to_string(),
                name: new_name,
                description: source.description.map(|d| format!("Copy of: {}", d)),
                category: source.category,
                processor: source.processor,
                config: source.config,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            };

            repo.create_preset(&cloned).await?;
            Ok(cloned)
        } else {
            Err(crate::Error::Validation(
                "Presets not supported (no repository)".to_string(),
            ))
        }
    }

    /// Get comprehensive pipeline statistics.
    /// Returns counts by status (pending, processing, completed, failed)
    /// and average processing time.
    pub async fn get_stats(&self) -> Result<PipelineStats> {
        let job_stats = self.job_queue.get_stats().await?;

        Ok(PipelineStats {
            pending: job_stats.pending,
            processing: job_stats.processing,
            completed: job_stats.completed,
            failed: job_stats.failed,
            cancelled: job_stats.cancelled,
            avg_processing_time_secs: job_stats.avg_processing_time_secs,
            queue_depth: self.queue_depth(),
            queue_status: self.queue_status(),
        })
    }
}
