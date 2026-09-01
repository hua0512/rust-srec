//! Media routes.

use std::path::PathBuf;

use axum::Router;
use axum::extract::{FromRef, Path, Query, Request, State};
use axum::http::header::AUTHORIZATION;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeFile;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

#[derive(Clone)]
pub struct MediaRouteState {
    auth_service: Option<std::sync::Arc<crate::api::auth_service::AuthService>>,
    session_repository: std::sync::Arc<dyn crate::database::repositories::SessionRepository>,
}

impl FromRef<AppState> for MediaRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
            session_repository: state.session_repository.clone(),
        }
    }
}

/// Create the media router.
pub fn router() -> Router<AppState> {
    Router::new().route("/{id}/content", get(get_media_content))
}

#[derive(serde::Deserialize)]
pub struct AuthQuery {
    pub token: Option<String>,
}

/// Turn a stored `media_outputs.file_path` into a path usable by the std/tokio APIs.
///
/// Windows note: some parts of the pipeline/tooling may emit extended-length paths
/// like `\\?\C:\...`. While this is valid for Win32 APIs, it can be a portability
/// footgun across libraries and runtimes. Normalize it to a regular path when possible.
///
/// Shared by [`get_media_content`] and the media output deletion in
/// [`crate::api::routes::pipeline::jobs`] so both resolve a stored path identically.
pub(crate) fn normalize_media_path(file_path: &str) -> PathBuf {
    let path = PathBuf::from(file_path);
    if cfg!(windows)
        && let Some(s) = path.to_str()
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            // `\\?\UNC\server\share\...` -> `\\server\share\...`
            return PathBuf::from(format!(r"\\{}", rest));
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            // `\\?\C:\...` -> `C:\...`
            return PathBuf::from(rest);
        }
    }
    path
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
    State(state): State<MediaRouteState>,
    Path(id): Path<String>,
    Query(query): Query<AuthQuery>,
    req: Request,
) -> ApiResult<Response> {
    let headers = req.headers();
    if let Some(auth_service) = &state.auth_service {
        let token = query.token.or_else(|| {
            headers
                .get(AUTHORIZATION)
                .and_then(|header| header.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(String::from)
        });
        let token = token.as_deref().ok_or_else(|| {
            ApiError::unauthorized("Missing or invalid Authorization header or token query")
        })?;

        auth_service
            .authorize_access_token(token, false)
            .await
            .map_err(ApiError::from)?;
    }

    let session_repo = &state.session_repository;

    // Query media output to get file path
    let media = session_repo
        .get_media_output(&id)
        .await
        .map_err(ApiError::from)?;

    let path = normalize_media_path(&media.file_path);

    let exists = tokio::fs::try_exists(&path)
        .await
        .map_err(|error| ApiError::from(crate::Error::io_path("try_exists", &path, error)))?;
    if !exists {
        return Err(ApiError::not_found(format!("Media file not found: {}", id)));
    }

    match ServeFile::new(path).try_call(req).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(ApiError::internal(format!("Failed to serve file: {}", e))),
    }
}
