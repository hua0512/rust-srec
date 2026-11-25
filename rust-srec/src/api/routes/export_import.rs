//! Configuration export and import routes.
//!
//! Provides endpoints to backup and restore the entire system configuration.

use axum::{
    Json, Router,
    extract::State,
    http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, REFERRER_POLICY},
    response::IntoResponse,
    routing::{get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::extract::ApiJson;
use crate::api::server::AppState;
use crate::config::backup::{ImportRequest, ImportResult};

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
        (status = 200, description = "Configuration export", body = crate::api::backup::ConfigExport)
    ),
    security(("bearer_auth" = []))
)]
pub async fn export_config(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let user_exports = if let Some(auth_service) = state.auth_service.as_ref() {
        crate::api::auth_store::export_users(auth_service.user_repository().as_ref()).await?
    } else {
        Vec::new()
    };
    let export = state.config_backup.export_config(user_exports).await?;

    let json = serde_json::to_string_pretty(&export)?;

    let filename = format!(
        "rust-srec-backup-{}.json",
        jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S")
    );
    let content_disposition = format!("attachment; filename=\"{}\"", filename);

    Ok((
        [
            (CONTENT_TYPE, "application/json".to_string()),
            (CONTENT_DISPOSITION, content_disposition),
            (CACHE_CONTROL, "private, no-store".to_string()),
            (REFERRER_POLICY, "no-referrer".to_string()),
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
    request_body = crate::api::backup::ImportRequest,
    responses(
        (status = 200, description = "Configuration imported", body = crate::api::backup::ImportResult),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn import_config(
    State(state): State<AppState>,
    ApiJson(request): ApiJson<ImportRequest>,
) -> ApiResult<Json<ImportResult>> {
    let config_backup = state.config_backup.clone();
    let auth_service = state.auth_service.clone();
    // Detached, so a caller that disconnects or times out cannot cut the
    // import short after its commit, nor skip the cache invalidation below.
    let outcome = tokio::spawn(async move {
        // Invalidate `AuthService`'s user/API-key cache synchronously inside
        // the commit hook before `finish_import` runs.
        let on_users_committed = auth_service.map(|auth| {
            Box::new(move || auth.invalidate_user_cache()) as Box<dyn FnOnce() + Send + 'static>
        });
        config_backup
            .import_config(request.config, request.mode, on_users_committed)
            .await
    })
    .await
    .map_err(|error| {
        tracing::error!(error = %error, "Configuration import task failed");
        ApiError::internal("Configuration import task failed")
    })?
    .map_err(ApiError::from)?;

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
        message.push_str(". Warnings: ");
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
    #[test]
    fn test_schema_version_at_least() {
        let is_at_least = crate::config::backup::schema_version_at_least;
        assert!(is_at_least("0.1.3", (0, 1, 3)));
        assert!(is_at_least("0.1.10", (0, 1, 3)));
        assert!(!is_at_least("0.1.2", (0, 1, 3)));
        assert!(is_at_least("0.1.3-alpha.1", (0, 1, 3)));
        assert!(!is_at_least("invalid", (0, 1, 3)));
    }
}
