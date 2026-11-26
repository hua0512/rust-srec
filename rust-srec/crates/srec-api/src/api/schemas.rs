//! Transport-layer OpenAPI (`utoipa`) and JSON Schema (`schemars`) mirrors for
//! domain and database types.
//!
//! Keeping schema derivation here ensures core domain, database, credential,
//! proxy, notification, and monitoring modules stay completely free of
//! `utoipa` and `schemars` dependencies while preserving identical OpenAPI
//! component names and JSON wire schemas. Each mirror has the name of the type
//! it copies; `rust-srec/tests/integration/schema_mirrors.rs` fails when their
//! fields, field types or `serde` settings drift apart.

#![allow(dead_code)]

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

// ============================================================================
// Health & Metrics
// ============================================================================

/// Filesystem capacity for one monitored path.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DiskUsage {
    /// Monitored path, matching the `disk:` component name suffix.
    pub path: String,
    /// Mount point of the filesystem `path` resolves to.
    pub mount_point: String,
    /// Filesystem size in bytes.
    pub total_bytes: u64,
    /// Bytes still writable.
    pub available_bytes: u64,
    /// `total_bytes - available_bytes`.
    pub used_bytes: u64,
    /// Percentage of `total_bytes` that is not available (0-100).
    pub used_percent: f32,
}

// ============================================================================
// Streamer & Priority
// ============================================================================

/// Priority level for resource allocation.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    /// VIP streamers, never miss. First to get download slots, last to be paused.
    High,
    /// Standard streamers. Fair scheduling.
    Normal,
    /// Background/archive streamers. Paused first during resource constraints.
    Low,
}

/// Streamer operational states.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StreamerState {
    /// The streamer is offline.
    NotLive,
    /// The streamer is currently live.
    Live,
    /// The streamer is online but outside the time window defined by filters.
    OutOfSchedule,
    /// The system has detected insufficient disk space.
    OutOfSpace,
    /// A persistent error is preventing monitoring.
    FatalError,
    /// The streamer's URL or ID is invalid on the platform.
    NotFound,
    /// The system is currently checking the streamer's status.
    InspectingLive,
    /// The streamer has been temporarily disabled due to repeated errors.
    TemporalDisabled,
    /// The streamer is in an error state (generic).
    Error,
    /// The streamer has been manually disabled.
    Disabled,
}

// ============================================================================
// Sessions
// ============================================================================

/// A high-confidence offline signal observed at the engine or danmu boundary
/// that bypasses the hysteresis quiet period.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", content = "count", rename_all = "snake_case")]
pub enum OfflineSignal {
    /// Danmu websocket sent an explicit stream-closed control frame.
    DanmuStreamClosed,
    /// `n` consecutive network failures inside the sliding failure window.
    ConsecutiveFailures(u32),
}

/// Typed payload for a session event.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEventPayload {
    /// A new session began. `from_hysteresis` is `true` only on the resume path.
    SessionStarted {
        from_hysteresis: bool,
        title: Option<String>,
    },
    /// The download ended with a non-authoritative cause; the lifecycle is
    /// waiting for either a resume or the backstop timer to elapse before
    /// committing `Ended`.
    HysteresisEntered {
        cause: TerminalCauseDto,
        resume_deadline: Timestamp,
    },
    /// A `LiveDetected` arrived inside the hysteresis window — the session
    /// continues with the same `session_id`.
    SessionResumed { hysteresis_duration_secs: u64 },
    /// The session committed to `Ended`.
    SessionEnded {
        cause: TerminalCauseDto,
        via_hysteresis: bool,
    },
}

/// Wire-format DTO for a session terminal cause.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalCauseDto {
    Completed,
    Failed { kind: String },
    Cancelled { cause: String },
    Rejected { reason: String },
    StreamerOffline,
    DefinitiveOffline { signal: OfflineSignal },
    UserDisabled,
    OutOfSchedule,
}

// ============================================================================
// Engines
// ============================================================================

/// Engine configuration database model.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct EngineConfigurationDbModel {
    pub id: String,
    pub name: String,
    /// Engine type: FFMPEG, STREAMLINK, MESIO
    pub engine_type: String,
    /// JSON blob for engine-specific configuration
    pub config: String,
}

/// Engine types.
#[derive(Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EngineType {
    Ffmpeg,
    Streamlink,
    Mesio,
}

// ============================================================================
// Pipeline, Jobs, Presets & Progress
// ============================================================================

/// Job execution status.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobStatus {
    /// Job is queued and waiting to be picked up by a worker.
    Pending,
    /// Job is currently being executed.
    Processing,
    /// Job finished successfully.
    Completed,
    /// Job failed after exhausting retries.
    Failed,
    Cancelled,
}

/// Pipeline step configuration.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PipelineStep {
    /// Reference to a named job preset (e.g., "remux").
    Preset {
        /// Name of the job preset to use.
        name: String,
    },
    /// Reference to a pipeline workflow (expands to multiple steps).
    Workflow {
        /// Name of the pipeline preset/workflow to expand.
        name: String,
    },
    /// Inline definition of a job step.
    Inline {
        /// Processor type (e.g., "remux", "execute").
        processor: String,
        /// Optional configuration for the processor.
        #[serde(default)]
        config: serde_json::Value,
    },
}

/// Automatic retries of a workflow step whose job failed.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct StepRetryPolicy {
    /// Attempts in total, including the first run; `1` means no retry.
    pub max_attempts: u32,
    /// Wait before the second attempt, in seconds, doubled for every further
    /// attempt and capped at six hours.
    #[serde(default)]
    pub backoff_secs: u64,
}

/// A step within a DAG pipeline with explicit dependencies.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagStep {
    /// Unique step identifier within the DAG (e.g., "remux", "upload", "notify").
    pub id: String,
    /// The processor or preset to run for this step.
    pub step: PipelineStep,
    /// IDs of steps this depends on (fan-in: waits for all to complete).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Automatic retries when this step's job fails; none by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<StepRetryPolicy>,
    /// Timeout for one attempt of this step's job, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// DAG pipeline definition with named steps and explicit dependencies.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct DagPipelineDefinition {
    /// Unique name for the DAG pipeline.
    pub name: String,
    /// All steps in the DAG.
    pub steps: Vec<DagStep>,
}

/// Job Preset configuration.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobPreset {
    /// Unique identifier (UUID).
    pub id: String,
    /// Unique name of the preset (e.g., "hq_remux", "fast_upload").
    pub name: String,
    /// Optional description of what this preset does.
    pub description: Option<String>,
    /// Category for organizing presets (e.g., "remux", "compression", "thumbnail").
    pub category: Option<String>,
    /// Processor type this preset applies to (e.g., "remux", "upload").
    pub processor: String,
    /// JSON blob for the processor configuration.
    pub config: String,
    /// Creation timestamp.
    pub created_at: Timestamp,
    /// Last update timestamp.
    pub updated_at: Timestamp,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProgressKind {
    Ffmpeg,
    Rclone,
    BaiduPcs,
    Compression,
    Copy,
    Danmaku,
}

/// A stage of a job that is not plain work towards its percent.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum JobPhase {
    Finalizing,
    Retrying,
    Preparing,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct JobProgressSnapshot {
    pub kind: ProgressKind,
    pub updated_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_done: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes_total: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_bytes_per_sec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_time_ms: Option<u64>,
    /// Media length in milliseconds that the FFmpeg run writes; the total for `out_time_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// FFmpeg processing speed as a multiple of real time, such as 2.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_x: Option<f64>,
    /// Files finished so far.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_done: Option<u64>,
    /// Files the job counts towards its progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files_total: Option<u64>,
    /// Absent while the job does plain work towards `percent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<JobPhase>,
    /// Name of the file in progress, without its directories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    /// The processor's own attempt within one job run, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    /// Attempts the processor makes at most within this job run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub raw: serde_json::Value,
}

/// What kind of configuration owns a pipeline that names a step.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StepUserKind {
    Global,
    Platform,
    Template,
    Streamer,
    Workflow,
}

/// A stored pipeline owner that names a step definition.
#[derive(Serialize, utoipa::ToSchema)]
pub struct StepUser {
    pub kind: StepUserKind,
    /// The owner's id; empty for the global config.
    pub id: String,
    /// The owner's name; empty for the global config.
    pub name: String,
    /// The owner is being removed but still runs the pipelines owed to its
    /// last session.
    pub removing: bool,
}

// ============================================================================
// Proxies
// ============================================================================

/// The connection a scope chooses.
#[derive(Default, Serialize, Deserialize, utoipa::ToSchema, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schemars(deny_unknown_fields)]
pub enum ProxyRoute {
    #[default]
    Inherit,
    /// Connect without any proxy, ignoring proxy environment variables.
    Direct,
    /// Use the proxy the environment configures, if any.
    System,
    /// Connect through a saved proxy.
    Proxy {
        #[schemars(regex(pattern = r"\S"))]
        id: String,
    },
}

/// Which setting decided a request's route.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Account,
    Streamer,
    Template,
    Platform,
    Global,
}

/// How a resolved route connects, as the API reports it.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Direct,
    System,
    Proxy,
}

/// A saved proxy as routes and notifications name it.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProxyName {
    pub id: String,
    pub name: String,
}

/// What keeps a proxy entry in use.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ProxyReferences {
    /// The global route names the entry.
    pub global: bool,
    pub platforms: Vec<NamedReference>,
    pub templates: Vec<TemplateReference>,
    pub streamers: Vec<NamedReference>,
    pub accounts: Vec<AccountReference>,
    pub notification_channels: Vec<NamedReference>,
    /// Job presets whose upload step chooses the entry.
    pub job_presets: Vec<NamedReference>,
    /// Configurations and workflows whose pipelines have an inline upload
    /// step choosing the entry.
    pub pipelines: Vec<StepUser>,
}

/// A platform, streamer, notification channel or job preset whose route
/// names the entry.
#[derive(Serialize, utoipa::ToSchema)]
pub struct NamedReference {
    pub id: String,
    pub name: String,
}

/// A template whose route names the entry.
#[derive(Serialize, utoipa::ToSchema)]
pub struct TemplateReference {
    pub id: String,
    pub name: String,
    /// The template was deleted and is kept only until the recordings using
    /// it finish; it releases the entry then.
    pub being_removed: bool,
}

/// An account whose own route names the entry.
#[derive(Serialize, utoipa::ToSchema)]
pub struct AccountReference {
    pub id: String,
    pub label: String,
    pub platform_id: String,
    pub platform_name: String,
}

/// What the API shows of the environment's proxy.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SystemProxySummary {
    pub detected: bool,
    /// `scheme://host[:port]`, without any login.
    pub url: Option<String>,
    pub authenticated: bool,
    pub no_proxy: Option<String>,
}

/// Why a check failed.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// The proxy refused the login (HTTP 407).
    ProxyAuthenticationRequired,
    /// The proxy, or the site through it, could not be reached.
    ConnectFailed,
    /// No answer within the time limit.
    Timeout,
    /// The TLS handshake with the site failed.
    Tls,
    /// Any other failure.
    Failed,
}

/// The result of one check.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ProbeOutcome {
    /// The site answered through the proxy. Any HTTP status except 407 counts.
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<ProbeErrorKind>,
}

// ============================================================================
// Notifications
// ============================================================================

/// Notification channel database model.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelDbModel {
    pub id: String,
    pub name: String,
    /// Channel type: Discord, Email, Telegram, Webhook
    pub channel_type: String,
    /// JSON blob for channel-specific settings
    pub settings: String,
    /// How deliveries connect; `inherit` follows the global route.
    #[serde(default)]
    pub proxy_route: ProxyRoute,
}

/// Notification channel types.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "PascalCase")]
pub enum ChannelType {
    Discord,
    Email,
    Gotify,
    Telegram,
    Webhook,
}

/// Notification event log database model.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationEventLogDbModel {
    pub id: String,
    pub event_type: String,
    pub priority: i32,
    /// JSON blob of the event payload
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streamer_id: Option<String>,
    /// Unix epoch milliseconds (UTC) when the event occurred.
    pub created_at: i64,
}

/// Web Push subscription database model.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct WebPushSubscriptionDbModel {
    pub id: String,
    pub user_id: String,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    /// Minimum priority to send (integer, Gotify-compatible 0-10 scale).
    pub min_priority: i32,
    pub created_at: i64,
    pub updated_at: i64,
    /// Optional next allowed attempt time.
    pub next_attempt_at: Option<i64>,
    /// Last time we received a 429 from the push service.
    pub last_429_at: Option<i64>,
}

/// Priority level for notifications (integer, 0-10 scale).
#[derive(Serialize)]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl utoipa::PartialSchema for NotificationPriority {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{Object, SchemaType, Type};
        Object::builder()
            .schema_type(SchemaType::Type(Type::Integer))
            .description(Some(
                "Priority level (integer, 0-10 scale): 2=Low, 5=Normal, 8=High, 10=Critical",
            ))
            .enum_values(Some([2i32, 5, 8, 10]))
            .build()
            .into()
    }
}

impl utoipa::ToSchema for NotificationPriority {}

/// Static metadata about a supported notification event type.
#[derive(Serialize, utoipa::ToSchema)]
pub struct NotificationEventTypeInfo {
    /// Canonical subscription key (snake_case).
    pub event_type: &'static str,
    /// Human-friendly label.
    pub label: &'static str,
    /// Default priority level (integer, Gotify-compatible 0-10 scale).
    pub priority: NotificationPriority,
    /// Additional accepted subscription keys (legacy / aliases).
    pub aliases: &'static [&'static str],
}

/// Public view of a configured notification channel instance.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelInstance {
    /// Unique channel instance key.
    pub key: String,
    /// DB channel id, when backed by `notification_channel`.
    pub channel_id: Option<String>,
    /// Human-friendly display name.
    pub display_name: String,
    /// Channel type (Discord/Email/Webhook).
    pub channel_type: String,
    /// Channel configuration source.
    pub source: NotificationChannelSource,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannelSource {
    Config,
    Dynamic,
    Database,
}

// ============================================================================
// Credentials & Login Sessions
// ============================================================================

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PoolStrategy {
    RoundRobin,
    Priority,
}

/// A scope that stores no selection inherits; at the platform that is anonymous.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialSelection {
    Inherit,
    None,
    Fixed {
        credential_id: String,
    },
    Pool {
        credential_ids: Vec<String>,
        strategy: PoolStrategy,
        failover: bool,
        max_attempts: u8,
        /// Check without an account when no account in the pool can be used.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        anonymous_fallback: bool,
    },
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialOwner {
    Platform { platform_id: String },
    Template { template_id: String },
    Streamer { streamer_id: String },
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolvedCredentialPolicy {
    pub platform_id: String,
    pub owner: CredentialOwner,
    pub selection: CredentialSelection,
    /// Opaque digest of canonical policy/owner/platform, independent of account material.
    pub generation: String,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialIdentity {
    Profile { profile_id: String },
    Anonymous,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialBinding {
    pub identity: CredentialIdentity,
    pub revision: u64,
    pub policy: ResolvedCredentialPolicy,
    pub epoch: u64,
}

/// What keeps a profile in use.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ProfileReferences {
    pub selections: Vec<SelectionReference>,
    pub recordings: Vec<RecordingReference>,
}

/// A scope whose selection lists the profile, with its display name.
#[derive(Serialize, utoipa::ToSchema)]
pub struct SelectionReference {
    pub owner: CredentialOwner,
    /// The platform's, template's or streamer's name.
    pub name: String,
    /// The platform the selection chooses accounts on.
    pub platform_id: String,
    pub platform_name: String,
}

/// A live recording bound to the profile.
#[derive(Serialize, utoipa::ToSchema)]
pub struct RecordingReference {
    pub session_id: String,
    /// Absent once the streamer itself has been deleted.
    pub streamer_id: Option<String>,
    /// The streamer's current name, else the name it was recorded under.
    pub streamer_name: String,
}

/// Input-only credential material.
#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialMaterial {
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub reauth_config: Option<serde_json::Value>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialProfileSummary {
    pub id: String,
    pub platform_config_id: String,
    pub label: String,
    pub enabled: bool,
    pub revision: i64,
    pub version: i64,
    pub has_cookies: bool,
    pub has_refresh_token: bool,
    pub has_access_token: bool,
    pub has_reauth: bool,
    /// The account's own route; `inherit` follows the recording using it.
    pub proxy_route: ProxyRoute,
    /// When an operation last received the account's material, in epoch milliseconds.
    pub last_used_at: Option<i64>,
}

/// The last conclusion about an account at its current revision.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialValidity {
    Unknown,
    Valid,
    NeedsRefresh,
    Invalid,
}

/// Why the stored health conclusion was reached.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HealthReason {
    /// The provider rejected the account; only a new login restores it.
    LoginRequired,
    /// A user-requested validation produced the conclusion.
    ManualValidation,
    /// A provider refresh failed without demanding a new login.
    RefreshFailed,
    /// A status check found the account needs repair before use.
    RepairRequired,
    /// An extraction was rejected and a repair is queued.
    AuthenticationFailed,
}

/// Why a credential policy has no account to offer right now.
#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    /// The remaining candidates need a new login.
    LoginRequired,
    /// Every candidate is disabled.
    ProfilesDisabled,
    /// A bound session's account cannot be used and switching is not allowed.
    BoundProfileUnavailable,
    /// The selection changed under a bound session that cannot switch.
    BindingPolicyChanged,
    /// The attempt budget ran out before an account succeeded.
    AttemptsExhausted,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialProfileHealth {
    pub profile_id: String,
    pub revision: i64,
    pub validity: CredentialValidity,
    pub last_check_at: Option<i64>,
    pub last_refresh_at: Option<i64>,
    pub refresh_failure_count: i64,
    pub last_failure_at: Option<i64>,
    pub last_notified_failure_count: i64,
    pub reason_code: Option<HealthReason>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct CredentialUnavailable {
    pub reason: UnavailableReason,
    pub policy_generation: String,
}

/// What the user has to do about an account.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// Only a new login or new cookies make the account usable again.
    LoginRequired,
    /// The platform rejected the account and its automatic refresh keeps failing.
    RefreshFailing,
}

/// An enabled account that needs the user, with its platform's name.
#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialAttention {
    pub profile: CredentialProfileSummary,
    pub platform_name: String,
    pub reason: AttentionReason,
    /// Health at the account's current revision.
    pub health: CredentialProfileHealth,
}

/// A Streamlink streamer's site and the accounts that name it.
#[derive(Serialize, utoipa::ToSchema)]
pub struct StreamerSite {
    /// The host of the streamer's URL, without a leading `www.`.
    pub host: String,
    /// The most specific site of an account that covers the host.
    pub site: Option<String>,
    /// The accounts whose sites cover the host, most specific site first.
    pub accounts: Vec<String>,
}

/// Why a streamer's checks found no usable account, and since when.
#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialBlock {
    pub reason: UnavailableReason,
    /// The platform whose accounts the streamer selects from.
    pub platform_id: String,
    /// The first check of the current blocked run.
    pub since: Timestamp,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProfileActionResult {
    pub supported: bool,
    pub profile: CredentialProfileSummary,
    pub health: Option<CredentialProfileHealth>,
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialLoginTarget {
    Create {
        platform_id: String,
        label: String,
        /// The new account's own route; `inherit` signs in through the platform's route.
        #[serde(default)]
        proxy_route: ProxyRoute,
    },
    Replace {
        profile_id: String,
        expected_version: i64,
    },
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialLoginGenerated {
    pub login_id: String,
    pub url: String,
    pub expires_at: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct CredentialLoginReceipt {
    pub status: String,
    pub profile_id: Option<String>,
    pub version: Option<i64>,
}

/// What accounts on a platform support.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ProviderCapabilities {
    /// Profiles may hold a refresh token.
    pub refresh_token: bool,
    /// Profiles may hold an access token.
    pub access_token: bool,
    /// An access token alone authenticates, without cookies.
    pub token_only: bool,
    /// Profiles may hold a username and password for automatic sign-in.
    pub reauth_login: bool,
    /// Accounts can be added by scanning a QR code.
    pub qr_login: bool,
    /// The provider can check whether an account still works.
    pub check: bool,
    /// The provider can repair a rejected account without the user.
    pub refresh: bool,
}

// ============================================================================
// Bigo Notifications Monitor
// ============================================================================

/// The listener's connection state.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ListenerState {
    /// Notifications are off.
    Disabled,
    /// Connecting or reconnecting.
    Connecting,
    Connected,
    /// Not connected, and waiting before the next attempt or for a settings change.
    Error,
}

/// How a Bigo streamer's start is noticed.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MonitoringMethod {
    Notifications,
    Polling,
}

/// Why a Bigo streamer is polled.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollingReason {
    /// The platform's `batch_detection` is off.
    NotificationsOff,
    /// No `notification_account` is chosen.
    NoAccount,
    /// The chosen account is missing, disabled or not a Bigo account.
    AccountUnusable,
    /// BIGO sees the account as signed out.
    SignedOut,
    /// The listener is connecting or waiting to reconnect.
    Disconnected,
    StreamerDisabled,
    /// A template or the streamer turned `batch_detection` off.
    BatchDetectionOff,
    /// The streamer is checked by Streamlink rather than the Bigo extractor.
    StreamlinkExtractor,
    /// No individual check has stored the streamer's uid yet.
    UidUnknown,
    /// The follow and live-notice requests are sent but not answered yet.
    Subscribing,
    /// BIGO refused the follow or the live notices.
    FollowRefused,
    /// BIGO confirmed the subscription but sent no push for the latest starts.
    NotificationsNotArriving,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct StreamerNotificationMethod {
    pub streamer_id: String,
    pub method: MonitoringMethod,
    /// Set exactly when `method` is `polling`.
    pub reason: Option<PollingReason>,
}

/// What `GET /api/config/platforms/bigo/notifications` reports.
#[derive(Serialize, utoipa::ToSchema)]
pub struct BigoNotificationStatus {
    /// The Bigo platform configuration, absent when there is none.
    pub platform_id: Option<String>,
    pub state: ListenerState,
    /// Why the listener is not connected, or `notifications_not_arriving`.
    pub reason: Option<PollingReason>,
    /// The last error, absent while connected.
    pub message: Option<String>,
    /// Streamers whose follow and live notices BIGO confirmed on the current connection.
    pub subscribed: usize,
    /// Enabled streamers that use notifications unless something stops them.
    pub total: usize,
    /// Every Bigo streamer, disabled ones included.
    pub streamers: Vec<StreamerNotificationMethod>,
    /// Starts that a check found on a subscribed streamer without a push from BIGO.
    pub missed_pushes: u64,
    /// When the latest of those was found.
    pub last_missed_at: Option<Timestamp>,
    /// When the listener lost its connection, or began connecting.
    pub down_since: Option<Timestamp>,
}
