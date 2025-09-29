use super::types::{DanmuSamplingConfig, DownloadRetryPolicy, EventHooks, ProxyConfig};

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Default)]
pub struct MergedConfig {
    // Global settings
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub max_concurrent_downloads: u64,
    pub max_concurrent_uploads: u64,
    pub streamer_check_delay_ms: u64,
    pub offline_check_delay_ms: u64,
    pub offline_check_count: u64,
    pub default_download_engine: String,
    pub min_segment_size_bytes: u64,
    pub max_download_duration_secs: u64,
    pub max_part_size_bytes: u64,
    pub record_danmu: bool,

    // Platform-specific settings
    pub fetch_delay_ms: u64,
    pub download_delay_ms: u64,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<serde_json::Value>,

    // Streamer/Template settings
    pub max_bitrate: Option<u64>,
    pub event_hooks: Option<EventHooks>,
    pub download_retry_policy: Option<DownloadRetryPolicy>,
    pub download_engine: String,
    pub danmu_sampling_config: Option<DanmuSamplingConfig>,
    pub proxy_config: Option<ProxyConfig>,
}

