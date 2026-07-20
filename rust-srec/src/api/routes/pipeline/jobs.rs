use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
};
use futures::future::join_all;

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    JobExecutionInfo as ApiJobExecutionInfo, JobFilterParams, JobLogEntry as ApiJobLogEntry,
    JobResponse, JobStatus as ApiJobStatus, MediaOutputResponse, PageResponse, PaginatedResponse,
    PaginationParams, PipelineStatsResponse, StepDurationInfo as ApiStepDurationInfo,
};
use crate::api::server::AppState;
use crate::database::models::{JobFilters, JobStatus, OutputFilters, Pagination};
use crate::pipeline::{Job, JobProgressSnapshot};

use super::{CreatePipelineRequest, CreatePipelineResponse, JobRouteState, OutputFilterParams};

#[utoipa::path(
    get,
    path = "/api/pipeline/jobs",
    tag = "pipeline",
    params(PaginationParams, JobFilterParams),
    responses(
        (status = 200, description = "List of jobs", body = PaginatedResponse<JobResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_jobs(
    State(state): State<JobRouteState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PaginatedResponse<JobResponse>>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

    // Convert API filter params to database filter types
    let db_filters = JobFilters {
        status: filters.status.map(api_status_to_job_status),
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

#[utoipa::path(
    get,
    path = "/api/pipeline/jobs/page",
    tag = "pipeline",
    params(PaginationParams, JobFilterParams),
    responses(
        (status = 200, description = "Page of jobs without total count", body = PageResponse<JobResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_jobs_page(
    State(state): State<JobRouteState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PageResponse<JobResponse>>> {
    let pipeline_manager = &state.pipeline_manager;

    let db_filters = JobFilters {
        status: filters.status.map(api_status_to_job_status),
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

#[utoipa::path(
    get,
    path = "/api/pipeline/jobs/{id}/logs",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID"), PaginationParams),
    responses(
        (status = 200, description = "Job execution logs", body = PaginatedResponse<ApiJobLogEntry>),
        (status = 404, description = "Job not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_job_logs(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<ApiJobLogEntry>>> {
    let pipeline_manager = &state.pipeline_manager;

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

#[utoipa::path(
    get,
    path = "/api/pipeline/jobs/{id}/progress",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID")),
    responses(
        (status = 200, description = "Job progress snapshot", body = JobProgressSnapshot),
        (status = 404, description = "Job or progress not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_job_progress(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobProgressSnapshot>> {
    let pipeline_manager = &state.pipeline_manager;

    let snapshot = pipeline_manager
        .get_job_progress(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("No progress available for job {}", id)))?;

    Ok(Json(snapshot))
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
#[utoipa::path(
    get,
    path = "/api/pipeline/jobs/{id}",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID")),
    responses(
        (status = 200, description = "Job details", body = JobResponse),
        (status = 404, description = "Job not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_job(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

    // Call PipelineManager.get_job
    let job = pipeline_manager
        .get_job(&id)
        .await
        .map_err(ApiError::from)?
        .ok_or_else(|| ApiError::not_found(format!("Job with id '{}' not found", id)))?;

    // Fetch streamer name
    let streamer_name = state
        .streamer_repository
        .get_streamer(&job.streamer_id)
        .await
        .ok()
        .map(|s| s.name);

    Ok(Json(job_to_response(&job, streamer_name)))
}

/// Retry a failed or cancelled job.
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
/// - `409 Conflict` - Job is not in a retryable terminal status ("failed" or "cancelled")
///
#[utoipa::path(
    post,
    path = "/api/pipeline/jobs/{id}/retry",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID")),
    responses(
        (status = 200, description = "Job retried", body = JobResponse),
        (status = 409, description = "Job not in failed status", body = crate::api::error::ApiErrorResponse),
        (status = 404, description = "Job not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn retry_job(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

    // Call PipelineManager.retry_job
    let job = pipeline_manager
        .retry_job(&id)
        .await
        .map_err(ApiError::from)?;

    // Fetch streamer name
    let streamer_name = state
        .streamer_repository
        .get_streamer(&job.streamer_id)
        .await
        .ok()
        .map(|s| s.name);

    Ok(Json(job_to_response(&job, streamer_name)))
}

/// Cancel an active job.
///
/// # Endpoint
///
/// `POST /api/pipeline/jobs/{id}/cancel`
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
/// - `400 Bad Request` - Job could not be cancelled due to an invalid state transition
///
/// # Behavior
///
/// - For pending jobs: Removes from queue and marks as "cancelled"
/// - For processing jobs: Signals cancellation to worker and marks as "cancelled"
/// - For cancelled jobs: Keeps the job in the cancelled state and re-signals cancellation if needed
///
/// To delete an entire DAG execution, use `DELETE /api/pipeline/dag/{dag_id}/delete`.
///
#[utoipa::path(
    post,
    path = "/api/pipeline/jobs/{id}/cancel",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID")),
    responses(
        (status = 200, description = "Job cancelled", body = crate::api::openapi::MessageResponse),
        (status = 400, description = "Job could not be cancelled", body = crate::api::error::ApiErrorResponse),
        (status = 404, description = "Job not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn cancel_job(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let pipeline_manager = &state.pipeline_manager;

    pipeline_manager
        .cancel_job(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Job '{}' cancelled successfully", id)
    })))
}

#[utoipa::path(
    delete,
    path = "/api/pipeline/jobs/{id}",
    tag = "pipeline",
    params(("id" = String, Path, description = "Job ID")),
    responses(
        (status = 200, description = "Job deleted", body = crate::api::openapi::MessageResponse),
        (status = 400, description = "Job could not be deleted", body = crate::api::error::ApiErrorResponse),
        (status = 404, description = "Job not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_job(
    State(state): State<JobRouteState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let pipeline_manager = &state.pipeline_manager;

    pipeline_manager
        .delete_job(&id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Job '{}' deleted successfully", id)
    })))
}

#[utoipa::path(
    delete,
    path = "/api/pipeline/{pipeline_id}",
    tag = "pipeline",
    params(("pipeline_id" = String, Path, description = "Pipeline ID")),
    responses(
        (status = 200, description = "Pipeline cancelled")
    ),
    security(("bearer_auth" = []))
)]
pub async fn cancel_pipeline(
    State(state): State<AppState>,
    Path(pipeline_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

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

#[utoipa::path(
    get,
    path = "/api/pipeline/outputs",
    tag = "pipeline",
    params(PaginationParams, OutputFilterParams),
    responses(
        (status = 200, description = "List of media outputs", body = PaginatedResponse<MediaOutputResponse>)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_outputs(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<OutputFilterParams>,
) -> ApiResult<Json<PaginatedResponse<MediaOutputResponse>>> {
    // Get session repository from state
    let session_repository = state.session_repository.clone();

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
            let created_at = crate::database::time::ms_to_datetime(output.created_at);

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
#[utoipa::path(
    get,
    path = "/api/pipeline/stats",
    tag = "pipeline",
    responses(
        (status = 200, description = "Pipeline statistics", body = PipelineStatsResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<PipelineStatsResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

    // Call PipelineManager.get_stats
    let stats = pipeline_manager.get_stats().await.map_err(ApiError::from)?;

    let response = PipelineStatsResponse {
        pending_count: stats.pending,
        processing_count: stats.processing,
        completed_count: stats.completed,
        failed_count: stats.failed,
        cancelled_count: stats.cancelled,
        avg_processing_time_secs: stats.avg_processing_time_secs,
    };

    Ok(Json(response))
}

#[utoipa::path(
    post,
    path = "/api/pipeline/create",
    tag = "pipeline",
    request_body = CreatePipelineRequest,
    responses(
        (status = 201, description = "Pipeline created", body = CreatePipelineResponse),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn create_pipeline(
    State(state): State<AppState>,
    Json(request): Json<CreatePipelineRequest>,
) -> ApiResult<Json<CreatePipelineResponse>> {
    // Get pipeline manager from state
    let pipeline_manager = &state.pipeline_manager;

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
            request.input_paths,
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
    let streamer_name = state
        .streamer_repository
        .get_streamer(&request.streamer_id)
        .await
        .ok()
        .map(|s| s.name);

    let response = CreatePipelineResponse {
        pipeline_id: result.dag_id,
        first_job: job_to_response(&first_job, streamer_name),
    };

    Ok(Json(response))
}

// ============================================================================
// Helper functions
// ============================================================================

/// Convert API JobStatus to persisted job status.
pub(super) fn api_status_to_job_status(status: ApiJobStatus) -> JobStatus {
    match status {
        ApiJobStatus::Pending => JobStatus::Pending,
        ApiJobStatus::Processing => JobStatus::Processing,
        ApiJobStatus::Completed => JobStatus::Completed,
        ApiJobStatus::Failed => JobStatus::Failed,
        ApiJobStatus::Cancelled => JobStatus::Cancelled,
    }
}

/// Convert persisted job status to API JobStatus.
pub(super) fn job_status_to_api_status(status: JobStatus) -> ApiJobStatus {
    match status {
        JobStatus::Pending => ApiJobStatus::Pending,
        JobStatus::Processing => ApiJobStatus::Processing,
        JobStatus::Completed => ApiJobStatus::Completed,
        JobStatus::Failed => ApiJobStatus::Failed,
        JobStatus::Cancelled => ApiJobStatus::Cancelled,
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
        status: job_status_to_api_status(job.status),
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
async fn fetch_streamer_names(state: &JobRouteState, jobs: &[Job]) -> HashMap<String, String> {
    let streamer_repository = state.streamer_repository.clone();

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
