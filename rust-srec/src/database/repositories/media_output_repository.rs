use crate::database::converters::string_to_datetime;
use crate::database::models::MediaOutput as DbMediaOutput;
use crate::database::repositories::RepositoryError;
use crate::domain::media_output::MediaOutput;
use crate::domain::types::MediaType;
use async_trait::async_trait;
use sqlx::SqlitePool;
use std::str::FromStr;

use super::errors::RepositoryResult;

#[async_trait]
pub trait MediaOutputRepository: Send + Sync {
    async fn create(&self, media_output: &MediaOutput) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<MediaOutput>>;
    async fn find_by_live_session_id(&self, session_id: &str)
    -> RepositoryResult<Vec<MediaOutput>>;
    async fn find_all(&self) -> RepositoryResult<Vec<MediaOutput>>;
    async fn update(&self, media_output: &MediaOutput) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteMediaOutputRepository {
    pool: SqlitePool,
}

impl SqliteMediaOutputRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl MediaOutputRepository for SqliteMediaOutputRepository {
    async fn create(&self, media_output: &MediaOutput) -> RepositoryResult<()> {
        let file_type = media_output.file_type.to_string();
        let size_bytes = media_output.size_bytes as i64;
        sqlx::query!(
            r#"
            INSERT INTO media_outputs (id, session_id, parent_media_output_id, file_path, file_type, size_bytes, created_at)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            media_output.id,
            media_output.session_id,
            media_output.parent_id,
            media_output.file_path,
            file_type,
            size_bytes,
            media_output.created_at
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<MediaOutput>> {
        let db_output = sqlx::query_as!(
            DbMediaOutput,
            r#"
            SELECT 
                id as "id!",
                session_id as "session_id!",
                parent_media_output_id as "parent_media_output_id?",
                file_path as "file_path!",
                file_type as "file_type!",
                size_bytes as "size_bytes!",
                created_at as "created_at!"
            FROM media_outputs 
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(db_output) = db_output {
            Ok(Some(MediaOutput {
                id: db_output.id,
                session_id: db_output.session_id,
                file_path: db_output.file_path,
                file_type: MediaType::from_str(&db_output.file_type)
                    .map_err(|e| RepositoryError::Validation(e))?,
                size_bytes: db_output.size_bytes as u64,
                parent_id: db_output.parent_media_output_id,
                created_at: string_to_datetime(&db_output.created_at)?,
            }))
        } else {
            Ok(None)
        }
    }

    async fn find_by_live_session_id(
        &self,
        session_id: &str,
    ) -> RepositoryResult<Vec<MediaOutput>> {
        let db_outputs = sqlx::query_as!(
            DbMediaOutput,
            r#"
            SELECT
                id as "id!",
                session_id as "session_id!",
                parent_media_output_id as "parent_media_output_id?",
                file_path as "file_path!",
                file_type as "file_type!",
                size_bytes as "size_bytes!",
                created_at as "created_at!"
            FROM media_outputs
            WHERE session_id = ?
            "#,
            session_id
        )
        .fetch_all(&self.pool)
        .await?;

        let mut outputs = Vec::new();
        for db_output in db_outputs {
            outputs.push(MediaOutput {
                id: db_output.id,
                session_id: db_output.session_id,
                file_path: db_output.file_path,
                file_type: MediaType::from_str(&db_output.file_type)
                    .map_err(|e| RepositoryError::Validation(e))?,
                created_at: string_to_datetime(&db_output.created_at)?,
                size_bytes: db_output.size_bytes as u64,
                parent_id: db_output.parent_media_output_id,
            });
        }
        Ok(outputs)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<MediaOutput>> {
        let db_outputs = sqlx::query_as!(
            DbMediaOutput,
            r#"
            SELECT 
                id as "id!",
                session_id as "session_id!",
                parent_media_output_id as "parent_media_output_id?",
                file_path as "file_path!",
                file_type as "file_type!",
                size_bytes as "size_bytes!",
                created_at as "created_at!"
            FROM media_outputs
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut outputs = Vec::new();
        for db_output in db_outputs {
            outputs.push(MediaOutput {
                id: db_output.id,
                session_id: db_output.session_id,
                file_path: db_output.file_path,
                file_type: MediaType::from_str(&db_output.file_type)
                    .map_err(|e| RepositoryError::Validation(e))?,
                created_at: string_to_datetime(&db_output.created_at)?,
                size_bytes: db_output.size_bytes as u64,
                parent_id: db_output.parent_media_output_id,
            });
        }
        Ok(outputs)
    }

    async fn update(&self, media_output: &MediaOutput) -> RepositoryResult<()> {
        let file_type = media_output.file_type.to_string();
        let size_bytes = media_output.size_bytes as i64;
        sqlx::query!(
            r#"
            UPDATE media_outputs
            SET file_path = ?, file_type = ?, size_bytes = ?, parent_media_output_id = ?
            WHERE id = ?
            "#,
            media_output.file_path,
            file_type,
            size_bytes,
            media_output.parent_id,
            media_output.id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!("DELETE FROM media_outputs WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
