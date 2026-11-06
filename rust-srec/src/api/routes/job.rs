//! Job preset management routes.
//!
//! This module provides REST API endpoints for managing job presets,
//! which are reusable configurations for individual pipeline processors.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/job/presets` | List job presets with optional category filter |
//! | GET | `/api/job/presets/{id}` | Get a single job preset by ID |
//! | POST | `/api/job/presets` | Create a new job preset |
//! | PUT | `/api/job/presets/{id}` | Update a job preset |
//! | DELETE | `/api/job/presets/{id}` | Delete a job preset |
//! | POST | `/api/job/presets/{id}/clone` | Clone a job preset |

use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::StatusCode,
    routing::{get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::models::PaginationParams;
use crate::api::server::AppState;

#[derive(Clone)]
pub struct JobPresetRouteState {
    pipeline_manager: std::sync::Arc<crate::pipeline::PipelineManager>,
}

impl FromRef<AppState> for JobPresetRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            pipeline_manager: state.pipeline_manager.clone(),
        }
    }
}

/// Create the job router.
///
/// # Routes
///
/// - `GET /presets` - List job presets with optional category filter
/// - `GET /presets/{id}` - Get a single job preset by ID
/// - `POST /presets` - Create a new job preset
/// - `PUT /presets/{id}` - Update a job preset
/// - `DELETE /presets/{id}` - Delete a job preset
/// - `POST /presets/{id}/clone` - Clone a job preset
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/presets", get(list_presets).post(create_preset))
        .route(
            "/presets/{id}",
            get(get_preset).put(update_preset).delete(delete_preset),
        )
        .route("/presets/{id}/clone", post(clone_preset))
}

/// Request body for creating a new job preset.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CreatePresetRequest {
    /// Unique ID for the preset.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Category for organizing presets (e.g., "remux", "compression", "thumbnail").
    pub category: Option<String>,
    /// Processor type (e.g., "remux", "upload").
    pub processor: String,
    /// Processor-specific configuration (JSON string).
    pub config: String,
}

/// Request body for updating a job preset.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdatePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Category for organizing presets.
    pub category: Option<String>,
    /// Processor type.
    pub processor: String,
    /// Processor-specific configuration.
    pub config: String,
}

/// Request body for cloning a job preset.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct ClonePresetRequest {
    /// New name for the cloned preset.
    pub new_name: String,
}

/// Query parameters for filtering presets.
#[derive(Debug, Clone, serde::Deserialize, Default, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PresetFilterParams {
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by processor type.
    pub processor: Option<String>,
    /// Exact preset name. Returns at most one preset (`job_presets.name` is UNIQUE); use this
    /// to resolve a preset a pipeline step references by name.
    pub name: Option<String>,
    /// Literal substring of name or description; ASCII case-insensitive.
    pub search: Option<String>,
}

/// Response for preset list with categories and pagination.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct PresetListResponse {
    /// List of presets.
    pub presets: Vec<crate::database::models::JobPreset>,
    /// Available categories.
    pub categories: Vec<String>,
    /// Total number of presets matching the filter.
    pub total: u64,
    /// Number of items returned.
    pub limit: u32,
    /// Number of items skipped.
    pub offset: u32,
}

#[utoipa::path(
    get,
    path = "/api/job/presets",
    tag = "job",
    params(PresetFilterParams, PaginationParams),
    responses(
        (status = 200, description = "List of job presets", body = PresetListResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_presets(
    State(state): State<JobPresetRouteState>,
    ApiQuery(filters): ApiQuery<PresetFilterParams>,
    ApiQuery(pagination): ApiQuery<PaginationParams>,
) -> ApiResult<Json<PresetListResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    let db_filters = crate::database::repositories::JobPresetFilters {
        category: filters.category,
        processor: filters.processor,
        name: filters.name,
        search: filters.search,
    };

    let effective_limit = pagination.effective_limit();
    let db_pagination =
        crate::database::models::Pagination::new(effective_limit, pagination.offset);

    let (presets, total) = pipeline_manager
        .list_presets_filtered(&db_filters, &db_pagination)
        .await?;

    let categories = pipeline_manager.list_preset_categories().await?;

    Ok(Json(PresetListResponse {
        presets,
        categories,
        total,
        limit: effective_limit,
        offset: pagination.offset,
    }))
}

#[utoipa::path(
    get,
    path = "/api/job/presets/{id}",
    tag = "job",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Job preset", body = crate::database::models::JobPreset),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_preset(
    State(state): State<JobPresetRouteState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = &state.pipeline_manager;

    let preset = pipeline_manager
        .get_preset(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Job preset {} not found", id)))?;

    Ok(Json(preset))
}

#[utoipa::path(
    post,
    path = "/api/job/presets",
    tag = "job",
    request_body = CreatePresetRequest,
    responses(
        (status = 201, description = "Preset created", body = crate::database::models::JobPreset),
        (status = 409, description = "Preset name already exists", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_preset(
    State(state): State<JobPresetRouteState>,
    ApiJson(payload): ApiJson<CreatePresetRequest>,
) -> ApiResult<(StatusCode, Json<crate::database::models::JobPreset>)> {
    let pipeline_manager = &state.pipeline_manager;

    let preset = crate::database::models::JobPreset {
        id: payload.id,
        name: payload.name,
        description: payload.description,
        category: payload.category,
        processor: payload.processor,
        config: payload.config,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Validate the preset
    preset
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // Check for duplicate name
    if pipeline_manager.name_exists(&preset.name, None).await? {
        return Err(ApiError::conflict(format!(
            "A preset with name '{}' already exists",
            preset.name
        )));
    }

    pipeline_manager.create_preset(&preset).await?;

    Ok((StatusCode::CREATED, Json(preset)))
}

#[utoipa::path(
    put,
    path = "/api/job/presets/{id}",
    tag = "job",
    params(("id" = String, Path, description = "Preset ID")),
    request_body = UpdatePresetRequest,
    responses(
        (status = 200, description = "Preset updated", body = crate::database::models::JobPreset),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse),
        (status = 409, description = "CONFLICT: the name is taken; PRESET_IN_USE: a renamed preset is still referenced by a stored pipeline", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_preset(
    State(state): State<JobPresetRouteState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(payload): ApiJson<UpdatePresetRequest>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = &state.pipeline_manager;

    // Check if preset exists
    let existing = pipeline_manager
        .get_preset(&id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("Job preset {} not found", id)))?;

    let preset = crate::database::models::JobPreset {
        id: id.clone(),
        name: payload.name,
        description: payload.description,
        category: payload.category,
        processor: payload.processor,
        config: payload.config,
        created_at: existing.created_at, // Preserve original creation time
        updated_at: chrono::Utc::now(),
    };

    // Validate the preset
    preset
        .validate()
        .map_err(|e| ApiError::bad_request(e.to_string()))?;

    // Check for duplicate name (excluding current preset)
    if pipeline_manager
        .name_exists(&preset.name, Some(&id))
        .await?
    {
        return Err(ApiError::conflict(format!(
            "A preset with name '{}' already exists",
            preset.name
        )));
    }

    pipeline_manager.update_preset(&preset).await?;

    Ok(Json(preset))
}

#[utoipa::path(
    delete,
    path = "/api/job/presets/{id}",
    tag = "job",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Preset deleted"),
        (status = 409, description = "PRESET_IN_USE: a stored pipeline references the preset by name", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_preset(
    State(state): State<JobPresetRouteState>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<()>> {
    let pipeline_manager = &state.pipeline_manager;

    pipeline_manager.delete_preset(&id).await?;
    Ok(Json(()))
}

#[utoipa::path(
    post,
    path = "/api/job/presets/{id}/clone",
    tag = "job",
    params(("id" = String, Path, description = "Preset ID to clone")),
    request_body = ClonePresetRequest,
    responses(
        (status = 201, description = "Preset cloned", body = crate::database::models::JobPreset),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn clone_preset(
    State(state): State<JobPresetRouteState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(payload): ApiJson<ClonePresetRequest>,
) -> ApiResult<(StatusCode, Json<crate::database::models::JobPreset>)> {
    let pipeline_manager = &state.pipeline_manager;

    let cloned = pipeline_manager.clone_preset(&id, payload.new_name).await?;

    Ok((StatusCode::CREATED, Json(cloned)))
}
