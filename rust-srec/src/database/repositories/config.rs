//! Configuration repository.

use async_trait::async_trait;
use chrono::Utc;
use sqlx::SqlitePool;

use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, TemplateConfigDbModel,
};
use crate::{Error, Result};

/// Configuration repository trait.
#[async_trait]
pub trait ConfigRepository: Send + Sync {
    // Global Config
    async fn get_global_config(&self) -> Result<GlobalConfigDbModel>;
    async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()>;
    async fn create_global_config(&self, config: &GlobalConfigDbModel) -> Result<()>;

    // Platform Config
    async fn get_platform_config(&self, id: &str) -> Result<PlatformConfigDbModel>;
    async fn get_platform_config_by_name(&self, name: &str) -> Result<PlatformConfigDbModel>;
    async fn list_platform_configs(&self) -> Result<Vec<PlatformConfigDbModel>>;
    async fn create_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()>;
    async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()>;
    async fn delete_platform_config(&self, id: &str) -> Result<()>;

    // Template Config
    async fn get_template_config(&self, id: &str) -> Result<TemplateConfigDbModel>;
    async fn get_template_config_by_name(&self, name: &str) -> Result<TemplateConfigDbModel>;
    async fn list_template_configs(&self) -> Result<Vec<TemplateConfigDbModel>>;
    async fn create_template_config(&self, config: &TemplateConfigDbModel) -> Result<()>;
    async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()>;
    async fn delete_template_config(&self, id: &str) -> Result<()>;

    // Engine Config
    async fn get_engine_config(&self, id: &str) -> Result<EngineConfigurationDbModel>;
    async fn list_engine_configs(&self) -> Result<Vec<EngineConfigurationDbModel>>;
    async fn create_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()>;
    async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()>;
    async fn delete_engine_config(&self, id: &str) -> Result<()>;
}

/// SQLx implementation of ConfigRepository.
pub struct SqlxConfigRepository {
    pool: SqlitePool,
}

impl SqlxConfigRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ConfigRepository for SqlxConfigRepository {
    async fn get_global_config(&self) -> Result<GlobalConfigDbModel> {
        let config =
            sqlx::query_as::<_, GlobalConfigDbModel>("SELECT * FROM global_config LIMIT 1")
                .fetch_optional(&self.pool)
                .await?;

        match config {
            Some(c) => Ok(c),
            None => {
                let default_config = GlobalConfigDbModel::default();
                self.create_global_config(&default_config).await?;
                Ok(default_config)
            }
        }
    }

    async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE global_config SET
                output_folder = ?,
                output_filename_template = ?,
                output_file_format = ?,
                min_segment_size_bytes = ?,
                max_download_duration_secs = ?,
                max_part_size_bytes = ?,
                record_danmu = ?,
                max_concurrent_downloads = ?,
                max_concurrent_uploads = ?,
                streamer_check_delay_ms = ?,
                proxy_config = ?,
                offline_check_delay_ms = ?,
                offline_check_count = ?,
                default_download_engine = ?,
                max_concurrent_cpu_jobs = ?,
                max_concurrent_io_jobs = ?,
                job_history_retention_days = ?,
                session_gap_time_secs = ?,
                pipeline = ?
            WHERE id = ?
            "#,
        )
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(config.record_danmu)
        .bind(config.max_concurrent_downloads)
        .bind(config.max_concurrent_uploads)
        .bind(config.streamer_check_delay_ms)
        .bind(&config.proxy_config)
        .bind(config.offline_check_delay_ms)
        .bind(config.offline_check_count)
        .bind(&config.default_download_engine)
        .bind(config.max_concurrent_cpu_jobs)
        .bind(config.max_concurrent_io_jobs)
        .bind(config.job_history_retention_days)
        .bind(config.session_gap_time_secs)
        .bind(&config.pipeline)
        .bind(&config.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn create_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO global_config (
                id, output_folder, output_filename_template, output_file_format,
                min_segment_size_bytes, max_download_duration_secs, max_part_size_bytes,
                record_danmu, max_concurrent_downloads, max_concurrent_uploads,
                streamer_check_delay_ms, proxy_config, offline_check_delay_ms,
                offline_check_count, default_download_engine, max_concurrent_cpu_jobs,
                max_concurrent_io_jobs, job_history_retention_days, session_gap_time_secs, pipeline
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&config.id)
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(config.record_danmu)
        .bind(config.max_concurrent_downloads)
        .bind(config.max_concurrent_uploads)
        .bind(config.streamer_check_delay_ms)
        .bind(&config.proxy_config)
        .bind(config.offline_check_delay_ms)
        .bind(config.offline_check_count)
        .bind(&config.default_download_engine)
        .bind(config.max_concurrent_cpu_jobs)
        .bind(config.max_concurrent_io_jobs)
        .bind(config.job_history_retention_days)
        .bind(config.session_gap_time_secs)
        .bind(&config.pipeline)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_platform_config(&self, id: &str) -> Result<PlatformConfigDbModel> {
        sqlx::query_as::<_, PlatformConfigDbModel>("SELECT * FROM platform_config WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("PlatformConfig", id))
    }

    async fn get_platform_config_by_name(&self, name: &str) -> Result<PlatformConfigDbModel> {
        sqlx::query_as::<_, PlatformConfigDbModel>(
            "SELECT * FROM platform_config WHERE platform_name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("PlatformConfig", name))
    }

    async fn list_platform_configs(&self) -> Result<Vec<PlatformConfigDbModel>> {
        let configs = sqlx::query_as::<_, PlatformConfigDbModel>(
            "SELECT * FROM platform_config ORDER BY platform_name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO platform_config (
                id, platform_name, fetch_delay_ms, download_delay_ms,
                cookies, platform_specific_config, proxy_config, record_danmu,
                output_folder, output_filename_template, download_engine, stream_selection_config,
                output_file_format, min_segment_size_bytes, max_download_duration_secs, max_part_size_bytes,
                download_retry_policy, event_hooks
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&config.id)
        .bind(&config.platform_name)
        .bind(config.fetch_delay_ms)
        .bind(config.download_delay_ms)
        .bind(&config.cookies)
        .bind(&config.platform_specific_config)
        .bind(&config.proxy_config)
        .bind(config.record_danmu)
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.download_engine)
        .bind(&config.stream_selection_config)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(&config.download_retry_policy)
        .bind(&config.event_hooks)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE platform_config SET
                platform_name = ?,
                fetch_delay_ms = ?,
                download_delay_ms = ?,
                cookies = ?,
                platform_specific_config = ?,
                proxy_config = ?,
                record_danmu = ?,
                output_folder = ?,
                output_filename_template = ?,
                download_engine = ?,
                download_engine = ?,
                stream_selection_config = ?,
                output_file_format = ?,
                min_segment_size_bytes = ?,
                max_download_duration_secs = ?,
                max_part_size_bytes = ?,
                download_retry_policy = ?,
                event_hooks = ?
            WHERE id = ?
            "#,
        )
        .bind(&config.platform_name)
        .bind(config.fetch_delay_ms)
        .bind(config.download_delay_ms)
        .bind(&config.cookies)
        .bind(&config.platform_specific_config)
        .bind(&config.proxy_config)
        .bind(config.record_danmu)
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.download_engine)
        .bind(&config.stream_selection_config)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(&config.download_retry_policy)
        .bind(&config.event_hooks)
        .bind(&config.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_platform_config(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM platform_config WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_template_config(&self, id: &str) -> Result<TemplateConfigDbModel> {
        sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("TemplateConfig", id))
    }

    async fn get_template_config_by_name(&self, name: &str) -> Result<TemplateConfigDbModel> {
        sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::not_found("TemplateConfig", name))
    }

    async fn list_template_configs(&self) -> Result<Vec<TemplateConfigDbModel>> {
        let configs = sqlx::query_as::<_, TemplateConfigDbModel>(
            "SELECT * FROM template_config ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO template_config (
                id, name, output_folder, output_filename_template,
                cookies, output_file_format, min_segment_size_bytes,
                max_download_duration_secs, max_part_size_bytes, record_danmu,
                platform_overrides, download_retry_policy, danmu_sampling_config,
                download_engine, engines_override, proxy_config, event_hooks, stream_selection_config,
                created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)

            "#,
        )
        .bind(&config.id)
        .bind(&config.name)
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.cookies)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(config.record_danmu)
        .bind(&config.platform_overrides)
        .bind(&config.download_retry_policy)
        .bind(&config.danmu_sampling_config)
        .bind(&config.download_engine)
        .bind(&config.engines_override)
        .bind(&config.proxy_config)
        .bind(&config.event_hooks)
        .bind(&config.stream_selection_config)
        .bind(config.created_at)
        .bind(config.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE template_config SET
                name = ?,
                output_folder = ?,
                output_filename_template = ?,
                output_filename_template = ?,
                cookies = ?,
                output_file_format = ?,
                min_segment_size_bytes = ?,
                max_download_duration_secs = ?,
                max_part_size_bytes = ?,
                record_danmu = ?,
                platform_overrides = ?,
                download_retry_policy = ?,
                danmu_sampling_config = ?,
                download_engine = ?,
                engines_override = ?,
                proxy_config = ?,
                event_hooks = ?,
                stream_selection_config = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&config.name)
        .bind(&config.output_folder)
        .bind(&config.output_filename_template)
        .bind(&config.cookies)
        .bind(&config.output_file_format)
        .bind(config.min_segment_size_bytes)
        .bind(config.max_download_duration_secs)
        .bind(config.max_part_size_bytes)
        .bind(config.record_danmu)
        .bind(&config.platform_overrides)
        .bind(&config.download_retry_policy)
        .bind(&config.danmu_sampling_config)
        .bind(&config.download_engine)
        .bind(&config.engines_override)
        .bind(&config.proxy_config)
        .bind(&config.event_hooks)
        .bind(&config.stream_selection_config)
        .bind(Utc::now())
        .bind(&config.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_template_config(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM template_config WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_engine_config(&self, id: &str) -> Result<EngineConfigurationDbModel> {
        sqlx::query_as::<_, EngineConfigurationDbModel>(
            "SELECT * FROM engine_configuration WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::not_found("EngineConfiguration", id))
    }

    async fn list_engine_configs(&self) -> Result<Vec<EngineConfigurationDbModel>> {
        let configs = sqlx::query_as::<_, EngineConfigurationDbModel>(
            "SELECT * FROM engine_configuration ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(configs)
    }

    async fn create_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO engine_configuration (id, name, engine_type, config)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(&config.id)
        .bind(&config.name)
        .bind(&config.engine_type)
        .bind(&config.config)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE engine_configuration SET
                name = ?,
                engine_type = ?,
                config = ?
            WHERE id = ?
            "#,
        )
        .bind(&config.name)
        .bind(&config.engine_type)
        .bind(&config.config)
        .bind(&config.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete_engine_config(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM engine_configuration WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration tests would go here with a test database
}
