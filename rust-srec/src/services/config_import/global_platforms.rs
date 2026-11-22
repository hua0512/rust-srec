//! Global, engine, and platform configuration import persistence.

use std::collections::HashSet;

use super::snapshot::ImportSnapshot;
use super::{ConfigurationImportError, db_json, proxies, validation_error, write_error};
use crate::config::backup::{ConfigExport, GlobalConfigExport, ImportStats, PlatformExport};
use crate::credentials::CredentialSelection;
use crate::database::models::{
    EngineConfigurationDbModel, EngineType, GlobalConfigDbModel, PlatformConfigDbModel,
};
use crate::database::repositories::config::{
    import_engine as persist_engine, import_platform as persist_platform,
};
use crate::database::repositories::proxies::RouteOwner;

pub(super) async fn apply_engines(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.engines {
        let model = if let Some(existing) = snapshot.engines.get(&item.name) {
            stats.engines_updated += 1;
            EngineConfigurationDbModel {
                id: existing.id.clone(),
                name: item.name.clone(),
                engine_type: item.engine_type.clone(),
                config: db_json(item.config.clone()),
            }
        } else {
            stats.engines_created += 1;
            EngineConfigurationDbModel::new(
                &item.name,
                EngineType::parse(&item.engine_type).ok_or_else(|| {
                    validation_error(format!("Invalid engine type '{}'", item.engine_type))
                })?,
                db_json(item.config.clone()),
            )
        };
        persist_engine(tx, &model).await.map_err(write_error)?;
    }
    if replace {
        let imported: HashSet<&str> = config
            .engines
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        for (name, existing) in &snapshot.engines {
            if !imported.contains(name.as_str()) {
                crate::database::repositories::config::delete_engine(tx, &existing.id).await?;
                stats.engines_deleted += 1;
            }
        }
    }
    Ok(())
}

/// Updates the `platform_config` rows named by the bundle, and in replace mode
/// resets omitted built-in platform rows back to clean `None` overrides,
/// `CredentialSelection::Inherit`, and `ProxyRoute::Inherit`.
pub(super) async fn apply_platforms(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    imported_proxies: &proxies::ImportedProxies,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    let mut imported_names: HashSet<&str> = HashSet::with_capacity(config.platforms.len());
    for item in &config.platforms {
        let existing = snapshot.platforms.get(&item.platform_name).ok_or_else(|| {
            validation_error(format!("Unknown platform '{}'", item.platform_name))
        })?;
        let model = platform_model(existing, item);
        let selection = match &item.credential_selection {
            Some(selection) => Some(selection.clone()),
            None if replace => Some(CredentialSelection::Inherit),
            None => None,
        };
        crate::database::repositories::credential_profiles::reject_platform_authentication(&model)
            .map_err(write_error)?;
        persist_platform(tx, &model, selection.as_ref())
            .await
            .map_err(write_error)?;
        imported_proxies
            .write_route(
                tx,
                &RouteOwner::Platform(model.id.clone()),
                item.proxy_route.as_ref(),
                replace,
            )
            .await?;
        imported_names.insert(item.platform_name.as_str());
        stats.platforms_updated += 1;
    }

    if replace {
        let has_default_streamlink = config
            .engines
            .iter()
            .any(|item| item.name == "default-streamlink");
        for (platform_name, existing) in &snapshot.platforms {
            if imported_names.contains(platform_name.as_str()) {
                continue;
            }
            let reset = reset_platform_model(existing, has_default_streamlink);
            persist_platform(tx, &reset, Some(&CredentialSelection::Inherit))
                .await
                .map_err(write_error)?;
            imported_proxies
                .write_route(tx, &RouteOwner::Platform(reset.id), None, true)
                .await?;
        }
    }

    Ok(())
}

pub(super) fn global_model(
    existing: &GlobalConfigDbModel,
    config: &ConfigExport,
) -> GlobalConfigDbModel {
    let GlobalConfigExport {
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
        proxy_config: _, // converted to a route before import
        proxy_route: _,  // written by the caller
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
    } = &config.global_config;
    let mut model = existing.clone();
    model.output_folder = output_folder.clone();
    model.output_filename_template = output_filename_template.clone();
    model.output_file_format = output_file_format.clone();
    model.min_segment_size_bytes = *min_segment_size_bytes;
    model.max_download_duration_secs = *max_download_duration_secs;
    model.max_part_size_bytes = *max_part_size_bytes;
    model.record_danmu = *record_danmu;
    model.danmu_statistics = danmu_statistics.clone();
    model.max_concurrent_downloads = *max_concurrent_downloads;
    model.max_concurrent_uploads = *max_concurrent_uploads;
    model.streamer_check_delay_ms = *streamer_check_delay_ms;
    model.offline_check_delay_ms = *offline_check_delay_ms;
    model.offline_check_count = *offline_check_count;
    model.default_download_engine = default_download_engine.clone();
    model.default_extractor = default_extractor.clone();
    model.max_concurrent_cpu_jobs = *max_concurrent_cpu_jobs;
    model.max_concurrent_io_jobs = *max_concurrent_io_jobs;
    model.job_history_retention_days = *job_history_retention_days;
    model.notification_event_log_retention_days = *notification_event_log_retention_days;
    model.output_retention_days = *output_retention_days;
    model.output_retention_delete_files = *output_retention_delete_files;
    model.pipeline = pipeline.clone().map(db_json);
    model.session_complete_pipeline = session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = paired_segment_pipeline.clone().map(db_json);
    if let Some(log_filter) = log_filter_directive {
        model.log_filter_directive = log_filter.clone();
    }
    model.auto_thumbnail = *auto_thumbnail;
    model.pipeline_cpu_job_timeout_secs = *pipeline_cpu_job_timeout_secs;
    model.pipeline_io_job_timeout_secs = *pipeline_io_job_timeout_secs;
    model.pipeline_execute_timeout_secs = *pipeline_execute_timeout_secs;
    model.queue_freshness_threshold_ms = *queue_freshness_threshold_ms;
    model.gpu_health_probe_interval_secs = *gpu_health_probe_interval_secs;
    model.stream_proxy_allow_private_targets = *stream_proxy_allow_private_targets;
    model
}

fn platform_model(
    existing: &PlatformConfigDbModel,
    source: &PlatformExport,
) -> PlatformConfigDbModel {
    let PlatformExport {
        platform_name: _,
        fetch_delay_ms,
        download_delay_ms,
        cookies: _,
        credential_selection: _,
        platform_specific_config,
        proxy_config: _,
        proxy_route: _,
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
    } = source;
    let mut model = existing.clone();
    model.fetch_delay_ms = *fetch_delay_ms;
    model.download_delay_ms = *download_delay_ms;
    model.platform_specific_config = platform_specific_config.clone().map(db_json);
    model.record_danmu = *record_danmu;
    model.danmu_statistics = danmu_statistics.clone();
    model.output_folder = output_folder.clone();
    model.output_filename_template = output_filename_template.clone();
    model.download_engine = download_engine.clone();
    model.extractor = extractor.clone();
    model.stream_selection_config = stream_selection_config.clone().map(db_json);
    model.output_file_format = output_file_format.clone();
    model.min_segment_size_bytes = *min_segment_size_bytes;
    model.max_download_duration_secs = *max_download_duration_secs;
    model.max_part_size_bytes = *max_part_size_bytes;
    model.download_retry_policy = download_retry_policy.clone().map(db_json);
    model.pipeline = pipeline.clone().map(db_json);
    model.session_complete_pipeline = session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = paired_segment_pipeline.clone().map(db_json);
    model.offline_check_count = *offline_check_count;
    model.offline_check_delay_ms = *offline_check_delay_ms;
    model
}

fn reset_platform_model(
    existing: &PlatformConfigDbModel,
    has_default_streamlink: bool,
) -> PlatformConfigDbModel {
    let is_streamlink = crate::domain::is_streamlink_platform(&existing.platform_name);
    PlatformConfigDbModel {
        id: existing.id.clone(),
        platform_name: existing.platform_name.clone(),
        fetch_delay_ms: None,
        download_delay_ms: None,
        platform_specific_config: is_streamlink.then(|| "{\"streamlink\":{}}".to_owned()),
        record_danmu: None,
        danmu_statistics: None,
        output_folder: None,
        output_filename_template: None,
        download_engine: (is_streamlink && has_default_streamlink)
            .then(|| "default-streamlink".to_owned()),
        extractor: None,
        stream_selection_config: None,
        output_file_format: None,
        min_segment_size_bytes: None,
        max_download_duration_secs: None,
        max_part_size_bytes: None,
        download_retry_policy: None,
        pipeline: None,
        session_complete_pipeline: None,
        paired_segment_pipeline: None,
        offline_check_count: None,
        offline_check_delay_ms: None,
    }
}
