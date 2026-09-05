use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, Query, State},
};

use crate::api::error::{ApiError, ApiResult};
use crate::database::models::job::{DagPipelineDefinition, PipelineStep};

use super::{
    BatchDagAction, BatchDagItemResult, BatchDagRequest, BatchDagResponse, DagCancelResponse,
    DagFilterParams, DagGraphEdge, DagGraphNode, DagGraphResponse, DagListItem, DagListResponse,
    DagPaginationParams, DagRetryResponse, DagStatsResponse, DagStatusResponse,
    DagStepStatusResponse, PipelineRouteState, ValidateDagRequest, ValidateDagResponse,
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
        .map(|step| {
            let processor =
                def_step_by_id
                    .get(step.step_id.as_str())
                    .map(|def_step| match def_step {
                        PipelineStep::Preset { name } => name.clone(),
                        PipelineStep::Workflow { name } => format!("workflow:{}", name),
                        PipelineStep::Inline { processor, .. } => processor.clone(),
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

    // Convert status string to match DAG execution status
    let status_filter = filters
        .status
        .as_ref()
        .map(|s| match s.to_uppercase().as_str() {
            "PENDING" => "PENDING",
            "PROCESSING" => "PROCESSING",
            "COMPLETED" => "COMPLETED",
            "FAILED" => "FAILED",
            "CANCELLED" => "CANCELLED",
            _ => s.as_str(),
        });

    let session_id_filter = filters.session_id.as_deref();

    // List DAG executions from dag_execution table
    let dags = dag_scheduler
        .list_dags(
            status_filter,
            session_id_filter,
            effective_limit,
            pagination.offset,
        )
        .await
        .map_err(ApiError::from)?;

    // Count total matching DAGs
    let total = dag_scheduler
        .count_dags(status_filter, session_id_filter)
        .await
        .map_err(ApiError::from)?;

    // One `WHERE id IN (...)` query for every streamer on the page. A failed
    // lookup only blanks the display names, as before.
    let streamer_ids: Vec<String> = dags
        .iter()
        .filter_map(|d| d.streamer_id.clone())
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
            let progress_percent = dag.progress_percent();

            let name = dag
                .dag_definition_name()
                .unwrap_or_else(|| "Unknown".to_string());

            let streamer_name = dag
                .streamer_id
                .as_ref()
                .and_then(|id| streamer_names.get(id).cloned());

            DagListItem {
                id: dag.id,
                name,
                status: dag.status,
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

    Ok(Json(serde_json::json!({
        "success": true,
        "count": retried_count,
        "message": format!("Successfully retried {} failed or cancelled DAGs", retried_count)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::routes::pipeline::tests::build_test_state;

    #[test]
    fn test_validate_batch_ids() {
        assert!(validate_batch_ids(&["dag-1".to_string()], "DAG").is_ok());
        assert!(validate_batch_ids(&[], "DAG").is_err());
        assert!(validate_batch_ids(&["".to_string()], "DAG").is_err());
        assert!(validate_batch_ids(&["   ".to_string()], "DAG").is_err());
        assert!(validate_batch_ids(&["dag-1".to_string(), "dag-1".to_string()], "DAG").is_err());

        let oversized = (0..=MAX_DAG_BATCH_SIZE)
            .map(|index| format!("dag-{index}"))
            .collect::<Vec<_>>();
        assert!(validate_batch_ids(&oversized, "DAG").is_err());
    }

    #[test]
    fn test_validate_batch_ids_names_the_entity() {
        let error = validate_batch_ids(&[], "output").expect_err("empty batch must be rejected");
        assert!(
            error.message.contains("output"),
            "message should name the entity, got: {}",
            error.message
        );
    }

    /// Pins the `#[serde(tag = "type", rename_all = "snake_case")]` wire format the
    /// frontend's discriminated union mirrors.
    #[test]
    fn test_batch_dag_action_deserialization() {
        for (raw, expected) in [
            ("cancel", BatchDagAction::Cancel),
            ("retry", BatchDagAction::Retry),
            ("delete", BatchDagAction::Delete),
        ] {
            let request: BatchDagRequest = serde_json::from_value(serde_json::json!({
                "ids": ["dag-1"],
                "action": { "type": raw }
            }))
            .expect("batch DAG request should deserialize");
            assert_eq!(request.action, expected);
            assert_eq!(request.ids, vec!["dag-1".to_string()]);
        }

        assert!(
            serde_json::from_value::<BatchDagRequest>(serde_json::json!({
                "ids": ["dag-1"],
                "action": { "type": "purge" }
            }))
            .is_err(),
            "unknown action variants must be rejected"
        );
    }

    /// ID validation runs before the scheduler lookup, so an empty batch is a 422
    /// even when DAG support is unavailable.
    #[tokio::test]
    async fn test_batch_dags_rejects_empty_ids_before_scheduler_check() {
        let state = build_test_state();
        let error = batch_dags(
            State(state),
            Json(BatchDagRequest {
                ids: vec![],
                action: BatchDagAction::Cancel,
            }),
        )
        .await
        .expect_err("empty batch must be rejected");

        assert_eq!(error.status, axum::http::StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "VALIDATION_ERROR");
    }

    /// A missing scheduler fails the whole request once rather than producing one
    /// identical per-item failure per ID.
    #[tokio::test]
    async fn test_batch_dags_requires_dag_scheduler() {
        let state = build_test_state();
        let error = batch_dags(
            State(state),
            Json(BatchDagRequest {
                ids: vec!["dag-1".to_string(), "dag-2".to_string()],
                action: BatchDagAction::Retry,
            }),
        )
        .await
        .expect_err("missing scheduler must fail the whole request");

        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_batch_dag_item_result_omits_empty_error_fields() {
        let json = serde_json::to_value(BatchDagItemResult {
            id: "dag-1".to_string(),
            success: true,
            code: None,
            error: None,
        })
        .unwrap();

        assert_eq!(json["success"], serde_json::Value::Bool(true));
        assert!(json.get("code").is_none());
        assert!(json.get("error").is_none());
    }
}
