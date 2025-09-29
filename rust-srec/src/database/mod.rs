use anyhow::Result;
use sqlx::SqlitePool;
use std::sync::Arc;

pub mod config_merger;
pub mod converters;
pub mod db;
pub mod models;
pub mod repositories;

use self::{
    db::create_pool,
    repositories::{
        ApiKeyRepository, EngineConfigRepository, FilterRepository, GlobalConfigRepository,
        JobRepository, LiveSessionRepository, MediaOutputRepository, NotificationChannelRepository,
        NotificationSubscriptionRepository, PlatformConfigRepository, SqliteApiKeyRepository,
        SqliteEngineConfigRepository, SqliteFilterRepository, SqliteGlobalConfigRepository,
        SqliteJobRepository, SqliteLiveSessionRepository, SqliteMediaOutputRepository,
        SqliteNotificationChannelRepository, SqliteNotificationSubscriptionRepository,
        SqlitePlatformConfigRepository, SqliteStreamerRepository, SqliteTemplateConfigRepository,
        SqliteUploadRecordRepository, StreamerRepository, TemplateConfigRepository,
        UploadRecordRepository,
    },
};

#[derive(Clone)]
pub struct DatabaseService {
    pub pool: SqlitePool,
}

impl DatabaseService {
    pub async fn new(database_url: &str) -> Result<Self> {
        let pool = create_pool(database_url).await?;
        Ok(Self { pool: pool })
    }

    pub fn api_keys(&self) -> Arc<dyn ApiKeyRepository> {
        Arc::new(SqliteApiKeyRepository::new(self.pool.clone()))
    }

    pub fn engine_configs(&self) -> Arc<dyn EngineConfigRepository> {
        Arc::new(SqliteEngineConfigRepository::new(self.pool.clone()))
    }

    pub fn filters(&self) -> Arc<dyn FilterRepository> {
        Arc::new(SqliteFilterRepository::new(self.pool.clone()))
    }

    pub fn global_configs(&self) -> Arc<dyn GlobalConfigRepository> {
        Arc::new(SqliteGlobalConfigRepository::new(self.pool.clone()))
    }

    pub fn jobs(&self) -> Arc<dyn JobRepository> {
        Arc::new(SqliteJobRepository::new(self.pool.clone()))
    }

    pub fn live_sessions(&self) -> Arc<dyn LiveSessionRepository> {
        Arc::new(SqliteLiveSessionRepository::new(
            self.pool.clone(),
            self.media_outputs(),
        ))
    }

    pub fn media_outputs(&self) -> Arc<dyn MediaOutputRepository> {
        Arc::new(SqliteMediaOutputRepository::new(self.pool.clone()))
    }

    pub fn notification_channels(&self) -> Arc<dyn NotificationChannelRepository> {
        Arc::new(SqliteNotificationChannelRepository::new(self.pool.clone()))
    }

    pub fn notification_subscriptions(&self) -> Arc<dyn NotificationSubscriptionRepository> {
        Arc::new(SqliteNotificationSubscriptionRepository::new(
            self.pool.clone(),
        ))
    }

    pub fn platform_configs(&self) -> Arc<dyn PlatformConfigRepository> {
        Arc::new(SqlitePlatformConfigRepository::new(self.pool.clone()))
    }

    pub fn streamers(&self) -> Arc<dyn StreamerRepository> {
        Arc::new(SqliteStreamerRepository::new(
            self.pool.clone(),
            self.filters(),
            self.platform_configs(),
            self.global_configs(),
            self.template_configs(),
        ))
    }

    pub fn template_configs(&self) -> Arc<dyn TemplateConfigRepository> {
        Arc::new(SqliteTemplateConfigRepository::new(self.pool.clone()))
    }

    pub fn upload_records(&self) -> Arc<dyn UploadRecordRepository> {
        Arc::new(SqliteUploadRecordRepository::new(self.pool.clone()))
    }
}
