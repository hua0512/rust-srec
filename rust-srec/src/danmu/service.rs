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
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::danmu::{
    DanmuSampler, DanmuSamplingConfig as SamplerConfig, DanmuStatistics, ProviderRegistry,
    create_sampler,
};
use crate::database::models::DanmuRateEntry;
use crate::database::repositories::SessionRepository;
use crate::domain::DanmuSamplingConfig;
use crate::error::{Error, Result};
use platforms_parser::danmaku::ConnectionConfig;

use super::events::{CollectionCommand, DanmuEvent};
use super::runner::{CollectionRunner, RunnerParams};

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
    /// Cancellation token for this collection
    cancel_token: CancellationToken,
    /// Command sender
    command_tx: mpsc::Sender<CollectionCommand>,
    /// Signals when the runner has fully stopped (including final XML flush/finalize),
    /// carrying final statistics when available.
    done_rx: Option<oneshot::Receiver<std::result::Result<DanmuStatistics, String>>>,
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
    /// Reverse index for fast lookups (streamer_id -> session_id).
    sessions_by_streamer: Arc<DashMap<String, String>>,
    /// Event sender
    event_tx: broadcast::Sender<DanmuEvent>,
    /// Global cancellation token
    cancel_token: CancellationToken,
    /// Session repository for persistence
    session_repo: Option<Arc<dyn crate::database::repositories::SessionRepository>>,
}

impl DanmuService {
    const DEFAULT_MAX_TOP_TALKERS: usize = 32;
    const DEFAULT_MAX_WORDS: usize = 50;
    const DEFAULT_RATE_BUCKET_SECS: u64 = 10;

    /// Create a new danmu service.
    pub fn new(config: DanmuServiceConfig) -> Self {
        let (event_tx, _) = broadcast::channel(256);

        Self {
            config,
            providers: Arc::new(ProviderRegistry::with_defaults()),
            collections: Arc::new(DashMap::new()),
            sessions_by_streamer: Arc::new(DashMap::new()),
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
            sessions_by_streamer: Arc::new(DashMap::new()),
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
        const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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
        // - Douyu: uses "rid" from extras
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
            "douyu" => {
                // Douyu uses rid for danmu connection
                extras
                    .as_ref()
                    .and_then(|e| e.get("rid"))
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

        // Build bounded per-session statistics/sampler state.
        let max_top_talkers =
            Self::DEFAULT_MAX_TOP_TALKERS.min(self.config.stats_buffer_size.max(10));
        let max_words = Self::DEFAULT_MAX_WORDS.min(self.config.stats_buffer_size.max(25));
        let stats = platforms_parser::danmaku::StatisticsAggregator::with_config(
            max_top_talkers,
            max_words,
            Self::DEFAULT_RATE_BUCKET_SECS,
        );
        let sampler: Box<dyn DanmuSampler> = if self.config.sampling_enabled {
            let sampling = sampling_config.unwrap_or_else(|| self.config.default_sampling.clone());
            let sampler_config = to_sampler_config(&sampling);
            create_sampler(&sampler_config)
        } else {
            Box::new(NoopSampler)
        };
        let cancel_token = self.cancel_token.child_token();

        let (ready_tx, ready_rx) = oneshot::channel::<Result<()>>();
        let (done_tx, done_rx) = oneshot::channel::<std::result::Result<DanmuStatistics, String>>();

        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            cancel_token: cancel_token.clone(),
            command_tx: command_tx.clone(),
            done_rx: Some(done_rx),
        };

        self.collections.insert(session_id.to_string(), state);
        self.sessions_by_streamer
            .insert(streamer_id.to_string(), session_id.to_string());

        // Start collection task
        let session_id_clone = session_id.to_string();
        let streamer_id_clone = streamer_id.to_string();
        let room_id_clone = room_id.clone();
        let event_tx = self.event_tx.clone();
        let collections = self.collections.clone();
        let sessions_by_streamer = self.sessions_by_streamer.clone();
        let session_repo = self.session_repo.clone();
        let provider = Arc::clone(&provider);
        let sampling_enabled = self.config.sampling_enabled;
        let conn_config = connection_config;
        let cancel_token_task = cancel_token.clone();

        tokio::spawn(async move {
            let runner = match tokio::time::timeout(
                CONNECT_TIMEOUT,
                CollectionRunner::new(RunnerParams {
                    session_id: session_id_clone.clone(),
                    streamer_id: streamer_id_clone.clone(),
                    room_id: room_id_clone,
                    provider: Arc::clone(&provider),
                    conn_config,
                    stats,
                    sampler,
                    sampling_enabled,
                    event_tx: event_tx.clone(),
                }),
            )
            .await
            {
                Ok(Ok(runner)) => {
                    let _ = ready_tx.send(Ok(()));
                    runner
                }
                Ok(Err(e)) => {
                    let error_message = e.to_string();
                    let _ = event_tx.send(DanmuEvent::Error {
                        session_id: session_id_clone.clone(),
                        error: error_message.clone(),
                    });
                    let _ = ready_tx.send(Err(e));
                    if let Some((_, state)) = collections.remove(&session_id_clone) {
                        let should_remove = sessions_by_streamer
                            .get(&state.streamer_id)
                            .is_some_and(|sid| sid.value() == &session_id_clone);
                        if should_remove {
                            sessions_by_streamer.remove(&state.streamer_id);
                        }
                    }
                    let _ = done_tx.send(Err(error_message));
                    return;
                }
                Err(_) => {
                    let message = format!(
                        "Danmu connection timed out after {:?} (session_id={})",
                        CONNECT_TIMEOUT, session_id_clone
                    );
                    let _ = ready_tx.send(Err(Error::from(
                        platforms_parser::danmaku::DanmakuError::connection(message.clone()),
                    )));
                    let _ = event_tx.send(DanmuEvent::Error {
                        session_id: session_id_clone.clone(),
                        error: message.clone(),
                    });
                    if let Some((_, state)) = collections.remove(&session_id_clone) {
                        let should_remove = sessions_by_streamer
                            .get(&state.streamer_id)
                            .is_some_and(|sid| sid.value() == &session_id_clone);
                        if should_remove {
                            sessions_by_streamer.remove(&state.streamer_id);
                        }
                    }
                    let _ = done_tx.send(Err(message));
                    return;
                }
            };

            let result = runner.run(command_rx, cancel_token_task).await;
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
                let should_remove = sessions_by_streamer
                    .get(&state.streamer_id)
                    .is_some_and(|sid| sid.value() == &session_id_clone);
                if should_remove {
                    sessions_by_streamer.remove(&state.streamer_id);
                }
                if let Ok(statistics) = &result {
                    persist_statistics(session_repo.as_deref(), &session_id_clone, statistics)
                        .await;
                    let _ = event_tx.send(DanmuEvent::CollectionStopped {
                        session_id: session_id_clone.clone(),
                        statistics: statistics.clone(),
                    });
                }
            }

            let done_value = result.map_err(|e| e.to_string());
            let _ = done_tx.send(done_value);
        });

        tokio::select! {
            ready = ready_rx => {
                match ready {
                    Ok(Ok(())) => {
                        let _ = self.event_tx.send(DanmuEvent::CollectionStarted {
                            session_id: session_id.to_string(),
                            streamer_id: streamer_id.to_string(),
                        });
                    }
                    Ok(Err(e)) => {
                        if let Some((_, state)) = self.collections.remove(session_id) {
                            let should_remove = self
                                .sessions_by_streamer
                                .get(&state.streamer_id)
                                .is_some_and(|sid| sid.value() == session_id);
                            if should_remove {
                                self.sessions_by_streamer.remove(&state.streamer_id);
                            }
                        }
                        return Err(e);
                    }
                    Err(_) => {
                        if let Some((_, state)) = self.collections.remove(session_id) {
                            let should_remove = self
                                .sessions_by_streamer
                                .get(&state.streamer_id)
                                .is_some_and(|sid| sid.value() == session_id);
                            if should_remove {
                                self.sessions_by_streamer.remove(&state.streamer_id);
                            }
                        }
                        return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                            "Danmu collection task stopped before it became ready",
                        )));
                    }
                }
            }
            _ = cancel_token.cancelled() => {
                if let Some((_, state)) = self.collections.remove(session_id) {
                    let should_remove = self
                        .sessions_by_streamer
                        .get(&state.streamer_id)
                        .is_some_and(|sid| sid.value() == session_id);
                    if should_remove {
                        self.sessions_by_streamer.remove(&state.streamer_id);
                    }
                }
                return Err(Error::from(platforms_parser::danmaku::DanmakuError::connection(
                    "Danmu collection cancelled before it became ready",
                )));
            }
        }

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

        let should_remove = self
            .sessions_by_streamer
            .get(&state.streamer_id)
            .is_some_and(|sid| sid.value() == session_id);
        if should_remove {
            self.sessions_by_streamer.remove(&state.streamer_id);
        }

        // Send stop command
        let _ = state.command_tx.send(CollectionCommand::Stop).await;

        // Cancel the collection task
        state.cancel_token.cancel();

        if let Some(done_rx) = state.done_rx {
            const STOP_TIMEOUT: Duration = Duration::from_secs(10);
            match tokio::time::timeout(STOP_TIMEOUT, done_rx).await {
                Ok(Ok(Ok(statistics))) => {
                    persist_statistics(self.session_repo.as_deref(), session_id, &statistics).await;
                    let _ = self.event_tx.send(DanmuEvent::CollectionStopped {
                        session_id: session_id.to_string(),
                        statistics: statistics.clone(),
                    });
                    return Ok(statistics);
                }
                Ok(Ok(Err(error))) => {
                    warn!(
                        session_id,
                        error, "Danmu collection ended without final statistics"
                    );
                }
                Ok(Err(_)) => {}
                Err(_) => {
                    warn!(
                        "Danmu collection stop timed out after {:?} (session_id={})",
                        STOP_TIMEOUT, session_id
                    );
                }
            }
        }

        Ok(DanmuStatistics::default())
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

    /// Get all active session IDs.
    pub fn active_sessions(&self) -> Vec<String> {
        self.collections.iter().map(|r| r.key().clone()).collect()
    }

    /// Get the session ID for a streamer if one exists.
    ///
    /// Iterates over active collections to find a session matching the given streamer ID.
    /// Returns the session ID if found, None otherwise.
    pub fn get_session_by_streamer(&self, streamer_id: &str) -> Option<String> {
        self.sessions_by_streamer
            .get(streamer_id)
            .map(|entry| entry.value().clone())
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

fn saturating_u64_to_i64(value: u64) -> i64 {
    if value > i64::MAX as u64 {
        i64::MAX
    } else {
        value as i64
    }
}

async fn persist_statistics(
    session_repo: Option<&dyn SessionRepository>,
    session_id: &str,
    statistics: &DanmuStatistics,
) {
    #[derive(serde::Serialize)]
    struct TopTalkerView<'a> {
        user_id: &'a str,
        username: &'a str,
        message_count: i64,
    }

    #[derive(serde::Serialize)]
    struct WordFrequencyView<'a> {
        word: &'a str,
        count: i64,
    }

    let Some(repo) = session_repo else {
        return;
    };

    let rate_timeseries = statistics
        .rate_timeseries
        .iter()
        .map(|entry| DanmuRateEntry {
            ts: entry.timestamp.timestamp_millis(),
            count: saturating_u64_to_i64(entry.count),
        });
    let danmu_rate_timeseries = match serde_json::to_string(&rate_timeseries.collect::<Vec<_>>()) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(session_id, %error, "Failed to serialize danmu rate timeseries");
            None
        }
    };

    let top_talkers = statistics.top_talkers.iter().map(|entry| TopTalkerView {
        user_id: entry.user_id.as_str(),
        username: entry.username.as_str(),
        message_count: saturating_u64_to_i64(entry.message_count),
    });
    let top_talkers = match serde_json::to_string(&top_talkers.collect::<Vec<_>>()) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(session_id, %error, "Failed to serialize top talkers");
            None
        }
    };

    let word_frequency = statistics
        .word_frequency
        .iter()
        .map(|entry| WordFrequencyView {
            word: entry.word.as_str(),
            count: saturating_u64_to_i64(entry.count),
        });
    let word_frequency = match serde_json::to_string(&word_frequency.collect::<Vec<_>>()) {
        Ok(value) => Some(value),
        Err(error) => {
            warn!(session_id, %error, "Failed to serialize word frequency");
            None
        }
    };

    if let Err(error) = repo
        .upsert_danmu_statistics(
            session_id,
            saturating_u64_to_i64(statistics.total_count),
            danmu_rate_timeseries.as_deref(),
            top_talkers.as_deref(),
            word_frequency.as_deref(),
        )
        .await
    {
        warn!(session_id, %error, "Failed to persist danmu statistics");
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
