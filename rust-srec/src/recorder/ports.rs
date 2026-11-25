//! Narrow ports and typed contracts for the recording control plane.
//!
//! These traits and value types decouple [`crate::recorder::StreamerFsm`] and
//! future actor/runtime adapters from concrete SQLite repositories, download
//! engines, danmu runners, and pipeline coordinators.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use jiff::Timestamp;

use crate::config::MergedConfig;
use crate::credentials::{CredentialBinding, CredentialSnapshot, UnavailableReason};
use crate::domain::streamer::FatalErrorType;
use crate::domain::{Priority, RetryPolicy, StreamerState};
use crate::downloader::IoErrorKindSer;
use crate::monitor::{FilterReason, InfraBlockReason, StreamInfo};
use crate::session::{TerminalCause, TerminalCauseDto};
use crate::streamer::StreamerMetadata;
use crate::streamer::metadata::{DEFAULT_OFFLINE_CHECK_COUNT, DEFAULT_OFFLINE_CHECK_DELAY_MS};

/// Default polling interval (60 seconds) when no configuration layer supplies one.
pub const DEFAULT_CHECK_INTERVAL_MS: u64 = 60_000;

/// Per-owner retirement bounds for an interactive deletion request.
pub const INTERACTIVE_RETIREMENT: RetirementBounds = RetirementBounds {
    actor: Duration::from_secs(10),
    download: Duration::from_secs(20),
    pipeline: Duration::from_secs(2),
};

/// Zero-wait retirement bounds used by background reaper sweeps and batch deletes.
pub const OBSERVE_RETIREMENT: RetirementBounds = RetirementBounds {
    actor: Duration::ZERO,
    download: Duration::ZERO,
    pipeline: Duration::ZERO,
};

/// Whether a persisted streamer state should have a resident monitoring actor.
///
/// Deleted rows, disabled streamers, fatal states, generic errors, and
/// persisted [`StreamerState::OutOfSpace`] rows stay dormant until cleared
/// (for `OutOfSpace`, by output-root gate recovery). Only active states,
/// including [`StreamerState::TemporalDisabled`], run a resident actor.
pub fn should_spawn_actor(state: StreamerState, deleted: bool) -> bool {
    !deleted
        && matches!(
            state,
            StreamerState::NotLive
                | StreamerState::Live
                | StreamerState::OutOfSchedule
                | StreamerState::InspectingLive
                | StreamerState::TemporalDisabled
        )
}

/// Per-owner bounds for retiring a deleted streamer before physical row removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetirementBounds {
    /// Bound on stopping the streamer actor task.
    pub actor: Duration,
    /// Bound on active download attempts finalizing.
    pub download: Duration,
    /// Bound on post-processing pipeline settlement.
    pub pipeline: Duration,
}

/// What one retirement attempt observed, per physical resource owner.
///
/// [`Self::is_quiescent`] is the precondition for physically reaping a deleted
/// streamer row: `false` means at least one owner may still write for the
/// streamer, so the row stays marked until a later reaper pass observes full
/// quiescence.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamerRetirement {
    /// Owners that had not settled when their bound expired.
    pub outstanding: Vec<String>,
}

impl StreamerRetirement {
    /// Whether every physical owner settled, allowing the caller to reap the row.
    pub fn is_quiescent(&self) -> bool {
        self.outstanding.is_empty()
    }
}

/// Resolved configuration snapshot consumed by [`crate::recorder::StreamerFsm`].
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedActorConfig {
    /// Streamer display name for audit rows, notifications, and templates.
    pub streamer_name: String,
    /// Streamer room URL.
    pub streamer_url: String,
    /// Normalized platform identifier (e.g. `"twitch"`, `"bilibili"`, `"bigo"`).
    pub platform: String,
    /// Last persisted avatar URL, used to detect avatar changes on live checks.
    pub avatar_url: Option<String>,
    /// Routine offline polling interval in milliseconds.
    pub check_interval_ms: u64,
    /// Short post-live / offline-confirmation polling interval in milliseconds.
    pub offline_check_interval_ms: u64,
    /// Consecutive offline polls required to confirm a live streamer went offline.
    pub offline_check_count: u32,
    /// Streamer scheduling and download queue priority.
    pub priority: Priority,
    /// Minimum video segment size in bytes; `0` disables small-segment discarding.
    pub min_segment_size_bytes: u64,
    /// Whether danmu collection is enabled for this streamer.
    pub record_danmu: bool,
    /// Whether a danmu `StreamClosed` control event authoritatively ends the session.
    pub end_stream_on_danmu_stream_closed: bool,
    /// Whether managed credentials govern extraction and recording for this streamer.
    pub managed_credentials: bool,
    /// Active credential policy generation digest, if managed.
    pub credential_policy_generation: Option<String>,
    /// Retry policy governing credential diagnostic backoff and maximum attempts.
    pub download_retry_policy: RetryPolicy,
    /// Whether platform batch precheck is enabled for routine offline discovery.
    pub batch_detection_enabled: bool,
}

impl Default for ResolvedActorConfig {
    fn default() -> Self {
        Self {
            streamer_name: String::new(),
            streamer_url: String::new(),
            platform: String::new(),
            avatar_url: None,
            check_interval_ms: DEFAULT_CHECK_INTERVAL_MS,
            offline_check_interval_ms: DEFAULT_OFFLINE_CHECK_DELAY_MS,
            offline_check_count: DEFAULT_OFFLINE_CHECK_COUNT,
            priority: Priority::Normal,
            min_segment_size_bytes: 0,
            record_danmu: false,
            end_stream_on_danmu_stream_closed: true,
            managed_credentials: false,
            credential_policy_generation: None,
            download_retry_policy: RetryPolicy::default(),
            batch_detection_enabled: false,
        }
    }
}

impl ResolvedActorConfig {
    /// Build a resolved actor config from hydrated streamer metadata and merged config.
    pub fn from_metadata_and_merged(
        metadata: &StreamerMetadata,
        merged: &MergedConfig,
        check_interval_ms: u64,
    ) -> Self {
        let end_stream_on_danmu_stream_closed = merged
            .platform_extras
            .as_ref()
            .and_then(|extras| extras.get("end_stream_on_danmu_stream_closed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let batch_detection_enabled = merged
            .platform_extras
            .as_ref()
            .and_then(|extras| extras.get("batch_detection"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        Self {
            streamer_name: metadata.name.clone(),
            streamer_url: metadata.url.clone(),
            platform: metadata.platform().to_owned(),
            avatar_url: metadata.avatar_url.clone(),
            check_interval_ms,
            offline_check_interval_ms: merged.offline_check_delay_ms,
            offline_check_count: merged.offline_check_count,
            priority: metadata.priority,
            min_segment_size_bytes: u64::try_from(merged.min_segment_size_bytes).unwrap_or(0),
            record_danmu: merged.record_danmu,
            end_stream_on_danmu_stream_closed,
            managed_credentials: merged.credential_policy.is_some(),
            credential_policy_generation: merged
                .credential_policy
                .as_ref()
                .map(|policy| policy.generation.clone()),
            download_retry_policy: merged.download_retry_policy.clone(),
            batch_detection_enabled,
        }
    }
}

/// Persisted streamer snapshot used to initialize [`crate::recorder::StreamerFsm`].
#[derive(Debug, Clone)]
pub struct InitialStreamerSnapshot {
    pub streamer_id: String,
    pub state: StreamerState,
    pub deleted: bool,
    pub consecutive_error_count: i32,
    pub disabled_until: Option<Timestamp>,
    pub last_error: Option<String>,
    pub config: ResolvedActorConfig,
    pub config_revision: u64,
}

/// Why a status or credential probe is being executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProbePurpose {
    /// Routine discovery when no session is open.
    Discovery,
    /// Bound liveness poll while a session is open.
    BoundPoll,
    /// Freshness extraction after waiting in the download queue.
    QueueStart,
    /// Credential recovery diagnostic after an auth/exit download failure.
    Recovery,
}

/// Request emitted by the FSM to probe streamer status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRequest {
    pub probe_id: u64,
    pub streamer_id: String,
    pub purpose: ProbePurpose,
    pub session_id: Option<String>,
    pub credential_binding: Option<CredentialBinding>,
    pub is_live_watchdog: bool,
}

/// Extracted live media payload returned by a probe.
///
/// `Debug` redacts headers, extras, credentials, and signed stream URLs.
#[derive(Clone)]
pub struct LiveProbePayload {
    pub title: String,
    pub category: Option<String>,
    pub avatar: Option<String>,
    pub streams: Vec<StreamInfo>,
    pub media_headers: Option<HashMap<String, String>>,
    pub media_extras: Option<HashMap<String, String>>,
    pub credential_binding: Option<CredentialBinding>,
    pub credential_snapshot: Option<Arc<CredentialSnapshot>>,
    pub next_check_hint: Option<Timestamp>,
}

impl std::fmt::Debug for LiveProbePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveProbePayload")
            .field("title", &self.title)
            .field("category", &self.category)
            .field("stream_count", &self.streams.len())
            .field("credential_binding", &self.credential_binding)
            .field("has_snapshot", &self.credential_snapshot.is_some())
            .field("next_check_hint", &self.next_check_hint)
            .field("network_material", &"[redacted]")
            .finish()
    }
}

/// Outcome of a successful probe.
#[derive(Debug, Clone)]
pub enum ProbeStatus {
    /// Streamer is live and passed configured filters.
    Live(Box<LiveProbePayload>),
    /// Streamer is offline.
    Offline {
        /// Whether a batch precheck live watch hint is armed for early wake.
        batch_live_hint_armed: bool,
    },
    /// Streamer is live but excluded by a filter.
    Filtered {
        reason: FilterReason,
        title: String,
        category: Option<String>,
    },
    /// Permanent platform status requiring manual intervention.
    Fatal {
        kind: FatalErrorType,
        message: String,
    },
}

/// Failure modes returned by a probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeFailure {
    /// Retryable transport, timeout, or parser error.
    Transient { message: String },
    /// Non-retryable probe error.
    Permanent { message: String },
    /// Managed credential policy had no usable account available.
    CredentialUnavailable {
        reason: UnavailableReason,
        policy_generation: String,
    },
    /// Bound credential profile or policy changed mid-probe.
    CredentialSourceChanged,
}

pub use crate::domain::notification::{OutboxEvent, StreamOnlineKind};

/// Whether `StartOrResumeSession` created a fresh row or reused an active one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStartDisposition {
    /// A fresh `live_sessions` row was inserted.
    Created,
    /// An existing open `live_sessions` row (`end_time IS NULL`) was reused.
    ReusedActive,
}

/// Durable database mutation emitted by [`crate::recorder::StreamerFsm`].
///
/// An actor executes at most one `DbCommitOp` in flight at a time.
#[derive(Debug, Clone)]
pub enum DbCommitOp {
    /// Atomically start a new session or reuse an open one, mark the streamer
    /// `LIVE`, commit any initial credential binding, and enqueue `StreamOnline`.
    StartOrResumeSession {
        streamer_id: String,
        streamer_name: String,
        title: String,
        category: Option<String>,
        current_avatar: Option<String>,
        new_avatar: Option<String>,
        credential_binding: Option<CredentialBinding>,
        now: Timestamp,
    },
    /// Atomically re-mark the streamer `LIVE` (checking inactive guard), append
    /// any new title, write resume audit rows, and enqueue `StreamOnline`
    /// (`HysteresisResumed`) for an existing open session.
    ResumeHysteresis {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        title: String,
        category: Option<String>,
        hysteresis_duration_secs: u64,
        now: Timestamp,
    },
    /// Atomically end an active session (if open), write the `session_ended`
    /// audit row, optionally update streamer state / errors / infra-block, and
    /// enqueue `StreamOffline` only when the row transitioned `NULL -> Some`
    /// and `suppress_offline_notification` is `false`.
    EndSession {
        streamer_id: String,
        streamer_name: String,
        session_id: Option<String>,
        cause: TerminalCause,
        via_hysteresis: bool,
        update_streamer_state: Option<StreamerState>,
        clear_errors: bool,
        checks_restored_threshold: Option<i32>,
        infra_block: Option<InfraBlockReason>,
        suppress_offline_notification: bool,
        now: Timestamp,
    },
    /// End a captured session only if its exact `(session_id, binding)` is still
    /// open and undergoing credential profile retirement.
    EndForCredentialRetirement {
        streamer_id: String,
        streamer_name: String,
        session_id: String,
        binding: CredentialBinding,
        via_hysteresis: bool,
        now: Timestamp,
    },
    /// Retroactively reattribute a recent ambiguous `via_hysteresis` audit row
    /// (`Completed` / `Failed`) to `UserDisabled` without modifying `end_time`
    /// or re-emitting notifications/pipelines.
    RewriteEndedCause {
        session_id: String,
        new_cause: TerminalCauseDto,
    },
    /// Persist a newly extracted or rotated credential binding on an open
    /// session, returning the authoritative committed `epoch`.
    CommitCredentialBinding {
        streamer_id: String,
        session_id: String,
        binding: CredentialBinding,
    },
    /// Persist a non-stopping filtered state (`TitleMismatch` / `CategoryMismatch`).
    RecordFilteredState {
        streamer_id: String,
        streamer_name: String,
        old_state: StreamerState,
        new_state: StreamerState,
        reason: String,
        now: Timestamp,
    },
    /// End any open session, set fatal streamer state + reason, and enqueue `FatalError`.
    RecordFatalError {
        streamer_id: String,
        streamer_name: String,
        session_id: Option<String>,
        error_type: FatalErrorType,
        message: String,
        new_state: StreamerState,
        now: Timestamp,
    },
    /// Increment consecutive error count, persist `disabled_until` if the
    /// backoff threshold is met, and enqueue `ChecksFailing` on exact crossing.
    RecordTransientError {
        streamer_id: String,
        streamer_name: String,
        platform: String,
        error: String,
        offline_check_count: u32,
        now: Timestamp,
    },
    /// Clear accumulated error state, optionally update streamer state, and
    /// enqueue `ChecksRestored` if the cleared run had reached the threshold.
    ClearErrors {
        streamer_id: String,
        streamer_name: String,
        checks_restored_threshold: i32,
        set_state: Option<StreamerState>,
        now: Timestamp,
    },
    /// Clear only `last_error` without modifying error count or backoff.
    ClearLastError { streamer_id: String },
    /// Persist an infrastructure block (`CircuitBreaker` or `OutputRootUnavailable`)
    /// without incrementing `consecutive_error_count`.
    SetInfraBlocked {
        streamer_id: String,
        reason: InfraBlockReason,
        now: Timestamp,
    },
    /// Append a mid-session title update (e.g. from danmu `RoomInfoChanged`).
    AddSessionTitle {
        session_id: String,
        title: String,
        now: Timestamp,
    },
    /// Record a best-effort `hysteresis_entered` audit row.
    RecordHysteresisEntered {
        streamer_id: String,
        session_id: String,
        cause: TerminalCauseDto,
        resume_deadline: Timestamp,
        occurred_at: Timestamp,
    },
}

/// Authoritative outcome of a committed [`DbCommitOp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbCommitOutcome {
    /// `StartOrResumeSession` committed with an authoritative `session_id` and
    /// optional committed `credential_binding` (carrying the DB-assigned `epoch`).
    SessionStarted {
        session_id: String,
        started_at: Timestamp,
        disposition: SessionStartDisposition,
        credential_binding: Option<CredentialBinding>,
    },
    /// `StartOrResumeSession` was suppressed by the DB-level inactive/deleted guard.
    SessionStartSuppressed {
        state: StreamerState,
        retry_after: Option<Duration>,
    },
    /// `ResumeHysteresis` succeeded for the existing open session.
    HysteresisResumed {
        session_id: String,
        started_at: Timestamp,
    },
    /// `ResumeHysteresis` was suppressed by the DB-level inactive/deleted guard.
    HysteresisResumeSuppressed {
        state: StreamerState,
        retry_after: Option<Duration>,
    },
    /// `EndSession` committed.
    SessionEnded {
        session_id: Option<String>,
        started_at: Option<Timestamp>,
        ended_at: Timestamp,
        /// Whether the session row transitioned from `end_time IS NULL` to `Some`.
        transitioned_from_open: bool,
        offline_event_emitted: bool,
        checks_restored_emitted: bool,
    },
    /// `EndForCredentialRetirement` committed.
    CredentialRetirementEnded { session_id: String, ended: bool },
    /// `RewriteEndedCause` committed.
    EndedCauseRewritten { session_id: String, updated: bool },
    /// `CommitCredentialBinding` committed with the authoritative `epoch`.
    CredentialBindingCommitted {
        session_id: String,
        binding: CredentialBinding,
    },
    /// `RecordTransientError` committed.
    TransientErrorRecorded {
        consecutive_error_count: i32,
        disabled_until: Option<Timestamp>,
        checks_failing_emitted: bool,
    },
    /// `ClearErrors` committed.
    ErrorsCleared { checks_restored_emitted: bool },
    /// `SetInfraBlocked` committed.
    InfraBlockedRecorded {
        state: StreamerState,
        disabled_until: Timestamp,
    },
    /// `RecordFatalError` committed.
    FatalErrorRecorded {
        session_ended: Option<String>,
        new_state: StreamerState,
    },
    /// `RecordFilteredState` committed.
    FilteredStateRecorded,
    /// `AddSessionTitle` committed.
    SessionTitleAdded { changed: bool },
    /// `ClearLastError` committed.
    LastErrorCleared,
    /// `RecordHysteresisEntered` committed.
    HysteresisEnteredRecorded,
}

/// Failure of a [`DbCommitOp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbCommitFailure {
    /// Transient database or I/O error (retryable).
    Transient { message: String },
    /// Credential binding commit failed because the session ended or policy/revision changed.
    CredentialBindingConflict { message: String },
}

/// Persisted video segment record decoupled from attempt generation control.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSegmentRecord {
    pub session_id: String,
    pub streamer_id: String,
    pub segment_index: u32,
    pub segment_path: String,
    pub started_at: Option<Timestamp>,
    pub completed_at: Timestamp,
    pub duration_secs: f64,
    pub size_bytes: u64,
    pub split_reason_code: Option<String>,
    pub split_reason_details_json: Option<String>,
}

/// Persisted danmu XML segment record decoupled from attempt generation control.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanmuSegmentRecord {
    pub session_id: String,
    pub streamer_id: String,
    pub segment_index: u32,
    pub output_path: String,
    pub message_count: u64,
    pub size_bytes: u64,
}

/// Specification to launch one supervised download attempt.
///
/// `Debug` redacts embedded media headers, extras, and credentials.
#[derive(Clone)]
pub struct AttemptStartSpec {
    pub attempt_id: u64,
    pub session_id: String,
    pub streamer_id: String,
    pub streamer_name: String,
    pub title: String,
    pub payload: Option<LiveProbePayload>,
    pub credential_binding: Option<CredentialBinding>,
    pub from_hysteresis_resume: bool,
    pub priority: Priority,
    /// `true` when the payload originates from a 30s-bounded credential diagnostic.
    pub using_recovered_media: bool,
}

impl std::fmt::Debug for AttemptStartSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttemptStartSpec")
            .field("attempt_id", &self.attempt_id)
            .field("session_id", &self.session_id)
            .field("streamer_id", &self.streamer_id)
            .field("from_hysteresis_resume", &self.from_hysteresis_resume)
            .field("priority", &self.priority)
            .field("using_recovered_media", &self.using_recovered_media)
            .field("credential_binding", &self.credential_binding)
            .field("has_payload", &self.payload.is_some())
            .finish()
    }
}

/// Why an attempt failed before entering the `Running` state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptStartFailure {
    /// Cancelled before engine start (by session stop or shutdown).
    Cancelled,
    /// Session binding epoch/identity no longer matches what was handed off.
    StaleBinding,
    /// Bound credential profile changed mid-startup; one fresh bound retry is allowed.
    CredentialsChanged,
    /// No eligible account is currently available; park in `WaitingCredentials`.
    CredentialsUnavailable,
    /// Scheduler feedback admission capacity is full; wait for `RetryAdmission`.
    SchedulerFeedbackBusy,
    /// Freshness recheck showed streamer is no longer live or has no streams.
    NotLiveAfterQueue,
    /// Engine circuit breaker rejected the start.
    CircuitBreaker {
        reason: String,
        retry_after_secs: u64,
    },
    /// Output-root write gate rejected the start.
    OutputRootUnavailable {
        path: PathBuf,
        io_kind: IoErrorKindSer,
        reason: String,
        retry_after_secs: u64,
    },
    /// Streamer error backoff rejected the start.
    StreamerBackoff {
        reason: String,
        retry_after_secs: u64,
    },
    /// Other startup error.
    Other { message: String },
}

/// Command to close a danmu XML segment and optionally discard it at source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanmuCloseSegmentSpec {
    pub session_id: String,
    pub segment_index: u32,
    /// When `true`, the danmu runner deletes the XML file and suppresses
    /// `DanmuEvent::SegmentCompleted` before acknowledging.
    pub discard: bool,
}

/// Reason a danmu collection is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DanmuStopKind {
    /// Session ended; clean up checkpoints.
    SessionEnded,
    /// Process shutdown; retain checkpoints for restart recovery.
    ServiceShutdown,
}

/// Result of attempting to stop a danmu collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DanmuStopResult {
    /// Collector task exited and released its registry slot.
    Terminated,
    /// Collector was already gone before the stop request.
    NotRegistered,
    /// Stop timed out or could not prove the collector exited.
    Unconfirmed { error: String },
}

/// The five post-commit notifications forwarded to the pipeline engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineNotification {
    VideoSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
        auto_thumbnail_candidate: bool,
    },
    DanmuCollectionStarted {
        session_id: String,
        streamer_id: String,
    },
    DanmuSegmentCompleted {
        session_id: String,
        streamer_id: String,
        segment_index: u32,
        path: PathBuf,
        message_count: u64,
    },
    DanmuCollectionStopped {
        session_id: String,
        total_count: u64,
    },
    SessionEnded {
        session_id: String,
        streamer_id: String,
        cause: TerminalCause,
        should_run_session_complete: bool,
    },
}

impl From<PipelineNotification> for crate::pipeline::PipelineNotification {
    fn from(notification: PipelineNotification) -> Self {
        match notification {
            PipelineNotification::VideoSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                auto_thumbnail_candidate,
            } => Self::VideoSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                auto_thumbnail_candidate,
            },
            PipelineNotification::DanmuCollectionStarted {
                session_id,
                streamer_id,
            } => Self::DanmuCollectionStarted {
                session_id,
                streamer_id,
            },
            PipelineNotification::DanmuSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                message_count,
            } => Self::DanmuSegmentCompleted {
                session_id,
                streamer_id,
                segment_index,
                path,
                message_count,
            },
            PipelineNotification::DanmuCollectionStopped {
                session_id,
                total_count,
            } => Self::DanmuCollectionStopped {
                session_id,
                total_count,
            },
            PipelineNotification::SessionEnded {
                session_id,
                streamer_id,
                cause,
                should_run_session_complete,
            } => Self::SessionEnded {
                session_id,
                streamer_id,
                cause: cause.as_str().to_owned(),
                should_run_session_complete,
            },
        }
    }
}

/// Why a streamer actor terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActorStopReason {
    /// Streamer was disabled by the user or configuration.
    Disabled,
    /// Streamer entered a fatal platform state (`NotFound` / `FatalError`).
    FatalError { state: StreamerState },
    /// Deleted streamer retirement completed or hit its bound.
    Retired {
        /// `true` only when every physical lease and DB write settled cleanly.
        quiescent: bool,
    },
    /// Process-wide shutdown.
    Shutdown,
}

/// Unidirectional read-only runtime projection published for API/MCP views and
/// `DanmuServiceProbe` health checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamerRuntimeView {
    pub streamer_id: String,
    pub monitor_state: StreamerState,
    pub active_session_id: Option<String>,
    pub recording_started_at: Option<Timestamp>,
    pub in_hysteresis: bool,
    pub attempt_active: bool,
    pub attempt_queued: bool,
    pub expects_danmu: bool,
    pub danmu_collecting: bool,
    pub danmu_link_down_since: Option<Instant>,
    pub lingering_physical_lease: bool,
    pub config_revision: u64,
}

/// Narrow async port for probing streamer status.
#[async_trait]
pub trait RecorderProberPort: Send + Sync {
    async fn probe(
        &self,
        request: ProbeRequest,
    ) -> crate::Result<std::result::Result<ProbeStatus, ProbeFailure>>;
}

/// Narrow async port for serialized per-streamer database commits and segment persistence.
#[async_trait]
pub trait RecorderStorePort: Send + Sync {
    async fn commit(&self, op: DbCommitOp)
    -> std::result::Result<DbCommitOutcome, DbCommitFailure>;

    async fn persist_video_segment(&self, record: VideoSegmentRecord) -> crate::Result<()>;

    async fn persist_danmu_segment(&self, record: DanmuSegmentRecord) -> crate::Result<()>;

    async fn remove_danmu_segment_output(
        &self,
        session_id: &str,
        danmu_path: &str,
    ) -> crate::Result<()>;

    async fn next_session_segment_index(&self, session_id: &str) -> crate::Result<u32>;
}

/// Narrow async port for managing supervised download attempts.
#[async_trait]
pub trait RecorderDownloadPort: Send + Sync {
    async fn start_attempt(&self, spec: AttemptStartSpec) -> crate::Result<()>;

    async fn stop_attempt(
        &self,
        attempt_id: u64,
        download_id: Option<&str>,
        cause: crate::downloader::DownloadStopCause,
    ) -> crate::Result<()>;
}

/// Narrow async port for danmu collection and segment synchronization.
#[async_trait]
pub trait RecorderDanmuPort: Send + Sync {
    async fn start_segment(
        &self,
        session_id: &str,
        segment_index: u32,
        video_segment_path: &str,
        started_at: Timestamp,
    ) -> crate::Result<()>;

    async fn close_segment(&self, spec: DanmuCloseSegmentSpec) -> crate::Result<()>;

    async fn stop_collection(
        &self,
        session_id: &str,
        reason: DanmuStopKind,
    ) -> crate::Result<DanmuStopResult>;
}

/// Narrow async port for forwarding post-commit pipeline notifications.
#[async_trait]
pub trait RecorderPipelinePort: Send + Sync {
    async fn notify(&self, notification: PipelineNotification) -> crate::Result<()>;
}

/// Read-only runtime view publisher.
pub trait RecorderRuntimeSink: Send + Sync {
    fn publish_view(&self, view: StreamerRuntimeView);
}
