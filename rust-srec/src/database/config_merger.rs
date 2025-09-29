use crate::domain::{
    config::MergedConfig, global_config::GlobalConfig, platform_config::PlatformConfig,
    template_config::TemplateConfig,
};
use serde_json::Value;

pub fn merge_configs(
    global_config: &GlobalConfig,
    platform_config: &PlatformConfig,
    template_config: Option<&TemplateConfig>,
    streamer_specific_config: Option<&Value>,
) -> MergedConfig {
    let mut merged = MergedConfig {
        output_folder: global_config.output_folder.clone(),
        output_filename_template: global_config.output_filename_template.clone(),
        output_file_format: global_config.output_file_format.clone(),
        max_concurrent_downloads: global_config.max_concurrent_downloads,
        max_concurrent_uploads: global_config.max_concurrent_uploads,
        streamer_check_delay_ms: global_config.streamer_check_delay_ms,
        offline_check_delay_ms: global_config.offline_check_delay_ms,
        offline_check_count: global_config.offline_check_count,
        default_download_engine: global_config.default_download_engine.clone(),
        min_segment_size_bytes: global_config.min_segment_size_bytes,
        max_download_duration_secs: global_config.max_download_duration_secs,
        max_part_size_bytes: global_config.max_part_size_bytes,
        record_danmu: global_config.record_danmu,
        fetch_delay_ms: platform_config.fetch_delay_ms,
        download_delay_ms: platform_config.download_delay_ms,
        cookies: platform_config.cookies.clone(),
        platform_specific_config: platform_config.platform_specific_config.clone(),
        proxy_config: platform_config.proxy_config.clone(),
        max_bitrate: None,
        event_hooks: None,
        download_retry_policy: None,
        download_engine: global_config.default_download_engine.clone(),
        danmu_sampling_config: None,
    };

    if let Some(template) = template_config {
        if let Some(folder) = &template.output_folder {
            merged.output_folder = folder.clone();
        }
        if let Some(filename) = &template.output_filename_template {
            merged.output_filename_template = filename.clone();
        }
        if let Some(format) = &template.output_file_format {
            merged.output_file_format = format.clone();
        }
        if let Some(engine) = &template.download_engine {
            merged.download_engine = engine.clone();
        }
        if let Some(bitrate) = template.max_bitrate {
            merged.max_bitrate = Some(bitrate);
        }
        if let Some(cookies) = &template.cookies {
            merged.cookies = Some(cookies.clone());
        }
        if let Some(policy) = &template.download_retry_policy {
            merged.download_retry_policy = Some(policy.clone());
        }
        if let Some(hooks) = &template.event_hooks {
            merged.event_hooks = Some(hooks.clone());
        }
        if let Some(danmu) = &template.danmu_sampling_config {
            merged.danmu_sampling_config = Some(danmu.clone());
        }
        if let Some(proxy) = &template.proxy_config {
            merged.proxy_config = Some(proxy.clone());
        }
        if let Some(record_danmu) = template.record_danmu {
            merged.record_danmu = record_danmu;
        }

        if let Some(overrides) = &template.platform_overrides {
            if let Some(platform_override) = overrides.get(&platform_config.platform_name) {
                if let Some(delay) = platform_override.fetch_delay_ms {
                    merged.fetch_delay_ms = delay;
                }
                if let Some(delay) = platform_override.download_delay_ms {
                    merged.download_delay_ms = delay;
                }
                if let Some(cookies) = &platform_override.cookies {
                    merged.cookies = Some(cookies.clone());
                }
                if let Some(proxy) = &platform_override.proxy_config {
                    merged.proxy_config = Some(proxy.clone());
                }
                if let Some(policy) = &platform_override.download_retry_policy {
                    merged.download_retry_policy = Some(policy.clone());
                }
                if let Some(engine) = &platform_override.download_engine {
                    merged.download_engine = engine.clone();
                }
            }
        }
    }

    if let Some(streamer_config) = streamer_specific_config {
        if let Some(folder) = streamer_config
            .get("output_folder")
            .and_then(|v| v.as_str())
        {
            merged.output_folder = folder.to_string();
        }
        if let Some(filename) = streamer_config
            .get("output_filename_template")
            .and_then(|v| v.as_str())
        {
            merged.output_filename_template = filename.to_string();
        }
        if let Some(format) = streamer_config
            .get("output_file_format")
            .and_then(|v| v.as_str())
        {
            merged.output_file_format = format.to_string();
        }
        if let Some(engine) = streamer_config
            .get("download_engine")
            .and_then(|v| v.as_str())
        {
            merged.download_engine = engine.to_string();
        }
        if let Some(bitrate) = streamer_config.get("max_bitrate").and_then(|v| v.as_u64()) {
            merged.max_bitrate = Some(bitrate);
        }
        if let Some(size) = streamer_config
            .get("min_segment_size_bytes")
            .and_then(|v| v.as_u64())
        {
            merged.min_segment_size_bytes = size;
        }
        if let Some(duration) = streamer_config
            .get("max_download_duration_secs")
            .and_then(|v| v.as_u64())
        {
            merged.max_download_duration_secs = duration;
        }
        if let Some(size) = streamer_config
            .get("max_part_size_bytes")
            .and_then(|v| v.as_u64())
        {
            merged.max_part_size_bytes = size;
        }
        if let Some(record) = streamer_config
            .get("record_danmu")
            .and_then(|v| v.as_bool())
        {
            merged.record_danmu = record;
        }
        if let Some(cookies) = streamer_config.get("cookies").and_then(|v| v.as_str()) {
            merged.cookies = Some(cookies.to_string());
        }
        if let Some(policy) = streamer_config
            .get("download_retry_policy")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
        {
            merged.download_retry_policy = Some(policy);
        }
        if let Some(hooks) = streamer_config
            .get("event_hooks")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
        {
            merged.event_hooks = Some(hooks);
        }
        if let Some(danmu) = streamer_config
            .get("danmu_sampling_config")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
        {
            merged.danmu_sampling_config = Some(danmu);
        }
        if let Some(proxy) = streamer_config
            .get("proxy_config")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
        {
            merged.proxy_config = Some(proxy);
        }
    }

    merged
}
