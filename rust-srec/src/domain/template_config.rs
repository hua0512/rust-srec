use super::types::{
    DanmuSamplingConfig, DownloadRetryPolicy, EnginesOverride, EventHooks, PlatformOverrides,
    ProxyConfig,
};
use crate::database::models;

#[derive(Debug, Clone, PartialEq)]
pub struct TemplateConfig {
    pub id: String,
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub max_bitrate: Option<u64>,
    pub cookies: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<u64>,
    pub max_download_duration_secs: Option<u64>,
    pub max_part_size_bytes: Option<u64>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<PlatformOverrides>,
    pub download_retry_policy: Option<DownloadRetryPolicy>,
    pub danmu_sampling_config: Option<DanmuSamplingConfig>,
    pub download_engine: Option<String>,
    pub engines_override: Option<EnginesOverride>,
    pub proxy_config: Option<ProxyConfig>,
    pub event_hooks: Option<EventHooks>,
}

impl From<models::TemplateConfig> for TemplateConfig {
    fn from(model: models::TemplateConfig) -> Self {
        Self {
            id: model.id,
            name: model.name,
            output_folder: model.output_folder,
            output_filename_template: model.output_filename_template,
            max_bitrate: model.max_bitrate.map(|v| v as u64),
            cookies: model.cookies,
            output_file_format: model.output_file_format,
            min_segment_size_bytes: model.min_segment_size_bytes.map(|v| v as u64),
            max_download_duration_secs: model.max_download_duration_secs.map(|v| v as u64),
            max_part_size_bytes: model.max_part_size_bytes.map(|v| v as u64),
            record_danmu: model.record_danmu,
            platform_overrides: model
                .platform_overrides
                .and_then(|v| serde_json::from_str(&v).ok()),
            download_retry_policy: model
                .download_retry_policy
                .and_then(|v| serde_json::from_str(&v).ok()),
            danmu_sampling_config: model
                .danmu_sampling_config
                .and_then(|v| serde_json::from_str(&v).ok()),
            download_engine: model.download_engine,
            engines_override: model
                .engines_override
                .and_then(|v| serde_json::from_str(&v).ok()),
            proxy_config: model
                .proxy_config
                .and_then(|v| serde_json::from_str(&v).ok()),
            event_hooks: model
                .event_hooks
                .and_then(|v| serde_json::from_str(&v).ok()),
        }
    }
}
