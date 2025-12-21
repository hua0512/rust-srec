//! Media routes.

use std::path::PathBuf;

use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeFile;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

/// Create the media router.
pub fn router() -> Router<AppState> {
    Router::new().route("/{id}/content", get(get_media_content))
}

#[derive(serde::Deserialize)]
pub struct AuthQuery {
    pub token: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/media/{id}/content",
    tag = "media",
    params(("id" = String, Path, description = "Media output ID")),
    responses(
        (status = 200, description = "Media file content"),
        (status = 404, description = "Media not found", body = crate::api::error::ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_media_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AuthQuery>,
    req: Request,
) -> ApiResult<Response> {
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Authentication not configured"))?;

    let headers = req.headers();
    let (token, source) = if let Some(t) = query.token {
        (t, "Query")
    } else if let Some(t) = headers
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from)
    {
        (t, "Header")
    } else {
        tracing::warn!("Missing or invalid Authorization header or token query");
        return Err(ApiError::unauthorized(
            "Missing or invalid Authorization header",
        ));
    };

    tracing::info!(
        "Validating token from {}: {}...",
        source,
        &token.chars().take(10).collect::<String>()
    );

    jwt_service.validate_token(&token).map_err(|e| {
        tracing::error!("Token validation failed (source: {}): {}", source, e);
        ApiError::unauthorized("Invalid or expired token")
    })?;

    let session_repo = state
        .session_repository
        .ok_or_else(|| ApiError::service_unavailable("Session repository not available"))?;

    // Query media output to get file path
    let media = session_repo
        .get_media_output(&id)
        .await
        .map_err(ApiError::from)?;
    let path = PathBuf::from(media.file_path);

    if !path.exists() {
        return Err(ApiError::not_found(format!("Media file not found: {}", id)));
    }

    match ServeFile::new(path).try_call(req).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(ApiError::internal(format!("Failed to serve file: {}", e))),
    }
}
