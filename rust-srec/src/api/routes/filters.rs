//! Filter management routes.

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
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
use crate::database::repositories::filter::FilterCommitHook;

#[derive(Clone)]
pub struct FilterRouteState {
    filter_repository: std::sync::Arc<dyn crate::database::repositories::FilterRepository>,
    config_service: std::sync::Arc<
        crate::config::ConfigService<
            crate::database::repositories::config::SqlxConfigRepository,
            crate::database::repositories::streamer::SqlxStreamerRepository,
        >,
    >,
}

impl FromRef<AppState> for FilterRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            filter_repository: state.filter_repository.clone(),
            config_service: state.config_service.clone(),
        }
    }
}

impl FilterRouteState {
    fn commit_hook(&self) -> FilterCommitHook {
        let config_service = self.config_service.clone();
        Box::new(move |owners| {
            for owner in owners {
                config_service.notify_streamer_filters_updated(owner);
            }
        })
    }
}

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
    let config: Value = serde_json::from_str(&model.config).map_err(ApiError::from)?;

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
            let mut typed: TimeBasedFilterConfig =
                serde_json::from_value(config.clone()).map_err(|e| {
                    ApiError::validation(format!("Invalid time-based filter config: {}", e))
                })?;
            typed.normalize();
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serialize_preserving_extensions(
                config,
                serde_json::to_value(&typed).map_err(ApiError::from)?,
            )
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
            let typed: CronFilterConfig = serde_json::from_value(config.clone())
                .map_err(|e| ApiError::validation(format!("Invalid cron filter config: {}", e)))?;
            typed
                .validate()
                .map_err(|e| ApiError::validation(e.to_string()))?;
            serialize_preserving_extensions(
                config,
                serde_json::to_value(&typed).map_err(ApiError::from)?,
            )
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

fn serialize_preserving_extensions(mut original: Value, normalized: Value) -> ApiResult<String> {
    if let (Value::Object(original), Value::Object(normalized)) = (&mut original, normalized) {
        original.extend(normalized);
    }
    serde_json::to_string(&original).map_err(ApiError::from)
}

/// Older TimeBased editors send complete schedule objects but omit timezone.
/// Only an absent member preserves the existing zone; explicit null selects UTC.
fn preserve_omitted_timezone(
    existing: &FilterDbModel,
    target: FilterType,
    replacement: &mut Value,
) {
    if existing.filter_type != FilterType::TimeBased.as_str() || target != FilterType::TimeBased {
        return;
    }
    if let Value::Object(replacement) = replacement
        && !replacement.contains_key("timezone")
        && let Ok(Value::Object(stored)) = serde_json::from_str(&existing.config)
        && let Some(timezone) = stored.get("timezone")
    {
        replacement.insert("timezone".into(), timezone.clone());
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
    State(state): State<FilterRouteState>,
    Path(streamer_id): Path<String>,
) -> ApiResult<Json<Vec<FilterResponse>>> {
    let filter_repo = &state.filter_repository;

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
    State(state): State<FilterRouteState>,
    Path(streamer_id): Path<String>,
    Json(request): Json<CreateFilterRequest>,
) -> ApiResult<(StatusCode, Json<FilterResponse>)> {
    let filter_repo = &state.filter_repository;

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
        .create_filter_with_commit_hook(&filter, state.commit_hook())
        .await
        .map_err(ApiError::from)?;

    model_to_response(&filter).map(|response| (StatusCode::CREATED, Json(response)))
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
    State(state): State<FilterRouteState>,
    Path((streamer_id, id)): Path<(String, String)>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = &state.filter_repository;

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
        (status = 404, description = "Filter not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "Filter changed concurrently; retry the request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_filter(
    State(state): State<FilterRouteState>,
    Path((streamer_id, id)): Path<(String, String)>,
    Json(request): Json<UpdateFilterRequest>,
) -> ApiResult<Json<FilterResponse>> {
    let filter_repo = &state.filter_repository;

    // A replacement may preserve stored members. Rebase the original request on
    // each fresh row so a concurrent update cannot restore an older timezone.
    for _ in 0..3 {
        let existing = filter_repo.get_filter(&id).await.map_err(ApiError::from)?;
        if existing.streamer_id != streamer_id {
            return Err(ApiError::not_found(format!(
                "Filter {} not found for streamer {}",
                id, streamer_id
            )));
        }

        let existing_type = FilterType::parse(&existing.filter_type).ok_or_else(|| {
            ApiError::validation(format!(
                "Invalid stored filter type: {}",
                existing.filter_type
            ))
        })?;

        let target_type = match request.filter_type.as_deref() {
            Some(ft_str) => FilterType::parse(ft_str)
                .ok_or_else(|| ApiError::validation(format!("Invalid filter type: {}", ft_str)))?,
            None => existing_type,
        };
        let type_changed = target_type != existing_type;

        if type_changed && request.config.is_none() {
            return Err(ApiError::validation(
                "config is required when changing filter_type".to_string(),
            ));
        }

        let mut filter = existing.clone();
        filter.filter_type = target_type.as_str().to_string();

        if let Some(mut config_value) = request.config.clone() {
            preserve_omitted_timezone(&existing, target_type, &mut config_value);
            filter.config = validate_and_serialize_config(target_type, config_value)?;
        }

        if filter_repo
            .update_filter_if_current(&existing, &filter, state.commit_hook())
            .await
            .map_err(ApiError::from)?
        {
            return model_to_response(&filter).map(Json);
        }
    }

    Err(ApiError::conflict(
        "Filter changed during the update; retry the request",
    ))
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
    State(state): State<FilterRouteState>,
    Path((streamer_id, id)): Path<(String, String)>,
) -> ApiResult<Json<serde_json::Value>> {
    let filter_repo = &state.filter_repository;

    // Check ownership before deleting
    let filter = filter_repo.get_filter(&id).await.map_err(ApiError::from)?;

    if filter.streamer_id != streamer_id {
        return Err(ApiError::not_found(format!(
            "Filter {} not found for streamer {}",
            id, streamer_id
        )));
    }

    filter_repo
        .delete_filter_with_commit_hook(&id, state.commit_hook())
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Filter {} deleted", id)
    })))
}

#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod timezone_tests;
