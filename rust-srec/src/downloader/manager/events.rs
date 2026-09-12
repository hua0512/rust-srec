//! Download event and terminal outcome contracts.

use chrono::{DateTime, Utc};

use crate::downloader::engine::{
    DownloadFailureKind, DownloadProgress, DownloadProtocol, DownloadStatus, EngineType,
    IoErrorKindSer,
};

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
    /// User cancelled the current recording; the scheduler checks status again.
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

/// Result of a graceful download-runtime shutdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadShutdownReport {
    /// Downloads that were active when shutdown fenced new attempts.
    pub stopped_download_ids: Vec<String>,
    /// Attempts that exceeded the graceful deadline but were still joined
    /// before shutdown continued.
    pub deadline_exceeded_download_ids: Vec<String>,
    /// Whether the required coordination consumer acknowledged its final
    /// marker. Producers are quiesced even when this is false.
    pub coordination_drained: bool,
    /// Lifecycle errors from attempts that ended earlier in the run. Reported
    /// and then ignored: they describe those recordings, not this shutdown.
    pub runtime_failures: Vec<String>,
    /// Attempt or coordination failures of the shutdown phase itself. Each one
    /// means shutdown could not prove an attempt finalized, so callers treat
    /// them as fatal.
    pub failures: Vec<String>,
    /// Soft-budget overruns that were still contained to completion. Reported
    /// so operators can raise the budget, never fatal.
    pub overruns: Vec<String>,
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
/// an explicit decision for every terminal variant, where a flat enum would
/// let a `_ => {}` catch-all silently drop one.
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
        /// External stop request that preceded the engine's clean completion.
        ///
        /// Completion describes the finalized recording output; this cause
        /// preserves the control-plane intent for scheduler state handling.
        stop_cause: Option<DownloadStopCause>,
    },
    /// Download failed — the engine gave up. Whatever output is on disk is
    /// final; no more segments will arrive.
    Failed {
        download_id: String,
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        engine_type: EngineType,
        /// Selected stream protocol. Mesio uses one engine type for both
        /// HLS and FLV, so lifecycle needs this to classify failures without
        /// guessing.
        protocol: DownloadProtocol,
        kind: DownloadFailureKind,
        error: String,
        recoverable: bool,
    },
    /// Download cancelled after the engine flushed its in-flight segment and
    /// all preceding segment events were translated. Cancellation preserves
    /// the logical session so a later attempt can resume it.
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
        /// needs to route this to the correct
        /// [`DownloadEndPolicy`](crate::scheduler::actor::DownloadEndPolicy) variant
        /// and, ultimately, the correct [`crate::monitor::InfraBlockReason`].
        kind: DownloadRejectedKind,
    },
}

impl DownloadManagerEvent {
    pub(crate) fn requires_coordination(&self) -> bool {
        matches!(
            self,
            Self::Progress(
                DownloadProgressEvent::SegmentStarted { .. }
                    | DownloadProgressEvent::SegmentCompleted { .. }
            ) | Self::Terminal(_)
        )
    }

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
    ///   is final and eligible for the session-complete pipeline.
    /// - [`Self::Cancelled`]: `false` — the engine did not report a clean
    ///   completion. The control-plane stop owner decides whether to end or
    ///   preserve the session (shutdown preserves it for restart).
    /// - [`Self::Rejected`]: `false` — the download never started, no
    ///   outputs exist.
    pub fn should_run_session_complete_pipeline(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. })
    }
}

/// Reason a [`DownloadTerminalEvent::Rejected`] event was emitted.
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
    /// The streamer is inside its error-backoff window (`disabled_until` set
    /// by `MonitorService::handle_error`). Emitted by the container's
    /// download-start gate rather than `preflight` — the backoff itself is
    /// already persisted, so the scheduler only reschedules the actor after
    /// `retry_after_secs`; no `InfraBlockReason` write is needed.
    StreamerBackoff,
}
