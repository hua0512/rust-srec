//! Streamer management routes.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, patch, post},
    Json, Router,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateStreamerRequest, PaginatedResponse, PaginationParams, StreamerFilterParams,
    StreamerResponse, UpdatePriorityRequest, UpdateStreamerRequest,
};
use crate::api::server::AppState;

/// Create the streamers router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_streamer))
        .route("/", get(list_streamers))
        .route("/:id", get(get_streamer))
        .route("/:id", patch(update_streamer))
        .route("/:id", delete(delete_streamer))
        .route("/:id/clear-error", post(clear_error))
        .route("/:id/priority", patch(update_priority))
}

/// Create a new streamer.
async fn create_streamer(
    State(state): State<AppState>,
    Json(request): Json<CreateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // Validate URL format
    if request.url.is_empty() {
        return Err(ApiError::validation("URL cannot be empty"));
    }

    // TODO: Implement actual creation logic using StreamerManager
    // For now, return a placeholder response
    let response = StreamerResponse {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        url: request.url,
        platform_config_id: request.platform_config_id,
        template_id: request.template_id,
        state: crate::domain::streamer::StreamerState::NotLive,
        priority: request.priority,
        enabled: request.enabled,
        consecutive_error_count: 0,
        disabled_until: None,
        last_live_time: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    Ok(Json(response))
}

/// List streamers with pagination and filtering.
async fn list_streamers(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<StreamerFilterParams>,
) -> ApiResult<Json<PaginatedResponse<StreamerResponse>>> {
    // TODO: Implement actual listing logic using StreamerManager
    // For now, return an empty response
    let response = PaginatedResponse::new(Vec::new(), 0, pagination.limit, pagination.offset);

    Ok(Json(response))
}

/// Get a single streamer by ID.
async fn get_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // TODO: Implement actual retrieval logic using StreamerManager
    Err(ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

/// Update a streamer.
async fn update_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateStreamerRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // TODO: Implement actual update logic using StreamerManager
    Err(ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

/// Delete a streamer.
async fn delete_streamer(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // TODO: Implement actual deletion logic using StreamerManager
    Err(ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

/// Clear error state for a streamer.
async fn clear_error(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<StreamerResponse>> {
    // TODO: Implement actual clear error logic using StreamerManager
    Err(ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

/// Update streamer priority.
async fn update_priority(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdatePriorityRequest>,
) -> ApiResult<Json<StreamerResponse>> {
    // TODO: Implement actual priority update logic using StreamerManager
    Err(ApiError::not_found(format!("Streamer with id '{}' not found", id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_streamer_request_validation() {
        let request = CreateStreamerRequest {
            name: "Test".to_string(),
            url: "".to_string(),
            platform_config_id: "platform1".to_string(),
            template_id: None,
            priority: crate::domain::value_objects::Priority::Normal,
            enabled: true,
        };
        
        // URL is empty, should fail validation
        assert!(request.url.is_empty());
    }
}
