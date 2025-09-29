use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::upload_record::UploadRecord;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait UploadRecordRepository {
    async fn create(&self, upload_record: &UploadRecord) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<UploadRecord>>;
    async fn find_all(&self) -> RepositoryResult<Vec<UploadRecord>>;
    async fn update(&self, upload_record: &UploadRecord) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteUploadRecordRepository {
    db: SqlitePool,
}

impl SqliteUploadRecordRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl UploadRecordRepository for SqliteUploadRecordRepository {
    async fn create(&self, upload_record: &UploadRecord) -> RepositoryResult<()> {
        let model = models::UploadRecord::from(upload_record);
        sqlx::query!(
            r#"
            INSERT INTO upload_record (id, media_output_id, platform, remote_path, status, metadata, created_at, completed_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            model.id,
            model.media_output_id,
            model.platform,
            model.remote_path,
            model.status,
            model.metadata,
            model.created_at,
            model.completed_at
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<UploadRecord>> {
        let record = sqlx::query_as!(
            models::UploadRecord,
            r#"
            SELECT
                id as "id!",
                media_output_id as "media_output_id!",
                platform as "platform!",
                remote_path as "remote_path!",
                status as "status!",
                metadata as "metadata?",
                created_at as "created_at!",
                completed_at as "completed_at?"
            FROM upload_record
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(UploadRecord::from);

        Ok(record)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<UploadRecord>> {
        let records = sqlx::query_as!(
            models::UploadRecord,
            r#"
            SELECT
                id as "id!",
                media_output_id as "media_output_id!",
                platform as "platform!",
                remote_path as "remote_path!",
                status as "status!",
                metadata as "metadata?",
                created_at as "created_at!",
                completed_at as "completed_at?"
            FROM upload_record
            "#,
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(UploadRecord::from)
        .collect();

        Ok(records)
    }

    async fn update(&self, upload_record: &UploadRecord) -> RepositoryResult<()> {
        let model = models::UploadRecord::from(upload_record);
        sqlx::query!(
            r#"
            UPDATE upload_record
            SET media_output_id = ?, platform = ?, remote_path = ?, status = ?, metadata = ?, completed_at = ?
            WHERE id = ?
            "#,
            model.media_output_id,
            model.platform,
            model.remote_path,
            model.status,
            model.metadata,
            model.completed_at,
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
            DELETE FROM upload_record
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
