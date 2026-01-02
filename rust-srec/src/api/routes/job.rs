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
    extract::{Path, Query, State},
    routing::{get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

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
pub struct PresetFilterParams {
    /// Filter by category.
    pub category: Option<String>,
    /// Filter by processor type.
    pub processor: Option<String>,
    /// Search query (matches name or description).
    pub search: Option<String>,
}

/// Pagination parameters for preset list.
#[derive(Debug, Clone, serde::Deserialize, utoipa::IntoParams)]
pub struct PresetPaginationParams {
    /// Number of items to return (default: 20, max: 100).
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Number of items to skip.
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    20
}

impl Default for PresetPaginationParams {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            offset: 0,
        }
    }
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
    params(PresetFilterParams, PresetPaginationParams),
    responses(
        (status = 200, description = "List of job presets", body = PresetListResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_presets(
    State(state): State<AppState>,
    Query(filters): Query<PresetFilterParams>,
    Query(pagination): Query<PresetPaginationParams>,
) -> ApiResult<Json<PresetListResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let db_filters = crate::database::repositories::JobPresetFilters {
        category: filters.category,
        processor: filters.processor,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination =
        crate::database::models::Pagination::new(effective_limit, pagination.offset);

    let (presets, total) = pipeline_manager
        .list_presets_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let categories = pipeline_manager
        .list_preset_categories()
        .await
        .map_err(ApiError::from)?;

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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let preset = pipeline_manager
        .get_preset(&id)
        .await
        .map_err(ApiError::from)?
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
    State(state): State<AppState>,
    Json(payload): Json<CreatePresetRequest>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

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
    if pipeline_manager
        .name_exists(&preset.name, None)
        .await
        .map_err(ApiError::from)?
    {
        return Err(ApiError::conflict(format!(
            "A preset with name '{}' already exists",
            preset.name
        )));
    }

    pipeline_manager
        .create_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(preset))
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
        (status = 409, description = "Preset name already exists", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePresetRequest>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Check if preset exists
    let existing = pipeline_manager
        .get_preset(&id)
        .await
        .map_err(ApiError::from)?
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
        .await
        .map_err(ApiError::from)?
    {
        return Err(ApiError::conflict(format!(
            "A preset with name '{}' already exists",
            preset.name
        )));
    }

    pipeline_manager
        .update_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(preset))
}

#[utoipa::path(
    delete,
    path = "/api/job/presets/{id}",
    tag = "job",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Preset deleted")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<()>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    pipeline_manager
        .delete_preset(&id)
        .await
        .map_err(ApiError::from)?;
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
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<ClonePresetRequest>,
) -> ApiResult<Json<crate::database::models::JobPreset>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let cloned = pipeline_manager
        .clone_preset(&id, payload.new_name)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(cloned))
}
