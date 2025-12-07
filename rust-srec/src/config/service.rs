//! Configuration service implementation.
//!
//! The ConfigService provides centralized access to all configuration data
//! with in-memory caching and event broadcasting for updates.

use std::sync::Arc;
use tokio::sync::broadcast;

use crate::Result;
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, TemplateConfigDbModel,
};
use crate::database::repositories::{config::ConfigRepository, streamer::StreamerRepository};
use crate::domain::config::MergedConfig;

use super::cache::ConfigCache;
use super::events::{ConfigEventBroadcaster, ConfigUpdateEvent};

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
        Self {
            config_repo,
            streamer_repo,
            cache: ConfigCache::new(),
            broadcaster: ConfigEventBroadcaster::new(),
        }
    }

    /// Create a new ConfigService with custom cache settings.
    pub fn with_cache(config_repo: Arc<C>, streamer_repo: Arc<S>, cache: ConfigCache) -> Self {
        Self {
            config_repo,
            streamer_repo,
            cache,
            broadcaster: ConfigEventBroadcaster::new(),
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

        self.broadcaster.publish(ConfigUpdateEvent::EngineUpdated {
            engine_id: config.id.clone(),
        });

        Ok(())
    }

    /// Update an engine configuration.
    pub async fn update_engine_config(&self, config: &EngineConfigurationDbModel) -> Result<()> {
        self.config_repo.update_engine_config(config).await?;

        // Engine updates may affect any streamer using this engine
        // For now, we don't track which streamers use which engines
        // so we just broadcast the event

        self.broadcaster.publish(ConfigUpdateEvent::EngineUpdated {
            engine_id: config.id.clone(),
        });

        tracing::info!("Engine config {} updated", config.id);
        Ok(())
    }

    // ========== Merged Config (Cached) ==========

    /// Get the merged configuration for a streamer.
    ///
    /// This method uses lazy loading with request deduplication:
    /// - Returns cached config if available
    /// - Deduplicates concurrent requests for the same streamer
    /// - Only one request will resolve the config while others wait
    pub async fn get_config_for_streamer(&self, streamer_id: &str) -> Result<MergedConfig> {
        // Check cache first
        if let Some(config) = self.cache.get(streamer_id) {
            tracing::trace!("Cache hit for streamer {}", streamer_id);
            return Ok(config);
        }

        // Check for in-flight request (deduplication)
        let (cell, is_new) = self.cache.get_or_create_in_flight(streamer_id);

        if !is_new {
            // Another request is already resolving this config, wait for it
            tracing::trace!("Waiting for in-flight request for streamer {}", streamer_id);

            // Wait for the OnceCell to be populated
            loop {
                if let Some(config) = cell.get() {
                    return Ok(config.clone());
                }
                // Also check cache in case it was populated
                if let Some(config) = self.cache.get(streamer_id) {
                    return Ok(config);
                }
                // Small yield to avoid busy-waiting
                tokio::task::yield_now().await;
            }
        }

        tracing::trace!("Cache miss for streamer {}, resolving config", streamer_id);

        // Resolve the config
        match self.resolve_config_for_streamer(streamer_id).await {
            Ok(config) => {
                // Complete the in-flight request (caches and notifies waiters)
                self.cache.complete_in_flight(streamer_id, config.clone());
                Ok(config)
            }
            Err(e) => {
                // Remove the in-flight entry on error
                self.cache.invalidate(streamer_id);
                Err(e)
            }
        }
    }

    /// Resolve the merged configuration for a streamer without caching.
    async fn resolve_config_for_streamer(&self, streamer_id: &str) -> Result<MergedConfig> {
        // Get the streamer
        let streamer = self.streamer_repo.get_streamer(streamer_id).await?;

        // Get global config
        let global = self.config_repo.get_global_config().await?;

        // Get platform config
        let platform = self
            .config_repo
            .get_platform_config(&streamer.platform_config_id)
            .await?;

        // Get template config if specified
        let template = if let Some(ref template_id) = streamer.template_config_id {
            Some(self.config_repo.get_template_config(template_id).await?)
        } else {
            None
        };

        // Build merged config
        let mut builder = MergedConfig::builder()
            .with_global(
                global.output_folder,
                global.output_filename_template,
                global.output_file_format,
                global.min_segment_size_bytes,
                global.max_download_duration_secs,
                global.max_part_size_bytes,
                global.record_danmu,
                parse_proxy_config(&global.proxy_config),
                global.default_download_engine,
            )
            .with_platform(
                platform.fetch_delay_ms,
                platform.download_delay_ms,
                platform.cookies.clone(),
                platform
                    .proxy_config
                    .as_ref()
                    .map(|s| parse_proxy_config(s)),
                platform.record_danmu,
                platform
                    .platform_specific_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .as_ref(), // Pass as Option<&Value>
                platform.output_folder.clone(),
                platform.output_filename_template.clone(),
                platform.download_engine.clone(),
                platform.max_bitrate,
                platform
                    .stream_selection_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                platform.output_file_format.clone(),
                platform.min_segment_size_bytes,
                platform.max_download_duration_secs,
                platform.max_part_size_bytes,
                platform
                    .download_retry_policy
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                platform
                    .event_hooks
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
            );

        // Apply template if present
        if let Some(template) = template {
            builder = builder.with_template(
                template.output_folder,
                template.output_filename_template,
                template.output_file_format,
                template.min_segment_size_bytes,
                template.max_download_duration_secs,
                template.max_part_size_bytes,
                template.record_danmu,
                template
                    .proxy_config
                    .as_ref()
                    .map(|s| parse_proxy_config(s)),
                template.cookies,
                template.download_engine,
                template
                    .download_retry_policy
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                template
                    .danmu_sampling_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                template.max_bitrate,
                template
                    .event_hooks
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                template
                    .stream_selection_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
            );
        }

        // Apply streamer-specific config
        let streamer_config = streamer
            .streamer_specific_config
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        builder = builder.with_streamer(
            streamer
                .download_retry_policy
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            streamer
                .danmu_sampling_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            streamer_config.as_ref(),
        );

        Ok(builder.build())
    }

    /// Invalidate the cached config for a specific streamer.
    pub fn invalidate_streamer(&self, streamer_id: &str) {
        self.cache.invalidate(streamer_id);

        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerUpdated {
                streamer_id: streamer_id.to_string(),
            });
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

/// Parse a JSON string into a ProxyConfig.
fn parse_proxy_config(json: &str) -> crate::domain::ProxyConfig {
    serde_json::from_str(json).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    // Tests will be added with mock repositories
}
