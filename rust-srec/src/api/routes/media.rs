//! Media routes.

use std::path::PathBuf;

use axum::Router;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower_http::services::ServeFile;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

/// Create the media router.
pub fn router() -> Router<AppState> {
    Router::new().route("/{id}/content", get(get_media_content))
}

/// Get media content by ID.
///
/// Query the media output by ID to get the file path, then serve the file.
async fn get_media_content(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
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

    // Serve the file
    let req = axum::http::Request::builder()
        .body(axum::body::Body::empty())
        .map_err(|e| ApiError::internal(e.to_string()))?;

    match ServeFile::new(path).try_call(req).await {
        Ok(response) => Ok(response.into_response()),
        Err(e) => Err(ApiError::internal(format!("Failed to serve file: {}", e))),
    }
}
