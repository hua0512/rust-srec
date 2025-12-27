//! Configuration service implementation.
//!
//! The ConfigService provides centralized access to all configuration data
//! with in-memory caching and event broadcasting for updates.
//!
//! Architecture:
//! - MergedConfigBuilder: Handles the merging logic for 4-layer config hierarchy
//! - ConfigResolver: Uses builder to resolve config, fetches from repositories
//! - ConfigService: Uses resolver + adds caching and event broadcasting

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::trace;

use crate::Result;
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, TemplateConfigDbModel,
};
use crate::database::repositories::{config::ConfigRepository, streamer::StreamerRepository};
use crate::domain::config::{ConfigResolver, MergedConfig};
use crate::domain::streamer::Streamer;
use crate::utils::json::{self, JsonContext};

use super::cache::ConfigCache;
use super::events::{ConfigEventBroadcaster, ConfigUpdateEvent};

/// Hard upper bound for a single streamer config resolution. This prevents `in_flight` entries
/// from getting stuck forever if an upstream call hangs.
const CONFIG_RESOLVE_HARD_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration service providing cached access to all configuration data.
///
/// The service maintains an in-memory cache of merged configurations and
/// broadcasts events when configurations change.
pub struct ConfigService<C, S>
where
    C: ConfigRepository + Send + Sync,
    S: StreamerRepository + Send + Sync,
{
    config_repo: Arc<C>,
    streamer_repo: Arc<S>,
    cache: ConfigCache,
    broadcaster: ConfigEventBroadcaster,
}

impl<C, S> ConfigService<C, S>
where
    C: ConfigRepository + Send + Sync,
    S: StreamerRepository + Send + Sync,
{
    /// Create a new ConfigService.
    pub fn new(config_repo: Arc<C>, streamer_repo: Arc<S>) -> Self {
        Self::with_cache_and_broadcaster(
            config_repo,
            streamer_repo,
            ConfigCache::new(),
            ConfigEventBroadcaster::new(),
        )
    }

    /// Create a new ConfigService with custom cache settings.
    pub fn with_cache(config_repo: Arc<C>, streamer_repo: Arc<S>, cache: ConfigCache) -> Self {
        Self::with_cache_and_broadcaster(
            config_repo,
            streamer_repo,
            cache,
            ConfigEventBroadcaster::new(),
        )
    }

    /// Create a new ConfigService with custom cache settings and a shared broadcaster.
    ///
    /// This should be used by the runtime `ServiceContainer` so the scheduler and other
    /// services see config update events from the API (global/platform/template/engine).
    pub fn with_cache_and_broadcaster(
        config_repo: Arc<C>,
        streamer_repo: Arc<S>,
        cache: ConfigCache,
        broadcaster: ConfigEventBroadcaster,
    ) -> Self {
        Self {
            config_repo,
            streamer_repo,
            cache,
            broadcaster,
        }
    }

    // ========== Event Broadcasting ==========

    /// Subscribe to configuration update events.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigUpdateEvent> {
        self.broadcaster.subscribe()
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.broadcaster.subscriber_count()
    }

    // ========== Global Config ==========

    /// Get the global configuration.
    pub async fn get_global_config(&self) -> Result<GlobalConfigDbModel> {
        self.config_repo.get_global_config().await
    }

    /// Update the global configuration.
    pub async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        self.config_repo.update_global_config(config).await?;

        // Invalidate all cached configs since global affects everything
        self.cache.invalidate_all();

        // Broadcast update event
        self.broadcaster.publish(ConfigUpdateEvent::GlobalUpdated);

        tracing::info!("Global config updated, cache invalidated");
        Ok(())
    }

    // ========== Platform Config ==========

    /// Get a platform configuration by ID.
    pub async fn get_platform_config(&self, id: &str) -> Result<PlatformConfigDbModel> {
        self.config_repo.get_platform_config(id).await
    }

    /// Get a platform configuration by name.
    pub async fn get_platform_config_by_name(&self, name: &str) -> Result<PlatformConfigDbModel> {
        self.config_repo.get_platform_config_by_name(name).await
    }

    /// List all platform configurations.
    pub async fn list_platform_configs(&self) -> Result<Vec<PlatformConfigDbModel>> {
        self.config_repo.list_platform_configs().await
    }

    /// Create a new platform configuration.
    pub async fn create_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        self.config_repo.create_platform_config(config).await?;

        self.broadcaster
            .publish(ConfigUpdateEvent::PlatformUpdated {
                platform_id: config.id.clone(),
            });

        Ok(())
    }

    /// Update a platform configuration.
    pub async fn update_platform_config(&self, config: &PlatformConfigDbModel) -> Result<()> {
        self.config_repo.update_platform_config(config).await?;

        // Invalidate configs for streamers on this platform
        self.invalidate_streamers_by_platform(&config.id).await?;

        self.broadcaster
            .publish(ConfigUpdateEvent::PlatformUpdated {
                platform_id: config.id.clone(),
            });

        tracing::info!("Platform config {} updated", config.id);
        Ok(())
    }

    // ========== Template Config ==========

    /// Get a template configuration by ID.
    pub async fn get_template_config(&self, id: &str) -> Result<TemplateConfigDbModel> {
        self.config_repo.get_template_config(id).await
    }

    /// Get a template configuration by name.
    pub async fn get_template_config_by_name(&self, name: &str) -> Result<TemplateConfigDbModel> {
        self.config_repo.get_template_config_by_name(name).await
    }

    /// List all template configurations.
    pub async fn list_template_configs(&self) -> Result<Vec<TemplateConfigDbModel>> {
        self.config_repo.list_template_configs().await
    }

    /// Create a new template configuration.
    pub async fn create_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        self.config_repo.create_template_config(config).await?;

        self.broadcaster
            .publish(ConfigUpdateEvent::TemplateUpdated {
                template_id: config.id.clone(),
            });

        Ok(())
    }

    /// Update a template configuration.
    pub async fn update_template_config(&self, config: &TemplateConfigDbModel) -> Result<()> {
        self.config_repo.update_template_config(config).await?;

        // Invalidate configs for streamers using this template
        self.invalidate_streamers_by_template(&config.id).await?;

        self.broadcaster
            .publish(ConfigUpdateEvent::TemplateUpdated {
                template_id: config.id.clone(),
            });

        tracing::info!("Template config {} updated", config.id);
        Ok(())
    }

    /// Delete a template configuration.
    pub async fn delete_template_config(&self, id: &str) -> Result<()> {
        // Invalidate before delete
        self.invalidate_streamers_by_template(id).await?;

        self.config_repo.delete_template_config(id).await?;

        self.broadcaster
            .publish(ConfigUpdateEvent::TemplateUpdated {
                template_id: id.to_string(),
            });

        Ok(())
    }

    // ========== Engine Config ==========

    /// Get an engine configuration by ID.
    pub async fn get_engine_config(&self, id: &str) -> Result<EngineConfigurationDbModel> {
        self.config_repo.get_engine_config(id).await
    }

    /// List all engine configurations.
    pub async fn list_engine_configs(&self) -> Result<Vec<EngineConfigurationDbModel>> {
        self.config_repo.list_engine_configs().await
    }

    /// Create a new engine configuration.
    pub async fn create_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        self.config_repo.create_engine_config(config).await?;

        // We don't track which streamers use which engine configs; invalidate all for correctness.
        self.cache.invalidate_all();

        self.broadcaster.publish(ConfigUpdateEvent::EngineUpdated {
            engine_id: config.id.clone(),
        });

        Ok(())
    }

    /// Update an engine configuration.
    pub async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        self.config_repo.update_engine_config(config).await?;

        // Engine updates may affect any streamer using this engine; since we don't track
        // usage, invalidate all cached merged configs for correctness.
        self.cache.invalidate_all();

        self.broadcaster.publish(ConfigUpdateEvent::EngineUpdated {
            engine_id: config.id.clone(),
        });

        tracing::info!("Engine config {} updated", config.id);
        Ok(())
    }

    /// Delete an engine configuration.
    pub async fn delete_engine_config(&self, id: &str) -> Result<()> {
        self.config_repo.delete_engine_config(id).await?;

        // Deleting an engine can affect any streamer that referenced it; invalidate all.
        self.cache.invalidate_all();

        self.broadcaster.publish(ConfigUpdateEvent::EngineUpdated {
            engine_id: id.to_string(),
        });

        tracing::info!("Engine config {} deleted", id);
        Ok(())
    }

    // ========== Merged Config (Cached) ==========

    /// Get the merged configuration for a streamer.
    ///
    /// This method uses lazy loading with request deduplication:
    /// - Returns cached config if available
    /// - Deduplicates concurrent requests for the same streamer
    /// - Only one request will resolve the config while others wait
    pub async fn get_config_for_streamer(&self, streamer_id: &str) -> Result<Arc<MergedConfig>> {
        // One retry is enough to handle "cache invalidated" races without creating
        // unbounded loops if the config is being updated repeatedly.
        for attempt in 0..2 {
            // Check cache first
            if let Some(config) = self.cache.get(streamer_id) {
                trace!("Cache hit for streamer {}", streamer_id);
                return Ok(config);
            }

            // Check for in-flight request (deduplication)
            let (cell, is_new) = self.cache.get_or_create_in_flight(streamer_id);

            if !is_new {
                // Another request is already resolving this config, wait for it
                trace!("Waiting for in-flight request for streamer {}", streamer_id);
                match self.cache.wait_for_in_flight(&cell).await {
                    Ok(config) => return Ok(config),
                    Err(message)
                        if attempt == 0
                            && (message.contains("Configuration invalidated")
                                || message.contains("Configuration cache invalidated")) =>
                    {
                        trace!(
                            "In-flight config was invalidated for streamer {}, retrying",
                            streamer_id
                        );
                        continue;
                    }
                    Err(message) => return Err(crate::Error::Configuration(message)),
                }
            }

            trace!("Cache miss for streamer {}, resolving config", streamer_id);

            // Resolve the config
            let resolve = tokio::time::timeout(
                CONFIG_RESOLVE_HARD_TIMEOUT,
                self.resolve_config_for_streamer(streamer_id),
            )
            .await;

            return match resolve {
                Ok(Ok(config)) => {
                    // Complete the in-flight request (caches and notifies waiters)
                    let config = Arc::new(config);
                    self.cache
                        .complete_in_flight(streamer_id, &cell, config.clone());
                    Ok(config)
                }
                Ok(Err(e)) => {
                    self.cache.fail_in_flight(
                        streamer_id,
                        &cell,
                        format!("Failed to resolve config for streamer {streamer_id}: {e}"),
                    );
                    Err(e)
                }
                Err(_) => {
                    let message = format!(
                        "Timed out resolving config for streamer {streamer_id} after {:?}",
                        CONFIG_RESOLVE_HARD_TIMEOUT
                    );
                    self.cache
                        .fail_in_flight(streamer_id, &cell, message.clone());
                    Err(crate::Error::Configuration(message))
                }
            };
        }

        Err(crate::Error::Configuration(format!(
            "Failed to resolve config for streamer {streamer_id} after retry"
        )))
    }

    /// Resolve the merged configuration for a streamer without caching.
    ///
    /// Delegates to ConfigResolver which handles the 4-layer merging logic.
    async fn resolve_config_for_streamer(&self, streamer_id: &str) -> Result<MergedConfig> {
        // Get the streamer and convert to domain entity
        let streamer_db = self.streamer_repo.get_streamer(streamer_id).await?;
        let streamer = self.convert_to_domain_streamer(&streamer_db)?;

        // Use ConfigResolver to handle the merging logic
        let resolver = ConfigResolver::new(Arc::clone(&self.config_repo));
        resolver.resolve_config_for_streamer(&streamer).await
    }

    /// Convert database streamer model to domain entity.
    fn convert_to_domain_streamer(
        &self,
        db_model: &crate::database::models::StreamerDbModel,
    ) -> Result<Streamer> {
        use crate::domain::StreamerUrl;

        let url = StreamerUrl::new(&db_model.url)?;

        let mut streamer = Streamer::new(&db_model.name, url, &db_model.platform_config_id);
        streamer.id = db_model.id.clone();
        streamer.template_config_id = db_model.template_config_id.clone();

        streamer.streamer_specific_config = json::parse_optional(
            db_model.streamer_specific_config.as_deref(),
            JsonContext::StreamerField {
                streamer_id: &db_model.id,
                field: "streamer_specific_config",
            },
            "Invalid streamer_specific_config JSON; ignoring",
        );

        Ok(streamer)
    }

    /// Invalidate the cached config for a specific streamer.
    pub fn invalidate_streamer(&self, streamer_id: &str) {
        self.cache.invalidate(streamer_id);
    }

    // ========== Cache Management ==========

    /// Get cache statistics.
    pub fn cache_stats(&self) -> super::cache::CacheStats {
        self.cache.stats()
    }

    /// Cleanup expired cache entries.
    pub fn cleanup_cache(&self) -> usize {
        self.cache.cleanup_expired()
    }

    // ========== Private Helpers ==========

    /// Invalidate cached configs for all streamers on a platform.
    async fn invalidate_streamers_by_platform(&self, platform_id: &str) -> Result<()> {
        let streamers = self
            .streamer_repo
            .list_streamers_by_platform(platform_id)
            .await?;

        for streamer in streamers {
            self.cache.invalidate(&streamer.id);
        }

        tracing::debug!(
            "Invalidated cache for streamers on platform {}",
            platform_id
        );

        Ok(())
    }

    /// Invalidate cached configs for all streamers using a template.
    async fn invalidate_streamers_by_template(&self, template_id: &str) -> Result<()> {
        let streamers = self
            .streamer_repo
            .list_streamers_by_template(template_id)
            .await?;

        for streamer in streamers {
            self.cache.invalidate(&streamer.id);
        }

        tracing::debug!(
            "Invalidated cache for streamers using template {}",
            template_id
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Tests will be added with mock repositories
}
