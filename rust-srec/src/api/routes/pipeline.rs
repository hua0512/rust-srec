//! Pipeline management routes.
//!
//! This module provides REST API endpoints for managing pipeline jobs,
//! including job listing, retrieval, retry, cancellation, and statistics.
//!
//! # Endpoints
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
//! | DELETE | `/api/pipeline/{pipeline_id}` | Cancel all jobs in a pipeline |
//! | GET | `/api/pipeline/pipelines` | List pipelines with filtering and pagination |
//! | GET | `/api/pipeline/outputs` | List media outputs with filtering |
//! | GET | `/api/pipeline/stats` | Get pipeline statistics |
//! | POST | `/api/pipeline/create` | Create a new pipeline |
//! | GET | `/api/pipeline/presets` | List pipeline presets (workflows) |
//! | GET | `/api/pipeline/presets/{id}` | Get a pipeline preset by ID |
//! | POST | `/api/pipeline/presets` | Create a pipeline preset |
//! | PUT | `/api/pipeline/presets/{id}` | Update a pipeline preset |
//! | DELETE | `/api/pipeline/presets/{id}` | Delete a pipeline preset |

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
use crate::database::models::job::PipelineStep;
use crate::database::models::{JobFilters, JobStatus as DbJobStatus, OutputFilters, Pagination};
use crate::pipeline::JobProgressSnapshot;
use crate::pipeline::{Job, JobStatus as QueueJobStatus};

/// Create the pipeline router.
///
/// # Routes
///
/// - `GET /jobs` - List jobs with filtering and pagination
/// - `GET /jobs/{id}` - Get a single job by ID
/// - `POST /jobs/{id}/retry` - Retry a failed job
/// - `DELETE /jobs/{id}` - Cancel a job
/// - `DELETE /{pipeline_id}` - Cancel all jobs in a pipeline
/// - `GET /pipelines` - List pipelines with filtering and pagination
/// - `GET /outputs` - List media outputs
/// - `GET /stats` - Get pipeline statistics
/// - `POST /create` - Create a new pipeline
/// - `GET /presets` - List pipeline presets (workflows)
/// - `GET /presets/{id}` - Get a pipeline preset by ID
/// - `POST /presets` - Create a pipeline preset
/// - `PUT /presets/{id}` - Update a pipeline preset
/// - `DELETE /presets/{id}` - Delete a pipeline preset
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
}

/// Request body for creating a new pipeline.
///
/// # Example
///
/// ```json
/// {
///     "session_id": "session-123",
///     "streamer_id": "streamer-456",
///     "input_path": "/recordings/stream.flv",
///     "steps": ["remux", "upload", "thumbnail"]
/// }
/// ```
///
/// # Fields
///
/// - `session_id` - The recording session ID this pipeline belongs to
/// - `streamer_id` - The streamer ID this pipeline belongs to
/// - `input_path` - Path to the input file to process
/// - `steps` - Optional list of processing steps. Requests without steps will be rejected.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePipelineRequest {
    /// Session ID for the pipeline.
    pub session_id: String,
    /// Streamer ID for the pipeline.
    pub streamer_id: String,
    /// Input file path.
    pub input_path: String,
    /// Pipeline steps (optional, but at least one step is required).
    pub steps: Option<Vec<PipelineStep>>,
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

/// Request body for creating a new pipeline preset.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Pipeline steps (list of job preset names).
    pub steps: Vec<String>,
}

/// Request body for updating a pipeline preset.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdatePipelinePresetRequest {
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Pipeline steps (list of job preset names).
    pub steps: Vec<String>,
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
    pub presets: Vec<crate::database::models::PipelinePreset>,
    /// Total number of presets matching the filter.
    pub total: u64,
    /// Number of items returned.
    pub limit: u32,
    /// Number of items skipped.
    pub offset: u32,
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
    pub session_id: Option<String>,
    pub status: String,
    pub job_count: i64,
    pub completed_count: i64,
    pub failed_count: i64,
    pub total_duration_secs: f64,
    pub created_at: String,
    pub updated_at: String,
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

    // Convert jobs to API response format
    let job_responses: Vec<JobResponse> = jobs.iter().map(job_to_response).collect();

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

    let job_responses: Vec<JobResponse> = jobs.iter().map(job_to_response).collect();
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

    let responses: Vec<PipelineSummaryResponse> = pipelines
        .into_iter()
        .map(|p| PipelineSummaryResponse {
            pipeline_id: p.pipeline_id,
            streamer_id: p.streamer_id,
            session_id: p.session_id,
            status: p.status,
            job_count: p.job_count,
            completed_count: p.completed_count,
            failed_count: p.failed_count,
            total_duration_secs: p.total_duration_secs,
            created_at: p.created_at,
            updated_at: p.updated_at,
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

    Ok(Json(job_to_response(&job)))
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

    Ok(Json(job_to_response(&job)))
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

    // Call PipelineManager.cancel_job
    pipeline_manager
        .cancel_job(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Job '{}' cancelled successfully", id)
    })))
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

/// Create a new pipeline with sequential job execution.
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
///     "steps": ["remux", "upload", "thumbnail"]
/// }
/// ```
///
/// # Fields
///
/// - `session_id` (required) - The recording session ID
/// - `streamer_id` (required) - The streamer ID
/// - `input_path` (required) - Path to the input file
/// - `steps` (optional) - Processing steps. Requests without steps are rejected.
///
/// # Response
///
/// Returns the pipeline ID and first job details.
///
/// ```json
/// {
///     "pipeline_id": "job-uuid-123",
///     "first_job": {
///         "id": "job-uuid-123",
///         "status": "pending",
///         "processor_type": "remux",
///         ...
///     }
/// }
/// ```
///
/// # Pipeline Execution
///
/// Jobs are executed sequentially. When a job completes, the next job in the
/// pipeline is created atomically within a database transaction. This ensures
/// crash-safe transitions between pipeline steps.
///
/// # Requirements
///
/// - 6.1: Persist job to database on enqueue
/// - 7.1: Create only first job with metadata for subsequent steps
/// - 7.5: Accept session_id, streamer_id, input_path, and optional steps
/// - 8.1: Create pipeline and return pipeline_id
/// - 8.2: Validate that at least one step is provided
async fn create_pipeline(
    State(state): State<AppState>,
    Json(request): Json<CreatePipelineRequest>,
) -> ApiResult<Json<CreatePipelineResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = state
        .pipeline_manager
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline service not available"))?;

    // Call PipelineManager.create_pipeline
    let result = pipeline_manager
        .create_pipeline(
            &request.session_id,
            &request.streamer_id,
            &request.input_path,
            request.steps,
        )
        .await
        .map_err(ApiError::from)?;

    // Get the first job details
    let first_job = pipeline_manager
        .get_job(&result.first_job_id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::internal("Failed to retrieve created job"))?;

    let response = CreatePipelineResponse {
        pipeline_id: result.pipeline_id,
        first_job: job_to_response(&first_job),
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
fn job_to_response(job: &Job) -> JobResponse {
    JobResponse {
        id: job.id.clone(),
        session_id: job.session_id.clone(),
        streamer_id: job.streamer_id.clone(),
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

    Ok(Json(PipelinePresetListResponse {
        presets,
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
) -> ApiResult<Json<crate::database::models::PipelinePreset>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    let preset = preset_repo
        .get_pipeline_preset(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Pipeline preset {} not found", id)))?;

    Ok(Json(preset))
}

/// Create a new pipeline preset.
///
/// # Endpoint
///
/// `POST /api/pipeline/presets`
async fn create_pipeline_preset(
    State(state): State<AppState>,
    Json(payload): Json<CreatePipelinePresetRequest>,
) -> ApiResult<Json<crate::database::models::PipelinePreset>> {
    let preset_repo = state
        .pipeline_preset_repository
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Pipeline preset service not available"))?;

    let steps_json = serde_json::to_string(&payload.steps)
        .map_err(|e| ApiError::bad_request(format!("Invalid steps: {}", e)))?;

    let preset = crate::database::models::PipelinePreset {
        id: uuid::Uuid::new_v4().to_string(),
        name: payload.name,
        description: payload.description,
        steps: steps_json,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    preset_repo
        .create_pipeline_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(preset))
}

/// Update an existing pipeline preset.
///
/// # Endpoint
///
/// `PUT /api/pipeline/presets/{id}`
async fn update_pipeline_preset(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePipelinePresetRequest>,
) -> ApiResult<Json<crate::database::models::PipelinePreset>> {
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

    let steps_json = serde_json::to_string(&payload.steps)
        .map_err(|e| ApiError::bad_request(format!("Invalid steps: {}", e)))?;

    let preset = crate::database::models::PipelinePreset {
        id: id.clone(),
        name: payload.name,
        description: payload.description,
        steps: steps_json,
        created_at: existing.created_at,
        updated_at: chrono::Utc::now(),
    };

    preset_repo
        .update_pipeline_preset(&preset)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(preset))
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
            "input_path": "/recordings/stream.flv"
        }"#;

        let request: CreatePipelineRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.session_id, "session-123");
        assert_eq!(request.streamer_id, "streamer-456");
        assert_eq!(request.input_path, "/recordings/stream.flv");
        assert!(request.steps.is_none());
    }

    #[test]
    fn test_create_pipeline_request_with_steps() {
        let json = r#"{
            "session_id": "session-123",
            "streamer_id": "streamer-456",
            "input_path": "/recordings/stream.flv",
            "steps": ["remux", "upload"]
        }"#;

        let request: CreatePipelineRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            request.steps,
            Some(vec![
                crate::database::models::job::PipelineStep::Preset("remux".to_string()),
                crate::database::models::job::PipelineStep::Preset("upload".to_string())
            ])
        );
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
}
