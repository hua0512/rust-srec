//! Template management routes.

use axum::{
    extract::{Path, Query, State},
    routing::{delete, get, patch, post},
    Json, Router,
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    CreateTemplateRequest, PaginatedResponse, PaginationParams, TemplateResponse,
    UpdateTemplateRequest,
};
use crate::api::server::AppState;

/// Create the templates router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", post(create_template))
        .route("/", get(list_templates))
        .route("/:id", get(get_template))
        .route("/:id", patch(update_template))
        .route("/:id", delete(delete_template))
}

/// Create a new template.
async fn create_template(
    State(state): State<AppState>,
    Json(request): Json<CreateTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    // Validate name
    if request.name.is_empty() {
        return Err(ApiError::validation("Template name cannot be empty"));
    }

    // TODO: Implement actual creation logic using ConfigService
    let response = TemplateResponse {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        output_folder: request.output_folder,
        output_filename_template: request.output_filename_template,
        output_file_format: request.output_file_format,
        download_engine: request.download_engine,
        record_danmu: request.record_danmu,
        platform_overrides: request.platform_overrides,
        engines_override: request.engines_override,
        usage_count: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    Ok(Json(response))
}

/// List all templates.
async fn list_templates(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<TemplateResponse>>> {
    // TODO: Implement actual listing logic using ConfigService
    let response = PaginatedResponse::new(Vec::new(), 0, pagination.limit, pagination.offset);

    Ok(Json(response))
}

/// Get a single template by ID.
async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<TemplateResponse>> {
    // TODO: Implement actual retrieval logic using ConfigService
    Err(ApiError::not_found(format!("Template with id '{}' not found", id)))
}

/// Update a template.
async fn update_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTemplateRequest>,
) -> ApiResult<Json<TemplateResponse>> {
    // TODO: Implement actual update logic using ConfigService
    Err(ApiError::not_found(format!("Template with id '{}' not found", id)))
}

/// Delete a template.
async fn delete_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // TODO: Implement actual deletion logic using ConfigService
    // Should check if any streamers are using this template
    Err(ApiError::not_found(format!("Template with id '{}' not found", id)))
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
}
