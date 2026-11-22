//! Startup job recovery and queue statistics.

use std::sync::atomic::Ordering;

use tracing::{info, warn};

use super::{JobQueue, JobStats, db_model_to_job};
use crate::Result;
use crate::database::models::{JobFilters, JobStatus, Pagination};

impl JobQueue {
    /// Recover jobs from database on startup.
    /// Loads pending jobs and resets processing jobs to pending.
    /// Cancelled jobs remain terminal until explicitly retried.
    pub async fn recover_jobs(&self) -> Result<usize> {
        let repo = &self.job_repository;

        // Reset processing jobs to pending (they were interrupted by shutdown)
        let reset_processing = repo.reset_processing_jobs().await?;
        if reset_processing > 0 {
            info!("Reset {} processing jobs to pending", reset_processing);
        }

        // No worker has started, so every staged file still recorded belongs to
        // an attempt of an earlier process that never finalized. Listing those
        // is best effort and must not hold back job recovery.
        if let Some(staged) = self.staged_output_repo.get() {
            match staged
                .mark_interrupted(crate::database::time::now_ms())
                .await
            {
                Ok(0) => {}
                Ok(interrupted) => warn!(
                    interrupted,
                    "Interrupted jobs may have left partial temporary files; they are listed for review"
                ),
                Err(error) => warn!(%error, "Failed to mark staged outputs of interrupted jobs"),
            }
        }

        // Load pending jobs into cache
        let filters = JobFilters {
            status: Some(JobStatus::Pending),
            ..Default::default()
        };
        let pagination = Pagination::new(10000, 0); // Load all pending jobs

        let (db_jobs, total) = repo.list_jobs_filtered(&filters, &pagination).await?;

        self.jobs_cache.clear();
        self.cancellation_tokens.clear();
        self.long_job_tracking.clear();
        self.persisted_log_cursor.clear();
        self.progress_cache.clear();
        for db_job in &db_jobs {
            let job = db_model_to_job(db_job);
            self.jobs_cache.insert(job.id.clone(), job);
        }

        // Update depth counter
        self.depth.store(total as usize, Ordering::SeqCst);

        info!("Recovered {} pending jobs from database", total);
        Ok(total as usize)
    }

    /// Read the current processing count without scanning completed job history.
    pub async fn count_processing_jobs(&self) -> Result<u64> {
        self.job_repository
            .count_jobs(&JobFilters {
                status: Some(crate::database::models::JobStatus::Processing),
                ..Default::default()
            })
            .await
    }

    /// Get job statistics.
    pub async fn get_stats(&self) -> Result<JobStats> {
        let counts = self.job_repository.get_job_counts_by_status().await?;
        let avg_processing_time = self.job_repository.get_avg_processing_time().await?;

        Ok(JobStats {
            pending: counts.pending,
            processing: counts.processing,
            completed: counts.completed,
            failed: counts.failed,
            cancelled: counts.cancelled,
            avg_processing_time_secs: avg_processing_time,
        })
    }
}
