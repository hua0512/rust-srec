//! Configuration resolution service.
//!
//! This module provides the ConfigResolver service that resolves the effective
//! configuration for a streamer by merging the 4-layer hierarchy:
//! Global → Platform → Template → Streamer

use crate::database::repositories::config::ConfigRepository;
use crate::domain::config::MergedConfig;
use crate::domain::streamer::Streamer;
use crate::domain::{ProxyConfig, RetryPolicy, DanmuSamplingConfig, EventHooks};
use crate::Error;
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
    pub async fn resolve_config_for_streamer(&self, streamer: &Streamer) -> Result<MergedConfig, Error> {
        // Start with builder
        let mut builder = MergedConfig::builder();

        // Layer 1: Global config
        let global_config = self.config_repo.get_global_config().await?;
        let global_proxy: ProxyConfig = serde_json::from_str(&global_config.proxy_config)
            .unwrap_or_default();
        
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
        );

        // Layer 2: Platform config
        let platform_config = self.config_repo.get_platform_config(&streamer.platform_config_id).await?;
        let platform_proxy: Option<ProxyConfig> = platform_config.proxy_config
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok());
        
        builder = builder.with_platform(
            platform_config.fetch_delay_ms,
            platform_config.download_delay_ms,
            platform_config.cookies.clone(),
            platform_proxy,
            platform_config.record_danmu,
        );

        // Layer 3: Template config (if assigned)
        if let Some(ref template_id) = streamer.template_config_id {
            let template_config = self.config_repo.get_template_config(template_id).await?;
            
            // Parse JSON fields
            let template_proxy: Option<ProxyConfig> = template_config.proxy_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_retry: Option<RetryPolicy> = template_config.download_retry_policy
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_danmu: Option<DanmuSamplingConfig> = template_config.danmu_sampling_config
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            let template_hooks: Option<EventHooks> = template_config.event_hooks
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());
            
            let template_stream_selection = template_config.stream_selection_config
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
                template_config.max_bitrate,
                template_hooks,
                template_stream_selection,
            );
        }

        // Layer 4: Streamer-specific config
        builder = builder.with_streamer(
            streamer.download_retry_policy.clone(),
            streamer.danmu_sampling_config.clone(),
            streamer.streamer_specific_config.as_ref(),
        );

        Ok(builder.build())
    }
}

#[cfg(test)]
mod tests {
    // Tests would require mocking the ConfigRepository
    // which is covered in integration tests
}
