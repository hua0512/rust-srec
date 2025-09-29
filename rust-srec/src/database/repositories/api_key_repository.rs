use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::api_key::ApiKey;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait ApiKeyRepository {
    async fn create(&self, api_key: &ApiKey) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<ApiKey>>;
    async fn find_all(&self) -> RepositoryResult<Vec<ApiKey>>;
    async fn update(&self, api_key: &ApiKey) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteApiKeyRepository {
    db: SqlitePool,
}

impl SqliteApiKeyRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl ApiKeyRepository for SqliteApiKeyRepository {
    async fn create(&self, api_key: &ApiKey) -> RepositoryResult<()> {
        let model = models::ApiKey::from(api_key);
        sqlx::query!(
            r#"
            INSERT INTO api_key (id, key_hash, name, role, created_at)
            VALUES (?, ?, ?, ?, ?)
            "#,
            model.id,
            model.key_hash,
            model.name,
            model.role,
            model.created_at
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<ApiKey>> {
        let api_key = sqlx::query_as!(
            models::ApiKey,
            r#"
            SELECT id as "id!", key_hash as "key_hash!", name as "name!", role as "role!", created_at as "created_at!"
            FROM api_key
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(ApiKey::from);

        Ok(api_key)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<ApiKey>> {
        let api_keys = sqlx::query_as!(
            models::ApiKey,
            r#"
            SELECT id as "id!", key_hash as "key_hash!", name as "name!", role as "role!", created_at as "created_at!"
            FROM api_key
            "#,
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(ApiKey::from)
        .collect();

        Ok(api_keys)
    }

    async fn update(&self, api_key: &ApiKey) -> RepositoryResult<()> {
        let model = models::ApiKey::from(api_key);
        sqlx::query!(
            r#"
            UPDATE api_key
            SET key_hash = ?, name = ?, role = ?
            WHERE id = ?
            "#,
            model.key_hash,
            model.name,
            model.role,
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
            DELETE FROM api_key
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
