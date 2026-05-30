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

    /// Get a job by ID.
    /// Retrieves job from repository.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        self.job_queue.get_job(id).await
    }

    /// Retry a failed job.
    /// Delegates to JobQueue.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        // If this is a DAG step job, retrying the underlying job is not enough: the parent DAG
        // must be reset to a non-terminal state and the step execution must be marked active
        // again so downstream steps can be scheduled when the job completes.
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

        if let Some(step_exec_id) = job_snapshot.dag_step_execution_id.as_deref() {
            let Some(dag_scheduler) = &self.dag_scheduler else {
                return Err(Error::Validation(
                    "DAG scheduler not configured. Call with_dag_repository() first.".to_string(),
                ));
            };

            let dag_id = match job_snapshot.pipeline_id.as_deref() {
                Some(existing_dag_id) => existing_dag_id.to_string(),
                None => dag_scheduler.get_step_execution(step_exec_id).await?.dag_id,
            };

            let dag = dag_scheduler.get_dag_status(&dag_id).await?;
            if matches!(
                dag.get_status(),
                Some(crate::database::models::DagExecutionStatus::Failed)
                    | Some(crate::database::models::DagExecutionStatus::Cancelled)
            ) {
                dag_scheduler.reset_dag_for_retry(&dag_id).await?;
            }
        }

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

    /// Cancel a job.
    /// Returns error for Completed/Failed jobs.
    /// Delegates to JobQueue.
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
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

    /// Cancel all jobs in a pipeline.
    /// Cancels all pending and processing jobs that belong to the specified pipeline.
    /// Returns the number of jobs cancelled.
    pub async fn cancel_pipeline(&self, pipeline_id: &str) -> Result<usize> {
        let cancelled_jobs = self.job_queue.cancel_pipeline(pipeline_id).await?;

        // Emit events for each cancelled job
        for job in &cancelled_jobs {
            let _ = self.event_tx.send(PipelineEvent::JobFailed {
                job_id: job.id.clone(),
                job_type: job.job_type.clone(),
                error: "Pipeline cancelled".to_string(),
            });
        }

        Ok(cancelled_jobs.len())
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
