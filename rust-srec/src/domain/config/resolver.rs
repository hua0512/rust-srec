//! Configuration resolution service.
//!
//! This module provides the ConfigResolver service that resolves the effective
//! configuration for a streamer by merging the 4-layer hierarchy:
//! Global → Platform → Template → Streamer

use tracing::debug;

use crate::Error;
use crate::credentials::{CredentialScope, CredentialSource};
use crate::database::models::job::DagPipelineDefinition;
use crate::database::repositories::config::ConfigRepository;
use crate::domain::config::ResolvedStreamerContext;
use crate::domain::config::merged::MergedConfig;
use crate::domain::streamer::Streamer;
use crate::domain::{DanmuSamplingConfig, EventHooks, ProxyConfig, RetryPolicy};
use crate::downloader::StreamSelectionConfig;
use crate::utils::json::{self, JsonContext};
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
        Ok(self
            .resolve_context_for_streamer(streamer)
            .await?
            .config
            .as_ref()
            .clone())
    }

    /// Resolve the effective configuration for a streamer plus runtime-only context.
    ///
    /// This returns the merged config along with the derived credential source, using the same
    /// platform/template records that were loaded during config resolution (no extra DB roundtrips).
    pub async fn resolve_context_for_streamer(
        &self,
        streamer: &Streamer,
    ) -> Result<ResolvedStreamerContext, Error> {
        // Start with builder
        debug!(
            "Resolving config for streamer: {} (Platform: {}, Template: {:?})",
            streamer.id, streamer.platform_config_id, streamer.template_config_id
        );
        let mut builder = MergedConfig::builder();

        // Layer 1: Global config
        let global_config = self.config_repo.get_global_config().await?;
        let global_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
            global_config.pipeline.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "global",
                scope_id: None,
                field: "pipeline",
            },
            "Invalid JSON config; ignoring",
        );
        let global_session_complete_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
            global_config.session_complete_pipeline.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "global",
                scope_id: None,
                field: "session_complete_pipeline",
            },
            "Invalid JSON config; ignoring",
        );
        let global_paired_segment_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
            global_config.paired_segment_pipeline.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "global",
                scope_id: None,
                field: "paired_segment_pipeline",
            },
            "Invalid JSON config; ignoring",
        );

        builder = builder.with_global(
            global_config.output_folder.clone(),
            global_config.output_filename_template.clone(),
            global_config.output_file_format.clone(),
            global_config.min_segment_size_bytes,
            global_config.max_download_duration_secs,
            global_config.max_part_size_bytes,
            global_config.record_danmu,
            json::parse_or_default(
                &global_config.proxy_config,
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "global",
                    scope_id: None,
                    field: "proxy_config",
                },
                "Invalid JSON config; using defaults",
            ),
            global_config.default_download_engine.clone(),
            global_config.session_gap_time_secs,
            global_pipeline,
            global_session_complete_pipeline,
            global_paired_segment_pipeline,
            global_config.auto_thumbnail,
        );

        // Layer 2: Platform config
        let platform_config = self
            .config_repo
            .get_platform_config(&streamer.platform_config_id)
            .await?;
        let platform_name = platform_config.platform_name.clone();
        let platform_proxy: Option<ProxyConfig> = json::parse_optional(
            platform_config.proxy_config.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "proxy_config",
            },
            "Invalid JSON config; ignoring",
        );

        let platform_stream_selection: Option<StreamSelectionConfig> = json::parse_optional(
            platform_config.stream_selection_config.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "stream_selection_config",
            },
            "Invalid JSON config; ignoring",
        );
        let platform_download_retry_policy: Option<RetryPolicy> = json::parse_optional(
            platform_config.download_retry_policy.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "download_retry_policy",
            },
            "Invalid JSON config; ignoring",
        );
        let platform_event_hooks: Option<EventHooks> = json::parse_optional(
            platform_config.event_hooks.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "event_hooks",
            },
            "Invalid JSON config; ignoring",
        );
        let platform_specific: Option<serde_json::Value> = json::parse_optional(
            platform_config.platform_specific_config.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "platform_specific_config",
            },
            "Invalid JSON config; ignoring",
        );
        let platform_refresh_token = platform_specific.as_ref().and_then(|v| {
            v.get("refresh_token")
                .and_then(|t| t.as_str())
                .map(String::from)
        });
        // `platform_specific_config` can also contain credential metadata (e.g. refresh_token),
        // but extractor `platform_extras` must not carry credentials.
        let platform_extras = platform_specific.map(strip_credential_fields_from_platform_extras);
        let platform_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
            platform_config.pipeline.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "pipeline",
            },
            "Invalid JSON config; ignoring",
        );
        let platform_session_complete_pipeline: Option<DagPipelineDefinition> =
            json::parse_optional(
                platform_config.session_complete_pipeline.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "platform",
                    scope_id: Some(&streamer.platform_config_id),
                    field: "session_complete_pipeline",
                },
                "Invalid JSON config; ignoring",
            );
        let platform_paired_segment_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
            platform_config.paired_segment_pipeline.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "platform",
                scope_id: Some(&streamer.platform_config_id),
                field: "paired_segment_pipeline",
            },
            "Invalid JSON config; ignoring",
        );

        builder = builder.with_platform(
            platform_config.fetch_delay_ms,
            platform_config.download_delay_ms,
            platform_config.cookies.clone(),
            platform_proxy,
            platform_config.record_danmu,
            platform_extras.as_ref(), // Pass as Option<&Value>
            platform_config.output_folder.clone(),
            platform_config.output_filename_template.clone(),
            platform_config.download_engine.clone(),
            platform_stream_selection,
            platform_config.output_file_format.clone(),
            platform_config.min_segment_size_bytes,
            platform_config.max_download_duration_secs,
            platform_config.max_part_size_bytes,
            platform_download_retry_policy,
            platform_event_hooks,
            platform_pipeline,
            platform_session_complete_pipeline,
            platform_paired_segment_pipeline,
        );

        let mut credential_source: Option<CredentialSource> = streamer
            .streamer_specific_config
            .as_ref()
            .and_then(|config| config.get("cookies").and_then(|v| v.as_str()))
            .map(str::to_string)
            .filter(|cookies| !cookies.trim().is_empty())
            .map(|cookies| {
                let refresh_token = streamer
                    .streamer_specific_config
                    .as_ref()
                    .and_then(|config| config.get("refresh_token").and_then(|v| v.as_str()))
                    .map(String::from);

                CredentialSource::new(
                    CredentialScope::Streamer {
                        streamer_id: streamer.id.clone(),
                        streamer_name: streamer.name.clone(),
                    },
                    cookies,
                    refresh_token,
                    platform_name.clone(),
                )
            });

        // Layer 3: Template config (if assigned)
        if let Some(ref template_id) = streamer.template_config_id {
            let template_config = self.config_repo.get_template_config(template_id).await?;

            // Parse JSON fields
            let template_proxy: Option<ProxyConfig> = json::parse_optional(
                template_config.proxy_config.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "proxy_config",
                },
                "Invalid JSON config; ignoring",
            );
            let template_retry: Option<RetryPolicy> = json::parse_optional(
                template_config.download_retry_policy.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "download_retry_policy",
                },
                "Invalid JSON config; ignoring",
            );
            let template_danmu: Option<DanmuSamplingConfig> = json::parse_optional(
                template_config.danmu_sampling_config.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "danmu_sampling_config",
                },
                "Invalid JSON config; ignoring",
            );
            let template_hooks: Option<EventHooks> = json::parse_optional(
                template_config.event_hooks.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "event_hooks",
                },
                "Invalid JSON config; ignoring",
            );

            let template_stream_selection: Option<StreamSelectionConfig> = json::parse_optional(
                template_config.stream_selection_config.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "stream_selection_config",
                },
                "Invalid JSON config; ignoring",
            );

            let template_engines_override: Option<serde_json::Value> = json::parse_optional(
                template_config.engines_override.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "engines_override",
                },
                "Invalid JSON config; ignoring",
            );

            // Parse platform_overrides to get platform-specific extras for this streamer's platform
            // platform_overrides is a JSON map: { "huya": {...}, "douyin": {...}, ... }
            let template_platform_overrides: Option<serde_json::Value> = json::parse_optional(
                template_config.platform_overrides.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "platform_overrides",
                },
                "Invalid JSON config; ignoring",
            );
            let template_refresh_token = template_platform_overrides
                .as_ref()
                .and_then(|map| map.get(&platform_name))
                .and_then(|entry| entry.get("refresh_token"))
                .and_then(|t| t.as_str())
                .map(String::from);

            let mut tpl_po_pipeline: Option<DagPipelineDefinition> = None;
            let mut tpl_po_session_complete: Option<DagPipelineDefinition> = None;
            let mut tpl_po_paired_segment: Option<DagPipelineDefinition> = None;
            let template_platform_extras: Option<serde_json::Value> = template_platform_overrides
                .and_then(|map| map.get(&platform_config.platform_name).cloned())
                .map(|mut entry| {
                    if let Some(obj) = entry.as_object_mut() {
                        tpl_po_pipeline = obj
                            .remove("pipeline")
                            .and_then(|v| serde_json::from_value(v).ok());
                        tpl_po_session_complete = obj
                            .remove("session_complete_pipeline")
                            .and_then(|v| serde_json::from_value(v).ok());
                        tpl_po_paired_segment = obj
                            .remove("paired_segment_pipeline")
                            .and_then(|v| serde_json::from_value(v).ok());
                    }
                    strip_credential_fields_from_platform_extras(entry)
                });

            let template_credential_candidate = if credential_source.is_none() {
                if let Some(cookies) = template_config.cookies.as_ref()
                    && !cookies.trim().is_empty()
                {
                    Some(CredentialSource::new(
                        CredentialScope::Template {
                            template_id: template_id.clone(),
                            template_name: template_config.name.clone(),
                        },
                        cookies.clone(),
                        template_refresh_token,
                        platform_name.clone(),
                    ))
                } else {
                    None
                }
            } else {
                None
            };
            let template_pipeline: Option<DagPipelineDefinition> = json::parse_optional(
                template_config.pipeline.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "template",
                    scope_id: Some(template_id),
                    field: "pipeline",
                },
                "Invalid JSON config; ignoring",
            );
            let template_session_complete_pipeline: Option<DagPipelineDefinition> =
                json::parse_optional(
                    template_config.session_complete_pipeline.as_deref(),
                    JsonContext::StreamerConfig {
                        streamer_id: &streamer.id,
                        scope: "template",
                        scope_id: Some(template_id),
                        field: "session_complete_pipeline",
                    },
                    "Invalid JSON config; ignoring",
                );
            let template_paired_segment_pipeline: Option<DagPipelineDefinition> =
                json::parse_optional(
                    template_config.paired_segment_pipeline.as_deref(),
                    JsonContext::StreamerConfig {
                        streamer_id: &streamer.id,
                        scope: "template",
                        scope_id: Some(template_id),
                        field: "paired_segment_pipeline",
                    },
                    "Invalid JSON config; ignoring",
                );

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
                template_engines_override,
                template_pipeline,
                template_session_complete_pipeline,
                template_paired_segment_pipeline,
                template_platform_extras, // platform_extras from platform_overrides
            );

            // Template platform overrides are more specific than top-level template
            // pipeline fields, so apply them after with_template().
            if let Some(pipe) = tpl_po_pipeline {
                builder = builder.override_pipeline(pipe);
            }
            if let Some(pipe) = tpl_po_session_complete {
                builder = builder.override_session_complete_pipeline(pipe);
            }
            if let Some(pipe) = tpl_po_paired_segment {
                builder = builder.override_paired_segment_pipeline(pipe);
            }

            if credential_source.is_none() {
                credential_source = template_credential_candidate;
            }
        }

        if credential_source.is_none()
            && let Some(cookies) = platform_config.cookies.as_ref()
            && !cookies.trim().is_empty()
        {
            credential_source = Some(CredentialSource::new(
                CredentialScope::Platform {
                    platform_id: streamer.platform_config_id.clone(),
                    platform_name: platform_name.clone(),
                },
                cookies.clone(),
                platform_refresh_token,
                platform_name.clone(),
            ));
        }

        // Layer 4: Streamer-specific config
        builder = builder.with_streamer(streamer.streamer_specific_config.as_ref());

        Ok(ResolvedStreamerContext {
            config: Arc::new(builder.build()),
            credential_source,
        })
    }
}

fn strip_credential_fields_from_platform_extras(
    mut extras: serde_json::Value,
) -> serde_json::Value {
    if let serde_json::Value::Object(ref mut map) = extras {
        // These keys belong to the credentials subsystem, not extractor config.
        map.remove("refresh_token");
        map.remove("last_cookie_check_date");
        map.remove("last_cookie_check_result");
    }
    extras
}

#[cfg(test)]
mod tests {
    // Tests would require mocking the ConfigRepository
    // which is covered in integration tests
}
