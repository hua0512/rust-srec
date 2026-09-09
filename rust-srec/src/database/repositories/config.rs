//! Configuration repository.

mod writes;
pub(crate) use writes::{
    delete_engine, import_engine, import_global, import_platform, import_template,
};

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, RetentionDays,
    TemplateConfigDbModel,
};
use crate::{Error, Result};

fn validate_global_retention(config: &GlobalConfigDbModel) -> Result<()> {
    for (field, days) in [
        (
            "job_history_retention_days",
            config.job_history_retention_days,
        ),
        (
            "notification_event_log_retention_days",
            config.notification_event_log_retention_days,
        ),
    ] {
        RetentionDays::try_from(days)
            .map_err(|error| Error::config(format!("{field}: {error}")))?;
    }
    Ok(())
}

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
    write_pool: SqlitePool,
}

impl SqlxConfigRepository {
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
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
        validate_global_retention(config)?;
        writes::write_global(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn create_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        validate_global_retention(config)?;
        writes::write_global(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
        )
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
        writes::write_platform(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
        )
        .await?;
        Ok(())
    }

    async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        writes::write_platform(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn delete_platform_config(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM platform_config WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
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
        writes::write_template(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
            config.updated_at.timestamp_millis(),
        )
        .await?;
        Ok(())
    }

    async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        let updated_at = crate::database::time::now_ms();
        writes::write_template(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
            updated_at,
        )
        .await?;
        Ok(())
    }

    async fn delete_template_config(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM template_config WHERE id = ?")
            .bind(id)
            .execute(&self.write_pool)
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
        writes::write_engine(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Insert,
        )
        .await?;
        Ok(())
    }

    async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        writes::write_engine(
            &mut *self.write_pool.acquire().await?,
            config,
            super::row_write::WriteMode::Update,
        )
        .await?;
        Ok(())
    }

    async fn delete_engine_config(&self, id: &str) -> Result<()> {
        writes::delete_engine(&mut *self.write_pool.acquire().await?, id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_retention_validation_rejects_negative_values() {
        let config = GlobalConfigDbModel {
            job_history_retention_days: -1,
            ..GlobalConfigDbModel::default()
        };

        let error = validate_global_retention(&config).expect_err("negative retention");
        assert!(error.to_string().contains("job_history_retention_days"));
    }
}
