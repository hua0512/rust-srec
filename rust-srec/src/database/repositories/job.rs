//! Job repository.

use crate::database::begin_immediate;
use crate::database::models::{
    JobCounts, JobDbModel, JobExecutionLogDbModel, JobExecutionProgressDbModel, JobFilters,
    Pagination,
};
use crate::database::retry::retry_on_sqlite_busy;
use crate::{Error, Result};
use async_trait::async_trait;
use sqlx::SqlitePool;

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
    /// Returns the number of rows updated (0 means the job was already in a terminal state).
    async fn mark_job_failed(&self, id: &str, error: &str) -> Result<u64>;
    /// Mark a job as INTERRUPTED and set completed_at.
    /// Returns the number of rows updated (0 means the job was already in a terminal state).
    async fn mark_job_interrupted(&self, id: &str) -> Result<u64>;
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
    /// Update a job only if its current status matches `expected_status`.
    /// Returns the number of rows updated.
    async fn update_job_if_status(&self, job: &JobDbModel, expected_status: &str) -> Result<u64>;
    async fn reset_interrupted_jobs(&self) -> Result<i32>;
    /// Reset processing jobs to pending (for recovery on startup).
    async fn reset_processing_jobs(&self) -> Result<i32>;
    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32>;
    async fn delete_job(&self, id: &str) -> Result<()>;

    // Purge methods
    /// Purge completed/failed jobs older than the specified number of days.
    /// Deletes jobs in batches to avoid long-running transactions.
    /// Returns the number of jobs deleted.
    async fn purge_jobs_older_than(&self, days: u32, batch_size: u32) -> Result<u64>;

    /// Get IDs of jobs that are eligible for purging.
    /// Returns job IDs for completed/failed jobs older than the specified days.
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

    // Filtering and pagination
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

    // Statistics
    /// Get job counts by status.
    async fn get_job_counts_by_status(&self) -> Result<JobCounts>;

    /// Get average processing time for completed jobs in seconds.
    async fn get_avg_processing_time(&self) -> Result<Option<f64>>;

    // Atomic pipeline operations

    /// Cancel all pending/processing jobs in a pipeline.
    /// Returns the number of jobs cancelled.
    async fn cancel_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64>;

    /// Get all jobs in a pipeline.
    async fn get_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<Vec<JobDbModel>>;

    /// Delete all jobs in a pipeline and their associated data (logs, progress).
    /// Returns the number of jobs deleted.
    async fn delete_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64>;
}

/// SQLx implementation of JobRepository.
pub struct SqlxJobRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxJobRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
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
        retry_on_sqlite_busy("create_job", || async {
            sqlx::query(
                r#"
                INSERT INTO job (
                    id, job_type, status, config, state, created_at, updated_at,
                    input, outputs, priority, streamer_id, session_id,
                    started_at, completed_at, error, retry_count,
                     pipeline_id, execution_info,
                    duration_secs, queue_wait_secs, dag_step_execution_id
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&job.id)
            .bind(&job.job_type)
            .bind(&job.status)
            .bind(&job.config)
            .bind(&job.state)
            .bind(job.created_at)
            .bind(job.updated_at)
            .bind(&job.input)
            .bind(&job.outputs)
            .bind(job.priority)
            .bind(&job.streamer_id)
            .bind(&job.session_id)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(&job.error)
            .bind(job.retry_count)
            .bind(&job.pipeline_id)
            .bind(&job.execution_info)
            .bind(job.duration_secs)
            .bind(job.queue_wait_secs)
            .bind(&job.dag_step_execution_id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn update_job_status(&self, id: &str, status: &str) -> Result<()> {
        retry_on_sqlite_busy("update_job_status", || async {
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE job SET status = ?, updated_at = ? WHERE id = ?")
                .bind(status)
                .bind(now)
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn mark_job_failed(&self, id: &str, error: &str) -> Result<u64> {
        retry_on_sqlite_busy("mark_job_failed", || async {
            let now = crate::database::time::now_ms();
            let res = sqlx::query(
                "UPDATE job SET status = 'FAILED', completed_at = ?, updated_at = ?, error = ? WHERE id = ? AND status IN ('PENDING', 'PROCESSING')",
            )
            .bind(now)
            .bind(now)
            .bind(error)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
            Ok(res.rows_affected())
        })
        .await
    }

    async fn mark_job_interrupted(&self, id: &str) -> Result<u64> {
        retry_on_sqlite_busy("mark_job_interrupted", || async {
            let now = crate::database::time::now_ms();
            let res = sqlx::query(
                "UPDATE job SET status = 'INTERRUPTED', completed_at = ?, updated_at = ? WHERE id = ? AND status IN ('PENDING', 'PROCESSING')",
            )
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
            Ok(res.rows_affected())
        })
        .await
    }

    async fn reset_job_for_retry(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("reset_job_for_retry", || async {
            let now = crate::database::time::now_ms();
            let res = sqlx::query(
                "UPDATE job SET status = 'PENDING', started_at = NULL, completed_at = NULL, error = NULL, retry_count = retry_count + 1, updated_at = ? WHERE id = ? AND status IN ('FAILED', 'INTERRUPTED')",
            )
            .bind(now)
            .bind(id)
            .execute(&self.write_pool)
            .await?;

            if res.rows_affected() == 0 {
                let status: Option<String> =
                    sqlx::query_scalar("SELECT status FROM job WHERE id = ?")
                        .bind(id)
                        .fetch_optional(&self.pool)
                        .await?;

                return match status {
                    None => Err(Error::not_found("Job", id)),
                    Some(status) => Err(Error::InvalidStateTransition {
                        from: status.to_ascii_uppercase(),
                        to: "PENDING".to_string(),
                    }),
                };
            }
            Ok(())
        })
        .await
    }

    async fn count_pending_jobs(&self, job_types: Option<&[String]>) -> Result<u64> {
        let (sql, bind_job_types) = match job_types {
            Some(types) if !types.is_empty() => {
                let placeholders = std::iter::repeat_n("?", types.len())
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
        if bind_job_types && let Some(types) = job_types {
            for jt in types {
                query = query.bind(jt);
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
            .bind(progress.updated_at)
            .execute(&self.write_pool)
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
            let now = crate::database::time::now_ms();

            // Avoid taking a write lock when there are no pending jobs: first select the next job id,
            // then claim it with a conditional UPDATE. This reduces lock contention under load.
            //
            // We keep ordering consistent with list_jobs_filtered: priority DESC, created_at DESC.
            for _ in 0..3 {
                let next_id: Option<String> = match job_types {
                    Some(types) if !types.is_empty() => {
                        let placeholders = types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
                        let sql = format!(
                            r#"
                            SELECT id
                            FROM job
                            WHERE status = 'PENDING' AND job_type IN ({})
                            ORDER BY priority DESC, created_at DESC
                            LIMIT 1
                            "#,
                            placeholders
                        );

                        let mut query = sqlx::query_scalar::<_, String>(&sql);
                        for jt in types {
                            query = query.bind(jt);
                        }
                        query.fetch_optional(&self.pool).await?
                    }
                    _ => {
                        sqlx::query_scalar::<_, String>(
                            r#"
                            SELECT id
                            FROM job
                            WHERE status = 'PENDING'
                            ORDER BY priority DESC, created_at DESC
                            LIMIT 1
                            "#,
                        )
                        .fetch_optional(&self.pool)
                        .await?
                    }
                };

                let Some(next_id) = next_id else {
                    return Ok(None);
                };

                let claimed = sqlx::query_as::<_, JobDbModel>(
                    r#"
                    UPDATE job
                    SET status = 'PROCESSING',
                        started_at = ?,
                        updated_at = ?
                    WHERE id = ?
                      AND status = 'PENDING'
                    RETURNING *
                    "#,
                )
                .bind(now)
                .bind(now)
                .bind(&next_id)
                .fetch_optional(&self.write_pool)
                .await?;

                if claimed.is_some() {
                    return Ok(claimed);
                }
            }

            Ok(None)
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
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE job SET execution_info = ?, updated_at = ? WHERE id = ?")
                .bind(execution_info)
                .bind(now)
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn update_job_state(&self, id: &str, state: &str) -> Result<()> {
        retry_on_sqlite_busy("update_job_state", || async {
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE job SET state = ?, updated_at = ? WHERE id = ?")
                .bind(state)
                .bind(now)
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn update_job(&self, job: &JobDbModel) -> Result<()> {
        retry_on_sqlite_busy("update_job", || async {
            let now = crate::database::time::now_ms();
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
                    pipeline_id = ?,
                    execution_info = ?,
                    duration_secs = ?,
                    queue_wait_secs = ?,
                    dag_step_execution_id = ?
                WHERE id = ?
                "#,
            )
            .bind(&job.job_type)
            .bind(&job.status)
            .bind(&job.config)
            .bind(&job.state)
            .bind(now)
            .bind(&job.input)
            .bind(&job.outputs)
            .bind(job.priority)
            .bind(&job.streamer_id)
            .bind(&job.session_id)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(&job.error)
            .bind(job.retry_count)
            .bind(&job.pipeline_id)
            .bind(&job.execution_info)
            .bind(job.duration_secs)
            .bind(job.queue_wait_secs)
            .bind(&job.dag_step_execution_id)
            .bind(&job.id)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn update_job_if_status(&self, job: &JobDbModel, expected_status: &str) -> Result<u64> {
        retry_on_sqlite_busy("update_job_if_status", || async {
            let now = crate::database::time::now_ms();
            let res = sqlx::query(
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
                    pipeline_id = ?,
                    execution_info = ?,
                    duration_secs = ?,
                    queue_wait_secs = ?,
                    dag_step_execution_id = ?
                WHERE id = ? AND status = ?
                "#,
            )
            .bind(&job.job_type)
            .bind(&job.status)
            .bind(&job.config)
            .bind(&job.state)
            .bind(now)
            .bind(&job.input)
            .bind(&job.outputs)
            .bind(job.priority)
            .bind(&job.streamer_id)
            .bind(&job.session_id)
            .bind(job.started_at)
            .bind(job.completed_at)
            .bind(&job.error)
            .bind(job.retry_count)
            .bind(&job.pipeline_id)
            .bind(&job.execution_info)
            .bind(job.duration_secs)
            .bind(job.queue_wait_secs)
            .bind(&job.dag_step_execution_id)
            .bind(&job.id)
            .bind(expected_status)
            .execute(&self.write_pool)
            .await?;

            Ok(res.rows_affected())
        })
        .await
    }

    async fn reset_interrupted_jobs(&self) -> Result<i32> {
        retry_on_sqlite_busy("reset_interrupted_jobs", || async {
            let now = crate::database::time::now_ms();
            let result = sqlx::query(
                "UPDATE job SET status = 'PENDING', updated_at = ? WHERE status = 'INTERRUPTED'",
            )
            .bind(now)
            .execute(&self.write_pool)
            .await?;
            Ok(result.rows_affected() as i32)
        })
        .await
    }

    async fn reset_processing_jobs(&self) -> Result<i32> {
        retry_on_sqlite_busy("reset_processing_jobs", || async {
            let now = crate::database::time::now_ms();
            let result = sqlx::query(
                "UPDATE job SET status = 'PENDING', started_at = NULL, updated_at = ? WHERE status = 'PROCESSING'",
            )
            .bind(now)
            .execute(&self.write_pool)
            .await?;
            Ok(result.rows_affected() as i32)
        })
        .await
    }

    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32> {
        retry_on_sqlite_busy("cleanup_old_jobs", || async {
            // First delete execution logs for old completed/failed jobs
            let cutoff_ms = crate::database::time::now_ms()
                - chrono::Duration::days(retention_days as i64).num_milliseconds();

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
            .bind(cutoff_ms)
            .execute(&self.write_pool)
            .await?;

            // Then delete the jobs
            let result = sqlx::query(
                "DELETE FROM job WHERE status IN ('COMPLETED', 'FAILED') AND updated_at < ?",
            )
            .bind(cutoff_ms)
            .execute(&self.write_pool)
            .await?;

            Ok(result.rows_affected() as i32)
        })
        .await
    }

    async fn delete_job(&self, id: &str) -> Result<()> {
        retry_on_sqlite_busy("delete_job", || async {
            // Execution logs are deleted via CASCADE
            sqlx::query("DELETE FROM job WHERE id = ?")
                .bind(id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn add_execution_log(&self, log: &JobExecutionLogDbModel) -> Result<()> {
        retry_on_sqlite_busy("add_execution_log", || async {
            sqlx::query(
                r#"
                INSERT INTO job_execution_logs (id, job_id, entry, created_at, level, message)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(&log.id)
            .bind(&log.job_id)
            .bind(&log.entry)
            .bind(log.created_at)
            .bind(&log.level)
            .bind(&log.message)
            .execute(&self.write_pool)
            .await?;
            Ok(())
        })
        .await
    }

    async fn add_execution_logs(&self, logs: &[JobExecutionLogDbModel]) -> Result<()> {
        if logs.is_empty() {
            return Ok(());
        }

        retry_on_sqlite_busy("add_execution_logs", || async {
            const MAX_ROWS_PER_INSERT: usize = 1000;

            let mut tx = begin_immediate(&self.write_pool).await?;

            for chunk in logs.chunks(MAX_ROWS_PER_INSERT) {
                let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                    "INSERT INTO job_execution_logs (id, job_id, entry, created_at, level, message) ",
                );
                builder.push_values(chunk.iter(), |mut b, log| {
                    b.push_bind(&log.id)
                        .push_bind(&log.job_id)
                        .push_bind(&log.entry)
                        .push_bind(log.created_at)
                        .push_bind(&log.level)
                        .push_bind(&log.message);
                });

                let res = builder
                    .build()
                    .persistent(false)
                    .execute(&mut *tx)
                    .await;

                if let Err(e) = res {
                    return Err(Error::from(e));
                }
            }

            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
        let logs = sqlx::query_as::<_, JobExecutionLogDbModel>(
            "SELECT id, job_id, entry, created_at, level, message FROM job_execution_logs WHERE job_id = ? ORDER BY created_at",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(logs)
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
        .await?;

        Ok((full, total as u64))
    }

    async fn delete_execution_logs_for_job(&self, job_id: &str) -> Result<()> {
        retry_on_sqlite_busy("delete_execution_logs_for_job", || async {
            sqlx::query("DELETE FROM job_execution_logs WHERE job_id = ?")
                .bind(job_id)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
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
        if let Some(job_types) = &filters.job_types
            && !job_types.is_empty()
        {
            let placeholders = job_types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            conditions.push(format!("job_type IN ({})", placeholders));
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
            count_query = count_query.bind(from_date.timestamp_millis());
        }
        if let Some(to_date) = &filters.to_date {
            count_query = count_query.bind(to_date.timestamp_millis());
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
            data_query = data_query.bind(from_date.timestamp_millis());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.timestamp_millis());
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
        if let Some(job_types) = &filters.job_types
            && !job_types.is_empty()
        {
            let placeholders = job_types.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
            conditions.push(format!("job_type IN ({})", placeholders));
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
            data_query = data_query.bind(from_date.timestamp_millis());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.timestamp_millis());
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
        // Prefers the processor-reported duration_secs (most accurate),
        // falls back to timestamp difference (started_at to completed_at) if unavailable
        let result: Option<f64> = sqlx::query_scalar(
            r#"
            SELECT AVG(
                COALESCE(
                    duration_secs,
                    CASE 
                        WHEN started_at IS NOT NULL AND completed_at IS NOT NULL THEN
                            (completed_at - started_at) / 1000.0
                        ELSE NULL
                    END
                )
            ) as avg_time
            FROM job
            WHERE status = 'COMPLETED'
              AND (duration_secs IS NOT NULL OR (started_at IS NOT NULL AND completed_at IS NOT NULL))
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(result)
    }

    async fn cancel_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64> {
        retry_on_sqlite_busy("cancel_jobs_by_pipeline", || async {
            let now = crate::database::time::now_ms();

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
            .bind(now)
            .bind(now)
            .bind(pipeline_id)
            .execute(&self.write_pool)
            .await?;

            Ok(result.rows_affected())
        })
        .await
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

    async fn delete_jobs_by_pipeline(&self, pipeline_id: &str) -> Result<u64> {
        retry_on_sqlite_busy("delete_jobs_by_pipeline", || async {
            // Delete the jobs (logs and progress omitted, they are deleted via CASCADE)
            let result = sqlx::query("DELETE FROM job WHERE pipeline_id = ?")
                .bind(pipeline_id)
                .execute(&self.write_pool)
                .await?;

            Ok(result.rows_affected())
        })
        .await
    }

    /// Purge completed/failed jobs older than the specified number of days.
    /// Deletes jobs in batches to avoid long-running transactions.
    /// Returns the number of jobs deleted.
    async fn purge_jobs_older_than(&self, days: u32, batch_size: u32) -> Result<u64> {
        let cutoff_ms = crate::database::time::now_ms()
            - chrono::Duration::days(days as i64).num_milliseconds();

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
            .bind(cutoff_ms)
            .bind(cutoff_ms)
            .bind(batch_size as i64)
            .fetch_all(&self.pool)
            .await?;

            if job_ids.is_empty() {
                break;
            }

            let batch_count = job_ids.len() as u64;

            const MAX_IDS_PER_IN: usize = 900;

            retry_on_sqlite_busy("purge_jobs_older_than_delete_execution_logs", || async {
                for chunk in job_ids.chunks(MAX_IDS_PER_IN) {
                    let mut builder = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
                        "DELETE FROM job_execution_logs WHERE job_id IN (",
                    );
                    let mut separated = builder.separated(", ");
                    for id in chunk {
                        separated.push_bind(id);
                    }
                    separated.push_unseparated(")");

                    builder
                        .build()
                        .persistent(false)
                        .execute(&self.write_pool)
                        .await?;
                }
                Ok(())
            })
            .await?;

            retry_on_sqlite_busy("purge_jobs_older_than_delete_jobs", || async {
                for chunk in job_ids.chunks(MAX_IDS_PER_IN) {
                    let mut builder =
                        sqlx::QueryBuilder::<sqlx::Sqlite>::new("DELETE FROM job WHERE id IN (");
                    let mut separated = builder.separated(", ");
                    for id in chunk {
                        separated.push_bind(id);
                    }
                    separated.push_unseparated(")");

                    builder
                        .build()
                        .persistent(false)
                        .execute(&self.write_pool)
                        .await?;
                }
                Ok(())
            })
            .await?;

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
    async fn get_purgeable_jobs(&self, days: u32, limit: u32) -> Result<Vec<String>> {
        let cutoff_ms = crate::database::time::now_ms()
            - chrono::Duration::days(days as i64).num_milliseconds();

        let job_ids: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT id FROM job 
            WHERE status IN ('COMPLETED', 'FAILED') 
            AND (completed_at < ? OR (completed_at IS NULL AND updated_at < ?))
            ORDER BY completed_at ASC, updated_at ASC
            LIMIT ?
            "#,
        )
        .bind(cutoff_ms)
        .bind(cutoff_ms)
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

        let repo = Arc::new(SqlxJobRepository::new(pool.clone(), pool));

        for i in 0..JOBS {
            let mut job = JobDbModel::new_with_input(
                "remux",
                format!("input-{i}"),
                0,
                Some("streamer".to_string()),
                Some("session".to_string()),
                "{}",
            );
            job.priority = (i % 3) as i32;
            repo.create_job(&job).await.unwrap();
        }

        let claimed_ids = Arc::new(DashSet::<String>::new());

        let mut join_set = tokio::task::JoinSet::new();
        for _ in 0..WORKERS {
            let repo = repo.clone();
            let claimed_ids = claimed_ids.clone();
            join_set.spawn(async move {
                while let Some(mut job) = repo.claim_next_pending_job(None).await.unwrap() {
                    assert!(
                        claimed_ids.insert(job.id.clone()),
                        "double-claim {}",
                        job.id
                    );
                    job.mark_completed();
                    repo.update_job(&job).await.unwrap();
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
