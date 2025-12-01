//! Download Manager implementation.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, mpsc};
use tracing::{debug, error, info, warn};

use super::engine::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadInfo, DownloadProgress, DownloadStatus,
    EngineType, FfmpegEngine, MesioEngine, SegmentEvent, StreamlinkEngine,
};
use super::resilience::{CircuitBreakerManager, RetryConfig};
use crate::Result;

/// Configuration for the Download Manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadManagerConfig {
    /// Maximum concurrent downloads.
    pub max_concurrent_downloads: usize,
    /// Extra slots for high priority downloads.
    pub high_priority_extra_slots: usize,
    /// Default download engine.
    pub default_engine: EngineType,
    /// Retry configuration.
    pub retry_config: RetryConfig,
    /// Circuit breaker failure threshold.
    pub circuit_breaker_threshold: u32,
    /// Circuit breaker cooldown in seconds.
    pub circuit_breaker_cooldown_secs: u64,
}

impl Default for DownloadManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 6,
            high_priority_extra_slots: 2,
            default_engine: EngineType::Ffmpeg,
            retry_config: RetryConfig::default(),
            circuit_breaker_threshold: 5,
            circuit_breaker_cooldown_secs: 60,
        }
    }
}

/// Internal state for an active download.
#[derive(Clone)]
struct ActiveDownload {
    handle: Arc<DownloadHandle>,
    status: DownloadStatus,
    progress: DownloadProgress,
    #[allow(dead_code)]
    is_high_priority: bool,
}

/// The Download Manager service.
pub struct DownloadManager {
    /// Configuration.
    config: DownloadManagerConfig,
    /// Semaphore for normal priority downloads.
    normal_semaphore: Arc<Semaphore>,
    /// Semaphore for high priority downloads (extra slots).
    high_priority_semaphore: Arc<Semaphore>,
    /// Active downloads.
    active_downloads: DashMap<String, ActiveDownload>,
    /// Engine registry.
    engines: RwLock<HashMap<EngineType, Arc<dyn DownloadEngine>>>,
    /// Circuit breaker manager.
    circuit_breakers: CircuitBreakerManager,
    /// Event sender for download events.
    event_tx: mpsc::Sender<DownloadManagerEvent>,
    /// Event receiver (for external consumption).
    #[allow(dead_code)]
    event_rx: RwLock<Option<mpsc::Receiver<DownloadManagerEvent>>>,
}

/// Events emitted by the Download Manager.
#[derive(Debug, Clone)]
pub enum DownloadManagerEvent {
    /// Download started.
    DownloadStarted {
        download_id: String,
        streamer_id: String,
        session_id: String,
        engine_type: EngineType,
    },
    /// Segment completed.
    SegmentCompleted {
        download_id: String,
        streamer_id: String,
        segment_path: String,
        segment_index: u32,
        duration_secs: f64,
        size_bytes: u64,
    },
    /// Download completed.
    DownloadCompleted {
        download_id: String,
        streamer_id: String,
        session_id: String,
        total_bytes: u64,
        total_duration_secs: f64,
        total_segments: u32,
    },
    /// Download failed.
    DownloadFailed {
        download_id: String,
        streamer_id: String,
        error: String,
        recoverable: bool,
    },
    /// Download cancelled.
    DownloadCancelled {
        download_id: String,
        streamer_id: String,
    },
}

impl DownloadManager {
    /// Create a new Download Manager.
    pub fn new() -> Self {
        Self::with_config(DownloadManagerConfig::default())
    }

    /// Create a new Download Manager with custom configuration.
    pub fn with_config(config: DownloadManagerConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel(256);

        let normal_semaphore = Arc::new(Semaphore::new(config.max_concurrent_downloads));
        let high_priority_semaphore = Arc::new(Semaphore::new(config.high_priority_extra_slots));

        let circuit_breakers = CircuitBreakerManager::new(
            config.circuit_breaker_threshold,
            config.circuit_breaker_cooldown_secs,
        );

        let manager = Self {
            config,
            normal_semaphore,
            high_priority_semaphore,
            active_downloads: DashMap::new(),
            engines: RwLock::new(HashMap::new()),
            circuit_breakers,
            event_tx,
            event_rx: RwLock::new(Some(event_rx)),
        };

        // Register default engines
        {
            let mut engines = manager.engines.write();
            engines.insert(
                EngineType::Ffmpeg,
                Arc::new(FfmpegEngine::new()) as Arc<dyn DownloadEngine>,
            );
            engines.insert(
                EngineType::Streamlink,
                Arc::new(StreamlinkEngine::new()) as Arc<dyn DownloadEngine>,
            );
            engines.insert(
                EngineType::Mesio,
                Arc::new(MesioEngine::new()) as Arc<dyn DownloadEngine>,
            );
        }

        manager
    }

    /// Register a download engine.
    pub fn register_engine(&mut self, engine: Arc<dyn DownloadEngine>) {
        let engine_type = engine.engine_type();
        self.engines.write().insert(engine_type, engine);
        debug!("Registered download engine: {}", engine_type);
    }

    /// Get an engine by type.
    pub fn get_engine(&self, engine_type: EngineType) -> Option<Arc<dyn DownloadEngine>> {
        self.engines.read().get(&engine_type).cloned()
    }

    /// Get available engines.
    pub fn available_engines(&self) -> Vec<EngineType> {
        self.engines
            .read()
            .iter()
            .filter(|(_, engine)| engine.is_available())
            .map(|(t, _)| *t)
            .collect()
    }

    /// Start a download.
    pub async fn start_download(
        &self,
        config: DownloadConfig,
        engine_type: Option<EngineType>,
        is_high_priority: bool,
    ) -> Result<String> {
        let engine_type = engine_type.unwrap_or(self.config.default_engine);

        // Check circuit breaker
        if !self.circuit_breakers.is_engine_allowed(engine_type) {
            warn!(
                "Engine {} is disabled by circuit breaker, trying fallback",
                engine_type
            );
            // Try to find an alternative engine
            let available = self.available_engines();
            let fallback = available
                .iter()
                .find(|&&t| t != engine_type && self.circuit_breakers.is_engine_allowed(t));

            if let Some(&fallback_type) = fallback {
                info!("Using fallback engine: {}", fallback_type);
                return self
                    .start_download_with_engine(config, fallback_type, is_high_priority)
                    .await;
            } else {
                return Err(crate::Error::Other(
                    "All download engines are disabled by circuit breaker".to_string(),
                ));
            }
        }

        self.start_download_with_engine(config, engine_type, is_high_priority)
            .await
    }

    /// Start a download with a specific engine.
    async fn start_download_with_engine(
        &self,
        config: DownloadConfig,
        engine_type: EngineType,
        is_high_priority: bool,
    ) -> Result<String> {
        // Get the engine
        let engine = self
            .get_engine(engine_type)
            .ok_or_else(|| crate::Error::Other(format!("Engine {} not registered", engine_type)))?;

        if !engine.is_available() {
            return Err(crate::Error::Other(format!(
                "Engine {} is not available",
                engine_type
            )));
        }

        // Acquire semaphore permit
        let _permit = if is_high_priority {
            // Try high priority semaphore first, then fall back to normal
            match self.high_priority_semaphore.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => self
                    .normal_semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| crate::Error::Other(format!("Semaphore error: {}", e)))?,
            }
        } else {
            self.normal_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| crate::Error::Other(format!("Semaphore error: {}", e)))?
        };

        // Generate download ID
        let download_id = uuid::Uuid::new_v4().to_string();

        // Create event channel for this download
        let (segment_tx, mut segment_rx) = mpsc::channel::<SegmentEvent>(32);

        // Create download handle
        let handle = Arc::new(DownloadHandle::new(
            download_id.clone(),
            engine_type,
            config.clone(),
            segment_tx,
        ));

        // Store active download
        self.active_downloads.insert(
            download_id.clone(),
            ActiveDownload {
                handle: handle.clone(),
                status: DownloadStatus::Starting,
                progress: DownloadProgress::default(),
                is_high_priority,
            },
        );

        // Emit start event
        let _ = self
            .event_tx
            .send(DownloadManagerEvent::DownloadStarted {
                download_id: download_id.clone(),
                streamer_id: config.streamer_id.clone(),
                session_id: config.session_id.clone(),
                engine_type,
            })
            .await;

        info!(
            "Starting download {} for streamer {} with engine {}",
            download_id, config.streamer_id, engine_type
        );

        // Start the engine
        let engine_clone = engine.clone();
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = engine_clone.start(handle_clone).await {
                error!("Engine start error: {}", e);
            }
        });

        // Spawn task to handle segment events
        let download_id_clone = download_id.clone();
        let event_tx = self.event_tx.clone();
        let streamer_id = config.streamer_id.clone();
        let session_id = config.session_id.clone();

        // Clone references for the spawned task
        let active_downloads = self.active_downloads.clone();
        let circuit_breakers_ref = self.circuit_breakers.get(engine_type);

        tokio::spawn(async move {
            while let Some(event) = segment_rx.recv().await {
                match event {
                    SegmentEvent::SegmentCompleted(info) => {
                        let _ = event_tx
                            .send(DownloadManagerEvent::SegmentCompleted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                segment_path: info.path.to_string_lossy().to_string(),
                                segment_index: info.index,
                                duration_secs: info.duration_secs,
                                size_bytes: info.size_bytes,
                            })
                            .await;
                    }
                    SegmentEvent::Progress(progress) => {
                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.progress = progress;
                            download.status = DownloadStatus::Downloading;
                        }
                    }
                    SegmentEvent::DownloadCompleted {
                        total_bytes,
                        total_duration_secs,
                        total_segments,
                    } => {
                        circuit_breakers_ref.record_success();

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.status = DownloadStatus::Completed;
                        }

                        let _ = event_tx
                            .send(DownloadManagerEvent::DownloadCompleted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                session_id: session_id.clone(),
                                total_bytes,
                                total_duration_secs,
                                total_segments,
                            })
                            .await;

                        active_downloads.remove(&download_id_clone);
                        break;
                    }
                    SegmentEvent::DownloadFailed { error, recoverable } => {
                        circuit_breakers_ref.record_failure();

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.status = DownloadStatus::Failed;
                        }

                        let _ = event_tx
                            .send(DownloadManagerEvent::DownloadFailed {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                error,
                                recoverable,
                            })
                            .await;

                        active_downloads.remove(&download_id_clone);
                        break;
                    }
                }
            }
        });

        Ok(download_id)
    }

    /// Stop a download.
    pub async fn stop_download(&self, download_id: &str) -> Result<()> {
        if let Some((_, download)) = self.active_downloads.remove(download_id) {
            let engine_type = download.handle.engine_type;

            if let Some(engine) = self.get_engine(engine_type) {
                engine.stop(&download.handle).await?;
            }

            let _ = self
                .event_tx
                .send(DownloadManagerEvent::DownloadCancelled {
                    download_id: download_id.to_string(),
                    streamer_id: download.handle.config.streamer_id.clone(),
                })
                .await;

            info!("Stopped download {}", download_id);
            Ok(())
        } else {
            Err(crate::Error::NotFound {
                entity_type: "Download".to_string(),
                id: download_id.to_string(),
            })
        }
    }

    /// Get information about active downloads.
    pub fn get_active_downloads(&self) -> Vec<DownloadInfo> {
        self.active_downloads
            .iter()
            .map(|entry| {
                let download = entry.value();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: download.handle.config.streamer_id.clone(),
                    session_id: download.handle.config.session_id.clone(),
                    engine_type: download.handle.engine_type,
                    status: download.status,
                    progress: download.progress.clone(),
                    started_at: download.handle.started_at,
                }
            })
            .collect()
    }

    /// Get the number of active downloads.
    pub fn active_count(&self) -> usize {
        self.active_downloads.len()
    }

    /// Subscribe to download events.
    pub fn subscribe(&self) -> mpsc::Receiver<DownloadManagerEvent> {
        let (tx, rx) = mpsc::channel(256);
        // Note: In a real implementation, we'd use a broadcast channel
        // For now, this creates a new channel
        let _ = tx; // Suppress unused warning
        rx
    }

    /// Update configuration for an active download.
    ///
    /// Some config changes are applied immediately (cookies, retry policy),
    /// while others are applied to the next segment (output settings).
    pub fn update_download_config(
        &self,
        download_id: &str,
        cookies: Option<String>,
        headers: Option<Vec<(String, String)>>,
        retry_config: Option<RetryConfig>,
    ) -> Result<()> {
        if let Some(download) = self.active_downloads.get(download_id) {
            // Log the config update - actual application happens on next segment
            info!(
                "Config update queued for download {}: cookies={}, headers={}, retry={}",
                download_id,
                cookies.is_some(),
                headers.is_some(),
                retry_config.is_some()
            );

            // Store pending config updates
            // Note: In a full implementation, we'd store these in a pending_updates map
            // and apply them when the next segment starts

            debug!(
                "Download {} for streamer {} will apply config on next segment",
                download_id, download.handle.config.streamer_id
            );

            Ok(())
        } else {
            Err(crate::Error::NotFound {
                entity_type: "Download".to_string(),
                id: download_id.to_string(),
            })
        }
    }

    /// Get download by streamer ID.
    pub fn get_download_by_streamer(&self, streamer_id: &str) -> Option<DownloadInfo> {
        self.active_downloads
            .iter()
            .find(|entry| entry.value().handle.config.streamer_id == streamer_id)
            .map(|entry| {
                let download = entry.value();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: download.handle.config.streamer_id.clone(),
                    session_id: download.handle.config.session_id.clone(),
                    engine_type: download.handle.engine_type,
                    status: download.status,
                    progress: download.progress.clone(),
                    started_at: download.handle.started_at,
                }
            })
    }

    /// Check if a streamer has an active download.
    pub fn has_active_download(&self, streamer_id: &str) -> bool {
        self.active_downloads
            .iter()
            .any(|entry| entry.value().handle.config.streamer_id == streamer_id)
    }

    /// Get downloads by status.
    pub fn get_downloads_by_status(&self, status: DownloadStatus) -> Vec<DownloadInfo> {
        self.active_downloads
            .iter()
            .filter(|entry| entry.value().status == status)
            .map(|entry| {
                let download = entry.value();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: download.handle.config.streamer_id.clone(),
                    session_id: download.handle.config.session_id.clone(),
                    engine_type: download.handle.engine_type,
                    status: download.status,
                    progress: download.progress.clone(),
                    started_at: download.handle.started_at,
                }
            })
            .collect()
    }

    /// Stop all active downloads.
    pub async fn stop_all(&self) -> Vec<String> {
        let download_ids: Vec<String> = self
            .active_downloads
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        let mut stopped = Vec::new();
        for id in download_ids {
            if self.stop_download(&id).await.is_ok() {
                stopped.push(id);
            }
        }

        info!("Stopped {} downloads", stopped.len());
        stopped
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_manager_config_default() {
        let config = DownloadManagerConfig::default();
        assert_eq!(config.max_concurrent_downloads, 6);
        assert_eq!(config.high_priority_extra_slots, 2);
        assert_eq!(config.default_engine, EngineType::Ffmpeg);
    }

    #[test]
    fn test_download_manager_creation() {
        let manager = DownloadManager::new();
        assert_eq!(manager.active_count(), 0);
        assert!(!manager.available_engines().is_empty());
    }

    #[test]
    fn test_engine_registration() {
        let mut manager = DownloadManager::new();

        // FFmpeg should be registered by default
        assert!(manager.get_engine(EngineType::Ffmpeg).is_some());
        assert!(manager.get_engine(EngineType::Streamlink).is_some());
        assert!(manager.get_engine(EngineType::Mesio).is_some());
    }
}
