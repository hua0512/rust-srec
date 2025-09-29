use crate::database::models::{self, EngineConfiguration};
use crate::database::repositories::errors::RepositoryError;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait EngineConfigRepository {
    async fn create(&self, engine_config: &EngineConfiguration) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<EngineConfiguration>>;
    async fn find_all(&self) -> RepositoryResult<Vec<EngineConfiguration>>;
    async fn update(&self, engine_config: &EngineConfiguration) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteEngineConfigRepository {
    db: SqlitePool,
}

impl SqliteEngineConfigRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl EngineConfigRepository for SqliteEngineConfigRepository {
    async fn create(&self, engine_config: &EngineConfiguration) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            INSERT INTO engine_configuration (id, name, engine_type, config)
            VALUES (?, ?, ?, ?)
            "#,
            engine_config.id,
            engine_config.name,
            engine_config.engine_type,
            engine_config.config
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<EngineConfiguration>> {
        let config = sqlx::query_as!(
            models::EngineConfiguration,
            r#"
            SELECT id as "id!", name as "name!", engine_type as "engine_type!", config as "config!"
            FROM engine_configuration
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(EngineConfiguration::from);

        Ok(config)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<EngineConfiguration>> {
        let configs = sqlx::query_as!(
            models::EngineConfiguration,
            r#"
            SELECT id as "id!", name as "name!", engine_type as "engine_type!", config as "config!"
            FROM engine_configuration
            "#
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(EngineConfiguration::from)
        .collect();

        Ok(configs)
    }

    async fn update(&self, engine_config: &EngineConfiguration) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            UPDATE engine_configuration
            SET name = ?, engine_type = ?, config = ?
            WHERE id = ?
            "#,
            engine_config.name,
            engine_config.engine_type,
            engine_config.config,
            engine_config.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            DELETE FROM engine_configuration
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
