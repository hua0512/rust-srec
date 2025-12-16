//! Job repository.

use async_trait::async_trait;
use rand::random;
use sqlx::SqlitePool;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;

use crate::database::models::{
    JobCounts, JobDbModel, JobExecutionLogDbModel, JobExecutionProgressDbModel, JobFilters,
    Pagination,
};
use crate::{Error, Result};

const SQLITE_BUSY_MAX_RETRIES: usize = 8;
const SQLITE_BUSY_BASE_DELAY_MS: u64 = 10;
const SQLITE_BUSY_MAX_DELAY_MS: u64 = 250;

fn is_sqlite_busy_error(err: &Error) -> bool {
    let Error::DatabaseSqlx(sqlx_err) = err else {
        return false;
    };

    let sqlx::Error::Database(db_err) = sqlx_err else {
        let msg = sqlx_err.to_string().to_ascii_lowercase();
        return msg.contains("database is locked") || msg.contains("database is busy");
    };

    let code = db_err.code().map(Cow::into_owned);
    if matches!(code.as_deref(), Some("5") | Some("6")) {
        return true;
    }

    let msg = db_err.message().to_ascii_lowercase();
    msg.contains("database is locked") || msg.contains("database is busy")
}

fn is_missing_execution_log_columns_error(err: &Error) -> bool {
    let msg = err.to_string().to_ascii_lowercase();
    msg.contains("no such column") && (msg.contains("level") || msg.contains("message"))
        || msg.contains("has no column named level")
        || msg.contains("has no column named message")
}

async fn retry_on_sqlite_busy<T, F, Fut>(op_name: &'static str, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut attempt = 0usize;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_sqlite_busy_error(&err) || attempt >= SQLITE_BUSY_MAX_RETRIES {
                    return Err(err);
                }

                let exp_backoff_ms = SQLITE_BUSY_BASE_DELAY_MS.saturating_mul(1u64 << attempt);
                let capped_ms = exp_backoff_ms.min(SQLITE_BUSY_MAX_DELAY_MS);
                let jitter_ms =
                    (random::<u64>() % (capped_ms / 4 + 1)).min(SQLITE_BUSY_MAX_DELAY_MS);
                let delay =
                    Duration::from_millis((capped_ms + jitter_ms).min(SQLITE_BUSY_MAX_DELAY_MS));

                debug!(
                    "SQLite busy during {}, retrying in {:?} (attempt {}/{})",
                    op_name,
                    delay,
                    attempt + 1,
                    SQLITE_BUSY_MAX_RETRIES
                );

                sleep(delay).await;
                attempt += 1;
            }
        }
    }
}

/// Summary of a pipeline (group of jobs with same pipeline_id).
#[derive(Debug, Clone)]
pub struct PipelineSummary {
    pub pipeline_id: String,
    pub streamer_id: String,
    pub streamer_name: Option<String>,
    pub session_id: Option<String>,
    pub status: String,
    pub job_count: i64,
    pub completed_count: i64,
    pub failed_count: i64,
    pub total_duration_secs: f64,
    pub created_at: String,
    pub updated_at: String,
}

/// Job repository trait.
#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn get_job(&self, id: &str) -> Result<JobDbModel>;
    async fn list_pending_jobs(&self, job_type: &str) -> Result<Vec<JobDbModel>>;
    async fn list_jobs_by_status(&self, status: &str) -> Result<Vec<JobDbModel>>;
    async fn list_recent_jobs(&self, limit: i32) -> Result<Vec<JobDbModel>>;
    async fn create_job(&self, job: &JobDbModel) -> Result<()>;
    async fn update_job_status(&self, id: &str, status: &str) -> Result<()>;
    /// Mark a job as FAILED and set error/completed_at.
    async fn mark_job_failed(&self, id: &str, error: &str) -> Result<()>;
    /// Mark a job as INTERRUPTED and set completed_at.
    async fn mark_job_interrupted(&self, id: &str) -> Result<()>;
    /// Reset a job for retry (PENDING, clear started/completed/error, increment retry_count).
    async fn reset_job_for_retry(&self, id: &str) -> Result<()>;
    /// Count pending jobs, optionally filtered by job types.
    async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64>;
    /// Upsert (replace) the latest execution progress snapshot for a job.
    async fn upsert_job_execution_progress(
        &self,
        progress: &JobExecutionProgressDbModel,
    ) -> Result<()>;
    /// Get the latest execution progress snapshot for a job.
    async fn get_job_execution_progress(
        &self,
        job_id: &str,
    ) -> Result<Option<JobExecutionProgressDbModel>>;
    /// Atomically claim (transition) the next pending job to PROCESSING.
    /// Returns the claimed job, if any.
    ///
    /// This is intended for the hot dequeue path to avoid a list+update race and
    /// to reduce DB round-trips.
    async fn claim_next_pending_job(
        &self,
        job_types: Option<&[String]>,
    ) -> Result<Option<JobDbModel>>;
    /// Fetch only the `execution_info` field for a job.
    async fn get_job_execution_info(&self, id: &str) -> Result<Option<String>>;
    /// Update only the `execution_info` field for a job.
    async fn update_job_execution_info(&self, id: &str, execution_info: &str) -> Result<()>;
    async fn update_job_state(&self, id: &str, state: &str) -> Result<()>;
    async fn update_job(&self, job: &JobDbModel) -> Result<()>;
    async fn reset_interrupted_jobs(&self) -> Result<i32>;
    /// Reset processing jobs to pending (for recovery on startup).
    async fn reset_processing_jobs(&self) -> Result<i32>;
    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32>;
    async fn delete_job(&self, id: &str) -> Result<()>;

    // Purge methods (Requirements 7.1, 7.3)
    /// Purge completed/failed jobs older than the specified number of days.
    /// Deletes jobs in batches to avoid long-running transactions.
    /// Returns the number of jobs deleted.
    /// Requirements: 7.1, 7.3
    async fn purge_jobs_older_than(&self, days: u32, batch_size: u32) -> Result<u64>;

    /// Get IDs of jobs that are eligible for purging.
    /// Returns job IDs for completed/failed jobs older than the specified days.
    /// Requirements: 7.1, 7.3
    async fn get_purgeable_jobs(&self, days: u32, limit: u32) -> Result<Vec<String>>;

    // Execution logs
    async fn add_execution_log(&self, log: &JobExecutionLogDbModel) -> Result<()>;
    /// Add multiple execution logs in one transaction.
    async fn add_execution_logs(&self, logs: &[JobExecutionLogDbModel]) -> Result<()>;
    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>>;
    /// List execution logs with pagination, returning (logs, total_count).
    async fn list_execution_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobExecutionLogDbModel>, u64)>;
    async fn delete_execution_logs_for_job(&self, job_id: &str) -> Result<()>;

    // Filtering and pagination (Requirements 1.1, 1.3, 1.4, 1.5)
    /// List jobs with optional filters and pagination.
    /// Returns a tuple of (jobs, total_count).
    async fn list_jobs_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)>;
    /// List jobs with optional filters and pagination, without running a `COUNT(*)`.
    async fn list_jobs_page_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<JobDbModel>>;

    // Statistics (Requirements 3.1, 3.2, 3.3)
    /// Get job counts by status.
    async fn get_job_counts_by_status(&self) -> Result<JobCounts>;

    /// Get average processing time for completed jobs in seconds.
    async fn get_avg_processing_time(&self) -> Result<Option<f64>>;

    // Atomic pipeline operations (Requirements 7.2, 7.3)
    /// Atomically complete a job and create the next job in the pipeline.
    /// This ensures crash-safe transition between pipeline steps.
    /// Returns the ID of the newly created job, if any.
    async fn complete_job_and_create_next(
        &self,
        job_id: &str,
        outputs_json: &str,
        duration_secs: f64,
        queue_wait_secs: f64,
        next_job: Option<&JobDbModel>,
    ) -> Result<Option<String>>;

    /// Cancel all pending/processing jobs in a pipeline.
    /// Returns the number of jobs cancelled.
    async fn cancel_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64>;

    /// Get all jobs in a pipeline.
    async fn get_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<Vec<JobDbModel>>;

    /// List pipelines (grouped by pipeline_id) with pagination.
    /// Returns summaries of pipelines and total count.
    async fn list_pipelines(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<PipelineSummary>, u64)>;
}

/// SQLx implementation of JobRepository.
pub struct SqlxJobRepository {
    pool: SqlitePool,
}

impl SqlxJobRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl JobRepository for SqlxJobRepository {
    async fn get_job(&self, id: &str) -> Result<JobDbModel> {
        sqlx::query_as::<_, JobDbModel>("SELECT * FROM job WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("Job", id))
    }

    async fn list_pending_jobs(&self, job_type: &str) -> Result<Vec<JobDbModel>> {
        let jobs = sqlx::query_as::<_, JobDbModel>(
            "SELECT * FROM job WHERE status = 'PENDING' AND job_type = ? ORDER BY created_at",
        )
        .bind(job_type)
        .fetch_all(&self.pool)
        .await?;
        Ok(jobs)
    }

    async fn list_jobs_by_status(&self, status: &str) -> Result<Vec<JobDbModel>> {
        let jobs = sqlx::query_as::<_, JobDbModel>(
            "SELECT * FROM job WHERE status = ? ORDER BY created_at DESC",
        )
        .bind(status)
        .fetch_all(&self.pool)
        .await?;
        Ok(jobs)
    }

    async fn list_recent_jobs(&self, limit: i32) -> Result<Vec<JobDbModel>> {
        let jobs =
            sqlx::query_as::<_, JobDbModel>("SELECT * FROM job ORDER BY created_at DESC LIMIT ?")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(jobs)
    }

    async fn create_job(&self, job: &JobDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO job (
                id, job_type, status, config, state, created_at, updated_at,
                input, outputs, priority, streamer_id, session_id,
                started_at, completed_at, error, retry_count,
                next_job_type, remaining_steps, pipeline_id, execution_info,
                duration_secs, queue_wait_secs
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&job.id)
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(&job.created_at)
        .bind(&job.updated_at)
        .bind(&job.input)
        .bind(&job.outputs)
        .bind(job.priority)
        .bind(&job.streamer_id)
        .bind(&job.session_id)
        .bind(&job.started_at)
        .bind(&job.completed_at)
        .bind(&job.error)
        .bind(job.retry_count)
        .bind(&job.next_job_type)
        .bind(&job.remaining_steps)
        .bind(&job.pipeline_id)
        .bind(&job.execution_info)
        .bind(job.duration_secs)
        .bind(job.queue_wait_secs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_job_status(&self, id: &str, status: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE job SET status = ?, updated_at = ? WHERE id = ?")
            .bind(status)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn mark_job_failed(&self, id: &str, error: &str) -> Result<()> {
        retry_on_sqlite_busy("mark_job_failed", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE job SET status = 'FAILED', completed_at = ?, updated_at = ?, error = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(error)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn mark_job_interrupted(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("mark_job_interrupted", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE job SET status = 'INTERRUPTED', completed_at = ?, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn reset_job_for_retry(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("reset_job_for_retry", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query(
                "UPDATE job SET status = 'PENDING', started_at = NULL, completed_at = NULL, error = NULL, retry_count = retry_count + 1, updated_at = ? WHERE id = ?",
            )
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64> {
        let (sql, bind_job_types) = match job_types {
            Some(types) if !types.is_empty() => {
                let placeholders = std::iter::repeat("?")
                    .take(types.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                (
                    format!(
                        "SELECT COUNT(*) FROM job WHERE status = 'PENDING' AND job_type IN ({})",
                        placeholders
                    ),
                    true,
                )
            }
            _ => (
                "SELECT COUNT(*) FROM job WHERE status = 'PENDING'".to_string(),
                false,
            ),
        };

        let mut query = sqlx::query_scalar::<_, i64>(&sql);
        if bind_job_types {
            if let Some(types) = job_types {
                for jt in types {
                    query = query.bind(jt);
                }
            }
        }

        let count = query.fetch_one(&self.pool).await?;
        Ok(count.max(0) as u64)
    }

    async fn upsert_job_execution_progress(
        &self,
        progress: &JobExecutionProgressDbModel,
    ) -> Result<()> {
        retry_on_sqlite_busy("upsert_job_execution_progress", || async {
            sqlx::query(
                r#"
                INSERT INTO job_execution_progress (job_id, kind, progress, updated_at)
                VALUES (?, ?, ?, ?)
                ON CONFLICT(job_id) DO UPDATE SET
                    kind = excluded.kind,
                    progress = excluded.progress,
                    updated_at = excluded.updated_at
                "#,
            )
            .bind(&progress.job_id)
            .bind(&progress.kind)
            .bind(&progress.progress)
            .bind(&progress.updated_at)
            .execute(&self.pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn get_job_execution_progress(
        &self,
        job_id: &str,
    ) -> Result<Option<JobExecutionProgressDbModel>> {
        let row = sqlx::query_as::<_, JobExecutionProgressDbModel>(
            "SELECT * FROM job_execution_progress WHERE job_id = ?",
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    async fn claim_next_pending_job(
        &self,
        job_types: Option<&[String]>,
    ) -> Result<Option<JobDbModel>> {
        retry_on_sqlite_busy("claim_next_pending_job", || async {
            let now = chrono::Utc::now().to_rfc3339();

            // SQLite supports `RETURNING` in modern versions; this keeps the claim
            // atomic and avoids a list+update race between workers.
            //
            // We keep ordering consistent with list_jobs_filtered: priority DESC, created_at DESC.
            let (sql, bind_job_types): (String, bool) = match job_types {
                Some(types) if !types.is_empty() => {
                    let placeholders = types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                    (
                        format!(
                            r#"
                            UPDATE job
                            SET status = 'PROCESSING',
                                started_at = ?,
                                updated_at = ?
                            WHERE id = (
                                SELECT id
                                FROM job
                                WHERE status = 'PENDING' AND job_type IN ({})
                                ORDER BY priority DESC, created_at DESC
                                LIMIT 1
                            )
                              AND status = 'PENDING'
                            RETURNING *
                            "#,
                            placeholders
                        ),
                        true,
                    )
                }
                _ => (
                    r#"
                    UPDATE job
                    SET status = 'PROCESSING',
                        started_at = ?,
                        updated_at = ?
                    WHERE id = (
                        SELECT id
                        FROM job
                        WHERE status = 'PENDING'
                        ORDER BY priority DESC, created_at DESC
                        LIMIT 1
                    )
                      AND status = 'PENDING'
                    RETURNING *
                    "#
                    .to_string(),
                    false,
                ),
            };

            let mut query = sqlx::query_as::<_, JobDbModel>(&sql).bind(&now).bind(&now);
            if bind_job_types {
                if let Some(types) = job_types {
                    for jt in types {
                        query = query.bind(jt);
                    }
                }
            }

            let claimed = query.fetch_optional(&self.pool).await?;
            Ok(claimed)
        })
        .await
    }

    async fn get_job_execution_info(&self, id: &str) -> Result<Option<String>> {
        sqlx::query_scalar::<_, String>("SELECT execution_info FROM job WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::from)
    }

    async fn update_job_execution_info(&self, id: &str, execution_info: &str) -> Result<()> {
        retry_on_sqlite_busy("update_job_execution_info", || async {
            let now = chrono::Utc::now().to_rfc3339();
            sqlx::query("UPDATE job SET execution_info = ?, updated_at = ? WHERE id = ?")
                .bind(execution_info)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn update_job_state(&self, id: &str, state: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("UPDATE job SET state = ?, updated_at = ? WHERE id = ?")
            .bind(state)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_job(&self, job: &JobDbModel) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE job SET
                job_type = ?,
                status = ?,
                config = ?,
                state = ?,
                updated_at = ?,
                input = ?,
                outputs = ?,
                priority = ?,
                streamer_id = ?,
                session_id = ?,
                started_at = ?,
                completed_at = ?,
                error = ?,
                retry_count = ?,
                next_job_type = ?,
                remaining_steps = ?,
                pipeline_id = ?,
                execution_info = ?,
                duration_secs = ?,
                queue_wait_secs = ?
            WHERE id = ?
            "#,
        )
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(&now)
        .bind(&job.input)
        .bind(&job.outputs)
        .bind(job.priority)
        .bind(&job.streamer_id)
        .bind(&job.session_id)
        .bind(&job.started_at)
        .bind(&job.completed_at)
        .bind(&job.error)
        .bind(job.retry_count)
        .bind(&job.next_job_type)
        .bind(&job.remaining_steps)
        .bind(&job.pipeline_id)
        .bind(&job.execution_info)
        .bind(job.duration_secs)
        .bind(job.queue_wait_secs)
        .bind(&job.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn reset_interrupted_jobs(&self) -> Result<i32> {
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE job SET status = 'PENDING', updated_at = ? WHERE status = 'INTERRUPTED'",
        )
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as i32)
    }

    async fn reset_processing_jobs(&self) -> Result<i32> {
        let now = chrono::Utc::now().to_rfc3339();
        let result = sqlx::query(
            "UPDATE job SET status = 'PENDING', started_at = NULL, updated_at = ? WHERE status = 'PROCESSING'",
        )
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() as i32)
    }

    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32> {
        // First delete execution logs for old completed/failed jobs
        let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        sqlx::query(
            r#"
            DELETE FROM job_execution_logs 
            WHERE job_id IN (
                SELECT id FROM job 
                WHERE status IN ('COMPLETED', 'FAILED') 
                AND updated_at < ?
            )
            "#,
        )
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await?;

        // Then delete the jobs
        let result = sqlx::query(
            "DELETE FROM job WHERE status IN ('COMPLETED', 'FAILED') AND updated_at < ?",
        )
        .bind(&cutoff_str)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() as i32)
    }

    async fn delete_job(&self, id: &str) -> Result<()> {
        // Execution logs are deleted via CASCADE
        sqlx::query("DELETE FROM job WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn add_execution_log(&self, log: &JobExecutionLogDbModel) -> Result<()> {
        retry_on_sqlite_busy("add_execution_log", || async {
            let attempt = sqlx::query(
                r#"
                INSERT INTO job_execution_logs (id, job_id, entry, created_at, level, message)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&log.id)
            .bind(&log.job_id)
            .bind(&log.entry)
            .bind(&log.created_at)
            .bind(&log.level)
            .bind(&log.message)
            .execute(&self.pool)
            .await;

            match attempt {
                Ok(_) => Ok(()),
                Err(e) => {
                    let err = Error::from(e);
                    if !is_missing_execution_log_columns_error(&err) {
                        return Err(err);
                    }

                    sqlx::query(
                        r#"
                        INSERT INTO job_execution_logs (id, job_id, entry, created_at)
                        VALUES (?, ?, ?, ?)
                        "#,
                    )
                    .bind(&log.id)
                    .bind(&log.job_id)
                    .bind(&log.entry)
                    .bind(&log.created_at)
                    .execute(&self.pool)
                    .await?;
                    Ok(())
                }
            }
        })
        .await
    }

    async fn add_execution_logs(&self, logs: &[JobExecutionLogDbModel]) -> Result<()> {
        if logs.is_empty() {
            return Ok(());
        }

        retry_on_sqlite_busy("add_execution_logs", || async {
            let mut tx = self.pool.begin().await?;
            for log in logs {
                let res = sqlx::query(
                    r#"
                    INSERT INTO job_execution_logs (id, job_id, entry, created_at, level, message)
                    VALUES (?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&log.id)
                .bind(&log.job_id)
                .bind(&log.entry)
                .bind(&log.created_at)
                .bind(&log.level)
                .bind(&log.message)
                .execute(&mut *tx)
                .await;

                if let Err(e) = res {
                    let err = Error::from(e);
                    if !is_missing_execution_log_columns_error(&err) {
                        return Err(err);
                    }

                    // Fallback to legacy schema (no structured columns).
                    tx.rollback().await?;
                    let mut tx = self.pool.begin().await?;
                    for log in logs {
                        sqlx::query(
                            r#"
                            INSERT INTO job_execution_logs (id, job_id, entry, created_at)
                            VALUES (?, ?, ?, ?)
                            "#,
                        )
                        .bind(&log.id)
                        .bind(&log.job_id)
                        .bind(&log.entry)
                        .bind(&log.created_at)
                        .execute(&mut *tx)
                        .await?;
                    }
                    tx.commit().await?;
                    return Ok(());
                }
            }
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
        let full = sqlx::query_as::<_, JobExecutionLogDbModel>(
            "SELECT id, job_id, entry, created_at, level, message FROM job_execution_logs WHERE job_id = ? ORDER BY created_at",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await;

        match full {
            Ok(logs) => Ok(logs),
            Err(e) => {
                let err = Error::from(e);
                if !is_missing_execution_log_columns_error(&err) {
                    return Err(err);
                }

                let logs = sqlx::query_as::<_, JobExecutionLogDbModel>(
                    "SELECT id, job_id, entry, created_at, NULL as level, NULL as message FROM job_execution_logs WHERE job_id = ? ORDER BY created_at",
                )
                .bind(job_id)
                .fetch_all(&self.pool)
                .await?;
                Ok(logs)
            }
        }
    }

    async fn list_execution_logs(
        &self,
        job_id: &str,
        pagination: &Pagination,
    ) -> Result<(Vec<JobExecutionLogDbModel>, u64)> {
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM job_execution_logs WHERE job_id = ?")
                .bind(job_id)
                .fetch_one(&self.pool)
                .await?;

        let full = sqlx::query_as::<_, JobExecutionLogDbModel>(
            "SELECT id, job_id, entry, created_at, level, message FROM job_execution_logs WHERE job_id = ? ORDER BY created_at LIMIT ? OFFSET ?",
        )
        .bind(job_id)
        .bind(pagination.limit as i64)
        .bind(pagination.offset as i64)
        .fetch_all(&self.pool)
        .await;

        match full {
            Ok(logs) => Ok((logs, total as u64)),
            Err(e) => {
                let err = Error::from(e);
                if !is_missing_execution_log_columns_error(&err) {
                    return Err(err);
                }

                let logs = sqlx::query_as::<_, JobExecutionLogDbModel>(
                    "SELECT id, job_id, entry, created_at, NULL as level, NULL as message FROM job_execution_logs WHERE job_id = ? ORDER BY created_at LIMIT ? OFFSET ?",
                )
                .bind(job_id)
                .bind(pagination.limit as i64)
                .bind(pagination.offset as i64)
                .fetch_all(&self.pool)
                .await?;
                Ok((logs, total as u64))
            }
        }
    }

    async fn delete_execution_logs_for_job(&self, job_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM job_execution_logs WHERE job_id = ?")
            .bind(job_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list_jobs_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)> {
        // Build dynamic WHERE clause
        let mut conditions: Vec<String> = Vec::new();

        if filters.status.is_some() {
            conditions.push("status = ?".to_string());
        }
        if filters.streamer_id.is_some() {
            conditions.push("streamer_id = ?".to_string());
        }
        if filters.session_id.is_some() {
            conditions.push("session_id = ?".to_string());
        }
        if filters.pipeline_id.is_some() {
            conditions.push("pipeline_id = ?".to_string());
        }
        if filters.from_date.is_some() {
            conditions.push("created_at >= ?".to_string());
        }
        if filters.to_date.is_some() {
            conditions.push("created_at <= ?".to_string());
        }
        if filters.job_type.is_some() {
            conditions.push("job_type = ?".to_string());
        }
        if let Some(job_types) = &filters.job_types {
            if !job_types.is_empty() {
                let placeholders = job_types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                conditions.push(format!("job_type IN ({})", placeholders));
            }
        }
        if filters.search.is_some() {
            conditions.push(
                "(id LIKE ? OR session_id LIKE ? OR streamer_id LIKE ? OR job_type LIKE ?)"
                    .to_string(),
            );
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Count query
        let count_sql = format!("SELECT COUNT(*) as count FROM job {}", where_clause);

        // Data query with pagination, ordered by priority (desc) then created_at (desc)
        let data_sql = format!(
            "SELECT * FROM job {} ORDER BY priority DESC, created_at DESC LIMIT ? OFFSET ?",
            where_clause
        );

        // Execute count query
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql);

        // Bind parameters for count query
        if let Some(status) = &filters.status {
            count_query = count_query.bind(status.as_str());
        }
        if let Some(streamer_id) = &filters.streamer_id {
            count_query = count_query.bind(streamer_id);
        }
        if let Some(session_id) = &filters.session_id {
            count_query = count_query.bind(session_id);
        }
        if let Some(pipeline_id) = &filters.pipeline_id {
            count_query = count_query.bind(pipeline_id);
        }
        if let Some(from_date) = &filters.from_date {
            count_query = count_query.bind(from_date.to_rfc3339());
        }
        if let Some(to_date) = &filters.to_date {
            count_query = count_query.bind(to_date.to_rfc3339());
        }
        if let Some(job_type) = &filters.job_type {
            count_query = count_query.bind(job_type);
        }
        if let Some(job_types) = &filters.job_types {
            for jt in job_types {
                count_query = count_query.bind(jt);
            }
        }
        if let Some(search) = &filters.search {
            let pattern = format!("%{}%", search);
            count_query = count_query
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }

        let total_count = count_query.fetch_one(&self.pool).await? as u64;

        // Execute data query
        let mut data_query = sqlx::query_as::<_, JobDbModel>(&data_sql);

        // Bind parameters for data query
        if let Some(status) = &filters.status {
            data_query = data_query.bind(status.as_str());
        }
        if let Some(streamer_id) = &filters.streamer_id {
            data_query = data_query.bind(streamer_id);
        }
        if let Some(session_id) = &filters.session_id {
            data_query = data_query.bind(session_id);
        }
        if let Some(pipeline_id) = &filters.pipeline_id {
            data_query = data_query.bind(pipeline_id);
        }
        if let Some(from_date) = &filters.from_date {
            data_query = data_query.bind(from_date.to_rfc3339());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.to_rfc3339());
        }
        if let Some(job_type) = &filters.job_type {
            data_query = data_query.bind(job_type);
        }
        if let Some(job_types) = &filters.job_types {
            for jt in job_types {
                data_query = data_query.bind(jt);
            }
        }
        if let Some(search) = &filters.search {
            let pattern = format!("%{}%", search);
            data_query = data_query
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }

        // Bind pagination parameters
        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let jobs = data_query.fetch_all(&self.pool).await?;

        Ok((jobs, total_count))
    }

    async fn list_jobs_page_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<Vec<JobDbModel>> {
        // Build dynamic WHERE clause (matches list_jobs_filtered).
        let mut conditions: Vec<String> = Vec::new();

        if filters.status.is_some() {
            conditions.push("status = ?".to_string());
        }
        if filters.streamer_id.is_some() {
            conditions.push("streamer_id = ?".to_string());
        }
        if filters.session_id.is_some() {
            conditions.push("session_id = ?".to_string());
        }
        if filters.pipeline_id.is_some() {
            conditions.push("pipeline_id = ?".to_string());
        }
        if filters.from_date.is_some() {
            conditions.push("created_at >= ?".to_string());
        }
        if filters.to_date.is_some() {
            conditions.push("created_at <= ?".to_string());
        }
        if filters.job_type.is_some() {
            conditions.push("job_type = ?".to_string());
        }
        if let Some(job_types) = &filters.job_types {
            if !job_types.is_empty() {
                let placeholders = job_types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                conditions.push(format!("job_type IN ({})", placeholders));
            }
        }
        if filters.search.is_some() {
            conditions.push(
                "(id LIKE ? OR session_id LIKE ? OR streamer_id LIKE ? OR job_type LIKE ?)"
                    .to_string(),
            );
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let data_sql = format!(
            "SELECT * FROM job {} ORDER BY priority DESC, created_at DESC LIMIT ? OFFSET ?",
            where_clause
        );

        let mut data_query = sqlx::query_as::<_, JobDbModel>(&data_sql);

        if let Some(status) = &filters.status {
            data_query = data_query.bind(status.as_str());
        }
        if let Some(streamer_id) = &filters.streamer_id {
            data_query = data_query.bind(streamer_id);
        }
        if let Some(session_id) = &filters.session_id {
            data_query = data_query.bind(session_id);
        }
        if let Some(pipeline_id) = &filters.pipeline_id {
            data_query = data_query.bind(pipeline_id);
        }
        if let Some(from_date) = &filters.from_date {
            data_query = data_query.bind(from_date.to_rfc3339());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.to_rfc3339());
        }
        if let Some(job_type) = &filters.job_type {
            data_query = data_query.bind(job_type);
        }
        if let Some(job_types) = &filters.job_types {
            for jt in job_types {
                data_query = data_query.bind(jt);
            }
        }
        if let Some(search) = &filters.search {
            let pattern = format!("%{}%", search);
            data_query = data_query
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }

        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let jobs = data_query.fetch_all(&self.pool).await?;
        Ok(jobs)
    }

    async fn get_job_counts_by_status(&self) -> Result<JobCounts> {
        // Use a single query with CASE statements for efficiency
        let row: (i64, i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                COALESCE(SUM(CASE WHEN status = 'PENDING' THEN 1 ELSE 0 END), 0) as pending,
                COALESCE(SUM(CASE WHEN status = 'PROCESSING' THEN 1 ELSE 0 END), 0) as processing,
                COALESCE(SUM(CASE WHEN status = 'COMPLETED' THEN 1 ELSE 0 END), 0) as completed,
                COALESCE(SUM(CASE WHEN status = 'FAILED' THEN 1 ELSE 0 END), 0) as failed,
                COALESCE(SUM(CASE WHEN status = 'INTERRUPTED' THEN 1 ELSE 0 END), 0) as interrupted
            FROM job
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(JobCounts {
            pending: row.0 as u64,
            processing: row.1 as u64,
            completed: row.2 as u64,
            failed: row.3 as u64,
            interrupted: row.4 as u64,
        })
    }

    async fn get_avg_processing_time(&self) -> Result<Option<f64>> {
        // Calculate average processing time for completed jobs
        // Processing time is the difference between completed_at and started_at
        // Falls back to updated_at - created_at for jobs without started_at/completed_at
        let result: Option<f64> = sqlx::query_scalar(
            r#"
            SELECT AVG(
                CASE 
                    WHEN started_at IS NOT NULL AND completed_at IS NOT NULL THEN
                        (julianday(completed_at) - julianday(started_at)) * 86400.0
                    ELSE
                        (julianday(updated_at) - julianday(created_at)) * 86400.0
                END
            ) as avg_time
            FROM job
            WHERE status = 'COMPLETED'
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(result)
    }

    async fn complete_job_and_create_next(
        &self,
        job_id: &str,
        outputs_json: &str,
        duration_secs: f64,
        queue_wait_secs: f64,
        next_job: Option<&JobDbModel>,
    ) -> Result<Option<String>> {
        retry_on_sqlite_busy("complete_job_and_create_next", || async {
            let now = chrono::Utc::now().to_rfc3339();

            // Start a transaction for atomic operation
            let mut tx = self.pool.begin().await?;

            // 1. Mark current job as COMPLETED
            sqlx::query(
                r#"
                UPDATE job SET
                    status = 'COMPLETED',
                    completed_at = ?,
                    updated_at = ?,
                    outputs = ?,
                    duration_secs = ?,
                    queue_wait_secs = ?
                WHERE id = ?
                "#,
            )
            .bind(&now)
            .bind(&now)
            .bind(outputs_json)
            .bind(duration_secs)
            .bind(queue_wait_secs)
            .bind(job_id)
            .execute(&mut *tx)
            .await?;

            // 2. Create next job if defined
            let next_job_id = if let Some(job) = next_job {
                sqlx::query(
                    r#"
                    INSERT INTO job (
                        id, job_type, status, config, state, created_at, updated_at,
                        input, outputs, priority, streamer_id, session_id,
                        started_at, completed_at, error, retry_count,
                        next_job_type, remaining_steps, pipeline_id, execution_info,
                        duration_secs, queue_wait_secs
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                )
                .bind(&job.id)
                .bind(&job.job_type)
                .bind(&job.status)
                .bind(&job.config)
                .bind(&job.state)
                .bind(&job.created_at)
                .bind(&job.updated_at)
                .bind(&job.input)
                .bind(&job.outputs)
                .bind(job.priority)
                .bind(&job.streamer_id)
                .bind(&job.session_id)
                .bind(&job.started_at)
                .bind(&job.completed_at)
                .bind(&job.error)
                .bind(job.retry_count)
                .bind(&job.next_job_type)
                .bind(&job.remaining_steps)
                .bind(&job.pipeline_id)
                .bind(&job.execution_info)
                .bind(job.duration_secs)
                .bind(job.queue_wait_secs)
                .execute(&mut *tx)
                .await?;

                Some(job.id.clone())
            } else {
                None
            };

            // 3. Commit transaction - atomic!
            tx.commit().await?;

            Ok(next_job_id)
        })
        .await
    }

    async fn cancel_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64> {
        let now = chrono::Utc::now().to_rfc3339();

        let result = sqlx::query(
            r#"
            UPDATE job SET
                status = 'INTERRUPTED',
                completed_at = ?,
                updated_at = ?
            WHERE pipeline_id = ?
              AND status IN ('PENDING', 'PROCESSING')
            "#,
        )
        .bind(&now)
        .bind(&now)
        .bind(pipeline_id)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    async fn get_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<Vec<JobDbModel>> {
        let jobs = sqlx::query_as::<_, JobDbModel>(
            "SELECT * FROM job WHERE pipeline_id = ? ORDER BY created_at",
        )
        .bind(pipeline_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(jobs)
    }

    async fn list_pipelines(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<PipelineSummary>, u64)> {
        // Build WHERE clause for filters (applied before aggregation)
        let mut where_conditions = vec!["pipeline_id IS NOT NULL".to_string()];

        if filters.streamer_id.is_some() {
            where_conditions.push("streamer_id = ?".to_string());
        }
        if filters.session_id.is_some() {
            where_conditions.push("session_id = ?".to_string());
        }
        if filters.search.is_some() {
            where_conditions.push(
                "(pipeline_id LIKE ? OR streamer_id LIKE ? OR session_id LIKE ?)".to_string(),
            );
        }

        let where_clause = where_conditions.join(" AND ");

        // Build HAVING clause for status filtering (applied after aggregation)
        let having_clause = if filters.status.is_some() {
            "HAVING computed_status = ?"
        } else {
            ""
        };

        // Query to get pipeline summaries with aggregation
        // Status logic: FAILED if any failed, PROCESSING if any processing, COMPLETED if all completed, else PENDING
        let query = format!(
            r#"
            SELECT
                pipeline_id,
                MAX(streamer_id) as streamer_id,
                MAX(session_id) as session_id,
                CASE
                    WHEN SUM(CASE WHEN status = 'FAILED' THEN 1 ELSE 0 END) > 0 THEN 'FAILED'
                    WHEN SUM(CASE WHEN status = 'PROCESSING' THEN 1 ELSE 0 END) > 0 THEN 'PROCESSING'
                    WHEN SUM(CASE WHEN status = 'INTERRUPTED' THEN 1 ELSE 0 END) > 0 THEN 'INTERRUPTED'
                    WHEN COUNT(*) = SUM(CASE WHEN status = 'COMPLETED' THEN 1 ELSE 0 END) THEN 'COMPLETED'
                    ELSE 'PENDING'
                END as computed_status,
                COUNT(*) as job_count,
                SUM(CASE WHEN status = 'COMPLETED' THEN 1 ELSE 0 END) as completed_count,
                SUM(CASE WHEN status = 'FAILED' THEN 1 ELSE 0 END) as failed_count,
                COALESCE(SUM(duration_secs), 0.0) as total_duration_secs,
                MIN(created_at) as created_at,
                MAX(updated_at) as updated_at
            FROM job
            WHERE {}
            GROUP BY pipeline_id
            {}
            ORDER BY MAX(created_at) DESC
            LIMIT ? OFFSET ?
            "#,
            where_clause, having_clause
        );

        // Count query needs to count pipelines with matching aggregated status
        let count_query = format!(
            r#"
            SELECT COUNT(*) FROM (
                SELECT
                    pipeline_id,
                    CASE
                        WHEN SUM(CASE WHEN status = 'FAILED' THEN 1 ELSE 0 END) > 0 THEN 'FAILED'
                        WHEN SUM(CASE WHEN status = 'PROCESSING' THEN 1 ELSE 0 END) > 0 THEN 'PROCESSING'
                        WHEN SUM(CASE WHEN status = 'INTERRUPTED' THEN 1 ELSE 0 END) > 0 THEN 'INTERRUPTED'
                        WHEN COUNT(*) = SUM(CASE WHEN status = 'COMPLETED' THEN 1 ELSE 0 END) THEN 'COMPLETED'
                        ELSE 'PENDING'
                    END as computed_status
                FROM job
                WHERE {}
                GROUP BY pipeline_id
                {}
            )
            "#,
            where_clause, having_clause
        );

        // Execute count query
        let count_builder = sqlx::query_scalar::<_, i64>(&count_query);
        let count_builder = {
            let mut q = count_builder;
            if let Some(ref streamer_id) = filters.streamer_id {
                q = q.bind(streamer_id);
            }
            if let Some(ref session_id) = filters.session_id {
                q = q.bind(session_id);
            }
            if let Some(ref search) = filters.search {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern.clone()).bind(pattern);
            }
            if let Some(ref status) = filters.status {
                q = q.bind(status.as_str());
            }
            q
        };
        let total: i64 = count_builder.fetch_one(&self.pool).await.unwrap_or(0);

        // Execute main query
        let mut query_builder = sqlx::query_as::<
            _,
            (
                String,
                String,
                Option<String>,
                String,
                i64,
                i64,
                i64,
                f64,
                String,
                String,
            ),
        >(&query);

        if let Some(ref streamer_id) = filters.streamer_id {
            query_builder = query_builder.bind(streamer_id);
        }
        if let Some(ref session_id) = filters.session_id {
            query_builder = query_builder.bind(session_id);
        }
        if let Some(ref search) = filters.search {
            let pattern = format!("%{}%", search);
            query_builder = query_builder
                .bind(pattern.clone())
                .bind(pattern.clone())
                .bind(pattern);
        }
        if let Some(ref status) = filters.status {
            query_builder = query_builder.bind(status.as_str());
        }
        query_builder = query_builder
            .bind(pagination.limit as i64)
            .bind(pagination.offset as i64);

        let rows = query_builder.fetch_all(&self.pool).await?;

        let summaries: Vec<PipelineSummary> = rows
            .into_iter()
            .map(|row| PipelineSummary {
                pipeline_id: row.0,
                streamer_id: row.1,
                streamer_name: None, // Populated at API layer
                session_id: row.2,
                status: row.3,
                job_count: row.4,
                completed_count: row.5,
                failed_count: row.6,
                total_duration_secs: row.7,
                created_at: row.8,
                updated_at: row.9,
            })
            .collect();

        Ok((summaries, total as u64))
    }

    /// Purge completed/failed jobs older than the specified number of days.
    /// Deletes jobs in batches to avoid long-running transactions.
    /// Returns the number of jobs deleted.
    /// Requirements: 7.1, 7.3
    async fn purge_jobs_older_than(&self, days: u32, batch_size: u32) -> Result<u64> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let mut total_deleted: u64 = 0;

        loop {
            // Get a batch of job IDs to delete
            let job_ids: Vec<String> = sqlx::query_scalar(
                r#"
                SELECT id FROM job 
                WHERE status IN ('COMPLETED', 'FAILED') 
                AND (completed_at < ? OR (completed_at IS NULL AND updated_at < ?))
                LIMIT ?
                "#,
            )
            .bind(&cutoff_str)
            .bind(&cutoff_str)
            .bind(batch_size as i64)
            .fetch_all(&self.pool)
            .await?;

            if job_ids.is_empty() {
                break;
            }

            let batch_count = job_ids.len() as u64;

            // Delete execution logs for these jobs first
            for job_id in &job_ids {
                sqlx::query("DELETE FROM job_execution_logs WHERE job_id = ?")
                    .bind(job_id)
                    .execute(&self.pool)
                    .await?;
            }

            // Delete the jobs
            for job_id in &job_ids {
                sqlx::query("DELETE FROM job WHERE id = ?")
                    .bind(job_id)
                    .execute(&self.pool)
                    .await?;
            }

            total_deleted += batch_count;

            // If we deleted less than batch_size, we're done
            if batch_count < batch_size as u64 {
                break;
            }
        }

        Ok(total_deleted)
    }

    /// Get IDs of jobs that are eligible for purging.
    /// Returns job IDs for completed/failed jobs older than the specified days.
    /// Requirements: 7.1, 7.3
    async fn get_purgeable_jobs(&self, days: u32, limit: u32) -> Result<Vec<String>> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        let job_ids: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT id FROM job 
            WHERE status IN ('COMPLETED', 'FAILED') 
            AND (completed_at < ? OR (completed_at IS NULL AND updated_at < ?))
            ORDER BY completed_at ASC, updated_at ASC
            LIMIT ?
            "#,
        )
        .bind(&cutoff_str)
        .bind(&cutoff_str)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(job_ids)
    }
}

#[cfg(test)]
mod stress_tests {
    use super::*;
    use dashmap::DashSet;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sqlite_claim_concurrent_no_double_claims() {
        const JOBS: usize = 50;
        const WORKERS: usize = 8;

        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("claim_smoke.db");
        let db_url = format!(
            "sqlite:{}?mode=rwc",
            db_path.to_string_lossy().replace('\\', "/")
        );

        let pool = crate::database::init_pool(&db_url).await.unwrap();
        crate::database::run_migrations(&pool).await.unwrap();

        let repo = Arc::new(SqlxJobRepository::new(pool));

        for i in 0..JOBS {
            let mut job = JobDbModel::new_pipeline(
                format!("input-{i}"),
                0,
                Some("streamer".to_string()),
                Some("session".to_string()),
                "{}",
            );
            job.job_type = "remux".to_string();
            job.priority = (i % 3) as i32;
            repo.create_job(&job).await.unwrap();
        }

        let claimed_ids = Arc::new(DashSet::<String>::new());

        let mut join_set = tokio::task::JoinSet::new();
        for _ in 0..WORKERS {
            let repo = repo.clone();
            let claimed_ids = claimed_ids.clone();
            join_set.spawn(async move {
                loop {
                    match repo.claim_next_pending_job(None).await.unwrap() {
                        Some(mut job) => {
                            assert!(
                                claimed_ids.insert(job.id.clone()),
                                "double-claim {}",
                                job.id
                            );
                            job.mark_completed();
                            repo.update_job(&job).await.unwrap();
                        }
                        None => break,
                    }
                }
            });
        }

        let joined = tokio::time::timeout(Duration::from_secs(10), async {
            while join_set.join_next().await.is_some() {}
        })
        .await;
        assert!(joined.is_ok(), "workers timed out");

        assert_eq!(claimed_ids.len(), JOBS, "not all jobs were claimed");

        let counts = repo.get_job_counts_by_status().await.unwrap();
        assert_eq!(counts.pending, 0);
        assert_eq!(counts.processing, 0);
        assert_eq!(counts.completed, JOBS as u64);
    }
}
