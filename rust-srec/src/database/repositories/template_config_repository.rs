use crate::database::models;
use crate::database::repositories::errors::RepositoryError;
use crate::domain::template_config::TemplateConfig;
use sqlx::SqlitePool;

use super::errors::RepositoryResult;

#[async_trait::async_trait]
pub trait TemplateConfigRepository: Send + Sync {
    async fn create(&self, template_config: &TemplateConfig) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<TemplateConfig>>;
    async fn find_all(&self) -> RepositoryResult<Vec<TemplateConfig>>;
    async fn update(&self, template_config: &TemplateConfig) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteTemplateConfigRepository {
    db: SqlitePool,
}

impl SqliteTemplateConfigRepository {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl TemplateConfigRepository for SqliteTemplateConfigRepository {
    async fn create(&self, template_config: &TemplateConfig) -> RepositoryResult<()> {
        let platform_overrides = template_config
            .platform_overrides
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let download_retry_policy = template_config
            .download_retry_policy
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let danmu_sampling_config = template_config
            .danmu_sampling_config
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let engines_override = template_config
            .engines_override
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let proxy_config = template_config
            .proxy_config
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let event_hooks = template_config
            .event_hooks
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        let max_bitrate = template_config.max_bitrate.map(|v| v as i64);
        let min_segment_size_bytes = template_config.min_segment_size_bytes.map(|v| v as i64);
        let max_download_duration_secs =
            template_config.max_download_duration_secs.map(|v| v as i64);
        let max_part_size_bytes = template_config.max_part_size_bytes.map(|v| v as i64);

        sqlx::query!(
            r#"
            INSERT INTO template_config (
                id, name, output_folder, output_filename_template, max_bitrate, cookies,
                output_file_format, min_segment_size_bytes, max_download_duration_secs,
                max_part_size_bytes, record_danmu, platform_overrides, download_retry_policy,
                danmu_sampling_config, download_engine, engines_override, proxy_config, event_hooks
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            template_config.id,
            template_config.name,
            template_config.output_folder,
            template_config.output_filename_template,
            max_bitrate,
            template_config.cookies,
            template_config.output_file_format,
            min_segment_size_bytes,
            max_download_duration_secs,
            max_part_size_bytes,
            template_config.record_danmu,
            platform_overrides,
            download_retry_policy,
            danmu_sampling_config,
            template_config.download_engine,
            engines_override,
            proxy_config,
            event_hooks
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<TemplateConfig>> {
        let config = sqlx::query_as!(
            models::TemplateConfig,
            r#"
            SELECT
                id as "id!",
                name as "name!", 
                output_folder as "output_folder?", 
                output_filename_template as "output_filename_template?", 
                max_bitrate as "max_bitrate?", 
                cookies as "cookies?",
                output_file_format as "output_file_format?", 
                min_segment_size_bytes as "min_segment_size_bytes?", 
                max_download_duration_secs as "max_download_duration_secs?",
                max_part_size_bytes as "max_part_size_bytes?", 
                record_danmu as "record_danmu?", 
                platform_overrides as "platform_overrides?", 
                download_retry_policy as "download_retry_policy?",
                danmu_sampling_config as "danmu_sampling_config?", 
                download_engine as "download_engine?", 
                engines_override as "engines_override?", 
                proxy_config as "proxy_config?", 
                event_hooks as "event_hooks?"
            FROM template_config
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .map(TemplateConfig::from);

        Ok(config)
    }

    async fn find_all(&self) -> RepositoryResult<Vec<TemplateConfig>> {
        let configs = sqlx::query_as!(
            models::TemplateConfig,
            r#"
            SELECT
                id as "id!",
                name as "name!", 
                output_folder as "output_folder?", 
                output_filename_template as "output_filename_template?", 
                max_bitrate as "max_bitrate?", 
                cookies as "cookies?",
                output_file_format as "output_file_format?", 
                min_segment_size_bytes as "min_segment_size_bytes?", 
                max_download_duration_secs as "max_download_duration_secs?",
                max_part_size_bytes as "max_part_size_bytes?", 
                record_danmu as "record_danmu?", 
                platform_overrides as "platform_overrides?", 
                download_retry_policy as "download_retry_policy?",
                danmu_sampling_config as "danmu_sampling_config?", 
                download_engine as "download_engine?", 
                engines_override as "engines_override?", 
                proxy_config as "proxy_config?", 
                event_hooks as "event_hooks?"
            FROM template_config
            "#
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?
        .into_iter()
        .map(TemplateConfig::from)
        .collect();

        Ok(configs)
    }

    async fn update(&self, template_config: &TemplateConfig) -> RepositoryResult<()> {
        let platform_overrides = template_config
            .platform_overrides
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let download_retry_policy = template_config
            .download_retry_policy
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let danmu_sampling_config = template_config
            .danmu_sampling_config
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let engines_override = template_config
            .engines_override
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let proxy_config = template_config
            .proxy_config
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());
        let event_hooks = template_config
            .event_hooks
            .as_ref()
            .and_then(|v| serde_json::to_string(v).ok());

        let max_bitrate = template_config.max_bitrate.map(|v| v as i64);
        let min_segment_size_bytes = template_config.min_segment_size_bytes.map(|v| v as i64);
        let max_download_duration_secs =
            template_config.max_download_duration_secs.map(|v| v as i64);
        let max_part_size_bytes = template_config.max_part_size_bytes.map(|v| v as i64);

        sqlx::query!(
            r#"
            UPDATE template_config
            SET
                name = ?, output_folder = ?, output_filename_template = ?, max_bitrate = ?,
                cookies = ?, output_file_format = ?, min_segment_size_bytes = ?,
                max_download_duration_secs = ?, max_part_size_bytes = ?, record_danmu = ?,
                platform_overrides = ?, download_retry_policy = ?, danmu_sampling_config = ?,
                download_engine = ?, engines_override = ?, proxy_config = ?, event_hooks = ?
            WHERE id = ?
            "#,
            template_config.name,
            template_config.output_folder,
            template_config.output_filename_template,
            max_bitrate,
            template_config.cookies,
            template_config.output_file_format,
            min_segment_size_bytes,
            max_download_duration_secs,
            max_part_size_bytes,
            template_config.record_danmu,
            platform_overrides,
            download_retry_policy,
            danmu_sampling_config,
            template_config.download_engine,
            engines_override,
            proxy_config,
            event_hooks,
            template_config.id
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        Ok(())
    }

    async fn delete(&self, id: &str) -> RepositoryResult<()> {
        sqlx::query!(
            r#"
            DELETE FROM template_config
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
