//! Pipeline management routes.

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get, post},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::models::{
    JobFilterParams, JobResponse, MediaOutputResponse, PaginatedResponse, PaginationParams,
    PipelineStatsResponse,
};
use crate::api::server::AppState;

/// Create the pipeline router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/:id", get(get_job))
        .route("/jobs/:id/retry", post(retry_job))
        .route("/jobs/:id", delete(cancel_job))
        .route("/outputs", get(list_outputs))
        .route("/stats", get(get_stats))
}

/// List pipeline jobs with pagination and filtering.
async fn list_jobs(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
    Query(filters): Query<JobFilterParams>,
) -> ApiResult<Json<PaginatedResponse<JobResponse>>> {
    // TODO: Implement actual listing logic using PipelineManager
    let response = PaginatedResponse::new(Vec::new(), 0, pagination.limit, pagination.offset);

    Ok(Json(response))
}

/// Get a single job by ID.
async fn get_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // TODO: Implement actual retrieval logic using PipelineManager
    Err(ApiError::not_found(format!(
        "Job with id '{}' not found",
        id
    )))
}

/// Retry a failed job.
async fn retry_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<JobResponse>> {
    // TODO: Implement actual retry logic using PipelineManager
    // Should reset job status to Pending and re-enqueue
    Err(ApiError::not_found(format!(
        "Job with id '{}' not found",
        id
    )))
}

/// Cancel or delete a job.
async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    // TODO: Implement actual cancellation logic using PipelineManager
    // Should cancel if in progress, delete if pending
    Err(ApiError::not_found(format!(
        "Job with id '{}' not found",
        id
    )))
}

/// List media outputs with pagination.
async fn list_outputs(
    State(state): State<AppState>,
    Query(pagination): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<MediaOutputResponse>>> {
    // TODO: Implement actual listing logic using SessionRepository
    let response = PaginatedResponse::new(Vec::new(), 0, pagination.limit, pagination.offset);

    Ok(Json(response))
}

/// Get pipeline statistics.
async fn get_stats(State(state): State<AppState>) -> ApiResult<Json<PipelineStatsResponse>> {
    // TODO: Implement actual statistics retrieval using PipelineManager
    let response = PipelineStatsResponse {
        pending_count: 0,
        processing_count: 0,
        completed_count: 0,
        failed_count: 0,
        avg_processing_time_secs: None,
    };

    Ok(Json(response))
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
}
