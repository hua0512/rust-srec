//! Configuration service implementation.
//!
//! The ConfigService provides centralized access to all configuration data
//! with in-memory caching and event broadcasting for updates.
//!
//! Architecture:
//! - MergedConfigBuilder: Handles the merging logic for 4-layer config hierarchy
//! - ConfigResolver: Uses builder to resolve config, fetches from repositories
//! - ConfigService: Uses resolver + adds caching and event broadcasting

use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::trace;

use crate::Result;
use crate::database::models::{
    EngineConfigurationDbModel, GlobalConfigDbModel, PlatformConfigDbModel, TemplateConfigDbModel,
};
use crate::database::repositories::{config::ConfigRepository, streamer::StreamerRepository};
use crate::domain::streamer::Streamer;
use crate::utils::json::{self, JsonContext};

use super::cache::{ConfigCache, InFlightRequest};
use super::events::{ConfigEventBroadcaster, ConfigUpdateEvent};
use super::{ConfigResolver, MergedConfig, ResolvedStreamerContext};

mod global_cache;
use global_cache::GlobalConfigCache;

/// Hard upper bound for a single streamer config resolution. This prevents `in_flight` entries
/// from getting stuck forever if an upstream call hangs.
const CONFIG_RESOLVE_HARD_TIMEOUT: Duration = Duration::from_secs(30);

/// Fails the in-flight `ConfigCache` entry for `streamer_id` on drop unless it was disarmed.
///
/// The owner that created a new in-flight entry via `ConfigCache::get_or_create_in_flight` must
/// eventually call `complete_in_flight` or `fail_in_flight`. If its task is cancelled mid-resolve
/// (e.g. an axum client disconnect during `resolve_context_for_streamer`), this guard's drop runs
/// `fail_in_flight` so the entry is removed and waiters wake; otherwise every later
/// `get_context_for_streamer` for that streamer would block on a request that never completes.
struct InFlightGuard<'a> {
    cache: &'a ConfigCache,
    streamer_id: &'a str,
    cell: InFlightRequest,
    armed: bool,
}

impl InFlightGuard<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.cache.fail_in_flight(
                self.streamer_id,
                &self.cell,
                format!(
                    "Config resolution for streamer {} was cancelled",
                    self.streamer_id
                ),
            );
        }
    }
}

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
    global_cache: GlobalConfigCache,
    filter_store: OnceLock<Arc<crate::database::filter_store::FilterStore>>,
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
            global_cache: GlobalConfigCache::default(),
            filter_store: OnceLock::new(),
        }
    }

    pub(crate) fn with_filter_store(
        self,
        store: Arc<crate::database::filter_store::FilterStore>,
    ) -> Self {
        self.filter_store.get_or_init(|| store);
        self
    }

    pub(crate) fn filter_store_for(
        &self,
        repository: Arc<dyn crate::database::repositories::FilterRepository>,
    ) -> Arc<crate::database::filter_store::FilterStore> {
        self.filter_store
            .get_or_init(|| Arc::new(crate::database::filter_store::FilterStore::new(repository)))
            .clone()
    }

    pub(crate) fn invalidate_filter_snapshots(&self, streamer_id: &str) {
        if let Some(store) = self.filter_store.get() {
            store.invalidate(streamer_id);
        }
    }

    pub(crate) fn invalidate_all_filter_snapshots(&self) {
        if let Some(store) = self.filter_store.get() {
            store.invalidate_all();
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

    /// Short-lived snapshot for high-frequency readers. Administrative read-modify-write
    /// callers keep using get_global_config, which always reads the authoritative row.
    pub async fn get_cached_global_config(&self) -> Result<Arc<GlobalConfigDbModel>> {
        self.global_cache
            .get_or_load(|| self.config_repo.get_global_config())
            .await
    }

    /// Update the global configuration.
    pub async fn update_global_config(&self, config: &GlobalConfigDbModel) -> Result<()> {
        self.config_repo.update_global_config(config).await?;
        self.global_cache.invalidate();
        self.invalidate_all_filter_snapshots();

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
        Ok(self
            .get_context_for_streamer(streamer_id)
            .await?
            .config
            .clone())
    }

    /// Read current configuration layers without waiting for another subscriber's
    /// invalidation, or cancelling an unrelated cached resolution in flight.
    pub(crate) async fn get_fresh_config_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Result<Arc<MergedConfig>> {
        tokio::time::timeout(
            CONFIG_RESOLVE_HARD_TIMEOUT,
            self.resolve_context_for_streamer(streamer_id),
        )
        .await
        .map_err(|_| crate::Error::config("Fresh streamer configuration resolution timed out"))?
        .map(|context| context.config)
    }

    /// Get the resolved streamer context for a streamer.
    ///
    /// This includes the merged config plus runtime-only derived values like `CredentialSource`.
    pub async fn get_context_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Result<Arc<ResolvedStreamerContext>> {
        // One retry is enough to handle "cache invalidated" races and a
        // cancelled owner (InFlightGuard's drop runs fail_in_flight) without
        // creating unbounded loops if the config is being updated repeatedly.
        for attempt in 0..2 {
            // Check cache first
            if let Some(context) = self.cache.get(streamer_id) {
                trace!("Cache hit for streamer {}", streamer_id);
                return Ok(context);
            }

            // Check for in-flight request (deduplication)
            let (cell, is_new) = self.cache.get_or_create_in_flight(streamer_id);

            if !is_new {
                // Another request is already resolving this config, wait for it
                trace!("Waiting for in-flight request for streamer {}", streamer_id);
                match self.cache.wait_for_in_flight(&cell).await {
                    Ok(context) => return Ok(context),
                    Err(message)
                        if attempt == 0
                            && (message.contains("Configuration invalidated")
                                || message.contains("Configuration cache invalidated")
                                || message.contains("was cancelled")) =>
                    {
                        trace!(
                            "In-flight config was invalidated or its owner was cancelled \
                             for streamer {}, retrying",
                            streamer_id
                        );
                        continue;
                    }
                    Err(message) => return Err(crate::Error::Configuration(message)),
                }
            }

            trace!("Cache miss for streamer {}, resolving config", streamer_id);

            // If this task is cancelled during resolution, the guard's drop runs
            // `fail_in_flight` so `cell` is not left in `ConfigCache::in_flight` forever.
            let mut guard = InFlightGuard {
                cache: &self.cache,
                streamer_id,
                cell: cell.clone(),
                armed: true,
            };

            // Resolve the config
            let resolve = tokio::time::timeout(
                CONFIG_RESOLVE_HARD_TIMEOUT,
                self.resolve_context_for_streamer(streamer_id),
            )
            .await;

            // Resolution completed without cancellation; the match arms below own the
            // complete/fail transition, so the guard's drop-time fallback is not needed.
            guard.disarm();

            return match resolve {
                Ok(Ok(context)) => {
                    // Complete the in-flight request (caches and notifies waiters)
                    let context = Arc::new(context);
                    self.cache
                        .complete_in_flight(streamer_id, &cell, context.clone());
                    Ok(context)
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

    /// Resolve the streamer context for a streamer without caching.
    ///
    /// Delegates to ConfigResolver which handles the 4-layer merging logic.
    async fn resolve_context_for_streamer(
        &self,
        streamer_id: &str,
    ) -> Result<ResolvedStreamerContext> {
        // Get the streamer and convert to domain entity
        let streamer_db = self.streamer_repo.get_streamer(streamer_id).await?;
        let streamer = self.convert_to_domain_streamer(&streamer_db)?;

        // Use ConfigResolver to handle the merging logic
        let resolver = ConfigResolver::new(Arc::clone(&self.config_repo));
        resolver.resolve_context_for_streamer(&streamer).await
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

    pub fn notify_streamer_filters_updated(&self, streamer_id: &str) {
        // Filters are stored separately from merged config, but changes should still invalidate
        // streamer config and trigger a scheduler re-check (OutOfSchedule smart-wake).
        self.invalidate_streamer(streamer_id);
        self.invalidate_filter_snapshots(streamer_id);
        self.broadcaster
            .publish(ConfigUpdateEvent::StreamerFiltersUpdated {
                streamer_id: streamer_id.to_string(),
            });
    }

    pub(crate) fn notify_import_committed(&self) {
        self.global_cache.invalidate();
        self.cache.invalidate_all();
        self.invalidate_all_filter_snapshots();
        self.broadcaster.publish(ConfigUpdateEvent::GlobalUpdated);
    }

    /// Invalidate cached configs for all streamers on a platform.
    pub async fn invalidate_platform(&self, platform_id: &str) -> Result<()> {
        self.invalidate_streamers_by_platform(platform_id).await
    }

    /// Invalidate cached configs for all streamers using a template.
    pub async fn invalidate_template(&self, template_id: &str) -> Result<()> {
        self.invalidate_streamers_by_template(template_id).await
    }

    // ========== Cache Management ==========

    /// Get cache statistics.
    pub fn cache_stats(&self) -> super::cache::CacheStats {
        self.cache.stats()
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
    use super::*;
    use crate::database::repositories::{SqlxConfigRepository, SqlxStreamerRepository};

    #[tokio::test]
    async fn global_hot_cache_invalidates_on_writes_and_import_but_admin_reads_are_authoritative() {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let repo = Arc::new(SqlxConfigRepository::new(pool.clone(), pool.clone()));
        let service = ConfigService::new(
            repo.clone(),
            Arc::new(SqlxStreamerRepository::new(pool.clone(), pool.clone())),
        );
        let mut config = service.get_global_config().await.unwrap();
        config.stream_proxy_allow_private_targets = true;
        service.update_global_config(&config).await.unwrap();
        assert!(
            service
                .get_cached_global_config()
                .await
                .unwrap()
                .stream_proxy_allow_private_targets
        );
        config.stream_proxy_allow_private_targets = false;
        service.update_global_config(&config).await.unwrap();
        assert!(
            !service
                .get_cached_global_config()
                .await
                .unwrap()
                .stream_proxy_allow_private_targets
        );

        config.output_folder = "/imported".to_string();
        repo.update_global_config(&config).await.unwrap();
        assert_ne!(
            service
                .get_cached_global_config()
                .await
                .unwrap()
                .output_folder,
            "/imported"
        );
        assert_eq!(
            service.get_global_config().await.unwrap().output_folder,
            "/imported"
        );
        service.notify_import_committed();
        assert_eq!(
            service
                .get_cached_global_config()
                .await
                .unwrap()
                .output_folder,
            "/imported"
        );
        pool.close().await;
    }
}
