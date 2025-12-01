//! Job repository.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{JobDbModel, JobExecutionLogDbModel};
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
    async fn cleanup_old_jobs(&self, retention_days: i32) -> Result<i32>;
    async fn delete_job(&self, id: &str) -> Result<()>;

    // Execution logs
    async fn add_execution_log(&self, log: &JobExecutionLogDbModel) -> Result<()>;
    async fn get_execution_logs(&self, job_id: &str) -> Result<Vec<JobExecutionLogDbModel>>;
    async fn delete_execution_logs_for_job(&self, job_id: &str) -> Result<()>;
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
            INSERT INTO job (id, job_type, status, config, state, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&job.id)
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(&job.created_at)
        .bind(&job.updated_at)
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
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&job.job_type)
        .bind(&job.status)
        .bind(&job.config)
        .bind(&job.state)
        .bind(&now)
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
}
