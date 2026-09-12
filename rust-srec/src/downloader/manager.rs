//! Download Manager implementation.

mod attempt;
mod configuration;
mod coordination;
mod events;

#[cfg(test)]
mod tests;

use coordination::DownloadEventPublisher;
pub(crate) use coordination::{
    DownloadCoordinationReceiver, DownloadCoordinationSender, download_coordination_channel,
};
pub use events::{
    ConfigUpdateType, DownloadManagerEvent, DownloadProgressEvent, DownloadRejectedKind,
    DownloadShutdownReport, DownloadStopCause, DownloadTerminalEvent, EngineEndSignal,
};

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadInfo,
    DownloadProgress, DownloadStatus, EngineType, FfmpegEngine, MesioEngine, StreamlinkEngine,
};
use super::output_root_gate::OutputRootGate;
use super::queue::{
    AcquireError as QueueAcquireError, AcquireRequest, ActiveSlot, DownloadQueue,
    PendingEntry as QueuePendingEntry, Priority, SlotGuard,
};
use super::resilience::{CircuitBreakerManager, EngineKey, RetryConfig};
use crate::Result;
use crate::database::repositories::config::ConfigRepository;

use attempt::{AttemptCompletion, AttemptSupervisor};

fn parse_engine_config<T: DeserializeOwned>(engine: &'static str, raw: &str) -> Result<T> {
    serde_json::from_str(raw)
        .map_err(|e| crate::Error::Other(format!("Failed to parse {} config: {}", engine, e)))
}

fn resolve_segment_path(path: &std::path::Path) -> String {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(current_dir) => current_dir.join(path),
            Err(error) => {
                warn!(
                    path = %path.display(),
                    error = %error,
                    "Failed to resolve relative segment path; preserving engine path"
                );
                path.to_path_buf()
            }
        }
    };

    resolved.to_string_lossy().into_owned()
}

/// Walk the `std::error::Error::source()` chain of `err` and return the
/// first `std::io::Error` found, if any. Used by `prepare_output_dir` to
/// hand the output-root write gate the raw `io::Error` so it can classify
/// the `ErrorKind` correctly (ENOENT vs ENOSPC vs EACCES etc.).
fn io_error_in_chain<'a>(err: &'a (dyn std::error::Error + 'static)) -> Option<&'a std::io::Error> {
    let mut current: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = current {
        if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
            return Some(io_err);
        }
        current = e.source();
    }
    None
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
    /// Successes required in half-open state to close the circuit.
    pub circuit_breaker_half_open_success_threshold: u32,
    /// Failures allowed in half-open state before reopening the circuit.
    pub circuit_breaker_half_open_failure_threshold: u32,
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
            circuit_breaker_half_open_success_threshold: 2,
            circuit_breaker_half_open_failure_threshold: 2,
        }
    }
}

/// Internal state for an active download.
struct ActiveDownload {
    handle: Arc<DownloadHandle>,
    phase: Arc<Mutex<AttemptPhase>>,
    completion: Arc<AttemptCompletion>,
    status: DownloadStatus,
    progress: DownloadProgress,
    /// Last known output path (from segments)
    pub output_path: Option<String>,
    current_segment_index: Option<u32>,
    current_engine_segment_index: Option<u32>,
    current_segment_path: Option<String>,
    current_segment_started_at: Option<DateTime<Utc>>,
    /// Queue slot held by the download. Released (and the next waiter
    /// woken) when this `ActiveDownload` entry is removed from the
    /// active map.
    #[expect(
        dead_code,
        reason = "owns queue capacity until the active download is removed"
    )]
    slot: Option<ActiveSlot>,
}

impl From<&ActiveDownload> for DownloadInfo {
    fn from(download: &ActiveDownload) -> Self {
        let config = download.handle.config.read();
        Self {
            id: download.handle.id.clone(),
            url: config.url.clone(),
            streamer_id: config.streamer_id.clone(),
            session_id: config.session_id.clone(),
            engine_type: download.handle.engine_type,
            status: download.status,
            progress: download.progress.clone(),
            started_at: download.handle.started_at,
        }
    }
}

#[derive(Debug, Clone)]
enum AttemptPhase {
    Running,
    StopRequested(DownloadStopCause),
    TerminalChosen,
}

impl AttemptPhase {
    fn stop_cause(&self) -> Option<DownloadStopCause> {
        match self {
            Self::StopRequested(cause) => Some(cause.clone()),
            Self::Running | Self::TerminalChosen => None,
        }
    }

    fn is_stop_requested(&self) -> bool {
        matches!(self, Self::StopRequested(_))
    }
}

/// The Download Manager service.
pub struct DownloadManager {
    /// Configuration.
    config: RwLock<DownloadManagerConfig>,
    /// Serializes configured-limit updates and temporary throttle changes through queue publication.
    throttle_factor: Mutex<Option<f32>>,
    /// Priority-aware queue managing concurrency across both
    /// normal-priority and high-priority extra slots.
    queue: Arc<DownloadQueue>,
    /// Active downloads.
    active_downloads: Arc<DashMap<String, ActiveDownload>>,
    /// Owns every recording attempt and linearizes attempt admission against
    /// shutdown. Engine and event-translator tasks never outlive this runtime.
    attempts: AttemptSupervisor,
    /// Fences preflight, queue admission, and pre-start terminal publication
    /// against the required-event shutdown marker.
    operation_gate: tokio::sync::RwLock<()>,
    accepting_operations: AtomicBool,
    /// Next session-scoped segment index keyed by recording session id.
    /// The per-download `engine_segment_index -> session_segment_index`
    /// mapping is held as a local variable in the spawn loop in
    /// `start_with_slot`, so this map carries only the monotonic counter.
    /// Cleared by `clear_session_segment_index` on
    /// `SessionTransition::Ended` (the only surface that's actually shared
    /// across download attempts within a session).
    session_segment_indices: Arc<DashMap<String, u32>>,
    /// Engine registry.
    engines: RwLock<HashMap<EngineType, Arc<dyn DownloadEngine>>>,
    /// Circuit breaker manager.
    circuit_breakers: CircuitBreakerManager,
    /// Output-root write gate. Optional so existing tests and simple callers
    /// (e.g. CLI utilities) can run without installing a full gate + recovery
    /// hook + notification service. Production is always wired up in
    /// [`crate::services::container`].
    ///
    /// Stored in a `OnceLock` so the services container can construct the
    /// download manager first and attach the gate later (one of the two
    /// container builders initializes `NotificationService` after the
    /// download manager, and the gate depends on the former). After the
    /// one-shot write, reads are lock-free.
    output_root_gate: OnceLock<Arc<OutputRootGate>>,
    /// Publishes best-effort observer events and reliable terminal events.
    events: DownloadEventPublisher,
    /// Config repository for resolving custom engines.
    config_repo: Option<Arc<dyn ConfigRepository>>,
    /// Queue-wait freshness threshold (ms). Read on the per-pipeline
    /// hot path, hence `AtomicI64` rather than the `RwLock`-guarded
    /// [`DownloadManagerConfig`].
    queue_freshness_threshold_ms: AtomicI64,
}

/// Input to [`DownloadManager::preflight`].
///
/// Carries just enough metadata to validate the engine, circuit
/// breaker, and output root before the slot is acquired. A full
/// [`DownloadConfig`] isn't required here — that gets passed to
/// [`DownloadManager::start_with_slot`] only after a slot is granted
/// (and after any post-acquire URL-freshness step the caller wants).
#[derive(Debug, Clone)]
pub struct PreflightRequest {
    pub streamer_id: String,
    pub streamer_name: String,
    pub session_id: String,
    pub output_dir: std::path::PathBuf,
    /// Engine id override; `None` means use the global default.
    pub engine_id: Option<String>,
    /// Per-engine config overrides forwarded from
    /// [`DownloadConfig::engines_override`].
    pub engines_override: Option<serde_json::Value>,
}

/// Resolved engine handle returned by [`DownloadManager::preflight`]
/// and consumed by [`DownloadManager::start_with_slot`].
#[derive(Clone)]
pub struct EngineHandle {
    pub(crate) engine: Arc<dyn DownloadEngine>,
    pub engine_type: EngineType,
    pub(crate) engine_key: EngineKey,
}

impl std::fmt::Debug for EngineHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHandle")
            .field("engine_type", &self.engine_type)
            .field("engine_key", &self.engine_key)
            .finish_non_exhaustive()
    }
}

impl DownloadManager {
    /// Create a new Download Manager.
    pub fn new() -> Self {
        Self::with_config(DownloadManagerConfig::default())
    }

    /// Create a new Download Manager with custom configuration.
    pub fn with_config(mut config: DownloadManagerConfig) -> Self {
        config.max_concurrent_downloads = config.max_concurrent_downloads.max(1);
        // Use broadcast channel to support multiple subscribers
        let (event_tx, _) = broadcast::channel(256);

        let queue = DownloadQueue::new(
            config.max_concurrent_downloads,
            config.high_priority_extra_slots,
        );

        let circuit_breakers = CircuitBreakerManager::with_half_open(
            config.circuit_breaker_threshold,
            config.circuit_breaker_cooldown_secs,
            config.circuit_breaker_half_open_success_threshold,
            config.circuit_breaker_half_open_failure_threshold,
        );

        let manager = Self {
            config: RwLock::new(config),
            throttle_factor: Mutex::new(None),
            queue,
            active_downloads: Arc::new(DashMap::new()),
            attempts: AttemptSupervisor::new(),
            operation_gate: tokio::sync::RwLock::new(()),
            accepting_operations: AtomicBool::new(true),
            session_segment_indices: Arc::new(DashMap::new()),
            engines: RwLock::new(HashMap::new()),
            circuit_breakers,
            output_root_gate: OnceLock::new(),
            events: DownloadEventPublisher::new(event_tx, None),
            config_repo: None,
            // Overwritten from persisted global config at boot.
            queue_freshness_threshold_ms: AtomicI64::new(60_000),
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

    pub(crate) fn set_scheduler_feedback(
        &self,
        feedback: Arc<crate::scheduler::feedback::SchedulerFeedback>,
    ) -> Result<()> {
        feedback.ensure_attempt_capacity(self.total_concurrent_slots());
        self.events
            .feedback
            .set(feedback)
            .map_err(|_| crate::Error::Other("scheduler feedback is already configured".to_owned()))
    }

    pub(crate) fn with_coordination_sender(mut self, sender: DownloadCoordinationSender) -> Self {
        self.events.coordination_tx = Some(sender);
        self
    }

    /// Attach an output-root write gate during construction.
    ///
    /// Prefer this over [`Self::set_output_root_gate`] when the gate's
    /// dependencies are available at construction time. When they aren't
    /// (e.g. the services container builds `NotificationService` after
    /// `DownloadManager`), use `set_output_root_gate` instead.
    pub fn with_output_root_gate(self, gate: Arc<OutputRootGate>) -> Self {
        self.set_output_root_gate(gate);
        self
    }

    /// Late-bind the output-root write gate. Used by
    /// the services container when the download manager is already
    /// wrapped in `Arc` and its dependencies are ready.
    ///
    /// Attempting to set the gate a second time is a no-op that logs a
    /// warning (OnceLock::set returns Err). The gate is expected to be
    /// configured exactly once per process.
    pub fn set_output_root_gate(&self, gate: Arc<OutputRootGate>) {
        if self.output_root_gate.set(gate).is_err() {
            warn!("Ignoring attempt to replace already-configured output-root gate");
        }
    }

    /// Clear session-scoped segment allocation state after a session reaches
    /// its final ended state. Only the monotonic counter is shared across
    /// download attempts within the session; the per-download
    /// `engine_segment_index -> session_segment_index` mapping is held local
    /// to the spawn loop in `start_with_slot` and drops automatically when
    /// that loop drains, so there is no per-download state to clear here.
    pub fn clear_session_segment_index(&self, session_id: &str) {
        self.session_segment_indices.remove(session_id);
    }

    fn seed_session_segment_index(indices: &DashMap<String, u32>, session_id: &str, next: u32) {
        indices
            .entry(session_id.to_string())
            .and_modify(|current| *current = (*current).max(next))
            .or_insert(next);
    }

    fn allocate_next_session_segment_index(
        indices: &DashMap<String, u32>,
        session_id: &str,
    ) -> u32 {
        let mut entry = indices.entry(session_id.to_string()).or_insert(0);
        let index = *entry;
        *entry = entry.saturating_add(1);
        index
    }

    /// Set the config repository.
    pub fn with_config_repo(mut self, config_repo: Arc<dyn ConfigRepository>) -> Self {
        self.config_repo = Some(config_repo);
        self
    }

    /// Register a download engine.
    pub fn register_engine(&self, engine: Arc<dyn DownloadEngine>) {
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
    ///
    /// Convenience wrapper that runs the split download startup pipeline
    /// (`preflight` → `acquire_slot` → `start_with_slot`) sequentially
    /// with no cancellation hook and no step-level visibility. Used by
    /// tests, the scheduler, and anything that does not need to react
    /// to "queued waiting for slot" or post-acquire freshness checks.
    /// New per-streamer pipelines should call the three methods
    /// directly so they can interleave freshness, cancellation, and
    /// danmu wiring between acquire and start.
    pub async fn start_download(
        &self,
        config: DownloadConfig,
        engine_id: Option<String>,
        is_high_priority: bool,
    ) -> Result<String> {
        let priority = if is_high_priority {
            Priority::High
        } else {
            Priority::Normal
        };
        let preflight_req = PreflightRequest {
            streamer_id: config.streamer_id.clone(),
            streamer_name: config.streamer_name.clone(),
            session_id: config.session_id.clone(),
            output_dir: config.output_dir.clone(),
            engine_id: engine_id.clone(),
            engines_override: config.engines_override.clone(),
        };
        let engine = self.preflight(preflight_req).await?;

        let acquire_req = AcquireRequest {
            session_id: config.session_id.clone(),
            streamer_id: config.streamer_id.clone(),
            streamer_name: config.streamer_name.clone(),
            engine_type: engine.engine_type,
            priority,
        };
        let slot = self
            .acquire_slot(acquire_req, CancellationToken::new())
            .await?;
        self.start_with_slot(slot, config, engine).await
    }

    /// Validate before acquiring a download slot.
    ///
    /// Resolves the requested engine, checks the streamer-scoped
    /// circuit breaker, checks the output-root write gate, and runs
    /// `prepare_output_dir`. Any failure emits the corresponding
    /// `DownloadRejected` event before returning the error, and never
    /// consumes a queue slot.
    ///
    /// On success, returns an [`EngineHandle`] that
    /// [`Self::start_with_slot`] consumes — this avoids re-resolving
    /// the engine after the slot is acquired.
    pub async fn preflight(&self, req: PreflightRequest) -> Result<EngineHandle> {
        let _operation = self.begin_operation().await?;
        let overrides = req.engines_override.as_ref();
        let (engine, engine_type, engine_key) = self
            .resolve_engine(req.engine_id.as_deref(), overrides)
            .await?;

        // Scope the circuit breaker to this streamer so one streamer's
        // CDN issues don't block unrelated streamers on the same engine.
        let engine_key = engine_key.for_streamer(&req.streamer_id);

        if !engine.is_available() {
            return Err(crate::Error::Other(format!(
                "Engine {} is not available",
                engine_type
            )));
        }

        // Check circuit breaker using the streamer-scoped key
        if !self.circuit_breakers.is_allowed(&engine_key) {
            warn!("Engine {} is disabled by circuit breaker", engine_key);
            let cooldown_secs = self.config.read().circuit_breaker_cooldown_secs;

            self.emit_rejected_admitted(
                req.streamer_id.clone(),
                req.streamer_name.clone(),
                req.session_id.clone(),
                format!("Circuit breaker open for engine {}", engine_key),
                Some(cooldown_secs),
                DownloadRejectedKind::CircuitBreaker,
            )
            .await?;

            return Err(crate::Error::Other(format!(
                "Engine {} is disabled by circuit breaker",
                engine_key
            )));
        }

        // Check the output-root write gate BEFORE acquiring any
        // resources. Hot path is ~O(1) when the gate map is empty.
        if let Some(gate) = self.output_root_gate.get()
            && let Err(blocked) = gate.check(&req.output_dir)
        {
            warn!(
                root = %blocked.root.display(),
                kind = blocked.kind.as_str(),
                "Output root gate rejected download (Degraded); emitting DownloadRejected"
            );
            let cooldown = super::output_root_gate::DEFAULT_GATE_COOLDOWN_SECS;
            self.emit_rejected_admitted(
                req.streamer_id.clone(),
                req.streamer_name.clone(),
                req.session_id.clone(),
                blocked.to_string(),
                Some(cooldown),
                DownloadRejectedKind::OutputRootUnavailable {
                    path: blocked.root.clone(),
                    io_kind: blocked.kind,
                },
            )
            .await?;
            return Err(crate::Error::Other(format!(
                "Output root {} is unwritable ({}); gate has the filesystem in Degraded state",
                blocked.root.display(),
                blocked.kind.as_str()
            )));
        }

        // Prepare the output directory BEFORE acquiring a queue slot —
        // a ENOENT/ENOSPC failure here would otherwise hold a slot
        // until the error path released it, starving healthy streamers.
        if let Err(engine_err) = self.prepare_output_dir_for_path(&req.output_dir).await {
            warn!(
                "Failed to prepare output directory for streamer {}: {}",
                req.streamer_id, engine_err
            );
            // For OutputRootUnavailable also emit DownloadRejected so
            // the scheduler can route to OutputRootBlocked.
            if let DownloadFailureKind::OutputRootUnavailable { io_kind } = engine_err.kind {
                let path = self
                    .output_root_gate
                    .get()
                    .map(|g| g.resolve_path(&req.output_dir))
                    .unwrap_or_else(|| super::output_root_gate::resolve_root(&req.output_dir, &[]));
                self.emit_rejected_admitted(
                    req.streamer_id.clone(),
                    req.streamer_name.clone(),
                    req.session_id.clone(),
                    engine_err.message.clone(),
                    Some(super::output_root_gate::DEFAULT_GATE_COOLDOWN_SECS),
                    DownloadRejectedKind::OutputRootUnavailable { path, io_kind },
                )
                .await?;
            }
            return Err(crate::Error::Other(engine_err.message));
        }

        Ok(EngineHandle {
            engine,
            engine_type,
            engine_key,
        })
    }

    /// Park on the priority-aware queue until a slot is available.
    ///
    /// Emits [`DownloadProgressEvent::DownloadQueued`] only when the
    /// request had to wait. The fast path (slot immediately available)
    /// returns without any event so a download that never queues
    /// produces only `DownloadStarted`.
    ///
    /// Honours the supplied `cancel` token: if it fires before a slot
    /// is granted, the future returns
    /// [`crate::Error::Other("download acquire cancelled")`] without
    /// holding any queue capacity.
    pub async fn acquire_slot(
        &self,
        req: AcquireRequest,
        cancel: CancellationToken,
    ) -> Result<SlotGuard> {
        // Maintenance may hold admission before the queue owns this request.
        // Cancel here without dropping the later queue acquire, which owns its
        // queued/dequeued event pair and waiter cleanup.
        let _operation = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(crate::Error::Other("download acquire cancelled".to_owned())),
            operation = self.begin_operation() => operation?,
        };
        let events_for_queue = self.events.clone();
        // Captured by the on_queued closure so the abort-emit branch
        // below can tell whether `DownloadQueued` actually fired
        // (slow path) or not (fast path — no event was emitted, so
        // no clearance is needed either).
        let queued_emitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let queued_emitted_cb = queued_emitted.clone();
        let result = self
            .queue
            .acquire(req.clone(), cancel, move |entry| {
                queued_emitted_cb.store(true, std::sync::atomic::Ordering::SeqCst);
                events_for_queue.publish(DownloadManagerEvent::Progress(
                    DownloadProgressEvent::DownloadQueued {
                        streamer_id: entry.streamer_id.clone(),
                        streamer_name: entry.streamer_name.clone(),
                        session_id: entry.session_id.clone(),
                        engine_type: entry.engine_type,
                        is_high_priority: entry.priority.is_high(),
                        queued_at_ms: entry.queued_at_ms,
                    },
                ));
            })
            .await;

        match result {
            Ok(slot) => Ok(slot),
            Err(qerr) => {
                // If the request emitted a `DownloadQueued` and is
                // now aborting without acquiring a slot, fire a
                // `DownloadDequeued` so subscribers can clear the
                // badge.
                // `DuplicateSession` does not get a clearance —
                // there's no badge for the duplicate; the original
                // pipeline still owns it.
                if queued_emitted.load(std::sync::atomic::Ordering::SeqCst)
                    && !matches!(qerr, QueueAcquireError::DuplicateSession(_))
                {
                    self.events.publish(DownloadManagerEvent::Progress(
                        DownloadProgressEvent::DownloadDequeued {
                            streamer_id: req.streamer_id.clone(),
                            streamer_name: req.streamer_name.clone(),
                            session_id: req.session_id.clone(),
                        },
                    ));
                }
                match qerr {
                    QueueAcquireError::Cancelled => Err(crate::Error::Other(
                        "download acquire cancelled".to_string(),
                    )),
                    QueueAcquireError::DuplicateSession(s) => Err(crate::Error::Other(format!(
                        "duplicate download for session {}",
                        s
                    ))),
                    QueueAcquireError::ShuttingDown => Err(crate::Error::Other(
                        "download manager shutting down".to_string(),
                    )),
                }
            }
        }
    }

    /// Spin up the engine on the acquired slot.
    ///
    /// Generates the download id, registers the active download (which
    /// takes ownership of the slot), emits
    /// [`DownloadProgressEvent::DownloadStarted`], and spawns the
    /// engine + segment-event handler. Returns the new download id.
    pub async fn start_with_slot(
        &self,
        slot: SlotGuard,
        config: DownloadConfig,
        engine: EngineHandle,
    ) -> Result<String> {
        self.start_with_slot_cancellable(slot, config, engine, &CancellationToken::new())
            .await?
            .ok_or_else(|| crate::Error::Other("download startup cancelled".to_string()))
    }

    /// Start an acquired recording unless its session is cancelled while waiting
    /// for admission. `None` releases the slot and clears a previously queued event.
    pub(crate) async fn start_with_slot_cancellable(
        &self,
        slot: SlotGuard,
        config: DownloadConfig,
        engine: EngineHandle,
        cancel: &CancellationToken,
    ) -> Result<Option<String>> {
        struct QueuedStartCleanup<'a> {
            manager: &'a DownloadManager,
            event: Option<DownloadProgressEvent>,
        }
        impl Drop for QueuedStartCleanup<'_> {
            fn drop(&mut self) {
                if let Some(event) = self.event.take() {
                    self.manager
                        .events
                        .publish(DownloadManagerEvent::Progress(event));
                }
            }
        }
        let mut cleanup = QueuedStartCleanup {
            manager: self,
            event: slot
                .queued_event_emitted()
                .then(|| DownloadProgressEvent::DownloadDequeued {
                    streamer_id: config.streamer_id.clone(),
                    streamer_name: config.streamer_name.clone(),
                    session_id: config.session_id.clone(),
                }),
        };
        let _operation = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(None),
            operation = self.begin_operation() => operation?,
        };
        let result = self
            .start_download_with_engine_and_slot(
                config,
                engine.engine,
                engine.engine_type,
                engine.engine_key,
                slot,
            )
            .await;
        if result.is_ok() {
            cleanup.event = None;
        }
        result.map(Some)
    }

    /// Emit the cleanup event for a queued slot that was granted but
    /// then abandoned before [`Self::start_with_slot`].
    pub fn emit_dequeued_for_slot(&self, slot: &SlotGuard, streamer_id: &str, streamer_name: &str) {
        if !slot.queued_event_emitted() {
            return;
        }

        self.events.publish(DownloadManagerEvent::Progress(
            DownloadProgressEvent::DownloadDequeued {
                streamer_id: streamer_id.to_string(),
                streamer_name: streamer_name.to_string(),
                session_id: slot.session_id().to_string(),
            },
        ));
    }

    /// Internal: spin up the engine on a slot already granted by the
    /// queue, generate a download id, register the active download,
    /// emit `DownloadStarted`, and spawn the segment-event handler.
    ///
    /// Preflight (engine availability, circuit breaker, output gate,
    /// `prepare_output_dir`) is the caller's responsibility — see
    /// [`Self::preflight`]. The slot is moved into the active-downloads
    /// entry; capacity is released when the entry is removed.
    /// Stop a download.
    pub async fn stop_download(&self, download_id: &str) -> Result<()> {
        self.stop_download_with_reason(download_id, DownloadStopCause::User)
            .await
    }

    /// Stop a download with an explicit reason and wait for it to finalize.
    ///
    /// Resolves only once the attempt has published its terminal outcome, which
    /// can take the engine's whole `graceful_stop_timeout_secs`. Callers that
    /// run inside a single-consumer coordination loop must use
    /// [`Self::request_stop_download`] instead so one slow finalization does
    /// not stall every other streamer's events behind it.
    pub async fn stop_download_with_reason(
        &self,
        download_id: &str,
        cause: DownloadStopCause,
    ) -> Result<()> {
        let completion = self.request_stop(download_id, cause)?;
        completion.wait().await.map_err(|message| {
            crate::Error::Other(format!(
                "download {download_id} did not stop cleanly: {message}"
            ))
        })?;
        info!(download_id, "Stopped download");
        Ok(())
    }

    /// Ask a download to stop and return without waiting for finalization.
    ///
    /// The attempt still publishes its terminal outcome through the required
    /// coordination channel, and `shutdown_until` still joins it, so nothing is
    /// lost by not awaiting here — only the completion timing is unobserved.
    pub fn request_stop_download(&self, download_id: &str, cause: DownloadStopCause) -> Result<()> {
        self.request_stop(download_id, cause)?;
        info!(download_id, "Requested download stop");
        Ok(())
    }

    fn request_stop(
        &self,
        download_id: &str,
        cause: DownloadStopCause,
    ) -> Result<Arc<AttemptCompletion>> {
        let Some(mut download) = self.active_downloads.get_mut(download_id) else {
            return Err(crate::Error::NotFound {
                entity_type: "Download".to_string(),
                id: download_id.to_string(),
            });
        };

        let (first_request, should_cancel) = {
            let mut phase = download.phase.lock();
            match &*phase {
                AttemptPhase::Running => {
                    *phase = AttemptPhase::StopRequested(cause);
                    (true, true)
                }
                AttemptPhase::StopRequested(_) => (false, true),
                AttemptPhase::TerminalChosen => (false, false),
            }
        };

        if first_request {
            download.status = DownloadStatus::Cancelled;
            let config = download.handle.config_snapshot();
            self.events.publish(DownloadManagerEvent::Progress(
                DownloadProgressEvent::Progress {
                    download_id: download_id.to_string(),
                    streamer_id: config.streamer_id,
                    streamer_name: config.streamer_name,
                    session_id: config.session_id,
                    status: DownloadStatus::Cancelled,
                    progress: download.progress.clone(),
                },
            ));
        }

        if should_cancel {
            download.handle.cancel();
        }
        Ok(download.completion.clone())
    }

    /// Get information about active downloads.
    pub fn get_active_downloads(&self) -> Vec<DownloadInfo> {
        self.active_downloads
            .iter()
            .map(|entry| DownloadInfo::from(entry.value()))
            .collect()
    }

    /// Get the number of active downloads.
    pub fn active_count(&self) -> usize {
        self.active_downloads.len()
    }

    /// Admit database maintenance without waiting for recordings or in-flight
    /// admission work. Holding the guard prevents new recordings from starting;
    /// active attempts can finish and release their entries independently.
    pub(crate) fn try_admit_maintenance(
        &self,
        max_active_downloads: usize,
    ) -> Option<tokio::sync::RwLockWriteGuard<'_, ()>> {
        let guard = self.operation_gate.try_write().ok()?;
        if !self.accepting_operations.load(Ordering::Acquire)
            || self.active_count() > max_active_downloads
        {
            return None;
        }
        Some(guard)
    }

    /// Snapshot of currently-pending acquires (downloads that emitted
    /// [`DownloadProgressEvent::DownloadQueued`] but have not yet
    /// transitioned to [`DownloadProgressEvent::DownloadStarted`]).
    pub fn snapshot_pending(&self) -> Vec<QueuePendingEntry> {
        self.queue.snapshot_pending()
    }

    /// Number of queued acquires waiting for a slot. Allocation-free
    /// counterpart to [`Self::snapshot_pending`] — use this when only
    /// the count matters (health check, metrics) and avoid cloning the
    /// pending entries.
    pub fn pending_count(&self) -> usize {
        self.queue.pending_count()
    }

    /// Mark the queue as shutting down. Subsequent acquires fail with
    /// `AcquireError::ShuttingDown`; pending acquires are notified and
    /// return the same error rather than waiting for a slot.
    ///
    /// Runtime shutdown uses [`Self::shutdown_until`] to fence admission and
    /// drain the active attempts after closing the queue.
    pub fn shutdown_queue(&self) {
        self.queue.shutdown();
    }

    /// Maximum normal-priority concurrent downloads.
    pub fn max_concurrent_downloads(&self) -> usize {
        self.config.read().max_concurrent_downloads
    }

    /// Extra slots reserved for high-priority downloads.
    pub fn high_priority_extra_slots(&self) -> usize {
        self.config.read().high_priority_extra_slots
    }

    /// Total concurrent download slots (normal + high priority extra).
    pub fn total_concurrent_slots(&self) -> usize {
        let config = self.config.read();
        config
            .max_concurrent_downloads
            .saturating_add(config.high_priority_extra_slots)
    }

    /// Adjust the configured normal-priority concurrency limit at runtime.
    /// A temporary throttle is reapplied to this new base before publishing queue capacity.
    ///
    /// Increasing capacity wakes any waiters that fit. Decreasing keeps
    /// in-flight downloads running until they release naturally; new
    /// acquires beyond the new limit queue.
    pub fn set_max_concurrent_downloads(&self, limit: usize) -> usize {
        let limit = limit.max(1);
        let factor = self.throttle_factor.lock();
        let mut config = self.config.write();
        config.max_concurrent_downloads = limit;
        self.apply_download_capacity(&config, *factor);
        limit
    }

    /// Set/release the pipeline's temporary reduction without changing configured capacity.
    /// Returns the configured and effective normal limits from the same mutation.
    pub(crate) fn set_download_throttle(&self, factor: Option<f32>) -> (usize, usize) {
        let mut throttle = self.throttle_factor.lock();
        *throttle = factor.map(|factor| {
            if factor.is_finite() {
                factor.clamp(0.0, 1.0)
            } else {
                1.0
            }
        });
        let config = self.config.read();
        let applied = self.apply_download_capacity(&config, *throttle);
        (config.max_concurrent_downloads, applied)
    }

    /// Caller holds throttle_factor and config, in that order, until publication completes.
    fn apply_download_capacity(
        &self,
        config: &DownloadManagerConfig,
        factor: Option<f32>,
    ) -> usize {
        let configured = config.max_concurrent_downloads;
        let effective = factor.map_or(configured, |factor| {
            ((configured as f64 * f64::from(factor)) as usize).clamp(1, configured)
        });
        if let Some(feedback) = self.events.feedback.get() {
            feedback.ensure_attempt_capacity(
                configured.saturating_add(config.high_priority_extra_slots),
            );
        }
        self.queue.set_normal_capacity(effective)
    }

    /// Current queue-wait freshness threshold in milliseconds.
    ///
    /// When a queued download has waited longer than this, the
    /// per-streamer pipeline re-checks the streamer with the monitor
    /// service to refresh stream URLs/headers before starting the
    /// engine. Below the threshold, the URLs captured at the original
    /// live event are reused.
    pub fn queue_freshness_threshold_ms(&self) -> i64 {
        self.queue_freshness_threshold_ms.load(Ordering::Relaxed)
    }

    /// Set the queue-wait freshness threshold in milliseconds at
    /// runtime. Negative values are clamped to zero (which means
    /// "always refetch on a wait"). Returns the applied value.
    pub fn set_queue_freshness_threshold_ms(&self, ms: i64) -> i64 {
        let clamped = ms.max(0);
        self.queue_freshness_threshold_ms
            .store(clamped, Ordering::Relaxed);
        clamped
    }

    /// Subscribe to download events.
    ///
    /// Returns a broadcast receiver that will receive all download events.
    /// Multiple subscribers can receive the same events concurrently.
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadManagerEvent> {
        self.events.subscribe()
    }

    /// Emit [`DownloadTerminalEvent::Rejected`] — the single construction
    /// point for rejection events. [`Self::preflight`] routes its circuit-
    /// breaker and output-root rejections through here, and callers that
    /// refuse a download before `preflight` runs (the container's
    /// temporarily-disabled gate) use it too, so every rejection consumer
    /// sees one shape: the session lifecycle ends the just-created/resumed
    /// session (`TerminalCause::Rejected` is an authoritative end), and the
    /// scheduler reschedules the streamer actor after `retry_after_secs`
    /// instead of leaving it parked in Live with no download and no
    /// `DownloadEnded` ever arriving.
    ///
    /// Parameter order mirrors the [`DownloadTerminalEvent::Rejected`]
    /// field order.
    pub async fn emit_rejected(
        &self,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        reason: String,
        retry_after_secs: Option<u64>,
        kind: DownloadRejectedKind,
    ) -> Result<()> {
        let _operation = self.begin_operation().await?;
        self.emit_rejected_admitted(
            streamer_id,
            streamer_name,
            session_id,
            reason,
            retry_after_secs,
            kind,
        )
        .await
    }

    async fn emit_rejected_admitted(
        &self,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        reason: String,
        retry_after_secs: Option<u64>,
        kind: DownloadRejectedKind,
    ) -> Result<()> {
        let feedback_streamer_id = streamer_id.clone();
        self.events
            .publish_and_wait(DownloadManagerEvent::Terminal(
                DownloadTerminalEvent::Rejected {
                    streamer_id,
                    streamer_name,
                    session_id,
                    reason,
                    retry_after_secs,
                    kind,
                },
            ))
            .await
            .map_err(|error| match error {
                coordination::PublicationError::FeedbackBusy => {
                    crate::Error::SchedulerFeedbackBusy {
                        streamer_id: feedback_streamer_id,
                    }
                }
                coordination::PublicationError::Coordination(message) => {
                    crate::Error::Other(message)
                }
            })
    }

    async fn begin_operation(&self) -> Result<tokio::sync::RwLockReadGuard<'_, ()>> {
        if !self.accepting_operations.load(Ordering::Acquire) {
            return Err(crate::Error::Other(
                "download manager shutting down".to_string(),
            ));
        }
        let guard = self.operation_gate.read().await;
        if !self.accepting_operations.load(Ordering::Acquire) {
            return Err(crate::Error::Other(
                "download manager shutting down".to_string(),
            ));
        }
        Ok(guard)
    }

    /// Get download by streamer ID.
    pub fn get_download_by_streamer(&self, streamer_id: &str) -> Option<DownloadInfo> {
        self.active_downloads
            .iter()
            // Compare under the config read lock; `config_snapshot()` would
            // deep-clone the whole `DownloadConfig` per scanned entry.
            .find(|entry| entry.value().handle.config.read().streamer_id == streamer_id)
            .map(|entry| DownloadInfo::from(entry.value()))
    }

    /// Check if a streamer has an active download.
    ///
    /// Only considers downloads with status Starting or Downloading as active.
    /// Failed, Completed, or Cancelled downloads are not considered active,
    /// preventing race conditions where a failed download blocks new attempts.
    pub fn has_active_download(&self, streamer_id: &str) -> bool {
        self.active_downloads.iter().any(|entry| {
            let download = entry.value();
            // Read the id under the config lock instead of cloning a full
            // `DownloadConfig` snapshot per entry.
            download.handle.config.read().streamer_id == streamer_id
                && matches!(
                    download.status,
                    DownloadStatus::Starting | DownloadStatus::Downloading
                )
        })
    }

    /// Fence new attempts, request a graceful stop for every active download,
    /// and join all engine/translator tasks against one absolute deadline.
    pub async fn shutdown_until(&self, deadline: tokio::time::Instant) -> DownloadShutdownReport {
        self.accepting_operations.store(false, Ordering::Release);
        self.attempts.close_admission();
        self.queue.shutdown();

        let (operation_fence, admission_deadline_exceeded) =
            match tokio::time::timeout_at(deadline, self.operation_gate.write()).await {
                Ok(guard) => (guard, false),
                Err(_) => {
                    warn!(
                        "Download operation grace period exceeded; awaiting admission containment"
                    );
                    // Queue shutdown wakes parked acquires and the admission flag
                    // rejects any second-phase start. Keep ownership here until
                    // every operation that crossed the fence has actually left;
                    // otherwise the required-event shutdown marker could race a
                    // late segment producer.
                    (self.operation_gate.write().await, true)
                }
            };

        let mut stopped_download_ids = self
            .active_downloads
            .iter()
            .map(|entry| entry.key().clone())
            .collect::<Vec<_>>();
        stopped_download_ids.sort();

        for download_id in &stopped_download_ids {
            // Publish the budget before requesting the stop so the engine's
            // cancellation branch clamps its `graceful_stop_timeout_secs`
            // against this deadline rather than outliving the whole shutdown.
            if let Some(download) = self.active_downloads.get(download_id) {
                download.handle.set_stop_deadline(deadline);
            }
            if let Err(error) = self.request_stop(download_id, DownloadStopCause::Shutdown) {
                debug!(%download_id, %error, "Download finished while shutdown was requesting stop");
            }
        }
        drop(operation_fence);

        let attempt_report = self.attempts.join_until(deadline).await;

        let mut failures = attempt_report.failures;
        let mut overruns = attempt_report.overruns;
        if admission_deadline_exceeded {
            overruns.push(
                "graceful download operation drain deadline exceeded; admitted operations were contained to completion"
                    .to_string(),
            );
        }
        let coordination_drained =
            match tokio::time::timeout_at(deadline, self.events.shutdown_coordination()).await {
                Ok(Ok(())) => true,
                Ok(Err(message)) => {
                    failures.push(format!(
                        "required download coordination drain failed: {message}"
                    ));
                    false
                }
                Err(_) => {
                    failures.push(
                        "deadline exceeded while draining required download events".to_string(),
                    );
                    false
                }
            };

        if let Some(feedback) = self.events.feedback.get() {
            feedback.shutdown().await;
        }

        DownloadShutdownReport {
            stopped_download_ids,
            deadline_exceeded_download_ids: attempt_report.deadline_exceeded_download_ids,
            coordination_drained,
            runtime_failures: attempt_report.runtime_failures,
            failures,
            overruns,
        }
    }

    /// Fence new attempts and abort the running ones instead of joining them.
    ///
    /// [`Self::shutdown_until`] contains attempts past its deadline, so an
    /// engine that never returns keeps it pending. Callers that must stop
    /// waiting use this: aborting drops each attempt future and kills the
    /// engine child it owns through `kill_on_drop`. Attempts that do not
    /// settle by `deadline` remain owned by the attempt supervisor.
    ///
    /// Returns the download ids that were still running.
    pub(crate) async fn abort_attempts(&self, deadline: tokio::time::Instant) -> Vec<String> {
        self.accepting_operations.store(false, Ordering::Release);
        self.queue.shutdown();
        let downloads = self.attempts.abort_running(deadline).await;
        if let Some(feedback) = self.events.feedback.get() {
            feedback.abort(deadline).await;
        }
        downloads
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}
