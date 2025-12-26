//! Danmu collection service.
//!
//! Manages danmu collection for live sessions with segment-based file writing.
//!
//! The danmu collection follows this model:
//! - **Session level**: WebSocket connection stays alive, statistics are aggregated
//! - **Segment level**: XML files are created/finalized per download segment
//!
//! When a new segment starts → create new danmu XML file
//! When segment closes → finalize that XML file, but keep collecting danmu
//! When session ends → stop collection entirely

use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::danmu::{
    DanmuSampler, DanmuSamplingConfig as SamplerConfig, DanmuStatistics, ProviderRegistry,
    StatisticsAggregator, create_sampler,
};
use crate::domain::DanmuSamplingConfig;
use crate::error::{Error, Result};
use platforms_parser::danmaku::ConnectionConfig;

use super::events::{CollectionCommand, DanmuEvent};

/// Configuration for the danmu service.
#[derive(Debug, Clone)]
pub struct DanmuServiceConfig {
    /// Whether statistics aggregation is enabled.
    ///
    /// When disabled, the service still records danmu to segment files, but does not compute
    /// per-session statistics (top talkers, word frequency, rate timeseries, etc.).
    pub statistics_enabled: bool,
    /// Whether sampling is enabled.
    ///
    /// Sampling is an optimization hint for statistics. When disabled, the sampler is not updated.
    pub sampling_enabled: bool,
    /// Default sampling configuration
    pub default_sampling: DanmuSamplingConfig,
    /// Buffer size for statistics (number of recent messages to keep)
    pub stats_buffer_size: usize,
}

impl Default for DanmuServiceConfig {
    fn default() -> Self {
        Self {
            statistics_enabled: false,
            sampling_enabled: false,
            default_sampling: DanmuSamplingConfig::default(),
            stats_buffer_size: 100,
        }
    }
}

/// Convert domain DanmuSamplingConfig to sampler config.
fn to_sampler_config(config: &DanmuSamplingConfig) -> SamplerConfig {
    use crate::domain::value_objects::SamplingStrategy;

    match config.strategy {
        SamplingStrategy::Fixed => SamplerConfig::Fixed {
            interval_secs: config.interval_secs as u64,
        },
        SamplingStrategy::Dynamic => SamplerConfig::Velocity {
            min_interval_secs: config.min_interval_secs as u64,
            max_interval_secs: config.max_interval_secs as u64,
            target_danmus_per_sample: config.target_danmus_per_sample,
        },
    }
}

/// Handle for controlling a danmu collection session.
#[derive(Clone)]
pub struct CollectionHandle {
    session_id: String,
    command_tx: mpsc::Sender<CollectionCommand>,
}

impl CollectionHandle {
    /// Start writing to a new segment file.
    ///
    /// The `start_time` is used to calculate danmu timestamp offsets.
    pub async fn start_segment(
        &self,
        segment_id: &str,
        output_path: PathBuf,
        start_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: segment_id.to_string(),
                output_path,
                start_time,
            })
            .await
            .map_err(|_| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Collection task not running",
                ))
            })
    }

    /// End the current segment file (finalize XML).
    pub async fn end_segment(&self, segment_id: &str) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::EndSegment {
                segment_id: segment_id.to_string(),
            })
            .await
            .map_err(|_| {
                Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Collection task not running",
                ))
            })
    }

    /// Get the session ID.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// State for an active danmu collection session.
struct CollectionState {
    /// Streamer ID
    streamer_id: String,
    /// Statistics aggregator (session-level)
    stats: Arc<Mutex<StatisticsAggregator>>,
    /// Cancellation token for this collection
    cancel_token: CancellationToken,
    /// Command sender
    command_tx: mpsc::Sender<CollectionCommand>,
}

#[derive(Debug, Default)]
struct NoopSampler;

impl DanmuSampler for NoopSampler {
    fn record_message(&mut self, _timestamp: chrono::DateTime<chrono::Utc>) {}

    fn should_sample(&self, _now: chrono::DateTime<chrono::Utc>) -> bool {
        false
    }

    fn mark_sampled(&mut self, _timestamp: chrono::DateTime<chrono::Utc>) {}

    fn current_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::MAX / 2)
    }

    fn reset(&mut self) {}
}

/// Danmu collection service.
pub struct DanmuService {
    /// Configuration
    config: DanmuServiceConfig,
    /// Provider registry
    providers: Arc<ProviderRegistry>,
    /// Active collections (session_id -> state)
    collections: Arc<DashMap<String, CollectionState>>,
    /// Event sender
    event_tx: broadcast::Sender<DanmuEvent>,
    /// Global cancellation token
    cancel_token: CancellationToken,
    /// Session repository for persistence
    session_repo: Option<Arc<dyn crate::database::repositories::SessionRepository>>,
}

impl DanmuService {
    /// Create a new danmu service.
    pub fn new(config: DanmuServiceConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        Self {
            config,
            providers: Arc::new(ProviderRegistry::with_defaults()),
            collections: Arc::new(DashMap::new()),
            event_tx,
            cancel_token: CancellationToken::new(),
            session_repo: None,
        }
    }

    /// Create a new danmu service with custom providers.
    pub fn with_providers(config: DanmuServiceConfig, providers: ProviderRegistry) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        Self {
            config,
            providers: Arc::new(providers),
            collections: Arc::new(DashMap::new()),
            event_tx,
            cancel_token: CancellationToken::new(),
            session_repo: None,
        }
    }

    /// Set the session repository for persistence.
    pub fn with_session_repository(
        mut self,
        repo: Arc<dyn crate::database::repositories::SessionRepository>,
    ) -> Self {
        self.session_repo = Some(repo);
        self
    }

    /// Get the session repository (if set).
    pub fn session_repo(
        &self,
    ) -> Option<&Arc<dyn crate::database::repositories::SessionRepository>> {
        self.session_repo.as_ref()
    }

    /// Subscribe to danmu events.
    pub fn subscribe(&self) -> broadcast::Receiver<DanmuEvent> {
        self.event_tx.subscribe()
    }

    /// Start danmu collection for a session.
    /// Returns a handle that can be used to control segment file writing.
    pub async fn start_collection(
        &self,
        session_id: &str,
        streamer_id: &str,
        streamer_url: &str,
        sampling_config: Option<DanmuSamplingConfig>,
        cookies: Option<String>,
        extras: Option<std::collections::HashMap<String, String>>,
    ) -> Result<CollectionHandle> {
        // Check if already collecting
        if self.collections.contains_key(session_id) {
            return Err(Error::from(
                platforms_parser::danmaku::DanmakuError::connection(format!(
                    "Collection already active for session {}",
                    session_id
                )),
            ));
        }

        // Find provider for URL
        let provider = self.providers.get_by_url(streamer_url).ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("No danmu provider for URL: {}", streamer_url),
            ))
        })?;

        // Extract room ID - use platform-specific extras when available
        // - Huya: uses "presenter_uid" from extras
        // - Douyin: uses "id_str" from extras
        // - Others: fallback to URL-based extraction
        let platform = provider.platform();
        let room_id = match platform {
            "huya" => {
                // Huya uses presenter_uid for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("presenter_uid"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            "douyin" => {
                // Douyin uses id_str (room_id) for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("id_str"))
                    .cloned()
                    .or_else(|| provider.extract_room_id(streamer_url))
            }
            _ => provider.extract_room_id(streamer_url),
        }
        .ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("Could not extract room ID from URL: {}", streamer_url),
            ))
        })?;

        // Build connection config
        let mut connection_config = ConnectionConfig::with_cookies(cookies.clone());
        if let Some(e) = extras {
            // Remove common fields that are used for room ID extraction but might be useful as extras too
            // We keep them in extras for now as it's cleaner
            connection_config = connection_config.with_extras(e);
        }

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(32);

        // Create state
        let stats = Arc::new(Mutex::new(StatisticsAggregator::new()));
        let sampler: Arc<Mutex<Box<dyn DanmuSampler>>> = if self.config.sampling_enabled {
            let sampling = sampling_config.unwrap_or_else(|| self.config.default_sampling.clone());
            let sampler_config = to_sampler_config(&sampling);
            Arc::new(Mutex::new(create_sampler(&sampler_config)))
        } else {
            Arc::new(Mutex::new(Box::new(NoopSampler)))
        };
        let cancel_token = self.cancel_token.child_token();

        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            stats: Arc::clone(&stats),
            cancel_token: cancel_token.clone(),
            command_tx: command_tx.clone(),
        };

        // Create runner
        let runner = super::runner::CollectionRunner::new(super::runner::RunnerParams {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
            room_id: room_id.clone(),
            provider: Arc::clone(&provider),
            conn_config: connection_config,
            stats: Arc::clone(&stats),
            statistics_enabled: self.config.statistics_enabled,
            sampler: Arc::clone(&sampler),
            sampling_enabled: self.config.sampling_enabled,
            event_tx: self.event_tx.clone(),
        })
        .await?;

        self.collections.insert(session_id.to_string(), state);

        // Emit event
        let _ = self.event_tx.send(DanmuEvent::CollectionStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
        });

        // Start collection task
        let session_id_clone = session_id.to_string();
        let event_tx = self.event_tx.clone();
        let collections = self.collections.clone();

        tokio::spawn(async move {
            let result = runner.run(command_rx, cancel_token).await;
            if let Err(e) = &result {
                let _ = event_tx.send(DanmuEvent::Error {
                    session_id: session_id_clone.clone(),
                    error: e.to_string(),
                });
            }

            // Ensure the in-memory collection state is cleaned up even if the runner exits due to
            // an error (or external cancellation) without an explicit `stop_collection()` call.
            //
            // If `stop_collection()` already removed the entry, `remove()` returns None and we
            // avoid emitting a duplicate stop event.
            if let Some((_, state)) = collections.remove(&session_id_clone) {
                let stats = state.stats.lock().await;
                let statistics = stats.current_stats();
                let _ = event_tx.send(DanmuEvent::CollectionStopped {
                    session_id: session_id_clone,
                    statistics,
                });
            }
        });

        Ok(CollectionHandle {
            session_id: session_id.to_string(),
            command_tx,
        })
    }

    /// Stop danmu collection for a session.
    pub async fn stop_collection(&self, session_id: &str) -> Result<DanmuStatistics> {
        // Get and remove state
        let (_, state) = self.collections.remove(session_id).ok_or_else(|| {
            Error::from(platforms_parser::danmaku::DanmakuError::connection(
                format!("No active collection for session {}", session_id),
            ))
        })?;

        // Send stop command
        let _ = state.command_tx.send(CollectionCommand::Stop).await;

        // Cancel the collection task
        state.cancel_token.cancel();

        // Finalize statistics
        let stats = state.stats.lock().await;
        let statistics = stats.current_stats();

        // Emit event
        let _ = self.event_tx.send(DanmuEvent::CollectionStopped {
            session_id: session_id.to_string(),
            statistics: statistics.clone(),
        });

        Ok(statistics)
    }

    /// Get a handle for an existing collection.
    pub fn get_handle(&self, session_id: &str) -> Option<CollectionHandle> {
        self.collections
            .get(session_id)
            .map(|state| CollectionHandle {
                session_id: session_id.to_string(),
                command_tx: state.command_tx.clone(),
            })
    }

    /// Check if collection is active for a session.
    pub fn is_collecting(&self, session_id: &str) -> bool {
        self.collections.contains_key(session_id)
    }

    /// Get current statistics for a session.
    pub async fn get_statistics(&self, session_id: &str) -> Option<DanmuStatistics> {
        if let Some(state) = self.collections.get(session_id) {
            let stats = state.stats.lock().await;
            Some(stats.current_stats())
        } else {
            None
        }
    }

    /// Get all active session IDs.
    pub fn active_sessions(&self) -> Vec<String> {
        self.collections.iter().map(|r| r.key().clone()).collect()
    }

    /// Get the session ID for a streamer if one exists.
    ///
    /// Iterates over active collections to find a session matching the given streamer ID.
    /// Returns the session ID if found, None otherwise.
    pub fn get_session_by_streamer(&self, streamer_id: &str) -> Option<String> {
        self.collections
            .iter()
            .find(|entry| entry.value().streamer_id == streamer_id)
            .map(|entry| entry.key().clone())
    }

    /// Shutdown the service.
    pub async fn shutdown(&self) {
        // Cancel all collections
        self.cancel_token.cancel();

        // Stop all active collections
        let session_ids: Vec<_> = self.collections.iter().map(|r| r.key().clone()).collect();
        for session_id in session_ids {
            let _ = self.stop_collection(&session_id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_danmu_service_creation() {
        let config = DanmuServiceConfig::default();
        let service = DanmuService::new(config);

        assert!(service.active_sessions().is_empty());
    }

    #[tokio::test]
    async fn test_is_collecting() {
        let config = DanmuServiceConfig::default();
        let service = DanmuService::new(config);

        assert!(!service.is_collecting("session1"));
    }

    #[tokio::test]
    async fn test_get_session_by_streamer_empty() {
        let config = DanmuServiceConfig::default();
        let service = DanmuService::new(config);

        // Should return None when no sessions exist
        assert!(service.get_session_by_streamer("streamer1").is_none());
    }
}
