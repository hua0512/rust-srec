//! Configuration database models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Global configuration database model.
/// A singleton table for application-wide default settings.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct GlobalConfigDbModel {
    pub id: String,
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub min_segment_size_bytes: i64,
    pub max_download_duration_secs: i64,
    pub max_part_size_bytes: i64,
    pub record_danmu: bool,
    pub max_concurrent_downloads: i32,
    pub max_concurrent_uploads: i32,
    pub streamer_check_delay_ms: i64,
    /// JSON serialized ProxyConfig
    pub proxy_config: String,
    pub offline_check_delay_ms: i64,
    pub offline_check_count: i32,
    /// Name of the default engine configuration
    pub default_download_engine: String,
    pub max_concurrent_cpu_jobs: i32,
    pub max_concurrent_io_jobs: i32,
    pub job_history_retention_days: i32,
    pub session_gap_time_secs: i64,
}

impl Default for GlobalConfigDbModel {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            output_folder: "./downloads".to_string(),
            output_filename_template: "{streamer}-{title}-%Y%m%d-%H%M%S".to_string(),
            output_file_format: "flv".to_string(),
            min_segment_size_bytes: 1048576, // 1MB
            max_download_duration_secs: 0,   // No limit
            max_part_size_bytes: 8589934592, // 8GB
            record_danmu: false,
            max_concurrent_downloads: 6,
            max_concurrent_uploads: 3,
            streamer_check_delay_ms: 60000,
            proxy_config: r#"{"enabled":false,"url":null}"#.to_string(),
            offline_check_delay_ms: 20000,
            offline_check_count: 3,
            default_download_engine: "mesio".to_string(),
            max_concurrent_cpu_jobs: 0, // Auto
            max_concurrent_io_jobs: 8,
            job_history_retention_days: 30,
            session_gap_time_secs: 600,
        }
    }
}

/// Platform configuration database model.
/// Stores settings specific to each supported streaming platform.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct PlatformConfigDbModel {
    pub id: String,
    pub platform_name: String,
    pub fetch_delay_ms: Option<i64>,
    pub download_delay_ms: Option<i64>,
    pub cookies: Option<String>,
    /// JSON blob for platform-specific settings
    pub platform_specific_config: Option<String>,
    /// JSON serialized ProxyConfig
    pub proxy_config: Option<String>,
    pub record_danmu: Option<bool>,

    // Explicit overrides
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub download_engine: Option<String>,
    pub max_bitrate: Option<i32>,
    /// JSON serialized StreamSelectionConfig
    pub stream_selection_config: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    /// JSON serialized RetryPolicy
    pub download_retry_policy: Option<String>,
    /// JSON serialized EventHooks
    pub event_hooks: Option<String>,
}

/// Template configuration database model.
/// Reusable configuration templates for streamers.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TemplateConfigDbModel {
    pub id: String,
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub max_bitrate: Option<i32>,
    pub cookies: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub record_danmu: Option<bool>,
    /// JSON map of platform names to their specific configuration overrides
    pub platform_overrides: Option<String>,
    /// JSON serialized RetryPolicy
    pub download_retry_policy: Option<String>,
    /// JSON serialized DanmuSamplingConfig
    pub danmu_sampling_config: Option<String>,
    /// Name of the engine configuration to use
    pub download_engine: Option<String>,
    /// JSON map for template-specific engine configurations
    pub engines_override: Option<String>,
    /// JSON serialized ProxyConfig
    pub proxy_config: Option<String>,
    /// JSON serialized EventHooks
    pub event_hooks: Option<String>,
    /// JSON serialized StreamSelectionConfig
    pub stream_selection_config: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TemplateConfigDbModel {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            output_folder: None,
            output_filename_template: None,
            max_bitrate: None,
            cookies: None,
            output_file_format: None,
            min_segment_size_bytes: None,
            max_download_duration_secs: None,
            max_part_size_bytes: None,
            record_danmu: None,
            platform_overrides: None,
            download_retry_policy: None,
            danmu_sampling_config: None,
            download_engine: None,
            engines_override: None,
            proxy_config: None,
            event_hooks: None,
            stream_selection_config: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_config_default() {
        let config = GlobalConfigDbModel::default();
        assert_eq!(config.output_folder, "./downloads");
        assert_eq!(config.max_concurrent_downloads, 6);
        assert!(!config.record_danmu);
    }

    #[test]
    fn test_template_config_new() {
        let template = TemplateConfigDbModel::new("test-template");
        assert_eq!(template.name, "test-template");
        assert!(template.output_folder.is_none());
    }
}
