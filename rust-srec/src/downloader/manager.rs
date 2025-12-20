//! Download Manager implementation.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, mpsc};
use tracing::{debug, error, info, warn};

use super::engine::{
    DownloadConfig, DownloadEngine, DownloadHandle, DownloadInfo, DownloadProgress, DownloadStatus,
    EngineType, FfmpegEngine, MesioEngine, SegmentEvent, StreamlinkEngine,
};
use super::resilience::{CircuitBreakerManager, EngineKey, RetryConfig};
use crate::Result;
use crate::database::models::engine::{
    FfmpegEngineConfig, MesioEngineConfig, StreamlinkEngineConfig,
};
use crate::database::repositories::config::ConfigRepository;
use crate::downloader::SegmentInfo;

/// Pending configuration update for an active download.
///
/// Stores configuration changes that will be applied when the next segment starts.
/// Multiple updates can be merged, with newer values overwriting older ones.
#[derive(Debug, Clone, Default)]
pub struct PendingConfigUpdate {
    /// Updated cookies (if any).
    pub cookies: Option<String>,
    /// Updated headers (if any).
    pub headers: Option<Vec<(String, String)>>,
    /// Updated retry configuration (if any).
    pub retry_config: Option<RetryConfig>,
    /// Timestamp when the update was queued.
    pub queued_at: DateTime<Utc>,
}

impl PendingConfigUpdate {
    /// Create a new pending config update with the current timestamp.
    pub fn new(
        cookies: Option<String>,
        headers: Option<Vec<(String, String)>>,
        retry_config: Option<RetryConfig>,
    ) -> Self {
        Self {
            cookies,
            headers,
            retry_config,
            queued_at: Utc::now(),
        }
    }

    /// Check if there are any pending updates.
    pub fn has_updates(&self) -> bool {
        self.cookies.is_some() || self.headers.is_some() || self.retry_config.is_some()
    }

    /// Merge another update into this one (newer values overwrite).
    pub fn merge(&mut self, other: PendingConfigUpdate) {
        if other.cookies.is_some() {
            self.cookies = other.cookies;
        }
        if other.headers.is_some() {
            self.headers = other.headers;
        }
        if other.retry_config.is_some() {
            self.retry_config = other.retry_config;
        }
        self.queued_at = other.queued_at;
    }
}

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
struct ActiveDownload {
    handle: Arc<DownloadHandle>,
    status: DownloadStatus,
    progress: DownloadProgress,
    #[allow(dead_code)]
    is_high_priority: bool,
    /// Last known output path (from segments)
    pub output_path: Option<String>,
    /// Semaphore permit guarding concurrency slot (dropped on removal)
    #[allow(dead_code)]
    permit: Option<OwnedSemaphorePermit>,
    /// Retry configuration override applied via config update.
    retry_config_override: Option<RetryConfig>,
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
    active_downloads: Arc<DashMap<String, ActiveDownload>>,
    /// Pending configuration updates keyed by download_id.
    pending_updates: Arc<DashMap<String, PendingConfigUpdate>>,
    /// Engine registry.
    engines: RwLock<HashMap<EngineType, Arc<dyn DownloadEngine>>>,
    /// Circuit breaker manager.
    circuit_breakers: CircuitBreakerManager,
    /// Broadcast sender for download events
    event_tx: broadcast::Sender<DownloadManagerEvent>,
    /// Config repository for resolving custom engines.
    config_repo: Option<Arc<dyn ConfigRepository>>,
}

/// Type of configuration that was updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigUpdateType {
    /// Only cookies were updated.
    Cookies,
    /// Only headers were updated.
    Headers,
    /// Only retry configuration was updated.
    RetryConfig,
    /// Multiple configuration types were updated.
    Multiple,
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
    /// Progress update for a download.
    Progress {
        download_id: String,
        streamer_id: String,
        session_id: String,
        progress: DownloadProgress,
    },
    /// Segment started - a new segment file has been opened for writing.
    SegmentStarted {
        download_id: String,
        streamer_id: String,
        session_id: String,
        segment_path: String,
        segment_index: u32,
    },
    /// Segment completed.
    SegmentCompleted {
        download_id: String,
        streamer_id: String,
        session_id: String,
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
        file_path: Option<String>,
    },
    /// Download failed.
    DownloadFailed {
        download_id: String,
        streamer_id: String,
        session_id: String,
        error: String,
        recoverable: bool,
    },
    /// Download cancelled.
    DownloadCancelled {
        download_id: String,
        streamer_id: String,
        session_id: String,
    },
    /// Configuration was updated for a download.
    ConfigUpdated {
        download_id: String,
        streamer_id: String,
        update_type: ConfigUpdateType,
    },
    /// Configuration update failed to apply.
    ConfigUpdateFailed {
        download_id: String,
        streamer_id: String,
        error: String,
    },
    /// Download was rejected before starting (e.g., circuit breaker open).
    ///
    /// Unlike DownloadFailed, this indicates the download never started.
    /// No download_id is available because the download was never created.
    DownloadRejected {
        streamer_id: String,
        session_id: String,
        reason: String,
        /// How long to wait before retrying (circuit breaker cooldown).
        retry_after_secs: Option<u64>,
    },
}

impl DownloadManager {
    /// Create a new Download Manager.
    pub fn new() -> Self {
        Self::with_config(DownloadManagerConfig::default())
    }

    /// Create a new Download Manager with custom configuration.
    pub fn with_config(config: DownloadManagerConfig) -> Self {
        // Use broadcast channel to support multiple subscribers
        let (event_tx, _) = broadcast::channel(256);

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
            active_downloads: Arc::new(DashMap::new()),
            pending_updates: Arc::new(DashMap::new()),
            engines: RwLock::new(HashMap::new()),
            circuit_breakers,
            event_tx,
            config_repo: None,
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

    /// Set the config repository.
    pub fn with_config_repo(mut self, config_repo: Arc<dyn ConfigRepository>) -> Self {
        self.config_repo = Some(config_repo);
        self
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
        engine_id: Option<String>,
        is_high_priority: bool,
    ) -> Result<String> {
        let overrides = config.engines_override.as_ref();
        let (engine, engine_type, engine_key) =
            self.resolve_engine(engine_id.as_deref(), overrides).await?;

        // Check circuit breaker using the specific engine key
        if !self.circuit_breakers.is_allowed(&engine_key) {
            warn!(
                "Engine {} is disabled by circuit breaker, trying fallback",
                engine_key
            );

            // Emit rejection event for visibility
            let _ = self.event_tx.send(DownloadManagerEvent::DownloadRejected {
                streamer_id: config.streamer_id.clone(),
                session_id: config.session_id.clone(),
                reason: format!("Circuit breaker open for engine {}", engine_key),
                retry_after_secs: Some(self.config.circuit_breaker_cooldown_secs),
            });

            // Try to find an alternative engine
            // For now, fallback to default ffmpeg if validation fails
            // TODO: Implement smarter fallback
            return Err(crate::Error::Other(format!(
                "Engine {} is disabled by circuit breaker",
                engine_key
            )));
        }

        self.start_download_with_engine(config, engine, engine_type, engine_key, is_high_priority)
            .await
    }

    /// Resolve engine from ID string.
    ///
    /// Returns (Engine instance, EngineType).
    /// Resolve engine to use.
    ///
    /// If an override value is provided for the resolved engine ID (either passed ID or global default),
    /// a new engine instance is created with the merged configuration.
    /// Otherwise, the shared cached engine instance is returned.
    async fn resolve_engine(
        &self,
        engine_id: Option<&str>,
        overrides: Option<&serde_json::Value>,
    ) -> Result<(Arc<dyn DownloadEngine>, EngineType, EngineKey)> {
        // Determine which engine ID we are using
        let target_id = if let Some(id) = engine_id {
            id
        } else {
            // Fallback to default engine type string
            self.config.default_engine.as_str()
        };

        // 1. Check for overrides first
        let specific_override = overrides.and_then(|o| o.get(target_id));

        // Compute override hash for circuit breaker key (if needed)
        let override_hash = specific_override.map(|v| {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            v.to_string().hash(&mut hasher);
            hasher.finish()
        });

        // If we have an override, we MUST create a new engine instance
        // We cannot reuse the shared engine because it has different config
        if let Some(override_config) = specific_override {
            debug!("Applying engine override for {}", target_id);

            // Need to know the type first
            // Try to parse ID as type, or look up in DB to get type
            let engine_type = if let Ok(t) = target_id.parse::<EngineType>() {
                t
            } else if let Some(repo) = &self.config_repo {
                if let Ok(config) = repo.get_engine_config(target_id).await {
                    config.engine_type.parse::<EngineType>().map_err(|_| {
                        crate::Error::Other(format!("Unknown engine type: {}", config.engine_type))
                    })?
                } else {
                    // Config not found, but we have override?
                    // Fallback to default? Or error?
                    // If ID was not a type and not in DB, we can't do much.
                    return Err(crate::Error::Other(format!(
                        "Unknown engine: {}",
                        target_id
                    )));
                }
            } else {
                return Err(crate::Error::Other(format!(
                    "Unknown engine: {}",
                    target_id
                )));
            };

            // Now we have the type. We need the BASE config to merge with.
            // If ID was a known type (e.g. "ffmpeg"), base config is Default::default().
            // If ID was a DB config, base config is from DB.

            match engine_type {
                EngineType::Ffmpeg => {
                    let mut base_config = if let Some(repo) = &self.config_repo {
                        if let Ok(c) = repo.get_engine_config(target_id).await {
                            serde_json::from_str::<FfmpegEngineConfig>(&c.config)
                                .unwrap_or_default()
                        } else {
                            FfmpegEngineConfig::default()
                        }
                    } else {
                        FfmpegEngineConfig::default()
                    };

                    // Merge override
                    if let Ok(merged) = Self::merge_config_json(&base_config, override_config) {
                        base_config = serde_json::from_value(merged).unwrap_or(base_config);
                    }

                    let key = EngineKey::with_override(
                        EngineType::Ffmpeg,
                        engine_id,
                        override_hash.unwrap(),
                    );
                    return Ok((
                        Arc::new(FfmpegEngine::with_config(base_config)),
                        EngineType::Ffmpeg,
                        key,
                    ));
                }
                EngineType::Streamlink => {
                    let mut base_config = if let Some(repo) = &self.config_repo {
                        if let Ok(c) = repo.get_engine_config(target_id).await {
                            serde_json::from_str::<StreamlinkEngineConfig>(&c.config)
                                .unwrap_or_default()
                        } else {
                            StreamlinkEngineConfig::default()
                        }
                    } else {
                        StreamlinkEngineConfig::default()
                    };

                    // Merge override
                    if let Ok(merged) = Self::merge_config_json(&base_config, override_config) {
                        base_config = serde_json::from_value(merged).unwrap_or(base_config);
                    }

                    let key = EngineKey::with_override(
                        EngineType::Streamlink,
                        engine_id,
                        override_hash.unwrap(),
                    );
                    return Ok((
                        Arc::new(StreamlinkEngine::with_config(base_config)),
                        EngineType::Streamlink,
                        key,
                    ));
                }
                EngineType::Mesio => {
                    let mut base_config = if let Some(repo) = &self.config_repo {
                        if let Ok(c) = repo.get_engine_config(target_id).await {
                            serde_json::from_str::<MesioEngineConfig>(&c.config).unwrap_or_default()
                        } else {
                            MesioEngineConfig::default()
                        }
                    } else {
                        MesioEngineConfig::default()
                    };

                    // Merge override
                    if let Ok(merged) = Self::merge_config_json(&base_config, override_config) {
                        base_config = serde_json::from_value(merged).unwrap_or(base_config);
                    }

                    let key = EngineKey::with_override(
                        EngineType::Mesio,
                        engine_id,
                        override_hash.unwrap(),
                    );
                    return Ok((
                        Arc::new(MesioEngine::with_config(base_config)),
                        EngineType::Mesio,
                        key,
                    ));
                }
            }
        }

        // 2. Normal resolution (no overrides)
        // If explicit ID provided
        if let Some(id) = engine_id {
            // Check if it's a known type string
            if let Ok(known_type) = id.parse::<EngineType>() {
                // Use default registered engine for this type
                let engine = self.get_engine(known_type).ok_or_else(|| {
                    crate::Error::Other(format!("Default engine {} not registered", known_type))
                })?;
                // Global default for this type
                let key = EngineKey::global(known_type);
                return Ok((engine, known_type, key));
            }

            // Otherwise try to look up in DB
            if let Some(repo) = &self.config_repo {
                if let Ok(config) = repo.get_engine_config(id).await {
                    // Found valid config, instantiate specific engine
                    if let Ok(engine_type) = config.engine_type.parse::<EngineType>() {
                        return match engine_type {
                            EngineType::Ffmpeg => {
                                let engine_config: FfmpegEngineConfig =
                                    serde_json::from_str(&config.config).map_err(|e| {
                                        crate::Error::Other(format!(
                                            "Failed to parse ffmpeg config: {}",
                                            e
                                        ))
                                    })?;
                                Ok((
                                    Arc::new(FfmpegEngine::with_config(engine_config))
                                        as Arc<dyn DownloadEngine>,
                                    engine_type,
                                    EngineKey::custom(engine_type, id),
                                ))
                            }
                            EngineType::Streamlink => {
                                let engine_config: StreamlinkEngineConfig =
                                    serde_json::from_str(&config.config).map_err(|e| {
                                        crate::Error::Other(format!(
                                            "Failed to parse streamlink config: {}",
                                            e
                                        ))
                                    })?;
                                Ok((
                                    Arc::new(StreamlinkEngine::with_config(engine_config))
                                        as Arc<dyn DownloadEngine>,
                                    engine_type,
                                    EngineKey::custom(engine_type, id),
                                ))
                            }
                            EngineType::Mesio => {
                                let engine_config: MesioEngineConfig =
                                    serde_json::from_str(&config.config).map_err(|e| {
                                        crate::Error::Other(format!(
                                            "Failed to parse mesio config: {}",
                                            e
                                        ))
                                    })?;
                                Ok((
                                    Arc::new(MesioEngine::with_config(engine_config))
                                        as Arc<dyn DownloadEngine>,
                                    engine_type,
                                    EngineKey::custom(engine_type, id),
                                ))
                            }
                        };
                    } else {
                        return Err(crate::Error::Other(format!(
                            "Unknown engine type in config: {}",
                            config.engine_type
                        )));
                    }
                } else {
                    warn!("Engine config {} not found, using default", id);
                }
            }
        }

        // Return default
        let default_type = self.config.default_engine;
        let engine = self.get_engine(default_type).ok_or_else(|| {
            crate::Error::Other(format!("Default engine {} not registered", default_type))
        })?;
        let key = EngineKey::global(default_type);
        Ok((engine, default_type, key))
    }

    /// Helper to merge a base config with JSON overrides
    fn merge_config_json<T: Serialize>(
        base: &T,
        override_val: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut base_val =
            serde_json::to_value(base).map_err(|e| crate::Error::Other(e.to_string()))?;
        json_patch::merge(&mut base_val, override_val);
        Ok(base_val)
    }

    /// Start a download with a specific engine.
    async fn start_download_with_engine(
        &self,
        config: DownloadConfig,
        engine: Arc<dyn DownloadEngine>,
        engine_type: EngineType,
        engine_key: EngineKey,
        is_high_priority: bool,
    ) -> Result<String> {
        if !engine.is_available() {
            return Err(crate::Error::Other(format!(
                "Engine {} is not available",
                engine_type
            )));
        }

        // Acquire semaphore permit and hold it until the download finishes
        let permit = if is_high_priority {
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
                output_path: None,
                permit: Some(permit),
                retry_config_override: None,
            },
        );

        // Emit start event (broadcast send is synchronous, ignore if no receivers)
        let _ = self.event_tx.send(DownloadManagerEvent::DownloadStarted {
            download_id: download_id.clone(),
            streamer_id: config.streamer_id.clone(),
            session_id: config.session_id.clone(),
            engine_type,
        });

        info!(
            "Starting download {} for streamer {} with engine {}",
            download_id, config.streamer_id, engine_type
        );

        // Start the engine
        let engine_clone = engine.clone();
        let handle_clone = handle.clone();
        tokio::spawn(async move {
            if let Err(e) = engine_clone.start(handle_clone.clone()).await {
                error!("Engine start error: {}", e);
                let _ = handle_clone
                    .event_tx
                    .send(SegmentEvent::DownloadFailed {
                        error: format!("Engine start error: {}", e),
                        recoverable: false,
                    })
                    .await;
            }
        });

        // Spawn task to handle segment events
        let download_id_clone = download_id.clone();
        let event_tx = self.event_tx.clone();
        let streamer_id = config.streamer_id.clone();
        let session_id = config.session_id.clone();

        // Clone references for the spawned task
        let active_downloads = self.active_downloads.clone();
        let pending_updates = self.pending_updates.clone();
        let circuit_breakers_ref = self.circuit_breakers.get(&engine_key);

        tokio::spawn(async move {
            while let Some(event) = segment_rx.recv().await {
                match event {
                    SegmentEvent::SegmentCompleted(info) => {
                        let SegmentInfo {
                            path,
                            duration_secs,
                            size_bytes,
                            index,
                            ..
                        } = info;
                        let segment_path = path.to_string_lossy().to_string();
                        // Broadcast send is synchronous, ignore if no receivers
                        let _ = event_tx.send(DownloadManagerEvent::SegmentCompleted {
                            download_id: download_id_clone.clone(),
                            streamer_id: streamer_id.clone(),
                            session_id: session_id.clone(),
                            segment_path: segment_path.clone(),
                            segment_index: index,
                            duration_secs,
                            size_bytes,
                        });

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.output_path = Some(segment_path);
                        }
                        debug!(
                            download_id = %download_id_clone,
                            path = %path.display(),
                            "Segment completed"
                        );
                    }
                    SegmentEvent::Progress(progress) => {
                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.progress = progress.clone();
                            download.status = DownloadStatus::Downloading;
                        }
                        // Broadcast progress event to WebSocket subscribers
                        let _ = event_tx.send(DownloadManagerEvent::Progress {
                            download_id: download_id_clone.clone(),
                            streamer_id: streamer_id.clone(),
                            session_id: session_id.clone(),
                            progress,
                        });
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

                        // remove download from active_downloads
                        // just before the event to avoid race condition
                        let output_path = if let Some((_, download)) =
                            active_downloads.remove(&download_id_clone)
                        {
                            download.output_path
                        } else {
                            None
                        };

                        pending_updates.remove(&download_id_clone);

                        let _ = event_tx.send(DownloadManagerEvent::DownloadCompleted {
                            download_id: download_id_clone.clone(),
                            streamer_id: streamer_id.clone(),
                            session_id: session_id.clone(),
                            total_bytes,
                            total_duration_secs,
                            total_segments,
                            file_path: output_path,
                        });

                        debug!(
                            download_id = %download_id_clone,
                            "Download completed"
                        );
                        break;
                    }
                    SegmentEvent::DownloadFailed { error, recoverable } => {
                        circuit_breakers_ref.record_failure();

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.status = DownloadStatus::Failed;
                        }

                        // remove download from active_downloads
                        // just before the event to avoid race condition
                        active_downloads.remove(&download_id_clone);
                        pending_updates.remove(&download_id_clone);

                        let _ = event_tx.send(DownloadManagerEvent::DownloadFailed {
                            download_id: download_id_clone.clone(),
                            streamer_id: streamer_id.clone(),
                            session_id: session_id.clone(),
                            error,
                            recoverable,
                        });

                        break;
                    }
                    SegmentEvent::SegmentStarted { path, sequence } => {
                        let segment_path = path.to_string_lossy().to_string();

                        // Emit segment started event
                        let _ = event_tx.send(DownloadManagerEvent::SegmentStarted {
                            download_id: download_id_clone.clone(),
                            streamer_id: streamer_id.clone(),
                            session_id: session_id.clone(),
                            segment_path: segment_path.clone(),
                            segment_index: sequence,
                        });

                        if let Some((_, pending_update)) =
                            pending_updates.remove(&download_id_clone)
                            && let Some(mut download) = active_downloads.get_mut(&download_id_clone)
                        {
                            DownloadManager::apply_pending_update_to_download(
                                &mut download,
                                pending_update,
                                &download_id_clone,
                                &streamer_id,
                                &event_tx,
                            );
                        }

                        debug!(
                            download_id = %download_id_clone,
                            path = %path.display(),
                            sequence = sequence,
                            "Segment started"
                        );
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

            let streamer_id = download.handle.config_snapshot().streamer_id;

            self.pending_updates.remove(download_id);

            // Broadcast send is synchronous, ignore if no receivers
            let _ = self.event_tx.send(DownloadManagerEvent::DownloadCancelled {
                download_id: download_id.to_string(),
                streamer_id: streamer_id.clone(),
                session_id: download.handle.config_snapshot().session_id,
            });

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
                let config_snapshot = download.handle.config_snapshot();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: config_snapshot.streamer_id,
                    session_id: config_snapshot.session_id,
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

    /// Maximum normal-priority concurrent downloads.
    pub fn max_concurrent_downloads(&self) -> usize {
        self.config.max_concurrent_downloads
    }

    /// Extra slots reserved for high-priority downloads.
    pub fn high_priority_extra_slots(&self) -> usize {
        self.config.high_priority_extra_slots
    }

    /// Total concurrent download slots (normal + high priority extra).
    pub fn total_concurrent_slots(&self) -> usize {
        self.config
            .max_concurrent_downloads
            .saturating_add(self.config.high_priority_extra_slots)
    }

    /// Subscribe to download events.
    ///
    /// Returns a broadcast receiver that will receive all download events.
    /// Multiple subscribers can receive the same events concurrently.
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadManagerEvent> {
        self.event_tx.subscribe()
    }

    /// Update configuration for an active download.
    ///
    /// Queues configuration updates (cookies, headers, retry policy) to be applied
    /// when the next segment starts. Multiple updates are merged, with newer values
    /// overwriting older ones.
    ///
    /// # Arguments
    /// * `download_id` - The ID of the download to update
    /// * `cookies` - Optional new cookies to apply
    /// * `headers` - Optional new headers to apply
    /// * `retry_config` - Optional new retry configuration to apply
    ///
    /// # Returns
    /// * `Ok(())` if the update was queued successfully
    /// * `Err(NotFound)` if the download does not exist
    pub fn update_download_config(
        &self,
        download_id: &str,
        cookies: Option<String>,
        headers: Option<Vec<(String, String)>>,
        retry_config: Option<RetryConfig>,
    ) -> Result<()> {
        // Validate download exists in active_downloads
        let download =
            self.active_downloads
                .get(download_id)
                .ok_or_else(|| crate::Error::NotFound {
                    entity_type: "Download".to_string(),
                    id: download_id.to_string(),
                })?;

        let streamer_id = download.handle.config_snapshot().streamer_id;
        // Drop the reference to avoid holding the lock while updating pending_updates
        drop(download);

        // Create the new pending update
        let new_update =
            PendingConfigUpdate::new(cookies.clone(), headers.clone(), retry_config.clone());

        // Only store if there are actual updates
        if new_update.has_updates() {
            // Create or merge PendingConfigUpdate in pending_updates map
            self.pending_updates
                .entry(download_id.to_string())
                .and_modify(|existing| {
                    existing.merge(new_update.clone());
                })
                .or_insert(new_update);

            // Log the queued update
            info!(
                "Config update queued for download {}: cookies={}, headers={}, retry={}",
                download_id,
                cookies.is_some(),
                headers.is_some(),
                retry_config.is_some()
            );

            debug!(
                "Download {} for streamer {} will apply config on next segment",
                download_id, streamer_id
            );
        } else {
            debug!(
                "Empty config update for download {} - no changes queued",
                download_id
            );
        }

        Ok(())
    }

    /// Get download by streamer ID.
    pub fn get_download_by_streamer(&self, streamer_id: &str) -> Option<DownloadInfo> {
        self.active_downloads
            .iter()
            .find(|entry| entry.value().handle.config_snapshot().streamer_id == streamer_id)
            .map(|entry| {
                let download = entry.value();
                let config_snapshot = download.handle.config_snapshot();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: config_snapshot.streamer_id,
                    session_id: config_snapshot.session_id,
                    engine_type: download.handle.engine_type,
                    status: download.status,
                    progress: download.progress.clone(),
                    started_at: download.handle.started_at,
                }
            })
    }

    /// Check if a streamer has an active download.
    ///
    /// Only considers downloads with status Starting or Downloading as active.
    /// Failed, Completed, or Cancelled downloads are not considered active,
    /// preventing race conditions where a failed download blocks new attempts.
    pub fn has_active_download(&self, streamer_id: &str) -> bool {
        self.active_downloads.iter().any(|entry| {
            let download = entry.value();
            download.handle.config_snapshot().streamer_id == streamer_id
                && matches!(
                    download.status,
                    DownloadStatus::Starting | DownloadStatus::Downloading
                )
        })
    }

    /// Take pending updates for a download (called by engines at segment boundaries).
    ///
    /// Atomically removes and returns the pending configuration update for the specified
    /// download. This should be called by download engines when starting a new segment
    /// to apply any queued configuration changes.
    ///
    /// # Arguments
    /// * `download_id` - The ID of the download to take pending updates for
    ///
    /// # Returns
    /// * `Some(PendingConfigUpdate)` if there were pending updates
    /// * `None` if no updates were pending for this download
    pub fn take_pending_updates(&self, download_id: &str) -> Option<PendingConfigUpdate> {
        self.pending_updates
            .remove(download_id)
            .map(|(_, update)| update)
    }

    /// Check if a download has pending configuration updates.
    ///
    /// # Arguments
    /// * `download_id` - The ID of the download to check
    ///
    /// # Returns
    /// * `true` if there are pending updates for this download
    /// * `false` otherwise
    pub fn has_pending_updates(&self, download_id: &str) -> bool {
        self.pending_updates.contains_key(download_id)
    }

    /// Emit a ConfigUpdated event for a successfully applied configuration update.
    ///
    /// This helper method determines the appropriate `ConfigUpdateType` based on which
    /// fields were present in the `PendingConfigUpdate` and emits the event via the
    /// broadcast channel.
    ///
    /// # Arguments
    /// * `download_id` - The ID of the download that was updated
    /// * `streamer_id` - The streamer ID associated with the download
    /// * `update` - The pending config update that was applied
    ///
    /// # Returns
    /// * `true` if the event was sent successfully (at least one receiver)
    /// * `false` if there were no receivers or the update had no changes
    pub fn emit_config_updated(
        &self,
        download_id: &str,
        streamer_id: &str,
        update: &PendingConfigUpdate,
    ) -> bool {
        // Don't emit if there are no actual updates
        if !update.has_updates() {
            return false;
        }

        let update_type = Self::determine_config_update_type(update);

        let event = DownloadManagerEvent::ConfigUpdated {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            update_type,
        };

        // Broadcast send returns Ok if at least one receiver got the message
        // Returns Err if there are no receivers, which is fine
        match self.event_tx.send(event) {
            Ok(_) => {
                debug!(
                    "Emitted ConfigUpdated event for download {} (streamer {})",
                    download_id, streamer_id
                );
                true
            }
            Err(_) => {
                // No receivers - this is not an error, just means no one is listening
                debug!(
                    "ConfigUpdated event for download {} had no receivers",
                    download_id
                );
                false
            }
        }
    }

    /// Emit a ConfigUpdateFailed event when a configuration update fails to apply.
    ///
    /// # Arguments
    /// * `download_id` - The ID of the download that failed to update
    /// * `streamer_id` - The streamer ID associated with the download
    /// * `error` - Description of the error that occurred
    ///
    /// # Returns
    /// * `true` if the event was sent successfully (at least one receiver)
    /// * `false` if there were no receivers
    pub fn emit_config_update_failed(
        &self,
        download_id: &str,
        streamer_id: &str,
        error: &str,
    ) -> bool {
        let event = DownloadManagerEvent::ConfigUpdateFailed {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            error: error.to_string(),
        };

        match self.event_tx.send(event) {
            Ok(_) => {
                warn!(
                    "Emitted ConfigUpdateFailed event for download {}: {}",
                    download_id, error
                );
                true
            }
            Err(_) => {
                debug!(
                    "ConfigUpdateFailed event for download {} had no receivers",
                    download_id
                );
                false
            }
        }
    }

    /// Determine the ConfigUpdateType based on which fields are present in the update.
    ///
    /// # Arguments
    /// * `update` - The pending config update to analyze
    ///
    /// # Returns
    /// The appropriate `ConfigUpdateType` variant:
    /// - `Multiple` if more than one field is set
    /// - `Cookies`, `Headers`, or `RetryConfig` if only one field is set
    /// - `Multiple` as fallback (should not happen if `has_updates()` is true)
    fn determine_config_update_type(update: &PendingConfigUpdate) -> ConfigUpdateType {
        let has_cookies = update.cookies.is_some();
        let has_headers = update.headers.is_some();
        let has_retry = update.retry_config.is_some();

        let count = [has_cookies, has_headers, has_retry]
            .iter()
            .filter(|&&x| x)
            .count();

        if count > 1 {
            ConfigUpdateType::Multiple
        } else if has_cookies {
            ConfigUpdateType::Cookies
        } else if has_headers {
            ConfigUpdateType::Headers
        } else if has_retry {
            ConfigUpdateType::RetryConfig
        } else {
            // Fallback - should not happen if has_updates() returned true
            ConfigUpdateType::Multiple
        }
    }

    fn apply_pending_update_to_download(
        download: &mut ActiveDownload,
        update: PendingConfigUpdate,
        download_id: &str,
        streamer_id: &str,
        event_tx: &broadcast::Sender<DownloadManagerEvent>,
    ) {
        let mut applied = false;
        let update_clone = update.clone();
        let PendingConfigUpdate {
            cookies,
            headers,
            retry_config,
            ..
        } = update;

        if cookies.is_some() || headers.is_some() {
            let mut cfg = download.handle.config.write();
            if let Some(cookie_val) = cookies.clone() {
                cfg.cookies = Some(cookie_val);
                applied = true;
            }
            if let Some(header_val) = headers.clone() {
                cfg.headers = header_val;
                applied = true;
            }
        }

        if let Some(retry) = retry_config {
            download.retry_config_override = Some(retry);
            applied = true;
        }

        if applied {
            let update_type = Self::determine_config_update_type(&update_clone);
            let _ = event_tx.send(DownloadManagerEvent::ConfigUpdated {
                download_id: download_id.to_string(),
                streamer_id: streamer_id.to_string(),
                update_type,
            });
        }
    }

    /// Get downloads by status.
    pub fn get_downloads_by_status(&self, status: DownloadStatus) -> Vec<DownloadInfo> {
        self.active_downloads
            .iter()
            .filter(|entry| entry.value().status == status)
            .map(|entry| {
                let download = entry.value();
                let config_snapshot = download.handle.config_snapshot();
                DownloadInfo {
                    id: download.handle.id.clone(),
                    streamer_id: config_snapshot.streamer_id,
                    session_id: config_snapshot.session_id,
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
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::thread;

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
        let manager = DownloadManager::new();

        // FFmpeg should be registered by default
        assert!(manager.get_engine(EngineType::Ffmpeg).is_some());
        assert!(manager.get_engine(EngineType::Streamlink).is_some());
        assert!(manager.get_engine(EngineType::Mesio).is_some());
    }

    #[test]
    fn test_determine_config_update_type_cookies_only() {
        let update = PendingConfigUpdate::new(Some("session=abc123".to_string()), None, None);
        assert_eq!(
            DownloadManager::determine_config_update_type(&update),
            ConfigUpdateType::Cookies
        );
    }

    #[test]
    fn test_determine_config_update_type_headers_only() {
        let update = PendingConfigUpdate::new(
            None,
            Some(vec![(
                "Authorization".to_string(),
                "Bearer token".to_string(),
            )]),
            None,
        );
        assert_eq!(
            DownloadManager::determine_config_update_type(&update),
            ConfigUpdateType::Headers
        );
    }

    #[test]
    fn test_determine_config_update_type_retry_only() {
        let update = PendingConfigUpdate::new(None, None, Some(RetryConfig::default()));
        assert_eq!(
            DownloadManager::determine_config_update_type(&update),
            ConfigUpdateType::RetryConfig
        );
    }

    #[test]
    fn test_determine_config_update_type_multiple() {
        let update = PendingConfigUpdate::new(
            Some("session=abc123".to_string()),
            Some(vec![(
                "Authorization".to_string(),
                "Bearer token".to_string(),
            )]),
            None,
        );
        assert_eq!(
            DownloadManager::determine_config_update_type(&update),
            ConfigUpdateType::Multiple
        );
    }

    #[test]
    fn test_determine_config_update_type_all_three() {
        let update = PendingConfigUpdate::new(
            Some("session=abc123".to_string()),
            Some(vec![(
                "Authorization".to_string(),
                "Bearer token".to_string(),
            )]),
            Some(RetryConfig::default()),
        );
        assert_eq!(
            DownloadManager::determine_config_update_type(&update),
            ConfigUpdateType::Multiple
        );
    }

    #[test]
    fn test_emit_config_updated_with_subscriber() {
        let manager = DownloadManager::new();
        let mut receiver = manager.subscribe();

        let update = PendingConfigUpdate::new(Some("session=abc123".to_string()), None, None);

        let result = manager.emit_config_updated("download-123", "streamer-456", &update);
        assert!(result);

        // Verify the event was received
        let event = receiver.try_recv().unwrap();
        match event {
            DownloadManagerEvent::ConfigUpdated {
                download_id,
                streamer_id,
                update_type,
            } => {
                assert_eq!(download_id, "download-123");
                assert_eq!(streamer_id, "streamer-456");
                assert_eq!(update_type, ConfigUpdateType::Cookies);
            }
            _ => panic!("Expected ConfigUpdated event"),
        }
    }

    #[test]
    fn test_emit_config_updated_no_updates() {
        let manager = DownloadManager::new();
        let _receiver = manager.subscribe();

        let update = PendingConfigUpdate::default();
        assert!(!update.has_updates());

        let result = manager.emit_config_updated("download-123", "streamer-456", &update);
        assert!(!result);
    }

    #[test]
    fn test_emit_config_update_failed_with_subscriber() {
        let manager = DownloadManager::new();
        let mut receiver = manager.subscribe();

        let result =
            manager.emit_config_update_failed("download-123", "streamer-456", "Connection timeout");
        assert!(result);

        // Verify the event was received
        let event = receiver.try_recv().unwrap();
        match event {
            DownloadManagerEvent::ConfigUpdateFailed {
                download_id,
                streamer_id,
                error,
            } => {
                assert_eq!(download_id, "download-123");
                assert_eq!(streamer_id, "streamer-456");
                assert_eq!(error, "Connection timeout");
            }
            _ => panic!("Expected ConfigUpdateFailed event"),
        }
    }

    // Helper function to create a test download manager with a mock active download
    fn create_manager_with_active_download(
        download_id: &str,
        streamer_id: &str,
    ) -> DownloadManager {
        let manager = DownloadManager::new();

        // Create a mock active download entry
        let (segment_tx, _segment_rx) = tokio::sync::mpsc::channel::<SegmentEvent>(32);
        let config = DownloadConfig::new(
            "http://test.example.com/stream",
            "/tmp/test",
            streamer_id,
            "test-session",
        );

        let handle = Arc::new(DownloadHandle::new(
            download_id.to_string(),
            EngineType::Ffmpeg,
            config,
            segment_tx,
        ));

        let active_download = ActiveDownload {
            handle,
            status: DownloadStatus::Downloading,
            progress: DownloadProgress::default(),
            is_high_priority: false,
            output_path: None,
            permit: None,
            retry_config_override: None,
        };

        manager
            .active_downloads
            .insert(download_id.to_string(), active_download);
        manager
    }

    // **Feature: download-config-updates, Property 5: Concurrent updates are thread-safe**
    // **Validates: Requirements 4.1, 4.3**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_concurrent_updates_are_thread_safe(
            num_threads in 2usize..5usize,
            updates_per_thread in 1usize..4usize,
            cookies in prop::collection::vec("[a-zA-Z0-9]{5,15}", 1..3),
        ) {
            // Create a manager with an active download
            let download_id = "test-download-concurrent";
            let streamer_id = "test-streamer";
            let manager = Arc::new(create_manager_with_active_download(download_id, streamer_id));

            // Spawn multiple threads that concurrently update the config
            let handles: Vec<_> = (0..num_threads)
                .map(|thread_idx| {
                    let manager_clone = Arc::clone(&manager);
                    let cookies_clone = cookies.clone();
                    let download_id = download_id.to_string();

                    thread::spawn(move || {
                        for update_idx in 0..updates_per_thread {
                            // Each thread uses a different cookie value based on thread and update index
                            let cookie_idx = (thread_idx + update_idx) % cookies_clone.len();
                            let cookie = Some(cookies_clone[cookie_idx].clone());

                            // This should not panic or cause data races
                            let result = manager_clone.update_download_config(
                                &download_id,
                                cookie,
                                None,
                                None,
                            );

                            // All updates should succeed since the download exists
                            assert!(result.is_ok(), "Update should succeed for existing download");
                        }
                    })
                })
                .collect();

            // Wait for all threads to complete
            for handle in handles {
                handle.join().expect("Thread should not panic");
            }

            // Property: After all concurrent updates, the pending_updates map should contain
            // a valid merged update (the final state should reflect a valid merge of all updates)
            let final_update = manager.take_pending_updates(download_id);

            // There should be a pending update since we made updates
            prop_assert!(
                final_update.is_some(),
                "Should have pending updates after concurrent updates"
            );

            let update = final_update.unwrap();

            // The update should have valid data (one of the cookies we provided)
            if let Some(ref cookie) = update.cookies {
                prop_assert!(
                    cookies.contains(cookie),
                    "Final cookie should be one of the provided cookies"
                );
            }

            // After taking, there should be no more pending updates
            prop_assert!(
                !manager.has_pending_updates(download_id),
                "Pending updates should be cleared after take"
            );
        }
    }
}
