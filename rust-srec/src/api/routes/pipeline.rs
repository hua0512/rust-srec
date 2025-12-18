//! Pipeline management routes (DAG-native).
//!
//! This module provides REST API endpoints for managing DAG pipeline jobs,
//! including job listing, retrieval, retry, cancellation, and statistics.
//!
//! All pipelines are DAG (Directed Acyclic Graph) pipelines supporting:
//! - Fan-out: One step can trigger multiple downstream steps
//! - Fan-in: Multiple steps can merge their outputs before a downstream step
//! - Parallel execution: Independent steps run concurrently
//!
//! # Endpoints
//!
//! ## Jobs
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/pipeline/jobs` | List jobs with filtering and pagination |
//! | GET | `/api/pipeline/jobs/page` | List jobs (no total count) |
//! | GET | `/api/pipeline/jobs/{id}` | Get a single job by ID |
//! | GET | `/api/pipeline/jobs/{id}/logs` | List job execution logs (paged) |
//! | GET | `/api/pipeline/jobs/{id}/progress` | Get latest job progress snapshot |
//! | POST | `/api/pipeline/jobs/{id}/retry` | Retry a failed job |
//! | DELETE | `/api/pipeline/jobs/{id}` | Cancel a pending or processing job |
//!
//! ## Pipelines
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/pipeline/pipelines` | List pipelines with filtering and pagination |
//! | DELETE | `/api/pipeline/{pipeline_id}` | Cancel all jobs in a pipeline |
//! | POST | `/api/pipeline/create` | Create a new DAG pipeline |
//! | POST | `/api/pipeline/validate` | Validate a DAG definition |
//!
//! ## DAG Status & Operations
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/pipeline/dags` | List all DAG executions with filtering and pagination |
//! | GET | `/api/pipeline/dag/{dag_id}` | Get full DAG status with all steps |
//! | GET | `/api/pipeline/dag/{dag_id}/graph` | Get DAG visualization data (nodes/edges) |
//! | GET | `/api/pipeline/dag/{dag_id}/stats` | Get DAG step statistics (blocked/pending/processing/etc.) |
//! | POST | `/api/pipeline/dag/{dag_id}/retry` | Retry all failed steps in a DAG |
//! | DELETE | `/api/pipeline/dag/{dag_id}` | Cancel a DAG execution and all its steps |
//!
//! ## Presets (Workflow Templates)
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/pipeline/presets` | List pipeline presets (DAG workflows) |
//! | GET | `/api/pipeline/presets/{id}` | Get a pipeline preset by ID |
//! | GET | `/api/pipeline/presets/{id}/preview` | Preview jobs from a preset |
//! | POST | `/api/pipeline/presets` | Create a pipeline preset |
//! | PUT | `/api/pipeline/presets/{id}` | Update a pipeline preset |
//! | DELETE | `/api/pipeline/presets/{id}` | Delete a pipeline preset |
//!
//! ## Other
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | GET | `/api/pipeline/outputs` | List media outputs with filtering |
//! | GET | `/api/pipeline/stats` | Get pipeline statistics |

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post},
};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    JobExecutionInfo as ApiJobExecutionInfo, JobFilterParams, JobLogEntry as ApiJobLogEntry,
    JobResponse, JobStatus as ApiJobStatus, MediaOutputResponse, PageResponse, PaginatedResponse,
    PaginationParams, PipelineStatsResponse, StepDurationInfo as ApiStepDurationInfo,
};
use crate::api::server::AppState;
use crate::database::models::job::{DagPipelineDefinition, DagStep, PipelineStep};
use crate::database::models::{JobFilters, JobStatus as DbJobStatus, OutputFilters, Pagination};
use crate::pipeline::JobProgressSnapshot;
use crate::pipeline::{Job, JobStatus as QueueJobStatus};

/// Create the pipeline router (DAG-native).
///
/// # Routes
///
/// - `GET /jobs` - List jobs with filtering and pagination
/// - `GET /jobs/{id}` - Get a single job by ID
/// - `POST /jobs/{id}/retry` - Retry a failed job
/// - `DELETE /jobs/{id}` - Cancel a job
/// - `DELETE /{pipeline_id}` - Cancel all jobs in a DAG pipeline
/// - `GET /pipelines` - List DAG pipelines with filtering and pagination
/// - `GET /outputs` - List media outputs
/// - `GET /stats` - Get pipeline statistics
/// - `POST /create` - Create a new DAG pipeline
/// - `GET /presets` - List pipeline presets (DAG workflows)
/// - `GET /presets/{id}` - Get a pipeline preset by ID
/// - `POST /presets` - Create a DAG pipeline preset
/// - `PUT /presets/{id}` - Update a DAG pipeline preset
/// - `DELETE /presets/{id}` - Delete a pipeline preset
/// - `GET /presets/{id}/preview` - Preview jobs from a preset
/// - `GET /dags` - List all DAG executions with filtering and pagination
/// - `GET /dag/{dag_id}` - Get full DAG status with all steps
/// - `GET /dag/{dag_id}/graph` - Get DAG visualization data
/// - `GET /dag/{dag_id}/stats` - Get DAG step statistics
/// - `POST /dag/{dag_id}/retry` - Retry failed steps in a DAG
/// - `DELETE /dag/{dag_id}` - Cancel a DAG execution
/// - `POST /validate` - Validate a DAG definition
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/page", get(list_jobs_page))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/logs", get(list_job_logs))
        .route("/jobs/{id}/progress", get(get_job_progress))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/jobs/{id}", delete(cancel_job))
        .route("/{pipeline_id}", delete(cancel_pipeline))
        .route("/pipelines", get(list_pipelines))
        .route("/outputs", get(list_outputs))
        .route("/stats", get(get_stats))
        .route("/create", post(create_pipeline))
        .route("/validate", post(validate_dag))
        .route(
            "/presets",
            get(list_pipeline_presets).post(create_pipeline_preset),
        )
        .route(
            "/presets/{id}",
            get(get_pipeline_preset_by_id)
                .put(update_pipeline_preset)
                .delete(delete_pipeline_preset),
        )
        .route("/presets/{id}/preview", get(preview_pipeline_preset))
        .route("/dags", get(list_dags))
        .route("/dag/{dag_id}", get(get_dag_status).delete(cancel_dag))
        .route("/dag/{dag_id}/graph", get(get_dag_graph))
        .route("/dag/{dag_id}/stats", get(get_dag_stats))
        .route("/dag/{dag_id}/retry", post(retry_dag))
}

/// Request body for creating a new DAG pipeline.
///
/// # Example
///
/// ```json
/// {
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "input_path": "/recordings/stream.flv",
///     "dag": {
///         "name": "my_pipeline",
///         "steps": [
///             {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
///             {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
///             {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux", "thumbnail"]}
///         ]
///     }
/// }
/// ```
///
/// # Fields
///
/// - `session_id` - The recording session ID this pipeline belongs to
/// - `streamer_id` - The streamer ID this pipeline belongs to
/// - `input_path` - Path to the input file to process
/// - `dag` - DAG pipeline definition with steps and dependencies
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePipelineRequest {
    /// Session ID for the pipeline.
    pub session_id: String,
    /// Streamer ID for the pipeline.
    pub streamer_id: String,
    /// Input file path.
    pub input_path: String,
    /// DAG pipeline definition.
    pub dag: DagPipelineDefinition,
}

/// Response body for pipeline creation.
///
/// # Example
///
/// ```json
/// {
///     "pipeline_id": "job-uuid-123",
///     "first_job": {
///         "id": "job-uuid-123",
///         "session_id": "session-123",
///         "streamer_id": "streamer-456",
///         "pipeline_id": "job-uuid-123",
///         "status": "pending",
///         "processor_type": "remux",
///         "input_path": ["/recordings/stream.flv"],
///         "created_at": "2025-12-03T10:00:00Z"
///     }
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct CreatePipelineResponse {
    /// Pipeline ID (same as first job's ID).
    pub pipeline_id: String,
    /// First job details.
    pub first_job: JobResponse,
}

/// Request body for creating a new DAG pipeline preset.
///
/// # Example
///
/// ```json
/// {
///     "name": "Stream Archive",
///     "description": "Remux, thumbnail, and upload workflow",
///     "dag": {
///         "name": "Stream Archive",
///         "steps": [
///             {"id": "remux", "step": {"type": "preset", "name": "remux_clean"}, "depends_on": []},
///             {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail_native"}, "depends_on": ["remux"]},
///             {"id": "upload", "step": {"type": "preset", "name": "upload_and_delete"}, "depends_on": ["remux", "thumbnail"]}
///         ]
///     }
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// DAG pipeline definition.
    pub dag: DagPipelineDefinition,
}

/// Request body for updating a DAG pipeline preset.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// DAG pipeline definition.
    pub dag: DagPipelineDefinition,
}

/// Query parameters for filtering pipeline presets.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PipelinePresetFilterParams {
    /// Search query (matches name or description).
    pub search: Option<String>,
}

/// Pagination parameters for pipeline preset list.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelinePresetPaginationParams {
    /// Number of items to return (default: 20, max: 100).
    #[serde(default = "default_preset_limit")]
    pub limit: u32,
    /// Number of items to skip.
    #[serde(default)]
    pub offset: u32,
}

fn default_preset_limit() -> u32 {
    20
}

impl Default for PipelinePresetPaginationParams {
    fn default() -> Self {
        Self {
            limit: default_preset_limit(),
            offset: 0,
        }
    }
}

/// Response for pipeline preset list with pagination.
#[derive(Debug, Clone, Serialize)]
pub struct PipelinePresetListResponse {
    /// List of pipeline presets.
    pub presets: Vec<PipelinePresetResponse>,
    /// Total number of presets matching the filter.
    pub total: u64,
    /// Number of items returned.
    pub limit: u32,
    /// Number of items skipped.
    pub offset: u32,
}

/// Response for a single DAG pipeline preset.
#[derive(Debug, Clone, Serialize)]
pub struct PipelinePresetResponse {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// DAG definition.
    pub dag: DagPipelineDefinition,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<crate::database::models::PipelinePreset> for PipelinePresetResponse {
    fn from(preset: crate::database::models::PipelinePreset) -> Self {
        let dag = preset.get_dag_definition().unwrap_or_else(|| {
            // Fallback: convert legacy steps to DAG format
            #[allow(deprecated)]
            let steps = preset.get_steps();
            let dag_steps: Vec<DagStep> = steps
                .iter()
                .enumerate()
                .map(|(i, step)| {
                    let step_id = match step {
                        PipelineStep::Preset { name } => name.clone(),
                        PipelineStep::Workflow { name } => name.clone(),
                        PipelineStep::Inline { processor, .. } => format!("{}_{}", processor, i),
                    };

                    if i == 0 {
                        DagStep::new(step_id, step.clone())
                    } else {
                        let prev_step_id = match &steps[i - 1] {
                            PipelineStep::Preset { name } => name.clone(),
                            PipelineStep::Workflow { name } => name.clone(),
                            PipelineStep::Inline { processor, .. } => {
                                format!("{}_{}", processor, i - 1)
                            }
                        };
                        DagStep::with_dependencies(step_id, step.clone(), vec![prev_step_id])
                    }
                })
                .collect();
            DagPipelineDefinition::new(&preset.name, dag_steps)
        });
        Self {
            id: preset.id,
            name: preset.name,
            description: preset.description,
            dag,
            created_at: preset.created_at,
            updated_at: preset.updated_at,
        }
    }
}

/// Query parameters for filtering media outputs.
///
/// # Example
///
/// ```
/// GET /api/pipeline/outputs?session_id=session-123&streamer_id=streamer-456
/// ```
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OutputFilterParams {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Search query (matches file path, session ID, or format).
    pub search: Option<String>,
}

/// Response for a single pipeline summary.
#[derive(Debug, Clone, Serialize)]
pub struct PipelineSummaryResponse {
    pub pipeline_id: String,
    pub streamer_id: String,
    pub streamer_name: Option<String>,
    pub session_id: Option<String>,
    pub status: String,
    pub job_count: i64,
    pub completed_count: i64,
    pub failed_count: i64,
    pub total_duration_secs: f64,
    pub created_at: String,
    pub updated_at: String,
}

// ============================================================================
// DAG Status and Graph Response Types
// ============================================================================

/// Response for DAG status with all steps.
#[derive(Debug, Clone, Serialize)]
pub struct DagStatusResponse {
    /// DAG execution ID.
    pub id: String,
    /// DAG name from definition.
    pub name: String,
    /// Overall DAG status.
    pub status: String,
    /// Associated streamer ID.
    pub streamer_id: Option<String>,
    /// Associated session ID.
    pub session_id: Option<String>,
    /// Total number of steps.
    pub total_steps: i32,
    /// Number of completed steps.
    pub completed_steps: i32,
    /// Number of failed steps.
    pub failed_steps: i32,
    /// Progress percentage (0-100).
    pub progress_percent: f64,
    /// All steps in the DAG with their status.
    pub steps: Vec<DagStepStatusResponse>,
    /// Error message if DAG failed.
    pub error: Option<String>,
    /// When the DAG was created.
    pub created_at: String,
    /// When the DAG was last updated.
    pub updated_at: String,
    /// When the DAG completed (if finished).
    pub completed_at: Option<String>,
}

/// Response for a single DAG step status.
#[derive(Debug, Clone, Serialize)]
pub struct DagStepStatusResponse {
    /// Step ID within the DAG.
    pub step_id: String,
    /// Step status (blocked, pending, processing, completed, failed, cancelled).
    pub status: String,
    /// Associated job ID (if job has been created).
    pub job_id: Option<String>,
    /// Step IDs this step depends on.
    pub depends_on: Vec<String>,
    /// Output paths produced by this step.
    pub outputs: Vec<String>,
    /// The processor type for this step.
    pub processor: Option<String>,
}

/// Response for DAG graph visualization.
#[derive(Debug, Clone, Serialize)]
pub struct DagGraphResponse {
    /// DAG execution ID.
    pub dag_id: String,
    /// DAG name.
    pub name: String,
    /// Graph nodes (steps).
    pub nodes: Vec<DagGraphNode>,
    /// Graph edges (dependencies).
    pub edges: Vec<DagGraphEdge>,
}

/// A node in the DAG graph.
#[derive(Debug, Clone, Serialize)]
pub struct DagGraphNode {
    /// Step ID (unique within DAG).
    pub id: String,
    /// Display label.
    pub label: String,
    /// Node status for styling.
    pub status: String,
    /// Processor type.
    pub processor: Option<String>,
    /// Associated job ID.
    pub job_id: Option<String>,
}

/// An edge in the DAG graph (dependency relationship).
#[derive(Debug, Clone, Serialize)]
pub struct DagGraphEdge {
    /// Source step ID (dependency).
    pub from: String,
    /// Target step ID (dependent).
    pub to: String,
}

/// Response for DAG retry operation.
#[derive(Debug, Clone, Serialize)]
pub struct DagRetryResponse {
    /// DAG execution ID.
    pub dag_id: String,
    /// Number of steps that were retried.
    pub retried_steps: usize,
    /// IDs of jobs created for retry.
    pub job_ids: Vec<String>,
    /// Message describing the retry operation.
    pub message: String,
}

/// Request body for DAG validation.
#[derive(Debug, Clone, Deserialize)]
pub struct ValidateDagRequest {
    /// DAG definition to validate.
    pub dag: DagPipelineDefinition,
}

/// Response for DAG validation.
#[derive(Debug, Clone, Serialize)]
pub struct ValidateDagResponse {
    /// Whether the DAG is valid.
    pub valid: bool,
    /// Validation errors (if any).
    pub errors: Vec<String>,
    /// Validation warnings (if any).
    pub warnings: Vec<String>,
    /// Detected root steps (no dependencies).
    pub root_steps: Vec<String>,
    /// Detected leaf steps (no dependents).
    pub leaf_steps: Vec<String>,
    /// Maximum depth of the DAG.
    pub max_depth: usize,
}

/// Response for pipeline preset preview.
#[derive(Debug, Clone, Serialize)]
pub struct PresetPreviewResponse {
    /// Preset ID.
    pub preset_id: String,
    /// Preset name.
    pub preset_name: String,
    /// Preview of jobs that would be created.
    pub jobs: Vec<PresetPreviewJob>,
    /// Execution order (topologically sorted).
    pub execution_order: Vec<String>,
}

/// A preview of a job that would be created from a preset.
#[derive(Debug, Clone, Serialize)]
pub struct PresetPreviewJob {
    /// Step ID.
    pub step_id: String,
    /// Processor type.
    pub processor: String,
    /// Dependencies (step IDs).
    pub depends_on: Vec<String>,
    /// Whether this is a root step (runs first).
    pub is_root: bool,
    /// Whether this is a leaf step (runs last).
    pub is_leaf: bool,
}

/// Query parameters for filtering DAG executions.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DagFilterParams {
    /// Filter by DAG status (PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED).
    pub status: Option<String>,
}

/// Pagination parameters for DAG list.
#[derive(Debug, Clone, Deserialize)]
pub struct DagPaginationParams {
    /// Number of items to return (default: 20, max: 100).
    #[serde(default = "default_dag_limit")]
    pub limit: u32,
    /// Number of items to skip.
    #[serde(default)]
    pub offset: u32,
}

fn default_dag_limit() -> u32 {
    20
}

impl Default for DagPaginationParams {
    fn default() -> Self {
        Self {
            limit: default_dag_limit(),
            offset: 0,
        }
    }
}

/// Response for DAG list with pagination.
#[derive(Debug, Clone, Serialize)]
pub struct DagListResponse {
    /// List of DAG executions.
    pub dags: Vec<DagListItem>,
    /// Total number of DAGs matching the filter.
    pub total: u64,
    /// Number of items returned.
    pub limit: u32,
    /// Number of items skipped.
    pub offset: u32,
}

/// A single DAG execution in the list response.
#[derive(Debug, Clone, Serialize)]
pub struct DagListItem {
    /// DAG execution ID.
    pub id: String,
    /// DAG name from definition.
    pub name: String,
    /// Overall DAG status.
    pub status: String,
    /// Associated streamer ID.
    pub streamer_id: Option<String>,
    /// Associated session ID.
    pub session_id: Option<String>,
    /// Total number of steps.
    pub total_steps: i32,
    /// Number of completed steps.
    pub completed_steps: i32,
    /// Number of failed steps.
    pub failed_steps: i32,
    /// Progress percentage (0-100).
    pub progress_percent: f64,
    /// When the DAG was created.
    pub created_at: String,
    /// When the DAG was last updated.
    pub updated_at: String,
}

/// Response for DAG cancellation.
#[derive(Debug, Clone, Serialize)]
pub struct DagCancelResponse {
    /// DAG execution ID.
    pub dag_id: String,
    /// Number of steps that were cancelled.
    pub cancelled_steps: u64,
    /// Message describing the cancellation.
    pub message: String,
}

/// Response for DAG step statistics.
#[derive(Debug, Clone, Serialize)]
pub struct DagStatsResponse {
    /// DAG execution ID.
    pub dag_id: String,
    /// Number of blocked steps (waiting for dependencies).
    pub blocked: u64,
    /// Number of pending steps (ready to run).
    pub pending: u64,
    /// Number of processing steps (currently running).
    pub processing: u64,
    /// Number of completed steps.
    pub completed: u64,
    /// Number of failed steps.
    pub failed: u64,
    /// Number of cancelled steps.
    pub cancelled: u64,
    /// Total number of steps.
    pub total: u64,
    /// Progress percentage (0-100).
    pub progress_percent: f64,
}

/// List pipeline jobs with pagination and filtering.
///
/// # Endpoint
///
/// `GET /api/pipeline/jobs`
///
/// # Query Parameters
///
/// - `limit` - Maximum number of results (default: 20, max: 100)
/// - `offset` - Number of results to skip (default: 0)
/// - `session_id` - Associated recording session ID
/// - `streamer_id` - Associated streamer ID
/// - `pipeline_id` - Associated pipeline ID (if part of a pipeline)
/// - `status` - Current job status (pending, processing, completed, failed, interrupted)
/// - `streamer_id` - Filter by streamer ID
/// - `session_id` - Filter by session ID
/// - `from_date` - Filter jobs created after this date (ISO 8601)
/// - `to_date` - Filter jobs created before this date (ISO 8601)
///
/// # Response
///
/// Returns a paginated list of jobs matching the filter criteria.
///
/// ```json
/// {
///     "items": [...],
///     "total": 100,
///     "limit": 20,
///     "offset": 0
/// }
/// ```
///
/// # Requirements
///
/// - 1.1: Return all jobs matching filter criteria with pagination
/// - 1.3: Filter by status
/// - 1.4: Filter by streamer_id
/// - 1.5: Filter by session_id
async fn list_jobs(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PaginatedResponse<JobResponse>>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Convert API filter params to database filter types
    let db_filters = JobFilters {
        status: filters.status.map(api_status_to_db_status),
        streamer_id: filters.streamer_id,
        session_id: filters.session_id,
        pipeline_id: filters.pipeline_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        job_type: None,
        job_types: None,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    // Call PipelineManager.list_jobs
    let (jobs, total) = pipeline_manager
        .list_jobs(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    // Batch-fetch streamer names
    let streamer_names = fetch_streamer_names(&state, &jobs).await;

    // Convert jobs to API response format
    let job_responses: Vec<JobResponse> = jobs
        .iter()
        .map(|job| {
            let name = streamer_names.get(&job.streamer_id).cloned();
            job_to_response(job, name)
        })
        .collect();

    let response = PaginatedResponse::new(job_responses, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

/// List pipeline jobs without computing a total count.
///
/// # Endpoint
///
/// `GET /api/pipeline/jobs/page`
async fn list_jobs_page(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PageResponse<JobResponse>>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let db_filters = JobFilters {
        status: filters.status.map(api_status_to_db_status),
        streamer_id: filters.streamer_id,
        session_id: filters.session_id,
        pipeline_id: filters.pipeline_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        job_type: None,
        job_types: None,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    let jobs = pipeline_manager
        .list_jobs_page(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    // Batch-fetch streamer names
    let streamer_names = fetch_streamer_names(&state, &jobs).await;

    let job_responses: Vec<JobResponse> = jobs
        .iter()
        .map(|job| {
            let name = streamer_names.get(&job.streamer_id).cloned();
            job_to_response(job, name)
        })
        .collect();
    Ok(Json(PageResponse::new(
        job_responses,
        effective_limit,
        pagination.offset,
    )))
}

/// List job execution logs (paged).
///
/// # Endpoint
///
/// `GET /api/pipeline/jobs/{id}/logs`
async fn list_job_logs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<ApiJobLogEntry>>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    let (logs, total) = pipeline_manager
        .list_job_logs(&id, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let response_logs: Vec<ApiJobLogEntry> = logs
        .into_iter()
        .map(|log| ApiJobLogEntry {
            timestamp: log.timestamp,
            level: format!("{:?}", log.level),
            message: log.message,
        })
        .collect();

    Ok(Json(PaginatedResponse::new(
        response_logs,
        total,
        effective_limit,
        pagination.offset,
    )))
}

/// Get latest job progress snapshot.
///
/// # Endpoint
///
/// `GET /api/pipeline/jobs/{id}/progress`
async fn get_job_progress(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobProgressSnapshot>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let snapshot = pipeline_manager
        .get_job_progress(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("No progress available for job {}", id)))?;

    Ok(Json(snapshot))
}

/// List pipelines with pagination and filtering.
///
/// # Endpoint
///
/// `GET /api/pipeline/pipelines`
///
/// # Query Parameters
///
/// - `limit` - Maximum number of results (default: 20, max: 100)
/// - `offset` - Number of results to skip (default: 0)
/// - `status` - Filter by overall pipeline status
/// - `streamer_id` - Filter by streamer ID
/// - `session_id` - Filter by session ID
/// - `search` - Search query
///
/// # Response
///
/// Returns a paginated list of pipeline summaries.
async fn list_pipelines(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PaginatedResponse<PipelineSummaryResponse>>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let db_filters = JobFilters {
        status: filters.status.map(api_status_to_db_status),
        streamer_id: filters.streamer_id,
        session_id: filters.session_id,
        pipeline_id: filters.pipeline_id,
        from_date: filters.from_date,
        to_date: filters.to_date,
        job_type: None,
        job_types: None,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    let (pipelines, total) = pipeline_manager
        .list_pipelines(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    // Batch-fetch streamer names
    let streamer_ids: HashSet<String> = pipelines.iter().map(|p| p.streamer_id.clone()).collect();
    let streamer_names: HashMap<String, String> = if let Some(repo) = &state.streamer_repository {
        let fetches = streamer_ids.into_iter().map(|streamer_id| {
            let repo = repo.clone();
            async move {
                let name = repo.get_streamer(&streamer_id).await.ok().map(|s| s.name);
                (streamer_id, name)
            }
        });
        join_all(fetches)
            .await
            .into_iter()
            .filter_map(|(id, name)| name.map(|n| (id, n)))
            .collect()
    } else {
        HashMap::new()
    };

    let responses: Vec<PipelineSummaryResponse> = pipelines
        .into_iter()
        .map(|p| {
            let streamer_name = streamer_names.get(&p.streamer_id).cloned();
            PipelineSummaryResponse {
                pipeline_id: p.pipeline_id,
                streamer_id: p.streamer_id,
                streamer_name,
                session_id: p.session_id,
                status: p.status,
                job_count: p.job_count,
                completed_count: p.completed_count,
                failed_count: p.failed_count,
                total_duration_secs: p.total_duration_secs,
                created_at: p.created_at,
                updated_at: p.updated_at,
            }
        })
        .collect();

    let response = PaginatedResponse::new(responses, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

/// Get a single job by ID.
///
/// # Endpoint
///
/// `GET /api/pipeline/jobs/{id}`
///
/// # Path Parameters
///
/// - `id` - The job ID (UUID)
///
/// # Response
///
/// Returns the job details if found.
///
/// ```json
/// {
///     "id": "job-uuid-123",
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "pipeline_id": "job-uuid-123",
///     "status": "completed",
///     "processor_type": "remux",
///     "input_path": ["/recordings/stream.flv"],
///     "output_path": ["/recordings/stream.mp4"],
///     "created_at": "2025-12-03T10:00:00Z",
///     "started_at": "2025-12-03T10:00:01Z",
///     "completed_at": "2025-12-03T10:05:00Z"
/// }
/// ```
///
/// # Errors
///
/// - `404 Not Found` - Job with the specified ID does not exist
///
/// # Requirements
///
/// - 1.2: Return job if exists or indicate not found
async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.get_job
    let job = pipeline_manager
        .get_job(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Job with id '{}' not found", id)))?;

    // Fetch streamer name
    let streamer_name = state.streamer_repository.as_ref().and_then(|repo| {
        futures::executor::block_on(repo.get_streamer(&job.streamer_id))
            .ok()
            .map(|s| s.name)
    });

    Ok(Json(job_to_response(&job, streamer_name)))
}

/// Retry a failed job.
///
/// # Endpoint
///
/// `POST /api/pipeline/jobs/{id}/retry`
///
/// # Path Parameters
///
/// - `id` - The job ID (UUID)
///
/// # Response
///
/// Returns the updated job with status reset to "pending".
///
/// ```json
/// {
///     "id": "job-uuid-123",
///     "status": "pending",
///     "retry_count": 1,
///     ...
/// }
/// ```
///
/// # Errors
///
/// - `404 Not Found` - Job with the specified ID does not exist
/// - `400 Bad Request` - Job is not in "failed" status
///
/// # Requirements
///
/// - 2.1: Reset failed job status to Pending and increment retry_count
/// - 2.2: Reject retry for jobs not in Failed status
async fn retry_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.retry_job
    let job = pipeline_manager
        .retry_job(&id)
        .await
        .map_err(ApiError::from)?;

    // Fetch streamer name
    let streamer_name = state.streamer_repository.as_ref().and_then(|repo| {
        futures::executor::block_on(repo.get_streamer(&job.streamer_id))
            .ok()
            .map(|s| s.name)
    });

    Ok(Json(job_to_response(&job, streamer_name)))
}

/// Cancel a pending or processing job.
///
/// # Endpoint
///
/// `DELETE /api/pipeline/jobs/{id}`
///
/// # Path Parameters
///
/// - `id` - The job ID (UUID)
///
/// # Response
///
/// Returns a success message on successful cancellation.
///
/// ```json
/// {
///     "success": true,
///     "message": "Job 'job-uuid-123' cancelled successfully"
/// }
/// ```
///
/// # Errors
///
/// - `404 Not Found` - Job with the specified ID does not exist
/// - `400 Bad Request` - Job is in a terminal status (completed or failed)
///
/// # Behavior
///
/// - For pending jobs: Removes from queue and marks as "interrupted"
/// - For processing jobs: Signals cancellation to worker and marks as "interrupted"
/// - For completed/failed jobs: Returns error (cannot cancel terminal jobs)
///
/// # Requirements
///
/// - 2.3: Cancel pending jobs by removing from queue
/// - 2.4: Cancel processing jobs by signaling cancellation
/// - 2.5: Reject cancellation for completed/failed jobs
async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.cancel_job. If it fails because the job is already
    // terminal (Completed/Failed), we try to DELETE it instead.
    match pipeline_manager.cancel_job(&id).await {
        Ok(_) => Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Job '{}' cancelled successfully", id)
        }))),
        Err(crate::Error::InvalidStateTransition { .. }) => {
            // Job is already in a terminal state (Completed/Failed), so delete it
            pipeline_manager
                .delete_job(&id)
                .await
                .map_err(ApiError::from)?;

            Ok(Json(serde_json::json!({
                "success": true,
                "message": format!("Job '{}' deleted successfully", id)
            })))
        }
        Err(e) => Err(ApiError::from(e)),
    }
}

/// Cancel all jobs in a pipeline.
///
/// # Endpoint
///
/// `DELETE /api/pipeline/{pipeline_id}`
///
/// # Path Parameters
///
/// - `pipeline_id` - The pipeline ID (UUID of the first job in the pipeline)
///
/// # Response
///
/// Returns a success message with the number of jobs cancelled.
///
/// ```json
/// {
///     "success": true,
///     "message": "Cancelled 3 jobs in pipeline 'pipeline-uuid-123'",
///     "cancelled_count": 3
/// }
/// ```
///
/// # Behavior
///
/// - Cancels all pending and processing jobs that belong to the pipeline
/// - Already completed or failed jobs are not affected
/// - Each cancelled job is marked as "interrupted"
/// - Processing jobs receive a cancellation signal
async fn cancel_pipeline(
    State(state): State<AppState>,
    Path(pipeline_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.cancel_pipeline
    let cancelled_count = pipeline_manager
        .cancel_pipeline(&pipeline_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Cancelled {} jobs in pipeline '{}'", cancelled_count, pipeline_id),
        "cancelled_count": cancelled_count
    })))
}

/// List media outputs with pagination and filtering.
///
/// # Endpoint
///
/// `GET /api/pipeline/outputs`
///
/// # Query Parameters
///
/// - `limit` - Maximum number of results (default: 20, max: 100)
/// - `offset` - Number of results to skip (default: 0)
/// - `session_id` - Filter by session ID
/// - `streamer_id` - Filter by streamer ID
///
/// # Response
///
/// Returns a paginated list of media outputs.
///
/// ```json
/// {
///     "items": [
///         {
///             "id": "output-uuid-123",
///             "session_id": "session-123",
///             "file_path": "/recordings/stream.mp4",
///             "file_size_bytes": 1073741824,
///             "format": "mp4",
///             "created_at": "2025-12-03T10:05:00Z"
///         }
///     ],
///     "total": 50,
///     "limit": 20,
///     "offset": 0
/// }
/// ```
///
/// # Requirements
///
/// - 5.1: Return outputs with pagination support
/// - 5.2: Include file path, size, duration, and format
/// - 5.3: Filter by session_id
/// - 5.4: Filter by streamer_id
async fn list_outputs(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<OutputFilterParams>,
) -> ApiResult<Json<PaginatedResponse<MediaOutputResponse>>> {
    // Get session repository from state
    let session_repository = state
        .session_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session service not available"))?
        .clone();

    let requested_streamer_id = filters.streamer_id.clone();

    // Convert API filter params to database filter types
    let db_filters = OutputFilters {
        session_id: filters.session_id,
        streamer_id: filters.streamer_id,
        search: filters.search,
    };

    let effective_limit = pagination.limit.min(100);
    let db_pagination = Pagination::new(effective_limit, pagination.offset);

    // Call SessionRepository.list_outputs_filtered
    let (outputs, total) = session_repository
        .list_outputs_filtered(&db_filters, &db_pagination)
        .await
        .map_err(ApiError::from)?;

    let streamer_id_by_session: HashMap<String, String> = if requested_streamer_id.is_none() {
        let mut session_ids: HashSet<String> = HashSet::new();
        for output in &outputs {
            session_ids.insert(output.session_id.clone());
        }

        let fetches = session_ids.into_iter().map(|session_id| {
            let session_repository = session_repository.clone();
            async move {
                let streamer_id = session_repository
                    .get_session(&session_id)
                    .await
                    .ok()
                    .map(|session| session.streamer_id);
                (session_id, streamer_id)
            }
        });

        join_all(fetches)
            .await
            .into_iter()
            .filter_map(|(session_id, streamer_id)| {
                streamer_id.map(|streamer_id| (session_id, streamer_id))
            })
            .collect()
    } else {
        HashMap::new()
    };

    // Convert outputs to API response format
    let output_responses: Vec<MediaOutputResponse> = outputs
        .iter()
        .map(|output| {
            let created_at = chrono::DateTime::parse_from_rfc3339(&output.created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now());

            let streamer_id = match requested_streamer_id.as_deref() {
                Some(streamer_id) => streamer_id.to_string(),
                None => streamer_id_by_session
                    .get(&output.session_id)
                    .cloned()
                    .unwrap_or_default(),
            };

            MediaOutputResponse {
                id: output.id.clone(),
                session_id: output.session_id.clone(),
                streamer_id,
                file_path: output.file_path.clone(),
                file_size_bytes: output.size_bytes as u64,
                duration_secs: None, // Not stored in current model
                format: output.file_type.clone(),
                created_at,
            }
        })
        .collect();

    let response =
        PaginatedResponse::new(output_responses, total, effective_limit, pagination.offset);
    Ok(Json(response))
}

/// Get pipeline statistics.
///
/// # Endpoint
///
/// `GET /api/pipeline/stats`
///
/// # Response
///
/// Returns aggregate statistics about pipeline jobs.
///
/// ```json
/// {
///     "pending_count": 5,
///     "processing_count": 2,
///     "completed_count": 100,
///     "failed_count": 3,
///     "avg_processing_time_secs": 45.5
/// }
/// ```
///
/// # Requirements
///
/// - 3.1: Return counts of jobs by status
/// - 3.2: Compute mean duration of completed jobs
/// - 3.3: Maintain accurate counts across state transitions
async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<PipelineStatsResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.get_stats
    let stats = pipeline_manager.get_stats().await.map_err(ApiError::from)?;

    let response = PipelineStatsResponse {
        pending_count: stats.pending,
        processing_count: stats.processing,
        completed_count: stats.completed,
        failed_count: stats.failed,
        avg_processing_time_secs: stats.avg_processing_time_secs,
    };

    Ok(Json(response))
}

/// Create a new DAG pipeline.
///
/// # Endpoint
///
/// `POST /api/pipeline/create`
///
/// # Request Body
///
/// ```json
/// {
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "input_path": "/recordings/stream.flv",
///     "dag": {
///         "name": "my_pipeline",
///         "steps": [
///             {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
///             {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
///             {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux", "thumbnail"]}
///         ]
///     }
/// }
/// ```
///
/// # Fields
///
/// - `session_id` (required) - The recording session ID
/// - `streamer_id` (required) - The streamer ID
/// - `input_path` (required) - Path to the input file
/// - `dag` (required) - DAG pipeline definition with steps and dependencies
///
/// # Response
///
/// Returns the pipeline ID and first job details.
///
/// # DAG Pipeline Features
///
/// - Fan-out: One step can trigger multiple downstream steps
/// - Fan-in: Multiple steps can merge their outputs before a downstream step
/// - Parallel execution: Independent steps (no dependencies between them) run concurrently
/// - Fail-fast: Any step failure cancels all pending/running jobs in the DAG
async fn create_pipeline(
    State(state): State<AppState>,
    Json(request): Json<CreatePipelineRequest>,
) -> ApiResult<Json<CreatePipelineResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Validate DAG has at least one step
    if request.dag.steps.is_empty() {
        return Err(ApiError::bad_request(
            "DAG pipeline must have at least one step",
        ));
    }

    // Create DAG pipeline
    let result = pipeline_manager
        .create_dag_pipeline(
            &request.session_id,
            &request.streamer_id,
            &request.input_path,
            request.dag,
        )
        .await
        .map_err(ApiError::from)?;

    // Get the first job details (first root job)
    let first_job_id = result
        .root_job_ids
        .first()
        .ok_or_else(|| ApiError::internal("DAG pipeline created but no root jobs returned"))?;

    let first_job = pipeline_manager
        .get_job(first_job_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("Failed to retrieve created job"))?;

    // Fetch streamer name (we have the ID from the request)
    let streamer_name = state.streamer_repository.as_ref().and_then(|repo| {
        futures::executor::block_on(repo.get_streamer(&request.streamer_id))
            .ok()
            .map(|s| s.name)
    });

    let response = CreatePipelineResponse {
        pipeline_id: result.dag_id,
        first_job: job_to_response(&first_job, streamer_name),
    };

    Ok(Json(response))
}

// ============================================================================
// Helper functions
// ============================================================================

/// Convert API JobStatus to database JobStatus.
fn api_status_to_db_status(status: ApiJobStatus) -> DbJobStatus {
    match status {
        ApiJobStatus::Pending => DbJobStatus::Pending,
        ApiJobStatus::Processing => DbJobStatus::Processing,
        ApiJobStatus::Completed => DbJobStatus::Completed,
        ApiJobStatus::Failed => DbJobStatus::Failed,
        ApiJobStatus::Interrupted => DbJobStatus::Interrupted,
    }
}

/// Convert queue JobStatus to API JobStatus.
fn queue_status_to_api_status(status: QueueJobStatus) -> ApiJobStatus {
    match status {
        QueueJobStatus::Pending => ApiJobStatus::Pending,
        QueueJobStatus::Processing => ApiJobStatus::Processing,
        QueueJobStatus::Completed => ApiJobStatus::Completed,
        QueueJobStatus::Failed => ApiJobStatus::Failed,
        QueueJobStatus::Interrupted => ApiJobStatus::Interrupted,
    }
}

/// Convert a Job to JobResponse.
fn job_to_response(job: &Job, streamer_name: Option<String>) -> JobResponse {
    JobResponse {
        id: job.id.clone(),
        session_id: job.session_id.clone(),
        streamer_id: job.streamer_id.clone(),
        streamer_name,
        pipeline_id: job.pipeline_id.clone(),
        status: queue_status_to_api_status(job.status),
        processor_type: job.job_type.clone(),
        input_path: job.inputs.clone(),
        output_path: if job.outputs.is_empty() {
            None
        } else {
            Some(job.outputs.clone())
        },
        error_message: job.error.clone(),
        progress: Some(0.0), // Progress tracking not implemented yet, default to 0.0
        created_at: job.created_at,
        started_at: job.started_at,
        completed_at: job.completed_at,
        execution_info: job.execution_info.as_ref().map(|info| ApiJobExecutionInfo {
            current_processor: info.current_processor.clone(),
            current_step: info.current_step,
            total_steps: info.total_steps,
            items_produced: info.items_produced.clone(),
            input_size_bytes: info.input_size_bytes,
            output_size_bytes: info.output_size_bytes,
            logs: info
                .logs
                .iter()
                .map(|log| ApiJobLogEntry {
                    timestamp: log.timestamp,
                    level: format!("{:?}", log.level),
                    message: log.message.clone(),
                })
                .collect(),
            log_lines_total: info.log_lines_total,
            log_warn_count: info.log_warn_count,
            log_error_count: info.log_error_count,
            step_durations: info
                .step_durations
                .iter()
                .map(|sd| ApiStepDurationInfo {
                    step: sd.step,
                    processor: sd.processor.clone(),
                    duration_secs: sd.duration_secs,
                    started_at: sd.started_at,
                    completed_at: sd.completed_at,
                })
                .collect(),
        }),
        duration_secs: job.duration_secs,
        queue_wait_secs: job.queue_wait_secs,
    }
}

/// Helper to batch-fetch streamer names for a list of jobs.
async fn fetch_streamer_names(state: &AppState, jobs: &[Job]) -> HashMap<String, String> {
    let streamer_repository = match &state.streamer_repository {
        Some(repo) => repo.clone(),
        None => return HashMap::new(),
    };

    // Collect unique streamer IDs
    let streamer_ids: HashSet<String> = jobs.iter().map(|j| j.streamer_id.clone()).collect();

    // Fetch streamers in parallel
    let fetches = streamer_ids.into_iter().map(|streamer_id| {
        let repo = streamer_repository.clone();
        async move {
            let name = repo.get_streamer(&streamer_id).await.ok().map(|s| s.name);
            (streamer_id, name)
        }
    });

    join_all(fetches)
        .await
        .into_iter()
        .filter_map(|(id, name)| name.map(|n| (id, n)))
        .collect()
}

// ============================================================================
// Pipeline Preset Handlers (Workflow Sequences)
// ============================================================================

/// List available pipeline presets (workflow sequences).
///
/// # Endpoint
///
/// `GET /api/pipeline/presets`
///
/// # Query Parameters
///
/// - `search` - Search query for name or description (optional)
/// - `limit` - Number of items to return (default: 20, max: 100)
/// - `offset` - Number of items to skip (default: 0)
///
/// # Response
///
/// Returns a paginated list of available pipeline presets.
async fn list_pipeline_presets(
    State(state): State<AppState>,
    Query(filters): Query<PipelinePresetFilterParams>,
    Query(pagination): Query<PipelinePresetPaginationParams>,
) -> ApiResult<Json<PipelinePresetListResponse>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

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

/// Get a pipeline preset by ID.
///
/// # Endpoint
///
/// `GET /api/pipeline/presets/{id}`
async fn get_pipeline_preset_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    let preset = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    Ok(Json(PipelinePresetResponse::from(preset)))
}

/// Create a new DAG pipeline preset.
///
/// # Endpoint
///
/// `POST /api/pipeline/presets`
///
/// Creates a new pipeline preset as a DAG (Directed Acyclic Graph).
async fn create_pipeline_preset(
    State(state): State<AppState>,
    Json(payload): Json<CreatePipelinePresetRequest>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    // Validate DAG has at least one step
    if payload.dag.steps.is_empty() {
        return Err(ApiError::bad_request(
            "DAG pipeline preset must have at least one step",
        ));
    }

    // Create DAG preset
    let mut preset = crate::database::models::PipelinePreset::new_dag(payload.name, payload.dag);
    if let Some(desc) = payload.description {
        preset = preset.with_description(desc);
    }

    preset_repo
        .create_pipeline_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(PipelinePresetResponse::from(preset)))
}

/// Update an existing DAG pipeline preset.
///
/// # Endpoint
///
/// `PUT /api/pipeline/presets/{id}`
async fn update_pipeline_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePipelinePresetRequest>,
) -> ApiResult<Json<PipelinePresetResponse>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

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
        steps: "[]".to_string(), // Empty legacy steps
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

/// Delete a pipeline preset.
///
/// # Endpoint
///
/// `DELETE /api/pipeline/presets/{id}`
async fn delete_pipeline_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<()>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    preset_repo
        .delete_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(()))
}

/// Preview jobs that would be created from a pipeline preset.
///
/// # Endpoint
///
/// `GET /api/pipeline/presets/{id}/preview`
///
/// Shows what jobs would be created when using this preset, including
/// the execution order and dependency relationships.
async fn preview_pipeline_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PresetPreviewResponse>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    let preset = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    let dag = preset
        .get_dag_definition()
        .ok_or_else(|| ApiError::internal("Pipeline preset has no DAG definition"))?;

    // Build dependency map for finding leaf steps
    let mut has_dependents: HashSet<String> = HashSet::new();
    for step in &dag.steps {
        for dep in &step.depends_on {
            has_dependents.insert(dep.clone());
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
            let is_leaf = !has_dependents.contains(&step.id);

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

/// Get full DAG status with all steps.
///
/// # Endpoint
///
/// `GET /api/pipeline/dag/{dag_id}`
///
/// Returns the complete status of a DAG pipeline including all steps,
/// their current status, and progress information.
async fn get_dag_status(
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagStatusResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Get DAG execution
    let dag = dag_scheduler
        .get_dag_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Get all steps
    let steps = dag_scheduler
        .get_dag_steps(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Get DAG definition for step processor info
    let dag_def = dag.get_dag_definition();

    // Build step responses
    let step_responses: Vec<DagStepStatusResponse> = steps
        .iter()
        .map(|step| {
            let processor = dag_def.as_ref().and_then(|def| {
                def.steps
                    .iter()
                    .find(|s| s.id == step.step_id)
                    .map(|s| match &s.step {
                        PipelineStep::Preset { name } => name.clone(),
                        PipelineStep::Workflow { name } => format!("workflow:{}", name),
                        PipelineStep::Inline { processor, .. } => processor.clone(),
                    })
            });

            DagStepStatusResponse {
                step_id: step.step_id.clone(),
                status: step.status.clone(),
                job_id: step.job_id.clone(),
                depends_on: step.get_depends_on(),
                outputs: step.get_outputs(),
                processor,
            }
        })
        .collect();

    let name = dag_def
        .map(|d| d.name)
        .unwrap_or_else(|| "Unknown".to_string());
    let progress_percent = dag.progress_percent();

    Ok(Json(DagStatusResponse {
        id: dag.id,
        name,
        status: dag.status,
        streamer_id: dag.streamer_id,
        session_id: dag.session_id,
        total_steps: dag.total_steps,
        completed_steps: dag.completed_steps,
        failed_steps: dag.failed_steps,
        progress_percent,
        steps: step_responses,
        error: dag.error,
        created_at: dag.created_at,
        updated_at: dag.updated_at,
        completed_at: dag.completed_at,
    }))
}

/// Get DAG graph visualization data.
///
/// # Endpoint
///
/// `GET /api/pipeline/dag/{dag_id}/graph`
///
/// Returns nodes and edges for visualizing the DAG as a graph.
async fn get_dag_graph(
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagGraphResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Get DAG execution
    let dag = dag_scheduler
        .get_dag_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Get all steps
    let steps = dag_scheduler
        .get_dag_steps(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Get DAG definition for step processor info
    let dag_def = dag.get_dag_definition();
    let name = dag_def
        .as_ref()
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    // Build nodes
    let nodes: Vec<DagGraphNode> = steps
        .iter()
        .map(|step| {
            let processor = dag_def.as_ref().and_then(|def| {
                def.steps
                    .iter()
                    .find(|s| s.id == step.step_id)
                    .map(|s| match &s.step {
                        PipelineStep::Preset { name } => name.clone(),
                        PipelineStep::Workflow { name } => name.clone(),
                        PipelineStep::Inline { processor, .. } => processor.clone(),
                    })
            });

            let label = processor.clone().unwrap_or_else(|| step.step_id.clone());

            DagGraphNode {
                id: step.step_id.clone(),
                label,
                status: step.status.clone(),
                processor,
                job_id: step.job_id.clone(),
            }
        })
        .collect();

    // Build edges from dependencies
    let mut edges: Vec<DagGraphEdge> = Vec::new();
    for step in &steps {
        for dep in step.get_depends_on() {
            edges.push(DagGraphEdge {
                from: dep,
                to: step.step_id.clone(),
            });
        }
    }

    Ok(Json(DagGraphResponse {
        dag_id,
        name,
        nodes,
        edges,
    }))
}

/// Retry failed steps in a DAG.
///
/// # Endpoint
///
/// `POST /api/pipeline/dag/{dag_id}/retry`
///
/// Retries all failed steps in the DAG. This will:
/// 1. Reset failed steps to pending
/// 2. Create new jobs for those steps
/// 3. Resume DAG execution
async fn retry_dag(
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagRetryResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Get DAG execution
    let dag = dag_scheduler
        .get_dag_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Check if DAG has failed steps
    if dag.failed_steps == 0 {
        return Err(ApiError::bad_request("DAG has no failed steps to retry"));
    }

    // Get all steps
    let steps = dag_scheduler
        .get_dag_steps(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Find failed steps
    let failed_steps: Vec<_> = steps.iter().filter(|s| s.status == "FAILED").collect();

    if failed_steps.is_empty() {
        return Err(ApiError::bad_request("No failed steps found to retry"));
    }

    // Retry each failed job
    let mut job_ids = Vec::new();
    for step in &failed_steps {
        if let Some(job_id) = &step.job_id {
            match pipeline_manager.retry_job(job_id).await {
                Ok(job) => {
                    job_ids.push(job.id);
                }
                Err(e) => {
                    tracing::warn!("Failed to retry job {}: {}", job_id, e);
                }
            }
        }
    }

    let retried_steps = job_ids.len();
    let message = if retried_steps == failed_steps.len() {
        format!("Successfully retried {} failed steps", retried_steps)
    } else {
        format!(
            "Retried {} of {} failed steps",
            retried_steps,
            failed_steps.len()
        )
    };

    Ok(Json(DagRetryResponse {
        dag_id,
        retried_steps,
        job_ids,
        message,
    }))
}

/// List all DAG executions with filtering and pagination.
///
/// # Endpoint
///
/// `GET /api/pipeline/dags`
///
/// # Query Parameters
///
/// - `status` - Filter by DAG status (PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED)
/// - `limit` - Maximum number of results (default: 20, max: 100)
/// - `offset` - Number of results to skip (default: 0)
///
/// # Response
///
/// Returns a list of DAG executions matching the filter criteria.
async fn list_dags(
    State(state): State<AppState>,
    Query(filters): Query<DagFilterParams>,
    Query(pagination): Query<DagPaginationParams>,
) -> ApiResult<Json<DagListResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let effective_limit = pagination.limit.min(100);

    // Get DAGs from repository via scheduler
    let dags = dag_scheduler
        .list_dags(
            filters.status.as_deref(),
            effective_limit,
            pagination.offset,
        )
        .await
        .map_err(ApiError::from)?;

    // Convert to response format
    let dag_items: Vec<DagListItem> = dags
        .into_iter()
        .map(|dag| {
            let name = dag
                .get_dag_definition()
                .map(|d| d.name)
                .unwrap_or_else(|| "Unknown".to_string());
            let progress_percent = dag.progress_percent();

            DagListItem {
                id: dag.id,
                name,
                status: dag.status,
                streamer_id: dag.streamer_id,
                session_id: dag.session_id,
                total_steps: dag.total_steps,
                completed_steps: dag.completed_steps,
                failed_steps: dag.failed_steps,
                progress_percent,
                created_at: dag.created_at,
                updated_at: dag.updated_at,
            }
        })
        .collect();

    // Get total count for pagination
    let total = dag_scheduler
        .count_dags(filters.status.as_deref())
        .await
        .map_err(ApiError::from)?;

    Ok(Json(DagListResponse {
        dags: dag_items,
        total,
        limit: effective_limit,
        offset: pagination.offset,
    }))
}

/// Cancel a DAG execution.
///
/// # Endpoint
///
/// `DELETE /api/pipeline/dag/{dag_id}`
///
/// # Path Parameters
///
/// - `dag_id` - The DAG execution ID
///
/// # Response
///
/// Returns the number of steps that were cancelled.
///
/// # Behavior
///
/// - Cancels all pending and processing steps in the DAG
/// - Marks the DAG as CANCELLED
/// - Already completed or failed steps are not affected
async fn cancel_dag(
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagCancelResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let cancelled_steps = dag_scheduler
        .cancel_dag(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let message = if cancelled_steps == 0 {
        format!("DAG '{}' cancelled (no active steps to cancel)", dag_id)
    } else {
        format!(
            "DAG '{}' cancelled successfully ({} steps cancelled)",
            dag_id, cancelled_steps
        )
    };

    Ok(Json(DagCancelResponse {
        dag_id,
        cancelled_steps,
        message,
    }))
}

/// Get DAG step statistics.
///
/// # Endpoint
///
/// `GET /api/pipeline/dag/{dag_id}/stats`
///
/// # Path Parameters
///
/// - `dag_id` - The DAG execution ID
///
/// # Response
///
/// Returns statistics about the DAG's steps including counts by status.
async fn get_dag_stats(
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagStatsResponse>> {
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let stats = dag_scheduler
        .get_dag_stats(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let total = stats.blocked
        + stats.pending
        + stats.processing
        + stats.completed
        + stats.failed
        + stats.cancelled;
    let progress_percent = if total > 0 {
        (stats.completed as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    Ok(Json(DagStatsResponse {
        dag_id,
        blocked: stats.blocked,
        pending: stats.pending,
        processing: stats.processing,
        completed: stats.completed,
        failed: stats.failed,
        cancelled: stats.cancelled,
        total,
        progress_percent,
    }))
}

/// Validate a DAG definition.
///
/// # Endpoint
///
/// `POST /api/pipeline/validate`
///
/// Validates a DAG definition without creating it. Checks for:
/// - Cycles in the dependency graph
/// - Missing dependencies
/// - Empty DAG
/// - Duplicate step IDs
async fn validate_dag(
    State(_state): State<AppState>,
    Json(request): Json<ValidateDagRequest>,
) -> ApiResult<Json<ValidateDagResponse>> {
    let dag = &request.dag;
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Check for empty DAG
    if dag.steps.is_empty() {
        errors.push("DAG must have at least one step".to_string());
        return Ok(Json(ValidateDagResponse {
            valid: false,
            errors,
            warnings,
            root_steps: vec![],
            leaf_steps: vec![],
            max_depth: 0,
        }));
    }

    // Check for duplicate step IDs
    let mut step_ids: HashSet<String> = HashSet::new();
    for step in &dag.steps {
        if !step_ids.insert(step.id.clone()) {
            errors.push(format!("Duplicate step ID: {}", step.id));
        }
    }

    // Check for missing dependencies
    for step in &dag.steps {
        for dep in &step.depends_on {
            if !step_ids.contains(dep) {
                errors.push(format!(
                    "Step '{}' depends on non-existent step '{}'",
                    step.id, dep
                ));
            }
        }
    }

    // Check for self-dependencies
    for step in &dag.steps {
        if step.depends_on.contains(&step.id) {
            errors.push(format!("Step '{}' depends on itself", step.id));
        }
    }

    // Check for cycles using DFS
    if let Some(cycle) = detect_cycle(dag) {
        errors.push(format!("Cycle detected: {}", cycle.join(" -> ")));
    }

    // Find root steps (no dependencies)
    let root_steps: Vec<String> = dag
        .steps
        .iter()
        .filter(|s| s.depends_on.is_empty())
        .map(|s| s.id.clone())
        .collect();

    if root_steps.is_empty() && !dag.steps.is_empty() {
        errors.push("DAG has no root steps (all steps have dependencies)".to_string());
    }

    // Find leaf steps (no dependents)
    let mut has_dependents: HashSet<String> = HashSet::new();
    for step in &dag.steps {
        for dep in &step.depends_on {
            has_dependents.insert(dep.clone());
        }
    }
    let leaf_steps: Vec<String> = dag
        .steps
        .iter()
        .filter(|s| !has_dependents.contains(&s.id))
        .map(|s| s.id.clone())
        .collect();

    // Calculate max depth
    let max_depth = calculate_max_depth(dag);

    // Add warnings
    if dag.steps.len() == 1 {
        warnings.push("DAG has only one step - consider if a pipeline is necessary".to_string());
    }

    if max_depth > 10 {
        warnings.push(format!(
            "DAG has depth {} - deep pipelines may be slow",
            max_depth
        ));
    }

    Ok(Json(ValidateDagResponse {
        valid: errors.is_empty(),
        errors,
        warnings,
        root_steps,
        leaf_steps,
        max_depth,
    }))
}

// ============================================================================
// DAG Validation Helper Functions
// ============================================================================

/// Detect cycles in a DAG using DFS.
fn detect_cycle(dag: &DagPipelineDefinition) -> Option<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut rec_stack: HashSet<String> = HashSet::new();
    let mut path: Vec<String> = Vec::new();

    // Build adjacency list
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for step in &dag.steps {
        adj.insert(step.id.clone(), step.depends_on.clone());
    }

    fn dfs(
        node: &str,
        adj: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        rec_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        visited.insert(node.to_string());
        rec_stack.insert(node.to_string());
        path.push(node.to_string());

        if let Some(deps) = adj.get(node) {
            for dep in deps {
                if !visited.contains(dep) {
                    if let Some(cycle) = dfs(dep, adj, visited, rec_stack, path) {
                        return Some(cycle);
                    }
                } else if rec_stack.contains(dep) {
                    // Found cycle
                    let mut cycle = path.clone();
                    cycle.push(dep.clone());
                    return Some(cycle);
                }
            }
        }

        path.pop();
        rec_stack.remove(node);
        None
    }

    for step in &dag.steps {
        if !visited.contains(&step.id) {
            if let Some(cycle) = dfs(&step.id, &adj, &mut visited, &mut rec_stack, &mut path) {
                return Some(cycle);
            }
        }
    }

    None
}

/// Calculate the maximum depth of a DAG.
fn calculate_max_depth(dag: &DagPipelineDefinition) -> usize {
    let mut depths: HashMap<String, usize> = HashMap::new();

    // Build adjacency list (reverse - from dependency to dependent)
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for step in &dag.steps {
        dependents.entry(step.id.clone()).or_default();
        for dep in &step.depends_on {
            dependents
                .entry(dep.clone())
                .or_default()
                .push(step.id.clone());
        }
    }

    // Find root steps
    let roots: Vec<String> = dag
        .steps
        .iter()
        .filter(|s| s.depends_on.is_empty())
        .map(|s| s.id.clone())
        .collect();

    // BFS to calculate depths
    let mut queue: std::collections::VecDeque<String> = roots.into_iter().collect();
    for step in &dag.steps {
        if step.depends_on.is_empty() {
            depths.insert(step.id.clone(), 1);
        }
    }

    while let Some(node) = queue.pop_front() {
        let current_depth = *depths.get(&node).unwrap_or(&1);
        if let Some(deps) = dependents.get(&node) {
            for dep in deps {
                let new_depth = current_depth + 1;
                let existing = depths.entry(dep.clone()).or_insert(0);
                if new_depth > *existing {
                    *existing = new_depth;
                    queue.push_back(dep.clone());
                }
            }
        }
    }

    depths.values().copied().max().unwrap_or(0)
}

/// Topologically sort DAG steps.
fn topological_sort(dag: &DagPipelineDefinition) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    // Build adjacency list
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for step in &dag.steps {
        adj.insert(step.id.clone(), step.depends_on.clone());
    }

    fn visit(
        node: &str,
        adj: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        result: &mut Vec<String>,
    ) {
        if visited.contains(node) {
            return;
        }
        visited.insert(node.to_string());

        if let Some(deps) = adj.get(node) {
            for dep in deps {
                visit(dep, adj, visited, result);
            }
        }

        result.push(node.to_string());
    }

    for step in &dag.steps {
        visit(&step.id, &adj, &mut visited, &mut result);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_stats_response_serialization() {
        let response = PipelineStatsResponse {
            pending_count: 10,
            processing_count: 2,
            completed_count: 100,
            failed_count: 5,
            avg_processing_time_secs: Some(45.5),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("pending_count"));
        assert!(json.contains("45.5"));
    }

    #[test]
    fn test_create_pipeline_request_deserialize() {
        let json = r#"{
            "session_id": "session-123",
            "streamer_id": "streamer-456",
            "input_path": "/recordings/stream.flv",
            "dag": {
                "name": "test_pipeline",
                "steps": [
                    {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []}
                ]
            }
        }"#;

        let request: CreatePipelineRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.session_id, "session-123");
        assert_eq!(request.streamer_id, "streamer-456");
        assert_eq!(request.input_path, "/recordings/stream.flv");
        assert_eq!(request.dag.name, "test_pipeline");
        assert_eq!(request.dag.steps.len(), 1);
    }

    #[test]
    fn test_create_pipeline_request_with_dag() {
        let json = r#"{
            "session_id": "session-123",
            "streamer_id": "streamer-456",
            "input_path": "/recordings/stream.flv",
            "dag": {
                "name": "my_pipeline",
                "steps": [
                    {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
                    {"id": "thumbnail", "step": {"type": "preset", "name": "thumbnail"}, "depends_on": ["remux"]},
                    {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux", "thumbnail"]}
                ]
            }
        }"#;

        let request: CreatePipelineRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.dag.name, "my_pipeline");
        assert_eq!(request.dag.steps.len(), 3);
        assert_eq!(request.dag.steps[0].id, "remux");
        assert!(request.dag.steps[0].depends_on.is_empty());
        assert_eq!(request.dag.steps[1].id, "thumbnail");
        assert_eq!(request.dag.steps[1].depends_on, vec!["remux"]);
        assert_eq!(request.dag.steps[2].id, "upload");
        assert_eq!(request.dag.steps[2].depends_on, vec!["remux", "thumbnail"]);
    }

    #[test]
    fn test_api_status_to_db_status() {
        assert_eq!(
            api_status_to_db_status(ApiJobStatus::Pending),
            DbJobStatus::Pending
        );
        assert_eq!(
            api_status_to_db_status(ApiJobStatus::Processing),
            DbJobStatus::Processing
        );
        assert_eq!(
            api_status_to_db_status(ApiJobStatus::Completed),
            DbJobStatus::Completed
        );
        assert_eq!(
            api_status_to_db_status(ApiJobStatus::Failed),
            DbJobStatus::Failed
        );
        assert_eq!(
            api_status_to_db_status(ApiJobStatus::Interrupted),
            DbJobStatus::Interrupted
        );
    }

    #[test]
    fn test_queue_status_to_api_status() {
        assert_eq!(
            queue_status_to_api_status(QueueJobStatus::Pending),
            ApiJobStatus::Pending
        );
        assert_eq!(
            queue_status_to_api_status(QueueJobStatus::Processing),
            ApiJobStatus::Processing
        );
        assert_eq!(
            queue_status_to_api_status(QueueJobStatus::Completed),
            ApiJobStatus::Completed
        );
        assert_eq!(
            queue_status_to_api_status(QueueJobStatus::Failed),
            ApiJobStatus::Failed
        );
        assert_eq!(
            queue_status_to_api_status(QueueJobStatus::Interrupted),
            ApiJobStatus::Interrupted
        );
    }

    // ========================================================================
    // DAG Validation Tests
    // ========================================================================

    #[test]
    fn test_detect_cycle_no_cycle() {
        let dag = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string()],
                ),
            ],
        );

        assert!(detect_cycle(&dag).is_none());
    }

    #[test]
    fn test_detect_cycle_with_cycle() {
        let dag = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep::with_dependencies(
                    "A",
                    PipelineStep::preset("remux"),
                    vec!["C".to_string()],
                ),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string()],
                ),
            ],
        );

        let cycle = detect_cycle(&dag);
        assert!(cycle.is_some());
    }

    #[test]
    fn test_calculate_max_depth_linear() {
        let dag = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string()],
                ),
            ],
        );

        assert_eq!(calculate_max_depth(&dag), 3);
    }

    #[test]
    fn test_calculate_max_depth_parallel() {
        // A and B run in parallel, C depends on both
        let dag = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::new("B", PipelineStep::preset("thumbnail")),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string(), "B".to_string()],
                ),
            ],
        );

        assert_eq!(calculate_max_depth(&dag), 2);
    }

    #[test]
    fn test_topological_sort() {
        let dag = DagPipelineDefinition::new(
            "test",
            vec![
                DagStep::new("A", PipelineStep::preset("remux")),
                DagStep::with_dependencies(
                    "B",
                    PipelineStep::preset("upload"),
                    vec!["A".to_string()],
                ),
                DagStep::with_dependencies(
                    "C",
                    PipelineStep::preset("notify"),
                    vec!["B".to_string()],
                ),
            ],
        );

        let order = topological_sort(&dag);

        // A must come before B, B must come before C
        let pos_a = order.iter().position(|x| x == "A").unwrap();
        let pos_b = order.iter().position(|x| x == "B").unwrap();
        let pos_c = order.iter().position(|x| x == "C").unwrap();

        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn test_validate_dag_request_deserialize() {
        let json = r#"{
            "dag": {
                "name": "test_pipeline",
                "steps": [
                    {"id": "remux", "step": {"type": "preset", "name": "remux"}, "depends_on": []},
                    {"id": "upload", "step": {"type": "preset", "name": "upload"}, "depends_on": ["remux"]}
                ]
            }
        }"#;

        let request: ValidateDagRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.dag.name, "test_pipeline");
        assert_eq!(request.dag.steps.len(), 2);
    }
}
