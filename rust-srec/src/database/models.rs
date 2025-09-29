use serde_json::Value;
use sqlx::FromRow;

// --- Structs for Database Table Mapping ---
// These structs derive `FromRow` for direct mapping from SQLx queries.
// They use database-compatible types (e.g., String for enums/paths, JSON for complex types).

// --- Configuration Hierarchy ---

/// Represents the `global_config` table. A singleton table that stores the default settings for the entire application.
#[derive(Debug, Clone, FromRow)]
pub struct GlobalConfig {
    pub id: String,
    pub output_folder: String,
    pub output_filename_template: Option<String>,
    pub output_file_format: Option<String>,
    pub max_concurrent_downloads: i64,
    pub max_concurrent_uploads: i64,
    pub streamer_check_delay_ms: i64,
    pub offline_check_delay_ms: i64,
    pub offline_check_count: i64,
    pub default_download_engine: String,
    pub proxy_config: String,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub record_danmu: Option<bool>,
}

/// Represents the `platform_config` table. Stores settings that apply to all streamers on a specific platform.
#[derive(Debug, Clone, FromRow)]
pub struct PlatformConfig {
    pub id: String,
    pub platform_name: String,
    pub fetch_delay_ms: i64,
    pub download_delay_ms: i64,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<String>,
    pub proxy_config: Option<String>,
    pub record_danmu: Option<bool>,
}

/// Represents the `template_config` table. A reusable collection of settings that can be applied to multiple streamers.
#[derive(Debug, Clone, FromRow)]
pub struct TemplateConfig {
    pub id: String,
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub max_bitrate: Option<i64>,
    pub cookies: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<String>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub download_engine: Option<String>,
    pub engines_override: Option<String>,
    pub proxy_config: Option<String>,
    pub event_hooks: Option<String>,
}

// --- Core Entities ---

/// Represents the `streamers` table. This is the central entity in the system.
#[derive(Debug, Clone, FromRow)]
pub struct Streamer {
    pub id: String,
    pub name: String,
    pub url: String,
    pub platform_config_id: String,
    pub template_config_id: Option<String>,
    pub state: String, // Represents the StreamerState enum
    pub last_live_time: Option<String>,
    pub streamer_specific_config: Option<String>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub consecutive_error_count: Option<i64>,
    pub disabled_until: Option<String>,
}

/// Represents the `filters` table. A set of conditions to determine if a live stream should be recorded.
#[derive(Debug, Clone, FromRow)]
pub struct Filter {
    pub id: String,
    pub streamer_id: String,
    pub filter_type: String, // Represents the FilterType enum
    pub config: String,
}

/// Represents the `live_sessions` table. Represents a single, continuous live stream event.
#[derive(Debug, Clone, FromRow)]
pub struct LiveSession {
    pub id: String,
    pub streamer_id: String,
    pub start_time: String,
    pub end_time: Option<String>,
    pub titles: Option<String>,
}

/// Represents the `media_outputs` table. Represents a single file generated during a live session.
#[derive(Debug, Clone, FromRow)]
pub struct MediaOutput {
    pub id: String,
    pub session_id: String,
    pub parent_media_output_id: Option<String>,
    pub file_path: String,
    pub file_type: String, // Represents the MediaType enum
    pub size_bytes: i64,   // Using i64 for SQLite's INTEGER type
    pub created_at: String,
}

/// Represents the `danmu_statistics` table. Aggregated statistics for danmu messages.
#[derive(Debug, Clone, FromRow)]
pub struct DanmuStatistics {
    pub id: String,
    pub session_id: String,
    pub total_danmus: i64, // Using i64 for SQLite's INTEGER type
    pub danmu_rate_timeseries: Option<String>,
    pub top_talkers: Option<String>,
    pub word_frequency: Option<String>,
}

// --- System and Job Management ---

/// Represents the `jobs` table. Represents a single asynchronous task.
#[derive(Debug, Clone, FromRow)]
pub struct Job {
    pub id: String,
    pub job_type: String, // Represents the JobType enum
    pub status: String,   // Represents the JobStatus enum
    pub context: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Represents the `engine_configurations` table. A reusable configuration for a download engine.
#[derive(Debug, Clone, FromRow)]
pub struct EngineConfiguration {
    pub id: String,
    pub name: String,
    pub engine_type: String, // Represents the EngineType enum
    pub config: Value,
}

/// Represents the `upload_records` table. A record of a file being uploaded to an external platform.
#[derive(Debug, Clone, FromRow)]
pub struct UploadRecord {
    pub id: String,
    pub media_output_id: String,
    pub platform: String,
    pub remote_path: String,
    pub status: String, // Represents the UploadStatus enum
    pub metadata: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

// --- Security and Notifications ---

/// Represents the `api_keys` table. Stores API keys for authenticating requests.
#[derive(Debug, Clone, FromRow)]
pub struct ApiKey {
    pub id: String,
    pub key_hash: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
}

/// Represents the `notification_channels` table. A configured destination for system event notifications.
#[derive(Debug, Clone, FromRow)]
pub struct NotificationChannel {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub settings: String,
}

/// Represents the `notification_subscriptions` table. Links a channel to events.
#[derive(Debug, Clone, FromRow)]
pub struct NotificationSubscription {
    pub channel_id: String,
    pub event_name: String,
}
