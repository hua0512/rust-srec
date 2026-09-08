//! Media routes.

use std::path::PathBuf;

use axum::Router;
use axum::extract::{FromRef, Path, Query, Request, State};
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
    crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers,
        query.token.as_deref(),
        crate::api::auth_request::AccessPolicy::Read,
    )
    .await?;

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
        Err(error) => Err(ApiError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{models::ApiKeyAccessLevel, repositories::SqlxSessionRepository};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn media_router_authenticates_read_keys_and_never_falls_back_from_a_header() {
        let fixture = crate::api::auth_request::tests::fixture().await;
        let (_, raw) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "media-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let state = MediaRouteState {
            auth_service: Some(fixture.service.clone()),
            session_repository: std::sync::Arc::new(SqlxSessionRepository::new(
                fixture.pool.clone(),
                fixture.pool.clone(),
            )),
        };
        let app = Router::new()
            .route("/{id}/content", get(get_media_content))
            .with_state(state);
        // An absent output distinguishes authorization success from rejection without filesystem IO.
        for (query, header, expected) in [
            (raw.as_str(), None, StatusCode::NOT_FOUND),
            (
                "invalid",
                Some(format!("Bearer {raw}")),
                StatusCode::NOT_FOUND,
            ),
            (
                raw.as_str(),
                Some("Basic malformed".to_owned()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                raw.as_str(),
                Some("Bearer invalid".to_owned()),
                StatusCode::UNAUTHORIZED,
            ),
        ] {
            let mut request = Request::builder().uri(format!("/absent/content?token={query}"));
            if let Some(header) = header {
                request = request.header("Authorization", header);
            }
            let response = app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
        }
        fixture.pool.close().await;
    }
}
