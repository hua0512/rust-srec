use tracing::error;

use super::types::ProxyConfig;
use crate::database::models::GlobalConfig as DbGlobalConfig;


#[derive(Debug, Clone)]
pub struct GlobalConfig {
    pub id: String,
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub max_concurrent_downloads: u64,
    pub max_concurrent_uploads: u64,
    pub streamer_check_delay_ms: u64,
    pub offline_check_delay_ms: u64,
    pub offline_check_count: u64,
    pub default_download_engine: String,
    pub proxy_config: ProxyConfig,
    pub min_segment_size_bytes: u64,
    pub max_download_duration_secs: u64,
    pub max_part_size_bytes: u64,
    pub record_danmu: bool,
}

impl From<DbGlobalConfig> for GlobalConfig {
    fn from(model: DbGlobalConfig) -> Self {
        Self {
            id: model.id,
            output_folder: model.output_folder,
            output_filename_template: model
                .output_filename_template
                .unwrap_or("{streamer}-{title}-{%Y%m%d-%H%M%S}".to_string()),
            output_file_format: model.output_file_format.unwrap_or("flv".to_string()),
            max_concurrent_downloads: model.max_concurrent_downloads as u64,
            max_concurrent_uploads: model.max_concurrent_uploads as u64,
            streamer_check_delay_ms: model.streamer_check_delay_ms as u64,
            offline_check_delay_ms: model.offline_check_delay_ms as u64,
            offline_check_count: model.offline_check_count as u64,
            default_download_engine: model.default_download_engine,
            proxy_config: serde_json::from_str(&model.proxy_config).unwrap_or_else(|e| {
                error!(
                    "Failed to parse proxy_config from DB: {}. Using default.",
                    e
                );
                ProxyConfig::default()
            }),
            min_segment_size_bytes: model.min_segment_size_bytes.unwrap_or(20 * 1024 * 1024) as u64,
            max_download_duration_secs: model.max_download_duration_secs.unwrap_or(0) as u64,
            max_part_size_bytes: model
                .max_part_size_bytes
                .unwrap_or(8 * 1024 * 1024 * 1024) as u64,
            record_danmu: model.record_danmu.unwrap_or(false),
        }
    }
}

impl From<&GlobalConfig> for DbGlobalConfig {
    fn from(domain: &GlobalConfig) -> Self {
        Self {
            id: domain.id.clone(),
            output_folder: domain.output_folder.clone(),
            output_filename_template: Some(domain.output_filename_template.clone()),
            output_file_format: Some(domain.output_file_format.clone()),
            max_concurrent_downloads: domain.max_concurrent_downloads as i64,
            max_concurrent_uploads: domain.max_concurrent_uploads as i64,
            streamer_check_delay_ms: domain.streamer_check_delay_ms as i64,
            offline_check_delay_ms: domain.offline_check_delay_ms as i64,
            offline_check_count: domain.offline_check_count as i64,
            default_download_engine: domain.default_download_engine.clone(),
            proxy_config: serde_json::to_string(&domain.proxy_config).unwrap_or_else(|e| {
                error!("Failed to serialize proxy_config: {}. Using default.", e);
                "{}".to_string()
            }),
            min_segment_size_bytes: Some(domain.min_segment_size_bytes as i64),
            max_download_duration_secs: Some(domain.max_download_duration_secs as i64),
            max_part_size_bytes: Some(domain.max_part_size_bytes as i64),
            record_danmu: Some(domain.record_danmu),
        }
    }
}
