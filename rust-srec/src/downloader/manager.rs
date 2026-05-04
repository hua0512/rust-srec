//! Download Manager implementation.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::engine::{
    DownloadConfig, DownloadEngine, DownloadFailureKind, DownloadHandle, DownloadInfo,
    DownloadProgress, DownloadStatus, EngineStartError, EngineType, FfmpegEngine, IoErrorKindSer,
    MesioEngine, SegmentEvent, StreamlinkEngine,
};
use super::output_root_gate::OutputRootGate;
use super::queue::{
    AcquireError as QueueAcquireError, AcquireRequest, ActiveSlot, DownloadQueue,
    PendingEntry as QueuePendingEntry, Priority, SlotGuard,
};
use super::resilience::{CircuitBreakerManager, EngineKey, RetryConfig};
use crate::Result;
use crate::database::models::engine::{
    FfmpegEngineConfig, MesioEngineConfig, StreamlinkEngineConfig,
};
use crate::database::repositories::config::ConfigRepository;
use crate::downloader::SegmentInfo;

fn parse_engine_config<T: DeserializeOwned>(engine: &'static str, raw: &str) -> Result<T> {
    serde_json::from_str(raw)
        .map_err(|e| crate::Error::Other(format!("Failed to parse {} config: {}", engine, e)))
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

// The per-tier concurrency primitives previously implemented here have
// been replaced by [`DownloadQueue`] (`super::queue`), which subsumes
// both the normal and high-priority pools, supports priority-aware
// wakeup, and exposes the pending-set used by the WebSocket snapshot.
// The struct used to be `ConcurrencyLimit`; callers now interact with
// `self.queue` directly.

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
    status: DownloadStatus,
    progress: DownloadProgress,
    #[allow(dead_code)]
    is_high_priority: bool,
    /// Last known output path (from segments)
    pub output_path: Option<String>,
    current_segment_index: Option<u32>,
    current_segment_path: Option<String>,
    current_segment_started_at: Option<DateTime<Utc>>,
    /// Queue slot held by the download. Released (and the next waiter
    /// woken) when this `ActiveDownload` entry is removed from the
    /// active map.
    #[allow(dead_code)]
    slot: Option<ActiveSlot>,
    /// Retry configuration override applied via config update.
    retry_config_override: Option<RetryConfig>,
}

/// The Download Manager service.
pub struct DownloadManager {
    /// Configuration.
    config: RwLock<DownloadManagerConfig>,
    /// Priority-aware queue managing concurrency across both
    /// normal-priority and high-priority extra slots.
    queue: Arc<DownloadQueue>,
    /// Active downloads.
    active_downloads: Arc<DashMap<String, ActiveDownload>>,
    /// Pending configuration updates keyed by download_id.
    pending_updates: Arc<DashMap<String, PendingConfigUpdate>>,
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

/// How the engine observed the end of a stream when emitting
/// [`DownloadTerminalEvent::Completed`].
///
/// Different signals carry different confidence about whether the upstream
/// stream is *actually* over vs. whether we just got disconnected and the
/// streamer will reappear with a fresh URL.
///
/// Consumers (today: [`crate::session::SessionLifecycle`]) use this to choose
/// between firing the session-complete pipeline immediately and entering a
/// hysteresis quiet-period that absorbs reconnects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EngineEndSignal {
    /// HLS playlist contained `#EXT-X-ENDLIST`. The platform itself marked
    /// the stream as complete — definitively over.
    HlsEndlist,
    /// Connection closed cleanly with no explicit end marker. Could be EOF,
    /// could be a reconnect-friendly drop. Ambiguous; the lifecycle should
    /// hold the session in hysteresis to absorb a possible resume.
    ///
    /// Used by mesio FLV (TCP close), mesio HLS without `#EXT-X-ENDLIST`,
    /// and any other engine that observes a clean disconnect without a
    /// platform-asserted end marker.
    CleanDisconnect,
    /// Subprocess (ffmpeg / streamlink) exited with status 0. Ambiguous —
    /// could be EOF, could be the process being killed cleanly externally.
    SubprocessExitZero,
    /// Engine doesn't expose a finer signal. Treat as non-authoritative
    /// (default for back-fill / unknown engines).
    #[default]
    Unknown,
}

impl EngineEndSignal {
    /// Whether this signal alone is sufficient to mark the stream as done
    /// without waiting for hysteresis. Today, only HLS `#EXT-X-ENDLIST` is.
    pub fn is_authoritative(&self) -> bool {
        matches!(self, Self::HlsEndlist)
    }

    /// Short, stable label for logging / metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::HlsEndlist => "hls_endlist",
            Self::CleanDisconnect => "clean_disconnect",
            Self::SubprocessExitZero => "subprocess_exit_zero",
            Self::Unknown => "unknown",
        }
    }
}

/// Reason why a download was stopped.
///
/// Used to disambiguate user cancellation from internal orchestration stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStopCause {
    /// User explicitly cancelled the download (typically implies "stop monitoring").
    User,
    /// Stream was determined to be offline (end-of-stream).
    StreamerOffline,
    /// Danmu stream emitted `StreamClosed` and we stop the download promptly.
    DanmuStreamClosed,
    /// The streamer is live but recording is no longer allowed (schedule window ended).
    OutOfSchedule,
    /// Streamer was disabled/deleted; downloads are stopped as part of cleanup.
    StreamerDisabled,
    /// Application shutdown.
    Shutdown,
    /// Other internal/system stop reason.
    Other(String),
}

impl DownloadStopCause {
    pub fn as_str(&self) -> &str {
        match self {
            Self::User => "user",
            Self::StreamerOffline => "streamer_offline",
            Self::DanmuStreamClosed => "danmu_stream_closed",
            Self::OutOfSchedule => "out_of_schedule",
            Self::StreamerDisabled => "streamer_disabled",
            Self::Shutdown => "shutdown",
            Self::Other(_) => "other",
        }
    }
}

/// Events emitted by the Download Manager.
///
/// Events are grouped into two categories:
///
/// - [`DownloadProgressEvent`]: in-flight notifications about an ongoing
///   download (start, progress, segment lifecycle, config changes). These do
///   not mark a session as finished.
/// - [`DownloadTerminalEvent`]: the download has stopped and no further events
///   for the same `(download_id, session_id)` pair will be emitted. Consumers
///   that need to react to "session ended" should match on this variant and
///   use [`DownloadTerminalEvent::should_run_session_complete_pipeline`] to
///   decide whether to fire the post-recording pipeline.
///
/// The split exists to make session-termination a first-class, non-droppable
/// signal: Rust's exhaustive pattern matching forces every consumer to make
/// an explicit decision for every terminal variant. A silent `_ => {}` on the
/// previous flat enum is how #520 (pipeline not running on `DownloadFailed`)
/// went unnoticed since the feature landed in PR #187.
#[derive(Debug, Clone)]
pub enum DownloadManagerEvent {
    Progress(DownloadProgressEvent),
    Terminal(DownloadTerminalEvent),
}

/// Non-terminal download notifications.
#[derive(Debug, Clone)]
pub enum DownloadProgressEvent {
    /// Download is parked waiting for a concurrency slot. Emitted only
    /// when the request had to wait (fast-path acquires emit
    /// [`Self::DownloadStarted`] directly with no preceding `DownloadQueued`).
    /// Cleared by a subsequent `DownloadStarted` for the same session
    /// when the engine starts, or by [`Self::DownloadDequeued`] if
    /// the pipeline aborts before then.
    DownloadQueued {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        engine_type: EngineType,
        is_high_priority: bool,
        queued_at_ms: i64,
    },
    /// Cleanup signal emitted when a previously-queued pipeline
    /// aborts before starting a download (cancellation, shutdown,
    /// freshness/state re-check, etc.).
    /// Not emitted for the fast-path acquire (which never emitted
    /// `DownloadQueued`). Frontend clears the streamer's "queued"
    /// badge on receipt; no scheduler action is taken — the
    /// streamer's actual state machine is driven elsewhere.
    DownloadDequeued {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
    },
    /// Download started.
    DownloadStarted {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        engine_type: EngineType,
        cdn_host: String,
        download_url: String,
    },
    /// Progress update for a download.
    Progress {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        status: DownloadStatus,
        progress: DownloadProgress,
    },
    /// Segment started - a new segment file has been opened for writing.
    SegmentStarted {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        segment_path: String,
        segment_index: u32,
        started_at: DateTime<Utc>,
    },
    /// Segment completed.
    SegmentCompleted {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        segment_path: String,
        segment_index: u32,
        started_at: Option<DateTime<Utc>>,
        completed_at: DateTime<Utc>,
        duration_secs: f64,
        size_bytes: u64,
        split_reason_code: Option<String>,
        split_reason_details_json: Option<String>,
    },
    /// Configuration was updated for a download.
    ConfigUpdated {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        update_type: ConfigUpdateType,
    },
    /// Configuration update failed to apply.
    ConfigUpdateFailed {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        error: String,
    },
}

/// Terminal download notifications: the download has stopped, no further
/// events for the same `(download_id, session_id)` pair will be emitted.
///
/// Consumers that need to react to "session ended" (notifications, pipeline
/// scheduling, DB status updates, …) should centralise on this enum and call
/// [`Self::should_run_session_complete_pipeline`] rather than re-deriving
/// the policy per site.
#[derive(Debug, Clone)]
pub enum DownloadTerminalEvent {
    /// Download completed normally — all segments flushed, outputs finalised.
    ///
    /// Important: a `Completed` event is *not* by itself authoritative
    /// proof that the upstream stream is over. A clean TCP close or a
    /// subprocess exiting with status 0 can mean either "EOF" or "we
    /// reconnected." Engines pass an [`EngineEndSignal`] hint with the
    /// event so [`crate::session::SessionLifecycle`] can tell HLS
    /// `#EXT-X-ENDLIST` (definitively over) apart from FLV clean
    /// disconnect (might be a reconnect-friendly drop).
    Completed {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        total_bytes: u64,
        total_duration_secs: f64,
        total_segments: u32,
        file_path: Option<String>,
        /// How the engine observed the end. Lifecycle reads this to decide
        /// whether to enter hysteresis (clean disconnect / subprocess exit
        /// → ambiguous) or commit Ended directly (HLS endlist → authoritative).
        engine_signal: EngineEndSignal,
    },
    /// Download failed — the engine gave up. Whatever output is on disk is
    /// final; no more segments will arrive.
    Failed {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        kind: DownloadFailureKind,
        error: String,
        recoverable: bool,
    },
    /// Download cancelled — stop requested externally (e.g. user, streamer
    /// disabled, shutdown). A final `Completed` may still arrive once the
    /// engine flushes the in-flight segment, so this variant is not treated
    /// as "session complete" by default.
    Cancelled {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        cause: DownloadStopCause,
    },
    /// Download was rejected before starting (e.g., circuit breaker open,
    /// output-root filesystem unwritable).
    ///
    /// Unlike [`Self::Failed`], this indicates the download never started.
    /// No `download_id` is available because the download was never created.
    Rejected {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        reason: String,
        /// How long to wait before retrying (cooldown of whichever subsystem
        /// rejected the download).
        retry_after_secs: Option<u64>,
        /// Why the download was rejected. Carries the payload the scheduler
        /// needs to route this to the correct [`DownloadEndPolicy`] variant
        /// and, ultimately, the correct [`crate::monitor::InfraBlockReason`].
        kind: DownloadRejectedKind,
    },
}

impl DownloadManagerEvent {
    /// Streamer id shared across both progress and terminal event shapes.
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::Progress(p) => p.streamer_id(),
            Self::Terminal(t) => t.streamer_id(),
        }
    }

    /// Streamer display name shared across both shapes.
    pub fn streamer_name(&self) -> &str {
        match self {
            Self::Progress(p) => p.streamer_name(),
            Self::Terminal(t) => t.streamer_name(),
        }
    }

    /// Recording session id. Present on every variant.
    pub fn session_id(&self) -> &str {
        match self {
            Self::Progress(p) => p.session_id(),
            Self::Terminal(t) => t.session_id(),
        }
    }
}

impl DownloadProgressEvent {
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::DownloadQueued { streamer_id, .. }
            | Self::DownloadDequeued { streamer_id, .. }
            | Self::DownloadStarted { streamer_id, .. }
            | Self::Progress { streamer_id, .. }
            | Self::SegmentStarted { streamer_id, .. }
            | Self::SegmentCompleted { streamer_id, .. }
            | Self::ConfigUpdated { streamer_id, .. }
            | Self::ConfigUpdateFailed { streamer_id, .. } => streamer_id,
        }
    }

    pub fn streamer_name(&self) -> &str {
        match self {
            Self::DownloadQueued { streamer_name, .. }
            | Self::DownloadDequeued { streamer_name, .. }
            | Self::DownloadStarted { streamer_name, .. }
            | Self::Progress { streamer_name, .. }
            | Self::SegmentStarted { streamer_name, .. }
            | Self::SegmentCompleted { streamer_name, .. }
            | Self::ConfigUpdated { streamer_name, .. }
            | Self::ConfigUpdateFailed { streamer_name, .. } => streamer_name,
        }
    }

    /// `session_id` is always present on live download events, but `ConfigUpdated`
    /// and `ConfigUpdateFailed` carry none today (they're scoped to a
    /// `download_id`). Returns an empty string for those variants — callers
    /// that need a real session id should only call this on variants that
    /// have one.
    pub fn session_id(&self) -> &str {
        match self {
            Self::DownloadQueued { session_id, .. }
            | Self::DownloadDequeued { session_id, .. }
            | Self::DownloadStarted { session_id, .. }
            | Self::Progress { session_id, .. }
            | Self::SegmentStarted { session_id, .. }
            | Self::SegmentCompleted { session_id, .. } => session_id,
            Self::ConfigUpdated { .. } | Self::ConfigUpdateFailed { .. } => "",
        }
    }
}

impl DownloadTerminalEvent {
    pub fn streamer_id(&self) -> &str {
        match self {
            Self::Completed { streamer_id, .. }
            | Self::Failed { streamer_id, .. }
            | Self::Cancelled { streamer_id, .. }
            | Self::Rejected { streamer_id, .. } => streamer_id,
        }
    }

    pub fn streamer_name(&self) -> &str {
        match self {
            Self::Completed { streamer_name, .. }
            | Self::Failed { streamer_name, .. }
            | Self::Cancelled { streamer_name, .. }
            | Self::Rejected { streamer_name, .. } => streamer_name,
        }
    }

    pub fn session_id(&self) -> &str {
        match self {
            Self::Completed { session_id, .. }
            | Self::Failed { session_id, .. }
            | Self::Cancelled { session_id, .. }
            | Self::Rejected { session_id, .. } => session_id,
        }
    }

    /// `download_id` is present for every terminal variant except
    /// [`Self::Rejected`] (rejection happens before the download is created).
    pub fn download_id(&self) -> Option<&str> {
        match self {
            Self::Completed { download_id, .. }
            | Self::Failed { download_id, .. }
            | Self::Cancelled { download_id, .. } => Some(download_id),
            Self::Rejected { .. } => None,
        }
    }

    /// Whether this termination represents the recording session reaching a
    /// final state with outputs on disk that are ready for post-processing.
    ///
    /// - [`Self::Completed`]: `true` — normal end, outputs finalised.
    /// - [`Self::Failed`]: `true` — the engine gave up; whatever's on disk
    ///   is final. Prior to this method existing, sessions that ended this
    ///   way silently skipped the session-complete pipeline.
    /// - [`Self::Cancelled`]: `false` — cancellation is a stop *request*; a
    ///   `Completed` may still arrive once the engine flushes the final
    ///   segment. Firing the pipeline early would cause missing inputs.
    /// - [`Self::Rejected`]: `false` — the download never started, no
    ///   outputs exist.
    pub fn should_run_session_complete_pipeline(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

/// Reason a [`DownloadManagerEvent::DownloadRejected`] event was emitted.
///
/// Distinct from the free-form `reason` string because the scheduler needs
/// structured data to decide which state to transition the streamer into —
/// circuit-breaker blocks go to `TemporalDisabled`, gate blocks go to
/// `OutOfSpace`.
#[derive(Debug, Clone)]
pub enum DownloadRejectedKind {
    /// Engine circuit breaker is open for the resolved engine.
    CircuitBreaker,
    /// Output-root write gate has the target filesystem in the Degraded state.
    /// The payload mirrors the gate's `GateBlocked` so the scheduler can
    /// construct an [`crate::monitor::InfraBlockReason::OutputRootUnavailable`].
    OutputRootUnavailable {
        path: std::path::PathBuf,
        io_kind: IoErrorKindSer,
    },
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
    pub fn with_config(config: DownloadManagerConfig) -> Self {
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
            queue,
            active_downloads: Arc::new(DashMap::new()),
            pending_updates: Arc::new(DashMap::new()),
            engines: RwLock::new(HashMap::new()),
            circuit_breakers,
            output_root_gate: OnceLock::new(),
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
    /// [`crate::services::container`] when the download manager is already
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
    ///
    /// Convenience wrapper that runs the three-phase split pipeline
    /// (`preflight` → `acquire_slot` → `start_with_slot`) sequentially
    /// with no cancellation hook and no per-phase visibility. Used by
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

    /// Phase 1: pre-acquire validation.
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

            let _ = self.event_tx.send(DownloadManagerEvent::Terminal(
                DownloadTerminalEvent::Rejected {
                    streamer_id: req.streamer_id.clone(),
                    streamer_name: req.streamer_name.clone(),
                    session_id: req.session_id.clone(),
                    reason: format!("Circuit breaker open for engine {}", engine_key),
                    retry_after_secs: Some(self.config.read().circuit_breaker_cooldown_secs),
                    kind: DownloadRejectedKind::CircuitBreaker,
                },
            ));

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
            let _ = self.event_tx.send(DownloadManagerEvent::Terminal(
                DownloadTerminalEvent::Rejected {
                    streamer_id: req.streamer_id.clone(),
                    streamer_name: req.streamer_name.clone(),
                    session_id: req.session_id.clone(),
                    reason: blocked.to_string(),
                    retry_after_secs: Some(cooldown),
                    kind: DownloadRejectedKind::OutputRootUnavailable {
                        path: blocked.root.clone(),
                        io_kind: blocked.kind,
                    },
                },
            ));
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
                let _ = self.event_tx.send(DownloadManagerEvent::Terminal(
                    DownloadTerminalEvent::Rejected {
                        streamer_id: req.streamer_id.clone(),
                        streamer_name: req.streamer_name.clone(),
                        session_id: req.session_id.clone(),
                        reason: engine_err.message.clone(),
                        retry_after_secs: Some(super::output_root_gate::DEFAULT_GATE_COOLDOWN_SECS),
                        kind: DownloadRejectedKind::OutputRootUnavailable { path, io_kind },
                    },
                ));
            }
            return Err(crate::Error::Other(engine_err.message));
        }

        Ok(EngineHandle {
            engine,
            engine_type,
            engine_key,
        })
    }

    /// Phase 2: park on the priority-aware queue until a slot is
    /// available.
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
        let event_tx_for_queue = self.event_tx.clone();
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
                let _ = event_tx_for_queue.send(DownloadManagerEvent::Progress(
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
                    let _ = self.event_tx.send(DownloadManagerEvent::Progress(
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

    /// Phase 3: spin up the engine on the slot acquired in phase 2.
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
        self.start_download_with_engine_and_slot(
            config,
            engine.engine,
            engine.engine_type,
            engine.engine_key,
            slot,
        )
        .await
    }

    /// Emit the cleanup event for a queued slot that was granted but
    /// then abandoned before [`Self::start_with_slot`].
    pub fn emit_dequeued_for_slot(&self, slot: &SlotGuard, streamer_id: &str, streamer_name: &str) {
        if !slot.queued_event_emitted() {
            return;
        }

        let _ = self.event_tx.send(DownloadManagerEvent::Progress(
            DownloadProgressEvent::DownloadDequeued {
                streamer_id: streamer_id.to_string(),
                streamer_name: streamer_name.to_string(),
                session_id: slot.session_id().to_string(),
            },
        ));
    }

    /// Prepare the output directory before starting an engine.
    ///
    /// This replaces the `ensure_output_dir` call that used to live inside
    /// each engine's `start()`. Centralizing it here means:
    ///
    /// - One call site for the filesystem side-effect, which in turn means
    ///   one place to wire the output-root write gate (record failures,
    ///   mark successful recoveries).
    /// - Consistent error classification via the (now-correct)
    ///   `EngineStartError::from(crate::Error)` impl that walks the error
    ///   source chain for `io::ErrorKind`.
    /// - Engines stop depending on `crate::utils::fs` and
    ///   `crate::downloader::engine::utils::ensure_output_dir`.
    ///
    /// Behaviour:
    ///
    /// 1. Call the real `ensure_output_dir` (tokio `create_dir_all`).
    /// 2. On `Ok`: if a gate is attached, call `mark_healthy` — idempotent,
    ///    a no-op unless the root was previously in Degraded state, so the
    ///    happy path pays only one `DashMap::get` for untracked roots.
    /// 3. On `Err`: if a gate is attached, walk the `crate::Error` source
    ///    chain to find the underlying `io::Error` and feed it to
    ///    `record_failure` before propagating. Emits at most one
    ///    `OutputPathInaccessible` notification per `Healthy → Degraded`
    ///    transition; idempotent for subsequent failures on an already
    ///    Degraded root.
    #[cfg(test)]
    async fn prepare_output_dir(
        &self,
        config: &DownloadConfig,
    ) -> std::result::Result<(), EngineStartError> {
        self.prepare_output_dir_for_path(&config.output_dir).await
    }

    /// Path-only variant of [`Self::prepare_output_dir`] used by
    /// [`Self::preflight`]. Same behaviour, just doesn't require a
    /// fully-built `DownloadConfig`.
    async fn prepare_output_dir_for_path(
        &self,
        output_dir: &std::path::Path,
    ) -> std::result::Result<(), EngineStartError> {
        match super::engine::utils::ensure_output_dir(output_dir).await {
            Ok(()) => {
                if let Some(gate) = self.output_root_gate.get() {
                    gate.mark_healthy(output_dir);
                }
                Ok(())
            }
            Err(crate_err) => {
                if let Some(gate) = self.output_root_gate.get()
                    && let Some(io_err) = io_error_in_chain(&crate_err)
                {
                    gate.record_failure(output_dir, io_err);
                }
                Err(EngineStartError::from(crate_err))
            }
        }
    }

    /// Resolve engine to use.
    ///
    /// If an override value is provided for the resolved engine ID (either passed ID or global default),
    /// a new engine instance is created with the merged configuration.
    /// Otherwise, the shared cached engine instance is returned.
    ///
    /// Returns (Engine instance, EngineType, EngineKey).
    async fn resolve_engine(
        &self,
        engine_id: Option<&str>,
        overrides: Option<&serde_json::Value>,
    ) -> Result<(Arc<dyn DownloadEngine>, EngineType, EngineKey)> {
        let default_engine = self.config.read().default_engine;
        let target_id = engine_id.unwrap_or(default_engine.as_str());

        // 1. Check for overrides first
        let specific_override = overrides.and_then(|o| o.get(target_id));

        // If we have an override, we MUST create a new engine instance
        // We cannot reuse the shared engine because it has different config
        if let Some(override_config) = specific_override {
            debug!("Applying engine override for {}", target_id);
            let override_hash = Self::hash_override(override_config);

            // Resolve engine type from ID string or DB lookup
            let engine_type = self.resolve_engine_type(target_id).await?;
            let key = EngineKey::with_override(engine_type, engine_id, override_hash);

            let engine: Arc<dyn DownloadEngine> = match engine_type {
                EngineType::Ffmpeg => {
                    let base_config = self
                        .load_engine_config_or_default::<FfmpegEngineConfig>(target_id)
                        .await;
                    let merged_config =
                        Self::apply_override_best_effort(base_config, override_config);
                    Arc::new(FfmpegEngine::with_config(merged_config))
                }
                EngineType::Streamlink => {
                    let base_config = self
                        .load_engine_config_or_default::<StreamlinkEngineConfig>(target_id)
                        .await;
                    let merged_config =
                        Self::apply_override_best_effort(base_config, override_config);
                    Arc::new(StreamlinkEngine::with_config(merged_config))
                }
                EngineType::Mesio => {
                    let base_config = self
                        .load_engine_config_or_default::<MesioEngineConfig>(target_id)
                        .await;
                    let merged_config =
                        Self::apply_override_best_effort(base_config, override_config);
                    Arc::new(MesioEngine::with_config(merged_config))
                }
            };

            return Ok((engine, engine_type, key));
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
                match repo.get_engine_config(id).await {
                    Ok(config) => {
                        let engine_type =
                            config.engine_type.parse::<EngineType>().map_err(|_| {
                                crate::Error::Other(format!(
                                    "Unknown engine type in config: {}",
                                    config.engine_type
                                ))
                            })?;

                        let key = EngineKey::custom(engine_type, id);
                        let engine: Arc<dyn DownloadEngine> = match engine_type {
                            EngineType::Ffmpeg => {
                                let engine_config: FfmpegEngineConfig =
                                    parse_engine_config("ffmpeg", &config.config)?;
                                Arc::new(FfmpegEngine::with_config(engine_config))
                            }
                            EngineType::Streamlink => {
                                let engine_config: StreamlinkEngineConfig =
                                    parse_engine_config("streamlink", &config.config)?;
                                Arc::new(StreamlinkEngine::with_config(engine_config))
                            }
                            EngineType::Mesio => {
                                let engine_config: MesioEngineConfig =
                                    parse_engine_config("mesio", &config.config)?;
                                Arc::new(MesioEngine::with_config(engine_config))
                            }
                        };

                        return Ok((engine, engine_type, key));
                    }
                    Err(_) => {
                        warn!("Engine config {} not found, using default", id);
                    }
                }
            }
        }

        // Return default
        let engine = self.get_engine(default_engine).ok_or_else(|| {
            crate::Error::Other(format!("Default engine {} not registered", default_engine))
        })?;
        let key = EngineKey::global(default_engine);
        Ok((engine, default_engine, key))
    }

    async fn load_engine_config_or_default<T>(&self, id: &str) -> T
    where
        T: DeserializeOwned + Default,
    {
        let Some(repo) = &self.config_repo else {
            return T::default();
        };

        match repo.get_engine_config(id).await {
            Ok(config) => serde_json::from_str::<T>(&config.config).unwrap_or_default(),
            Err(_) => T::default(),
        }
    }

    fn apply_override_best_effort<T>(mut base: T, override_val: &serde_json::Value) -> T
    where
        T: Serialize + DeserializeOwned,
    {
        if let Ok(merged) = Self::merge_config_json(&base, override_val)
            && let Ok(updated) = serde_json::from_value::<T>(merged)
        {
            base = updated;
        }

        base
    }

    /// Resolve engine type from an ID string.
    ///
    /// First tries to parse as a known `EngineType`, then falls back to DB lookup.
    async fn resolve_engine_type(&self, id: &str) -> Result<EngineType> {
        // Try parsing as known type first
        if let Ok(t) = id.parse::<EngineType>() {
            return Ok(t);
        }

        // Try DB lookup
        let Some(repo) = &self.config_repo else {
            return Err(crate::Error::Other(format!("Unknown engine: {}", id)));
        };

        let config = repo
            .get_engine_config(id)
            .await
            .map_err(|_| crate::Error::Other(format!("Unknown engine: {}", id)))?;

        config.engine_type.parse::<EngineType>().map_err(|_| {
            crate::Error::Other(format!("Unknown engine type: {}", config.engine_type))
        })
    }

    /// Helper to merge a base config with JSON overrides (RFC 7386 JSON Merge Patch).
    fn merge_config_json<T: Serialize>(
        base: &T,
        override_val: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut base_val =
            serde_json::to_value(base).map_err(|e| crate::Error::Other(e.to_string()))?;
        Self::json_merge(&mut base_val, override_val);
        Ok(base_val)
    }

    /// RFC 7386 JSON Merge Patch: recursively merge `patch` into `target`.
    fn json_merge(target: &mut serde_json::Value, patch: &serde_json::Value) {
        if let serde_json::Value::Object(patch_map) = patch {
            if !target.is_object() {
                *target = serde_json::Value::Object(serde_json::Map::new());
            }
            let target_map = target.as_object_mut().unwrap();
            for (key, value) in patch_map {
                if value.is_null() {
                    target_map.remove(key);
                } else if let Some(existing) = target_map.get_mut(key) {
                    Self::json_merge(existing, value);
                } else {
                    target_map.insert(key.clone(), value.clone());
                }
            }
        } else {
            *target = patch.clone();
        }
    }

    fn hash_override(override_val: &serde_json::Value) -> u64 {
        use std::hash::{Hash, Hasher};

        let canonical = Self::canonicalize_json(override_val);
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        canonical.to_string().hash(&mut hasher);
        hasher.finish()
    }

    fn canonicalize_json(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut canonical = serde_json::Map::with_capacity(map.len());
                for key in keys {
                    if let Some(child) = map.get(key) {
                        canonical.insert(key.clone(), Self::canonicalize_json(child));
                    }
                }
                serde_json::Value::Object(canonical)
            }
            serde_json::Value::Array(items) => {
                let canonical_items: Vec<_> = items.iter().map(Self::canonicalize_json).collect();
                serde_json::Value::Array(canonical_items)
            }
            _ => value.clone(),
        }
    }

    /// Internal: spin up the engine on a slot already granted by the
    /// queue, generate a download id, register the active download,
    /// emit `DownloadStarted`, and spawn the segment-event handler.
    ///
    /// Preflight (engine availability, circuit breaker, output gate,
    /// `prepare_output_dir`) is the caller's responsibility — see
    /// [`Self::preflight`]. The slot is moved into the active-downloads
    /// entry; capacity is released when the entry is removed.
    async fn start_download_with_engine_and_slot(
        &self,
        config: DownloadConfig,
        engine: Arc<dyn DownloadEngine>,
        engine_type: EngineType,
        engine_key: EngineKey,
        slot: SlotGuard,
    ) -> Result<String> {
        let is_high_priority = matches!(slot.priority(), Priority::High);
        let active_slot = slot.into_active();

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
        let cdn_host = crate::utils::url::extract_host(&config.url).unwrap_or_default();
        self.active_downloads.insert(
            download_id.clone(),
            ActiveDownload {
                handle: handle.clone(),
                status: DownloadStatus::Starting,
                progress: DownloadProgress::default(),
                is_high_priority,
                output_path: None,
                current_segment_index: None,
                current_segment_path: None,
                current_segment_started_at: None,
                slot: Some(active_slot),
                retry_config_override: None,
            },
        );

        // Emit start event (broadcast send is synchronous, ignore if no receivers)
        let _ = self.event_tx.send(DownloadManagerEvent::Progress(
            DownloadProgressEvent::DownloadStarted {
                download_id: download_id.clone(),
                streamer_id: config.streamer_id.clone(),
                streamer_name: config.streamer_name.clone(),
                session_id: config.session_id.clone(),
                engine_type,
                cdn_host,
                download_url: config.url.clone(),
            },
        ));

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
                        kind: e.kind,
                        message: format!("Engine start error: {}", e),
                    })
                    .await;
            }
        });

        // Spawn task to handle segment events
        let download_id_clone = download_id.clone();
        let event_tx = self.event_tx.clone();
        let streamer_id = config.streamer_id.clone();
        let streamer_name = config.streamer_name.clone();
        let session_id = config.session_id.clone();

        // Clone references for the spawned task
        let active_downloads = self.active_downloads.clone();
        let pending_updates = self.pending_updates.clone();
        let circuit_breakers_ref = self.circuit_breakers.get(&engine_key);
        // Handle into the segment event loop so runtime ENOSPC from the
        // engine stderr readers can reach `gate.record_failure` — the
        // mid-stream case where today's date dir already exists and
        // `prepare_output_dir` has nothing to detect.
        let output_root_gate_ref: Option<Arc<OutputRootGate>> =
            self.output_root_gate.get().cloned();

        tokio::spawn(async move {
            // Limit how often we broadcast progress updates (per download).
            // Engines may emit progress 1-10x/sec; broadcasting every tick can overwhelm
            // tokio::broadcast (clone-per-subscriber) and the WS clients.
            const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(250);
            let mut last_progress_emit = Instant::now() - PROGRESS_MIN_INTERVAL;

            while let Some(event) = segment_rx.recv().await {
                match event {
                    SegmentEvent::SegmentCompleted(info) => {
                        let SegmentInfo {
                            path,
                            duration_secs,
                            size_bytes,
                            index,
                            started_at: info_started_at,
                            completed_at,
                            split_reason_code,
                            split_reason_details_json,
                            ..
                        } = info;
                        // Normalize path
                        let normalized_path = tokio::fs::canonicalize(&path)
                            .await
                            .unwrap_or_else(|_| path.clone());
                        let segment_path = normalized_path.to_string_lossy().to_string();
                        // Prefer started_at from SegmentInfo (shared between start/complete callbacks),
                        // fall back to the active_downloads lookup for backward compat.
                        let started_at = info_started_at.or_else(|| {
                            active_downloads
                                .get(&download_id_clone)
                                .and_then(|download| {
                                    if download.current_segment_index == Some(index) {
                                        download.current_segment_started_at.as_ref().cloned()
                                    } else {
                                        None
                                    }
                                })
                        });

                        // Broadcast send is synchronous, ignore if no receivers
                        let _ = event_tx.send(DownloadManagerEvent::Progress(
                            DownloadProgressEvent::SegmentCompleted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                segment_path: segment_path.clone(),
                                segment_index: index,
                                started_at,
                                completed_at,
                                duration_secs,
                                size_bytes,
                                split_reason_code,
                                split_reason_details_json,
                            },
                        ));

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.output_path = Some(segment_path);
                            if download.current_segment_index == Some(index) {
                                download.current_segment_index = None;
                                download.current_segment_path = None;
                                download.current_segment_started_at = None;
                            }
                        }
                        debug!(
                            download_id = %download_id_clone,
                            path = %normalized_path.display(),
                            "Segment completed"
                        );
                    }
                    SegmentEvent::Progress(progress) => {
                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.progress = progress.clone();
                            download.status = DownloadStatus::Downloading;
                        }

                        // Broadcast progress event to WebSocket subscribers (throttled).
                        if last_progress_emit.elapsed() >= PROGRESS_MIN_INTERVAL {
                            last_progress_emit = Instant::now();
                            let _ = event_tx.send(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress,
                                },
                            ));
                        }
                    }
                    SegmentEvent::DownloadCompleted {
                        total_bytes,
                        total_duration_secs,
                        total_segments,
                        engine_signal,
                    } => {
                        circuit_breakers_ref.record_success();

                        // If progress is throttled, the latest tick might not have been broadcast.
                        // Emit one final progress update before sending the terminal event.
                        if let Some(download) = active_downloads.get(&download_id_clone) {
                            let final_progress = download.progress.clone();
                            let _ = event_tx.send(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress: final_progress,
                                },
                            ));
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

                        // Dropping the active download removes its
                        // ActiveSlot, which releases the queue capacity
                        // and wakes the next waiter automatically.

                        let _ = event_tx.send(DownloadManagerEvent::Terminal(
                            DownloadTerminalEvent::Completed {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                total_bytes,
                                total_duration_secs,
                                total_segments,
                                file_path: output_path,
                                // Forwarded from the engine's SegmentEvent::
                                // DownloadCompleted unchanged. Lifecycle reads
                                // this to decide hysteresis vs direct Ended.
                                engine_signal,
                            },
                        ));

                        debug!(
                            download_id = %download_id_clone,
                            "Download completed"
                        );
                        break;
                    }
                    SegmentEvent::DiskFull { output_dir, detail } => {
                        // Out-of-band signal only — the engine will still
                        // emit its own DownloadFailed on exit. Feeding the
                        // gate here short-circuits other streamers under
                        // the same root before they reach the engine.
                        if let Some(gate) = output_root_gate_ref.as_ref() {
                            let synthetic_io_err =
                                std::io::Error::new(std::io::ErrorKind::StorageFull, detail);
                            gate.record_failure(&output_dir, &synthetic_io_err);
                        } else {
                            debug!(
                                "DiskFull event received but no output-root gate attached; ignoring"
                            );
                        }
                    }
                    SegmentEvent::DownloadFailed { kind, message } => {
                        if kind.affects_circuit_breaker() {
                            circuit_breakers_ref.record_failure();
                        }

                        let recoverable = kind.is_recoverable();

                        // Emit one final progress update (best-effort) before the failure event.
                        if let Some(download) = active_downloads.get(&download_id_clone) {
                            let final_progress = download.progress.clone();
                            let _ = event_tx.send(DownloadManagerEvent::Progress(
                                DownloadProgressEvent::Progress {
                                    download_id: download_id_clone.clone(),
                                    streamer_id: streamer_id.clone(),
                                    streamer_name: streamer_name.clone(),
                                    session_id: session_id.clone(),
                                    status: DownloadStatus::Downloading,
                                    progress: final_progress,
                                },
                            ));
                        }

                        // remove download from active_downloads
                        // just before the event to avoid race condition
                        active_downloads.remove(&download_id_clone);
                        pending_updates.remove(&download_id_clone);

                        // Dropping the active download removes its
                        // ActiveSlot, which releases the queue capacity
                        // and wakes the next waiter automatically.

                        let _ = event_tx.send(DownloadManagerEvent::Terminal(
                            DownloadTerminalEvent::Failed {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                kind,
                                error: message,
                                recoverable,
                            },
                        ));

                        break;
                    }
                    SegmentEvent::SegmentStarted {
                        path,
                        sequence,
                        started_at,
                    } => {
                        let segment_path = path.to_string_lossy().to_string();

                        if let Some(mut download) = active_downloads.get_mut(&download_id_clone) {
                            download.current_segment_index = Some(sequence);
                            download.current_segment_path = Some(segment_path.clone());
                            download.current_segment_started_at = Some(started_at);
                        }

                        // Emit segment started event
                        let _ = event_tx.send(DownloadManagerEvent::Progress(
                            DownloadProgressEvent::SegmentStarted {
                                download_id: download_id_clone.clone(),
                                streamer_id: streamer_id.clone(),
                                streamer_name: streamer_name.clone(),
                                session_id: session_id.clone(),
                                segment_path: segment_path.clone(),
                                segment_index: sequence,
                                started_at,
                            },
                        ));

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
        self.stop_download_with_reason(download_id, DownloadStopCause::User)
            .await
    }

    /// Stop a download with an explicit reason.
    pub async fn stop_download_with_reason(
        &self,
        download_id: &str,
        cause: DownloadStopCause,
    ) -> Result<()> {
        if let Some((_, download)) = self.active_downloads.remove(download_id) {
            let engine_type = download.handle.engine_type;

            // Snapshot config once to avoid repeated lock acquisitions.
            let config_snap = download.handle.config_snapshot();
            let streamer_id = config_snap.streamer_id;
            let streamer_name = config_snap.streamer_name;
            let session_id = config_snap.session_id;

            // Emit one final progress update before cancellation.
            let _ = self.event_tx.send(DownloadManagerEvent::Progress(
                DownloadProgressEvent::Progress {
                    download_id: download_id.to_string(),
                    streamer_id: streamer_id.clone(),
                    streamer_name: streamer_name.clone(),
                    session_id: session_id.clone(),
                    status: DownloadStatus::Cancelled,
                    progress: download.progress.clone(),
                },
            ));

            if let Some(engine) = self.get_engine(engine_type) {
                engine.stop(&download.handle).await?;
            }

            self.pending_updates.remove(download_id);

            // Broadcast send is synchronous, ignore if no receivers
            let _ = self.event_tx.send(DownloadManagerEvent::Terminal(
                DownloadTerminalEvent::Cancelled {
                    download_id: download_id.to_string(),
                    streamer_id,
                    streamer_name,
                    session_id,
                    cause,
                },
            ));

            info!("Stopped download {}", download_id);

            // Dropping the ActiveSlot inside the removed download
            // releases the queue capacity and wakes the next waiter
            // automatically.

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
                    url: config_snapshot.url.clone(),
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

    /// Snapshot of currently-pending acquires (downloads that emitted
    /// [`DownloadProgressEvent::DownloadQueued`] but have not yet
    /// transitioned to [`DownloadProgressEvent::DownloadStarted`]).
    pub fn snapshot_pending(&self) -> Vec<QueuePendingEntry> {
        self.queue.snapshot_pending()
    }

    /// Mark the queue as shutting down. Subsequent acquires fail with
    /// `AcquireError::ShuttingDown`; pending acquires are notified and
    /// return the same error rather than waiting for a slot.
    ///
    /// Call this BEFORE [`Self::stop_all`] during graceful shutdown so
    /// queued pipelines released by `stop_all`'s slot drops don't try
    /// to spin up new engines as the rest of the system tears down.
    /// Active downloads aren't affected — they continue until
    /// `stop_all` drops them.
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

    /// Adjust the normal-priority concurrency limit at runtime.
    ///
    /// Increasing capacity wakes any waiters that fit. Decreasing keeps
    /// in-flight downloads running until they release naturally; new
    /// acquires beyond the new limit queue.
    pub fn set_max_concurrent_downloads(&self, limit: usize) -> usize {
        let limit = limit.max(1);

        {
            let mut config = self.config.write();
            config.max_concurrent_downloads = limit;
        }

        self.queue.set_normal_capacity(limit)
    }

    /// Adjust the number of high-priority extra slots at runtime (0 disables high-priority slots).
    pub fn set_high_priority_extra_slots(&self, slots: usize) -> usize {
        {
            let mut config = self.config.write();
            config.high_priority_extra_slots = slots;
        }

        self.queue.set_high_extra_capacity(slots)
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
                    url: config_snapshot.url.clone(),
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

        let streamer_name = self
            .active_downloads
            .get(download_id)
            .map(|d| d.handle.config.read().streamer_name.clone())
            .unwrap_or_else(|| streamer_id.to_string());

        let event = DownloadManagerEvent::Progress(DownloadProgressEvent::ConfigUpdated {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name,
            update_type,
        });

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
        let streamer_name = self
            .active_downloads
            .get(download_id)
            .map(|d| d.handle.config.read().streamer_name.clone())
            .unwrap_or_else(|| streamer_id.to_string());

        let event = DownloadManagerEvent::Progress(DownloadProgressEvent::ConfigUpdateFailed {
            download_id: download_id.to_string(),
            streamer_id: streamer_id.to_string(),
            streamer_name,
            error: error.to_string(),
        });

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
            let streamer_name = download.handle.config.read().streamer_name.clone();
            let _ = event_tx.send(DownloadManagerEvent::Progress(
                DownloadProgressEvent::ConfigUpdated {
                    download_id: download_id.to_string(),
                    streamer_id: streamer_id.to_string(),
                    streamer_name,
                    update_type,
                },
            ));
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
                    url: config_snapshot.url.clone(),
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
            if self
                .stop_download_with_reason(&id, DownloadStopCause::Shutdown)
                .await
                .is_ok()
            {
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
    use std::sync::atomic::Ordering;

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

    #[tokio::test]
    async fn test_runtime_reconfigure_max_concurrent_downloads() {
        // Validates the public contract: increasing capacity is
        // immediately observable, decreasing capacity is observable
        // even though existing in-flight downloads aren't preempted.
        let config = DownloadManagerConfig {
            max_concurrent_downloads: 2,
            high_priority_extra_slots: 0,
            ..Default::default()
        };
        let manager = DownloadManager::with_config(config);

        // Increase beyond initial capacity.
        assert_eq!(manager.set_max_concurrent_downloads(4), 4);
        assert_eq!(manager.max_concurrent_downloads(), 4);

        // Decrease — getter reflects the new value immediately.
        assert_eq!(manager.set_max_concurrent_downloads(1), 1);
        assert_eq!(manager.max_concurrent_downloads(), 1);

        // After saturating, a third acquire queues. We verify by
        // calling acquire on the queue directly — this mirrors the
        // semaphore-probe assertions the previous version made, but
        // through the public abstraction.
        let q = manager.queue.clone();
        let req = AcquireRequest {
            session_id: "s1".to_string(),
            streamer_id: "x".to_string(),
            streamer_name: "x".to_string(),
            engine_type: EngineType::Ffmpeg,
            priority: Priority::Normal,
        };
        let _slot1 = q
            .acquire(req, CancellationToken::new(), |_| {})
            .await
            .unwrap();
        assert_eq!(q.in_flight(), 1);

        // Second acquire must queue (capacity = 1).
        let q2 = q.clone();
        let h = tokio::spawn(async move {
            q2.acquire(
                AcquireRequest {
                    session_id: "s2".to_string(),
                    streamer_id: "x".to_string(),
                    streamer_name: "x".to_string(),
                    engine_type: EngineType::Ffmpeg,
                    priority: Priority::Normal,
                },
                CancellationToken::new(),
                |_| {},
            )
            .await
        });
        // Wait for it to register as pending.
        for _ in 0..50 {
            if q.pending_count() == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert_eq!(q.pending_count(), 1);

        // Bump the limit; the waiter should fire.
        manager.set_max_concurrent_downloads(2);
        let _slot2 = h.await.unwrap().unwrap();
        assert_eq!(q.in_flight(), 2);
    }

    #[tokio::test]
    async fn queued_slot_abandoned_after_acquire_emits_dequeued() {
        let config = DownloadManagerConfig {
            max_concurrent_downloads: 1,
            high_priority_extra_slots: 0,
            ..Default::default()
        };
        let manager = Arc::new(DownloadManager::with_config(config));
        let mut events = manager.subscribe();

        let first = manager
            .acquire_slot(
                AcquireRequest {
                    session_id: "active-session".to_string(),
                    streamer_id: "streamer-active".to_string(),
                    streamer_name: "Active".to_string(),
                    engine_type: EngineType::Ffmpeg,
                    priority: Priority::Normal,
                },
                CancellationToken::new(),
            )
            .await
            .unwrap();

        let waiter_manager = manager.clone();
        let waiter = tokio::spawn(async move {
            waiter_manager
                .acquire_slot(
                    AcquireRequest {
                        session_id: "queued-session".to_string(),
                        streamer_id: "streamer-queued".to_string(),
                        streamer_name: "Queued".to_string(),
                        engine_type: EngineType::Ffmpeg,
                        priority: Priority::Normal,
                    },
                    CancellationToken::new(),
                )
                .await
                .unwrap()
        });

        let queued_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            queued_event,
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadQueued {
                ref session_id,
                ..
            }) if session_id == "queued-session"
        ));

        drop(first);
        let slot = waiter.await.unwrap();
        assert!(slot.queued_event_emitted());

        manager.emit_dequeued_for_slot(&slot, "streamer-queued", "Queued");

        let dequeued_event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            dequeued_event,
            DownloadManagerEvent::Progress(DownloadProgressEvent::DownloadDequeued {
                ref session_id,
                ..
            }) if session_id == "queued-session"
        ));
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
            DownloadManagerEvent::Progress(DownloadProgressEvent::ConfigUpdated {
                download_id,
                streamer_id,
                update_type,
                ..
            }) => {
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
            DownloadManagerEvent::Progress(DownloadProgressEvent::ConfigUpdateFailed {
                download_id,
                streamer_id,
                error,
                ..
            }) => {
                assert_eq!(download_id, "download-123");
                assert_eq!(streamer_id, "streamer-456");
                assert_eq!(error, "Connection timeout");
            }
            _ => panic!("Expected ConfigUpdateFailed event"),
        }
    }

    // ========== Output-root write gate integration (#508) ==========

    /// Build a `DownloadConfig` pointed at `output_dir`, with the other
    /// fields set to minimal plausible values. The URL/streamer fields are
    /// only used for logging — we never actually spawn an engine in these
    /// tests.
    fn test_config_with_output_dir(output_dir: std::path::PathBuf) -> DownloadConfig {
        DownloadConfig::new(
            "https://example.com/test.flv",
            output_dir,
            "test-streamer-id",
            "TestStreamer",
            "test-session-id",
        )
    }

    /// Wrap a `DownloadManager` with a freshly constructed gate and return
    /// both the manager and a counter the recovery hook bumps each time it
    /// fires. Used by the three `prepare_output_dir_*` tests below.
    fn manager_with_gate() -> (
        DownloadManager,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<super::super::output_root_gate::OutputRootGate>,
    ) {
        use super::super::output_root_gate::{OutputRootGate, RecoveryHook};
        use std::sync::Weak;
        use std::sync::atomic::AtomicUsize;

        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let hook: RecoveryHook = Arc::new(move |_root: &std::path::Path| {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        let gate = OutputRootGate::new(
            Weak::new(),
            hook,
            vec![],
            Duration::from_secs(1), // short cooldown for the test runner
        );
        let manager = DownloadManager::new();
        manager.set_output_root_gate(gate.clone());
        (manager, counter, gate)
    }

    #[tokio::test]
    async fn prepare_output_dir_happy_path_returns_ok() {
        // Baseline: a real, writable temp dir. The gate starts Healthy and
        // stays Healthy; `ensure_output_dir` creates the nested subdir that
        // doesn't yet exist inside the temp root.
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("huya").join("X").join("20260415");
        let (manager, counter, gate) = manager_with_gate();

        let config = test_config_with_output_dir(nested.clone());
        let result = manager.prepare_output_dir(&config).await;

        assert!(result.is_ok(), "happy path should succeed: {:?}", result);
        assert!(
            nested.is_dir(),
            "nested output dir should have been created"
        );
        // Recovery hook only fires on a Degraded → Healthy transition.
        // A first-ever success against an untracked root is a no-op for the
        // gate, so the counter stays at zero.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        // And the gate snapshot should be empty (no roots ever tracked).
        assert!(gate.snapshot().is_empty());
    }

    #[tokio::test]
    async fn prepare_output_dir_on_unwritable_parent_trips_gate() {
        // Force create_dir_all to fail portably. GHA's Windows runner
        // runs as admin, so `C:\nonexistent\...` is creatable there;
        // Linux CI sandboxes often mount `/` read-only → EROFS. Instead,
        // put a regular file in a tempdir and point the output path at
        // a child of it — `create_dir_all` then fails with NotADirectory
        // on Unix and ERROR_DIRECTORY on Windows, both of which the gate
        // classifies into DownloadFailureKind::OutputRootUnavailable.
        // We don't pin the exact io_kind because the classification
        // bucket (never `Other`) is what's actually under test.
        let temp = tempfile::tempdir().expect("tempdir");
        let blocker = temp.path().join("blocker");
        std::fs::write(&blocker, b"i am a file, not a dir").unwrap();
        let bad_output = blocker.join("huya").join("X").join("20260415");
        let (manager, counter, gate) = manager_with_gate();

        let config = test_config_with_output_dir(bad_output.clone());
        let result = manager.prepare_output_dir(&config).await;

        // Must fail with OutputRootUnavailable{...}, NOT the generic Io
        // kind — this proves `EngineStartError::from` correctly walked
        // the error chain and the manager's `prepare_output_dir` routed
        // the io::Error into the gate before returning.
        let err = result.expect_err("should fail on unwritable parent");
        let io_kind = match err.kind {
            DownloadFailureKind::OutputRootUnavailable { io_kind } => io_kind,
            other => panic!("expected OutputRootUnavailable{{_}}, got {:?}", other),
        };
        assert!(
            !matches!(io_kind, IoErrorKindSer::Other),
            "expected a classified io_kind (NotFound / PermissionDenied / \
             ReadOnlyFilesystem / StorageFull / TimedOut), got Other"
        );

        // The gate must now be tracking a Degraded root with a matching
        // cached error kind.
        let snapshot = gate.snapshot();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(
            snapshot[0].state,
            crate::downloader::RootHealthState::Degraded
        );
        let last_error = snapshot[0]
            .last_error
            .as_ref()
            .expect("degraded root must have cached error");
        assert_eq!(last_error.0, io_kind);

        // A second `prepare_output_dir` call inside the cooldown window
        // must fast-reject via `gate.check()`, not re-try `create_dir_all`.
        // This is the key property that stops the 508 cascade.
        let result2 = manager.prepare_output_dir(&config).await;
        let err2 = result2.expect_err("second call should fast-reject");
        assert!(matches!(
            err2.kind,
            DownloadFailureKind::OutputRootUnavailable { .. }
        ));
        // Recovery hook must NOT have fired — we're still Degraded.
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn prepare_output_dir_recovers_after_path_becomes_valid() {
        // Trip the gate by pointing at a child of a regular file (fails
        // portably on Unix ENOTDIR and Windows ERROR_DIRECTORY — tokio's
        // create_dir_all will not recreate a file-as-directory ancestor).
        // Then fix the filesystem, wait past the cooldown, retry. The
        // winning CAS caller should see ensure_output_dir succeed, flip
        // the gate to Healthy, and fire the recovery hook exactly once.
        let temp = tempfile::tempdir().expect("tempdir");

        use super::super::output_root_gate::{OutputRootGate, RecoveryHook};
        use std::sync::Weak;
        use std::sync::atomic::AtomicUsize;

        let counter = Arc::new(AtomicUsize::new(0));
        let c2 = counter.clone();
        let hook: RecoveryHook = Arc::new(move |_root: &std::path::Path| {
            c2.fetch_add(1, Ordering::SeqCst);
        });
        let configured_root = temp.path().to_path_buf();
        let gate = OutputRootGate::new(
            Weak::new(),
            hook,
            vec![configured_root.clone()],
            Duration::from_secs(1),
        );
        let manager = DownloadManager::new();
        manager.set_output_root_gate(gate.clone());

        // `doomed` is a regular file; any path under it will fail
        // create_dir_all because an ancestor component is not a directory.
        let doomed = temp.path().join("doomed");
        std::fs::write(&doomed, b"i am a file, not a dir").unwrap();
        let under_doomed = doomed.join("will-fail");

        // Now `create_dir_all(under_doomed)` will fail with NotADirectory
        // or similar because one of the ancestor components is a regular
        // file. On Linux this surfaces as ErrorKind::NotFound or
        // ErrorKind::NotADirectory depending on kernel version; either
        // way the gate records a failure.
        let bad_config = test_config_with_output_dir(under_doomed.clone());
        let first = manager.prepare_output_dir(&bad_config).await;
        assert!(first.is_err(), "should fail when ancestor is a file");
        let snap = gate.snapshot();
        assert_eq!(snap.len(), 1, "gate should be tracking the configured root");
        assert_eq!(snap[0].state, crate::downloader::RootHealthState::Degraded);
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // Fix the filesystem: remove the blocking file and recreate the
        // directory structure. Wait past the 1s cooldown before retrying.
        std::fs::remove_file(&doomed).unwrap();
        std::fs::create_dir_all(&under_doomed).unwrap();
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // Retry. The winning CAS caller's ensure_output_dir succeeds, the
        // gate flips to Healthy, the recovery hook fires.
        let second = manager.prepare_output_dir(&bad_config).await;
        assert!(
            second.is_ok(),
            "retry should succeed after cooldown and filesystem fix: {:?}",
            second
        );

        // Recovery hook is spawned on a tokio task; yield so it runs.
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "recovery hook should fire exactly once per Degraded→Healthy transition"
        );

        // Gate snapshot should now show Healthy.
        let snap = gate.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].state, crate::downloader::RootHealthState::Healthy);
    }

    #[tokio::test]
    async fn prepare_output_dir_without_gate_is_transparent() {
        // Safety guarantee: installing no gate must leave prepare_output_dir
        // behaving exactly like the old inline `ensure_output_dir` call —
        // success creates the dir, failure returns a classified
        // EngineStartError, no panics, no hidden state.
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("a").join("b").join("c");
        let manager = DownloadManager::new();
        // Deliberately NO set_output_root_gate.

        let config = test_config_with_output_dir(nested.clone());
        assert!(manager.prepare_output_dir(&config).await.is_ok());
        assert!(nested.is_dir());
    }
}
