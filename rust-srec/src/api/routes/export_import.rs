//! Configuration export and import routes.
//!
//! Provides endpoints to backup and restore the entire system configuration.

use axum::{
    Json, Router,
    extract::State,
    http::header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::models::{
    EngineConfigurationDbModel, FilterDbModel, NotificationChannelDbModel, PlatformConfigDbModel,
    StreamerDbModel, TemplateConfigDbModel,
};

/// Current schema version for exports.
const EXPORT_SCHEMA_VERSION: &str = "0.1.0";

/// Create the export/import router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export", get(export_config))
        .route("/import", post(import_config))
}

// ============================================================================
// Export Data Models
// ============================================================================

/// Complete configuration export bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigExport {
    /// Schema version for compatibility checking.
    pub version: String,
    /// ISO 8601 timestamp of export.
    pub exported_at: String,
    /// Global configuration.
    pub global_config: GlobalConfigExport,
    /// All templates.
    pub templates: Vec<TemplateExport>,
    /// All streamers with their filters.
    pub streamers: Vec<StreamerExport>,
    /// All engine configurations.
    pub engines: Vec<EngineExport>,
    /// All platform configurations.
    pub platforms: Vec<PlatformExport>,
    /// All notification channels with subscriptions.
    pub notification_channels: Vec<NotificationChannelExport>,
}

/// Global configuration for export (excludes internal ID).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfigExport {
    pub output_folder: String,
    pub output_filename_template: String,
    pub output_file_format: String,
    pub min_segment_size_bytes: i64,
    pub max_download_duration_secs: i64,
    pub max_part_size_bytes: i64,
    pub record_danmu: bool,
    pub max_concurrent_downloads: i32,
    pub max_concurrent_uploads: i32,
    pub streamer_check_delay_ms: i64,
    pub proxy_config: String,
    pub offline_check_delay_ms: i64,
    pub offline_check_count: i32,
    pub default_download_engine: String,
    pub max_concurrent_cpu_jobs: i32,
    pub max_concurrent_io_jobs: i32,
    pub job_history_retention_days: i32,
    pub session_gap_time_secs: i64,
    pub pipeline: Option<String>,
    pub log_filter_directive: Option<String>,
}

/// Template for export (uses name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateExport {
    pub name: String,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub cookies: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub record_danmu: Option<bool>,
    pub platform_overrides: Option<String>,
    pub download_retry_policy: Option<String>,
    pub danmu_sampling_config: Option<String>,
    pub download_engine: Option<String>,
    pub engines_override: Option<String>,
    pub proxy_config: Option<String>,
    pub event_hooks: Option<String>,
    pub stream_selection_config: Option<String>,
    pub pipeline: Option<String>,
}

/// Streamer for export (uses URL as identifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerExport {
    pub name: String,
    pub url: String,
    /// Platform name (resolved from platform_config_id).
    pub platform: String,
    /// Template name (resolved from template_config_id).
    pub template: Option<String>,
    pub priority: String,
    pub streamer_specific_config: Option<String>,
    /// Associated filters.
    pub filters: Vec<FilterExport>,
}

/// Filter for export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterExport {
    pub filter_type: String,
    pub config: String,
}

/// Engine configuration for export (uses name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineExport {
    pub name: String,
    pub engine_type: String,
    pub config: String,
}

/// Platform configuration for export (uses platform_name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformExport {
    pub platform_name: String,
    pub fetch_delay_ms: Option<i64>,
    pub download_delay_ms: Option<i64>,
    pub cookies: Option<String>,
    pub platform_specific_config: Option<String>,
    pub proxy_config: Option<String>,
    pub record_danmu: Option<bool>,
    pub output_folder: Option<String>,
    pub output_filename_template: Option<String>,
    pub download_engine: Option<String>,
    pub stream_selection_config: Option<String>,
    pub output_file_format: Option<String>,
    pub min_segment_size_bytes: Option<i64>,
    pub max_download_duration_secs: Option<i64>,
    pub max_part_size_bytes: Option<i64>,
    pub download_retry_policy: Option<String>,
    pub event_hooks: Option<String>,
    pub pipeline: Option<String>,
}

/// Notification channel for export (uses name as identifier).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationChannelExport {
    pub name: String,
    pub channel_type: String,
    pub settings: String,
    pub subscriptions: Vec<String>,
}

// ============================================================================
// Import Data Models
// ============================================================================

/// Import mode.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Merge: Update existing entities, add new ones, keep entities not in import.
    #[default]
    Merge,
    /// Replace: Delete all existing entities and import fresh.
    Replace,
}

/// Import request with optional mode.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportRequest {
    /// The configuration data to import.
    pub config: ConfigExport,
    /// Import mode: "merge" (default) or "replace".
    #[serde(default)]
    pub mode: ImportMode,
}

/// Import result.
#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub success: bool,
    pub message: String,
    pub stats: ImportStats,
}

/// Statistics about what was imported.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ImportStats {
    pub templates_created: u32,
    pub templates_updated: u32,
    pub templates_deleted: u32,
    pub streamers_created: u32,
    pub streamers_updated: u32,
    pub streamers_deleted: u32,
    pub engines_created: u32,
    pub engines_updated: u32,
    pub engines_deleted: u32,
    pub platforms_updated: u32,
    pub channels_created: u32,
    pub channels_updated: u32,
    pub channels_deleted: u32,
}

// ============================================================================
// Export Handler
// ============================================================================

/// Export the entire configuration.
///
/// GET /api/config/backup/export
async fn export_config(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    let streamer_manager = state
        .streamer_manager
        .as_ref()
        .ok_or_else(|| ApiError::internal("StreamerManager not available"))?;

    let notification_repo = state
        .notification_repository
        .as_ref()
        .ok_or_else(|| ApiError::internal("NotificationRepository not available"))?;

    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::internal("FilterRepository not available"))?;

    // Collect all data
    let global_config = config_service
        .get_global_config()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get global config: {}", e)))?;

    let templates = config_service
        .list_template_configs()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list templates: {}", e)))?;

    let engines = config_service
        .list_engine_configs()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list engines: {}", e)))?;

    let platforms = config_service
        .list_platform_configs()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list platforms: {}", e)))?;

    // Get streamers from manager and convert to list
    let streamer_metadata_list = streamer_manager.get_all();

    let channels = notification_repo
        .list_channels()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list notification channels: {}", e)))?;

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

    // Export streamers with filters
    let mut streamer_exports = Vec::new();
    for streamer in &streamer_metadata_list {
        let filters = filter_repo
            .get_filters_for_streamer(&streamer.id)
            .await
            .unwrap_or_default();

        let filter_exports: Vec<FilterExport> = filters
            .iter()
            .map(|f| FilterExport {
                filter_type: f.filter_type.clone(),
                config: f.config.clone(),
            })
            .collect();

        let platform_name = platform_map
            .get(&streamer.platform_config_id)
            .cloned()
            .unwrap_or_else(|| streamer.platform_config_id.clone());

        let template_name = streamer
            .template_config_id
            .as_ref()
            .and_then(|id| template_map.get(id).cloned());

        streamer_exports.push(StreamerExport {
            name: streamer.name.clone(),
            url: streamer.url.clone(),
            platform: platform_name,
            template: template_name,
            priority: streamer.priority.as_str().to_string(),
            streamer_specific_config: streamer.streamer_specific_config.clone(),
            filters: filter_exports,
        });
    }

    // Export notification channels with subscriptions
    let mut channel_exports = Vec::new();
    for channel in &channels {
        let subscriptions = notification_repo
            .get_subscriptions_for_channel(&channel.id)
            .await
            .unwrap_or_default();

        channel_exports.push(NotificationChannelExport {
            name: channel.name.clone(),
            channel_type: channel.channel_type.clone(),
            settings: channel.settings.clone(),
            subscriptions,
        });
    }

    let export = ConfigExport {
        version: EXPORT_SCHEMA_VERSION.to_string(),
        exported_at: Utc::now().to_rfc3339(),
        global_config: GlobalConfigExport {
            output_folder: global_config.output_folder,
            output_filename_template: global_config.output_filename_template,
            output_file_format: global_config.output_file_format,
            min_segment_size_bytes: global_config.min_segment_size_bytes,
            max_download_duration_secs: global_config.max_download_duration_secs,
            max_part_size_bytes: global_config.max_part_size_bytes,
            record_danmu: global_config.record_danmu,
            max_concurrent_downloads: global_config.max_concurrent_downloads,
            max_concurrent_uploads: global_config.max_concurrent_uploads,
            streamer_check_delay_ms: global_config.streamer_check_delay_ms,
            proxy_config: global_config.proxy_config,
            offline_check_delay_ms: global_config.offline_check_delay_ms,
            offline_check_count: global_config.offline_check_count,
            default_download_engine: global_config.default_download_engine,
            max_concurrent_cpu_jobs: global_config.max_concurrent_cpu_jobs,
            max_concurrent_io_jobs: global_config.max_concurrent_io_jobs,
            job_history_retention_days: global_config.job_history_retention_days,
            session_gap_time_secs: global_config.session_gap_time_secs,
            pipeline: global_config.pipeline,
            log_filter_directive: Some(global_config.log_filter_directive),
        },
        templates: templates
            .iter()
            .map(|t| TemplateExport {
                name: t.name.clone(),
                output_folder: t.output_folder.clone(),
                output_filename_template: t.output_filename_template.clone(),
                cookies: t.cookies.clone(),
                output_file_format: t.output_file_format.clone(),
                min_segment_size_bytes: t.min_segment_size_bytes,
                max_download_duration_secs: t.max_download_duration_secs,
                max_part_size_bytes: t.max_part_size_bytes,
                record_danmu: t.record_danmu,
                platform_overrides: t.platform_overrides.clone(),
                download_retry_policy: t.download_retry_policy.clone(),
                danmu_sampling_config: t.danmu_sampling_config.clone(),
                download_engine: t.download_engine.clone(),
                engines_override: t.engines_override.clone(),
                proxy_config: t.proxy_config.clone(),
                event_hooks: t.event_hooks.clone(),
                stream_selection_config: t.stream_selection_config.clone(),
                pipeline: t.pipeline.clone(),
            })
            .collect(),
        streamers: streamer_exports,
        engines: engines
            .iter()
            .map(|e| EngineExport {
                name: e.name.clone(),
                engine_type: e.engine_type.clone(),
                config: e.config.clone(),
            })
            .collect(),
        platforms: platforms
            .iter()
            .map(|p| PlatformExport {
                platform_name: p.platform_name.clone(),
                fetch_delay_ms: p.fetch_delay_ms,
                download_delay_ms: p.download_delay_ms,
                cookies: p.cookies.clone(),
                platform_specific_config: p.platform_specific_config.clone(),
                proxy_config: p.proxy_config.clone(),
                record_danmu: p.record_danmu,
                output_folder: p.output_folder.clone(),
                output_filename_template: p.output_filename_template.clone(),
                download_engine: p.download_engine.clone(),
                stream_selection_config: p.stream_selection_config.clone(),
                output_file_format: p.output_file_format.clone(),
                min_segment_size_bytes: p.min_segment_size_bytes,
                max_download_duration_secs: p.max_download_duration_secs,
                max_part_size_bytes: p.max_part_size_bytes,
                download_retry_policy: p.download_retry_policy.clone(),
                event_hooks: p.event_hooks.clone(),
                pipeline: p.pipeline.clone(),
            })
            .collect(),
        notification_channels: channel_exports,
    };

    let json = serde_json::to_string_pretty(&export)
        .map_err(|e| ApiError::internal(format!("Failed to serialize export: {}", e)))?;

    let filename = format!(
        "rust-srec-backup-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let content_disposition = format!("attachment; filename=\"{}\"", filename);

    Ok((
        [
            (CONTENT_TYPE, "application/json".to_string()),
            (CONTENT_DISPOSITION, content_disposition),
        ],
        json,
    ))
}

// ============================================================================
// Import Handler
// ============================================================================

/// Import configuration with merge or replace mode.
///
/// POST /api/config/backup/import
///
/// Modes:
/// - `merge` (default): Update existing entities, add new ones, keep entities not in import
/// - `replace`: Delete all existing entities and import fresh
async fn import_config(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> ApiResult<Json<ImportResult>> {
    let config = request.config;
    let mode = request.mode;

    // Validate schema version
    if !config.version.starts_with("0.") {
        return Err(ApiError::bad_request(format!(
            "Unsupported schema version: {}. Expected 0.x",
            config.version
        )));
    }

    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::internal("ConfigService not available"))?;

    let streamer_repo = state
        .streamer_repository
        .as_ref()
        .ok_or_else(|| ApiError::internal("StreamerRepository not available"))?;

    let notification_repo = state
        .notification_repository
        .as_ref()
        .ok_or_else(|| ApiError::internal("NotificationRepository not available"))?;

    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::internal("FilterRepository not available"))?;

    let mut stats = ImportStats::default();
    let is_replace = mode == ImportMode::Replace;

    // 1. Update global config
    let mut global = config_service
        .get_global_config()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to get global config: {}", e)))?;

    global.output_folder = config.global_config.output_folder;
    global.output_filename_template = config.global_config.output_filename_template;
    global.output_file_format = config.global_config.output_file_format;
    global.min_segment_size_bytes = config.global_config.min_segment_size_bytes;
    global.max_download_duration_secs = config.global_config.max_download_duration_secs;
    global.max_part_size_bytes = config.global_config.max_part_size_bytes;
    global.record_danmu = config.global_config.record_danmu;
    global.max_concurrent_downloads = config.global_config.max_concurrent_downloads;
    global.max_concurrent_uploads = config.global_config.max_concurrent_uploads;
    global.streamer_check_delay_ms = config.global_config.streamer_check_delay_ms;
    global.proxy_config = config.global_config.proxy_config;
    global.offline_check_delay_ms = config.global_config.offline_check_delay_ms;
    global.offline_check_count = config.global_config.offline_check_count;
    global.default_download_engine = config.global_config.default_download_engine;
    global.max_concurrent_cpu_jobs = config.global_config.max_concurrent_cpu_jobs;
    global.max_concurrent_io_jobs = config.global_config.max_concurrent_io_jobs;
    global.job_history_retention_days = config.global_config.job_history_retention_days;
    global.session_gap_time_secs = config.global_config.session_gap_time_secs;
    global.pipeline = config.global_config.pipeline;
    if let Some(log_filter) = config.global_config.log_filter_directive {
        global.log_filter_directive = log_filter;
    }

    config_service
        .update_global_config(&global)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update global config: {}", e)))?;

    // 2. Import engines (by name)
    let existing_engines = config_service
        .list_engine_configs()
        .await
        .unwrap_or_default();
    let engine_name_map: HashMap<String, EngineConfigurationDbModel> = existing_engines
        .into_iter()
        .map(|e| (e.name.clone(), e))
        .collect();

    for engine_export in &config.engines {
        if let Some(existing) = engine_name_map.get(&engine_export.name) {
            // Update existing
            let mut updated = existing.clone();
            updated.engine_type = engine_export.engine_type.clone();
            updated.config = engine_export.config.clone();
            config_service
                .update_engine_config(&updated)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to update engine: {}", e)))?;
            stats.engines_updated += 1;
        } else {
            // Create new
            let new_engine = EngineConfigurationDbModel::new(
                engine_export.name.clone(),
                crate::database::models::EngineType::parse(&engine_export.engine_type)
                    .unwrap_or(crate::database::models::EngineType::Mesio),
                engine_export.config.clone(),
            );
            config_service
                .create_engine_config(&new_engine)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to create engine: {}", e)))?;
            stats.engines_created += 1;
        }
    }

    // In replace mode, delete engines not in the import
    if is_replace {
        let imported_engine_names: std::collections::HashSet<&str> =
            config.engines.iter().map(|e| e.name.as_str()).collect();
        for (name, engine) in &engine_name_map {
            if !imported_engine_names.contains(name.as_str()) {
                if config_service
                    .delete_engine_config(&engine.id)
                    .await
                    .is_ok()
                {
                    stats.engines_deleted += 1;
                }
            }
        }
    }

    // 3. Import templates (by name)
    let existing_templates = config_service
        .list_template_configs()
        .await
        .unwrap_or_default();
    let template_name_map: HashMap<String, TemplateConfigDbModel> = existing_templates
        .into_iter()
        .map(|t| (t.name.clone(), t))
        .collect();

    // Build new template name -> id map for streamer import
    let mut new_template_name_to_id: HashMap<String, String> = HashMap::new();

    for template_export in &config.templates {
        if let Some(existing) = template_name_map.get(&template_export.name) {
            // Update existing
            let mut updated = existing.clone();
            updated.output_folder = template_export.output_folder.clone();
            updated.output_filename_template = template_export.output_filename_template.clone();
            updated.cookies = template_export.cookies.clone();
            updated.output_file_format = template_export.output_file_format.clone();
            updated.min_segment_size_bytes = template_export.min_segment_size_bytes;
            updated.max_download_duration_secs = template_export.max_download_duration_secs;
            updated.max_part_size_bytes = template_export.max_part_size_bytes;
            updated.record_danmu = template_export.record_danmu;
            updated.platform_overrides = template_export.platform_overrides.clone();
            updated.download_retry_policy = template_export.download_retry_policy.clone();
            updated.danmu_sampling_config = template_export.danmu_sampling_config.clone();
            updated.download_engine = template_export.download_engine.clone();
            updated.engines_override = template_export.engines_override.clone();
            updated.proxy_config = template_export.proxy_config.clone();
            updated.event_hooks = template_export.event_hooks.clone();
            updated.stream_selection_config = template_export.stream_selection_config.clone();
            updated.pipeline = template_export.pipeline.clone();
            updated.updated_at = Utc::now();

            config_service
                .update_template_config(&updated)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to update template: {}", e)))?;
            new_template_name_to_id.insert(updated.name.clone(), updated.id.clone());
            stats.templates_updated += 1;
        } else {
            // Create new
            let mut new_template = TemplateConfigDbModel::new(&template_export.name);
            new_template.output_folder = template_export.output_folder.clone();
            new_template.output_filename_template =
                template_export.output_filename_template.clone();
            new_template.cookies = template_export.cookies.clone();
            new_template.output_file_format = template_export.output_file_format.clone();
            new_template.min_segment_size_bytes = template_export.min_segment_size_bytes;
            new_template.max_download_duration_secs = template_export.max_download_duration_secs;
            new_template.max_part_size_bytes = template_export.max_part_size_bytes;
            new_template.record_danmu = template_export.record_danmu;
            new_template.platform_overrides = template_export.platform_overrides.clone();
            new_template.download_retry_policy = template_export.download_retry_policy.clone();
            new_template.danmu_sampling_config = template_export.danmu_sampling_config.clone();
            new_template.download_engine = template_export.download_engine.clone();
            new_template.engines_override = template_export.engines_override.clone();
            new_template.proxy_config = template_export.proxy_config.clone();
            new_template.event_hooks = template_export.event_hooks.clone();
            new_template.stream_selection_config = template_export.stream_selection_config.clone();
            new_template.pipeline = template_export.pipeline.clone();

            config_service
                .create_template_config(&new_template)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to create template: {}", e)))?;
            new_template_name_to_id.insert(new_template.name.clone(), new_template.id.clone());
            stats.templates_created += 1;
        }
    }

    // In replace mode, delete templates not in the import
    if is_replace {
        let imported_template_names: std::collections::HashSet<&str> =
            config.templates.iter().map(|t| t.name.as_str()).collect();
        for (name, template) in &template_name_map {
            if !imported_template_names.contains(name.as_str()) {
                if config_service
                    .delete_template_config(&template.id)
                    .await
                    .is_ok()
                {
                    stats.templates_deleted += 1;
                }
            }
        }
    }

    // Also add existing templates to the map (only if not deleted)
    for (name, template) in template_name_map.iter() {
        if !new_template_name_to_id.contains_key(name) {
            new_template_name_to_id.insert(name.clone(), template.id.clone());
        }
    }

    // 4. Import platforms (by platform_name)
    let existing_platforms = config_service
        .list_platform_configs()
        .await
        .unwrap_or_default();
    let platform_name_map: HashMap<String, PlatformConfigDbModel> = existing_platforms
        .into_iter()
        .map(|p| (p.platform_name.clone(), p))
        .collect();

    // Build platform name -> id map for streamer import
    let mut platform_name_to_id: HashMap<String, String> = platform_name_map
        .iter()
        .map(|(name, p)| (name.clone(), p.id.clone()))
        .collect();

    for platform_export in &config.platforms {
        if let Some(existing) = platform_name_map.get(&platform_export.platform_name) {
            // Update existing
            let mut updated = existing.clone();
            updated.fetch_delay_ms = platform_export.fetch_delay_ms;
            updated.download_delay_ms = platform_export.download_delay_ms;
            updated.cookies = platform_export.cookies.clone();
            updated.platform_specific_config = platform_export.platform_specific_config.clone();
            updated.proxy_config = platform_export.proxy_config.clone();
            updated.record_danmu = platform_export.record_danmu;
            updated.output_folder = platform_export.output_folder.clone();
            updated.output_filename_template = platform_export.output_filename_template.clone();
            updated.download_engine = platform_export.download_engine.clone();
            updated.stream_selection_config = platform_export.stream_selection_config.clone();
            updated.output_file_format = platform_export.output_file_format.clone();
            updated.min_segment_size_bytes = platform_export.min_segment_size_bytes;
            updated.max_download_duration_secs = platform_export.max_download_duration_secs;
            updated.max_part_size_bytes = platform_export.max_part_size_bytes;
            updated.download_retry_policy = platform_export.download_retry_policy.clone();
            updated.event_hooks = platform_export.event_hooks.clone();
            updated.pipeline = platform_export.pipeline.clone();

            config_service
                .update_platform_config(&updated)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to update platform: {}", e)))?;
            platform_name_to_id.insert(updated.platform_name.clone(), updated.id.clone());
            stats.platforms_updated += 1;
        }
        // Note: We don't create new platforms as they're seeded by the system
    }

    // 5. Import streamers (by URL)
    let existing_streamers = streamer_repo.list_streamers().await.unwrap_or_default();
    let streamer_url_map: HashMap<String, StreamerDbModel> = existing_streamers
        .into_iter()
        .map(|s| (s.url.clone(), s))
        .collect();

    for streamer_export in &config.streamers {
        // Resolve platform ID
        let platform_id = platform_name_to_id
            .get(&streamer_export.platform)
            .cloned()
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "Unknown platform '{}' for streamer '{}'",
                    streamer_export.platform, streamer_export.name
                ))
            })?;

        // Resolve template ID
        let template_id = streamer_export
            .template
            .as_ref()
            .and_then(|name| new_template_name_to_id.get(name).cloned());

        if let Some(existing) = streamer_url_map.get(&streamer_export.url) {
            // Update existing
            let mut updated = existing.clone();
            updated.name = streamer_export.name.clone();
            updated.platform_config_id = platform_id;
            updated.template_config_id = template_id;
            updated.priority = streamer_export.priority.clone();
            updated.streamer_specific_config = streamer_export.streamer_specific_config.clone();
            updated.updated_at = Utc::now().to_rfc3339();

            streamer_repo
                .update_streamer(&updated)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to update streamer: {}", e)))?;

            // Update filters: delete existing, add new
            filter_repo
                .delete_filters_for_streamer(&updated.id)
                .await
                .ok();

            for filter_export in &streamer_export.filters {
                let filter = FilterDbModel::new(
                    &updated.id,
                    crate::database::models::FilterType::parse(&filter_export.filter_type)
                        .unwrap_or(crate::database::models::FilterType::Keyword),
                    &filter_export.config,
                );
                filter_repo.create_filter(&filter).await.ok();
            }

            stats.streamers_updated += 1;
        } else {
            // Create new
            let mut new_streamer =
                StreamerDbModel::new(&streamer_export.name, &streamer_export.url, &platform_id);
            new_streamer.template_config_id = template_id;
            new_streamer.priority = streamer_export.priority.clone();
            new_streamer.streamer_specific_config =
                streamer_export.streamer_specific_config.clone();

            streamer_repo
                .create_streamer(&new_streamer)
                .await
                .map_err(|e| ApiError::internal(format!("Failed to create streamer: {}", e)))?;

            // Add filters
            for filter_export in &streamer_export.filters {
                let filter = FilterDbModel::new(
                    &new_streamer.id,
                    crate::database::models::FilterType::parse(&filter_export.filter_type)
                        .unwrap_or(crate::database::models::FilterType::Keyword),
                    &filter_export.config,
                );
                filter_repo.create_filter(&filter).await.ok();
            }

            stats.streamers_created += 1;
        }
    }

    // In replace mode, delete streamers not in the import
    if is_replace {
        let imported_streamer_urls: std::collections::HashSet<&str> =
            config.streamers.iter().map(|s| s.url.as_str()).collect();
        for (url, streamer) in &streamer_url_map {
            if !imported_streamer_urls.contains(url.as_str()) {
                // Delete filters first
                filter_repo
                    .delete_filters_for_streamer(&streamer.id)
                    .await
                    .ok();
                if streamer_repo.delete_streamer(&streamer.id).await.is_ok() {
                    stats.streamers_deleted += 1;
                }
            }
        }
    }

    // 6. Import notification channels (by name)
    let existing_channels = notification_repo.list_channels().await.unwrap_or_default();
    let channel_name_map: HashMap<String, NotificationChannelDbModel> = existing_channels
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();

    for channel_export in &config.notification_channels {
        if let Some(existing) = channel_name_map.get(&channel_export.name) {
            // Update existing
            let mut updated = existing.clone();
            updated.channel_type = channel_export.channel_type.clone();
            updated.settings = channel_export.settings.clone();

            notification_repo
                .update_channel(&updated)
                .await
                .map_err(|e| {
                    ApiError::internal(format!("Failed to update notification channel: {}", e))
                })?;

            // Update subscriptions
            notification_repo.unsubscribe_all(&updated.id).await.ok();

            for event in &channel_export.subscriptions {
                notification_repo.subscribe(&updated.id, event).await.ok();
            }

            stats.channels_updated += 1;
        } else {
            // Create new
            let new_channel = NotificationChannelDbModel::new(
                &channel_export.name,
                crate::database::models::ChannelType::parse(&channel_export.channel_type)
                    .unwrap_or(crate::database::models::ChannelType::Webhook),
                &channel_export.settings,
            );

            notification_repo
                .create_channel(&new_channel)
                .await
                .map_err(|e| {
                    ApiError::internal(format!("Failed to create notification channel: {}", e))
                })?;

            // Add subscriptions
            for event in &channel_export.subscriptions {
                notification_repo
                    .subscribe(&new_channel.id, event)
                    .await
                    .ok();
            }

            stats.channels_created += 1;
        }
    }

    // In replace mode, delete channels not in the import
    if is_replace {
        let imported_channel_names: std::collections::HashSet<&str> = config
            .notification_channels
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        for (name, channel) in &channel_name_map {
            if !imported_channel_names.contains(name.as_str()) {
                // Delete subscriptions first
                notification_repo.unsubscribe_all(&channel.id).await.ok();
                if notification_repo.delete_channel(&channel.id).await.is_ok() {
                    stats.channels_deleted += 1;
                }
            }
        }
    }

    // Reload streamer manager to pick up changes
    if let Some(streamer_manager) = state.streamer_manager.as_ref() {
        let _ = streamer_manager.hydrate().await;
    }

    // Reload notification service to pick up channel changes
    if let Some(notification_service) = state.notification_service.as_ref() {
        let _ = notification_service.reload_from_db().await;
    }

    Ok(Json(ImportResult {
        success: true,
        message: "Configuration imported successfully".to_string(),
        stats,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_config_export_serialization_with_log_filter() {
        let export = GlobalConfigExport {
            output_folder: "test".to_string(),
            output_filename_template: "test".to_string(),
            output_file_format: "test".to_string(),
            min_segment_size_bytes: 0,
            max_download_duration_secs: 0,
            max_part_size_bytes: 0,
            record_danmu: false,
            max_concurrent_downloads: 0,
            max_concurrent_uploads: 0,
            streamer_check_delay_ms: 0,
            proxy_config: "test".to_string(),
            offline_check_delay_ms: 0,
            offline_check_count: 0,
            default_download_engine: "test".to_string(),
            max_concurrent_cpu_jobs: 0,
            max_concurrent_io_jobs: 0,
            job_history_retention_days: 0,
            session_gap_time_secs: 0,
            pipeline: None,
            log_filter_directive: Some("rust_srec=debug".to_string()),
        };
        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("rust_srec=debug"));
        assert!(json.contains("log_filter_directive"));
    }
}
