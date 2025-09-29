use std::sync::Arc;

use crate::database::config_merger::merge_configs;
use crate::database::{
    models,
    repositories::{
        errors::{RepositoryError, RepositoryResult},
        filter_repository::FilterRepository,
        global_config_repository::GlobalConfigRepository,
        platform_config_repository::PlatformConfigRepository,
        template_config_repository::TemplateConfigRepository,
    },
};
use crate::domain::{
    global_config::GlobalConfig, platform_config::PlatformConfig, streamer::Streamer,
    types::StreamerUrl,
};
use async_trait::async_trait;
use sqlx::SqlitePool;

#[async_trait]
pub trait StreamerRepository: Send + Sync {
    async fn create(&self, streamer: &Streamer) -> RepositoryResult<()>;
    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<Streamer>>;
    async fn find_all(&self) -> RepositoryResult<Vec<Streamer>>;
    async fn update(&self, streamer: &Streamer) -> RepositoryResult<()>;
    async fn delete(&self, id: &str) -> RepositoryResult<()>;
}

pub struct SqliteStreamerRepository {
    db: SqlitePool,
    filter_repository: Arc<dyn FilterRepository>,
    platform_config_repository: Arc<dyn PlatformConfigRepository>,
    global_config_repository: Arc<dyn GlobalConfigRepository>,
    template_config_repository: Arc<dyn TemplateConfigRepository>,
}

impl SqliteStreamerRepository {
    pub fn new(
        db: SqlitePool,
        filter_repository: Arc<dyn FilterRepository>,
        platform_config_repository: Arc<dyn PlatformConfigRepository>,
        global_config_repository: Arc<dyn GlobalConfigRepository>,
        template_config_repository: Arc<dyn TemplateConfigRepository>,
    ) -> Self {
        Self {
            db,
            filter_repository,
            platform_config_repository,
            global_config_repository,
            template_config_repository,
        }
    }

    async fn map_to_domain(&self, model: models::Streamer) -> RepositoryResult<Streamer> {
        let filters = self
            .filter_repository
            .find_by_streamer_id(&model.id)
            .await?;

        let global_config: GlobalConfig = self
            .global_config_repository
            .get()
            .await?
            .ok_or_else(|| RepositoryError::NotFound)?
            .into();

        let platform_config: PlatformConfig = self
            .platform_config_repository
            .find_by_id(&model.platform_config_id)
            .await?
            .ok_or_else(|| {
                RepositoryError::Validation(format!(
                    "Platform config not found for streamer {}",
                    model.id
                ))
            })?
            .into();

        let template_config = if let Some(template_id) = &model.template_config_id {
            self.template_config_repository
                .find_by_id(template_id)
                .await?
                .map(Into::into)
        } else {
            None
        };

        let streamer_specific_config: Option<serde_json::Value> = model
            .streamer_specific_config
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        let config = merge_configs(
            &global_config,
            &platform_config,
            template_config.as_ref(),
            streamer_specific_config.as_ref(),
        );

        Ok(Streamer {
            id: model.id.clone(),
            name: model.name,
            url: StreamerUrl(model.url),
            state: model.state.parse().map_err(|e| {
                RepositoryError::Validation(format!("Failed to parse state: {}", e))
            })?,
            consecutive_error_count: model.consecutive_error_count.unwrap_or(0) as u32,
            disabled_until: model
                .disabled_until
                .map(|s| {
                    s.parse().map_err(|e| {
                        RepositoryError::Validation(format!(
                            "Failed to parse disabled_until: {}",
                            e
                        ))
                    })
                })
                .transpose()?,
            config,
            filters,
            live_sessions: vec![],
            platform_config_id: model.platform_config_id,
            template_config_id: model.template_config_id,
        })
    }

    fn map_to_database(&self, domain: &Streamer) -> models::Streamer {
        let streamer_specific_config = serde_json::to_string(&domain.config).ok();

        models::Streamer {
            id: domain.id.clone(),
            name: domain.name.clone(),
            url: domain.url.0.clone(),
            platform_config_id: domain.platform_config_id.clone(),
            template_config_id: domain.template_config_id.clone(),
            state: domain.state.to_string(),
            last_live_time: None,
            streamer_specific_config,
            download_retry_policy: None,
            danmu_sampling_config: None,
            consecutive_error_count: Some(domain.consecutive_error_count as i64),
            disabled_until: domain.disabled_until.map(|dt| dt.to_rfc3339()),
        }
    }
}

#[async_trait]
impl StreamerRepository for SqliteStreamerRepository {
    async fn create(&self, streamer: &Streamer) -> RepositoryResult<()> {
        let model = self.map_to_database(streamer);
        sqlx::query!(
            r#"
            INSERT INTO streamers (id, name, url, platform_config_id, template_config_id, state, consecutive_error_count, disabled_until)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
            model.id,
            model.name,
            model.url,
            model.platform_config_id,
            model.template_config_id,
            model.state,
            model.consecutive_error_count,
            model.disabled_until
        )
        .execute(&self.db)
        .await
        .map_err(RepositoryError::from)?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> RepositoryResult<Option<Streamer>> {
        let model = sqlx::query_as!(
            models::Streamer,
            r#"
            SELECT id as "id!", name as "name!", url as "url!", platform_config_id as "platform_config_id!", template_config_id, state as "state!", last_live_time, streamer_specific_config, download_retry_policy, danmu_sampling_config, consecutive_error_count, disabled_until
            FROM streamers
            WHERE id = ?
            "#,
            id
        )
        .fetch_optional(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        match model {
            Some(model) => Ok(Some(self.map_to_domain(model).await?)),
            None => Ok(None),
        }
    }

    async fn find_all(&self) -> RepositoryResult<Vec<Streamer>> {
        let models = sqlx::query_as!(
            models::Streamer,
            r#"
            SELECT 
                id as "id!", 
                name as "name!", 
                url as "url!", 
                platform_config_id as "platform_config_id!", 
                template_config_id, 
                state as "state!", 
                last_live_time, 
                streamer_specific_config, 
                download_retry_policy, 
                danmu_sampling_config, 
                consecutive_error_count, 
                disabled_until
            FROM streamers
            "#
        )
        .fetch_all(&self.db)
        .await
        .map_err(RepositoryError::from)?;

        let mut streamers = Vec::new();
        for model in models {
            streamers.push(self.map_to_domain(model).await?);
        }
        Ok(streamers)
    }

    async fn update(&self, streamer: &Streamer) -> RepositoryResult<()> {
        let model = self.map_to_database(streamer);
        sqlx::query!(
            r#"
            UPDATE streamers
            SET name = ?, url = ?, platform_config_id = ?, template_config_id = ?, state = ?, consecutive_error_count = ?, disabled_until = ?
            WHERE id = ?
            "#,
            model.name,
            model.url,
            model.platform_config_id,
            model.template_config_id,
            model.state,
            model.consecutive_error_count,
            model.disabled_until,
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
            DELETE FROM streamers
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
