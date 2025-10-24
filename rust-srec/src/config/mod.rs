use crate::database::config_merger;
use crate::database::DatabaseService;
use crate::domain::{
    config::MergedConfig, global_config::GlobalConfig, platform_config::PlatformConfig,
    streamer::Streamer, template_config::TemplateConfig,
};
use anyhow::Result;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

#[derive(Debug, Clone)]
pub enum ConfigChangeEvent {
    GlobalConfigUpdated(Arc<GlobalConfig>),
    PlatformConfigUpdated(Arc<PlatformConfig>),
    TemplateConfigUpdated(Arc<TemplateConfig>),
    StreamerUpdated(Arc<Streamer>),
    StreamerDeleted(String),
}

#[derive(Clone)]
pub struct ConfigService {
    db_service: Arc<DatabaseService>,
    global_config: Arc<RwLock<Arc<GlobalConfig>>>,
    platform_configs: Arc<DashMap<String, Arc<PlatformConfig>>>,
    template_configs: Arc<DashMap<String, Arc<TemplateConfig>>>,
    streamers: Arc<DashMap<String, Arc<Streamer>>>,
    event_tx: broadcast::Sender<ConfigChangeEvent>,
}

impl ConfigService {
    pub async fn new(db_service: Arc<DatabaseService>) -> Result<Self> {
        let (event_tx, _) = broadcast::channel(100);
        let service = Self {
            db_service,
            global_config: Arc::new(RwLock::new(Arc::new(GlobalConfig::default()))),
            platform_configs: Arc::new(DashMap::new()),
            template_configs: Arc::new(DashMap::new()),
            streamers: Arc::new(DashMap::new()),
            event_tx,
        };
        service.load_all_configs().await?;
        Ok(service)
    }

    async fn load_all_configs(&self) -> Result<()> {
        // Load Global Config
        let global_config = self.db_service.global_configs().get().await?.unwrap_or_default();
        *self.global_config.write().await = Arc::new(global_config);

        // Load Platform Configs
        let platform_configs = self.db_service.platform_configs().find_all().await?;
        for config in platform_configs {
            self.platform_configs
                .insert(config.id.clone(), Arc::new(config.into()));
        }

        // Load Template Configs
        let template_configs = self.db_service.template_configs().find_all().await?;
        for config in template_configs {
            self.template_configs
                .insert(config.id.clone(), Arc::new(config.into()));
        }

        // Load Streamers
        let streamers = self.db_service.streamers().find_all().await?;
        for streamer in streamers {
            self.streamers.insert(streamer.id.clone(), Arc::new(streamer));
        }

        Ok(())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChangeEvent> {
        self.event_tx.subscribe()
    }

    pub async fn get_merged_config(&self, streamer_id: &str) -> Result<MergedConfig> {
        let streamer = self
            .streamers
            .get(streamer_id)
            .ok_or_else(|| anyhow::anyhow!("Streamer with id {} not found in cache", streamer_id))?
            .clone();

        let template_config = streamer
            .template_config_id
            .as_ref()
            .and_then(|id| self.template_configs.get(id.as_str()))
            .map(|r| r.value().clone());

        let platform_config = self
            .platform_configs
            .get(&streamer.platform_config_id)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Platform config with id {} not found for streamer {}",
                    streamer.platform_config_id,
                    streamer_id
                )
            })?
            .clone();

        let global_config = self.global_config.read().await.clone();

        let merged = config_merger::merge_configs(
            &global_config,
            &platform_config,
            template_config.as_deref(),
            Some(&serde_json::to_value(&streamer.config)?),
        );

        Ok(merged)
    }

    pub async fn get_global_config(&self) -> Arc<GlobalConfig> {
        self.global_config.read().await.clone()
    }

    pub fn get_platform_config(&self, id: &str) -> Option<Arc<PlatformConfig>> {
        self.platform_configs.get(id).map(|r| r.value().clone())
    }

    pub fn get_template_config(&self, id: &str) -> Option<Arc<TemplateConfig>> {
        self.template_configs.get(id).map(|r| r.value().clone())
    }

    pub fn get_streamer(&self, id: &str) -> Option<Arc<Streamer>> {
        self.streamers.get(id).map(|r| r.value().clone())
    }

    pub fn get_all_streamers(&self) -> Vec<Arc<Streamer>> {
        self.streamers.iter().map(|r| r.value().clone()).collect()
    }

    pub fn get_db_service(&self) -> Arc<DatabaseService> {
        self.db_service.clone()
    }

    pub async fn update_global_config(&self, config: GlobalConfig) -> Result<()> {
        self.db_service.global_configs().update(&config).await?;
        let config_arc = Arc::new(config);
        *self.global_config.write().await = config_arc.clone();
        self.event_tx
            .send(ConfigChangeEvent::GlobalConfigUpdated(config_arc))
            .ok();
        Ok(())
    }

    pub async fn update_platform_config(&self, config: PlatformConfig) -> Result<()> {
        self.db_service
            .platform_configs()
            .update(&config.clone().into())
            .await?;
        let config_arc = Arc::new(config);
        self.platform_configs
            .insert(config_arc.id.clone(), config_arc.clone());
        self.event_tx
            .send(ConfigChangeEvent::PlatformConfigUpdated(config_arc))
            .ok();
        Ok(())
    }

    pub async fn update_template_config(&self, config: TemplateConfig) -> Result<()> {
        self.db_service
            .template_configs()
            .update(&config)
            .await?;
        let config_arc = Arc::new(config);
        self.template_configs
            .insert(config_arc.id.clone(), config_arc.clone());
        self.event_tx
            .send(ConfigChangeEvent::TemplateConfigUpdated(config_arc))
            .ok();
        Ok(())
    }

    pub async fn update_streamer(&self, streamer: Streamer) -> Result<()> {
        self.db_service.streamers().update(&streamer).await?;
        let streamer_arc = Arc::new(streamer);
        self.streamers
            .insert(streamer_arc.id.clone(), streamer_arc.clone());
        self.event_tx
            .send(ConfigChangeEvent::StreamerUpdated(streamer_arc))
            .ok();
        Ok(())
    }

    pub async fn delete_streamer(&self, streamer_id: &str) -> Result<()> {
        self.db_service.streamers().delete(streamer_id).await?;
        self.streamers.remove(streamer_id);
        self.event_tx
            .send(ConfigChangeEvent::StreamerDeleted(streamer_id.to_string()))
            .ok();
        Ok(())
    }
}