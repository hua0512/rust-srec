use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
};

use crate::api::error::{ApiError, ApiResult};
use crate::database::models::{Pagination, PipelineStep};

use super::{
    CreatePipelinePresetRequest, PipelinePresetFilterParams, PipelinePresetListResponse,
    PipelinePresetPaginationParams, PipelinePresetResponse, PresetPreviewJob,
    PresetPreviewResponse, PresetRouteState, UpdatePipelinePresetRequest, topological_sort,
};

#[utoipa::path(
    get,
    path = "/api/pipeline/presets",
    tag = "pipeline",
    params(PipelinePresetFilterParams, PipelinePresetPaginationParams),
    responses(
        (status = 200, description = "List of pipeline presets", body = PipelinePresetListResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_pipeline_presets(
    State(state): State<PresetRouteState>,
    Query(filters): Query<PipelinePresetFilterParams>,
    Query(pagination): Query<PipelinePresetPaginationParams>,
) -> ApiResult<Json<PipelinePresetListResponse>> {
    let preset_repo = &state.pipeline_preset_repository;

    let db_filters = crate::database::repositories::PipelinePresetFilters {
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    let (presets, total) = preset_repo
        .list_pipeline_presets_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let response_presets: Vec<PipelinePresetResponse> = presets
        .into_iter()
        .map(PipelinePresetResponse::from)
        .collect();

    Ok(Json(PipelinePresetListResponse {
        presets: response_presets,
        total,
        limit: effective_limit,
        offset: pagination.offset,
    }))
}

#[utoipa::path(
    get,
    path = "/api/pipeline/presets/{id}",
    tag = "pipeline",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Pipeline preset", body = PipelinePresetResponse),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_pipeline_preset_by_id(
    State(state): State<PresetRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = &state.pipeline_preset_repository;

    let preset = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    Ok(Json(PipelinePresetResponse::from(preset)))
}

#[utoipa::path(
    post,
    path = "/api/pipeline/presets",
    tag = "pipeline",
    request_body = CreatePipelinePresetRequest,
    responses(
        (status = 201, description = "Preset created", body = PipelinePresetResponse),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_pipeline_preset(
    State(state): State<PresetRouteState>,
    Json(payload): Json<CreatePipelinePresetRequest>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = &state.pipeline_preset_repository;

    // Validate DAG has at least one step
    if payload.dag.steps.is_empty() {
        return Err(ApiError::bad_request(
            "DAG pipeline preset must have at least one step",
        ));
    }

    // Create DAG preset
    let mut preset = crate::database::models::PipelinePreset::new(payload.name, payload.dag);
    if let Some(desc) = payload.description {
        preset = preset.with_description(desc);
    }

    preset_repo
        .create_pipeline_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(PipelinePresetResponse::from(preset)))
}

#[utoipa::path(
    put,
    path = "/api/pipeline/presets/{id}",
    tag = "pipeline",
    params(("id" = String, Path, description = "Preset ID")),
    request_body = UpdatePipelinePresetRequest,
    responses(
        (status = 200, description = "Preset updated", body = PipelinePresetResponse),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn update_pipeline_preset(
    State(state): State<PresetRouteState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePipelinePresetRequest>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = &state.pipeline_preset_repository;

    // Check if preset exists
    let existing = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    // Validate DAG has at least one step
    if payload.dag.steps.is_empty() {
        return Err(ApiError::bad_request(
            "DAG pipeline preset must have at least one step",
        ));
    }

    let dag_json = serde_json::to_string(&payload.dag)
        .map_err(|e| ApiError::bad_request(format!("Invalid DAG definition: {}", e)))?;

    let preset = crate::database::models::PipelinePreset {
        id: id.clone(),
        name: payload.name,
        description: payload.description,
        dag_definition: Some(dag_json),
        pipeline_type: Some("dag".to_string()),
        created_at: existing.created_at,
        updated_at: chrono::Utc::now(),
    };

    preset_repo
        .update_pipeline_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(PipelinePresetResponse::from(preset)))
}

#[utoipa::path(
    delete,
    path = "/api/pipeline/presets/{id}",
    tag = "pipeline",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Preset deleted")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_pipeline_preset(
    State(state): State<PresetRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<()>> {
    let preset_repo = &state.pipeline_preset_repository;

    preset_repo
        .delete_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(()))
}

#[utoipa::path(
    get,
    path = "/api/pipeline/presets/{id}/preview",
    tag = "pipeline",
    params(("id" = String, Path, description = "Preset ID")),
    responses(
        (status = 200, description = "Preset preview", body = PresetPreviewResponse),
        (status = 404, description = "Preset not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn preview_pipeline_preset(
    State(state): State<PresetRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PresetPreviewResponse>> {
    let preset_repo = &state.pipeline_preset_repository;

    let preset = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    let dag = preset
        .get_dag_definition()
        .ok_or_else(|| ApiError::internal("Pipeline preset has no DAG definition"))?;

    // Build dependency map for finding leaf steps; borrows step ids from
    // `dag`, which outlives every use below.
    let mut has_dependents: HashSet<&str> = HashSet::new();
    for step in &dag.steps {
        for dep in &step.depends_on {
            has_dependents.insert(dep.as_str());
        }
    }

    // Build preview jobs
    let jobs: Vec<PresetPreviewJob> = dag
        .steps
        .iter()
        .map(|step| {
            let processor = match &step.step {
                PipelineStep::Preset { name } => name.clone(),
                PipelineStep::Workflow { name } => format!("workflow:{}", name),
                PipelineStep::Inline { processor, .. } => processor.clone(),
            };
            let is_root = step.depends_on.is_empty();
            let is_leaf = !has_dependents.contains(step.id.as_str());

            PresetPreviewJob {
                step_id: step.id.clone(),
                processor,
                depends_on: step.depends_on.clone(),
                is_root,
                is_leaf,
            }
        })
        .collect();

    // Compute topological order
    let execution_order = topological_sort(&dag);

    Ok(Json(PresetPreviewResponse {
        preset_id: preset.id,
        preset_name: preset.name,
        jobs,
        execution_order,
    }))
}

// ============================================================================
// DAG Status, Graph, Retry, and Validation Handlers
// ============================================================================
