//! Merged configuration.

use crate::domain::{DanmuSamplingConfig, EventHooks, ProxyConfig, RetryPolicy};
use crate::downloader::StreamSelectionConfig;
use serde::{Deserialize, Serialize};

/// Fully resolved configuration for a streamer.
///
/// This represents the result of merging the 4-layer configuration hierarchy:
/// Global → Platform → Template → Streamer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergedConfig {
    // Output settings
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,

    // Size and duration limits
    pub min_segment_size_bytes: i64,
    pub max_download_duration_secs: i64,
    pub max_part_size_bytes: i64,

    // Danmu settings
    pub record_danmu: bool,
    pub danmu_sampling_config: DanmuSamplingConfig,

    // Network settings
    pub proxy_config: ProxyConfig,
    pub cookies: Option<String>,

    // Engine settings
    pub download_engine: String,
    pub download_retry_policy: RetryPolicy,

    // Quality settings
    pub max_bitrate: Option<i32>,

    // Event hooks
    pub event_hooks: EventHooks,

    // Platform-specific
    pub fetch_delay_ms: i64,
    pub download_delay_ms: i64,

    // Stream selection settings
    pub stream_selection: StreamSelectionConfig,
}

impl MergedConfig {
    /// Create a builder for MergedConfig.
    pub fn builder() -> MergedConfigBuilder {
        MergedConfigBuilder::default()
    }
}

/// Builder for MergedConfig.
#[derive(Debug, Default)]
pub struct MergedConfigBuilder {
    output_folder: Option<String>,
    output_filename_template: Option<String>,
    output_file_format: Option<String>,
    min_segment_size_bytes: Option<i64>,
    max_download_duration_secs: Option<i64>,
    max_part_size_bytes: Option<i64>,
    record_danmu: Option<bool>,
    danmu_sampling_config: Option<DanmuSamplingConfig>,
    proxy_config: Option<ProxyConfig>,
    cookies: Option<String>,
    download_engine: Option<String>,
    download_retry_policy: Option<RetryPolicy>,
    max_bitrate: Option<i32>,
    event_hooks: Option<EventHooks>,
    fetch_delay_ms: Option<i64>,
    download_delay_ms: Option<i64>,
    stream_selection: Option<StreamSelectionConfig>,
}

impl MergedConfigBuilder {
    /// Apply global config as the base layer.
    #[allow(clippy::too_many_arguments)]
    pub fn with_global(
        mut self,
        output_folder: String,
        output_filename_template: String,
        output_file_format: String,
        min_segment_size_bytes: i64,
        max_download_duration_secs: i64,
        max_part_size_bytes: i64,
        record_danmu: bool,
        proxy_config: ProxyConfig,
        download_engine: String,
    ) -> Self {
        self.output_folder = Some(output_folder);
        self.output_filename_template = Some(output_filename_template);
        self.output_file_format = Some(output_file_format);
        self.min_segment_size_bytes = Some(min_segment_size_bytes);
        self.max_download_duration_secs = Some(max_download_duration_secs);
        self.max_part_size_bytes = Some(max_part_size_bytes);
        self.record_danmu = Some(record_danmu);
        self.proxy_config = Some(proxy_config);
        self.download_engine = Some(download_engine);
        self.danmu_sampling_config = Some(DanmuSamplingConfig::default());
        self.download_retry_policy = Some(RetryPolicy::default());
        self.event_hooks = Some(EventHooks::default());
        self
    }

    /// Apply platform config layer.
    pub fn with_platform(
        mut self,
        fetch_delay_ms: i64,
        download_delay_ms: i64,
        cookies: Option<String>,
        proxy_config: Option<ProxyConfig>,
        record_danmu: Option<bool>,
    ) -> Self {
        self.fetch_delay_ms = Some(fetch_delay_ms);
        self.download_delay_ms = Some(download_delay_ms);

        if cookies.is_some() {
            self.cookies = cookies;
        }
        if let Some(proxy) = proxy_config {
            self.proxy_config = Some(proxy);
        }
        if let Some(danmu) = record_danmu {
            self.record_danmu = Some(danmu);
        }
        self
    }

    /// Apply template config layer.
    #[allow(clippy::too_many_arguments)]
    pub fn with_template(
        mut self,
        output_folder: Option<String>,
        output_filename_template: Option<String>,
        output_file_format: Option<String>,
        min_segment_size_bytes: Option<i64>,
        max_download_duration_secs: Option<i64>,
        max_part_size_bytes: Option<i64>,
        record_danmu: Option<bool>,
        proxy_config: Option<ProxyConfig>,
        cookies: Option<String>,
        download_engine: Option<String>,
        download_retry_policy: Option<RetryPolicy>,
        danmu_sampling_config: Option<DanmuSamplingConfig>,
        max_bitrate: Option<i32>,
        event_hooks: Option<EventHooks>,
        stream_selection: Option<StreamSelectionConfig>,
    ) -> Self {
        if let Some(v) = output_folder {
            self.output_folder = Some(v);
        }
        if let Some(v) = output_filename_template {
            self.output_filename_template = Some(v);
        }
        if let Some(v) = output_file_format {
            self.output_file_format = Some(v);
        }
        if let Some(v) = min_segment_size_bytes {
            self.min_segment_size_bytes = Some(v);
        }
        if let Some(v) = max_download_duration_secs {
            self.max_download_duration_secs = Some(v);
        }
        if let Some(v) = max_part_size_bytes {
            self.max_part_size_bytes = Some(v);
        }
        if let Some(v) = record_danmu {
            self.record_danmu = Some(v);
        }
        if let Some(v) = proxy_config {
            self.proxy_config = Some(v);
        }
        if cookies.is_some() {
            self.cookies = cookies;
        }
        if let Some(v) = download_engine {
            self.download_engine = Some(v);
        }
        if let Some(v) = download_retry_policy {
            self.download_retry_policy = Some(v);
        }
        if let Some(v) = danmu_sampling_config {
            self.danmu_sampling_config = Some(v);
        }
        if max_bitrate.is_some() {
            self.max_bitrate = max_bitrate;
        }
        if let Some(v) = event_hooks {
            // Merge event hooks
            if let Some(existing) = &self.event_hooks {
                self.event_hooks = Some(existing.merge(&v));
            } else {
                self.event_hooks = Some(v);
            }
        }
        if let Some(v) = stream_selection {
            // Merge stream selection config
            if let Some(existing) = &self.stream_selection {
                self.stream_selection = Some(existing.merge(&v));
            } else {
                self.stream_selection = Some(v);
            }
        }
        self
    }

    /// Apply streamer-specific config layer.
    pub fn with_streamer(
        mut self,
        download_retry_policy: Option<RetryPolicy>,
        danmu_sampling_config: Option<DanmuSamplingConfig>,
        streamer_config: Option<&serde_json::Value>,
    ) -> Self {
        if let Some(v) = download_retry_policy {
            self.download_retry_policy = Some(v);
        }
        if let Some(v) = danmu_sampling_config {
            self.danmu_sampling_config = Some(v);
        }

        // Parse streamer-specific config JSON
        if let Some(config) = streamer_config {
            if let Some(v) = config.get("output_folder").and_then(|v| v.as_str()) {
                self.output_folder = Some(v.to_string());
            }
            if let Some(v) = config
                .get("output_filename_template")
                .and_then(|v| v.as_str())
            {
                self.output_filename_template = Some(v.to_string());
            }
            if let Some(v) = config.get("download_engine").and_then(|v| v.as_str()) {
                self.download_engine = Some(v.to_string());
            }
            if let Some(v) = config.get("record_danmu").and_then(|v| v.as_bool()) {
                self.record_danmu = Some(v);
            }
            if let Some(v) = config.get("cookies").and_then(|v| v.as_str()) {
                self.cookies = Some(v.to_string());
            }
            if let Some(v) = config.get("max_bitrate").and_then(|v| v.as_i64()) {
                self.max_bitrate = Some(v as i32);
            }
            // Parse stream selection config from streamer-specific config
            if let Some(stream_sel) = config.get("stream_selection") {
                if let Ok(v) = serde_json::from_value::<StreamSelectionConfig>(stream_sel.clone()) {
                    if let Some(existing) = &self.stream_selection {
                        self.stream_selection = Some(existing.merge(&v));
                    } else {
                        self.stream_selection = Some(v);
                    }
                }
            }
        }
        self
    }

    /// Build the final MergedConfig.
    pub fn build(self) -> MergedConfig {
        MergedConfig {
            output_folder: self
                .output_folder
                .unwrap_or_else(|| "./downloads".to_string()),
            output_filename_template: self
                .output_filename_template
                .unwrap_or_else(|| "{streamer}-{title}-%Y%m%d-%H%M%S".to_string()),
            output_file_format: self.output_file_format.unwrap_or_else(|| "flv".to_string()),
            min_segment_size_bytes: self.min_segment_size_bytes.unwrap_or(1048576),
            max_download_duration_secs: self.max_download_duration_secs.unwrap_or(0),
            max_part_size_bytes: self.max_part_size_bytes.unwrap_or(8589934592),
            record_danmu: self.record_danmu.unwrap_or(false),
            danmu_sampling_config: self.danmu_sampling_config.unwrap_or_default(),
            proxy_config: self.proxy_config.unwrap_or_default(),
            cookies: self.cookies,
            download_engine: self.download_engine.unwrap_or_else(|| "mesio".to_string()),
            download_retry_policy: self.download_retry_policy.unwrap_or_default(),
            max_bitrate: self.max_bitrate,
            event_hooks: self.event_hooks.unwrap_or_default(),
            fetch_delay_ms: self.fetch_delay_ms.unwrap_or(60000),
            download_delay_ms: self.download_delay_ms.unwrap_or(1000),
            stream_selection: self.stream_selection.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merged_config_builder() {
        let config = MergedConfig::builder()
            .with_global(
                "./downloads".to_string(),
                "{streamer}-{title}".to_string(),
                "flv".to_string(),
                1024,
                0,
                8589934592,
                false,
                ProxyConfig::disabled(),
                "mesio".to_string(),
            )
            .with_platform(60000, 1000, None, None, None)
            .build();

        assert_eq!(config.output_folder, "./downloads");
        assert_eq!(config.download_engine, "mesio");
        assert!(!config.record_danmu);
    }

    #[test]
    fn test_layer_override() {
        let config = MergedConfig::builder()
            .with_global(
                "./downloads".to_string(),
                "{streamer}".to_string(),
                "flv".to_string(),
                1024,
                0,
                8589934592,
                false,
                ProxyConfig::disabled(),
                "mesio".to_string(),
            )
            .with_platform(60000, 1000, None, None, Some(true))
            .with_template(
                Some("./custom".to_string()),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Some("mesio".to_string()),
                None,
                None,
                None,
                None,
                None, // stream_selection
            )
            .build();

        // Template overrides global
        assert_eq!(config.output_folder, "./custom");
        assert_eq!(config.download_engine, "mesio");
        // Platform overrides global
        assert!(config.record_danmu);
    }
}
