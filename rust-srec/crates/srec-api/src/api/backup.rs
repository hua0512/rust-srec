//! Configuration backup port and OpenAPI transport schema definitions.
//!
//! The API layer depends on [`ConfigBackupPort`] rather than the concrete
//! export/import services in `crate::services`. The top-level backend
//! composition module implements the port and binds it into `AppState`.

use crate::api::error::ApiError;
use crate::config::backup::{
    ConfigExport as CoreConfigExport, ImportMode as CoreImportMode, ImportStats as CoreImportStats,
    UserExport as CoreUserExport,
};

/// Errors surfaced across [`ConfigBackupPort`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigBackupError {
    #[error("{0}")]
    Validation(String),
    #[error(transparent)]
    MissingProxy(#[from] crate::proxies::ProxyError),
    #[error("configuration backup database operation failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] crate::Error),
}

impl From<ConfigBackupError> for ApiError {
    fn from(error: ConfigBackupError) -> Self {
        match error {
            ConfigBackupError::Validation(message) => ApiError::bad_request(message),
            ConfigBackupError::MissingProxy(error) => ApiError::from(error),
            ConfigBackupError::Database(error) => ApiError::from(error),
            ConfigBackupError::Other(error) => ApiError::from(error),
        }
    }
}

/// Summary returned by [`ConfigBackupPort::import_config`].
pub struct ConfigBackupImportOutcome {
    pub stats: CoreImportStats,
    pub warnings: Vec<String>,
}

/// Narrow application-layer port for exporting and importing configuration backups.
#[async_trait::async_trait]
pub trait ConfigBackupPort: Send + Sync {
    /// Export the complete configuration backup, combining persisted domain
    /// configuration with caller-supplied auth user exports.
    async fn export_config(
        &self,
        user_exports: Vec<CoreUserExport>,
    ) -> Result<CoreConfigExport, ConfigBackupError>;

    /// Apply a configuration backup transactionally, invoking
    /// `on_users_committed` synchronously inside the post-commit hook when
    /// provided.
    async fn import_config(
        &self,
        config: CoreConfigExport,
        mode: CoreImportMode,
        on_users_committed: Option<Box<dyn FnOnce() + Send + 'static>>,
    ) -> Result<ConfigBackupImportOutcome, ConfigBackupError>;
}

// ============================================================================
// OpenAPI Backup Schema DTOs
//
// Mirror types matching the exact JSON wire format of `crate::config::backup`
// so OpenAPI generation stays completely inside `src/api`.
// ============================================================================

/// Complete configuration export bundle.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ConfigExport {
    /// Schema version for compatibility checking.
    pub version: String,
    /// ISO 8601 timestamp of export.
    pub exported_at: String,
    /// Global configuration.
    pub global_config: GlobalConfigExport,
    /// All templates.
    pub templates: Vec<TemplateExport>,
    /// All streamers with their filters.
    pub streamers: Vec<StreamerExport>,
    /// All engine configurations.
    pub engines: Vec<EngineExport>,
    /// All platform configurations.
    pub platforms: Vec<PlatformExport>,
    /// All notification channels with subscriptions.
    pub notification_channels: Vec<NotificationChannelExport>,
    /// All job presets (processor configurations).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub job_presets: Vec<JobPresetExport>,
    /// All pipeline presets (workflow configurations).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline_presets: Vec<PipelinePresetExport>,
    /// All users (authentication accounts).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<UserExport>,
    /// Secret-bearing profiles, present only in the managed-credential format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub credential_profiles: Vec<CredentialProfileExport>,
    /// Saved proxies, logins included; routes name them. Present only in the
    /// managed-credential format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<ProxyExport>,
}

/// A saved proxy in a backup. Bundles name proxies; IDs stay local to each
/// installation.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProxyExport {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// A route in a backup, naming its proxy.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupRoute {
    Inherit,
    Direct,
    System,
    Proxy { name: String },
}

/// Explicit sensitive backup representation. Ordinary profile DTOs never serialize material.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialProfileExport {
    pub id: String,
    /// Profiles belong to this platform; any scope on it may select them.
    pub platform: String,
    pub label: String,
    pub enabled: bool,
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    pub reauth_config: Option<serde_json::Value>,
    /// The account's own route; absent keeps the stored route on a merge
    /// and inherits on a replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
    /// The sites a Streamlink account is for; absent keeps the stored sites
    /// on a merge and names none on a replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sites: Option<Vec<String>>,
}

/// Global configuration for export (excludes internal ID).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct GlobalConfigExport {
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub min_segment_size_bytes: i64,
    pub max_download_duration_secs: i64,
    pub max_part_size_bytes: i64,
    pub record_danmu: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// JSON `DanmuStatisticsConfig`; absent inherits the layer above.
    pub danmu_statistics: Option<String>,
    pub max_concurrent_downloads: i32,
    pub max_concurrent_uploads: i32,
    pub streamer_check_delay_ms: i64,
    /// Proxy settings of backups without saved proxies; see [`BackupRoute`].
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub proxy_config: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
    pub offline_check_delay_ms: i64,
    pub offline_check_count: i32,
    pub default_download_engine: String,
    /// Absent in backups that predate the setting; resolves to auto.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_extractor: Option<String>,
    pub max_concurrent_cpu_jobs: i32,
    pub max_concurrent_io_jobs: i32,
    pub job_history_retention_days: i32,
    #[serde(default)]
    pub notification_event_log_retention_days: i32,
    #[serde(default)]
    pub output_retention_days: i32,
    #[serde(default)]
    pub output_retention_delete_files: bool,
    pub pipeline: Option<serde_json::Value>,
    pub session_complete_pipeline: Option<serde_json::Value>,
    pub paired_segment_pipeline: Option<serde_json::Value>,
    pub log_filter_directive: Option<String>,
    pub auto_thumbnail: bool,
    /// Maximum execution time (seconds) for a single CPU-bound pipeline job.
    #[serde(default)]
    pub pipeline_cpu_job_timeout_secs: i64,
    /// Maximum execution time (seconds) for a single IO-bound pipeline job.
    #[serde(default)]
    pub pipeline_io_job_timeout_secs: i64,
    /// Maximum execution time (seconds) for the `execute` processor command.
    #[serde(default)]
    pub pipeline_execute_timeout_secs: i64,
    /// Milliseconds a queued download may wait before refetching live state.
    #[serde(default)]
    pub queue_freshness_threshold_ms: i64,
    /// Seconds between `nvidia-smi` probes by the GPU health monitor.
    #[serde(default)]
    pub gpu_health_probe_interval_secs: i64,
    /// Whether the stream proxy may fetch targets on private networks.
    #[serde(default)]
    pub stream_proxy_allow_private_targets: bool,
}

/// Template for export (uses name as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct TemplateExport {
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    /// Read only from backups written before credential profiles; import
    /// converts it into a profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub record_danmu: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// JSON `DanmuStatisticsConfig`; absent inherits the layer above.
    pub danmu_statistics: Option<String>,
    pub platform_overrides: Option<serde_json::Value>,
    pub download_retry_policy: Option<serde_json::Value>,
    pub download_engine: Option<String>,
    /// Absent in backups that predate the setting; inherits the layer above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    pub engines_override: Option<serde_json::Value>,
    /// Proxy settings of backups without saved proxies; see [`BackupRoute`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
    pub stream_selection_config: Option<serde_json::Value>,
    pub pipeline: Option<serde_json::Value>,
    pub session_complete_pipeline: Option<serde_json::Value>,
    pub paired_segment_pipeline: Option<serde_json::Value>,
    #[serde(default)]
    pub offline_check_count: Option<i32>,
    #[serde(default)]
    pub offline_check_delay_ms: Option<i64>,
}

/// Streamer for export (uses URL as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct StreamerExport {
    pub name: String,
    pub url: String,
    /// Platform name (resolved from platform_config_id).
    pub platform: String,
    /// Template name (resolved from template_config_id).
    pub template: Option<String>,
    pub priority: String,
    /// Streamer operational state (e.g. NOT_LIVE, LIVE).
    #[serde(default)]
    pub state: String,
    /// Streamer avatar URL (if known).
    pub avatar_url: Option<String>,
    pub streamer_specific_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
    /// Associated filters.
    pub filters: Vec<FilterExport>,
}

/// Filter for export.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct FilterExport {
    pub filter_type: String,
    pub config: serde_json::Value,
}

/// Engine configuration for export (uses name as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct EngineExport {
    pub name: String,
    pub engine_type: String,
    pub config: serde_json::Value,
}

/// Platform configuration for export (uses platform_name as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PlatformExport {
    pub platform_name: String,
    pub fetch_delay_ms: Option<i64>,
    pub download_delay_ms: Option<i64>,
    /// Read only from backups written before credential profiles; import
    /// converts it into a profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_selection: Option<crate::api::schemas::CredentialSelection>,
    pub platform_specific_config: Option<serde_json::Value>,
    /// Proxy settings of backups without saved proxies; see [`BackupRoute`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_config: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
    pub record_danmu: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// JSON `DanmuStatisticsConfig`; absent inherits the layer above.
    pub danmu_statistics: Option<String>,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub download_engine: Option<String>,
    /// Absent in backups that predate the setting; inherits the layer above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extractor: Option<String>,
    pub stream_selection_config: Option<serde_json::Value>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub download_retry_policy: Option<serde_json::Value>,
    pub pipeline: Option<serde_json::Value>,
    pub session_complete_pipeline: Option<serde_json::Value>,
    pub paired_segment_pipeline: Option<serde_json::Value>,
    #[serde(default)]
    pub offline_check_count: Option<i32>,
    #[serde(default)]
    pub offline_check_delay_ms: Option<i64>,
}

/// Notification channel for export (uses name as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelExport {
    pub name: String,
    pub channel_type: String,
    pub settings: serde_json::Value,
    pub subscriptions: Vec<String>,
    /// How deliveries connect; absent keeps the stored route on a merge and
    /// inherits on a replace. Releases before channel routes ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_route: Option<BackupRoute>,
}

/// Job preset configuration for export.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct JobPresetExport {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub processor: String,
    pub config: serde_json::Value,
}

/// Pipeline preset configuration for export.
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct PipelinePresetExport {
    pub name: String,
    pub description: Option<String>,
    pub dag_definition: Option<serde_json::Value>,
    pub pipeline_type: Option<String>,
}

/// User account for export (uses username as identifier).
#[allow(dead_code)]
#[derive(serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct UserExport {
    pub id: String,
    pub username: String,
    /// Argon2id password hash (sensitive but required for full restore).
    pub password_hash: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub is_active: bool,
    pub must_change_password: bool,
    #[serde(default)]
    pub last_login_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Import mode.
#[allow(dead_code)]
#[derive(Default, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Merge: Update existing entities, add new ones, keep entities not in import.
    #[default]
    Merge,
    /// Replace: Delete all existing entities and import fresh.
    Replace,
}

/// Import request with optional mode.
#[allow(dead_code)]
#[derive(serde::Deserialize, utoipa::ToSchema)]
pub struct ImportRequest {
    /// The configuration data to import.
    pub config: ConfigExport,
    /// Import mode: "merge" (default) or "replace".
    #[serde(default)]
    pub mode: ImportMode,
}

/// Import result.
#[allow(dead_code)]
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ImportResult {
    pub success: bool,
    pub message: String,
    pub stats: ImportStats,
}

/// Statistics about what was imported.
#[allow(dead_code)]
#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct ImportStats {
    pub templates_created: u32,
    pub templates_updated: u32,
    pub templates_deleted: u32,
    pub streamers_created: u32,
    pub streamers_updated: u32,
    pub streamers_deleted: u32,
    pub engines_created: u32,
    pub engines_updated: u32,
    pub engines_deleted: u32,
    pub platforms_updated: u32,
    pub channels_created: u32,
    pub channels_updated: u32,
    pub channels_deleted: u32,
    pub job_presets_created: u32,
    pub job_presets_updated: u32,
    pub job_presets_deleted: u32,
    pub pipeline_presets_created: u32,
    pub pipeline_presets_updated: u32,
    pub pipeline_presets_deleted: u32,
    pub users_created: u32,
    pub users_updated: u32,
    pub users_deleted: u32,
}
