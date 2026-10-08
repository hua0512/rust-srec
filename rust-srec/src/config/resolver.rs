//! Configuration resolution service.
//!
//! This module provides the ConfigResolver service that resolves the effective
//! configuration for a streamer by merging the 4-layer hierarchy:
//! Global → Platform → Template → Streamer

use platforms_parser::extractor::factory::ExtractorSelection;
use tracing::{debug, warn};

use crate::Error;
use crate::database::models::job::DagPipelineDefinition;
use crate::database::repositories::config::ConfigRepository;
use crate::domain::streamer::Streamer;
use crate::domain::{DanmuStatisticsConfig, RetryPolicy};
use crate::downloader::StreamSelectionConfig;
use crate::utils::json::{self, JsonContext};
use std::sync::Arc;

use super::{
    GlobalConfigLayer, MergedConfig, PlatformConfigLayer, ResolvedStreamerContext,
    TemplateConfigLayer,
};

/// Parse a stored extractor name into [`ExtractorSelection`].
///
/// `None` means the layer expresses no preference and the layer below it wins. An unrecognized
/// name is treated the same way rather than failing resolution, so a bad value degrades to
/// inheritance instead of stopping the streamer from recording.
fn parse_extractor(value: Option<&str>, scope: &str) -> Option<ExtractorSelection> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    match value.parse::<ExtractorSelection>() {
        Ok(selection) => Some(selection),
        Err(e) => {
            warn!("Ignoring {} extractor setting: {}", scope, e);
            None
        }
    }
}

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
    /// The merged config carries the streamer's route and account selection policy, read with
    /// the same platform/template records loaded for the merge.
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
        let global_danmu_statistics: Option<DanmuStatisticsConfig> = json::parse_optional(
            global_config.danmu_statistics.as_deref(),
            JsonContext::StreamerConfig {
                streamer_id: &streamer.id,
                scope: "global",
                scope_id: None,
                field: "danmu_statistics",
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

        builder = builder.with_global(GlobalConfigLayer {
            output_folder: global_config.output_folder.clone(),
            output_filename_template: global_config.output_filename_template.clone(),
            output_file_format: global_config.output_file_format.clone(),
            min_segment_size_bytes: global_config.min_segment_size_bytes,
            max_download_duration_secs: global_config.max_download_duration_secs,
            max_part_size_bytes: global_config.max_part_size_bytes,
            record_danmu: global_config.record_danmu,
            danmu_statistics: global_danmu_statistics,
            download_engine: global_config.default_download_engine.clone(),
            extractor: parse_extractor(global_config.default_extractor.as_deref(), "global"),
            pipeline: global_pipeline,
            session_complete_pipeline: global_session_complete_pipeline,
            paired_segment_pipeline: global_paired_segment_pipeline,
            auto_thumbnail: global_config.auto_thumbnail,
            offline_check_count: global_config.offline_check_count.max(0) as u32,
            offline_check_delay_ms: global_config.offline_check_delay_ms.max(0) as u64,
        });

        // Layer 2: Platform config
        let platform_config = self
            .config_repo
            .get_platform_config(&streamer.platform_config_id)
            .await?;

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

        builder = builder.with_platform(PlatformConfigLayer {
            fetch_delay_ms: platform_config.fetch_delay_ms,
            download_delay_ms: platform_config.download_delay_ms,
            record_danmu: platform_config.record_danmu,
            danmu_statistics: json::parse_optional(
                platform_config.danmu_statistics.as_deref(),
                JsonContext::StreamerConfig {
                    streamer_id: &streamer.id,
                    scope: "platform",
                    scope_id: Some(&platform_config.platform_name),
                    field: "danmu_statistics",
                },
                "Invalid JSON config; ignoring",
            ),
            platform_specific_config: platform_specific,
            output_folder: platform_config.output_folder.clone(),
            output_filename_template: platform_config.output_filename_template.clone(),
            download_engine: platform_config.download_engine.clone(),
            extractor: parse_extractor(platform_config.extractor.as_deref(), "platform"),
            stream_selection: platform_stream_selection,
            output_file_format: platform_config.output_file_format.clone(),
            min_segment_size_bytes: platform_config.min_segment_size_bytes,
            max_download_duration_secs: platform_config.max_download_duration_secs,
            max_part_size_bytes: platform_config.max_part_size_bytes,
            download_retry_policy: platform_download_retry_policy,
            pipeline: platform_pipeline,
            session_complete_pipeline: platform_session_complete_pipeline,
            paired_segment_pipeline: platform_paired_segment_pipeline,
            offline_check_count: platform_config.offline_check_count,
            offline_check_delay_ms: platform_config.offline_check_delay_ms,
        });

        // Layer 3: Template config (if assigned)
        if let Some(ref template_id) = streamer.template_config_id {
            let template_config = self.config_repo.get_template_config(template_id).await?;

            // Parse JSON fields
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
                        // The template form stores extractor options in this wrapper. Keep
                        // supporting flat configurations; explicit nested nulls must replace
                        // old flat overrides so the subsequent layer merge can inherit.
                        if let Some(serde_json::Value::Object(specific)) =
                            obj.remove("platform_specific_config")
                        {
                            obj.extend(specific);
                        }
                    }
                    entry
                });

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

            builder = builder.with_template(TemplateConfigLayer {
                output_folder: template_config.output_folder,
                output_filename_template: template_config.output_filename_template,
                output_file_format: template_config.output_file_format,
                min_segment_size_bytes: template_config.min_segment_size_bytes,
                max_download_duration_secs: template_config.max_download_duration_secs,
                max_part_size_bytes: template_config.max_part_size_bytes,
                record_danmu: template_config.record_danmu,
                danmu_statistics: json::parse_optional(
                    template_config.danmu_statistics.as_deref(),
                    JsonContext::StreamerConfig {
                        streamer_id: &streamer.id,
                        scope: "template",
                        scope_id: Some(&template_config.id),
                        field: "danmu_statistics",
                    },
                    "Invalid JSON config; ignoring",
                ),
                download_engine: template_config.download_engine,
                extractor: parse_extractor(template_config.extractor.as_deref(), "template"),
                download_retry_policy: template_retry,
                stream_selection: template_stream_selection,
                engines_override: template_engines_override,
                pipeline: template_pipeline,
                session_complete_pipeline: template_session_complete_pipeline,
                paired_segment_pipeline: template_paired_segment_pipeline,
                platform_extras: template_platform_extras,
                offline_check_count: template_config.offline_check_count,
                offline_check_delay_ms: template_config.offline_check_delay_ms,
            });

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
        }

        // Layer 4: Streamer-specific config
        builder = builder.with_streamer(streamer.streamer_specific_config.as_ref());

        let selections = self
            .config_repo
            .streamer_credential_selections(
                &streamer.id,
                &platform_config.id,
                streamer.template_config_id.as_deref(),
            )
            .await?;
        let mut config = builder.build();
        config.proxy_route = self
            .config_repo
            .resolve_streamer_route(
                &streamer.id,
                &platform_config.id,
                streamer.template_config_id.as_deref(),
            )
            .await?;
        config.credential_policy =
            crate::credentials::resolve_authentication(&platform_config.id, &selections)?;
        // Account material comes only from the selected profile; configuration
        // extras keep content settings such as room passwords.
        config.platform_extras = config.platform_extras.map(|extras| {
            crate::credentials::isolate_platform_authentication_extras(
                &platform_config.platform_name,
                extras,
            )
        });
        Ok(ResolvedStreamerContext {
            config: Arc::new(config),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::SqlxConfigRepository;
    use crate::database::{init_pool, run_migrations};
    use crate::domain::StreamerUrl;
    use serde_json::json;

    /// Extras reach extractors: account fields from any layer are removed
    /// once the layers are merged, nested option objects included.
    #[tokio::test]
    async fn merged_extras_carry_no_account_fields() {
        let pool = init_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::query("UPDATE platform_config SET platform_specific_config = ? WHERE id = 'platform-bilibili'")
            .bind(json!({"quality": 20000, "refresh_token": "platform-secret"}).to_string())
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO template_config (id, name, platform_overrides) VALUES ('account-template', 'Accounts', ?)",
        )
        .bind(
            json!({"bilibili": {
                "access_token": "template-secret",
                "platform_specific_config": {"session_cookies": "template-secret"}
            }})
            .to_string(),
        )
        .execute(&pool)
        .await
        .unwrap();
        let resolver = ConfigResolver::new(Arc::new(SqlxConfigRepository::new(
            pool.clone(),
            pool.clone(),
        )));
        let mut streamer = Streamer::new(
            "Accounts",
            StreamerUrl::new("https://live.bilibili.com/2").unwrap(),
            "platform-bilibili",
        )
        .with_template("account-template");
        streamer.streamer_specific_config = Some(json!({"platform_extras": {
            "quality": 80,
            "last_cookie_check_result": "streamer-secret",
            "platform_extras": {"cookies": "streamer-secret"}
        }}));

        let config = resolver
            .resolve_config_for_streamer(&streamer)
            .await
            .unwrap();
        let extras = config.platform_extras.unwrap();
        assert_eq!(extras["quality"], 80);
        assert!(!extras.to_string().contains("secret"), "{extras}");
    }

    #[tokio::test]
    async fn bilibili_quality_inherits_through_saved_template_and_streamer_options() {
        let pool = init_pool("sqlite::memory:").await.unwrap();
        run_migrations(&pool).await.unwrap();
        sqlx::query("UPDATE platform_config SET platform_specific_config = ? WHERE id = 'platform-bilibili'")
            .bind(json!({"quality": 20000, "end_stream_on_danmu_stream_closed": false}).to_string())
            .execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO template_config (id, name) VALUES ('quality-template', 'Quality test')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let resolver = ConfigResolver::new(Arc::new(SqlxConfigRepository::new(
            pool.clone(),
            pool.clone(),
        )));
        let mut streamer = Streamer::new(
            "Quality test",
            StreamerUrl::new("https://live.bilibili.com/1").unwrap(),
            "platform-bilibili",
        )
        .with_template("quality-template");

        for (template, streamer_options, expected) in [
            (json!({}), json!({}), 20000),
            (
                json!({"platform_specific_config": {"quality": null}}),
                json!({"quality": null}),
                20000,
            ),
            (
                json!({"platform_specific_config": {"quality": 400}}),
                json!({}),
                400,
            ),
            (json!({"quality": 400}), json!({}), 400),
            (
                json!({"quality": 400, "platform_specific_config": {"quality": null}}),
                json!({}),
                20000,
            ),
            (
                json!({"platform_specific_config": {"quality": 400}}),
                json!({"quality": 80}),
                80,
            ),
            (
                json!({"platform_specific_config": {"quality": 400}}),
                json!({"quality": null}),
                400,
            ),
            (
                json!({"platform_specific_config": {"quality": 0}}),
                json!({}),
                0,
            ),
        ] {
            sqlx::query(
                "UPDATE template_config SET platform_overrides = ? WHERE id = 'quality-template'",
            )
            .bind(json!({"bilibili": template}).to_string())
            .execute(&pool)
            .await
            .unwrap();
            streamer.streamer_specific_config = Some(json!({"platform_extras": streamer_options}));
            let config = resolver
                .resolve_config_for_streamer(&streamer)
                .await
                .unwrap();
            let extras = config.platform_extras.unwrap();
            assert_eq!(
                extras["quality"], expected,
                "template: {template}; streamer: {streamer_options}"
            );
            assert_eq!(extras["end_stream_on_danmu_stream_closed"], false);
            assert!(extras.get("platform_specific_config").is_none());
        }
    }
}
