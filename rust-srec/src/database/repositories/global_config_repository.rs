use crate::database::models::GlobalConfig as DbGlobalConfig;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::global_config as domain;
use async_trait::async_trait;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait]
pub trait GlobalConfigRepository: Send + Sync {
    async fn get(&self) -> RepositoryResult<Option<domain::GlobalConfig>>;
    async fn update(&self, global_config: &domain::GlobalConfig) -> RepositoryResult<()>;
}

pub struct SqliteGlobalConfigRepository {
    db: SqlitePool,
}

impl SqliteGlobalConfigRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GlobalConfigRepository for SqliteGlobalConfigRepository {
    async fn get(&self) -> RepositoryResult<Option<domain::GlobalConfig>> {
        let config = sqlx::query_as!(
            DbGlobalConfig,
            r#"
            SELECT
                id as "id!",
                output_folder as "output_folder!",
                output_filename_template as "output_filename_template!",
                output_file_format as "output_file_format!",
                max_concurrent_downloads,
                max_concurrent_uploads,
                streamer_check_delay_ms,
                offline_check_delay_ms,
                offline_check_count,
                default_download_engine as "default_download_engine!",
                proxy_config as "proxy_config!",
                min_segment_size_bytes,
                max_download_duration_secs,
                max_part_size_bytes,
                record_danmu
            FROM global_config
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(domain::GlobalConfig::from);

        Ok(config)
    }

    async fn update(&self, global_config: &domain::GlobalConfig) -> RepositoryResult<()> {
        let db_config = DbGlobalConfig::from(global_config);
        sqlx::query!(
            r#"
            UPDATE global_config
            SET
                output_folder = ?,
                output_filename_template = ?,
                output_file_format = ?,
                max_concurrent_downloads = ?,
                max_concurrent_uploads = ?,
                streamer_check_delay_ms = ?,
                offline_check_delay_ms = ?,
                offline_check_count = ?,
                default_download_engine = ?,
                proxy_config = ?,
                min_segment_size_bytes = ?,
                max_download_duration_secs = ?,
                max_part_size_bytes = ?,
                record_danmu = ?
            WHERE id = ?
            "#,
            db_config.output_folder,
            db_config.output_filename_template,
            db_config.output_file_format,
            db_config.max_concurrent_downloads,
            db_config.max_concurrent_uploads,
            db_config.streamer_check_delay_ms,
            db_config.offline_check_delay_ms,
            db_config.offline_check_count,
            db_config.default_download_engine,
            db_config.proxy_config,
            db_config.min_segment_size_bytes,
            db_config.max_download_duration_secs,
            db_config.max_part_size_bytes,
            db_config.record_danmu,
            db_config.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }
}
