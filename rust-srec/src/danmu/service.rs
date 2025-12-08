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

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::danmu::providers::ProviderRegistry;
use crate::danmu::sampler::{DanmuSampler, DanmuSamplingConfig as SamplerConfig, create_sampler};
use crate::danmu::statistics::{DanmuStatistics, StatisticsAggregator};
use crate::danmu::{DanmuMessage, DanmuProvider, DanmuType};
use crate::domain::DanmuSamplingConfig;
use crate::error::{Error, Result};

/// Events emitted by the danmu service.
#[derive(Debug, Clone)]
pub enum DanmuEvent {
    /// Collection started for a session
    CollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    /// Collection stopped for a session
    CollectionStopped {
        session_id: String,
        statistics: DanmuStatistics,
    },
    /// Segment file started
    SegmentStarted {
        session_id: String,
        segment_id: String,
        output_path: PathBuf,
    },
    /// Segment file completed
    SegmentCompleted {
        session_id: String,
        segment_id: String,
        output_path: PathBuf,
        message_count: u64,
    },
    /// Connection lost and reconnecting
    Reconnecting { session_id: String, attempt: u32 },
    /// Reconnection failed
    ReconnectFailed { session_id: String, error: String },
    /// Error during collection
    Error { session_id: String, error: String },
}

/// Commands sent to the collection task.
#[derive(Debug)]
enum CollectionCommand {
    /// Start a new segment file
    StartSegment {
        segment_id: String,
        output_path: PathBuf,
    },
    /// End the current segment file
    EndSegment { segment_id: String },
    /// Stop collection entirely
    Stop,
}

/// Configuration for the danmu service.
#[derive(Debug, Clone)]
pub struct DanmuServiceConfig {
    /// Output directory for danmu XML files
    pub output_dir: PathBuf,
    /// Default sampling configuration
    pub default_sampling: DanmuSamplingConfig,
    /// Maximum reconnect attempts
    pub max_reconnect_attempts: u32,
    /// Initial reconnect delay in milliseconds
    pub initial_reconnect_delay_ms: u64,
    /// Maximum reconnect delay in milliseconds
    pub max_reconnect_delay_ms: u64,
    /// Buffer size for statistics (number of recent messages to keep)
    pub stats_buffer_size: usize,
}

impl Default for DanmuServiceConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./danmu"),
            default_sampling: DanmuSamplingConfig::default(),
            max_reconnect_attempts: 10,
            initial_reconnect_delay_ms: 1000,
            max_reconnect_delay_ms: 60000,
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
    pub async fn start_segment(&self, segment_id: &str, output_path: PathBuf) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::StartSegment {
                segment_id: segment_id.to_string(),
                output_path,
            })
            .await
            .map_err(|_| Error::DanmuError("Collection task not running".to_string()))
    }

    /// End the current segment file (finalize XML).
    pub async fn end_segment(&self, segment_id: &str) -> Result<()> {
        self.command_tx
            .send(CollectionCommand::EndSegment {
                segment_id: segment_id.to_string(),
            })
            .await
            .map_err(|_| Error::DanmuError("Collection task not running".to_string()))
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
    /// Platform
    platform: String,
    /// Room ID
    room_id: String,
    /// Statistics aggregator (session-level)
    stats: Arc<Mutex<StatisticsAggregator>>,
    /// Sampler
    sampler: Arc<Mutex<Box<dyn DanmuSampler>>>,
    /// Cancellation token for this collection
    cancel_token: CancellationToken,
    /// Start time
    start_time: DateTime<Utc>,
    /// Command sender
    command_tx: mpsc::Sender<CollectionCommand>,
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
        }
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
    ) -> Result<CollectionHandle> {
        // Check if already collecting
        if self.collections.contains_key(session_id) {
            return Err(Error::DanmuError(format!(
                "Collection already active for session {}",
                session_id
            )));
        }

        // Find provider for URL
        let provider = self.providers.get_by_url(streamer_url).ok_or_else(|| {
            Error::DanmuError(format!("No danmu provider for URL: {}", streamer_url))
        })?;

        // Extract room ID
        let room_id = provider.extract_room_id(streamer_url).ok_or_else(|| {
            Error::DanmuError(format!(
                "Could not extract room ID from URL: {}",
                streamer_url
            ))
        })?;

        // Create command channel
        let (command_tx, command_rx) = mpsc::channel(32);

        // Create state
        let sampling = sampling_config.unwrap_or_else(|| self.config.default_sampling.clone());
        let sampler_config = to_sampler_config(&sampling);
        let stats = Arc::new(Mutex::new(StatisticsAggregator::new()));
        let sampler: Arc<Mutex<Box<dyn DanmuSampler>>> =
            Arc::new(Mutex::new(create_sampler(&sampler_config)));
        let cancel_token = self.cancel_token.child_token();

        let state = CollectionState {
            streamer_id: streamer_id.to_string(),
            platform: provider.platform().to_string(),
            room_id: room_id.clone(),
            stats: Arc::clone(&stats),
            sampler: Arc::clone(&sampler),
            cancel_token: cancel_token.clone(),
            start_time: Utc::now(),
            command_tx: command_tx.clone(),
        };

        // Store state
        self.collections.insert(session_id.to_string(), state);

        // Emit event
        let _ = self.event_tx.send(DanmuEvent::CollectionStarted {
            session_id: session_id.to_string(),
            streamer_id: streamer_id.to_string(),
        });

        // Start collection task
        let session_id_clone = session_id.to_string();
        let event_tx = self.event_tx.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            if let Err(e) = run_collection(
                &session_id_clone,
                provider,
                &room_id,
                stats,
                sampler,
                command_rx,
                &event_tx,
                &config,
                cancel_token,
            )
            .await
            {
                let _ = event_tx.send(DanmuEvent::Error {
                    session_id: session_id_clone.clone(),
                    error: e.to_string(),
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
            Error::DanmuError(format!("No active collection for session {}", session_id))
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

/// Run the collection loop for a session.
#[allow(clippy::too_many_arguments)]
async fn run_collection(
    session_id: &str,
    provider: Arc<dyn DanmuProvider>,
    room_id: &str,
    stats: Arc<Mutex<StatisticsAggregator>>,
    sampler: Arc<Mutex<Box<dyn DanmuSampler>>>,
    mut command_rx: mpsc::Receiver<CollectionCommand>,
    event_tx: &broadcast::Sender<DanmuEvent>,
    config: &DanmuServiceConfig,
    cancel_token: CancellationToken,
) -> Result<()> {
    // Current segment writer (None if no segment is active)
    let mut current_writer: Option<(String, XmlDanmuWriter)> = None;

    // Connect to danmu stream
    let mut connection = provider.connect(room_id).await?;
    let mut reconnect_attempts = 0;
    let mut reconnect_delay = config.initial_reconnect_delay_ms;

    loop {
        tokio::select! {
            biased;

            // Handle commands (highest priority)
            cmd = command_rx.recv() => {
                match cmd {
                    Some(CollectionCommand::StartSegment { segment_id, output_path }) => {
                        // Finalize previous segment if any
                        if let Some((old_id, mut writer)) = current_writer.take() {
                            let count = writer.message_count;
                            let path = writer.output_path();
                            writer.finalize().await?;
                            let _ = event_tx.send(DanmuEvent::SegmentCompleted {
                                session_id: session_id.to_string(),
                                segment_id: old_id,
                                output_path: path,
                                message_count: count,
                            });
                        }

                        // Create output directory if needed
                        if let Some(parent) = output_path.parent() {
                            tokio::fs::create_dir_all(parent).await?;
                        }

                        // Start new segment
                        let writer = XmlDanmuWriter::new(&output_path).await?;
                        let _ = event_tx.send(DanmuEvent::SegmentStarted {
                            session_id: session_id.to_string(),
                            segment_id: segment_id.clone(),
                            output_path: output_path.clone(),
                        });
                        current_writer = Some((segment_id, writer));
                    }
                    Some(CollectionCommand::EndSegment { segment_id }) => {
                        // Finalize segment if it matches
                        if let Some((current_id, mut writer)) = current_writer.take() {
                            if current_id == segment_id {
                                let count = writer.message_count;
                                let path = writer.output_path();
                                writer.finalize().await?;
                                let _ = event_tx.send(DanmuEvent::SegmentCompleted {
                                    session_id: session_id.to_string(),
                                    segment_id,
                                    output_path: path,
                                    message_count: count,
                                });
                            } else {
                                // Put it back if segment_id doesn't match
                                current_writer = Some((current_id, writer));
                            }
                        }
                    }
                    Some(CollectionCommand::Stop) | None => {
                        // Finalize current segment and exit
                        if let Some((segment_id, mut writer)) = current_writer.take() {
                            let count = writer.message_count;
                            let path = writer.output_path();
                            writer.finalize().await?;
                            let _ = event_tx.send(DanmuEvent::SegmentCompleted {
                                session_id: session_id.to_string(),
                                segment_id,
                                output_path: path,
                                message_count: count,
                            });
                        }
                        let _ = provider.disconnect(&mut connection).await;
                        break;
                    }
                }
            }

            // Handle cancellation
            _ = cancel_token.cancelled() => {
                // Finalize current segment and exit
                if let Some((segment_id, mut writer)) = current_writer.take() {
                    let count = writer.message_count;
                    let path = writer.output_path();
                    writer.finalize().await?;
                    let _ = event_tx.send(DanmuEvent::SegmentCompleted {
                        session_id: session_id.to_string(),
                        segment_id,
                        output_path: path,
                        message_count: count,
                    });
                }
                let _ = provider.disconnect(&mut connection).await;
                break;
            }

            // Receive danmu messages
            result = provider.receive(&connection) => {
                match result {
                    Ok(Some(message)) => {
                        // Reset reconnect state on successful receive
                        reconnect_attempts = 0;
                        reconnect_delay = config.initial_reconnect_delay_ms;

                        // Update session-level statistics
                        {
                            let is_gift = matches!(message.message_type, DanmuType::Gift | DanmuType::SuperChat);
                            let mut stats_guard = stats.lock().await;
                            stats_guard.record_message(
                                &message.user_id,
                                &message.username,
                                &message.content,
                                is_gift,
                                message.timestamp,
                            );
                        }

                        // Update sampler
                        {
                            let mut sampler_guard = sampler.lock().await;
                            sampler_guard.record_message(message.timestamp);
                        }

                        // Write to current segment file (if any)
                        if let Some((_, ref mut writer)) = current_writer {
                            writer.write_message(&message).await?;
                        }
                    }
                    Ok(None) => {
                        // No message available, continue
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    Err(e) => {
                        // Connection error, attempt reconnect
                        tracing::warn!("Danmu connection error for {}: {}", session_id, e);

                        if reconnect_attempts >= config.max_reconnect_attempts {
                            let _ = event_tx.send(DanmuEvent::ReconnectFailed {
                                session_id: session_id.to_string(),
                                error: format!("Max reconnect attempts ({}) exceeded", config.max_reconnect_attempts),
                            });
                            return Err(e);
                        }

                        reconnect_attempts += 1;
                        let _ = event_tx.send(DanmuEvent::Reconnecting {
                            session_id: session_id.to_string(),
                            attempt: reconnect_attempts,
                        });

                        // Exponential backoff
                        tokio::time::sleep(tokio::time::Duration::from_millis(reconnect_delay)).await;
                        reconnect_delay = (reconnect_delay * 2).min(config.max_reconnect_delay_ms);

                        // Attempt reconnect
                        match provider.connect(room_id).await {
                            Ok(new_conn) => {
                                connection = new_conn;
                            }
                            Err(e) => {
                                tracing::error!("Reconnect failed for {}: {}", session_id, e);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// XML writer for danmu messages.
struct XmlDanmuWriter {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    message_count: u64,
}

impl XmlDanmuWriter {
    async fn new(path: &PathBuf) -> Result<Self> {
        let file = tokio::fs::File::create(path).await?;
        let mut writer = Self {
            path: path.clone(),
            file: Some(file),
            message_count: 0,
        };

        // Write XML header
        writer.write_header().await?;

        Ok(writer)
    }

    fn output_path(&self) -> PathBuf {
        self.path.clone()
    }

    async fn write_header(&mut self) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if let Some(file) = &mut self.file {
            file.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
                .await?;
            file.write_all(b"<danmu>\n").await?;
        }
        Ok(())
    }

    async fn write_message(&mut self, message: &DanmuMessage) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if let Some(file) = &mut self.file {
            let xml = format!(
                "  <d p=\"{},{},{},{}\">{}</d>\n",
                message.timestamp.timestamp_millis(),
                message_type_to_int(&message.message_type),
                escape_xml(&message.user_id),
                escape_xml(&message.username),
                escape_xml(&message.content),
            );
            file.write_all(xml.as_bytes()).await?;
            self.message_count += 1;

            // Flush periodically
            if self.message_count % 100 == 0 {
                file.flush().await?;
            }
        }
        Ok(())
    }

    async fn finalize(&mut self) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        if let Some(file) = &mut self.file {
            file.write_all(b"</danmu>\n").await?;
            file.flush().await?;
        }
        self.file = None;
        Ok(())
    }
}

fn message_type_to_int(msg_type: &DanmuType) -> u8 {
    match msg_type {
        DanmuType::Chat => 1,
        DanmuType::Gift => 2,
        DanmuType::SuperChat => 3,
        DanmuType::System => 4,
        DanmuType::UserJoin => 5,
        DanmuType::Follow => 6,
        DanmuType::Subscription => 7,
        DanmuType::Other => 0,
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("hello"), "hello");
        assert_eq!(escape_xml("<script>"), "&lt;script&gt;");
        assert_eq!(escape_xml("a & b"), "a &amp; b");
        assert_eq!(escape_xml("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_message_type_to_int() {
        assert_eq!(message_type_to_int(&DanmuType::Chat), 1);
        assert_eq!(message_type_to_int(&DanmuType::Gift), 2);
        assert_eq!(message_type_to_int(&DanmuType::SuperChat), 3);
    }

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
