//! Job enqueue, dequeue, completion, failure, retry, cancellation, and fan-out.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use jiff::Timestamp;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::logs_progress::{extend_logs_capped, update_log_summary};
use super::{
    AttemptToken, FailedUploadContext, FailureMetadata, Job, JobExecutionInfo, JobFailureOutcome,
    JobLogEntry, JobQueue, JobResult, LogPersistence, db_model_to_job, is_thumbnail_job_type,
    job_to_db_model, non_empty, upload_kind_for_job_type,
};
use crate::database::models::{JobFilters, JobStatus, Pagination};
use crate::database::repositories::JobRepository;
use crate::pipeline::processors::{UploadItemStatus, UploadResultItem};
use crate::pipeline::upload_events::{UploadStatusEvent, UploadTerminalStatus};
use crate::utils::json::{self, JsonContext};
use crate::{Error, Result};

impl JobQueue {
    /// Enqueue a new job.
    pub async fn enqueue(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();

        self.job_repository
            .create_job(&job_to_db_model(&job))
            .await?;

        // Add to in-memory cache
        self.jobs_cache.insert(job_id.clone(), job);

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued job {} of type {}", job_id, job_type);

        self.wake_workers();

        Ok(job_id)
    }

    /// Enqueue an existing job (already persisted to database).
    /// This adds the job to the in-memory cache and notifies workers.
    /// Used by DagScheduler when creating jobs for DAG steps.
    pub async fn enqueue_existing(&self, job: Job) -> Result<String> {
        let job_id = job.id.clone();
        let job_type = job.job_type.clone();

        // Add to in-memory cache (job is already in database)
        self.jobs_cache.insert(job_id.clone(), job);

        self.depth.fetch_add(1, Ordering::SeqCst);

        info!("Enqueued existing job {} of type {}", job_id, job_type);

        self.wake_workers();

        Ok(job_id)
    }

    /// Dequeue a job for processing.
    pub async fn dequeue(&self, job_types: Option<&[String]>) -> Result<Option<Job>> {
        let _attempt_lease = self.attempt_gate.read().await;
        // Note: This is called frequently by worker pools, so we use trace level
        // to avoid log spam. Use debug level only when a job is actually dequeued.

        let Some(db_job) = self
            .job_repository
            .claim_next_pending_job(job_types)
            .await?
        else {
            return Ok(None);
        };
        let mut job = db_model_to_job(&db_job);
        job.status = JobStatus::Processing;
        if job.started_at.is_none() {
            job.started_at = Some(Timestamp::now());
        }

        // Resolve missing metadata (streamer_name, session_title)
        // These are not stored in the database, so we look them up from repositories
        self.resolve_job_metadata(&mut job).await;

        // Update cache
        self.jobs_cache.insert(job.id.clone(), job.clone());

        // Create cancellation token for this job (do not overwrite an existing token,
        // e.g. if a cancellation raced with dequeue).
        let attempt = self
            .cancellation_tokens
            .entry(job.id.clone())
            .or_default()
            .id();

        self.announce_started(&job, attempt);
        Ok(Some(job))
    }

    /// Count pending jobs, optionally filtered by job types.
    pub async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64> {
        self.job_repository.count_pending_jobs(job_types).await
    }

    /// Mark a job as completed.
    pub async fn complete(&self, job_id: &str, result: JobResult) -> Result<()> {
        self.complete_if_processing_inner(job_id, None, result)
            .await?;
        Ok(())
    }

    /// Mark a specific worker attempt as completed and report whether the job
    /// left an active state. A cancelled or superseded attempt is rejected, so
    /// it cannot overwrite the attempt a retry has started since.
    pub(crate) async fn complete_attempt(
        &self,
        job_id: &str,
        attempt: &AttemptToken,
        result: JobResult,
    ) -> Result<bool> {
        self.complete_if_processing_inner(job_id, Some(attempt), result)
            .await
    }

    async fn complete_if_processing_inner(
        &self,
        job_id: &str,
        expected_attempt: Option<&AttemptToken>,
        result: JobResult,
    ) -> Result<bool> {
        let attempt_lease = self.attempt_gate.read().await;
        if let Some(attempt) = expected_attempt {
            let owns_active = self
                .cancellation_tokens
                .get(job_id)
                .is_some_and(|current| current.id() == attempt.id() && !current.is_cancelled());
            if !owns_active {
                drop(attempt_lease);
                self.finalize_attempt(job_id, attempt).await;
                return Ok(false);
            }
        }

        // Capture outputs for persistence before they are moved into cache/DB models.
        let outputs_for_persist = result.outputs.clone();
        let repo = &self.job_repository;
        let mut db_job = repo.get_job(job_id).await?;

        let completed_job_type = db_job.job_type.clone();
        let completed_session_id = db_job.session_id.clone();
        let completed_streamer_id = db_job.streamer_id.clone();

        if db_job.get_status() != Some(JobStatus::Processing) {
            if let Some(attempt) = expected_attempt {
                drop(attempt_lease);
                self.finalize_attempt(job_id, attempt).await;
            } else {
                self.finalize_cancelled_job(job_id);
            }
            return Ok(false);
        }

        db_job.mark_completed();
        if !result.outputs.is_empty() {
            db_job.set_outputs(&result.outputs);
        }
        db_job.duration_secs = Some(result.duration_secs);

        // Calculate queue wait time (DB stores epoch millis)
        if let (created_ms, Some(started_ms)) = (db_job.created_at, db_job.started_at) {
            let wait_ms = started_ms.saturating_sub(created_ms);
            db_job.queue_wait_secs = Some((wait_ms as f64 / 1000.0).max(0.0));
        }

        let updated = repo
            .update_job_if_status(&db_job, JobStatus::Processing)
            .await?;
        if updated == 0 {
            if let Some(attempt) = expected_attempt {
                drop(attempt_lease);
                self.finalize_attempt(job_id, attempt).await;
            } else {
                self.finalize_cancelled_job(job_id);
            }
            return Ok(false);
        }

        // The COMPLETED row above is the recovery boundary for DAG jobs. Execution logs, their
        // summary and the size figures are auxiliary observability writes and cannot make a
        // processor rerun.
        let new_logs = if result.logs.is_empty() {
            Vec::new()
        } else {
            match self
                .persist_logs_to_db(job_id, &result.logs, LogPersistence::Snapshot)
                .await
            {
                Ok(new_logs) => new_logs,
                Err(error) => {
                    warn!(
                        job_id = %job_id,
                        error = %error,
                        "Failed to persist completed job execution logs"
                    );
                    Vec::new()
                }
            }
        };
        let has_sizes = result.input_size_bytes.is_some() || result.output_size_bytes.is_some();
        if !new_logs.is_empty() || has_sizes {
            let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
                db_job.execution_info.as_deref(),
                JsonContext::JobField {
                    job_id: &db_job.id,
                    field: "execution_info",
                },
                "Invalid execution_info JSON; resetting to defaults",
            );
            update_log_summary(&mut exec_info, &new_logs);
            extend_logs_capped(&mut exec_info, &new_logs);
            if result.input_size_bytes.is_some() {
                exec_info.input_size_bytes = result.input_size_bytes;
            }
            if result.output_size_bytes.is_some() {
                exec_info.output_size_bytes = result.output_size_bytes;
            }
            match serde_json::to_string(&exec_info) {
                Ok(execution_info) => {
                    if let Err(error) = repo
                        .update_job_execution_info(job_id, &execution_info)
                        .await
                    {
                        warn!(
                            job_id = %job_id,
                            error = %error,
                            "Failed to persist completed job execution info"
                        );
                    }
                }
                Err(error) => {
                    warn!(
                        job_id = %job_id,
                        error = %error,
                        "Failed to serialize completed job execution info"
                    );
                }
            }
        }

        // Persist thumbnail outputs to the session media_outputs table.
        //
        // This covers:
        // - Direct thumbnail jobs (job_type == "thumbnail")
        // - Preset-driven DAG steps (job_type == "thumbnail_<preset>")
        //
        // Segment persistence is handled by the download manager event path.
        if is_thumbnail_job_type(&completed_job_type)
            && let Some(session_id) = completed_session_id.as_deref()
        {
            for output_path in &outputs_for_persist {
                self.persist_thumbnail_output(session_id, output_path).await;
            }
        }

        // Persist per-file upload results and broadcast the terminal event
        // for upload job types (persist_upload_records itself no-ops for
        // everything else).
        if upload_kind_for_job_type(&completed_job_type).is_some() {
            self.persist_upload_records(
                job_id,
                &completed_job_type,
                completed_streamer_id.as_deref(),
                completed_session_id.as_deref(),
                &result.uploads,
            )
            .await;

            let (mut succeeded, mut failed, mut skipped) = (0u32, 0u32, 0u32);
            for item in &result.uploads {
                match item.status {
                    UploadItemStatus::Completed => succeeded += 1,
                    UploadItemStatus::Failed => failed += 1,
                    UploadItemStatus::Skipped => skipped += 1,
                }
            }
            self.emit_upload_event(UploadStatusEvent::Terminal {
                job_id: job_id.to_string(),
                streamer_id: completed_streamer_id.clone(),
                status: UploadTerminalStatus::Completed,
                files_succeeded: succeeded,
                files_failed: failed,
                files_skipped: skipped,
                error: None,
            });
        }
        self.jobs_cache.remove(job_id);
        self.finish_long_job_tracking(job_id, true);

        // Cleanup in-memory tracking for this job.
        let _ = self.cancellation_tokens.remove_if(job_id, |_, current| {
            expected_attempt.is_none_or(|attempt| current.id() == attempt.id())
        });
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);

        self.decrement_depth(1);
        info!("Job {} completed in {:.2}s", job_id, result.duration_secs);

        Ok(true)
    }
    /// Mark a job as failed.
    pub async fn fail(&self, job_id: &str, error: &str) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                None,
                error,
                None,
                None,
                FailureMetadata::DatabaseLog,
                &[],
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(job_id, error, "Job failed");
        }
        Ok(outcome)
    }

    pub(crate) async fn fail_attempt(
        &self,
        job_id: &str,
        attempt: &AttemptToken,
        error: &str,
    ) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                Some(attempt),
                error,
                None,
                None,
                FailureMetadata::DatabaseLog,
                &[],
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(job_id, error, "Job failed");
        }
        Ok(outcome)
    }

    /// Fail before processor invocation without rewriting unavailable or invalid metadata.
    pub async fn fail_execution_start(
        &self,
        job_id: &str,
        error: &str,
    ) -> Result<JobFailureOutcome> {
        self.fail_internal(
            job_id,
            None,
            error,
            None,
            None,
            FailureMetadata::Preserve,
            &[],
        )
        .await
    }

    pub(crate) async fn fail_execution_start_attempt(
        &self,
        job_id: &str,
        attempt: &AttemptToken,
        error: &str,
    ) -> Result<JobFailureOutcome> {
        self.fail_internal(
            job_id,
            Some(attempt),
            error,
            None,
            None,
            FailureMetadata::Preserve,
            &[],
        )
        .await
    }

    /// Mark a job as failed with step information for observability.
    /// Records the error message, failing step, and processor name in execution_info.
    /// `retry_after` is when the job's automatic retry is due, if one is planned.
    /// `uploads` carries the per-file results an upload processor reported for
    /// a partially failed batch; inputs it does not cover are recorded as failed
    /// with the job's error.
    pub async fn fail_with_step_info(
        &self,
        job_id: &str,
        error: &str,
        retry_after: Option<Timestamp>,
        processor_name: Option<&str>,
        uploads: &[UploadResultItem],
    ) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                None,
                error,
                retry_after,
                processor_name,
                FailureMetadata::DatabaseLog,
                uploads,
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(job_id, processor_name, error, "Job failed at pipeline step");
        }
        Ok(outcome)
    }

    pub(crate) async fn fail_attempt_with_step_info(
        &self,
        job_id: &str,
        attempt: &AttemptToken,
        error: &str,
        retry_after: Option<Timestamp>,
        processor_name: Option<&str>,
        uploads: &[UploadResultItem],
    ) -> Result<JobFailureOutcome> {
        let outcome = self
            .fail_internal(
                job_id,
                Some(attempt),
                error,
                retry_after,
                processor_name,
                FailureMetadata::DatabaseLog,
                uploads,
            )
            .await?;
        if outcome == JobFailureOutcome::Transitioned {
            warn!(job_id, processor_name, error, "Job failed at pipeline step");
        }
        Ok(outcome)
    }

    /// Get a job by ID.
    pub async fn get_job(&self, id: &str) -> Result<Option<Job>> {
        let repo = &self.job_repository;
        match repo.get_job(id).await {
            Ok(db_job) => {
                let job = db_model_to_job(&db_job);
                if let Some(info) = &job.execution_info
                    && !info.logs.is_empty()
                {
                    let (_, rows) = repo.list_execution_logs(id, &Pagination::new(0, 0)).await?;
                    if rows > 0 {
                        let owner: Arc<str> = Arc::from(id);
                        for entry in &info.logs {
                            entry.mark_persisted(owner.clone());
                        }
                    }
                    // An empty log table may be a legacy snapshot-only job. Its
                    // entries must be copied on the next snapshot update, before
                    // new rows make list_job_logs stop using its legacy fallback.
                }
                Ok(Some(job))
            }
            Err(Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// List jobs with filters and pagination.
    pub async fn list_jobs(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<Job>, u64)> {
        let (db_jobs, total) = self
            .job_repository
            .list_jobs_filtered(filters, pagination)
            .await?;
        let jobs = db_jobs.iter().map(db_model_to_job).collect();
        Ok((jobs, total))
    }

    /// List jobs with filters and pagination, without running a total `COUNT(*)`.
    pub async fn list_jobs_page(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<Job>> {
        let db_jobs = self
            .job_repository
            .list_jobs_page_filtered(filters, pagination)
            .await?;
        Ok(db_jobs.iter().map(db_model_to_job).collect())
    }

    /// Failed jobs whose scheduled retry is due at `now`, earliest first.
    pub async fn list_due_retries(&self, now: Timestamp) -> Result<Vec<Job>> {
        Ok(self
            .job_repository
            .list_jobs_due_for_retry(now.as_millisecond())
            .await?
            .iter()
            .map(db_model_to_job)
            .collect())
    }

    /// Drop the scheduled retry of `job_id`; the job stays failed.
    pub async fn clear_scheduled_retry(&self, job_id: &str) -> Result<()> {
        self.job_repository.clear_job_retry(job_id).await?;
        if let Some(mut job) = self.jobs_cache.get_mut(job_id) {
            job.retry_after = None;
        }
        Ok(())
    }

    /// Retry a failed or cancelled job.
    /// Returns error if job is not in a retryable terminal status.
    pub async fn retry_job(&self, id: &str) -> Result<Job> {
        let attempt_lease = self.attempt_gate.write().await;
        self.job_repository.reset_job_for_retry(id).await?;

        let _ = self.cancellation_tokens.remove(id);
        self.long_job_tracking.remove(id);
        let _ = self.persisted_log_cursor.remove(id);
        self.progress_cache.remove(id);
        self.jobs_cache.remove(id);

        self.depth.fetch_add(1, Ordering::SeqCst);
        drop(attempt_lease);
        self.wake_workers();

        let updated_job = db_model_to_job(&self.job_repository.get_job(id).await?);
        info!("Job {} retried (attempt {})", id, updated_job.retry_count);
        Ok(updated_job)
    }

    /// Cancel a job.
    /// Returns the cancelled job, or error for Completed/Failed jobs.
    pub async fn cancel_job(&self, id: &str) -> Result<Job> {
        let _attempt_lease = self.attempt_gate.write().await;

        // Get the job under `attempt_gate` so a concurrent `dequeue` cannot
        // claim or populate `jobs_cache` / `cancellation_tokens` after cancellation.
        let job = self
            .get_job(id)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))?;

        // Validate job is not in terminal status
        if job.status == JobStatus::Completed || job.status == JobStatus::Failed {
            return Err(Error::InvalidStateTransition {
                from: job.status.as_str().to_string(),
                to: "CANCELLED".to_string(),
            });
        }

        let is_processing = job.status == JobStatus::Processing;
        let has_token = self.cancellation_tokens.contains_key(id);

        if is_processing || has_token {
            let token = self.cancellation_tokens.entry(id.to_string()).or_default();
            token.cancel();
        }

        let updated = self.job_repository.mark_job_cancelled(id).await?;

        // Update cache and get the updated job
        let cancelled_job = {
            if let Some(mut cached_job) = self.jobs_cache.get_mut(id) {
                cached_job.status = JobStatus::Cancelled;
                cached_job.completed_at = Some(Timestamp::now());
                cached_job.clone()
            } else {
                // Job not in cache, return original with updated status
                let mut updated = job.clone();
                updated.status = JobStatus::Cancelled;
                updated.completed_at = Some(Timestamp::now());
                updated
            }
        };

        let keep_tracking = is_processing || self.cancellation_tokens.contains_key(id);
        if !keep_tracking {
            let _ = self.cancellation_tokens.remove(id);
            let _ = self.persisted_log_cursor.remove(id);
            self.progress_cache.remove(id);
        }

        self.jobs_cache.remove(id);

        if updated > 0 {
            self.decrement_depth(1);

            // job was fetched before the transition, so its type/streamer
            // fields are intact even though the cache entry is gone now.
            self.emit_upload_cancelled(id, &job.job_type, &job.streamer_id);
            self.finish_long_job_tracking(id, true);
        } else {
            self.long_job_tracking.remove(id);
        }

        info!("Job {} cancelled", id);
        Ok(cancelled_job)
    }

    /// Delete a job.
    /// Removes from database and cache.
    pub async fn delete_job(&self, id: &str) -> Result<()> {
        // Try to get from cache first to check status
        if let Some(job) = self.jobs_cache.get(id) {
            match job.status {
                JobStatus::Pending | JobStatus::Processing => {
                    return Err(Error::InvalidStateTransition {
                        from: job.status.as_str().to_string(),
                        to: "DELETED".to_string(),
                    });
                }
                _ => {} // Terminal states are fine for deletion
            }
        } else {
            // If not in cache, check DB
            match self.job_repository.get_job(id).await {
                Ok(job) => {
                    let status = match job.get_status() {
                        Some(s) => s,
                        None => {
                            return Err(Error::Other(format!(
                                "Invalid job status for job {}: {}",
                                id, job.status
                            )));
                        }
                    };

                    match status {
                        JobStatus::Pending | JobStatus::Processing => {
                            return Err(Error::InvalidStateTransition {
                                from: status.as_str().to_string(),
                                to: "DELETED".to_string(),
                            });
                        }
                        _ => {}
                    }
                }
                Err(Error::NotFound { .. }) => {
                    // Job doesn't exist, so deletion is trivially successful
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }

        // Proceed with deletion

        // 1. Delete from database
        self.job_repository.delete_job(id).await?;

        // 2. Remove from in-memory components
        self.jobs_cache.remove(id);
        self.cancellation_tokens.remove(id);
        self.long_job_tracking.remove(id);
        self.persisted_log_cursor.remove(id);
        self.progress_cache.remove(id);

        info!("Job {} deleted", id);
        Ok(())
    }

    /// Cancel all jobs in a pipeline.
    /// Returns the list of cancelled jobs.
    pub async fn cancel_pipeline(&self, pipeline_id: &str) -> Result<Vec<Job>> {
        let _attempt_lease = self.attempt_gate.write().await;
        let repo = &self.job_repository;
        // Get all jobs in the pipeline before cancelling
        let db_jobs = repo.get_jobs_by_pipeline(pipeline_id).await?;

        let count = repo.cancel_jobs_by_pipeline(pipeline_id).await?;
        info!(
            "Cancelled {} jobs in pipeline {} (database)",
            count, pipeline_id
        );

        // Convert to Job and collect cancelled ones while recording DB-processing jobs.
        let mut db_processing_ids = HashSet::new();
        let mut affected_ids = HashSet::new();
        let mut cancelled_jobs = Vec::new();
        for db_job in db_jobs {
            match db_job.get_status() {
                Some(JobStatus::Processing) => {
                    db_processing_ids.insert(db_job.id.clone());
                    affected_ids.insert(db_job.id.clone());
                    let mut job = db_model_to_job(&db_job);
                    job.status = JobStatus::Cancelled;
                    job.completed_at = Some(Timestamp::now());
                    cancelled_jobs.push(job);
                }
                Some(JobStatus::Pending) => {
                    affected_ids.insert(db_job.id.clone());
                    let mut job = db_model_to_job(&db_job);
                    job.status = JobStatus::Cancelled;
                    job.completed_at = Some(Timestamp::now());
                    cancelled_jobs.push(job);
                }
                Some(JobStatus::Cancelled) => {
                    affected_ids.insert(db_job.id.clone());
                }
                _ => {}
            }
        }

        // Include any cached entries for this pipeline.
        for entry in self.jobs_cache.iter() {
            let job = entry.value();
            if job.pipeline_id.as_deref() == Some(pipeline_id)
                && matches!(
                    job.status,
                    JobStatus::Pending | JobStatus::Processing | JobStatus::Cancelled
                )
            {
                affected_ids.insert(entry.key().clone());
            }
        }

        for id in &affected_ids {
            let cache_processing = self.jobs_cache.get(id).is_some_and(|job| {
                job.pipeline_id.as_deref() == Some(pipeline_id)
                    && job.status == JobStatus::Processing
            });
            let has_token = self.cancellation_tokens.contains_key(id);
            let keep_tracking = db_processing_ids.contains(id) || cache_processing || has_token;

            if keep_tracking {
                let token = self.cancellation_tokens.entry(id.clone()).or_default();
                token.cancel();
            } else {
                let _ = self.cancellation_tokens.remove(id);
                let _ = self.persisted_log_cursor.remove(id);
                self.progress_cache.remove(id);
            }

            self.jobs_cache.remove(id);
        }

        self.decrement_depth(count as usize);

        for job in &cancelled_jobs {
            self.emit_upload_cancelled(&job.id, &job.job_type, &job.streamer_id);
            self.finish_long_job_tracking(&job.id, true);
        }
        for id in &affected_ids {
            self.long_job_tracking.remove(id);
        }

        info!(
            "Cancelled {} jobs in pipeline {}",
            cancelled_jobs.len(),
            pipeline_id
        );
        Ok(cancelled_jobs)
    }

    /// Get the cancellation token for a job.
    pub async fn get_cancellation_token(&self, job_id: &str) -> Option<CancellationToken> {
        self.cancellation_tokens
            .get(job_id)
            .map(|t| t.token.clone())
    }

    /// The current attempt of a dequeued job, for the worker that must later
    /// finalize exactly that attempt.
    pub(crate) fn attempt_token(&self, job_id: &str) -> Option<AttemptToken> {
        self.cancellation_tokens.get(job_id).map(|t| t.clone())
    }

    /// Drop every in-memory trace of jobs whose rows have been deleted.
    ///
    /// `DagScheduler::delete_dag` removes job rows through
    /// `JobRepository::delete_jobs_by_pipeline`, so no terminal transition will ever run for
    /// them and nothing else clears what `enqueue`, `dequeue` and `progress_reporter` left
    /// behind. Cancelling the token stops a worker that is still inside `Processor::process`
    /// for a deleted job, and any entry still in an active state is removed from `depth`,
    /// which feeds `depth_status` and the throttle controller.
    pub(crate) fn forget_jobs(&self, job_ids: &[String]) {
        let mut still_active = 0usize;

        for job_id in job_ids {
            if let Some((_, job)) = self.jobs_cache.remove(job_id)
                && matches!(job.status, JobStatus::Pending | JobStatus::Processing)
            {
                still_active += 1;
            }
            if let Some((_, token)) = self.cancellation_tokens.remove(job_id) {
                token.cancel();
            }
            self.finish_long_job_tracking(job_id, true);
            let _ = self.persisted_log_cursor.remove(job_id);
            self.progress_cache.remove(job_id);
        }

        if still_active > 0 {
            self.decrement_depth(still_active);
        }
    }

    pub fn finalize_cancelled_job(&self, job_id: &str) {
        let _ = self.cancellation_tokens.remove(job_id);
        self.clear_attempt_tracking(job_id);
    }

    /// Finalize `attempt` unless a retry has already dequeued a successor under
    /// the same job ID. The successor then owns the job's token, log cursor,
    /// progress and cache entry, which a late cleanup must not remove.
    pub(crate) async fn finalize_attempt(&self, job_id: &str, attempt: &AttemptToken) {
        let _attempt_lease = self.attempt_gate.write().await;
        let removed = self
            .cancellation_tokens
            .remove_if(job_id, |_, current| current.attempt == attempt.attempt);
        if removed.is_none() && self.cancellation_tokens.contains_key(job_id) {
            return;
        }
        self.clear_attempt_tracking(job_id);
    }

    pub(super) fn clear_attempt_tracking(&self, job_id: &str) {
        self.long_job_tracking.remove(job_id);
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);
        self.jobs_cache.remove(job_id);
    }

    // ========================================================================
    // Fan-out and Multi-input Support Methods
    // ========================================================================

    /// Split a multi-input job into separate jobs for single-input processors.
    /// Creates one job per input file, all sharing the same pipeline context.
    /// Returns the IDs of the newly created jobs.
    pub async fn split_job_for_single_input(&self, job: &Job) -> Result<Vec<String>> {
        if job.inputs.len() <= 1 {
            // No splitting needed
            return Ok(vec![job.id.clone()]);
        }

        let mut created_job_ids = Vec::new();

        for input in job.inputs.iter() {
            // Create a new job for each input
            let mut split_job = Job::new_pipeline_step(
                job.job_type.clone(),
                vec![input.clone()],
                vec![], // Outputs will be determined by processor
                job.streamer_id.clone(),
                job.session_id.clone(),
                job.pipeline_id.clone(),
            )
            .with_priority(job.priority)
            .with_config(job.config.clone().unwrap_or_else(|| "{}".to_string()));

            if let Some(name) = job.streamer_name.as_ref() {
                split_job = split_job.with_streamer_name(name.clone());
            }
            if let Some(title) = job.session_title.as_ref() {
                split_job = split_job.with_session_title(title.clone());
            }
            if let Some(platform) = job.platform.as_ref() {
                split_job = split_job.with_platform(platform.clone());
            }
            if let Some(session_start) = job.session_start {
                split_job = split_job.with_session_start(session_start);
            }
            split_job.manifest = job.manifest.clone();

            match self.enqueue(split_job).await {
                Ok(job_id) => created_job_ids.push(job_id),
                Err(error) => {
                    self.rollback_split_jobs(&created_job_ids).await;
                    return Err(error);
                }
            }
        }

        // Mark the original job as completed (it was split)
        let repo = &self.job_repository;
        let result = async {
            let mut db_job = repo.get_job(&job.id).await?;
            db_job.mark_completed();
            db_job.set_outputs(&[]); // No outputs, job was split
            repo.update_job(&db_job).await
        }
        .await;
        if let Err(error) = result {
            self.rollback_split_jobs(&created_job_ids).await;
            return Err(error);
        }

        // Remove cancellation token for original job
        let _ = self.cancellation_tokens.remove(&job.id);
        self.finish_long_job_tracking(&job.id, true);
        let _ = self.persisted_log_cursor.remove(&job.id);
        self.progress_cache.remove(&job.id);
        self.jobs_cache.remove(&job.id);

        // Adjust queue depth to account for the original job completing.
        // The split jobs are enqueued and already increment depth.
        self.decrement_depth(1);

        info!(
            "Split job {} into {} jobs for single-input processing",
            job.id,
            created_job_ids.len()
        );

        Ok(created_job_ids)
    }

    async fn rollback_split_jobs(&self, job_ids: &[String]) {
        for job_id in job_ids {
            if let Err(error) = self.cancel_job(job_id).await {
                warn!(
                    %job_id,
                    %error,
                    "Failed to cancel a split job after fan-out failed"
                );
            }
        }
    }

    async fn persist_failure_metadata(
        &self,
        repo: &dyn JobRepository,
        job_id: &str,
        processor_name: Option<&str>,
        log_entry: &JobLogEntry,
    ) -> Result<()> {
        let exec_info_str = repo.get_job_execution_info(job_id).await?;
        let mut exec_info: JobExecutionInfo = json::parse_optional_or_default(
            exec_info_str.as_deref(),
            JsonContext::JobField {
                job_id,
                field: "execution_info",
            },
            "Invalid execution_info JSON; resetting to defaults",
        );

        if let Some(name) = processor_name {
            exec_info.current_processor = Some(name.to_string());
        }

        extend_logs_capped(&mut exec_info, std::slice::from_ref(log_entry));
        update_log_summary(&mut exec_info, std::slice::from_ref(log_entry));

        self.persist_logs_to_db(
            job_id,
            std::slice::from_ref(log_entry),
            LogPersistence::Append,
        )
        .await?;

        let exec_info_json = serde_json::to_string(&exec_info)?;
        repo.update_job_execution_info(job_id, &exec_info_json)
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn fail_internal(
        &self,
        job_id: &str,
        expected_attempt: Option<&AttemptToken>,
        error: &str,
        retry_after: Option<Timestamp>,
        processor_name: Option<&str>,
        metadata: FailureMetadata,
        uploads: &[UploadResultItem],
    ) -> Result<JobFailureOutcome> {
        let log_entry = JobLogEntry::error(format!("Job failed: {}", error));

        // The failure write can make a retry due at once. `retry_job` needs the
        // exclusive lease, so it cannot hand the job to a successor attempt
        // before the cleanup below has removed this attempt's tracking.
        let attempt_lease = self.attempt_gate.read().await;
        if let Some(attempt) = expected_attempt {
            let owns_active = self
                .cancellation_tokens
                .get(job_id)
                .is_some_and(|current| current.id() == attempt.id() && !current.is_cancelled());
            if !owns_active {
                drop(attempt_lease);
                self.finalize_attempt(job_id, attempt).await;
                return Ok(JobFailureOutcome::Unchanged);
            }
        }

        // Capture everything needed to persist failed per-file results and
        // emit the terminal event before cache cleanup evicts the job.
        let failed_upload_ctx = self.jobs_cache.get(job_id).and_then(|job| {
            upload_kind_for_job_type(&job.job_type).map(|_| FailedUploadContext {
                job_type: job.job_type.clone(),
                streamer_id: non_empty(&job.streamer_id),
                session_id: non_empty(&job.session_id),
                inputs: job.inputs.clone(),
            })
        });

        let updated = self
            .job_repository
            .mark_job_failed(
                job_id,
                error,
                retry_after.map(|retry_after| retry_after.as_millisecond()),
            )
            .await?;
        if updated == 0 {
            if let Some(attempt) = expected_attempt {
                drop(attempt_lease);
                self.finalize_attempt(job_id, attempt).await;
            } else {
                self.finalize_cancelled_job(job_id);
            }
            return Ok(JobFailureOutcome::Unchanged);
        }

        // FAILED is already durable. Auxiliary metadata errors must not suppress a
        // retry or skip terminal cleanup after this transition has won.
        if metadata != FailureMetadata::Preserve
            && let Err(error) = self
                .persist_failure_metadata(
                    self.job_repository.as_ref(),
                    job_id,
                    processor_name,
                    &log_entry,
                )
                .await
        {
            warn!(job_id, %error, "Failed to persist failed job execution metadata");
        }

        // Persist per-file rows only after this failure transition wins.
        // A concurrent cancellation that wins first returns above without
        // leaving FAILED upload records on a CANCELLED job. Results the
        // processor reported for individual files are kept as reported, so a
        // file that did transfer before the batch failed is not recorded as
        // failed; every other input failed with the job's error.
        let mut file_counts = (0u32, 0u32, 0u32);
        if let Some(context) = &failed_upload_ctx {
            let reported: HashMap<&str, &UploadResultItem> = uploads
                .iter()
                .map(|item| (item.local_path.as_str(), item))
                .collect();
            let items: Vec<UploadResultItem> = context
                .inputs
                .iter()
                .map(|path| {
                    reported
                        .get(path.as_str())
                        .map(|item| (*item).clone())
                        .unwrap_or_else(|| UploadResultItem {
                            local_path: path.clone(),
                            remote_path: None,
                            size_bytes: None,
                            status: UploadItemStatus::Failed,
                            error: Some(error.to_string()),
                        })
                })
                .collect();
            for item in &items {
                match item.status {
                    UploadItemStatus::Completed => file_counts.0 += 1,
                    UploadItemStatus::Failed => file_counts.1 += 1,
                    UploadItemStatus::Skipped => file_counts.2 += 1,
                }
            }
            self.persist_upload_records(
                job_id,
                &context.job_type,
                context.streamer_id.as_deref(),
                context.session_id.as_deref(),
                &items,
            )
            .await;
        }
        self.jobs_cache.remove(job_id);
        self.finish_long_job_tracking(job_id, true);

        // Remove cancellation token for this attempt
        let _ = self.cancellation_tokens.remove_if(job_id, |_, current| {
            expected_attempt.is_none_or(|attempt| current.id() == attempt.id())
        });
        let _ = self.persisted_log_cursor.remove(job_id);
        self.progress_cache.remove(job_id);

        self.decrement_depth(1);

        if let Some(context) = failed_upload_ctx {
            self.emit_upload_event(UploadStatusEvent::Terminal {
                job_id: job_id.to_string(),
                streamer_id: context.streamer_id,
                status: UploadTerminalStatus::Failed,
                files_succeeded: file_counts.0,
                files_failed: file_counts.1,
                files_skipped: file_counts.2,
                error: Some(error.to_string()),
            });
        }
        Ok(JobFailureOutcome::Transitioned)
    }
}
