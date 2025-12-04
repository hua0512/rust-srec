//! Job repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{JobCounts, JobDbModel, JobExecutionLogDbModel, JobFilters, Pagination};
use crate::{Error, Result};

/// Job repository trait.
#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn get_job(&self, id: &str) -> Result<JobDbModel>;
    async fn list_pending_jobs(&self, job_type: &str) -> Result<Vec<JobDbModel>>;
    async fn list_jobs_by_status(&self, status: &str) -> Result<Vec<JobDbModel>>;
    async fn list_recent_jobs(&self, limit: i32) -> Result<Vec<JobDbModel>>;
    async fn create_job(&self, job: &JobDbModel) -> Result<()>;
    async fn update_job_status(&self, id: &str, status: &str) -> Result<()>;
    async fn update_job_state(&self, id: &str, state: &str) -> Result<()>;
    async fn update_job(&self, job: &JobDbModel) -> Result<()>;
    async fn reset_interrupted_jobs(&self) -> Result<i32>;
    /// Reset processing jobs to pending (for recovery on startup).
    async fn reset_processing_jobs(&self) -> Result<i32>;
    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32>;
    async fn delete_job(&self, id: &str) -> Result<()>;

    // Execution logs
    async fn add_execution_log(&self, log: &JobExecutionLogDbModel) -> Result<()>;
    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>>;
    async fn delete_execution_logs_for_job(&self, job_id: &str) -> Result<()>;

    // Filtering and pagination (Requirements 1.1, 1.3, 1.4, 1.5)
    /// List jobs with optional filters and pagination.
    /// Returns a tuple of (jobs, total_count).
    async fn list_jobs_filtered(
        &self,
        filters: &JobFilters,
        pagination: &Pagination,
    ) -> Result<(Vec<JobDbModel>, u64)>;

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
        output: &str,
        next_job: Option<&JobDbModel>,
    ) -> Result<Option<String>>;
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
                next_job_type, remaining_steps, pipeline_id
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                pipeline_id = ?
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

    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>> {
        let logs = sqlx::query_as::<_, JobExecutionLogDbModel>(
            "SELECT * FROM job_execution_logs WHERE job_id = ? ORDER BY created_at",
        )
        .bind(job_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(logs)
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
            // streamer_id is now a direct column
            conditions.push("streamer_id = ?".to_string());
        }
        if filters.session_id.is_some() {
            // session_id is now a direct column
            conditions.push("session_id = ?".to_string());
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
        if let Some(from_date) = &filters.from_date {
            count_query = count_query.bind(from_date.to_rfc3339());
        }
        if let Some(to_date) = &filters.to_date {
            count_query = count_query.bind(to_date.to_rfc3339());
        }
        if let Some(job_type) = &filters.job_type {
            count_query = count_query.bind(job_type);
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
        if let Some(from_date) = &filters.from_date {
            data_query = data_query.bind(from_date.to_rfc3339());
        }
        if let Some(to_date) = &filters.to_date {
            data_query = data_query.bind(to_date.to_rfc3339());
        }
        if let Some(job_type) = &filters.job_type {
            data_query = data_query.bind(job_type);
        }

        // Bind pagination parameters
        data_query = data_query.bind(pagination.limit as i64);
        data_query = data_query.bind(pagination.offset as i64);

        let jobs = data_query.fetch_all(&self.pool).await?;

        Ok((jobs, total_count))
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
        output: &str,
        next_job: Option<&JobDbModel>,
    ) -> Result<Option<String>> {
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
                outputs = ?
            WHERE id = ?
            "#,
        )
        .bind(&now)
        .bind(&now)
        .bind(format!("[\"{}\"]", output))
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
                    next_job_type, remaining_steps, pipeline_id
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            .execute(&mut *tx)
            .await?;

            Some(job.id.clone())
        } else {
            None
        };

        // 3. Commit transaction - atomic!
        tx.commit().await?;

        Ok(next_job_id)
    }
}
