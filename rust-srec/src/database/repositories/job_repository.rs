use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::job::Job;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait JobRepository {
    async fn create(&self, job: &Job) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<Job>>;
    async fn find_all(&self) -> RepositoryResult<Vec<Job>>;
    async fn update(&self, job: &Job) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteJobRepository {
    db: SqlitePool,
}

impl SqliteJobRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl JobRepository for SqliteJobRepository {
    async fn create(&self, job: &Job) -> RepositoryResult<()> {
        let model = models::Job::from(job);
        sqlx::query!(
            r#"
            INSERT INTO job (id, job_type, status, context, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
            model.id,
            model.job_type,
            model.status,
            model.context,
            model.created_at,
            model.updated_at
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<Job>> {
        let job = sqlx::query_as!(
            models::Job,
            r#"
            SELECT
                id as "id!",
                job_type,
                status,
                context,
                created_at,
                updated_at
            FROM job
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(Job::from);

        Ok(job)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<Job>> {
        let job = sqlx::query_as!(
            models::Job,
            r#"
            SELECT
                id as "id!",
                job_type,
                status,
                context,
                created_at,
                updated_at
            FROM job
            "#,
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(Job::from)
        .collect();

        Ok(job)
    }

    async fn update(&self, job: &Job) -> RepositoryResult<()> {
        let model = models::Job::from(job);
        sqlx::query!(
            r#"
            UPDATE job
            SET job_type = ?, status = ?, context = ?, updated_at = ?
            WHERE id = ?
            "#,
            model.job_type,
            model.status,
            model.context,
            model.updated_at,
            model.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            DELETE FROM job
            WHERE id = ?
            "#,
            id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }
}
