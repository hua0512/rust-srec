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
use std::collections::HashMap;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::config::backup::{
    ConfigExport, EngineExport, FilterExport, GlobalConfigExport, ImportRequest, ImportResult,
    JobPresetExport, NotificationChannelExport, PipelinePresetExport, PlatformExport,
    StreamerExport, TemplateExport, UserExport, unwrap_json_value,
};
use crate::database::models::{StreamerDbModel, UserDbModel};

/// Current schema version for exports.
const EXPORT_SCHEMA_VERSION: &str = "0.1.7";

/// Helper to parse a database string into a normalized JSON Value.
fn parse_db_config(s: impl Into<String>) -> serde_json::Value {
    let s = s.into();
    if s.is_empty() {
        return serde_json::Value::Null;
    }
    let value = serde_json::from_str(&s).unwrap_or(serde_json::Value::String(s));
    unwrap_json_value(value)
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
        filters,
    }
}

/// Create the export/import router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/export", get(export_config))
        .route("/import", post(import_config))
}

// ============================================================================
// Export Handler
// ============================================================================

#[utoipa::path(
    get,
    path = "/api/config/backup/export",
    tag = "export_import",
    responses(
        (status = 200, description = "Configuration export", body = ConfigExport)
    ),
    security(("bearer_auth" = []))
)]
pub async fn export_config(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let config_service = &state.config_service;

    let streamer_repo = &state.streamer_repository;

    let notification_repo = &state.notification_repository;

    let filter_repo = &state.filter_repository;

    let job_preset_repo = &state.job_preset_repository;

    let pipeline_preset_repo = &state.pipeline_preset_repository;

    let user_exports = if let Some(auth_service) = state.auth_service.as_ref() {
        let user_repo = auth_service.user_repository();
        let total_users = user_repo
            .count()
            .await
            .map_err(|e| ApiError::internal(format!("Failed to count users: {}", e)))?;

        let users = user_repo
            .list(total_users, 0)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to list users: {}", e)))?;

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

    let streamers = streamer_repo
        .list_streamers()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list streamers: {}", e)))?;

    let channels = notification_repo
        .list_channels()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list notification channels: {}", e)))?;

    let job_presets = job_preset_repo
        .list_presets()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list job presets: {}", e)))?;

    let pipeline_presets = pipeline_preset_repo
        .list_pipeline_presets()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list pipeline presets: {}", e)))?;

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
    for streamer in &streamers {
        let filters = filter_repo
            .get_filters_for_streamer(&streamer.id)
            .await
            .unwrap_or_default();

        let filter_exports: Vec<FilterExport> = filters
            .iter()
            .map(|f| FilterExport {
                filter_type: f.filter_type.clone(),
                config: parse_db_config(f.config.clone()),
            })
            .collect();

        streamer_exports.push(build_streamer_export(
            streamer,
            &platform_map,
            &template_map,
            filter_exports,
        ));
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
            settings: parse_db_config(channel.settings.clone()),
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
            proxy_config: parse_db_config(global_config.proxy_config),
            offline_check_delay_ms: global_config.offline_check_delay_ms,
            offline_check_count: global_config.offline_check_count,
            default_download_engine: global_config.default_download_engine,
            max_concurrent_cpu_jobs: global_config.max_concurrent_cpu_jobs,
            max_concurrent_io_jobs: global_config.max_concurrent_io_jobs,
            job_history_retention_days: global_config.job_history_retention_days,
            notification_event_log_retention_days: global_config
                .notification_event_log_retention_days,
            pipeline: global_config.pipeline.map(parse_db_config),
            session_complete_pipeline: global_config.session_complete_pipeline.map(parse_db_config),
            paired_segment_pipeline: global_config.paired_segment_pipeline.map(parse_db_config),
            log_filter_directive: Some(global_config.log_filter_directive),
            auto_thumbnail: global_config.auto_thumbnail,

            pipeline_cpu_job_timeout_secs: global_config.pipeline_cpu_job_timeout_secs,
            pipeline_io_job_timeout_secs: global_config.pipeline_io_job_timeout_secs,
            pipeline_execute_timeout_secs: global_config.pipeline_execute_timeout_secs,
            queue_freshness_threshold_ms: global_config.queue_freshness_threshold_ms,
            gpu_health_probe_interval_secs: global_config.gpu_health_probe_interval_secs,
            stream_proxy_allow_private_targets: global_config.stream_proxy_allow_private_targets,
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
                platform_overrides: t.platform_overrides.clone().map(parse_db_config),
                download_retry_policy: t.download_retry_policy.clone().map(parse_db_config),
                danmu_sampling_config: t.danmu_sampling_config.clone().map(parse_db_config),
                download_engine: t.download_engine.clone(),
                engines_override: t.engines_override.clone().map(parse_db_config),
                proxy_config: t.proxy_config.clone().map(parse_db_config),
                event_hooks: t.event_hooks.clone().map(parse_db_config),
                stream_selection_config: t.stream_selection_config.clone().map(parse_db_config),
                pipeline: t.pipeline.clone().map(parse_db_config),
                session_complete_pipeline: t.session_complete_pipeline.clone().map(parse_db_config),
                paired_segment_pipeline: t.paired_segment_pipeline.clone().map(parse_db_config),
                offline_check_count: t.offline_check_count,
                offline_check_delay_ms: t.offline_check_delay_ms,
            })
            .collect(),
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
            .iter()
            .map(|p| PlatformExport {
                platform_name: p.platform_name.clone(),
                fetch_delay_ms: p.fetch_delay_ms,
                download_delay_ms: p.download_delay_ms,
                cookies: p.cookies.clone(),
                platform_specific_config: p.platform_specific_config.clone().map(parse_db_config),
                proxy_config: p.proxy_config.clone().map(parse_db_config),
                record_danmu: p.record_danmu,
                output_folder: p.output_folder.clone(),
                output_filename_template: p.output_filename_template.clone(),
                download_engine: p.download_engine.clone(),
                stream_selection_config: p.stream_selection_config.clone().map(parse_db_config),
                output_file_format: p.output_file_format.clone(),
                min_segment_size_bytes: p.min_segment_size_bytes,
                max_download_duration_secs: p.max_download_duration_secs,
                max_part_size_bytes: p.max_part_size_bytes,
                download_retry_policy: p.download_retry_policy.clone().map(parse_db_config),
                event_hooks: p.event_hooks.clone().map(parse_db_config),
                pipeline: p.pipeline.clone().map(parse_db_config),
                session_complete_pipeline: p.session_complete_pipeline.clone().map(parse_db_config),
                paired_segment_pipeline: p.paired_segment_pipeline.clone().map(parse_db_config),
                offline_check_count: p.offline_check_count,
                offline_check_delay_ms: p.offline_check_delay_ms,
            })
            .collect(),
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

#[utoipa::path(
    post,
    path = "/api/config/backup/import",
    tag = "export_import",
    request_body = ImportRequest,
    responses(
        (status = 200, description = "Configuration imported", body = ImportResult),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn import_config(
    State(state): State<AppState>,
    Json(request): Json<ImportRequest>,
) -> ApiResult<Json<ImportResult>> {
    let outcome = state
        .configuration_import_service
        .import(request.config, request.mode)
        .await
        .map_err(|error| match error {
            crate::services::config_import::ConfigurationImportError::Validation(message) => {
                ApiError::bad_request(message)
            }
            crate::services::config_import::ConfigurationImportError::Database(error) => {
                ApiError::internal(format!("Configuration import failed: {error}"))
            }
        })?;

    // The import writes user rows in its own transaction without going
    // through `AuthService`, so cached `authorize_access_token` state may no
    // longer match the rows just written; drop it all.
    if let Some(auth_service) = state.auth_service.as_ref() {
        auth_service.invalidate_user_cache();
    }

    let stats = outcome.stats;
    let mut message = format!(
        "Imported: {} templates, {} streamers, {} engines, {} platforms updated, {} channels, {} job presets, {} pipeline presets, {} users",
        stats.templates_created + stats.templates_updated,
        stats.streamers_created + stats.streamers_updated,
        stats.engines_created + stats.engines_updated,
        stats.platforms_updated,
        stats.channels_created + stats.channels_updated,
        stats.job_presets_created + stats.job_presets_updated,
        stats.pipeline_presets_created + stats.pipeline_presets_updated,
        stats.users_created + stats.users_updated
    );
    if !outcome.warnings.is_empty() {
        message.push_str(". Runtime reload warnings: ");
        message.push_str(&outcome.warnings.join("; "));
    }

    Ok(Json(ImportResult {
        success: true,
        message,
        stats,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_version_at_least() {
        let is_at_least = crate::config::backup::schema_version_at_least;
        assert!(is_at_least("0.1.3", (0, 1, 3)));
        assert!(is_at_least("0.1.10", (0, 1, 3)));
        assert!(!is_at_least("0.1.2", (0, 1, 3)));
        assert!(is_at_least("0.1.3-alpha.1", (0, 1, 3)));
        assert!(!is_at_least("invalid", (0, 1, 3)));
    }

    #[test]
    fn test_streamer_export_serialization_includes_avatar_url() {
        let streamer = StreamerExport {
            name: "test".to_string(),
            url: "https://example.com/streamer".to_string(),
            platform: "twitch".to_string(),
            template: None,
            priority: "NORMAL".to_string(),
            state: "NOT_LIVE".to_string(),
            avatar_url: Some("https://example.com/avatar.png".to_string()),
            streamer_specific_config: None,
            filters: vec![],
        };

        let json = serde_json::to_value(&streamer).unwrap();
        assert_eq!(json["state"], "NOT_LIVE");
        assert_eq!(json["avatar_url"], "https://example.com/avatar.png");
    }

    #[test]
    fn test_streamer_export_deserialization_missing_avatar_url_defaults_to_none() {
        let json = r#"{
            "name":"test",
            "url":"https://example.com/streamer",
            "platform":"twitch",
            "template":null,
            "priority":"NORMAL",
            "state":"NOT_LIVE",
            "streamer_specific_config":null,
            "filters":[]
        }"#;

        let streamer: StreamerExport = serde_json::from_str(json).unwrap();
        assert_eq!(streamer.avatar_url, None);
    }

    #[test]
    fn test_streamer_export_deserialization_missing_state_defaults_to_not_live() {
        let json = r#"{
            "name":"test",
            "url":"https://example.com/streamer",
            "platform":"twitch",
            "template":null,
            "priority":"NORMAL",
            "avatar_url":null,
            "streamer_specific_config":null,
            "filters":[]
        }"#;

        let streamer: StreamerExport = serde_json::from_str(json).unwrap();
        assert_eq!(streamer.state, "NOT_LIVE");
    }

    #[test]
    fn test_user_export_serialization_includes_sensitive_fields() {
        let user = UserExport {
            id: "user-1".to_string(),
            username: "admin".to_string(),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$abc$def".to_string(),
            email: None,
            roles: vec!["admin".to_string(), "user".to_string()],
            is_active: true,
            must_change_password: false,
            last_login_at: None,
            created_at: 1_767_225_600_000,
            updated_at: 1_767_225_600_000,
        };

        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(json["username"], "admin");
        assert_eq!(
            json["password_hash"],
            "$argon2id$v=19$m=19456,t=2,p=1$abc$def"
        );
        assert_eq!(json["roles"][0], "admin");
    }

    #[test]
    fn test_build_streamer_export_includes_streamer_specific_config_refresh_token() {
        let mut streamer =
            StreamerDbModel::new("test", "https://live.bilibili.com/123", "platform-bilibili");
        streamer.streamer_specific_config =
            Some(r#"{"refresh_token":"rt","cookies":"c"}"#.to_string());

        let platform_map =
            HashMap::from([("platform-bilibili".to_string(), "bilibili".to_string())]);
        let template_map = HashMap::new();

        let export = build_streamer_export(&streamer, &platform_map, &template_map, vec![]);
        assert_eq!(export.platform, "bilibili");
        assert_eq!(
            export
                .streamer_specific_config
                .and_then(|v| v.get("refresh_token").cloned()),
            Some(serde_json::Value::String("rt".to_string()))
        );
    }

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
            proxy_config: serde_json::Value::String("test".to_string()),
            offline_check_delay_ms: 0,
            offline_check_count: 0,
            default_download_engine: "test".to_string(),
            max_concurrent_cpu_jobs: 0,
            max_concurrent_io_jobs: 0,
            job_history_retention_days: 0,
            notification_event_log_retention_days: 0,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            log_filter_directive: Some("rust_srec=debug".to_string()),
            auto_thumbnail: false,

            pipeline_cpu_job_timeout_secs: 3600,
            pipeline_io_job_timeout_secs: 3600,
            pipeline_execute_timeout_secs: 3600,
            queue_freshness_threshold_ms: 60_000,
            gpu_health_probe_interval_secs: 30,
            stream_proxy_allow_private_targets: false,
        };
        let json = serde_json::to_string(&export).unwrap();
        assert!(json.contains("rust_srec=debug"));
        assert!(json.contains("log_filter_directive"));
    }

    #[test]
    fn test_global_config_export_accepts_legacy_session_gap_time() {
        let json = serde_json::json!({
            "output_folder": "test",
            "output_filename_template": "test",
            "output_file_format": "test",
            "min_segment_size_bytes": 0,
            "max_download_duration_secs": 0,
            "max_part_size_bytes": 0,
            "record_danmu": false,
            "max_concurrent_downloads": 0,
            "max_concurrent_uploads": 0,
            "streamer_check_delay_ms": 0,
            "proxy_config": "test",
            "offline_check_delay_ms": 0,
            "offline_check_count": 0,
            "default_download_engine": "test",
            "max_concurrent_cpu_jobs": 0,
            "max_concurrent_io_jobs": 0,
            "job_history_retention_days": 0,
            "notification_event_log_retention_days": 0,
            "session_gap_time_secs": 3600,
            "pipeline": null,
            "session_complete_pipeline": null,
            "paired_segment_pipeline": null,
            "log_filter_directive": "rust_srec=debug",
            "auto_thumbnail": false,
            "pipeline_cpu_job_timeout_secs": 3600,
            "pipeline_io_job_timeout_secs": 3600,
            "pipeline_execute_timeout_secs": 3600,
            "queue_freshness_threshold_ms": 60_000
        });

        let export: GlobalConfigExport = serde_json::from_value(json).unwrap();
        assert_eq!(
            export.log_filter_directive.as_deref(),
            Some("rust_srec=debug")
        );
    }

    #[test]
    fn test_unwrap_json_value() {
        let double_encoded = serde_json::Value::String("{\"a\": 1}".to_string());
        let unwrapped = unwrap_json_value(double_encoded);
        assert_eq!(unwrapped["a"], 1);

        let triple_encoded = serde_json::Value::String("\"{\\\"b\\\": 2}\"".to_string());
        let unwrapped2 = unwrap_json_value(triple_encoded);
        assert_eq!(unwrapped2["b"], 2);

        let normal = serde_json::json!({"c": 3});
        let unwrapped3 = unwrap_json_value(normal.clone());
        assert_eq!(unwrapped3, normal);
    }

    #[test]
    fn test_parse_db_config() {
        let db_string = "{\"a\": 1}".to_string();
        let parsed = parse_db_config(db_string);
        assert_eq!(parsed["a"], 1);

        let escaped_db_string = "\"{\\\"b\\\": 2}\"".to_string();
        let parsed2 = parse_db_config(escaped_db_string);
        assert_eq!(parsed2["b"], 2);

        let non_json = "plain text".to_string();
        let parsed3 = parse_db_config(non_json);
        assert_eq!(parsed3.as_str().unwrap(), "plain text");
    }
}
