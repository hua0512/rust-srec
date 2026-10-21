use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
};

use crate::api::error::{ApiError, ApiResult};
use crate::database::models::dag_progress_percent;
use crate::database::models::job::{
    DagExecutionStatus, DagPipelineDefinition, DagStepStatus, JobStatus, PipelineStep,
};
use crate::database::repositories::DagListFilter;
use crate::pipeline::DagScheduler;

use super::jobs::job_status_to_api_status;
use super::{
    BatchDagAction, BatchDagItemResult, BatchDagRequest, BatchDagResponse, DagCancelResponse,
    DagFilterParams, DagGraphEdge, DagGraphNode, DagGraphResponse, DagListItem, DagListResponse,
    DagListStatsResponse, DagPaginationParams, DagRetryResponse, DagStatsResponse,
    DagStatusResponse, DagStepStatusResponse, PipelineRouteState, ValidateDagRequest,
    ValidateDagResponse,
};

/// Maximum number of DAG IDs accepted by `POST /api/pipeline/dags/batch`.
pub(super) const MAX_DAG_BATCH_SIZE: usize = 100;

/// Reject a batch whose IDs are missing, oversized, blank or repeated.
///
/// `entity` names the ID kind in the error message so [`batch_dags`] and the media
/// output batch in [`super::jobs`] can share one implementation.
pub(super) fn validate_batch_ids(ids: &[String], entity: &str) -> ApiResult<()> {
    if ids.is_empty() {
        return Err(ApiError::validation(format!(
            "At least one {entity} ID is required"
        )));
    }
    if ids.len() > MAX_DAG_BATCH_SIZE {
        return Err(ApiError::validation(format!(
            "A batch may contain at most {MAX_DAG_BATCH_SIZE} {entity} IDs"
        )));
    }
    if ids.iter().any(|id| id.trim().is_empty()) {
        return Err(ApiError::validation(format!(
            "{entity} IDs cannot be empty"
        )));
    }

    let unique_ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if unique_ids.len() != ids.len() {
        return Err(ApiError::validation(format!(
            "{entity} IDs must be unique within a batch"
        )));
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/pipeline/dag/{dag_id}",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG status with all steps", body = DagStatusResponse),
        (status = 404, description = "DAG not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_dag_status(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagStatusResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // The same status the DAG list shows, so opening a listed DAG agrees with
    // it. One read: a DAG finishing between two reads would otherwise show as
    // finished with the step counts from before.
    let (dag, display_status) = dag_scheduler
        .get_dag_with_display_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let steps = dag_scheduler
        .get_dag_steps_with_jobs(&dag_id)
        .await
        .map_err(ApiError::from)?;
    let dag_status = dag.get_status();

    // Get DAG definition for step processor info
    let dag_def = dag.get_dag_definition();

    // Index definition steps by id. `or_insert` keeps the first entry on a
    // duplicate id, matching a linear scan of `def.steps` with `find`.
    let mut def_step_by_id: HashMap<&str, &PipelineStep> = HashMap::new();
    if let Some(def) = dag_def.as_ref() {
        for def_step in &def.steps {
            def_step_by_id
                .entry(def_step.id.as_str())
                .or_insert(&def_step.step);
        }
    }

    // Build step responses
    let step_responses: Vec<DagStepStatusResponse> = steps
        .iter()
        .map(|row| {
            let step = &row.step;
            let processor =
                def_step_by_id
                    .get(step.step_id.as_str())
                    .map(|def_step| match def_step {
                        PipelineStep::Preset { name } => name.clone(),
                        PipelineStep::Workflow { name } => format!("workflow:{}", name),
                        PipelineStep::Inline { processor, .. } => processor.clone(),
                    });
            let job_status = row.job_status.as_deref().and_then(JobStatus::parse);
            let job_id = step.job_id.as_deref();
            let retry_after = row
                .job_retry_after
                .filter(|_| job_id.is_some_and(|job_id| step.awaits_job(job_id, dag_status)));
            let running = step.get_status() == Some(DagStepStatus::Processing)
                && job_status == Some(JobStatus::Processing);
            let progress = job_id
                .filter(|_| running)
                .and_then(|job_id| dag_scheduler.cached_job_progress(job_id));

            DagStepStatusResponse {
                step_id: step.step_id.clone(),
                status: step.status.clone(),
                job_id: step.job_id.clone(),
                job_status: job_status.map(job_status_to_api_status),
                started_at: row.job_started_at,
                retry_after,
                progress,
                depends_on: step.get_depends_on(),
                outputs: step.get_outputs(),
                processor,
            }
        })
        .collect();

    let name = dag_def
        .map(|d| d.name)
        .unwrap_or_else(|| "Unknown".to_string());
    let completed_steps = steps
        .iter()
        .filter(|row| row.step.get_status() == Some(DagStepStatus::Completed))
        .count();
    let progress_percent = dag_progress_percent(
        steps.len() as i32,
        completed_steps as i32,
        step_responses
            .iter()
            .filter_map(|step| step.progress.as_ref()?.percent),
    );

    Ok(Json(DagStatusResponse {
        id: dag.id,
        name,
        status: display_status.as_str().to_string(),
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

#[utoipa::path(
    get,
    path = "/api/pipeline/dag/{dag_id}/graph",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG graph visualization data", body = DagGraphResponse),
        (status = 404, description = "DAG not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_dag_graph(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagGraphResponse>> {
    let pipeline_manager = &state.pipeline_manager;

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

    // Index definition steps by id. `or_insert` keeps the first entry on a
    // duplicate id, matching a linear scan of `def.steps` with `find`.
    let mut def_step_by_id: HashMap<&str, &PipelineStep> = HashMap::new();
    if let Some(def) = dag_def.as_ref() {
        for def_step in &def.steps {
            def_step_by_id
                .entry(def_step.id.as_str())
                .or_insert(&def_step.step);
        }
    }

    // Build nodes
    let nodes: Vec<DagGraphNode> = steps
        .iter()
        .map(|step| {
            let processor =
                def_step_by_id
                    .get(step.step_id.as_str())
                    .map(|def_step| match def_step {
                        PipelineStep::Preset { name } | PipelineStep::Workflow { name } => {
                            name.clone()
                        }
                        PipelineStep::Inline { processor, .. } => processor.clone(),
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

#[utoipa::path(
    post,
    path = "/api/pipeline/dag/{dag_id}/retry",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG retry result", body = DagRetryResponse),
        (status = 400, description = "No failed steps", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn retry_dag(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagRetryResponse>> {
    retry_dag_inner(&state, &dag_id).await.map(Json)
}

/// Core of [`retry_dag`], shared with [`batch_dags`].
///
/// Kept separate from the handler so a batch item reports exactly the error the
/// single-DAG endpoint would have produced for the same ID.
async fn retry_dag_inner(state: &PipelineRouteState, dag_id: &str) -> ApiResult<DagRetryResponse> {
    if state.pipeline_manager.dag_scheduler().is_none() {
        return Err(ApiError::service_unavailable("DAG scheduler not available"));
    }
    let result = state
        .pipeline_manager
        .retry_dag(dag_id)
        .await
        .map_err(ApiError::from)?;
    Ok(DagRetryResponse {
        dag_id: result.dag_id,
        retried_steps: result.retried_steps,
        job_ids: result.job_ids,
        message: result.message,
    })
}

/// Reported percents of the running steps of `dag_ids`, keyed by DAG ID, for
/// [`dag_progress_percent`]. A running job that has not reported a percent is
/// left out.
async fn running_step_percents(
    dag_scheduler: &DagScheduler,
    dag_ids: &[&str],
) -> ApiResult<HashMap<String, Vec<f32>>> {
    let running = dag_scheduler
        .list_running_step_jobs(dag_ids)
        .await
        .map_err(ApiError::from)?;
    let mut percents: HashMap<String, Vec<f32>> = HashMap::new();
    for (dag_id, job_id) in running {
        if let Some(percent) = dag_scheduler
            .cached_job_progress(&job_id)
            .and_then(|progress| progress.percent)
        {
            percents.entry(dag_id).or_default().push(percent);
        }
    }
    Ok(percents)
}

#[utoipa::path(
    get,
    path = "/api/pipeline/dags",
    tag = "pipeline",
    params(DagFilterParams, DagPaginationParams),
    responses(
        (status = 200, description = "List of DAG executions", body = DagListResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn list_dags(
    State(state): State<PipelineRouteState>,
    Query(filters): Query<DagFilterParams>,
    Query(pagination): Query<DagPaginationParams>,
) -> ApiResult<Json<DagListResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let effective_limit = pagination.limit.min(100);

    // Filter by display status so the list agrees with `GET /api/pipeline/dags/stats`.
    let status_filter = filters
        .status
        .as_deref()
        .map(|status| {
            DagExecutionStatus::parse(&status.to_uppercase())
                .ok_or_else(|| ApiError::bad_request(format!("Invalid DAG status: {status}")))
        })
        .transpose()?;

    let filter = DagListFilter {
        status: status_filter,
        session_id: filters.session_id.as_deref(),
        search: filters.search.as_deref(),
    };

    let dags = dag_scheduler
        .list_dag_summaries(&filter, effective_limit, pagination.offset)
        .await
        .map_err(ApiError::from)?;

    let total = dag_scheduler
        .count_dags_by_display_status(&filter)
        .await
        .map_err(ApiError::from)?;

    // A DAG displayed as PENDING has no running job, so only PROCESSING ones
    // can have running steps.
    let processing_dag_ids: Vec<&str> = dags
        .iter()
        .filter(|dag| dag.display_status == DagExecutionStatus::Processing)
        .map(|dag| dag.id.as_str())
        .collect();
    let mut running_percents = running_step_percents(dag_scheduler, &processing_dag_ids).await?;

    // One `WHERE id IN (...)` query for every streamer on the page. A failed
    // lookup only blanks the display names.
    let streamer_ids: Vec<String> = dags
        .iter()
        .filter_map(|dag| dag.streamer_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let streamer_names: HashMap<String, String> = match state
        .streamer_repository
        .get_streamers_by_ids(&streamer_ids)
        .await
    {
        Ok(streamers) => streamers.into_iter().map(|s| (s.id, s.name)).collect(),
        Err(error) => {
            tracing::warn!(%error, "failed to resolve streamer names for DAG list");
            HashMap::new()
        }
    };

    // Convert to response format
    let dag_items: Vec<DagListItem> = dags
        .into_iter()
        .map(|dag| {
            // `completed_steps` is incremented in the transaction that
            // completes a step and `total_steps` is the number of step rows
            // published with the DAG, so these match the step rows.
            let progress_percent = dag_progress_percent(
                dag.total_steps,
                dag.completed_steps,
                running_percents.remove(&dag.id).unwrap_or_default(),
            );

            let streamer_name = dag
                .streamer_id
                .as_ref()
                .and_then(|id| streamer_names.get(id).cloned());

            DagListItem {
                id: dag.id,
                name: dag.name.unwrap_or_else(|| "Unknown".to_string()),
                status: dag.display_status.as_str().to_string(),
                streamer_id: dag.streamer_id,
                streamer_name,
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

    Ok(Json(DagListResponse {
        dags: dag_items,
        total,
        limit: effective_limit,
        offset: pagination.offset,
    }))
}

/// Count DAG executions by the status shown in `GET /api/pipeline/dags`.
///
/// A DAG that is not finished counts as `PENDING` until one of its jobs is
/// running on a worker.
#[utoipa::path(
    get,
    path = "/api/pipeline/dags/stats",
    tag = "pipeline",
    responses(
        (status = 200, description = "DAG execution counts by status", body = DagListStatsResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_dag_list_stats(
    State(state): State<PipelineRouteState>,
) -> ApiResult<Json<DagListStatsResponse>> {
    let dag_scheduler = state
        .pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let counts = dag_scheduler
        .get_dag_display_counts()
        .await
        .map_err(ApiError::from)?;

    Ok(Json(DagListStatsResponse {
        pending_count: counts.pending,
        processing_count: counts.processing,
        completed_count: counts.completed,
        failed_count: counts.failed,
        cancelled_count: counts.cancelled,
        avg_duration_secs: counts.avg_duration_secs,
    }))
}

#[utoipa::path(
    post,
    path = "/api/pipeline/dags/retry_failed",
    tag = "pipeline",
    responses(
        (status = 200, description = "Bulk retry result", body = serde_json::Value)
    ),
    security(("bearer_auth" = []))
)]
pub async fn retry_all_failed_dags(
    State(state): State<PipelineRouteState>,
) -> ApiResult<Json<serde_json::Value>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let failed_dags = dag_scheduler
        .list_dags(Some("FAILED"), None, 1000, 0)
        .await
        .map_err(ApiError::from)?;
    let cancelled_dags = dag_scheduler
        .list_dags(Some("CANCELLED"), None, 1000, 0)
        .await
        .map_err(ApiError::from)?;

    let dags: Vec<_> = failed_dags.into_iter().chain(cancelled_dags).collect();

    if dags.is_empty() {
        return Ok(Json(serde_json::json!({
            "success": true,
            "count": 0,
            "message": "No failed or cancelled DAGs found"
        })));
    }

    let mut retried_count = 0;
    for dag in dags {
        match pipeline_manager.retry_dag(&dag.id).await {
            Ok(_) => retried_count += 1,
            Err(error) => tracing::warn!(dag_id = %dag.id, %error, "Failed to retry DAG"),
        }
    }

    let noun = if retried_count == 1 { "DAG" } else { "DAGs" };
    Ok(Json(serde_json::json!({
        "success": true,
        "count": retried_count,
        "message": format!("Successfully retried {retried_count} failed or cancelled {noun}")
    })))
}

/// Apply one action to several DAGs, reporting a result per ID.
///
/// Items are applied sequentially and successful ones are never rolled back, so a
/// partially applied batch is the normal outcome when the selection mixes statuses:
/// [`crate::pipeline::PipelineManager::cancel_dag`] rejects a DAG that is already
/// terminal and [`retry_dag_inner`] rejects one that is not, while
/// [`crate::pipeline::PipelineManager::delete_dag`] accepts any status. Those
/// per-item rejections are reported in `results`, not as a failed request — only ID
/// validation and a missing DAG scheduler produce a non-200.
#[utoipa::path(
    post,
    path = "/api/pipeline/dags/batch",
    tag = "pipeline",
    request_body = BatchDagRequest,
    responses(
        (status = 200, description = "Per-DAG results for the requested action", body = BatchDagResponse),
        (status = 422, description = "Invalid batch request", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "DAG scheduler not available", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn batch_dags(
    State(state): State<PipelineRouteState>,
    Json(request): Json<BatchDagRequest>,
) -> ApiResult<Json<BatchDagResponse>> {
    validate_batch_ids(&request.ids, "DAG")?;

    // Without a scheduler every item would fail identically, so fail the request once
    // instead of returning N copies of the same 503 in `results`.
    state
        .pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let pipeline_manager = &state.pipeline_manager;
    let action = request.action;
    let requested = request.ids.len();
    let mut results = Vec::with_capacity(requested);

    for id in request.ids {
        // Collects `ApiResult` rather than `crate::Result` because `retry_dag_inner`
        // already produces `ApiError`; routing it through `crate::Error` would flatten
        // its bad-request status gates into 500s and make an item's `code` differ from
        // what `retry_dag` returns for the same DAG.
        let result: ApiResult<()> = async {
            match action {
                BatchDagAction::Cancel => {
                    pipeline_manager
                        .cancel_dag(&id)
                        .await
                        .map_err(ApiError::from)?;
                }
                BatchDagAction::Retry => {
                    retry_dag_inner(&state, &id).await?;
                }
                BatchDagAction::Delete => {
                    pipeline_manager
                        .delete_dag(&id)
                        .await
                        .map_err(ApiError::from)?;
                }
            }
            Ok(())
        }
        .await;

        match result {
            Ok(()) => results.push(BatchDagItemResult {
                id,
                success: true,
                code: None,
                error: None,
            }),
            Err(api_error) => {
                tracing::warn!(
                    dag_id = %id,
                    error_code = %api_error.code,
                    "Batch DAG action failed"
                );
                results.push(BatchDagItemResult {
                    id,
                    success: false,
                    code: Some(api_error.code),
                    error: Some(api_error.message),
                });
            }
        }
    }

    let succeeded = results.iter().filter(|result| result.success).count();
    Ok(Json(BatchDagResponse {
        requested,
        succeeded,
        failed: requested - succeeded,
        results,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/pipeline/dag/{dag_id}",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG cancelled", body = DagCancelResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn cancel_dag(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagCancelResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    // Preserve service-unavailable semantics if DAG support isn't configured.
    pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    let cancelled_steps = pipeline_manager
        .cancel_dag(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let message = if cancelled_steps == 0 {
        format!("DAG '{}' cancelled (no active steps to cancel)", dag_id)
    } else {
        let steps = if cancelled_steps == 1 {
            "step"
        } else {
            "steps"
        };
        format!("DAG '{dag_id}' cancelled successfully ({cancelled_steps} {steps} cancelled)")
    };

    Ok(Json(DagCancelResponse {
        dag_id,
        cancelled_steps,
        message,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/pipeline/dag/{dag_id}/delete",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG deleted")
    ),
    security(("bearer_auth" = []))
)]
pub async fn delete_dag(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let pipeline_manager = &state.pipeline_manager;

    // Preserve service-unavailable semantics if DAG support isn't configured.
    pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Goes through the manager so a DAG that is still running is cancelled, and the pipeline
    // coordinator notified, before its rows are removed.
    pipeline_manager
        .delete_dag(&dag_id)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({
        "dag_id": dag_id,
        "message": format!("DAG '{}' deleted successfully", dag_id)
    })))
}

#[utoipa::path(
    get,
    path = "/api/pipeline/dag/{dag_id}/stats",
    tag = "pipeline",
    params(("dag_id" = String, Path, description = "DAG execution ID")),
    responses(
        (status = 200, description = "DAG step statistics", body = DagStatsResponse),
        (status = 404, description = "DAG not found", body = crate::api::error::ApiErrorResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_dag_stats(
    State(state): State<PipelineRouteState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagStatsResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Read the display status first so an unknown DAG is a 404; its step
    // counts alone would all be zero. As in `list_dags`, only a DAG displayed
    // as PROCESSING has a running job, so only its running steps are read.
    let display_status = dag_scheduler
        .get_dag_display_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let stats = dag_scheduler
        .get_dag_stats(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let total = stats.total();
    let running_percents = if display_status == DagExecutionStatus::Processing {
        running_step_percents(dag_scheduler, &[dag_id.as_str()])
            .await?
            .remove(&dag_id)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let progress_percent =
        dag_progress_percent(total as i32, stats.completed as i32, running_percents);

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

#[utoipa::path(
    post,
    path = "/api/pipeline/validate",
    tag = "pipeline",
    request_body = ValidateDagRequest,
    responses(
        (status = 200, description = "DAG validation result", body = ValidateDagResponse)
    ),
    security(("bearer_auth" = []))
)]
pub async fn validate_dag(
    State(state): State<PipelineRouteState>,
    Json(request): Json<ValidateDagRequest>,
) -> ApiResult<Json<ValidateDagResponse>> {
    let dag = &request.dag;
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Maximum allowed steps to prevent DoS
    const MAX_STEPS: usize = 1000;

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

    // Check for too many steps (prevent DoS)
    if dag.steps.len() > MAX_STEPS {
        errors.push(format!(
            "DAG has {} steps, maximum allowed is {}",
            dag.steps.len(),
            MAX_STEPS
        ));
        return Ok(Json(ValidateDagResponse {
            valid: false,
            errors,
            warnings,
            root_steps: vec![],
            leaf_steps: vec![],
            max_depth: 0,
        }));
    }

    let n = dag.steps.len();

    // Build id -> index map with capacity pre-allocation
    let mut id_to_idx: HashMap<&str, usize> = HashMap::with_capacity(n);
    for (i, step) in dag.steps.iter().enumerate() {
        if id_to_idx.insert(&step.id, i).is_some() {
            errors.push(format!("Duplicate step ID: {}", step.id));
        }
    }

    // Pre-allocate vectors for graph representation
    let mut in_degree: Vec<usize> = vec![0; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_dependents = vec![false; n];

    // Single pass: build graph, check missing deps, check self-deps
    for (i, step) in dag.steps.iter().enumerate() {
        for dep in &step.depends_on {
            // Check self-dependency
            if dep == &step.id {
                errors.push(format!("Step '{}' depends on itself", step.id));
                continue;
            }

            // Check missing dependency
            match id_to_idx.get(dep.as_str()) {
                Some(&dep_idx) => {
                    dependents[dep_idx].push(i);
                    in_degree[i] += 1;
                    has_dependents[dep_idx] = true;
                }
                None => {
                    errors.push(format!(
                        "Step '{}' depends on non-existent step '{}'",
                        step.id, dep
                    ));
                }
            }
        }
    }

    // Find root and leaf steps (single pass using pre-computed data)
    let mut root_steps: Vec<String> = Vec::new();
    let mut leaf_steps: Vec<String> = Vec::new();
    for (i, step) in dag.steps.iter().enumerate() {
        if in_degree[i] == 0 {
            root_steps.push(step.id.clone());
        }
        if !has_dependents[i] {
            leaf_steps.push(step.id.clone());
        }
    }

    if root_steps.is_empty() && n > 0 {
        errors.push("DAG has no root steps (all steps have dependencies)".to_string());
    }

    // Cycle detection + depth calculation in single Kahn's algorithm pass
    // This is O(V+E) and cannot infinite loop
    let mut queue: Vec<usize> = Vec::with_capacity(n);
    let mut depths: Vec<usize> = vec![0; n];
    let mut remaining_in_degree = in_degree.clone();

    // Initialize queue with roots
    for i in 0..n {
        if remaining_in_degree[i] == 0 {
            queue.push(i);
            depths[i] = 1;
        }
    }

    let mut processed = 0;
    let mut head = 0;

    // Process queue (using head pointer instead of pop for speed)
    while head < queue.len() {
        let node = queue[head];
        head += 1;
        processed += 1;

        let current_depth = depths[node];

        for &dependent in &dependents[node] {
            // Update max depth for this dependent
            let new_depth = current_depth + 1;
            if new_depth > depths[dependent] {
                depths[dependent] = new_depth;
            }

            // Decrease in-degree
            remaining_in_degree[dependent] -= 1;
            if remaining_in_degree[dependent] == 0 {
                queue.push(dependent);
            }
        }
    }

    // If we didn't process all nodes, there's a cycle
    if processed < n {
        // Find cycle for error message (nodes with remaining in-degree > 0)
        let cycle_nodes: Vec<String> = (0..n)
            .filter(|&i| remaining_in_degree[i] > 0)
            .take(5) // Limit to first 5 to avoid huge error messages
            .map(|i| dag.steps[i].id.clone())
            .collect();
        errors.push(format!(
            "Cycle detected involving: {}{}",
            cycle_nodes.join(" -> "),
            if cycle_nodes.len() == 5 { " ..." } else { "" }
        ));
    }

    let max_depth = depths.iter().copied().max().unwrap_or(0);

    // Add warnings
    if n == 1 {
        warnings.push("DAG has only one step - consider if a pipeline is necessary".to_string());
    }

    if max_depth > 10 {
        warnings.push(format!(
            "DAG has depth {} - deep pipelines may be slow",
            max_depth
        ));
    }

    // With the graph itself sound, check what creation would check: presets
    // and workflows resolve, processors exist, and no step removes files a
    // sibling still reads.
    if errors.is_empty() {
        let analysis = state
            .pipeline_manager
            .analyze_dag_definition(dag.clone())
            .await
            .map_err(ApiError::from)?;
        errors.extend(analysis.errors);
        warnings.extend(analysis.warnings);
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

/// Topologically sort DAG steps using Kahn's algorithm with integer indexing.
/// O(V+E) time complexity, guaranteed to terminate.
pub(super) fn topological_sort(dag: &DagPipelineDefinition) -> Vec<String> {
    if dag.steps.is_empty() {
        return Vec::new();
    }

    let n = dag.steps.len();

    // Build id -> index map
    let id_to_idx: HashMap<&str, usize> = dag
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();

    // Build graph
    let mut in_degree: Vec<usize> = vec![0; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, step) in dag.steps.iter().enumerate() {
        for dep in &step.depends_on {
            if let Some(&dep_idx) = id_to_idx.get(dep.as_str()) {
                dependents[dep_idx].push(i);
                in_degree[i] += 1;
            }
        }
    }

    // Kahn's algorithm
    let mut result: Vec<String> = Vec::with_capacity(n);
    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut head = 0;

    while head < queue.len() {
        let node = queue[head];
        head += 1;
        result.push(dag.steps[node].id.clone());

        for &dependent in &dependents[node] {
            in_degree[dependent] -= 1;
            if in_degree[dependent] == 0 {
                queue.push(dependent);
            }
        }
    }

    result
}
