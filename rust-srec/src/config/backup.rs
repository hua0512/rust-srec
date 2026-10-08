//! Versioned configuration backup contract shared by transport and application layers.

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::database::repositories::credential_selections::SELECTION_KEY;

mod filter_timezones;
pub(crate) use filter_timezones::{
    EXPORT_SCHEMA_VERSION, export_filter_config, import_filter_config,
};

/// Schema of bundles that carry managed credential profiles or selections.
pub(crate) const MANAGED_CREDENTIAL_SCHEMA_VERSION: &str = "1.0.0";

pub(crate) fn schema_version_at_least(version: &str, min: (u32, u32, u32)) -> bool {
    fn parse_segment(segment: &str) -> Option<u32> {
        let digits: String = segment.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        digits.parse().ok()
    }

    let mut parts = version.split('.');
    let Some(major) = parts.next().and_then(parse_segment) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(parse_segment) else {
        return false;
    };
    let Some(patch) = parts.next().and_then(parse_segment) else {
        return false;
    };

    (major, minor, patch) >= min
}

fn parse_timestamp_ms_str(value: &str) -> Result<i64, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("timestamp string is empty".to_string());
    }

    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc).timestamp_millis());
    }

    // Accept common legacy/SQLite-ish formats.
    // Note: if the timestamp has no timezone, we assume UTC.
    if let Ok(dt) = DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%:z") {
        return Ok(dt.with_timezone(&Utc).timestamp_millis());
    }
    if let Ok(dt) = DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%:z") {
        return Ok(dt.with_timezone(&Utc).timestamp_millis());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).timestamp_millis());
    }
    if let Ok(naive) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc).timestamp_millis());
    }

    if let Ok(ms) = value.parse::<i64>() {
        return Ok(ms);
    }

    Err(format!(
        "invalid timestamp value '{value}' (expected RFC3339 or epoch ms)"
    ))
}

/// Timestamp wire shape accepted by `deserialize_timestamp_ms` and
/// `deserialize_opt_timestamp_ms`: epoch milliseconds, or a string parsed by
/// `parse_timestamp_ms_str`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawTimestamp {
    Ms(i64),
    Str(String),
}

impl RawTimestamp {
    fn into_ms(self) -> Result<i64, String> {
        match self {
            Self::Ms(ms) => Ok(ms),
            Self::Str(s) => parse_timestamp_ms_str(&s),
        }
    }
}

fn deserialize_timestamp_ms<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    RawTimestamp::deserialize(deserializer)?
        .into_ms()
        .map_err(D::Error::custom)
}

fn deserialize_opt_timestamp_ms<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match Option::<RawTimestamp>::deserialize(deserializer)? {
        None => Ok(None),
        Some(raw) => raw.into_ms().map(Some).map_err(D::Error::custom),
    }
}

fn default_streamer_state() -> String {
    "NOT_LIVE".to_string()
}

fn default_notification_event_log_retention_days() -> i32 {
    30
}

/// Unwraps double-encoded JSON values.
pub(crate) fn unwrap_json_value(mut v: serde_json::Value) -> serde_json::Value {
    while let Some(s) = v.as_str() {
        if let Ok(inner) = serde_json::from_str::<serde_json::Value>(s) {
            v = inner;
        } else {
            break;
        }
    }
    v
}

/// Converts a JSON value to a database string representation.
/// Normalizes null and empty strings to an empty string so that
/// `parse_or_default` in resolver.rs handles them correctly.
pub(crate) fn json_value_to_db_string(v: serde_json::Value) -> String {
    let v = unwrap_json_value(v);
    match &v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) if s.is_empty() => String::new(),
        _ => v.to_string(),
    }
}

// ============================================================================
// Export Data Models
// ============================================================================

/// Complete configuration export bundle.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProxyExport {
    pub name: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl std::fmt::Debug for ProxyExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProxyExport")
            .field("name", &self.name)
            .field("login", &"[redacted]")
            .finish()
    }
}

impl ProxyExport {
    pub fn endpoint(&self) -> crate::proxies::ProxyEndpoint {
        crate::proxies::ProxyEndpoint {
            url: self.url.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
        }
    }
}

/// A route in a backup, naming its proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BackupRoute {
    Inherit,
    Direct,
    System,
    Proxy { name: String },
}

impl BackupRoute {
    /// The route a stored route writes, naming proxies through `names`
    /// (entry ID to name).
    pub fn from_route(
        route: &crate::proxies::ProxyRoute,
        names: &std::collections::HashMap<String, String>,
    ) -> crate::Result<Self> {
        use crate::proxies::ProxyRoute;
        Ok(match route {
            ProxyRoute::Inherit => Self::Inherit,
            ProxyRoute::Direct => Self::Direct,
            ProxyRoute::System => Self::System,
            ProxyRoute::Proxy { id } => Self::Proxy {
                name: names
                    .get(id)
                    .cloned()
                    .ok_or_else(|| crate::proxies::ProxyError::Missing(id.clone()))?,
            },
        })
    }

    /// The stored route, finding proxies through `ids`: entry IDs by
    /// [`crate::proxies::name_key`]. The error is a name `ids` lacks.
    pub fn to_route(
        &self,
        ids: &std::collections::HashMap<String, String>,
    ) -> std::result::Result<crate::proxies::ProxyRoute, &str> {
        use crate::proxies::ProxyRoute;
        Ok(match self {
            Self::Inherit => ProxyRoute::Inherit,
            Self::Direct => ProxyRoute::Direct,
            Self::System => ProxyRoute::System,
            Self::Proxy { name } => ProxyRoute::Proxy {
                id: ids
                    .get(&crate::proxies::name_key(name))
                    .ok_or(name.as_str())?
                    .clone(),
            },
        })
    }

    /// The `proxy_config` a backup without saved proxies carries instead:
    /// releases before saved proxies read only that.
    pub fn legacy_config(&self) -> Option<serde_json::Value> {
        match self {
            Self::Inherit | Self::Proxy { .. } => None,
            Self::Direct => Some(serde_json::json!({"enabled": false})),
            Self::System => Some(serde_json::json!({"enabled": true, "use_system_proxy": true})),
        }
    }
}

/// Explicit sensitive backup representation. Ordinary profile DTOs never serialize material.
#[derive(Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

impl std::fmt::Debug for CredentialProfileExport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialProfileExport")
            .field("id", &self.id)
            .field("material", &"[redacted]")
            .finish()
    }
}

impl ConfigExport {
    /// Filtering callers must keep profile dependencies or reject the incomplete
    /// graph. This also prevents a concurrent owner deletion producing a lossy backup.
    pub fn validate_credential_graph(&self) -> crate::Result<()> {
        use std::collections::{HashMap, HashSet};
        let platforms: HashSet<&str> = self
            .platforms
            .iter()
            .map(|p| p.platform_name.as_str())
            .collect();
        let profiles: HashMap<_, _> = self
            .credential_profiles
            .iter()
            .map(|p| (p.id.as_str(), p))
            .collect();
        if profiles.len() != self.credential_profiles.len() {
            return Err(crate::Error::validation(
                "duplicate credential profile in export",
            ));
        }
        if profiles
            .values()
            .any(|profile| !platforms.contains(profile.platform.as_str()))
        {
            return Err(crate::Error::validation(
                "export omits a credential profile platform",
            ));
        }
        let check = |selection: &crate::credentials::CredentialSelection,
                     platform: &str|
         -> crate::Result<()> {
            selection.validate()?;
            for id in selection.profile_ids() {
                let profile = profiles.get(id).ok_or_else(|| {
                    crate::Error::validation("export omits a referenced credential profile")
                })?;
                if profile.platform != platform {
                    return Err(crate::Error::validation(
                        "export contains a credential profile reference from another platform",
                    ));
                }
            }
            Ok(())
        };
        for platform in &self.platforms {
            if let Some(selection) = &platform.credential_selection {
                check(selection, &platform.platform_name)?;
            }
        }
        for template in &self.templates {
            if let Some(entries) = template
                .platform_overrides
                .as_ref()
                .and_then(serde_json::Value::as_object)
            {
                for (platform, value) in entries {
                    if let Some(selection) = value.get(SELECTION_KEY) {
                        if !platforms.contains(platform.as_str()) {
                            return Err(crate::Error::validation(
                                "export omits a template policy platform",
                            ));
                        }
                        check(
                            &crate::credentials::CredentialSelection::from_value(
                                selection.clone(),
                            )?,
                            platform,
                        )?;
                    }
                }
            }
        }
        for streamer in &self.streamers {
            if let Some(selection) = streamer
                .streamer_specific_config
                .as_ref()
                .and_then(|v| v.get(SELECTION_KEY))
            {
                if !platforms.contains(streamer.platform.as_str()) {
                    return Err(crate::Error::validation(
                        "export omits a streamer policy dependency",
                    ));
                }
                check(
                    &crate::credentials::CredentialSelection::from_value(selection.clone())?,
                    &streamer.platform,
                )?;
            }
        }
        Ok(())
    }

    /// Compute from the selected graph, including explicit inherit/none policies.
    pub fn has_managed_credentials(&self) -> bool {
        !self.credential_profiles.is_empty()
            || self
                .platforms
                .iter()
                .any(|p| p.credential_selection.is_some())
            || self.templates.iter().any(|t| {
                t.platform_overrides
                    .as_ref()
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|entries| entries.values().any(has_selection))
            })
            || self.streamers.iter().any(|s| {
                s.streamer_specific_config
                    .as_ref()
                    .is_some_and(has_selection)
            })
    }

    /// Saved proxies need the managed format: older releases cannot read
    /// routes that name them.
    pub fn needs_managed_format(&self) -> bool {
        self.has_managed_credentials() || !self.proxies.is_empty()
    }

    /// Picks the schema version and the proxy form it carries. Bundles
    /// without managed credentials or saved proxies stay readable by
    /// releases before saved proxies, which read `proxy_config` only.
    pub fn update_schema_version(&mut self) {
        if !self.needs_managed_format() {
            self.write_legacy_proxy_settings();
        }
        self.version = if self.needs_managed_format() {
            MANAGED_CREDENTIAL_SCHEMA_VERSION
        } else {
            EXPORT_SCHEMA_VERSION
        }
        .to_owned();
    }
}

impl ConfigExport {
    fn write_legacy_proxy_settings(&mut self) {
        let legacy =
            |route: &mut Option<BackupRoute>| route.take().and_then(|route| route.legacy_config());
        if let Some(config) = legacy(&mut self.global_config.proxy_route) {
            self.global_config.proxy_config = config;
        }
        for platform in &mut self.platforms {
            platform.proxy_config = legacy(&mut platform.proxy_route);
        }
        for template in &mut self.templates {
            template.proxy_config = legacy(&mut template.proxy_route);
        }
        for streamer in &mut self.streamers {
            if let Some(config) = legacy(&mut streamer.proxy_route) {
                let document = streamer
                    .streamer_specific_config
                    .get_or_insert_with(|| serde_json::Value::Object(Default::default()));
                if let Some(fields) = document.as_object_mut() {
                    fields.insert("proxy_config".to_owned(), config);
                }
            }
        }
    }
}

fn has_selection(value: &serde_json::Value) -> bool {
    value
        .get(SELECTION_KEY)
        .is_some_and(|policy| !policy.is_null())
}

/// Global configuration for export (excludes internal ID).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    pub max_concurrent_cpu_jobs: i32,
    pub max_concurrent_io_jobs: i32,
    pub job_history_retention_days: i32,
    #[serde(default = "default_notification_event_log_retention_days")]
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
    #[serde(default = "default_pipeline_job_timeout_secs")]
    pub pipeline_cpu_job_timeout_secs: i64,
    /// Maximum execution time (seconds) for a single IO-bound pipeline job.
    #[serde(default = "default_pipeline_job_timeout_secs")]
    pub pipeline_io_job_timeout_secs: i64,
    /// Maximum execution time (seconds) for the `execute` processor command.
    #[serde(default = "default_pipeline_job_timeout_secs")]
    pub pipeline_execute_timeout_secs: i64,
    /// Milliseconds a queued download may wait before refetching live state.
    #[serde(default = "default_queue_freshness_threshold_ms")]
    pub queue_freshness_threshold_ms: i64,
    /// Seconds between `nvidia-smi` probes by the GPU health monitor.
    #[serde(default = "default_gpu_health_probe_interval_secs")]
    pub gpu_health_probe_interval_secs: i64,
    /// Whether the stream proxy may fetch targets on private networks.
    /// Defaults to false so backups from older versions import fail-closed.
    #[serde(default)]
    pub stream_proxy_allow_private_targets: bool,
}

fn default_pipeline_job_timeout_secs() -> i64 {
    3600
}

fn default_queue_freshness_threshold_ms() -> i64 {
    60_000
}

fn default_gpu_health_probe_interval_secs() -> i64 {
    30
}

/// Template for export (uses name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StreamerExport {
    pub name: String,
    pub url: String,
    /// Platform name (resolved from platform_config_id).
    pub platform: String,
    /// Template name (resolved from template_config_id).
    pub template: Option<String>,
    pub priority: String,
    /// Streamer operational state (e.g. NOT_LIVE, LIVE).
    #[serde(default = "default_streamer_state")]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FilterExport {
    pub filter_type: String,
    pub config: serde_json::Value,
}

/// Engine configuration for export (uses name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EngineExport {
    pub name: String,
    pub engine_type: String,
    pub config: serde_json::Value,
}

/// Platform configuration for export (uses platform_name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PlatformExport {
    pub platform_name: String,
    pub fetch_delay_ms: Option<i64>,
    pub download_delay_ms: Option<i64>,
    /// Read only from backups written before credential profiles; import
    /// converts it into a profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_selection: Option<crate::credentials::CredentialSelection>,
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NotificationChannelExport {
    pub name: String,
    pub channel_type: String,
    pub settings: serde_json::Value,
    pub subscriptions: Vec<String>,
}

/// Job preset configuration for export.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct JobPresetExport {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub processor: String,
    pub config: serde_json::Value,
}

/// Pipeline preset configuration for export.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PipelinePresetExport {
    pub name: String,
    pub description: Option<String>,
    pub dag_definition: Option<serde_json::Value>,
    pub pipeline_type: Option<String>,
}

/// User account for export (uses username as identifier).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct UserExport {
    pub id: String,
    pub username: String,
    /// Argon2id password hash (sensitive but required for full restore).
    pub password_hash: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub is_active: bool,
    pub must_change_password: bool,
    #[serde(default, deserialize_with = "deserialize_opt_timestamp_ms")]
    pub last_login_at: Option<i64>,
    #[serde(deserialize_with = "deserialize_timestamp_ms")]
    pub created_at: i64,
    #[serde(deserialize_with = "deserialize_timestamp_ms")]
    pub updated_at: i64,
}

// ============================================================================
// Import Data Models
// ============================================================================

/// Import mode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Merge: Update existing entities, add new ones, keep entities not in import.
    #[default]
    Merge,
    /// Replace: Delete all existing entities and import fresh.
    Replace,
}

/// Import request with optional mode.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct ImportRequest {
    /// The configuration data to import.
    pub config: ConfigExport,
    /// Import mode: "merge" (default) or "replace".
    #[serde(default)]
    pub mode: ImportMode,
}

/// Import result.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ImportResult {
    pub success: bool,
    pub message: String,
    pub stats: ImportStats,
}

/// Statistics about what was imported.
#[derive(Debug, Clone, serde::Serialize, Default, utoipa::ToSchema)]
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
