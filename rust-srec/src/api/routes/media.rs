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

    // Windows note: some parts of the pipeline/tooling may emit extended-length paths
    // like `\\?\C:\...`. While this is valid for Win32 APIs, it can be a portability
    // footgun across libraries and runtimes. Normalize it to a regular path when possible.
    let mut path = PathBuf::from(&media.file_path);
    if cfg!(windows)
        && let Some(s) = path.to_str()
    {
        if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
            // `\\?\UNC\server\share\...` -> `\\server\share\...`
            path = PathBuf::from(format!(r"\\{}", rest));
        } else if let Some(rest) = s.strip_prefix(r"\\?\") {
            // `\\?\C:\...` -> `C:\...`
            path = PathBuf::from(rest);
        }
    }

    if !path.exists() {
        return Err(ApiError::not_found(format!("Media file not found: {}", id)));
    }

    match ServeFile::new(path).try_call(req).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(ApiError::internal(format!("Failed to serve file: {}", e))),
    }
}
