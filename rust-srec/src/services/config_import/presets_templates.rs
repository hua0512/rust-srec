//! Template, job preset, and pipeline preset import persistence and retirement.

use std::collections::{HashMap, HashSet};

use super::snapshot::ImportSnapshot;
use super::{ConfigurationImportError, db_json, proxies, validation_error, write_error};
use crate::config::backup::{ConfigExport, ImportStats, TemplateExport};
use crate::credentials::CredentialOwner;
use crate::database::models::{JobPreset, PipelinePreset, TemplateConfigDbModel};
use crate::database::repositories::config::import_template as persist_template;
use crate::database::repositories::config_retirement::{RetiredConfigKind, delete_or_defer};
use crate::database::repositories::credential_selections;
use crate::database::repositories::preset::{
    import_job_preset as persist_job_preset, import_pipeline_preset as persist_pipeline_preset,
};
use crate::database::repositories::proxies::RouteOwner;

pub(super) async fn apply_templates(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    imported_proxies: &proxies::ImportedProxies,
    stats: &mut ImportStats,
) -> Result<HashMap<String, String>, ConfigurationImportError> {
    let mut template_ids: HashMap<String, String> = if replace {
        HashMap::new()
    } else {
        snapshot
            .templates
            .iter()
            .map(|(name, model)| (name.clone(), model.id.clone()))
            .collect()
    };
    for item in &config.templates {
        let existing = snapshot.templates.get(&item.name);
        let model = template_model(existing, item);
        crate::database::repositories::credential_profiles::reject_template_authentication(&model)
            .map_err(write_error)?;
        if replace && let Some(existing) = existing {
            credential_selections::clear_owner(
                tx,
                &CredentialOwner::Template {
                    template_id: existing.id.clone(),
                },
            )
            .await
            .map_err(write_error)?;
        }
        persist_template(tx, &model).await.map_err(write_error)?;
        imported_proxies
            .write_route(
                tx,
                &RouteOwner::Template(model.id.clone()),
                item.proxy_route.as_ref(),
                replace,
            )
            .await?;
        template_ids.insert(model.name.clone(), model.id.clone());
        if existing.is_some() {
            stats.templates_updated += 1;
        } else {
            stats.templates_created += 1;
        }
    }
    Ok(template_ids)
}

pub(super) async fn delete_unimported_templates(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    replace: bool,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    if !replace {
        return Ok(());
    }
    let imported_templates: HashSet<&str> = config
        .templates
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    for (name, existing) in &snapshot.templates {
        if !imported_templates.contains(name.as_str()) {
            delete_or_defer(tx, RetiredConfigKind::Template, &existing.id).await?;
            stats.templates_deleted += 1;
        }
    }
    Ok(())
}

fn template_model(
    existing: Option<&TemplateConfigDbModel>,
    source: &TemplateExport,
) -> TemplateConfigDbModel {
    let TemplateExport {
        name,
        output_folder,
        output_filename_template,
        cookies: _,
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
        proxy_config: _,
        proxy_route: _,
        stream_selection_config,
        pipeline,
        session_complete_pipeline,
        paired_segment_pipeline,
        offline_check_count,
        offline_check_delay_ms,
    } = source;
    let mut model = existing
        .cloned()
        .unwrap_or_else(|| TemplateConfigDbModel::new(name));
    model.name = name.clone();
    model.output_folder = output_folder.clone();
    model.output_filename_template = output_filename_template.clone();
    model.output_file_format = output_file_format.clone();
    model.min_segment_size_bytes = *min_segment_size_bytes;
    model.max_download_duration_secs = *max_download_duration_secs;
    model.max_part_size_bytes = *max_part_size_bytes;
    model.record_danmu = *record_danmu;
    model.danmu_statistics = danmu_statistics.clone();
    model.platform_overrides = platform_overrides.clone().map(db_json);
    model.download_retry_policy = download_retry_policy.clone().map(db_json);
    model.download_engine = download_engine.clone();
    model.extractor = extractor.clone();
    model.engines_override = engines_override.clone().map(db_json);
    model.stream_selection_config = stream_selection_config.clone().map(db_json);
    model.pipeline = pipeline.clone().map(db_json);
    model.session_complete_pipeline = session_complete_pipeline.clone().map(db_json);
    model.paired_segment_pipeline = paired_segment_pipeline.clone().map(db_json);
    model.offline_check_count = *offline_check_count;
    model.offline_check_delay_ms = *offline_check_delay_ms;
    model.updated_at = jiff::Timestamp::now();
    model
}

pub(super) async fn apply_job_presets(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.job_presets {
        let existing = snapshot.job_presets.get(&item.name);
        let mut model = existing
            .cloned()
            .unwrap_or_else(|| JobPreset::new(&item.name, &item.processor, item.config.clone()));
        model.name = item.name.clone();
        model.description = item.description.clone();
        model.category = item.category.clone();
        model.processor = item.processor.clone();
        model.config = crate::config::backup::unwrap_json_value(item.config.clone()).to_string();
        model.updated_at = jiff::Timestamp::now();
        persist_job_preset(tx, &model).await.map_err(write_error)?;
        if existing.is_some() {
            stats.job_presets_updated += 1;
        } else {
            stats.job_presets_created += 1;
        }
    }
    Ok(())
}

pub(super) async fn apply_pipeline_presets(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    stats: &mut ImportStats,
) -> Result<(), ConfigurationImportError> {
    for item in &config.pipeline_presets {
        let existing = snapshot.pipeline_presets.get(&item.name);
        let dag_value = item.dag_definition.clone().ok_or_else(|| {
            validation_error(format!(
                "Missing dag_definition for pipeline preset '{}'",
                item.name
            ))
        })?;
        let raw_dag = crate::config::backup::unwrap_json_value(dag_value);
        let dag: crate::database::models::job::DagPipelineDefinition =
            serde_json::from_value(raw_dag.clone()).map_err(|error| {
                validation_error(format!(
                    "Invalid dag_definition for pipeline preset '{}': {}",
                    item.name, error
                ))
            })?;
        let mut model = existing
            .cloned()
            .unwrap_or_else(|| PipelinePreset::new(&item.name, dag));
        model.name = item.name.clone();
        model.description = item.description.clone();
        if existing.is_some() {
            model.dag_definition = Some(raw_dag.to_string());
        }
        model.pipeline_type = item
            .pipeline_type
            .clone()
            .or_else(|| Some("dag".to_string()));
        model.updated_at = jiff::Timestamp::now();
        persist_pipeline_preset(tx, &model)
            .await
            .map_err(write_error)?;
        if existing.is_some() {
            stats.pipeline_presets_updated += 1;
        } else {
            stats.pipeline_presets_created += 1;
        }
    }
    Ok(())
}

pub(super) async fn delete_omitted_presets(
    tx: &mut sqlx::SqliteConnection,
    snapshot: &ImportSnapshot,
    config: &ConfigExport,
    stats: &mut ImportStats,
) -> Result<Vec<String>, ConfigurationImportError> {
    use crate::database::repositories::preset::{Owners, StepRef, step_users, summarize_users};

    let imported_presets: HashSet<&str> = config
        .job_presets
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    let imported_workflows: HashSet<&str> = config
        .pipeline_presets
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    let mut omitted: Vec<(StepRef, &str, &str)> = snapshot
        .pipeline_presets
        .iter()
        .filter(|(name, _)| !imported_workflows.contains(name.as_str()))
        .map(|(name, model)| (StepRef::Workflow, name.as_str(), model.id.as_str()))
        .chain(
            snapshot
                .job_presets
                .iter()
                .filter(|(name, _)| !imported_presets.contains(name.as_str()))
                .map(|(name, model)| (StepRef::JobPreset, name.as_str(), model.id.as_str())),
        )
        .collect();
    loop {
        let before = omitted.len();
        let mut still_used = Vec::new();
        for (step, name, id) in omitted.drain(..) {
            let users = step_users(tx, step, id, name, Owners::Kept).await?;
            if !users.is_empty() {
                still_used.push((step, name, id, users));
                continue;
            }
            delete_or_defer(tx, step.into(), id).await?;
            match step {
                StepRef::JobPreset => stats.job_presets_deleted += 1,
                StepRef::Workflow => stats.pipeline_presets_deleted += 1,
            }
        }
        if still_used.len() == before {
            return Ok(still_used
                .into_iter()
                .map(|(step, name, _, users)| {
                    format!(
                        "kept {step} '{name}' because the imported configuration still uses it: {}",
                        summarize_users(&users)
                    )
                })
                .collect());
        }
        omitted = still_used
            .into_iter()
            .map(|(step, name, id, _)| (step, name, id))
            .collect();
    }
}
