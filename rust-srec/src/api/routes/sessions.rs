//! Session management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    PaginatedResponse, PaginationParams, SessionFilterParams, SessionResponse,
};
use crate::api::server::AppState;

/// Create the sessions router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_sessions))
        .route("/:id", get(get_session))
}

/// List sessions with pagination and filtering.
async fn list_sessions(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<SessionFilterParams>,
) -> ApiResult<Json<PaginatedResponse<SessionResponse>>> {
    // TODO: Implement actual listing logic using SessionRepository
    let response = PaginatedResponse::new(Vec::new(), 0, pagination.limit, pagination.offset);

    Ok(Json(response))
}

/// Get a single session by ID.
async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionResponse>> {
    // TODO: Implement actual retrieval logic using SessionRepository
    Err(ApiError::not_found(format!(
        "Session with id '{}' not found",
        id
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_filter_params_default() {
        let params = SessionFilterParams::default();
        assert!(params.streamer_id.is_none());
        assert!(params.from_date.is_none());
        assert!(params.to_date.is_none());
        assert!(params.active_only.is_none());
    }
}
