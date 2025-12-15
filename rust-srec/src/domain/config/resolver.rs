//! Configuration resolution service.
//!
//! This module provides the ConfigResolver service that resolves the effective
//! configuration for a streamer by merging the 4-layer hierarchy:
//! Global → Platform → Template → Streamer

use tracing::debug;

use crate::Error;
use crate::database::models::job::PipelineStep;
use crate::database::repositories::config::ConfigRepository;
use crate::domain::config::MergedConfig;
use crate::domain::streamer::Streamer;
use crate::domain::{DanmuSamplingConfig, EventHooks, ProxyConfig, RetryPolicy};
use std::sync::Arc;

/// Service for resolving configuration for streamers.
pub struct ConfigResolver<R: ConfigRepository> {
    config_repo: Arc<R>,
}

impl<R: ConfigRepository> ConfigResolver<R> {
    /// Create a new config resolver.
    pub fn new(config_repo: Arc<R>) -> Self {
        Self { config_repo }
    }

    /// Resolve the effective configuration for a streamer.
    ///
    /// This merges configuration from all 4 layers:
    /// 1. Global config (base)
    /// 2. Platform config (overrides global)
    /// 3. Template config (overrides platform)
    /// 4. Streamer-specific config (overrides template)
    pub async fn resolve_config_for_streamer(
        &self,
        streamer: &Streamer,
    ) -> Result<MergedConfig, Error> {
        // Start with builder
        debug!(
            "Resolving config for streamer: {} (Platform: {}, Template: {:?})",
            streamer.id, streamer.platform_config_id, streamer.template_config_id
        );
        let mut builder = MergedConfig::builder();

        // Layer 1: Global config
        let global_config = self.config_repo.get_global_config().await?;
        let global_proxy: ProxyConfig =
            serde_json::from_str(&global_config.proxy_config).unwrap_or_default();
        let global_pipeline: Option<Vec<PipelineStep>> = global_config
            .pipeline
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        builder = builder.with_global(
            global_config.output_folder,
            global_config.output_filename_template,
            global_config.output_file_format,
            global_config.min_segment_size_bytes,
            global_config.max_download_duration_secs,
            global_config.max_part_size_bytes,
            global_config.record_danmu,
            global_proxy,
            global_config.default_download_engine,
            global_config.session_gap_time_secs,
            global_pipeline,
        );

        // Layer 2: Platform config
        let platform_config = self
            .config_repo
            .get_platform_config(&streamer.platform_config_id)
            .await?;
        let platform_proxy: Option<ProxyConfig> = platform_config
            .proxy_config
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());
        let platform_pipeline: Option<Vec<PipelineStep>> = platform_config
            .pipeline
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());

        builder = builder.with_platform(
            platform_config.fetch_delay_ms,
            platform_config.download_delay_ms,
            platform_config.cookies.clone(),
            platform_proxy,
            platform_config.record_danmu,
            platform_config
                .platform_specific_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok())
                .as_ref(), // Pass as Option<&Value>
            platform_config.output_folder.clone(),
            platform_config.output_filename_template.clone(),
            platform_config.download_engine.clone(),
            platform_config
                .stream_selection_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            platform_config.output_file_format.clone(),
            platform_config.min_segment_size_bytes,
            platform_config.max_download_duration_secs,
            platform_config.max_part_size_bytes,
            platform_config
                .download_retry_policy
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            platform_config
                .event_hooks
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok()),
            platform_pipeline,
        );

        // Layer 3: Template config (if assigned)
        if let Some(ref template_id) = streamer.template_config_id {
            let template_config = self.config_repo.get_template_config(template_id).await?;

            // Parse JSON fields
            let template_proxy: Option<ProxyConfig> = template_config
                .proxy_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_retry: Option<RetryPolicy> = template_config
                .download_retry_policy
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_danmu: Option<DanmuSamplingConfig> = template_config
                .danmu_sampling_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_hooks: Option<EventHooks> = template_config
                .event_hooks
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());

            let template_stream_selection = template_config
                .stream_selection_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());

            let template_pipeline: Option<Vec<PipelineStep>> = template_config
                .pipeline
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());

            builder = builder.with_template(
                template_config.output_folder,
                template_config.output_filename_template,
                template_config.output_file_format,
                template_config.min_segment_size_bytes,
                template_config.max_download_duration_secs,
                template_config.max_part_size_bytes,
                template_config.record_danmu,
                template_proxy,
                template_config.cookies,
                template_config.download_engine,
                template_retry,
                template_danmu,
                template_hooks,
                template_stream_selection,
                template_config
                    .engines_override
                    .as_ref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                template_pipeline,
            );
        }

        // Layer 4: Streamer-specific config
        builder = builder.with_streamer(streamer.streamer_specific_config.as_ref());

        Ok(builder.build())
    }
}

#[cfg(test)]
mod tests {
    // Tests would require mocking the ConfigRepository
    // which is covered in integration tests
}
