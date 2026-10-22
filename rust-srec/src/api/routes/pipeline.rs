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
//! | POST | `/api/pipeline/jobs/{id}/retry` | Retry a failed or cancelled job |
//! | POST | `/api/pipeline/jobs/{id}/cancel` | Cancel an active job |
//! | DELETE | `/api/pipeline/jobs/{id}` | Delete a terminal job |
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
//! | POST | `/api/pipeline/dag/{dag_id}/retry` | Retry failed or cancelled steps in a DAG |
//! | DELETE | `/api/pipeline/dag/{dag_id}` | Cancel a DAG execution and all its steps |
//! | POST | `/api/pipeline/dags/batch` | Cancel, retry or delete several DAG executions |
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
//! | GET | `/api/pipeline/outputs/summary` | Count and total size of matching outputs, per file type |
//! | DELETE | `/api/pipeline/outputs/{id}` | Delete a media output, optionally with its file |
//! | POST | `/api/pipeline/outputs/batch-delete` | Delete several media outputs |
//! | GET | `/api/pipeline/stats` | Get pipeline statistics |
//! | GET | `/api/pipeline/stalled-operations` | List ended jobs whose file operation still holds a worker |
//! | GET | `/api/pipeline/interrupted-outputs` | List partial files left by interrupted jobs |
//! | DELETE | `/api/pipeline/interrupted-outputs/{id}` | Delete or keep one partial file and forget it |
//! | POST | `/api/pipeline/interrupted-outputs/batch-delete` | Delete or keep several partial files |

pub(crate) mod dag;
pub(crate) mod interrupted;
pub(crate) mod jobs;
pub(crate) mod presets;

// Handler imports feed `router()` below; external code (e.g.
// `api::openapi::ApiDoc`) addresses handlers through the `dag`/`jobs`/
// `presets` submodules directly, so no re-export is needed.
use dag::{
    batch_dags, cancel_dag, delete_dag, get_dag_graph, get_dag_list_stats, get_dag_stats,
    get_dag_status, list_dags, retry_all_failed_dags, retry_dag, validate_dag,
};
use jobs::{
    batch_delete_outputs, cancel_job, cancel_pipeline, create_pipeline, delete_job, delete_output,
    get_job, get_job_progress, get_stats, list_job_logs, list_job_uploads, list_jobs,
    list_jobs_page, list_outputs, list_stalled_operations, retry_job, summarize_outputs,
};
use presets::{
    create_pipeline_preset, delete_pipeline_preset, get_pipeline_preset_by_id,
    list_pipeline_presets, preview_pipeline_preset, update_pipeline_preset,
};

use interrupted::{
    batch_delete_interrupted_outputs, delete_interrupted_output, list_interrupted_outputs,
};

use dag::topological_sort;

use axum::{
    Router,
    extract::FromRef,
    routing::{delete, get, post},
};
use std::sync::Arc;

use crate::api::models::{JobResponse, JobStatus as ApiJobStatus};
use crate::api::server::AppState;
use crate::database::models::job::DagPipelineDefinition;
use crate::database::repositories::{
    PipelinePresetRepository, SessionRepository, StreamerRepository, UploadRecordRepository,
};
use crate::pipeline::JobProgressSnapshot;

/// Dependencies shared by the job and DAG execution endpoints in `jobs` and
/// `dag` (everything routed through `pipeline_manager`, plus
/// `streamer_repository` for resolving streamer display names).
#[derive(Clone)]
pub struct PipelineRouteState {
    pipeline_manager: Arc<crate::pipeline::PipelineManager>,
    streamer_repository: Arc<dyn StreamerRepository>,
}

impl FromRef<AppState> for PipelineRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            pipeline_manager: state.pipeline_manager.clone(),
            streamer_repository: state.streamer_repository.clone(),
        }
    }
}

/// Dependencies of the pipeline-preset endpoints in `presets`: the preset
/// store, plus `pipeline_manager` to check a saved definition the way DAG
/// creation will.
#[derive(Clone)]
pub struct PresetRouteState {
    pipeline_preset_repository: Arc<dyn PipelinePresetRepository>,
    pipeline_manager: Arc<crate::pipeline::PipelineManager>,
}

impl FromRef<AppState> for PresetRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            pipeline_preset_repository: state.pipeline_preset_repository.clone(),
            pipeline_manager: state.pipeline_manager.clone(),
        }
    }
}

/// Dependencies of `jobs::list_outputs`, which reads media outputs from the
/// session store rather than from `pipeline_manager`, plus the upload-record
/// store used to annotate each output with its upload results.
#[derive(Clone)]
pub struct OutputRouteState {
    session_repository: Arc<dyn SessionRepository>,
    upload_record_repository: Arc<dyn UploadRecordRepository>,
}

impl FromRef<AppState> for OutputRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            session_repository: state.session_repository.clone(),
            upload_record_repository: state.upload_record_repository.clone(),
        }
    }
}

/// Dependencies of `jobs::list_job_uploads` (durable per-file upload results).
#[derive(Clone)]
pub struct UploadRouteState {
    upload_record_repository: Arc<dyn UploadRecordRepository>,
}

impl FromRef<AppState> for UploadRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            upload_record_repository: state.upload_record_repository.clone(),
        }
    }
}

/// Create the pipeline router (DAG-native).
///
/// # Routes
///
/// - `GET /jobs` - List jobs with filtering and pagination
/// - `GET /jobs/{id}` - Get a single job by ID
/// - `POST /jobs/{id}/retry` - Retry a failed or cancelled job
/// - `POST /jobs/{id}/cancel` - Cancel an active job
/// - `DELETE /jobs/{id}` - Delete a terminal job
/// - `DELETE /{pipeline_id}` - Cancel all jobs in a DAG pipeline
/// - `GET /pipelines` - List DAG pipelines with filtering and pagination
/// - `GET /outputs` - List media outputs
/// - `GET /stats` - Get pipeline statistics
/// - `GET /stalled-operations` - List ended jobs whose file operation still holds a worker
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
/// - `POST /dag/{dag_id}/retry` - Retry failed or cancelled steps in a DAG
/// - `DELETE /dag/{dag_id}` - Cancel a DAG execution
/// - `POST /dags/batch` - Cancel, retry or delete several DAG executions
/// - `DELETE /outputs/{id}` - Delete a media output, optionally with its file
/// - `POST /outputs/batch-delete` - Delete several media outputs
/// - `GET /interrupted-outputs` - List partial files left by interrupted jobs
/// - `DELETE /interrupted-outputs/{id}` - Delete or keep one partial file
/// - `POST /interrupted-outputs/batch-delete` - Delete or keep several partial files
/// - `POST /validate` - Validate a DAG definition
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/page", get(list_jobs_page))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/logs", get(list_job_logs))
        .route("/jobs/{id}/progress", get(get_job_progress))
        .route("/jobs/{id}/uploads", get(list_job_uploads))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}", delete(delete_job))
        .route("/{pipeline_id}", delete(cancel_pipeline))
        .route("/outputs", get(list_outputs))
        .route("/outputs/summary", get(summarize_outputs))
        .route("/outputs/batch-delete", post(batch_delete_outputs))
        .route("/outputs/{id}", delete(delete_output))
        .route("/interrupted-outputs", get(list_interrupted_outputs))
        .route(
            "/interrupted-outputs/batch-delete",
            post(batch_delete_interrupted_outputs),
        )
        .route(
            "/interrupted-outputs/{id}",
            delete(delete_interrupted_output),
        )
        .route("/stats", get(get_stats))
        .route("/stalled-operations", get(list_stalled_operations))
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
        .route("/dags/stats", get(get_dag_list_stats))
        .route("/dags/retry_failed", post(retry_all_failed_dags))
        .route("/dags/batch", post(batch_dags))
        .route("/dag/{dag_id}", get(get_dag_status).delete(cancel_dag))
        .route("/dag/{dag_id}/delete", delete(delete_dag))
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
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CreatePipelineRequest {
    /// Session ID for the pipeline.
    pub session_id: String,
    /// Streamer ID for the pipeline.
    pub streamer_id: String,
    /// Input file paths.
    pub input_paths: Vec<String>,
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct CreatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// DAG pipeline definition.
    pub dag: DagPipelineDefinition,
}

/// Request body for updating a DAG pipeline preset.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct UpdatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// DAG pipeline definition.
    pub dag: DagPipelineDefinition,
}

/// Query parameters for filtering pipeline presets.
#[derive(Debug, Clone, serde::Deserialize, Default, utoipa::IntoParams)]
pub struct PipelinePresetFilterParams {
    /// Literal substring of name or description; ASCII case-insensitive.
    pub search: Option<String>,
}

/// Pagination parameters for pipeline preset list.
#[derive(Debug, Clone, serde::Deserialize, utoipa::IntoParams)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
            // Default to empty DAG if missing
            DagPipelineDefinition::new(&preset.name, vec![])
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
/// ```text
/// GET /api/pipeline/outputs?session_id=session-123&streamer_id=streamer-456
/// ```
#[derive(Debug, Clone, serde::Deserialize, Default, utoipa::IntoParams)]
pub struct OutputFilterParams {
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Filter by streamer ID.
    pub streamer_id: Option<String>,
    /// Filter by media file type: `VIDEO`, `AUDIO`, `THUMBNAIL`, or `DANMU_XML`.
    /// This is the same value the list response returns as `format`.
    pub file_type: Option<String>,
    /// Literal substring of file path, session ID, or file type; ASCII case-insensitive.
    pub search: Option<String>,
}

/// Media outputs of one file type matching a filter, with their combined size.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct MediaOutputTypeSummaryResponse {
    /// Media file type: `VIDEO`, `AUDIO`, `THUMBNAIL`, or `DANMU_XML`.
    pub file_type: String,
    /// Number of matching outputs of this type.
    pub count: u64,
    /// Combined size of those outputs, in bytes.
    pub size_bytes: u64,
}

/// Totals for the media outputs matching a filter, overall and per file type.
///
/// `by_type` ignores the `file_type` filter so the caller can render a type
/// picker with stable counts while one type is selected.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct MediaOutputSummaryResponse {
    /// Number of matching outputs across all types.
    pub total_count: u64,
    /// Combined size of all matching outputs, in bytes.
    pub total_size_bytes: u64,
    /// Per-type breakdown. Types with no matching outputs are omitted.
    pub by_type: Vec<MediaOutputTypeSummaryResponse>,
}

// ============================================================================
// DAG Status and Graph Response Types
// ============================================================================

/// Response for DAG status with all steps.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagStatusResponse {
    /// DAG execution ID.
    pub id: String,
    /// DAG name from definition.
    pub name: String,
    /// Overall DAG status; a DAG that is not finished is PENDING until one of
    /// its jobs is running.
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
    /// Progress percentage (0-100): completed steps count fully, running steps
    /// by their reported progress, and every other step not at all.
    pub progress_percent: f64,
    /// All steps in the DAG with their status.
    pub steps: Vec<DagStepStatusResponse>,
    /// Error message if DAG failed.
    pub error: Option<String>,
    /// When the DAG was created.
    pub created_at: i64,
    /// When the DAG was last updated.
    pub updated_at: i64,
    /// When the DAG completed (if finished).
    pub completed_at: Option<i64>,
}

/// Response for a single DAG step status.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagStepStatusResponse {
    /// Step ID within the DAG.
    pub step_id: String,
    /// Step status: BLOCKED, PENDING, PROCESSING, COMPLETED, FAILED or CANCELLED.
    pub status: String,
    /// Associated job ID (if job has been created).
    pub job_id: Option<String>,
    /// Status of the step's job; null while the step has no job. A PROCESSING
    /// step whose job is PENDING is queued, and one whose job is FAILED waits
    /// for its automatic retry when `retry_after` is set.
    pub job_status: Option<ApiJobStatus>,
    /// When the job's latest attempt started, in epoch milliseconds.
    pub started_at: Option<i64>,
    /// When a step waiting for its automatic retry runs again, in epoch
    /// milliseconds; null when no retry will run, including in a cancelled or
    /// failed workflow.
    pub retry_after: Option<i64>,
    /// The running job's latest progress snapshot, with `raw` always null. Set
    /// only while the step and its job are both PROCESSING and the job has
    /// reported progress; null does not mean 0%.
    pub progress: Option<JobProgressSnapshot>,
    /// Step IDs this step depends on.
    pub depends_on: Vec<String>,
    /// Output paths produced by this step.
    pub outputs: Vec<String>,
    /// The processor type for this step.
    pub processor: Option<String>,
}

/// Response for DAG graph visualization.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagGraphEdge {
    /// Source step ID (dependency).
    pub from: String,
    /// Target step ID (dependent).
    pub to: String,
}

/// Response for DAG retry operation.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct ValidateDagRequest {
    /// DAG definition to validate.
    pub dag: DagPipelineDefinition,
}

/// Response for DAG validation.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Deserialize, Default, utoipa::IntoParams)]
pub struct DagFilterParams {
    /// Filter by DAG status (PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED).
    /// A DAG that is not finished is PENDING until one of its jobs is running.
    pub status: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
    /// Case-insensitive substring of the DAG's ID, name, session ID, streamer
    /// ID, or streamer name.
    pub search: Option<String>,
}

/// Pagination parameters for DAG list.
#[derive(Debug, Clone, serde::Deserialize, utoipa::IntoParams)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagListItem {
    /// DAG execution ID.
    pub id: String,
    /// DAG name from definition.
    pub name: String,
    /// Overall DAG status; a DAG that is not finished is PENDING until one of
    /// its jobs is running.
    pub status: String,
    /// Associated streamer ID.
    pub streamer_id: Option<String>,
    /// Associated streamer name.
    pub streamer_name: Option<String>,
    /// Associated session ID.
    pub session_id: Option<String>,
    /// Total number of steps.
    pub total_steps: i32,
    /// Number of completed steps.
    pub completed_steps: i32,
    /// Number of failed steps.
    pub failed_steps: i32,
    /// Progress percentage (0-100): completed steps count fully, running steps
    /// by their reported progress, and every other step not at all.
    pub progress_percent: f64,
    /// When the DAG was created.
    pub created_at: i64,
    /// When the DAG was last updated.
    pub updated_at: i64,
}

/// Response for DAG cancellation.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagCancelResponse {
    /// DAG execution ID.
    pub dag_id: String,
    /// Number of steps that were cancelled.
    pub cancelled_steps: u64,
    /// Message describing the cancellation.
    pub message: String,
}

/// DAG execution counts by the status shown in the DAG list.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DagListStatsResponse {
    /// DAGs that are not finished and have no job running.
    pub pending_count: u64,
    /// DAGs with at least one job running.
    pub processing_count: u64,
    pub completed_count: u64,
    pub failed_count: u64,
    pub cancelled_count: u64,
    /// Average time from creation to completion of completed DAGs, in seconds.
    pub avg_duration_secs: Option<f64>,
}

/// Response for DAG step statistics.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
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
    /// Progress percentage (0-100): completed steps count fully, running steps
    /// by their reported progress, and every other step not at all.
    pub progress_percent: f64,
}

/// Action applied to every DAG in a batch request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BatchDagAction {
    /// Cancel a DAG that is still pending or processing.
    Cancel,
    /// Retry the failed or cancelled steps of a terminal DAG.
    Retry,
    /// Delete a DAG with its steps and jobs, cancelling it first when it is still running.
    Delete,
}

/// Request to apply one action to multiple DAG executions.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct BatchDagRequest {
    /// Unique DAG execution IDs to process.
    pub ids: Vec<String>,
    /// Action applied to each DAG.
    pub action: BatchDagAction,
}

/// Result of a batch action for one DAG.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchDagItemResult {
    /// DAG execution ID supplied in the request.
    pub id: String,
    /// Whether the action succeeded.
    pub success: bool,
    /// Stable error code when the action failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Human-readable error when the action failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Summary of a batch DAG action.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchDagResponse {
    /// Number of unique DAGs requested.
    pub requested: usize,
    /// Number of successful actions.
    pub succeeded: usize,
    /// Number of failed actions.
    pub failed: usize,
    /// Per-DAG action results.
    pub results: Vec<BatchDagItemResult>,
}

/// Request to delete multiple media outputs.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct BatchDeleteOutputsRequest {
    /// Unique media output IDs to delete.
    pub ids: Vec<String>,
    /// Whether to also remove each output's file from disk.
    #[serde(default)]
    pub delete_file: bool,
}

/// Summary of a batch media output deletion.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct BatchDeleteOutputsResponse {
    /// Number of unique outputs requested.
    pub requested: usize,
    /// Number of outputs deleted.
    pub succeeded: usize,
    /// Number of outputs that could not be deleted.
    pub failed: usize,
    /// Per-output deletion results.
    pub results: Vec<BatchDagItemResult>,
}

/// Response for a single media output deletion.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct DeleteOutputResponse {
    /// Media output ID that was deleted.
    pub id: String,
    /// Whether the output's file was removed from disk.
    pub file_deleted: bool,
    /// Message describing the deletion.
    pub message: String,
}

/// Query parameters for deleting a media output.
#[derive(Debug, Clone, Default, serde::Deserialize, utoipa::IntoParams)]
pub struct DeleteOutputParams {
    /// Whether to also remove the output's file from disk.
    #[serde(default)]
    pub delete_file: bool,
}

#[cfg(test)]
mod tests {
    #[test]
    fn stalled_operation_response_serialization() {
        let response = crate::api::models::StalledOperationListResponse {
            items: vec![crate::api::models::StalledOperationResponse {
                job_id: "job-1".to_string(),
                job_type: "copy_move".to_string(),
                processor: "CopyMoveProcessor".to_string(),
                worker_type: "io".to_string(),
                streamer_id: "streamer-1".to_string(),
                input_file_name: Some("part.flv".to_string()),
                input_count: 2,
                ended_at: chrono::DateTime::from_timestamp_millis(1_767_323_045_123).unwrap(),
                stalled_secs: 95,
            }],
        };

        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "items": [{
                    "job_id": "job-1",
                    "job_type": "copy_move",
                    "processor": "CopyMoveProcessor",
                    "worker_type": "io",
                    "streamer_id": "streamer-1",
                    "input_file_name": "part.flv",
                    "input_count": 2,
                    "ended_at": "2026-01-02T03:04:05.123Z",
                    "stalled_secs": 95
                }]
            })
        );
    }
}
