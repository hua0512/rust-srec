use crate::database::{models::PlatformConfig, repositories::errors::RepositoryError};
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait PlatformConfigRepository: Send + Sync {
    async fn create(&self, platform_config: &PlatformConfig) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<PlatformConfig>>;
    async fn find_all(&self) -> RepositoryResult<Vec<PlatformConfig>>;
    async fn update(&self, platform_config: &PlatformConfig) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqlitePlatformConfigRepository {
    db: SqlitePool,
}

impl SqlitePlatformConfigRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl PlatformConfigRepository for SqlitePlatformConfigRepository {
    async fn create(&self, platform_config: &PlatformConfig) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms, cookies, platform_specific_config, proxy_config)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
            platform_config.id,
            platform_config.platform_name,
            platform_config.fetch_delay_ms,
            platform_config.download_delay_ms,
            platform_config.cookies,
            platform_config.platform_specific_config,
            platform_config.proxy_config
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<PlatformConfig>> {
        let config = sqlx::query_as!(
            PlatformConfig,
            r#"
            SELECT 
                id as "id!", 
                platform_name as "platform_name!", 
                fetch_delay_ms as "fetch_delay_ms!", 
                download_delay_ms as "download_delay_ms!", 
                cookies as "cookies?", 
                platform_specific_config as "platform_specific_config?", 
                proxy_config as "proxy_config?",
                record_danmu as "record_danmu?"
            FROM platform_config
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(PlatformConfig::from);

        Ok(config)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<PlatformConfig>> {
        let configs = sqlx::query_as!(
            PlatformConfig,
            r#"
            SELECT 
                id as "id!", 
                platform_name as "platform_name!", 
                fetch_delay_ms as "fetch_delay_ms!", 
                download_delay_ms as "download_delay_ms!", 
                cookies as "cookies?", 
                platform_specific_config as "platform_specific_config?", 
                proxy_config as "proxy_config?",
                record_danmu as "record_danmu?"
            FROM platform_config
            "#
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .collect();

        Ok(configs)
    }

    async fn update(&self, platform_config: &PlatformConfig) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            UPDATE platform_config
            SET platform_name = ?, fetch_delay_ms = ?, download_delay_ms = ?, cookies = ?, platform_specific_config = ?, proxy_config = ?
            WHERE id = ?
            "#,
            platform_config.platform_name,
            platform_config.fetch_delay_ms,
            platform_config.download_delay_ms,
            platform_config.cookies,
            platform_config.platform_specific_config,
            platform_config.proxy_config,
            platform_config.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            DELETE FROM platform_config
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
