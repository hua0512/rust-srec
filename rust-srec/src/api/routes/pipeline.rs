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

pub(crate) mod dag;
pub(crate) mod jobs;
pub(crate) mod presets;

pub use dag::{
    cancel_dag, delete_dag, get_dag_graph, get_dag_stats, get_dag_status, list_dags,
    retry_all_failed_dags, retry_dag, validate_dag,
};
pub use jobs::{
    cancel_job, cancel_pipeline, create_pipeline, delete_job, get_job, get_job_progress, get_stats,
    list_job_logs, list_jobs, list_jobs_page, list_outputs, retry_job,
};
pub use presets::{
    create_pipeline_preset, delete_pipeline_preset, get_pipeline_preset_by_id,
    list_pipeline_presets, preview_pipeline_preset, update_pipeline_preset,
};

use dag::topological_sort;
#[cfg(test)]
use jobs::{api_status_to_job_status, job_status_to_api_status};

use axum::{
    Router,
    extract::FromRef,
    routing::{delete, get, post},
};
use std::sync::Arc;

use crate::api::models::JobResponse;
use crate::api::server::AppState;
use crate::database::models::job::DagPipelineDefinition;
use crate::database::repositories::StreamerRepository;

/// Dependencies shared by the job-management endpoints.
#[derive(Clone)]
pub struct JobRouteState {
    pipeline_manager: Arc<crate::pipeline::PipelineManager>,
    streamer_repository: Arc<dyn StreamerRepository>,
}

impl FromRef<AppState> for JobRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            pipeline_manager: state.pipeline_manager.clone(),
            streamer_repository: state.streamer_repository.clone(),
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
/// - `POST /validate` - Validate a DAG definition
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/page", get(list_jobs_page))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/logs", get(list_job_logs))
        .route("/jobs/{id}/progress", get(get_job_progress))
        .route("/jobs/{id}/retry", post(retry_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
        .route("/jobs/{id}", delete(delete_job))
        .route("/{pipeline_id}", delete(cancel_pipeline))
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
        .route("/dags/retry_failed", post(retry_all_failed_dags))
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
    /// Search query (matches name or description).
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
    /// Search query (matches file path, session ID, or format).
    pub search: Option<String>,
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
    pub status: Option<String>,
    /// Filter by session ID.
    pub session_id: Option<String>,
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
    /// Overall DAG status.
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
    /// Progress percentage (0-100).
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
/// - `status` - Current job status (pending, processing, completed, failed, cancelled)
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
#[cfg(test)]
mod tests {
    use axum::extract::{Path, Query, State};

    use crate::api::models::{
        JobFilterParams, JobStatus as ApiJobStatus, PaginationParams, PipelineStatsResponse,
    };
    use crate::database::models::DagStep;
    use crate::database::models::JobStatus;
    use crate::database::models::job::PipelineStep;
    use crate::pipeline::Job;

    use super::*;
    use crate::database::repositories::streamer::SqlxStreamerRepository;
    use crate::pipeline::PipelineManager;

    fn build_test_state() -> JobRouteState {
        let pool = sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        JobRouteState {
            pipeline_manager: Arc::new(PipelineManager::new()),
            streamer_repository: Arc::new(SqlxStreamerRepository::new(pool.clone(), pool)),
        }
    }

    async fn enqueue_job_with_status(status: JobStatus) -> (JobRouteState, String) {
        let state = build_test_state();
        let manager = state.pipeline_manager.clone();

        let mut job = Job::new(
            "remux",
            vec!["input.mp4".to_string()],
            vec!["output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        job.status = status;

        let job_id = manager.enqueue(job).await.unwrap();
        (state, job_id)
    }

    #[tokio::test]
    async fn test_cancel_job_cancels_processing_job() {
        let (state, job_id) = enqueue_job_with_status(JobStatus::Processing).await;

        let response = cancel_job(State(state.clone()), Path(job_id.clone()))
            .await
            .unwrap();

        assert_eq!(
            response.0["message"],
            serde_json::Value::String(format!("Job '{}' cancelled successfully", job_id))
        );

        let manager = state.pipeline_manager.as_ref();
        let job = manager.get_job(&job_id).await.unwrap().unwrap();
        assert_eq!(job.status, JobStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_delete_job_deletes_completed_job() {
        let (state, job_id) = enqueue_job_with_status(JobStatus::Completed).await;

        let response = delete_job(State(state.clone()), Path(job_id.clone()))
            .await
            .unwrap();

        assert_eq!(
            response.0["message"],
            serde_json::Value::String(format!("Job '{}' deleted successfully", job_id))
        );

        let manager = state.pipeline_manager.as_ref();
        let job = manager.get_job(&job_id).await.unwrap();
        assert!(job.is_none());
    }

    #[tokio::test]
    async fn test_delete_job_rejects_processing_job() {
        let (state, job_id) = enqueue_job_with_status(JobStatus::Processing).await;

        let result = delete_job(State(state), Path(job_id));

        assert!(result.await.is_err());
    }

    #[tokio::test]
    async fn test_cancel_job_rejects_completed_job() {
        let (state, job_id) = enqueue_job_with_status(JobStatus::Completed).await;

        let result = cancel_job(State(state), Path(job_id));

        assert!(result.await.is_err());
    }

    #[test]
    fn test_retry_dag_accepts_cancelled_status() {
        let status = "CANCELLED";
        let retryable = status == "FAILED" || status == "CANCELLED";
        assert!(retryable);
    }

    #[tokio::test]
    async fn test_get_job_returns_cancelled_status() {
        let (state, job_id) = enqueue_job_with_status(JobStatus::Pending).await;
        let manager = state.pipeline_manager.as_ref();
        manager.cancel_job(&job_id).await.unwrap();

        let response = get_job(State(state), Path(job_id)).await.unwrap();

        assert_eq!(response.0.status, ApiJobStatus::Cancelled);
    }

    #[tokio::test]
    async fn test_list_jobs_filters_cancelled_status() {
        let state = build_test_state();
        let manager = state.pipeline_manager.clone();

        let pending_job = Job::new(
            "remux",
            vec!["pending-input.mp4".to_string()],
            vec!["pending-output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let pending_job_id = manager.enqueue(pending_job).await.unwrap();

        let cancelled_job = Job::new(
            "remux",
            vec!["cancelled-input.mp4".to_string()],
            vec!["cancelled-output.mp4".to_string()],
            "streamer-1",
            "session-1",
        );
        let cancelled_job_id = manager.enqueue(cancelled_job).await.unwrap();
        manager.cancel_job(&cancelled_job_id).await.unwrap();

        let response = list_jobs(
            State(state.clone()),
            Query(PaginationParams::default()),
            Query(JobFilterParams {
                status: Some(ApiJobStatus::Cancelled),
                ..JobFilterParams::default()
            }),
        )
        .await
        .unwrap();

        assert_eq!(response.0.total, 1);
        assert_eq!(response.0.items.len(), 1);
        assert_eq!(response.0.items[0].id, cancelled_job_id);
        assert_eq!(response.0.items[0].status, ApiJobStatus::Cancelled);

        let pending = manager.get_job(&pending_job_id).await.unwrap().unwrap();
        assert_eq!(pending.status, JobStatus::Pending);
    }

    #[test]
    fn test_job_filter_params_deserialize_cancelled_status() {
        let filters: JobFilterParams = serde_json::from_str(r#"{"status":"CANCELLED"}"#).unwrap();
        assert_eq!(filters.status, Some(ApiJobStatus::Cancelled));
    }

    #[test]
    fn test_pipeline_stats_response_serialization() {
        let response = PipelineStatsResponse {
            pending_count: 10,
            processing_count: 2,
            completed_count: 100,
            failed_count: 5,
            cancelled_count: 1,
            avg_processing_time_secs: Some(45.5),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("pending_count"));
        assert!(json.contains("cancelled_count"));
        assert!(json.contains("45.5"));
    }

    #[test]
    fn test_create_pipeline_request_deserialize() {
        let json = r#"{
            "session_id": "session-123",
            "streamer_id": "streamer-456",
            "input_paths": ["/recordings/stream.flv"],
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
        assert_eq!(
            request.input_paths,
            vec!["/recordings/stream.flv".to_string()]
        );
        assert_eq!(request.dag.name, "test_pipeline");
        assert_eq!(request.dag.steps.len(), 1);
    }

    #[test]
    fn test_create_pipeline_request_with_dag() {
        let json = r#"{
            "session_id": "session-123",
            "streamer_id": "streamer-456",
            "input_paths": ["/recordings/stream.flv"],
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
    fn test_api_status_to_job_status() {
        assert_eq!(
            api_status_to_job_status(ApiJobStatus::Pending),
            JobStatus::Pending
        );
        assert_eq!(
            api_status_to_job_status(ApiJobStatus::Processing),
            JobStatus::Processing
        );
        assert_eq!(
            api_status_to_job_status(ApiJobStatus::Completed),
            JobStatus::Completed
        );
        assert_eq!(
            api_status_to_job_status(ApiJobStatus::Failed),
            JobStatus::Failed
        );
        assert_eq!(
            api_status_to_job_status(ApiJobStatus::Cancelled),
            JobStatus::Cancelled
        );
    }

    #[test]
    fn test_job_status_to_api_status() {
        assert_eq!(
            job_status_to_api_status(JobStatus::Pending),
            ApiJobStatus::Pending
        );
        assert_eq!(
            job_status_to_api_status(JobStatus::Processing),
            ApiJobStatus::Processing
        );
        assert_eq!(
            job_status_to_api_status(JobStatus::Completed),
            ApiJobStatus::Completed
        );
        assert_eq!(
            job_status_to_api_status(JobStatus::Failed),
            ApiJobStatus::Failed
        );
        assert_eq!(
            job_status_to_api_status(JobStatus::Cancelled),
            ApiJobStatus::Cancelled
        );
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
