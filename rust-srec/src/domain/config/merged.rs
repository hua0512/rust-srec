//! Merged configuration.

use crate::database::models::job::DagPipelineDefinition;
use crate::domain::{DanmuSamplingConfig, EventHooks, ProxyConfig, RetryPolicy};
use crate::downloader::StreamSelectionConfig;
use serde::{Deserialize, Serialize};
use tracing::debug;

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

    // Event hooks
    pub event_hooks: EventHooks,

    // Platform-specific
    pub fetch_delay_ms: i64,
    pub download_delay_ms: i64,
    pub session_gap_time_secs: i64,

    // Stream selection settings
    pub stream_selection: StreamSelectionConfig,

    // Engine overrides from template
    pub engines_override: Option<serde_json::Value>,

    // Pipeline configuration
    pub pipeline: Option<DagPipelineDefinition>,
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
    event_hooks: Option<EventHooks>,
    fetch_delay_ms: Option<i64>,
    download_delay_ms: Option<i64>,
    session_gap_time_secs: Option<i64>,
    stream_selection: Option<StreamSelectionConfig>,
    engines_override: Option<serde_json::Value>,
    pipeline: Option<DagPipelineDefinition>,
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
        session_gap_time_secs: i64,
        pipeline: Option<DagPipelineDefinition>,
    ) -> Self {
        debug!(
            "[Layer 1: Global] Setting base config: output_folder={}, output_format={}, engine={}, record_danmu={}, session_gap={}s, pipeline_steps={}",
            output_folder,
            output_file_format,
            download_engine,
            record_danmu,
            session_gap_time_secs,
            pipeline.as_ref().map(|p| p.steps.len()).unwrap_or(0)
        );
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
        self.session_gap_time_secs = Some(session_gap_time_secs);
        self.pipeline = pipeline;
        self
    }

    /// Apply platform config layer.
    #[allow(clippy::too_many_arguments)]
    pub fn with_platform(
        mut self,
        fetch_delay_ms: Option<i64>,
        download_delay_ms: Option<i64>,
        cookies: Option<String>,
        proxy_config: Option<ProxyConfig>,
        record_danmu: Option<bool>,
        platform_specific_config: Option<&serde_json::Value>,
        output_folder: Option<String>,
        output_filename_template: Option<String>,
        download_engine: Option<String>,
        stream_selection: Option<StreamSelectionConfig>,
        output_file_format: Option<String>,
        min_segment_size_bytes: Option<i64>,
        max_download_duration_secs: Option<i64>,
        max_part_size_bytes: Option<i64>,
        download_retry_policy: Option<RetryPolicy>,
        event_hooks: Option<EventHooks>,
        pipeline: Option<DagPipelineDefinition>,
    ) -> Self {
        debug!(
            "[Layer 2: Platform] Applying overrides: output_folder={:?}, engine={:?}, record_danmu={:?}, cookies={}, stream_selection={}, pipeline_steps={}",
            output_folder,
            download_engine,
            record_danmu,
            cookies.is_some(),
            stream_selection.is_some(),
            pipeline.as_ref().map(|p| p.steps.len()).unwrap_or(0)
        );
        if let Some(v) = fetch_delay_ms {
            self.fetch_delay_ms = Some(v);
        }
        if let Some(v) = download_delay_ms {
            self.download_delay_ms = Some(v);
        }

        if cookies.is_some() {
            self.cookies = cookies;
        }
        if let Some(proxy) = proxy_config {
            self.proxy_config = Some(proxy);
        }
        if let Some(danmu) = record_danmu {
            self.record_danmu = Some(danmu);
        }

        if let Some(pipe) = pipeline {
            debug!("Platform config override: pipeline");
            self.pipeline = Some(pipe);
        }

        // Apply platform-specific config overrides
        if let Some(config) = platform_specific_config {
            debug!("Applying platform-specific config overrides");
        }

        // Apply explicit overrides
        if let Some(v) = output_folder {
            debug!("Platform override: output_folder = {}", v);
            self.output_folder = Some(v);
        }
        if let Some(v) = output_filename_template {
            debug!("Platform override: output_filename_template = {}", v);
            self.output_filename_template = Some(v);
        }
        if let Some(v) = download_engine {
            debug!("Platform override: download_engine = {}", v);
            self.download_engine = Some(v);
        }
        if let Some(v) = stream_selection {
            if let Some(existing) = &self.stream_selection {
                debug!("Platform override: merging stream_selection");
                self.stream_selection = Some(existing.merge(&v));
            } else {
                debug!("Platform override: stream_selection");
                self.stream_selection = Some(v);
            }
        }
        if let Some(v) = output_file_format {
            debug!("Platform override: output_file_format = {}", v);
            self.output_file_format = Some(v);
        }
        if let Some(v) = min_segment_size_bytes {
            debug!("Platform override: min_segment_size_bytes = {}", v);
            self.min_segment_size_bytes = Some(v);
        }
        if let Some(v) = max_download_duration_secs {
            debug!("Platform override: max_download_duration_secs = {}", v);
            self.max_download_duration_secs = Some(v);
        }
        if let Some(v) = max_part_size_bytes {
            debug!("Platform override: max_part_size_bytes = {}", v);
            self.max_part_size_bytes = Some(v);
        }
        if let Some(v) = download_retry_policy {
            debug!("Platform override: download_retry_policy");
            self.download_retry_policy = Some(v);
        }
        if let Some(v) = event_hooks {
            if let Some(existing) = &self.event_hooks {
                debug!("Platform override: merging event_hooks");
                self.event_hooks = Some(existing.merge(&v));
            } else {
                debug!("Platform override: event_hooks");
                self.event_hooks = Some(v);
            }
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
        event_hooks: Option<EventHooks>,
        stream_selection: Option<StreamSelectionConfig>,
        engines_override: Option<serde_json::Value>,
        pipeline: Option<DagPipelineDefinition>,
    ) -> Self {
        debug!(
            "[Layer 3: Template] Applying overrides: output_folder={:?}, engine={:?}, record_danmu={:?}, cookies={}, stream_selection={}, engines_override={}, pipeline_steps={}",
            output_folder,
            download_engine,
            record_danmu,
            cookies.is_some(),
            stream_selection.is_some(),
            engines_override.is_some(),
            pipeline.as_ref().map(|p| p.steps.len()).unwrap_or(0)
        );
        if let Some(v) = output_folder {
            debug!("Template override: output_folder = {}", v);
            self.output_folder = Some(v);
        }
        if let Some(v) = output_filename_template {
            debug!("Template override: output_filename_template = {}", v);
            self.output_filename_template = Some(v);
        }
        if let Some(v) = output_file_format {
            debug!("Template override: output_file_format = {}", v);
            self.output_file_format = Some(v);
        }
        if let Some(v) = min_segment_size_bytes {
            debug!("Template override: min_segment_size_bytes = {}", v);
            self.min_segment_size_bytes = Some(v);
        }
        if let Some(v) = max_download_duration_secs {
            debug!("Template override: max_download_duration_secs = {}", v);
            self.max_download_duration_secs = Some(v);
        }
        if let Some(v) = max_part_size_bytes {
            debug!("Template override: max_part_size_bytes = {}", v);
            self.max_part_size_bytes = Some(v);
        }
        if let Some(v) = record_danmu {
            debug!("Template override: record_danmu = {}", v);
            self.record_danmu = Some(v);
        }
        if let Some(v) = proxy_config {
            debug!("Template override: proxy_config");
            self.proxy_config = Some(v);
        }
        if cookies.is_some() {
            debug!("Template override: cookies");
            self.cookies = cookies;
        }
        if let Some(v) = download_engine {
            debug!("Template override: download_engine = {}", v);
            self.download_engine = Some(v);
        }
        if let Some(v) = download_retry_policy {
            debug!("Template override: download_retry_policy");
            self.download_retry_policy = Some(v);
        }
        if let Some(v) = danmu_sampling_config {
            debug!("Template override: danmu_sampling_config");
            self.danmu_sampling_config = Some(v);
        }
        if let Some(v) = event_hooks {
            // Merge event hooks
            if let Some(existing) = &self.event_hooks {
                debug!("Template override: merging event_hooks");
                self.event_hooks = Some(existing.merge(&v));
            } else {
                debug!("Template override: event_hooks");
                self.event_hooks = Some(v);
            }
        }
        if let Some(v) = stream_selection {
            // Merge stream selection config
            if let Some(existing) = &self.stream_selection {
                debug!("Template override: merging stream_selection");
                self.stream_selection = Some(existing.merge(&v));
            } else {
                debug!("Template override: stream_selection");
                self.stream_selection = Some(v);
            }
        }
        if let Some(v) = engines_override {
            debug!("Template override: engines_override");
            self.engines_override = Some(v);
        }
        if let Some(pipe) = pipeline {
            debug!("Template override: pipeline");
            self.pipeline = Some(pipe);
        }
        self
    }

    /// Apply streamer-specific config layer.
    pub fn with_streamer(mut self, streamer_config: Option<&serde_json::Value>) -> Self {
        debug!(
            "[Layer 4: Streamer] Applying overrides: streamer_config={}",
            streamer_config.is_some()
        );

        // Parse streamer-specific config JSON
        if let Some(config) = streamer_config {
            debug!("Applying streamer-specific config overrides: {}", config);
            if let Some(v) = config.get("output_folder").and_then(|v| v.as_str()) {
                debug!("Streamer config override: output_folder = {}", v);
                self.output_folder = Some(v.to_string());
            }
            if let Some(v) = config
                .get("output_filename_template")
                .and_then(|v| v.as_str())
            {
                debug!("Streamer config override: output_filename_template = {}", v);
                self.output_filename_template = Some(v.to_string());
            }
            if let Some(v) = config.get("download_engine").and_then(|v| v.as_str()) {
                debug!("Streamer config override: download_engine = {}", v);
                self.download_engine = Some(v.to_string());
            }
            if let Some(v) = config.get("record_danmu").and_then(|v| v.as_bool()) {
                debug!("Streamer config override: record_danmu = {}", v);
                self.record_danmu = Some(v);
            }
            if let Some(v) = config.get("cookies").and_then(|v| v.as_str()) {
                debug!("Streamer config override: cookies");
                self.cookies = Some(v.to_string());
            }
            if let Some(v) = config.get("max_part_size_bytes").and_then(|v| v.as_i64()) {
                debug!("Streamer config override: max_part_size_bytes = {}", v);
                self.max_part_size_bytes = Some(v);
            }
            if let Some(v) = config
                .get("max_download_duration_secs")
                .and_then(|v| v.as_i64())
            {
                debug!(
                    "Streamer config override: max_download_duration_secs = {}",
                    v
                );
                self.max_download_duration_secs = Some(v);
            }
            if let Some(v) = config.get("output_file_format").and_then(|v| v.as_str()) {
                debug!("Streamer config override: output_file_format = {}", v);
                self.output_file_format = Some(v.to_string());
            }
            if let Some(v) = config
                .get("min_segment_size_bytes")
                .and_then(|v| v.as_i64())
            {
                debug!("Streamer config override: min_segment_size_bytes = {}", v);
                self.min_segment_size_bytes = Some(v);
            }

            // Parse proxy config from streamer-specific config
            if let Some(proxy_val) = config.get("proxy_config") {
                if let Ok(v) = serde_json::from_value::<ProxyConfig>(proxy_val.clone()) {
                    debug!("Streamer config override: proxy_config");
                    self.proxy_config = Some(v);
                }
            }

            // Parse stream selection config from streamer-specific config
            if let Some(stream_sel) = config.get("stream_selection_config") {
                if let Ok(v) = serde_json::from_value::<StreamSelectionConfig>(stream_sel.clone()) {
                    if let Some(existing) = &self.stream_selection {
                        debug!("Streamer config override: merging stream_selection");
                        self.stream_selection = Some(existing.merge(&v));
                    } else {
                        debug!("Streamer config override: stream_selection");
                        self.stream_selection = Some(v);
                    }
                }
            }

            // Parse event hooks from streamer-specific config
            if let Some(hooks_val) = config.get("event_hooks") {
                if let Ok(v) = serde_json::from_value::<EventHooks>(hooks_val.clone()) {
                    if let Some(existing) = &self.event_hooks {
                        debug!("Streamer config override: merging event_hooks");
                        self.event_hooks = Some(existing.merge(&v));
                    } else {
                        debug!("Streamer config override: event_hooks");
                        self.event_hooks = Some(v);
                    }
                }
            }

            // Parse pipeline config from streamer-specific config (Flexible: Enum untagged)
            if let Some(pipeline_val) = config.get("pipeline") {
                if let Ok(v) = serde_json::from_value::<DagPipelineDefinition>(pipeline_val.clone())
                {
                    debug!(
                        "Streamer config override: pipeline ({} items/nodes)",
                        v.steps.len()
                    );
                    self.pipeline = Some(v);
                }
            }

            if let Some(v) = config.get("download_retry_policy") {
                if let Ok(v) = serde_json::from_value::<RetryPolicy>(v.clone()) {
                    debug!("Streamer config override: download_retry_policy");
                    self.download_retry_policy = Some(v);
                }
            }

            if let Some(v) = config.get("danmu_sampling_config") {
                if let Ok(v) = serde_json::from_value::<DanmuSamplingConfig>(v.clone()) {
                    debug!("Streamer config override: danmu_sampling_config");
                    self.danmu_sampling_config = Some(v);
                }
            }
        }
        self
    }

    /// Build the final MergedConfig.
    pub fn build(self) -> MergedConfig {
        let output_folder = self
            .output_folder
            .unwrap_or_else(|| "./downloads".to_string());
        let output_filename_template = self
            .output_filename_template
            .unwrap_or_else(|| "{streamer}-{title}-%Y%m%d-%H%M%S".to_string());
        let output_file_format = self.output_file_format.unwrap_or_else(|| "flv".to_string());
        let download_engine = self.download_engine.unwrap_or_else(|| "mesio".to_string());
        let record_danmu = self.record_danmu.unwrap_or(false);
        let stream_selection = self.stream_selection.unwrap_or_default();
        let pipeline = self.pipeline;

        debug!(
            "[Config Merge Complete] Final config: output_folder={}, format={}, engine={}, record_danmu={}, stream_selection={:?}, pipeline_steps={}",
            output_folder,
            output_file_format,
            download_engine,
            record_danmu,
            stream_selection,
            pipeline.as_ref().map(|p| p.steps.len()).unwrap_or(0)
        );

        MergedConfig {
            output_folder,
            output_filename_template,
            output_file_format,
            min_segment_size_bytes: self.min_segment_size_bytes.unwrap_or(1048576),
            max_download_duration_secs: self.max_download_duration_secs.unwrap_or(0),
            max_part_size_bytes: self.max_part_size_bytes.unwrap_or(8589934592),
            record_danmu,
            danmu_sampling_config: self.danmu_sampling_config.unwrap_or_default(),
            proxy_config: self.proxy_config.unwrap_or_default(),
            cookies: self.cookies,
            download_engine,
            download_retry_policy: self.download_retry_policy.unwrap_or_default(),
            event_hooks: self.event_hooks.unwrap_or_default(),
            fetch_delay_ms: self.fetch_delay_ms.unwrap_or(60000),
            download_delay_ms: self.download_delay_ms.unwrap_or(1000),
            session_gap_time_secs: self.session_gap_time_secs.unwrap_or(3600),
            stream_selection,
            engines_override: self.engines_override,
            pipeline,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::database::models::PipelineStep;

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
                600,
                None, // pipeline
            )
            .with_platform(
                Some(60000),
                Some(1000),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
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
                "ffmpeg".to_string(),
                600,
                None,
            )
            .with_platform(
                Some(60000),
                Some(1000),
                None,
                None,
                Some(true),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
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
                None, // stream_selection
                None, // engines_override
                None, // pipeline
            )
            .build();

        // Template overrides global
        assert_eq!(config.output_folder, "./custom");
        assert_eq!(config.download_engine, "mesio");
        // Platform overrides global
        assert!(config.record_danmu);
    }

    #[test]
    fn test_streamer_pipeline_override() {
        // Create a streamer-specific config with custom pipeline
        // PipelineStep uses #[serde(tag = "type")], so Preset requires {"type": "preset", "name": "..."}
        let streamer_config = serde_json::json!({
            "pipeline": [
                {"type": "preset", "name": "fast_remux"},
                {"type": "preset", "name": "s3_upload"}
            ]
        });

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
                600,
                Some(DagPipelineDefinition::new(
                    "global",
                    vec![
                        crate::database::models::job::DagStep::new(
                            "step1",
                            PipelineStep::preset("remux"),
                        ),
                        crate::database::models::job::DagStep::new(
                            "step2",
                            PipelineStep::preset("upload"),
                        ),
                    ],
                )),
            )
            .with_platform(
                Some(60000),
                Some(1000),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .with_streamer(Some(&streamer_config))
            .build();

        // Streamer pipeline should override global
        let dag = config.pipeline.expect("Pipeline should be set");
        assert_eq!(dag.steps.len(), 2);
        assert!(matches!(&dag.steps[0].step, PipelineStep::Preset{name} if name == "fast_remux"));
        assert!(matches!(&dag.steps[1].step, PipelineStep::Preset{name} if name == "s3_upload"));
    }

    #[test]
    fn test_streamer_inline_pipeline() {
        // Create a streamer-specific config with inline pipeline step
        // PipelineStep uses #[serde(tag = "type")]:
        // - Preset is {"type": "preset", "name": "..."}
        // - Inline is {"type": "inline", "processor": "...", "config": {...}}
        let streamer_config = serde_json::json!({
            "pipeline": [
                {"type": "preset", "name": "remux"},
                {
                    "type": "inline",
                    "processor": "execute",
                    "config": {
                        "command": "echo {input}"
                    }
                }
            ]
        });

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
                600,
                None,
            )
            .with_platform(
                Some(60000),
                Some(1000),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .with_streamer(Some(&streamer_config))
            .build();

        // Should have 2 steps: preset + inline
        let dag = config.pipeline.expect("Pipeline should be set");
        assert_eq!(dag.steps.len(), 2);
        assert!(matches!(&dag.steps[0].step, PipelineStep::Preset{name} if name == "remux"));
        assert!(
            matches!(&dag.steps[1].step, PipelineStep::Inline { processor, .. } if processor == "execute")
        );
    }
}
