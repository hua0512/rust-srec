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
use crate::database::models::{
    CategoryFilterConfig, CronFilterConfig, FilterConfigValidator, FilterDbModel, FilterType,
    KeywordFilterConfig, RegexFilterConfig, TimeBasedFilterConfig,
};

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

fn validate_and_serialize_config(filter_type: FilterType, config: Value) -> ApiResult<String> {
    match filter_type {
        FilterType::TimeBased => {
            let mut typed: TimeBasedFilterConfig = serde_json::from_value(config).map_err(|e| {
                ApiError::validation(format!("Invalid time-based filter config: {}", e))
            })?;
            typed.normalize();
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serde_json::to_string(&typed).map_err(|e| {
                ApiError::validation(format!(
                    "Failed to serialize time-based filter config: {}",
                    e
                ))
            })
        }
        FilterType::Keyword => {
            let mut typed: KeywordFilterConfig = serde_json::from_value(config).map_err(|e| {
                ApiError::validation(format!("Invalid keyword filter config: {}", e))
            })?;
            typed.normalize();
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serde_json::to_string(&typed).map_err(|e| {
                ApiError::validation(format!("Failed to serialize keyword filter config: {}", e))
            })
        }
        FilterType::Category => {
            let mut typed: CategoryFilterConfig = serde_json::from_value(config).map_err(|e| {
                ApiError::validation(format!("Invalid category filter config: {}", e))
            })?;
            typed.normalize();
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serde_json::to_string(&typed).map_err(|e| {
                ApiError::validation(format!("Failed to serialize category filter config: {}", e))
            })
        }
        FilterType::Cron => {
            let typed: CronFilterConfig = serde_json::from_value(config)
                .map_err(|e| ApiError::validation(format!("Invalid cron filter config: {}", e)))?;
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serde_json::to_string(&typed).map_err(|e| {
                ApiError::validation(format!("Failed to serialize cron filter config: {}", e))
            })
        }
        FilterType::Regex => {
            let typed: RegexFilterConfig = serde_json::from_value(config)
                .map_err(|e| ApiError::validation(format!("Invalid regex filter config: {}", e)))?;
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serde_json::to_string(&typed).map_err(|e| {
                ApiError::validation(format!("Failed to serialize regex filter config: {}", e))
            })
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/streamers/{streamer_id}/filters",
    tag = "filters",
    params(("streamer_id" = String, Path, description = "Streamer ID")),
    responses(
        (status = 200, description = "List of filters", body = Vec<FilterResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_filters(
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

#[utoipa::path(
    post,
    path = "/api/streamers/{streamer_id}/filters",
    tag = "filters",
    params(("streamer_id" = String, Path, description = "Streamer ID")),
    request_body = CreateFilterRequest,
    responses(
        (status = 201, description = "Filter created", body = FilterResponse),
        (status = 422, description = "Validation error", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_filter(
    State(state): State<AppState>,
    Path(streamer_id): Path<String>,
    Json(request): Json<CreateFilterRequest>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = state
        .filter_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Filter service not available"))?;

    if request.streamer_id != streamer_id {
        return Err(ApiError::validation(format!(
            "streamer_id mismatch: path={} body={}",
            streamer_id, request.streamer_id
        )));
    }

    // Validate filter type
    let filter_type = FilterType::parse(&request.filter_type).ok_or_else(|| {
        ApiError::validation(format!("Invalid filter type: {}", request.filter_type))
    })?;

    // Validate + normalize config and store canonical JSON.
    let config_str = validate_and_serialize_config(filter_type, request.config)?;

    // Create DB model
    let filter = FilterDbModel::new(&streamer_id, filter_type, config_str);

    // Save to DB
    filter_repo
        .create_filter(&filter)
        .await
        .map_err(ApiError::from)?;

    // Notify scheduler/runtime that filter rules changed for this streamer.
    if let Some(config_service) = &state.config_service {
        config_service.notify_streamer_filters_updated(&streamer_id);
    }

    model_to_response(&filter).map(Json)
}

#[utoipa::path(
    get,
    path = "/api/streamers/{streamer_id}/filters/{id}",
    tag = "filters",
    params(
        ("streamer_id" = String, Path, description = "Streamer ID"),
        ("id" = String, Path, description = "Filter ID")
    ),
    responses(
        (status = 200, description = "Filter details", body = FilterResponse),
        (status = 404, description = "Filter not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_filter(
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

#[utoipa::path(
    patch,
    path = "/api/streamers/{streamer_id}/filters/{id}",
    tag = "filters",
    params(
        ("streamer_id" = String, Path, description = "Streamer ID"),
        ("id" = String, Path, description = "Filter ID")
    ),
    request_body = UpdateFilterRequest,
    responses(
        (status = 200, description = "Filter updated", body = FilterResponse),
        (status = 404, description = "Filter not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_filter(
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

    let existing_type = FilterType::parse(&filter.filter_type).ok_or_else(|| {
        ApiError::validation(format!(
            "Invalid stored filter type: {}",
            filter.filter_type
        ))
    })?;

    let (target_type, type_changed) = match request.filter_type.as_deref() {
        Some(ft_str) => {
            let parsed = FilterType::parse(ft_str)
                .ok_or_else(|| ApiError::validation(format!("Invalid filter type: {}", ft_str)))?;
            (parsed, parsed.as_str() != filter.filter_type.as_str())
        }
        None => (existing_type, false),
    };

    if type_changed && request.config.is_none() {
        return Err(ApiError::validation(
            "config is required when changing filter_type".to_string(),
        ));
    }

    // Apply type update first (canonical string form).
    if request.filter_type.is_some() {
        filter.filter_type = target_type.as_str().to_string();
    }

    // Validate + normalize config if provided.
    if let Some(config_value) = request.config {
        filter.config = validate_and_serialize_config(target_type, config_value)?;
    }

    // Save updates
    filter_repo
        .update_filter(&filter)
        .await
        .map_err(ApiError::from)?;

    if let Some(config_service) = &state.config_service {
        config_service.notify_streamer_filters_updated(&streamer_id);
    }

    model_to_response(&filter).map(Json)
}

#[utoipa::path(
    delete,
    path = "/api/streamers/{streamer_id}/filters/{id}",
    tag = "filters",
    params(
        ("streamer_id" = String, Path, description = "Streamer ID"),
        ("id" = String, Path, description = "Filter ID")
    ),
    responses(
        (status = 200, description = "Filter deleted", body = crate::api::openapi::MessageResponse),
        (status = 404, description = "Filter not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_filter(
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

    if let Some(config_service) = &state.config_service {
        config_service.notify_streamer_filters_updated(&streamer_id);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Filter {} deleted", id)
    })))
}
