//! Configuration export: assembles the full backup document from the database.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use sqlx::SqlitePool;

use crate::config::ConfigService;
use crate::config::backup::{
    BackupRoute, ConfigExport, EXPORT_SCHEMA_VERSION, EngineExport, FilterExport,
    GlobalConfigExport, JobPresetExport, NotificationChannelExport, PipelinePresetExport,
    PlatformExport, ProxyExport, StreamerExport, TemplateExport, UserExport, export_filter_config,
    unwrap_json_value,
};
use crate::credentials::{CredentialOwner, CredentialSelection};
use crate::database::models::{
    GlobalConfigDbModel, NotificationChannelDbModel, PlatformConfigDbModel, StreamerDbModel,
    TemplateConfigDbModel, UserDbModel,
};
use crate::database::repositories::proxies::RouteOwner;
use crate::database::repositories::{
    FilterRepository, JobPresetRepository, NotificationRepository, PipelinePresetRepository,
    StoredSelection, StreamerRepository, UserRepository, config::SqlxConfigRepository,
    credential_selections, streamer::SqlxStreamerRepository,
};

type RuntimeConfigService = ConfigService<SqlxConfigRepository, SqlxStreamerRepository>;

/// Helper to parse a database string into a normalized JSON Value.
fn parse_db_config(s: impl Into<String>) -> serde_json::Value {
    let s = s.into();
    if s.is_empty() {
        return serde_json::Value::Null;
    }
    let value = serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s));
    unwrap_json_value(value)
}

// The configuration exports below destructure their row without `..`, so a
// new column does not compile until it is exported or skipped on purpose.

fn global_export(config: GlobalConfigDbModel, route: BackupRoute) -> GlobalConfigExport {
    let GlobalConfigDbModel {
        id: _, // the singleton row's ID is local to each installation
        output_folder,
        output_filename_template,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        record_danmu,
        danmu_statistics,
        max_concurrent_downloads,
        max_concurrent_uploads,
        streamer_check_delay_ms,
        offline_check_delay_ms,
        offline_check_count,
        default_download_engine,
        default_extractor,
        max_concurrent_cpu_jobs,
        max_concurrent_io_jobs,
        job_history_retention_days,
        notification_event_log_retention_days,
        output_retention_days,
        output_retention_delete_files,
        pipeline,
        session_complete_pipeline,
        paired_segment_pipeline,
        log_filter_directive,
        auto_thumbnail,
        pipeline_cpu_job_timeout_secs,
        pipeline_io_job_timeout_secs,
        pipeline_execute_timeout_secs,
        queue_freshness_threshold_ms,
        gpu_health_probe_interval_secs,
        stream_proxy_allow_private_targets,
    } = config;
    GlobalConfigExport {
        output_folder,
        output_filename_template,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        record_danmu,
        danmu_statistics,
        max_concurrent_downloads,
        max_concurrent_uploads,
        streamer_check_delay_ms,
        proxy_config: serde_json::Value::Null,
        proxy_route: Some(route),
        offline_check_delay_ms,
        offline_check_count,
        default_download_engine,
        default_extractor,
        max_concurrent_cpu_jobs,
        max_concurrent_io_jobs,
        job_history_retention_days,
        notification_event_log_retention_days,
        output_retention_days,
        output_retention_delete_files,
        pipeline: pipeline.map(parse_db_config),
        session_complete_pipeline: session_complete_pipeline.map(parse_db_config),
        paired_segment_pipeline: paired_segment_pipeline.map(parse_db_config),
        log_filter_directive: Some(log_filter_directive),
        auto_thumbnail,
        pipeline_cpu_job_timeout_secs,
        pipeline_io_job_timeout_secs,
        pipeline_execute_timeout_secs,
        queue_freshness_threshold_ms,
        gpu_health_probe_interval_secs,
        stream_proxy_allow_private_targets,
    }
}

fn template_export(
    template: TemplateConfigDbModel,
    selections: &[StoredSelection],
    route: Option<BackupRoute>,
) -> TemplateExport {
    let TemplateConfigDbModel {
        id: _, // bundles name templates; IDs stay local
        name,
        output_folder,
        output_filename_template,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        record_danmu,
        danmu_statistics,
        platform_overrides,
        download_retry_policy,
        download_engine,
        extractor,
        engines_override,
        stream_selection_config,
        pipeline,
        session_complete_pipeline,
        paired_segment_pipeline,
        offline_check_count,
        offline_check_delay_ms,
        created_at: _, // an import stamps its own times
        updated_at: _,
    } = template;
    TemplateExport {
        name,
        output_folder,
        output_filename_template,
        cookies: None,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        record_danmu,
        danmu_statistics,
        platform_overrides: credential_selections::inject_overrides(
            platform_overrides.map(parse_db_config),
            selections,
        ),
        download_retry_policy: download_retry_policy.map(parse_db_config),
        download_engine,
        extractor,
        engines_override: engines_override.map(parse_db_config),
        proxy_config: None,
        proxy_route: route,
        stream_selection_config: stream_selection_config.map(parse_db_config),
        pipeline: pipeline.map(parse_db_config),
        session_complete_pipeline: session_complete_pipeline.map(parse_db_config),
        paired_segment_pipeline: paired_segment_pipeline.map(parse_db_config),
        offline_check_count,
        offline_check_delay_ms,
    }
}

fn platform_export(
    platform: PlatformConfigDbModel,
    credential_selection: Option<CredentialSelection>,
    route: Option<BackupRoute>,
) -> PlatformExport {
    let PlatformConfigDbModel {
        id: _, // bundles name platforms; IDs stay local
        platform_name,
        fetch_delay_ms,
        download_delay_ms,
        platform_specific_config,
        record_danmu,
        danmu_statistics,
        output_folder,
        output_filename_template,
        download_engine,
        extractor,
        stream_selection_config,
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        download_retry_policy,
        pipeline,
        session_complete_pipeline,
        paired_segment_pipeline,
        offline_check_count,
        offline_check_delay_ms,
    } = platform;
    PlatformExport {
        platform_name,
        fetch_delay_ms,
        download_delay_ms,
        cookies: None,
        credential_selection,
        platform_specific_config: platform_specific_config.map(parse_db_config),
        proxy_config: None,
        proxy_route: route,
        record_danmu,
        danmu_statistics,
        output_folder,
        output_filename_template,
        download_engine,
        extractor,
        stream_selection_config: stream_selection_config.map(parse_db_config),
        output_file_format,
        min_segment_size_bytes,
        max_download_duration_secs,
        max_part_size_bytes,
        download_retry_policy: download_retry_policy.map(parse_db_config),
        pipeline: pipeline.map(parse_db_config),
        session_complete_pipeline: session_complete_pipeline.map(parse_db_config),
        paired_segment_pipeline: paired_segment_pipeline.map(parse_db_config),
        offline_check_count,
        offline_check_delay_ms,
    }
}

fn build_streamer_export(
    streamer: &StreamerDbModel,
    platform_map: &HashMap<String, String>,
    template_map: &HashMap<String, String>,
    filters: Vec<FilterExport>,
) -> StreamerExport {
    let platform_name = platform_map
        .get(&streamer.platform_config_id)
        .cloned()
        .unwrap_or_else(|| streamer.platform_config_id.clone());

    let template_name = streamer
        .template_config_id
        .as_ref()
        .and_then(|id| template_map.get(id).cloned());

    StreamerExport {
        name: streamer.name.clone(),
        url: streamer.url.clone(),
        platform: platform_name,
        template: template_name,
        priority: streamer.priority.clone(),
        state: streamer.state.clone(),
        avatar_url: streamer.avatar.clone(),
        streamer_specific_config: streamer
            .streamer_specific_config
            .clone()
            .map(parse_db_config),
        proxy_route: None,
        filters,
    }
}
async fn export_streamers(
    streamers: &[StreamerDbModel],
    platform_map: &HashMap<String, String>,
    template_map: &HashMap<String, String>,
    repository: &dyn FilterRepository,
) -> crate::Result<Vec<StreamerExport>> {
    let ids: Vec<String> = streamers
        .iter()
        .map(|streamer| streamer.id.clone())
        .collect();
    let mut filters = repository.get_filters_for_streamers(&ids).await?;
    Ok(streamers
        .iter()
        .map(|streamer| {
            let children = filters
                .remove(&streamer.id)
                .unwrap_or_default()
                .into_iter()
                .map(|filter| FilterExport {
                    config: export_filter_config(
                        &filter.filter_type,
                        parse_db_config(filter.config),
                    ),
                    filter_type: filter.filter_type,
                })
                .collect();
            build_streamer_export(streamer, platform_map, template_map, children)
        })
        .collect())
}

async fn export_channels(
    channels: &[NotificationChannelDbModel],
    repository: &dyn NotificationRepository,
) -> crate::Result<Vec<NotificationChannelExport>> {
    let ids: Vec<String> = channels.iter().map(|channel| channel.id.clone()).collect();
    let mut subscriptions = repository.get_subscriptions_for_channels(&ids).await?;
    Ok(channels
        .iter()
        .map(|channel| NotificationChannelExport {
            name: channel.name.clone(),
            channel_type: channel.channel_type.clone(),
            settings: parse_db_config(channel.settings.clone()),
            subscriptions: subscriptions.remove(&channel.id).unwrap_or_default(),
        })
        .collect())
}

pub(crate) struct ConfigurationExportService {
    write_pool: SqlitePool,
    config_service: Arc<RuntimeConfigService>,
    streamer_repository: Arc<dyn StreamerRepository>,
    filter_repository: Arc<dyn FilterRepository>,
    notification_repository: Arc<dyn NotificationRepository>,
    job_preset_repository: Arc<dyn JobPresetRepository>,
    pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
}

impl ConfigurationExportService {
    pub fn new(
        write_pool: SqlitePool,
        config_service: Arc<RuntimeConfigService>,
        streamer_repository: Arc<dyn StreamerRepository>,
        filter_repository: Arc<dyn FilterRepository>,
        notification_repository: Arc<dyn NotificationRepository>,
        job_preset_repository: Arc<dyn JobPresetRepository>,
        pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
    ) -> Self {
        Self {
            write_pool,
            config_service,
            streamer_repository,
            filter_repository,
            notification_repository,
            job_preset_repository,
            pipeline_preset_repository,
        }
    }

    /// Assemble the complete backup. Any read failure fails the export, so a
    /// returned document never silently omits configuration.
    ///
    /// `users` is `None` when authentication is disabled; the backup then
    /// carries no accounts.
    pub async fn export(&self, users: Option<&dyn UserRepository>) -> crate::Result<ConfigExport> {
        let config_service = &self.config_service;

        let user_exports = if let Some(user_repo) = users {
            let total_users = user_repo.count().await?;

            let users = user_repo.list(total_users, 0).await?;

            users
                .into_iter()
                .map(|u: UserDbModel| {
                    let roles = u.get_roles();
                    UserExport {
                        id: u.id,
                        username: u.username,
                        password_hash: u.password_hash,
                        email: u.email,
                        roles,
                        is_active: u.is_active,
                        must_change_password: u.must_change_password,
                        last_login_at: u.last_login_at,
                        created_at: u.created_at,
                        updated_at: u.updated_at,
                    }
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let global_config = config_service.get_global_config().await?;

        let mut templates = config_service.list_template_configs().await?;
        let retired_templates = self.retired_template_ids().await?;
        templates.retain(|template| !retired_templates.contains(&template.id));

        let engines = config_service.list_engine_configs().await?;

        let platforms = config_service.list_platform_configs().await?;

        let streamers = self.streamer_repository.list_streamers().await?;

        let channels = self.notification_repository.list_channels().await?;

        let job_presets = self.job_preset_repository.list_presets().await?;

        let pipeline_presets = self
            .pipeline_preset_repository
            .list_pipeline_presets()
            .await?;

        // Build platform ID to name map for streamer export
        let platform_map: HashMap<String, String> = platforms
            .iter()
            .map(|p| (p.id.clone(), p.platform_name.clone()))
            .collect();

        // Build template ID to name map for streamer export
        let template_map: HashMap<String, String> = templates
            .iter()
            .map(|t| (t.id.clone(), t.name.clone()))
            .collect();

        let mut streamer_exports = export_streamers(
            &streamers,
            &platform_map,
            &template_map,
            self.filter_repository.as_ref(),
        )
        .await?;

        // Backups carry each account selection inside the configuration it belongs to.
        let mut platform_selections = HashMap::new();
        let mut template_selections: HashMap<String, Vec<StoredSelection>> = HashMap::new();
        let mut streamer_selections = HashMap::new();
        for stored in config_service.list_credential_selections().await? {
            match &stored.owner {
                CredentialOwner::Platform { platform_id } => {
                    platform_selections.insert(platform_id.clone(), stored.selection);
                }
                CredentialOwner::Template { template_id } => {
                    template_selections
                        .entry(template_id.clone())
                        .or_default()
                        .push(stored);
                }
                CredentialOwner::Streamer { streamer_id } => {
                    streamer_selections.insert(streamer_id.clone(), stored.selection);
                }
            }
        }
        for (streamer, export) in streamers.iter().zip(streamer_exports.iter_mut()) {
            export.streamer_specific_config = credential_selections::inject_document(
                export.streamer_specific_config.take(),
                streamer_selections.get(&streamer.id),
            );
        }
        let channel_exports =
            export_channels(&channels, self.notification_repository.as_ref()).await?;

        // Backups name proxies; IDs stay local to each installation.
        let proxies =
            crate::database::repositories::proxies::list(&mut *self.write_pool.acquire().await?)
                .await?;
        let proxy_names: HashMap<String, String> = proxies
            .iter()
            .map(|entry| (entry.id.clone(), entry.name.clone()))
            .collect();
        let backup_route =
            |route: &crate::proxies::ProxyRoute| BackupRoute::from_route(route, &proxy_names);
        let platform_routes = config_service
            .proxy_routes_of_kind(&RouteOwner::Platform(String::new()))
            .await?;
        let template_routes = config_service
            .proxy_routes_of_kind(&RouteOwner::Template(String::new()))
            .await?;
        let streamer_routes = config_service
            .proxy_routes_of_kind(&RouteOwner::Streamer(String::new()))
            .await?;
        let global_route =
            backup_route(&config_service.proxy_route_of(&RouteOwner::Global).await?)?;
        let scope_route = |routes: &HashMap<String, crate::proxies::ProxyRoute>, id: &str| {
            routes
                .get(id)
                .map_or(Ok(BackupRoute::Inherit), backup_route)
                .map(Some)
        };
        for (streamer, export) in streamers.iter().zip(streamer_exports.iter_mut()) {
            export.proxy_route = scope_route(&streamer_routes, &streamer.id)?;
        }
        let credential_profiles =
            super::config_import::export_profiles(&self.write_pool, &proxy_names).await?;
        let mut export = ConfigExport {
            version: EXPORT_SCHEMA_VERSION.to_string(),
            exported_at: Utc::now().to_rfc3339(),
            global_config: global_export(global_config, global_route),
            templates: templates
                .into_iter()
                .map(|t| -> crate::Result<TemplateExport> {
                    let route = scope_route(&template_routes, &t.id)?;
                    let selections = template_selections
                        .get(&t.id)
                        .map_or(&[][..], Vec::as_slice);
                    Ok(template_export(t, selections, route))
                })
                .collect::<crate::Result<_>>()?,
            streamers: streamer_exports,
            engines: engines
                .iter()
                .map(|e| EngineExport {
                    name: e.name.clone(),
                    engine_type: e.engine_type.clone(),
                    config: parse_db_config(e.config.clone()),
                })
                .collect(),
            platforms: platforms
                .into_iter()
                .map(|p| -> crate::Result<PlatformExport> {
                    let route = scope_route(&platform_routes, &p.id)?;
                    let selection = platform_selections.get(&p.id).cloned();
                    Ok(platform_export(p, selection, route))
                })
                .collect::<crate::Result<_>>()?,
            notification_channels: channel_exports,
            job_presets: job_presets
                .into_iter()
                .map(|jp| JobPresetExport {
                    name: jp.name,
                    description: jp.description,
                    category: jp.category,
                    processor: jp.processor,
                    config: serde_json::from_str(&jp.config)
                        .unwrap_or(serde_json::Value::String(jp.config)),
                })
                .collect(),
            pipeline_presets: pipeline_presets
                .into_iter()
                .map(|pp| PipelinePresetExport {
                    name: pp.name,
                    description: pp.description,
                    dag_definition: pp.dag_definition.map(parse_db_config),
                    pipeline_type: pp.pipeline_type,
                })
                .collect(),
            users: user_exports,
            credential_profiles,
            proxies: proxies
                .into_iter()
                .map(|entry| ProxyExport {
                    name: entry.name,
                    url: entry.url,
                    username: entry.username,
                    password: entry.password,
                })
                .collect(),
        };
        export.update_schema_version();
        export.validate_credential_graph()?;
        Ok(export)
    }

    async fn retired_template_ids(&self) -> crate::Result<HashSet<String>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT config_id FROM retirement_config_deletions WHERE kind = 'template'",
        )
        .fetch_all(&self.write_pool)
        .await?;
        Ok(ids.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn export_related_data_batches_queries_and_preserves_parent_and_child_order() {
        use crate::database::repositories::{
            SqlxFilterRepository, SqlxNotificationRepository, SqlxStreamerRepository,
            StreamerRepository,
        };
        use std::sync::atomic::Ordering;
        let (pool, queries) = crate::api::batch_lookup::tests::fixture().await;
        let streamer_repo = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let notification_repo = SqlxNotificationRepository::new(pool.clone(), pool.clone());
        let filter_repo = SqlxFilterRepository::new(pool.clone(), pool.clone());
        let mut streamers = streamer_repo.list_streamers().await.unwrap();
        let mut channels = notification_repo.list_channels().await.unwrap();
        streamers.reverse();
        channels.reverse();
        queries.store(0, Ordering::SeqCst);
        let exported = export_streamers(&streamers, &HashMap::new(), &HashMap::new(), &filter_repo)
            .await
            .unwrap();
        assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
        assert_eq!(exported.len(), streamers.len());
        for (export, owner) in exported.iter().zip(&streamers) {
            assert_eq!(export.name, owner.name);
            if owner.id == "owner-1000" {
                assert!(export.filters.is_empty());
            } else {
                assert_eq!(
                    export
                        .filters
                        .iter()
                        .map(|filter| filter.filter_type.as_str())
                        .collect::<Vec<_>>(),
                    ["A", "Z"]
                );
                assert_eq!(export.filters[0].config, serde_json::json!({"value":1}));
                assert_eq!(export.filters[1].config, serde_json::json!({"value":2}));
            }
        }
        let exported = export_channels(&channels, &notification_repo)
            .await
            .unwrap();
        assert_eq!(queries.swap(0, Ordering::SeqCst), 3);
        assert_eq!(exported.len(), channels.len());
        for (export, owner) in exported.iter().zip(&channels) {
            assert_eq!(export.name, owner.name);
            assert_eq!(export.settings, serde_json::json!({"enabled":true}));
            if owner.id == "owner-1000" {
                assert!(export.subscriptions.is_empty());
            } else {
                assert_eq!(export.subscriptions, ["AEvent", "ZEvent"]);
            }
        }
        assert!(
            export_streamers(&[], &HashMap::new(), &HashMap::new(), &filter_repo)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            export_channels(&[], &notification_repo)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(queries.load(Ordering::SeqCst), 0);
        pool.close().await;
    }

    #[tokio::test]
    async fn filter_exports_materialize_utc_without_losing_explicit_zones_or_extensions() {
        use crate::database::models::{FilterDbModel, FilterType};
        use crate::database::repositories::{
            SqlxFilterRepository, SqlxStreamerRepository, StreamerRepository,
        };
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        let streamers = SqlxStreamerRepository::new(pool.clone(), pool.clone());
        let filters = SqlxFilterRepository::new(pool.clone(), pool.clone());
        let owner = StreamerDbModel::new(
            "Export timezone",
            "https://example.test/timezone",
            "platform-huya",
        );
        streamers.create_streamer(&owner).await.unwrap();
        for zone in [None, Some("local"), Some("Europe/Madrid")] {
            let config = serde_json::json!({"days_of_week":["Monday"],"start_time":"09:00","end_time":"17:00","timezone":zone,"extension":[1,true]});
            filters
                .create_filter(&FilterDbModel::new(
                    &owner.id,
                    FilterType::TimeBased,
                    config.to_string(),
                ))
                .await
                .unwrap();
        }
        filters
            .create_filter(&FilterDbModel::new(
                &owner.id,
                FilterType::Cron,
                serde_json::json!({"expression":"0 * * * * *","extension":[1,true]}).to_string(),
            ))
            .await
            .unwrap();
        let exported = export_streamers(
            std::slice::from_ref(&owner),
            &HashMap::new(),
            &HashMap::new(),
            &filters,
        )
        .await
        .unwrap();
        assert_eq!(exported[0].filters.len(), 4);
        let mut zones = Vec::new();
        for filter in &exported[0].filters {
            assert_eq!(filter.config["extension"], serde_json::json!([1, true]));
            zones.push(filter.config["timezone"].as_str().unwrap().to_owned());
        }
        zones.sort();
        assert_eq!(zones, vec!["Europe/Madrid", "UTC", "UTC", "local"]);
        assert_eq!(EXPORT_SCHEMA_VERSION, "0.1.8");
    }
}
