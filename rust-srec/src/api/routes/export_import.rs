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

use crate::api::error::{ApiError, ApiResult};
use crate::api::extract::ApiJson;
use crate::api::server::AppState;
use crate::config::backup::{ConfigExport, ImportRequest, ImportResult};

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
    let users = state
        .auth_service
        .as_ref()
        .map(|auth_service| auth_service.user_repository());
    let export = state
        .configuration_export_service
        .export(users.as_deref())
        .await?;

    let json = serde_json::to_string_pretty(&export)?;

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
    description = "Import configuration transactionally. Every successful import revokes all refresh tokens and their bound access-token sessions, including Merge imports that omit users. User-email conflicts with retained accounts are rejected before configuration writes.",
    request_body = ImportRequest,
    responses(
        (status = 200, description = "Configuration imported", body = ImportResult),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn import_config(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<ImportRequest>,
) -> ApiResult<Json<ImportResult>> {
    let import_service = state.configuration_import_service.clone();
    let auth_service = state.auth_service.clone();
    // Detached, so a caller that disconnects or times out cannot cut the
    // import short after its commit, nor skip the cache invalidation below.
    let outcome = tokio::spawn(async move {
        let outcome = import_service.import(request.config, request.mode).await;
        // The import writes user rows in its own transaction without going
        // through `AuthService`, so cached `authorize_access_token` state may
        // no longer match them. It is dropped on failure too: a failure after
        // the commit still leaves the new rows in place.
        if let Some(auth_service) = auth_service {
            auth_service.invalidate_user_cache();
        }
        outcome
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "Configuration import task failed");
        ApiError::internal("Configuration import task failed")
    })?
    .map_err(|error| match error {
        crate::services::config_import::ConfigurationImportError::Validation(message) => {
            ApiError::bad_request(message)
        }
        crate::services::config_import::ConfigurationImportError::Database(error) => {
            ApiError::from(error)
        }
    })?;

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
    use crate::config::backup::{GlobalConfigExport, StreamerExport};

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
        assert_eq!(export.output_retention_days, 0);
        assert!(!export.output_retention_delete_files);
        assert_eq!(
            export.log_filter_directive.as_deref(),
            Some("rust_srec=debug")
        );
    }
}
