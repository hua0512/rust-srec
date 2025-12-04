//! Template management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, patch, post},
};
use chrono::Utc;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateTemplateRequest, PaginatedResponse, PaginationParams, TemplateResponse,
    UpdateTemplateRequest,
};
use crate::api::server::AppState;
use crate::database::models::TemplateConfigDbModel;

/// Create the templates router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_template))
        .route("/", get(list_templates))
        .route("/{id}", get(get_template))
        .route("/{id}", patch(update_template))
        .route("/{id}", delete(delete_template))
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
        platform_overrides: model
            .platform_overrides
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
        engines_override: model
            .engines_override
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok()),
        usage_count,
        created_at: Utc::now(), // Not stored in DB model
        updated_at: Utc::now(), // Not stored in DB model
    }
}

/// Create a new template.
///
/// POST /api/templates
async fn create_template(
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
    template.platform_overrides = request
        .platform_overrides
        .map(|v| serde_json::to_string(&v).unwrap_or_default());
    template.engines_override = request
        .engines_override
        .map(|v| serde_json::to_string(&v).unwrap_or_default());

    // Create the template
    config_service
        .create_template_config(&template)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to create template: {}", e)))?;

    Ok(Json(db_model_to_response(&template, 0)))
}

/// List all templates.
///
/// GET /api/templates
async fn list_templates(
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
    let limit = pagination.limit.min(100) as usize;

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

    let response = PaginatedResponse::new(templates, total, pagination.limit, pagination.offset);
    Ok(Json(response))
}

/// Get a single template by ID.
///
/// GET /api/templates/:id
async fn get_template(
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
    let template = config_service
        .get_template_config(&id)
        .await
        .map_err(|e| {
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

/// Update a template.
///
/// PATCH /api/templates/:id
async fn update_template(
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
    let mut template = config_service
        .get_template_config(&id)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::not_found(format!("Template with id '{}' not found", id))
            } else {
                ApiError::internal(format!("Failed to get template: {}", e))
            }
        })?;

    // Apply partial updates
    if let Some(name) = request.name {
        if name.is_empty() {
            return Err(ApiError::validation("Template name cannot be empty"));
        }
        template.name = name;
    }
    if let Some(output_folder) = request.output_folder {
        template.output_folder = Some(output_folder);
    }
    if let Some(output_filename_template) = request.output_filename_template {
        template.output_filename_template = Some(output_filename_template);
    }
    if let Some(output_file_format) = request.output_file_format {
        template.output_file_format = Some(output_file_format);
    }
    if let Some(download_engine) = request.download_engine {
        template.download_engine = Some(download_engine);
    }
    if let Some(record_danmu) = request.record_danmu {
        template.record_danmu = Some(record_danmu);
    }
    if let Some(platform_overrides) = request.platform_overrides {
        template.platform_overrides = Some(serde_json::to_string(&platform_overrides).unwrap_or_default());
    }
    if let Some(engines_override) = request.engines_override {
        template.engines_override = Some(serde_json::to_string(&engines_override).unwrap_or_default());
    }

    // Update the template
    config_service
        .update_template_config(&template)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to update template: {}", e)))?;

    // Count streamers using this template
    let usage_count = streamer_manager
        .map(|sm| sm.get_by_template(&id).len() as u32)
        .unwrap_or(0);

    Ok(Json(db_model_to_response(&template, usage_count)))
}

/// Delete a template.
///
/// DELETE /api/templates/:id
async fn delete_template(
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
    config_service
        .get_template_config(&id)
        .await
        .map_err(|e| {
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
