//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tracing::info;

use crate::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
use crate::database::repositories::{
    config::SqlxConfigRepository,
    streamer::SqlxStreamerRepository,
};
use crate::streamer::StreamerManager;
use crate::Result;

/// Default cache TTL (1 hour).
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Service container holding all application services.
pub struct ServiceContainer {
    /// Database connection pool.
    pub pool: SqlitePool,
    /// Configuration service.
    pub config_service: Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
    /// Streamer manager.
    pub streamer_manager: Arc<StreamerManager<SqlxStreamerRepository>>,
    /// Event broadcaster (shared between services).
    pub event_broadcaster: ConfigEventBroadcaster,
}

impl ServiceContainer {
    /// Create a new service container with the given database pool.
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        Self::with_config(pool, DEFAULT_CACHE_TTL, DEFAULT_EVENT_CAPACITY).await
    }

    /// Create a new service container with custom configuration.
    pub async fn with_config(
        pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
    ) -> Result<Self> {
        info!("Initializing service container");

        // Create repositories
        let config_repo = Arc::new(SqlxConfigRepository::new(pool.clone()));
        let streamer_repo = Arc::new(SqlxStreamerRepository::new(pool.clone()));

        // Create shared event broadcaster
        let event_broadcaster = ConfigEventBroadcaster::with_capacity(event_capacity);

        // Create config service with custom cache
        let cache = ConfigCache::with_ttl(cache_ttl);
        let config_service = Arc::new(ConfigService::with_cache(
            config_repo.clone(),
            streamer_repo.clone(),
            cache,
        ));

        // Create streamer manager
        let streamer_manager = Arc::new(StreamerManager::new(
            streamer_repo,
            event_broadcaster.clone(),
        ));

        info!("Service container initialized");

        Ok(Self {
            pool,
            config_service,
            streamer_manager,
            event_broadcaster,
        })
    }

    /// Initialize all services (hydrate data, start background tasks, etc.).
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing services");

        // Hydrate streamer manager from database
        let streamer_count = self.streamer_manager.hydrate().await?;
        info!("Hydrated {} streamers", streamer_count);

        // Subscribe streamer manager to config events
        self.setup_event_subscriptions();

        info!("Services initialized");
        Ok(())
    }

    /// Set up event subscriptions between services.
    fn setup_event_subscriptions(&self) {
        let _streamer_manager = self.streamer_manager.clone();
        let mut receiver = self.event_broadcaster.subscribe();

        // Spawn a task to handle config update events
        tokio::spawn(async move {
            use crate::config::ConfigUpdateEvent;

            while let Ok(event) = receiver.recv().await {
                match event {
                    ConfigUpdateEvent::StreamerUpdated { streamer_id } => {
                        // Refresh streamer metadata when config changes
                        tracing::debug!(
                            "Received streamer config update event: {}",
                            streamer_id
                        );
                        // Note: In a full implementation, we would refresh the
                        // streamer's metadata from the database here
                    }
                    ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                        tracing::debug!(
                            "Received platform config update event: {}",
                            platform_id
                        );
                        // Platform updates may affect multiple streamers
                    }
                    ConfigUpdateEvent::TemplateUpdated { template_id } => {
                        tracing::debug!(
                            "Received template config update event: {}",
                            template_id
                        );
                        // Template updates may affect multiple streamers
                    }
                    ConfigUpdateEvent::GlobalUpdated => {
                        tracing::debug!("Received global config update event");
                        // Global updates affect all streamers
                    }
                    ConfigUpdateEvent::EngineUpdated { engine_id } => {
                        tracing::debug!(
                            "Received engine config update event: {}",
                            engine_id
                        );
                    }
                }
            }
        });
    }

    /// Shutdown all services gracefully.
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down services");

        // Close database pool
        self.pool.close().await;

        info!("Services shut down");
        Ok(())
    }

    /// Get service statistics.
    pub fn stats(&self) -> ServiceStats {
        ServiceStats {
            streamer_count: self.streamer_manager.count(),
            active_streamer_count: self.streamer_manager.active_count(),
            live_streamer_count: self.streamer_manager.live_count(),
            disabled_streamer_count: self.streamer_manager.disabled_count(),
            cache_stats: self.config_service.cache_stats(),
            event_subscriber_count: self.event_broadcaster.subscriber_count(),
        }
    }
}

/// Service statistics.
#[derive(Debug, Clone)]
pub struct ServiceStats {
    /// Total number of streamers.
    pub streamer_count: usize,
    /// Number of active streamers.
    pub active_streamer_count: usize,
    /// Number of live streamers.
    pub live_streamer_count: usize,
    /// Number of disabled streamers.
    pub disabled_streamer_count: usize,
    /// Cache statistics.
    pub cache_stats: crate::config::CacheStats,
    /// Number of event subscribers.
    pub event_subscriber_count: usize,
}

#[cfg(test)]
mod tests {
    // Integration tests would go here with a test database
}
