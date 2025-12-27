//! Template management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post, put},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateTemplateRequest, PaginatedResponse, PaginationParams, TemplateResponse,
    UpdateTemplateRequest,
};
use crate::api::server::AppState;
use crate::database::models::TemplateConfigDbModel;
use crate::utils::json::{self, JsonContext};
use tracing::info;

/// Request to clone a template.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CloneTemplateRequest {
    /// New name for the cloned template.
    pub new_name: String,
}

/// Create the templates router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_template))
        .route("/", get(list_templates))
        .route("/{id}", get(get_template))
        .route("/{id}", put(update_template))
        .route("/{id}", delete(delete_template))
        .route("/{id}/clone", post(clone_template))
}

/// Convert TemplateConfigDbModel to TemplateResponse.
fn db_model_to_response(model: &TemplateConfigDbModel, usage_count: u32) -> TemplateResponse {
    TemplateResponse {
        id: model.id.clone(),
        name: model.name.clone(),
        output_folder: model.output_folder.clone(),
        output_filename_template: model.output_filename_template.clone(),
        output_file_format: model.output_file_format.clone(),
        download_engine: model.download_engine.clone(),
        record_danmu: model.record_danmu,
        platform_overrides: json::parse_optional_value_non_null(
            model.platform_overrides.as_deref(),
            JsonContext::TemplateField {
                template_id: &model.id,
                field: "platform_overrides",
            },
            "Invalid template JSON field; omitting from response",
        ),
        engines_override: json::parse_optional_value_non_null(
            model.engines_override.as_deref(),
            JsonContext::TemplateField {
                template_id: &model.id,
                field: "engines_override",
            },
            "Invalid template JSON field; omitting from response",
        ),
        stream_selection_config: model.stream_selection_config.clone(),
        cookies: model.cookies.clone(),
        min_segment_size_bytes: model.min_segment_size_bytes,
        max_download_duration_secs: model.max_download_duration_secs,
        max_part_size_bytes: model.max_part_size_bytes,
        download_retry_policy: model.download_retry_policy.clone(),
        danmu_sampling_config: model.danmu_sampling_config.clone(),
        proxy_config: model.proxy_config.clone(),
        event_hooks: model.event_hooks.clone(),
        pipeline: model.pipeline.clone(),
        session_complete_pipeline: model.session_complete_pipeline.clone(),
        paired_segment_pipeline: model.paired_segment_pipeline.clone(),
        usage_count,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

#[utoipa::path(
    post,
    path = "/api/templates",
    tag = "templates",
    request_body = CreateTemplateRequest,
    responses(
        (status = 201, description = "Template created", body = TemplateResponse),
        (status = 422, description = "Validation error", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_template(
    State(state): State<AppState>,
    Json(request): Json<CreateTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    // Validate name
    if request.name.is_empty() {
        return Err(ApiError::validation("Template name cannot be empty"));
    }

    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Create the template config model
    let mut template = TemplateConfigDbModel::new(&request.name);
    template.output_folder = request.output_folder;
    template.output_filename_template = request.output_filename_template;
    template.output_file_format = request.output_file_format;
    template.download_engine = request.download_engine;
    template.record_danmu = request.record_danmu;

    template.platform_overrides = match request.platform_overrides {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| {
            ApiError::internal(format!("Failed to serialize platform_overrides: {e}"))
        })?),
        None => None,
    };
    template.engines_override = match request.engines_override {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| {
            ApiError::internal(format!("Failed to serialize engines_override: {e}"))
        })?),
        None => None,
    };
    template.stream_selection_config = request.stream_selection_config;
    template.cookies = request.cookies;
    template.min_segment_size_bytes = request.min_segment_size_bytes;
    template.max_download_duration_secs = request.max_download_duration_secs;
    template.max_part_size_bytes = request.max_part_size_bytes;
    template.download_retry_policy = request.download_retry_policy;
    template.danmu_sampling_config = request.danmu_sampling_config;
    template.proxy_config = request.proxy_config;
    template.event_hooks = request.event_hooks;
    template.pipeline = request.pipeline;
    template.session_complete_pipeline = request.session_complete_pipeline;
    template.paired_segment_pipeline = request.paired_segment_pipeline;

    // Create the template
    config_service
        .create_template_config(&template)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create template: {}", e)))?;

    Ok(Json(db_model_to_response(&template, 0)))
}

#[utoipa::path(
    get,
    path = "/api/templates",
    tag = "templates",
    params(PaginationParams),
    responses(
        (status = 200, description = "List of templates", body = PaginatedResponse<TemplateResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_templates(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<TemplateResponse>>> {
    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get streamer manager to count usage
    let streamer_manager = state.streamer_manager.as_ref();

    // Get all templates
    let templates = config_service
        .list_template_configs()
        .await
        .map_err(|e| ApiError::internal(format!("Failed to list templates: {}", e)))?;

    let total = templates.len() as u64;

    // Apply pagination
    let offset = pagination.offset as usize;
    let effective_limit = pagination.limit.min(100);
    let limit = effective_limit as usize;

    let templates: Vec<TemplateResponse> = templates
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|t| {
            // Count streamers using this template
            let usage_count = streamer_manager
                .map(|sm| sm.get_by_template(&t.id).len() as u32)
                .unwrap_or(0);
            db_model_to_response(&t, usage_count)
        })
        .collect();

    let response = PaginatedResponse::new(templates, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

#[utoipa::path(
    get,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template details", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateResponse>> {
    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get streamer manager to count usage
    let streamer_manager = state.streamer_manager.as_ref();

    // Get the template
    let template = config_service.get_template_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Template with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get template: {}", e))
        }
    })?;

    // Count streamers using this template
    let usage_count = streamer_manager
        .map(|sm| sm.get_by_template(&id).len() as u32)
        .unwrap_or(0);

    Ok(Json(db_model_to_response(&template, usage_count)))
}

#[utoipa::path(
    put,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    request_body = UpdateTemplateRequest,
    responses(
        (status = 200, description = "Template updated", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get streamer manager to count usage
    let streamer_manager = state.streamer_manager.as_ref();

    // Get the existing template
    let mut template = config_service.get_template_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Template with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get template: {}", e))
        }
    })?;

    // Replace all fields (PUT semantics)
    if let Some(name) = request.name {
        if name.is_empty() {
            return Err(ApiError::validation("Template name cannot be empty"));
        }
        template.name = name;
    }

    // For optional fields, direct assignment handles both Some(v) and None (clearing)
    template.output_folder = request.output_folder;
    template.output_filename_template = request.output_filename_template;
    template.output_file_format = request.output_file_format;
    template.download_engine = request.download_engine;
    template.record_danmu = request.record_danmu;
    template.platform_overrides = match request.platform_overrides {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| {
            ApiError::internal(format!("Failed to serialize platform_overrides: {e}"))
        })?),
        None => None,
    };
    template.engines_override = match request.engines_override {
        Some(v) if v.is_null() => None,
        Some(v) => Some(serde_json::to_string(&v).map_err(|e| {
            ApiError::internal(format!("Failed to serialize engines_override: {e}"))
        })?),
        None => None,
    };
    template.stream_selection_config = request.stream_selection_config;
    template.cookies = request.cookies;
    template.min_segment_size_bytes = request.min_segment_size_bytes;
    template.max_download_duration_secs = request.max_download_duration_secs;
    template.max_part_size_bytes = request.max_part_size_bytes;
    template.download_retry_policy = request.download_retry_policy;
    template.danmu_sampling_config = request.danmu_sampling_config;
    template.proxy_config = request.proxy_config;
    template.event_hooks = request.event_hooks;
    template.pipeline = request.pipeline;
    template.session_complete_pipeline = request.session_complete_pipeline;
    template.paired_segment_pipeline = request.paired_segment_pipeline;

    // Update the template
    config_service
        .update_template_config(&template)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update template: {}", e)))?;

    info!("Updated template '{}' (id: {})", template.name, id);

    // Count streamers using this template
    let usage_count = streamer_manager
        .map(|sm| sm.get_by_template(&id).len() as u32)
        .unwrap_or(0);

    Ok(Json(db_model_to_response(&template, usage_count)))
}

#[utoipa::path(
    delete,
    path = "/api/templates/{id}",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID")),
    responses(
        (status = 200, description = "Template deleted", body = crate::api::openapi::MessageResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "Template in use", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get streamer manager to check usage
    let streamer_manager = state.streamer_manager.as_ref();

    // Check if template exists
    config_service.get_template_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Template with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get template: {}", e))
        }
    })?;

    // Check if any streamers are using this template
    if let Some(sm) = streamer_manager {
        let streamers_using = sm.get_by_template(&id);
        if !streamers_using.is_empty() {
            return Err(ApiError::conflict(format!(
                "Cannot delete template '{}': {} streamer(s) are using it",
                id,
                streamers_using.len()
            )));
        }
    }

    // Delete the template
    config_service
        .delete_template_config(&id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to delete template: {}", e)))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Template '{}' deleted successfully", id)
    })))
}

#[utoipa::path(
    post,
    path = "/api/templates/{id}/clone",
    tag = "templates",
    params(("id" = String, Path, description = "Template ID to clone")),
    request_body = CloneTemplateRequest,
    responses(
        (status = 201, description = "Template cloned", body = TemplateResponse),
        (status = 404, description = "Template not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "Template name already exists", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn clone_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<CloneTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    // Validate new name
    if request.new_name.is_empty() {
        return Err(ApiError::validation("Template name cannot be empty"));
    }

    // Get config service from state
    let config_service = state
        .config_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Config service not available"))?;

    // Get the existing template
    let existing = config_service.get_template_config(&id).await.map_err(|e| {
        if e.to_string().contains("not found") {
            ApiError::not_found(format!("Template with id '{}' not found", id))
        } else {
            ApiError::internal(format!("Failed to get template: {}", e))
        }
    })?;

    // Check if a template with the new name already exists
    if config_service
        .get_template_config_by_name(&request.new_name)
        .await
        .is_ok()
    {
        return Err(ApiError::conflict(format!(
            "A template with name '{}' already exists",
            request.new_name
        )));
    }

    // Create the cloned template with a new ID and name
    let mut cloned = TemplateConfigDbModel::new(&request.new_name);
    cloned.output_folder = existing.output_folder;
    cloned.output_filename_template = existing.output_filename_template;
    cloned.output_file_format = existing.output_file_format;
    cloned.download_engine = existing.download_engine;
    cloned.record_danmu = existing.record_danmu;
    cloned.platform_overrides = existing.platform_overrides;
    cloned.engines_override = existing.engines_override;
    cloned.stream_selection_config = existing.stream_selection_config;
    cloned.cookies = existing.cookies;
    cloned.min_segment_size_bytes = existing.min_segment_size_bytes;
    cloned.max_download_duration_secs = existing.max_download_duration_secs;
    cloned.max_part_size_bytes = existing.max_part_size_bytes;
    cloned.download_retry_policy = existing.download_retry_policy;
    cloned.danmu_sampling_config = existing.danmu_sampling_config;
    cloned.proxy_config = existing.proxy_config;
    cloned.event_hooks = existing.event_hooks;
    cloned.pipeline = existing.pipeline;
    cloned.session_complete_pipeline = existing.session_complete_pipeline;
    cloned.paired_segment_pipeline = existing.paired_segment_pipeline;

    // Create the cloned template
    config_service
        .create_template_config(&cloned)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create cloned template: {}", e)))?;

    info!(
        "Cloned template '{}' (id: {}) to '{}' (id: {})",
        existing.name, id, cloned.name, cloned.id
    );

    Ok(Json(db_model_to_response(&cloned, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_response_serialization() {
        let response = TemplateResponse {
            id: "123".to_string(),
            name: "Test Template".to_string(),
            output_folder: Some("/downloads".to_string()),
            output_filename_template: None,
            output_file_format: Some("mp4".to_string()),
            download_engine: None,
            record_danmu: Some(true),
            platform_overrides: None,
            engines_override: None,
            stream_selection_config: None,
            cookies: None,
            min_segment_size_bytes: None,
            max_download_duration_secs: None,
            max_part_size_bytes: None,
            download_retry_policy: None,
            danmu_sampling_config: None,
            proxy_config: None,
            event_hooks: None,
            pipeline: None,
            session_complete_pipeline: None,
            paired_segment_pipeline: None,
            usage_count: 5,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("Test Template"));
        assert!(json.contains("mp4"));
    }

    #[test]
    fn test_db_model_to_response() {
        let model = TemplateConfigDbModel::new("test");
        let response = db_model_to_response(&model, 3);

        assert_eq!(response.name, "test");
        assert_eq!(response.usage_count, 3);
    }
}
