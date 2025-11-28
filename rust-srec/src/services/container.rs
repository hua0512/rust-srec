//! Service container for dependency injection.
//!
//! The ServiceContainer holds references to all application services
//! and manages their lifecycle.

use std::sync::Arc;
use std::time::Duration;

use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::{ConfigCache, ConfigEventBroadcaster, ConfigService};
use crate::database::repositories::{
    config::SqlxConfigRepository,
    streamer::SqlxStreamerRepository,
};
use crate::downloader::{DownloadConfig, DownloadManager, DownloadManagerConfig, StreamSelector};
use crate::monitor::{MonitorEvent, MonitorEventBroadcaster};
use crate::pipeline::{PipelineManager, PipelineManagerConfig, PipelineEvent};
use crate::streamer::StreamerManager;
use crate::Result;

/// Default cache TTL (1 hour).
const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

/// Default event channel capacity.
const DEFAULT_EVENT_CAPACITY: usize = 256;

/// Default shutdown timeout.
const DEFAULT_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(30);

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
    /// Download manager.
    pub download_manager: Arc<DownloadManager>,
    /// Pipeline manager.
    pub pipeline_manager: Arc<PipelineManager>,
    /// Monitor event broadcaster.
    pub monitor_event_broadcaster: MonitorEventBroadcaster,
    /// Cancellation token for graceful shutdown.
    cancellation_token: CancellationToken,
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

        // Create download manager with default config
        let download_manager = Arc::new(DownloadManager::with_config(
            DownloadManagerConfig::default(),
        ));

        // Create pipeline manager with default config
        let pipeline_manager = Arc::new(PipelineManager::with_config(
            PipelineManagerConfig::default(),
        ));

        // Create monitor event broadcaster
        let monitor_event_broadcaster = MonitorEventBroadcaster::with_capacity(event_capacity);

        // Create cancellation token for graceful shutdown
        let cancellation_token = CancellationToken::new();

        info!("Service container initialized");

        Ok(Self {
            pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            cancellation_token,
        })
    }

    /// Create a new service container with custom download and pipeline configs.
    pub async fn with_full_config(
        pool: SqlitePool,
        cache_ttl: Duration,
        event_capacity: usize,
        download_config: DownloadManagerConfig,
        pipeline_config: PipelineManagerConfig,
    ) -> Result<Self> {
        info!("Initializing service container with full configuration");

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

        // Create download manager with custom config
        let download_manager = Arc::new(DownloadManager::with_config(download_config));

        // Create pipeline manager with custom config
        let pipeline_manager = Arc::new(PipelineManager::with_config(pipeline_config));

        // Create monitor event broadcaster
        let monitor_event_broadcaster = MonitorEventBroadcaster::with_capacity(event_capacity);

        // Create cancellation token for graceful shutdown
        let cancellation_token = CancellationToken::new();

        info!("Service container initialized with full configuration");

        Ok(Self {
            pool,
            config_service,
            streamer_manager,
            event_broadcaster,
            download_manager,
            pipeline_manager,
            monitor_event_broadcaster,
            cancellation_token,
        })
    }

    /// Initialize all services (hydrate data, start background tasks, etc.).
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing services");

        // Hydrate streamer manager from database
        let streamer_count = self.streamer_manager.hydrate().await?;
        info!("Hydrated {} streamers", streamer_count);

        // Start pipeline manager
        self.pipeline_manager.start();
        info!("Pipeline manager started");

        // Subscribe streamer manager to config events
        self.setup_config_event_subscriptions();

        // Wire download events to pipeline manager
        self.setup_download_event_subscriptions();

        // Wire monitor events to download manager
        self.setup_monitor_event_subscriptions();

        info!("Services initialized");
        Ok(())
    }

    /// Set up config event subscriptions between services.
    fn setup_config_event_subscriptions(&self) {
        let _streamer_manager = self.streamer_manager.clone();
        let mut receiver = self.event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        // Spawn a task to handle config update events
        tokio::spawn(async move {
            use crate::config::ConfigUpdateEvent;

            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Config event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                match event {
                                    ConfigUpdateEvent::StreamerUpdated { streamer_id } => {
                                        debug!(
                                            "Received streamer config update event: {}",
                                            streamer_id
                                        );
                                    }
                                    ConfigUpdateEvent::PlatformUpdated { platform_id } => {
                                        debug!(
                                            "Received platform config update event: {}",
                                            platform_id
                                        );
                                    }
                                    ConfigUpdateEvent::TemplateUpdated { template_id } => {
                                        debug!(
                                            "Received template config update event: {}",
                                            template_id
                                        );
                                    }
                                    ConfigUpdateEvent::GlobalUpdated => {
                                        debug!("Received global config update event");
                                    }
                                    ConfigUpdateEvent::EngineUpdated { engine_id } => {
                                        debug!(
                                            "Received engine config update event: {}",
                                            engine_id
                                        );
                                    }
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    /// Set up download event subscriptions to pipeline manager.
    fn setup_download_event_subscriptions(&self) {
        let pipeline_manager = self.pipeline_manager.clone();
        let mut receiver = self.download_manager.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Download event handler shutting down");
                        break;
                    }
                    event = receiver.recv() => {
                        match event {
                            Some(download_event) => {
                                pipeline_manager.handle_download_event(download_event).await;
                            }
                            None => break,
                        }
                    }
                }
            }
        });
    }

    /// Set up monitor event subscriptions to download manager.
    fn setup_monitor_event_subscriptions(&self) {
        let download_manager = self.download_manager.clone();
        let streamer_manager = self.streamer_manager.clone();
        let config_service = self.config_service.clone();
        let mut receiver = self.monitor_event_broadcaster.subscribe();
        let cancellation_token = self.cancellation_token.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancellation_token.cancelled() => {
                        debug!("Monitor event handler shutting down");
                        break;
                    }
                    result = receiver.recv() => {
                        match result {
                            Ok(event) => {
                                Self::handle_monitor_event(
                                    &download_manager,
                                    &streamer_manager,
                                    &config_service,
                                    event,
                                ).await;
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });
    }

    /// Handle monitor events to trigger downloads.
    async fn handle_monitor_event(
        download_manager: &Arc<DownloadManager>,
        streamer_manager: &Arc<StreamerManager<SqlxStreamerRepository>>,
        config_service: &Arc<ConfigService<SqlxConfigRepository, SqlxStreamerRepository>>,
        event: MonitorEvent,
    ) {
        match event {
            MonitorEvent::StreamerLive {
                streamer_id,
                streamer_name,
                title,
                streams,
                ..
            } => {
                info!(
                    "Streamer {} ({}) went live: {} ({} streams available)",
                    streamer_name, streamer_id, title, streams.len()
                );

                // Check if already downloading
                if download_manager.has_active_download(&streamer_id) {
                    debug!("Download already active for {}", streamer_id);
                    return;
                }

                // Validate we have streams to download
                if streams.is_empty() {
                    warn!(
                        "Streamer {} has no streams available, cannot start download",
                        streamer_id
                    );
                    return;
                }

                // Get streamer metadata for priority
                let streamer_metadata = streamer_manager.get_streamer(&streamer_id);
                let is_high_priority = streamer_metadata
                    .as_ref()
                    .map(|s| s.priority == crate::domain::Priority::High)
                    .unwrap_or(false);

                // Load merged config for this streamer to get stream selection preferences
                let merged_config = match config_service.get_config_for_streamer(&streamer_id).await {
                    Ok(config) => config,
                    Err(e) => {
                        warn!(
                            "Failed to load config for streamer {}, using defaults: {}",
                            streamer_id, e
                        );
                        // Use default config if we can't load the merged config
                        crate::domain::config::MergedConfig::builder().build()
                    }
                };

                // Select the best stream based on merged config preferences
                let stream_selector = StreamSelector::with_config(merged_config.stream_selection.clone());
                let best_stream = match stream_selector.select_best(&streams) {
                    Some(stream) => stream,
                    None => {
                        warn!(
                            "No suitable stream found for streamer {} after filtering",
                            streamer_id
                        );
                        return;
                    }
                };
                let stream_url = best_stream.url.clone();
                let stream_format = best_stream.stream_format.as_str();
                let media_format = best_stream.media_format.as_str();

                // Extract headers from stream extras if needed
                let headers: Vec<(String, String)> = if best_stream.is_headers_needed {
                    best_stream.extras
                        .as_ref()
                        .and_then(|extras| extras.get("headers"))
                        .and_then(|h| h.as_object())
                        .map(|headers_map| {
                            headers_map
                                .iter()
                                .filter_map(|(k, v)| {
                                    v.as_str().map(|val| (k.clone(), val.to_string()))
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };

                // Create download config using the actual stream URL and merged config settings
                let session_id = uuid::Uuid::new_v4().to_string();
                let output_dir = format!("{}/{}/{}", merged_config.output_folder, streamer_id, session_id);

                let mut config = DownloadConfig::new(
                    stream_url.clone(),
                    output_dir,
                    streamer_id.clone(),
                    session_id,
                )
                .with_filename_template(&merged_config.output_filename_template.replace("{streamer}", &streamer_name))
                .with_output_format(&merged_config.output_file_format)
                .with_max_segment_duration(merged_config.max_download_duration_secs as u64);

                // Add headers if needed
                for (key, value) in headers {
                    config = config.with_header(key, value);
                }

                info!(
                    "Starting download for {} with stream URL: {} (stream_format: {}, media_format: {}, headers_needed: {}, output: {})",
                    streamer_name, stream_url, stream_format, media_format, best_stream.is_headers_needed, merged_config.output_folder
                );

                // Start download
                match download_manager
                    .start_download(config, None, is_high_priority)
                    .await
                {
                    Ok(download_id) => {
                        info!(
                            "Started download {} for streamer {} (priority: {})",
                            download_id,
                            streamer_id,
                            if is_high_priority { "high" } else { "normal" }
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to start download for streamer {}: {}",
                            streamer_id, e
                        );
                    }
                }
            }
            MonitorEvent::StreamerOffline {
                streamer_id,
                streamer_name,
                ..
            } => {
                info!("Streamer {} ({}) went offline", streamer_name, streamer_id);

                // Stop download if active
                if let Some(download_info) = download_manager.get_download_by_streamer(&streamer_id) {
                    match download_manager.stop_download(&download_info.id).await {
                        Ok(()) => {
                            info!(
                                "Stopped download {} for streamer {}",
                                download_info.id, streamer_id
                            );
                        }
                        Err(e) => {
                            warn!(
                                "Failed to stop download for streamer {}: {}",
                                streamer_id, e
                            );
                        }
                    }
                }
            }
            _ => {
                // Other events don't trigger download actions
            }
        }
    }

    /// Shutdown all services gracefully.
    pub async fn shutdown(&self) -> Result<()> {
        self.shutdown_with_timeout(DEFAULT_SHUTDOWN_TIMEOUT).await
    }

    /// Shutdown all services gracefully with a custom timeout.
    pub async fn shutdown_with_timeout(&self, timeout: Duration) -> Result<()> {
        info!("Shutting down services (timeout: {:?})", timeout);

        // Signal all background tasks to stop
        self.cancellation_token.cancel();

        // Stop accepting new downloads
        info!("Stopping download manager...");
        let stopped_downloads = self.download_manager.stop_all().await;
        info!("Stopped {} active downloads", stopped_downloads.len());

        // Stop pipeline manager and drain job queue
        info!("Stopping pipeline manager...");
        self.pipeline_manager.stop().await;
        info!("Pipeline manager stopped");

        // Wait for background tasks with timeout
        let shutdown_result = tokio::time::timeout(timeout, async {
            // Give background tasks time to clean up
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;

        if shutdown_result.is_err() {
            warn!("Shutdown timeout reached, forcing shutdown");
        }

        // Close database pool
        info!("Closing database pool...");
        self.pool.close().await;

        info!("Services shut down");
        Ok(())
    }

    /// Get the cancellation token for external use.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation_token.clone()
    }

    /// Check if shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        self.cancellation_token.is_cancelled()
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
            active_downloads: self.download_manager.active_count(),
            pipeline_queue_depth: self.pipeline_manager.queue_depth(),
        }
    }

    /// Subscribe to pipeline events.
    pub fn subscribe_pipeline_events(&self) -> tokio::sync::broadcast::Receiver<PipelineEvent> {
        self.pipeline_manager.subscribe()
    }

    /// Subscribe to monitor events.
    pub fn subscribe_monitor_events(&self) -> tokio::sync::broadcast::Receiver<MonitorEvent> {
        self.monitor_event_broadcaster.subscribe()
    }

    /// Get the monitor event broadcaster for external use.
    pub fn monitor_broadcaster(&self) -> &MonitorEventBroadcaster {
        &self.monitor_event_broadcaster
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
    /// Number of active downloads.
    pub active_downloads: usize,
    /// Pipeline job queue depth.
    pub pipeline_queue_depth: usize,
}

#[cfg(test)]
mod tests {
    // Integration tests would go here with a test database
}
