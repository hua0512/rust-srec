//! Core state, input, and effect types for [`StreamerFsm`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;

use crate::credentials::{CredentialBinding, CredentialSnapshot};
use crate::danmu::DanmuControlEvent;
use crate::domain::StreamerState;
use crate::downloader::engine::{DownloadProgress, DownloadProtocol, EngineType};
use crate::downloader::{DownloadStopCause, DownloadTerminalEvent};
use crate::monitor::StreamInfo;
use crate::session::{EngineKind, TerminalCause};

use super::error::ErrorTracker;
use super::hysteresis::{OfflineConfirmation, SlidingFailureWindow};
use super::segment::SessionSegmentGuard;
use crate::recorder::ports::{
    ActorStopReason, AttemptStartFailure, AttemptStartSpec, DanmuCloseSegmentSpec,
    DanmuSegmentRecord, DanmuStopKind, DanmuStopResult, DbCommitFailure, DbCommitOp,
    DbCommitOutcome, LiveProbePayload, PipelineNotification, ProbeFailure, ProbePurpose,
    ProbeRequest, ProbeStatus, ResolvedActorConfig, RetirementBounds, StreamerRuntimeView,
    VideoSegmentRecord,
};

/// Explicit clock and deterministic jitter inputs passed to [`StreamerFsm::step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepContext {
    /// Monotonic clock for deadlines, TTLs, and sliding windows.
    pub now: Instant,
    /// Wall-clock timestamp for persisted rows, smart-wake hints, and `disabled_until`.
    pub wall_now: Timestamp,
    /// Polling jitter factor in basis points (`9000..=11000`, representing ±10%).
    pub jitter_bps: u16,
}

impl StepContext {
    /// Create a context with explicit clocks and jitter factor.
    pub fn new(now: Instant, wall_now: Timestamp, jitter_bps: u16) -> Self {
        Self {
            now,
            wall_now,
            jitter_bps,
        }
    }

    /// Create a zero-jitter (`10_000` bps) deterministic context.
    pub fn deterministic(now: Instant, wall_now: Timestamp) -> Self {
        Self {
            now,
            wall_now,
            jitter_bps: 10_000,
        }
    }
}

/// Pure synchronous Sans-IO state machine for a single streamer's recording
/// and monitoring lifecycle.
///
/// Outer monitoring schedule ([`MonitorPhase`]), monitor-side offline
/// confirmation ([`OfflineConfirmation`]), and recording session ownership
/// ([`SessionSlot`]) evolve orthogonally so an active recording can observe
/// offline watchdog signals, hold a bounded non-extending quiet period, and
/// serialize at most one in-flight database commit at a time without blocking
/// stop commands or trailing segment persistence.
#[derive(Debug)]
pub struct StreamerFsm {
    pub streamer_id: String,
    pub config: ResolvedActorConfig,
    pub config_revision: u64,
    /// Outer monitoring schedule phase.
    pub monitor: MonitorPhase,
    /// Orthogonal monitor-side offline confirmation counter.
    pub offline_confirmation: OfflineConfirmation,
    /// Independent session lifecycle slot, including two-phase start handshake
    /// and 60-second retroactive reattribution retention.
    pub session: SessionSlot,
    /// Currently in-flight status or credential recovery probe (at most one).
    pub in_flight_probe: Option<InFlightProbe>,
    /// Currently in-flight database commit operation (at most one).
    pub pending_db: Option<PendingDbOp>,
    /// Queued follow-up database writes waiting for `pending_db` to settle.
    pub(super) queued_db: VecDeque<QueuedDbOp>,
    /// Urgent stop intent deferred while a session-start or other DB commit was in flight.
    pub(super) deferred_stop: Option<StopKind>,
    /// Cold-start reconciliation flag: cleared only after the first applied
    /// `Live` or `Offline` database commit; failed or suppressed writes keep it `true`.
    pub initial_reconcile_pending: bool,
    /// Consecutive check/download error counter and backoff state.
    pub errors: ErrorTracker,
    /// Per-streamer sliding window of Mesio network failures for definitive-offline promotion.
    pub network_failures: SlidingFailureWindow,
    /// Set when a stop deadline elapses before physical download/danmu leases
    /// confirm exit. Prevents starting new downloads or danmu on the same
    /// output until [`FsmInput::PhysicalLeaseReleased`] clears it.
    pub lingering_physical_lease: bool,
    /// Whether the most recent offline probe armed a batch-live watch hint.
    pub batch_live_hint_armed: bool,
    /// Segment discard/dedupe guards for sessions whose DB row already closed
    /// while trailing download segments or danmu `CollectionStopped` still drain.
    pub(super) draining_segment_guards: HashMap<String, SessionSegmentGuard>,
    /// Monotonic sequence counter for `probe_id`, `op_id`, `attempt_id`, and `timer_id`.
    pub next_seq: u64,
}

/// Outer monitoring scheduling phase, orthogonal to [`OfflineConfirmation`] and [`SessionSlot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorPhase {
    /// Polling on normal or post-live cadence (or parked when `next_check` is `None`).
    Idle {
        next_check: Option<Instant>,
        /// Present only when `next_check` came from a recurring poll interval
        /// and may be brought forward by a faster config update.
        recurring_interval_ms: Option<u64>,
        /// Saved routine offline poll deadline when a batch-live hint brought a check forward.
        batch_routine_deadline: Option<Instant>,
    },
    /// Streamer was observed live but filtered out.
    Filtered {
        reason: FilteredKind,
        next_check: Instant,
        recurring_interval_ms: Option<u64>,
    },
    /// Current broadcast was refused by the engine (e.g. DRM / unsupported stream).
    /// Suppresses `Live` until a probe observes `Offline`.
    RefusedBroadcast {
        next_check: Instant,
        recurring_interval_ms: Option<u64>,
    },
    /// Waiting out a temporary backoff or infrastructure cooldown.
    ///
    /// Persisted `OutOfSpace` streamers remain dormant at startup; `OutputRootCooldown`
    /// only holds a running actor while an output-root rejection is cooling down or retiring.
    Blocked {
        kind: BlockedKind,
        retry_at: Instant,
    },
    /// Actively monitoring a live session via 5-minute stall detection, 2-hour
    /// watchdog floor, and schedule-end boundary hints.
    LiveWatchdog {
        last_activity_at: Instant,
        next_filter_hint: Option<Timestamp>,
        watchdog_backoff_until: Option<Instant>,
    },
    /// Coordinating physical download/danmu shutdown and final DB commit under a deadline.
    Stopping(StoppingState),
    /// Actor has permanently stopped for this generation.
    Terminated,
}

/// Distinguishes schedule-window filtering (which tears down active recordings
/// and uses smart-wake) from metadata filtering (which never interrupts an
/// active recording).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilteredKind {
    OutOfSchedule { smart_wake: Option<Timestamp> },
    MetadataMismatch,
}

/// Reason the monitor is parked in [`MonitorPhase::Blocked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockedKind {
    TemporalDisabled,
    OutputRootCooldown,
}

/// Reason the FSM entered [`MonitorPhase::Stopping`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopKind {
    /// User or configuration disabled the streamer (20s deadline; sets
    /// `lingering_physical_lease` on expiry, commits `UserDisabled`).
    Disable,
    /// Recording schedule window closed (20s deadline; sets
    /// `lingering_physical_lease` on expiry, commits `OutOfSchedule`).
    OutOfSchedule { smart_wake: Option<Timestamp> },
    /// Managed credential profile retirement for an exact captured `(session_id, binding)`.
    /// Never commits session end if physical download or danmu fail to confirm stop within 20s.
    CredentialRetire {
        session_id: String,
        binding: CredentialBinding,
    },
    /// Streamer deletion retirement; physical quiescence is tracked independently
    /// of control-plane deadline expiry so `reap_deleted` never runs prematurely.
    Retire { bounds: RetirementBounds },
    /// Process-wide shutdown; preserves open sessions and danmu checkpoints.
    Shutdown,
}

/// Active stop coordination state inside [`MonitorPhase::Stopping`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppingState {
    pub kind: StopKind,
    pub deadline: Instant,
    pub awaiting_download: bool,
    pub awaiting_danmu: bool,
    pub download_confirmed: bool,
    pub danmu_confirmed: bool,
    pub danmu_unconfirmed: bool,
    pub db_end_dispatched: bool,
    pub db_end_committed: bool,
    pub db_end_failed: bool,
    pub deadline_expired: bool,
}

impl StoppingState {
    /// Whether every physical resource that was running when stop started has
    /// confirmed termination without hitting an unconfirmed outcome.
    pub fn physical_confirmed(&self) -> bool {
        self.download_confirmed && self.danmu_confirmed && !self.danmu_unconfirmed
    }
}

/// Independent session ownership slot.
#[derive(Debug)]
pub enum SessionSlot {
    /// No open or recently-ended session tracked in memory.
    None,
    /// Two-phase session start/resume handshake awaiting authoritative SQLite
    /// `session_id`, `started_at`, `Created | ReusedActive | SuppressedInactive`,
    /// and committed `CredentialBinding.epoch`.
    CommittingStart {
        op_id: u64,
        probe_payload: Box<LiveProbePayload>,
        from_hysteresis: bool,
        /// Preserves the open session across hysteresis-resume or stalled re-emit
        /// commits so segment guards, danmu state, and quiet-period handles survive
        /// rollback or suppression.
        prior_open: Option<Box<OpenSession>>,
    },
    /// Authoritative open recording session (`end_time IS NULL` in SQLite).
    Open(Box<OpenSession>),
    /// 60-second retention window for the most recently ended session, enabling
    /// `Disable` to retroactively reattribute ambiguous `via_hysteresis` ends
    /// (`Completed` / `Failed`) to `UserDisabled`.
    RecentlyEnded {
        session_id: String,
        reattributable_timer_end: bool,
        evict_at: Instant,
    },
}

/// Active recording session tracked inside [`SessionSlot::Open`].
#[derive(Debug)]
pub struct OpenSession {
    pub session_id: String,
    pub started_at: Timestamp,
    pub title: String,
    pub credential_binding: Option<CredentialBinding>,
    /// Freshly extracted media bundle from a credential recovery diagnostic (30s TTL).
    pub recovered_media: Option<RecoveredMedia>,
    /// Number of credential diagnostic attempts consumed by this session.
    pub diagnostic_attempts: u32,
    /// Download IDs that have already triggered a credential diagnostic in this session.
    pub diagnosed_download_ids: HashSet<String>,
    /// Download attempt lifecycle state.
    pub attempt: AttemptPhase,
    /// Danmu collection lifecycle state.
    pub danmu: DanmuSessionState,
    /// Non-extending quiet-period backstop armed only on ambiguous engine terminals.
    pub quiet_period: Option<QuietPeriod>,
    /// Per-session video/danmu segment discard and reverse-cleanup guard.
    pub segment_guard: SessionSegmentGuard,
    /// Whether the session has already used its one automatic retry after a
    /// `CredentialsChanged` startup exit.
    pub credentials_changed_retried: bool,
}

/// Media extracted by a successful credential diagnostic for reuse by the next
/// download attempt within [`crate::recorder::fsm::credentials::RECOVERED_MEDIA_TTL`] (30 seconds).
///
/// `Debug` redacts headers, extras, credentials, and stream URLs.
#[derive(Clone)]
pub struct RecoveredMedia {
    pub binding: CredentialBinding,
    pub snapshot: Arc<CredentialSnapshot>,
    pub streams: Vec<StreamInfo>,
    pub media_headers: Option<HashMap<String, String>>,
    pub media_extras: Option<HashMap<String, String>>,
    pub extracted_at: Instant,
}

impl std::fmt::Debug for RecoveredMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveredMedia")
            .field("binding", &self.binding)
            .field("stream_count", &self.streams.len())
            .field("extracted_at", &self.extracted_at)
            .field("network_material", &"[redacted]")
            .finish()
    }
}

/// Download attempt lifecycle inside an [`OpenSession`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptPhase {
    /// No download attempt currently starting or running.
    Idle,
    /// Startup task in flight (preflight -> slot acquire -> freshness -> engine start).
    Starting {
        attempt_id: u64,
        queued: bool,
        credential_binding: Option<CredentialBinding>,
    },
    /// Engine actively recording.
    Running {
        attempt_id: u64,
        download_id: String,
        engine_kind: EngineKind,
        engine_type: EngineType,
        protocol: DownloadProtocol,
    },
    /// Session remains open awaiting usable managed credentials.
    WaitingCredentials,
    /// Waiting out diagnostic backoff or running a `Recovery` probe after an
    /// auth/exit download failure.
    DiagnosingCredentials {
        probe_id: Option<u64>,
        backoff_until: Option<Instant>,
        failed_download_id: String,
        original_cause: TerminalCause,
        original_error: String,
    },
    /// Stop requested for an attempt; awaiting terminal confirmation.
    StoppingAttempt {
        attempt_id: u64,
        download_id: Option<String>,
    },
}

/// Danmu collection lifecycle inside an [`OpenSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DanmuSessionState {
    Off,
    Starting,
    Running { link_down_since: Option<Instant> },
    Stopping,
}

/// Non-extending quiet-period state armed on ambiguous download terminals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuietPeriod {
    pub timer_id: u64,
    pub cause: TerminalCause,
    pub entered_at: Timestamp,
    /// Fixed deadline set once when transitioning `None -> Some`; never extended.
    pub backstop_deadline: Instant,
    /// Bounded remaining offline checks frozen when `backstop_deadline` elapses
    /// while a probe or offline-confirmation sequence is already in flight.
    pub grace_remaining_checks: Option<u32>,
    /// `true` after a timer-expiry DB commit failed and is retrying every 1s
    /// under the same `timer_id`.
    pub retrying_commit: bool,
    /// Next 1-second retry instant while `retrying_commit` is `true`.
    pub next_retry_at: Option<Instant>,
}

/// Metadata tracking the single in-flight probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InFlightProbe {
    pub probe_id: u64,
    pub purpose: ProbePurpose,
    pub is_live_watchdog: bool,
    pub started_at: Instant,
    pub for_session_id: Option<String>,
}

/// Metadata tracking the single in-flight database commit.
#[derive(Debug)]
pub struct PendingDbOp {
    pub op_id: u64,
    pub kind: PendingDbKind,
}

/// Discriminator describing how to reconcile a [`DbCommitOutcome`] when the
/// in-flight database commit completes.
#[derive(Debug)]
pub enum PendingDbKind {
    StartOrResume,
    ResumeHysteresis,
    EndSession {
        session_id: Option<String>,
        cause: TerminalCause,
        via_hysteresis: bool,
        from_quiet_period_timer_id: Option<u64>,
        sets_offline: bool,
        sets_out_of_schedule: bool,
        clears_errors: bool,
        infra_block: Option<crate::monitor::InfraBlockReason>,
        for_stop: bool,
    },
    EndForCredentialRetirement {
        session_id: String,
    },
    RewriteEndedCause {
        session_id: String,
        for_stop: bool,
    },
    CommitCredentialBinding {
        session_id: String,
        staged_media: Option<Box<StagedRecoveredMedia>>,
        original_cause: TerminalCause,
        original_error: String,
    },
    RecordTransientError {
        error_message: String,
    },
    ClearErrors {
        target_state: Option<StreamerState>,
    },
    ClearLastError,
    RecordFatalError {
        new_state: StreamerState,
    },
    RecordFilteredState,
    SetInfraBlocked {
        state: StreamerState,
    },
    AddSessionTitle,
    RecordHysteresisEntered,
}

/// Extracted diagnostic media awaiting the authoritative `CredentialBinding.epoch`
/// returned by [`DbCommitOp::CommitCredentialBinding`].
#[derive(Clone)]
pub struct StagedRecoveredMedia {
    pub snapshot: Arc<CredentialSnapshot>,
    pub streams: Vec<StreamInfo>,
    pub media_headers: Option<HashMap<String, String>>,
    pub media_extras: Option<HashMap<String, String>>,
    pub extracted_at: Instant,
}

impl std::fmt::Debug for StagedRecoveredMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StagedRecoveredMedia")
            .field("stream_count", &self.streams.len())
            .field("extracted_at", &self.extracted_at)
            .field("network_material", &"[redacted]")
            .finish()
    }
}

/// A queued database write awaiting completion of the currently in-flight `pending_db`.
#[derive(Debug)]
pub(super) struct QueuedDbOp {
    pub op_id: u64,
    pub kind: PendingDbKind,
    pub op: DbCommitOp,
}

/// Events fed into [`StreamerFsm::step`].
#[derive(Debug, Clone)]
pub enum FsmInput {
    /// Earliest deadline returned by [`StreamerFsm::next_deadline`] elapsed.
    TimerFired,
    /// Immediate status check requested externally (manual API, filter change, etc.).
    CheckNow,
    /// Batch precheck signaled this streamer may be live before its routine poll.
    BatchLiveHint,
    /// Scheduler feedback or recording admission capacity recovered.
    RetryAdmission,
    /// Freshly resolved configuration carrying a monotonic revision fence.
    ConfigUpdated {
        revision: u64,
        config: Box<ResolvedActorConfig>,
    },
    /// Operator or output-root gate cleared this streamer's error state.
    ErrorCleared,
    /// Credential profiles or selection changed for an owner covering this streamer.
    CredentialMaterialChanged,
    /// In-flight probe finished.
    ProbeCompleted {
        probe_id: u64,
        result: std::result::Result<ProbeStatus, ProbeFailure>,
    },
    /// In-flight database commit settled.
    DbCommitted {
        op_id: u64,
        result: std::result::Result<DbCommitOutcome, DbCommitFailure>,
    },
    /// Download attempt entered the slot wait queue.
    AttemptQueued { attempt_id: u64, session_id: String },
    /// Queued download attempt exited the wait queue without starting an engine.
    AttemptDequeued { attempt_id: u64, session_id: String },
    /// Download engine started capturing bytes.
    AttemptStarted {
        attempt_id: u64,
        session_id: String,
        download_id: String,
        engine_type: EngineType,
        protocol: DownloadProtocol,
    },
    /// Startup pipeline aborted before the engine started.
    AttemptStartFailed {
        attempt_id: u64,
        session_id: String,
        reason: AttemptStartFailure,
    },
    /// Throttled progress heartbeat from the active download.
    DownloadHeartbeat {
        session_id: String,
        download_id: String,
        progress: Option<DownloadProgress>,
    },
    /// Engine opened a new output segment file.
    SegmentStarted {
        attempt_id: u64,
        session_id: String,
        download_id: String,
        segment_index: u32,
        segment_path: String,
        started_at: Timestamp,
    },
    /// Engine closed an output segment file.
    ///
    /// Session-level persistence is decoupled from attempt generation control:
    /// even if `attempt_id` is stale or the actor is stopping, a non-discarded
    /// trailing segment is still persisted and forwarded to the pipeline.
    SegmentCompleted {
        attempt_id: u64,
        session_id: String,
        download_id: String,
        segment_index: u32,
        segment_path: String,
        started_at: Option<Timestamp>,
        completed_at: Timestamp,
        duration_secs: f64,
        size_bytes: u64,
        effective_size_bytes: u64,
        split_reason_code: Option<String>,
        split_reason_details_json: Option<String>,
        split_by_format_change: bool,
    },
    /// Download attempt reached its terminal outcome.
    DownloadEnded {
        attempt_id: u64,
        event: DownloadTerminalEvent,
    },
    /// Danmu collector connected and started for `session_id`.
    DanmuCollectionStarted { session_id: String },
    /// Danmu XML segment finalized on disk.
    DanmuSegmentCompleted {
        session_id: String,
        segment_index: u32,
        output_path: String,
        message_count: u64,
        size_bytes: u64,
    },
    /// Danmu platform control signal (room title change or stream closed).
    DanmuControl {
        session_id: String,
        control: DanmuControlEvent,
    },
    /// Danmu collector lost its connection and began reconnecting.
    DanmuReconnecting { session_id: String, attempt: u32 },
    /// Danmu collector reconnected.
    DanmuReconnected {
        session_id: String,
        attempts: u32,
        downtime_secs: u64,
    },
    /// Danmu collector exited on the FIFO coordination channel.
    DanmuCollectionStopped {
        session_id: String,
        total_count: u64,
        result: DanmuStopResult,
    },
    /// A lingering physical download or danmu lease that outlived a stop
    /// deadline has now exited.
    PhysicalLeaseReleased,
    /// User or configuration disabled the streamer.
    Disable,
    /// Retire a specific bound credential profile for a captured open session.
    CredentialRetire {
        session_id: String,
        binding: CredentialBinding,
    },
    /// Retire a streamer marked with `deleted_at` before physical row removal.
    Retire { bounds: RetirementBounds },
    /// Graceful process shutdown.
    Shutdown,
}

/// Side effects emitted synchronously by [`StreamerFsm::step`].
#[derive(Debug, Clone)]
pub enum FsmEffect {
    /// Start an asynchronous status or credential recovery probe.
    SpawnProbe(ProbeRequest),
    /// Cancel an in-flight probe.
    CancelProbe { probe_id: u64 },
    /// Execute a serialized database commit (at most one in flight per actor).
    CommitDb { op_id: u64, op: DbCommitOp },
    /// Launch a supervised download attempt.
    StartAttempt(Box<AttemptStartSpec>),
    /// Request an in-flight or active download attempt to stop.
    StopAttempt {
        attempt_id: u64,
        download_id: Option<String>,
        cause: DownloadStopCause,
    },
    /// Start a paired danmu XML segment.
    StartDanmuSegment {
        session_id: String,
        segment_index: u32,
        video_segment_path: String,
        started_at: Timestamp,
    },
    /// Close (and optionally discard at source) a paired danmu XML segment.
    CloseDanmuSegment(DanmuCloseSegmentSpec),
    /// Stop danmu collection for a session.
    StopDanmu {
        session_id: String,
        reason: DanmuStopKind,
    },
    /// Remove discarded video and/or danmu files from disk.
    DeleteDiscardedFiles {
        video_path: Option<String>,
        danmu_path: Option<String>,
    },
    /// Reverse-clean a danmu media output row that finished before its paired
    /// video segment was discarded.
    RemoveDanmuSegmentOutput {
        session_id: String,
        danmu_path: String,
    },
    /// Persist a non-discarded video segment into `session_segments`.
    PersistVideoSegment(VideoSegmentRecord),
    /// Persist a non-discarded danmu XML segment into `media_outputs`.
    PersistDanmuSegment(DanmuSegmentRecord),
    /// Deliver one of the five post-commit pipeline notifications.
    NotifyPipeline(PipelineNotification),
    /// Publish an updated read-only runtime view snapshot.
    PublishRuntimeView(StreamerRuntimeView),
    /// Notify the actor loop / supervisor that this actor has terminated.
    ActorStopped { reason: ActorStopReason },
}
