//! Filter management routes.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get, patch, post},
};
use serde_json::Value;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{CreateFilterRequest, FilterResponse, UpdateFilterRequest};
use crate::api::server::AppState;
use crate::database::models::{FilterDbModel, FilterType};

/// Create the filters router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(list_filters))
        .route("/", post(create_filter))
        .route("/{id}", get(get_filter))
        .route("/{id}", patch(update_filter))
        .route("/{id}", delete(delete_filter))
}

/// Convert FilterDbModel to FilterResponse.
fn model_to_response(model: &FilterDbModel) -> ApiResult<FilterResponse> {
    let config: Value = serde_json::from_str(&model.config)
        .map_err(|e| ApiError::internal(format!("Failed to parse filter config: {}", e)))?;

    Ok(FilterResponse {
        id: model.id.clone(),
        streamer_id: model.streamer_id.clone(),
        filter_type: model.filter_type.clone(),
        config,
    })
}

/// List filters for a streamer.
///
/// GET /api/streamers/{streamer_id}/filters
async fn list_filters(
    State(state): State<AppState>,
    Path(streamer_id): Path<String>,
) -> ApiResult<Json<Vec<FilterResponse>>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    let filters = filter_repo
        .get_filters_for_streamer(&streamer_id)
        .await
        .map_err(ApiError::from)?;

    let response: Result<Vec<_>, _> = filters.iter().map(model_to_response).collect();
    Ok(Json(response?))
}

/// Create a new filter.
///
/// POST /api/streamers/{streamer_id}/filters
async fn create_filter(
    State(state): State<AppState>,
    Path(streamer_id): Path<String>,
    Json(request): Json<CreateFilterRequest>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    // Validate filter type
    let filter_type = FilterType::parse(&request.filter_type).ok_or_else(|| {
        ApiError::validation(format!("Invalid filter type: {}", request.filter_type))
    })?;

    // Serialize config to string
    let config_str = serde_json::to_string(&request.config)
        .map_err(|e| ApiError::validation(format!("Invalid config JSON: {}", e)))?;

    // Create DB model
    let filter = FilterDbModel::new(streamer_id, filter_type, config_str);

    // Save to DB
    filter_repo
        .create_filter(&filter)
        .await
        .map_err(ApiError::from)?;

    model_to_response(&filter).map(Json)
}

/// Get a specific filter.
///
/// GET /api/streamers/{streamer_id}/filters/{id}
async fn get_filter(
    State(state): State<AppState>,
    Path((streamer_id, id)): Path<(String, String)>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    let filter = filter_repo.get_filter(&id).await.map_err(ApiError::from)?;

    // Verify streamer ID matches
    if filter.streamer_id != streamer_id {
        return Err(ApiError::not_found(format!(
            "Filter {} not found for streamer {}",
            id, streamer_id
        )));
    }

    model_to_response(&filter).map(Json)
}

/// Update a filter.
///
/// PATCH /api/streamers/{streamer_id}/filters/{id}
async fn update_filter(
    State(state): State<AppState>,
    Path((streamer_id, id)): Path<(String, String)>,
    Json(request): Json<UpdateFilterRequest>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    // Get existing filter
    let mut filter = filter_repo.get_filter(&id).await.map_err(ApiError::from)?;

    // Verify streamer ID matches
    if filter.streamer_id != streamer_id {
        return Err(ApiError::not_found(format!(
            "Filter {} not found for streamer {}",
            id, streamer_id
        )));
    }

    // Update fields if provided
    if let Some(ft_str) = request.filter_type {
        let _ = FilterType::parse(&ft_str)
            .ok_or_else(|| ApiError::validation(format!("Invalid filter type: {}", ft_str)))?;
        filter.filter_type = ft_str;
    }

    if let Some(config_value) = request.config {
        filter.config = serde_json::to_string(&config_value)
            .map_err(|e| ApiError::validation(format!("Invalid config JSON: {}", e)))?;
    }

    // Save updates
    filter_repo
        .update_filter(&filter)
        .await
        .map_err(ApiError::from)?;

    model_to_response(&filter).map(Json)
}

/// Delete a filter.
///
/// DELETE /api/streamers/{streamer_id}/filters/{id}
async fn delete_filter(
    State(state): State<AppState>,
    Path((streamer_id, id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    // Check ownership before deleting
    let filter = filter_repo.get_filter(&id).await.map_err(ApiError::from)?;

    if filter.streamer_id != streamer_id {
        return Err(ApiError::not_found(format!(
            "Filter {} not found for streamer {}",
            id, streamer_id
        )));
    }

    filter_repo
        .delete_filter(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Filter {} deleted", id)
    })))
}
