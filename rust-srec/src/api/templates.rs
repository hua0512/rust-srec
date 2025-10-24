use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::{
    api::{AppState, streamers::ErrorResponse},
    domain::template_config::TemplateConfig,
};

pub fn templates_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_templates))
        .route("/", post(create_template))
        .route("/:id", get(get_template))
        .route("/:id", put(update_template))
        .route("/:id", delete(delete_template))
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateTemplate {
    pub name: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateTemplate {
    pub name: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/templates",
    responses(
        (status = 200, description = "List all templates successfully", body = [TemplateConfig]),
        (status = 500, description = "Failed to list templates", body = ErrorResponse)
    )
)]
async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<TemplateConfig>>, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.template_configs().find_all().await {
        Ok(templates) => Ok(Json(templates)),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to list templates: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    post,
    path = "/api/templates",
    request_body = CreateTemplate,
    responses(
        (status = 201, description = "Template created successfully", body = TemplateConfig),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 500, description = "Failed to create template", body = ErrorResponse)
    )
)]
async fn create_template(
    State(state): State<AppState>,
    Json(payload): Json<CreateTemplate>,
) -> Result<(StatusCode, Json<TemplateConfig>), (StatusCode, Json<ErrorResponse>)> {
    let template = TemplateConfig {
        id: Uuid::new_v4().to_string(),
        name: payload.name,
        output_folder: None,
        output_filename_template: None,
        max_bitrate: None,
        cookies: None,
        output_file_format: None,
        min_segment_size_bytes: None,
        max_download_duration_secs: None,
        max_part_size_bytes: None,
        record_danmu: None,
        platform_overrides: None,
        download_retry_policy: None,
        danmu_sampling_config: None,
        download_engine: None,
        engines_override: None,
        proxy_config: None,
        event_hooks: None,
    };

    match state.db_service.template_configs().create(&template).await {
        Ok(_) => Ok((StatusCode::CREATED, Json(template))),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to create template: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    get,
    path = "/api/templates/{id}",
    params(
        ("id" = String, Path, description = "Template id")
    ),
    responses(
        (status = 200, description = "Template found", body = TemplateConfig),
        (status = 404, description = "Template not found", body = ErrorResponse),
        (status = 500, description = "Failed to get template", body = ErrorResponse)
    )
)]
async fn get_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<TemplateConfig>, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.template_configs().find_by_id(&id).await {
        Ok(Some(template)) => Ok(Json(template)),
        Ok(None) => {
            let error_response = ErrorResponse {
                message: format!("Template with id {} not found", id),
            };
            Err((StatusCode::NOT_FOUND, Json(error_response)))
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to get template: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    put,
    path = "/api/templates/{id}",
    params(
        ("id" = String, Path, description = "Template id")
    ),
    request_body = UpdateTemplate,
    responses(
        (status = 200, description = "Template updated successfully", body = TemplateConfig),
        (status = 400, description = "Invalid input", body = ErrorResponse),
        (status = 404, description = "Template not found", body = ErrorResponse),
        (status = 500, description = "Failed to update template", body = ErrorResponse)
    )
)]
async fn update_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateTemplate>,
) -> Result<Json<TemplateConfig>, (StatusCode, Json<ErrorResponse>)> {
    let mut template = match state.db_service.template_configs().find_by_id(&id).await {
        Ok(Some(template)) => template,
        Ok(None) => {
            let error_response = ErrorResponse {
                message: format!("Template with id {} not found", id),
            };
            return Err((StatusCode::NOT_FOUND, Json(error_response)));
        }
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to get template: {}", e),
            };
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)));
        }
    };

    if let Some(name) = payload.name {
        template.name = name;
    }

    match state.db_service.template_configs().update(&template).await {
        Ok(_) => Ok(Json(template)),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to update template: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/api/templates/{id}",
    params(
        ("id" = String, Path, description = "Template id")
    ),
    responses(
        (status = 204, description = "Template deleted successfully"),
        (status = 404, description = "Template not found", body = ErrorResponse),
        (status = 500, description = "Failed to delete template", body = ErrorResponse)
    )
)]
async fn delete_template(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    match state.db_service.template_configs().delete(&id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(e) => {
            let error_response = ErrorResponse {
                message: format!("Failed to delete template: {}", e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)))
        }
    }
}