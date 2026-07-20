use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
};

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::models::JobStatus;
use crate::database::models::job::{DagPipelineDefinition, PipelineStep};

use super::{
    DagCancelResponse, DagFilterParams, DagGraphEdge, DagGraphNode, DagGraphResponse, DagListItem,
    DagListResponse, DagPaginationParams, DagRetryResponse, DagStatsResponse, DagStatusResponse,
    DagStepStatusResponse, ValidateDagRequest, ValidateDagResponse,
};

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
    State(state): State<AppState>,
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
    State(state): State<AppState>,
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
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<DagRetryResponse>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Get DAG execution
    let dag = dag_scheduler
        .get_dag_status(&dag_id)
        .await
        .map_err(ApiError::from)?;

    if dag.status != "FAILED" && dag.status != "CANCELLED" {
        return Err(ApiError::bad_request(
            "DAG is not in FAILED or CANCELLED status",
        ));
    }

    // Get all steps
    let steps = dag_scheduler
        .get_dag_steps(&dag_id)
        .await
        .map_err(ApiError::from)?;

    // Find retryable steps (failed steps + cancelled steps with an existing job).
    // Cancelled steps with a job_id typically represent fail-fast cancelled in-flight work.
    let retryable_steps: Vec<_> = steps
        .iter()
        .filter(|s| matches!(s.status.as_str(), "FAILED" | "CANCELLED") && s.job_id.is_some())
        .collect();

    if retryable_steps.is_empty() {
        return Err(ApiError::bad_request(
            "No failed or cancelled steps found to retry",
        ));
    }

    // Prepare DAG for retry so downstream steps can be scheduled again.
    dag_scheduler
        .reset_dag_for_retry(&dag_id)
        .await
        .map_err(ApiError::from)?;

    let mut job_ids = Vec::new();
    let mut reconciled_steps = 0usize;
    for step in &retryable_steps {
        let Some(job_id) = &step.job_id else {
            continue;
        };

        let job = match pipeline_manager.get_job(job_id).await {
            Ok(Some(job)) => job,
            Ok(None) => {
                tracing::warn!("Failed to retry job {}: job not found", job_id);
                continue;
            }
            Err(e) => {
                tracing::warn!("Failed to load job {} for DAG retry: {}", job_id, e);
                continue;
            }
        };

        match job.status {
            JobStatus::Failed | JobStatus::Cancelled => {
                match pipeline_manager.retry_job(job_id).await {
                    Ok(job) => job_ids.push(job.id),
                    Err(e) => tracing::warn!("Failed to retry job {}: {}", job_id, e),
                }
            }
            JobStatus::Completed => {
                if let Err(e) = dag_scheduler
                    .on_job_completed(
                        &step.id,
                        &job.outputs,
                        job.streamer_name.as_deref(),
                        job.session_title.as_deref(),
                        job.platform.as_deref(),
                        job.session_start,
                    )
                    .await
                {
                    tracing::warn!(
                        "Failed to reconcile completed job {} for DAG step {}: {}",
                        job_id,
                        step.id,
                        e
                    );
                } else {
                    reconciled_steps += 1;
                }
            }
            _ => {
                tracing::debug!(
                    "Skipping DAG retry for job {} in status {:?}",
                    job_id,
                    job.status
                );
            }
        }
    }

    let retried_steps = job_ids.len();
    let message = if retried_steps == retryable_steps.len() {
        format!("Successfully retried {} steps", retried_steps)
    } else {
        format!(
            "Retried {} of {} steps (reconciled {} already-completed steps)",
            retried_steps,
            retryable_steps.len(),
            reconciled_steps
        )
    };

    Ok(Json(DagRetryResponse {
        dag_id,
        retried_steps,
        job_ids,
        message,
    }))
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
    State(state): State<AppState>,
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

    // Batch-fetch streamer names
    let streamer_ids: std::collections::HashSet<String> =
        dags.iter().filter_map(|d| d.streamer_id.clone()).collect();
    let repo = state.streamer_repository.clone();
    let fetches = streamer_ids.into_iter().map(|streamer_id| {
        let repo = repo.clone();
        async move {
            let name = repo.get_streamer(&streamer_id).await.ok().map(|s| s.name);
            (streamer_id, name)
        }
    });
    let streamer_names: std::collections::HashMap<String, String> =
        futures::future::join_all(fetches)
            .await
            .into_iter()
            .filter_map(|(id, name)| name.map(|n| (id, n)))
            .collect();

    // Convert to response format
    let dag_items: Vec<DagListItem> = dags
        .into_iter()
        .map(|dag| {
            let progress_percent = dag.progress_percent();

            // Parse DAG definition to get the name
            let name = dag
                .get_dag_definition()
                .map(|def| def.name)
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
    State(state): State<AppState>,
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
        let steps = dag_scheduler
            .get_dag_steps(&dag.id)
            .await
            .map_err(ApiError::from)?;

        // Find retryable steps (failed + cancelled with a job).
        let retryable_steps: Vec<_> = steps
            .iter()
            .filter(|s| matches!(s.status.as_str(), "FAILED" | "CANCELLED") && s.job_id.is_some())
            .collect();

        if retryable_steps.is_empty() {
            if dag.status == "FAILED" || dag.status == "CANCELLED" {
                let _ = dag_scheduler.reset_dag_for_retry(&dag.id).await;
                retried_count += 1;
            }
            continue;
        }

        // Prepare DAG for retry
        if let Err(e) = dag_scheduler.reset_dag_for_retry(&dag.id).await {
            tracing::warn!("Failed to reset DAG {} for retry: {}", dag.id, e);
            continue;
        }

        for step in retryable_steps {
            if let (Some(job_id), Ok(Some(job))) = (
                &step.job_id,
                pipeline_manager
                    .get_job(step.job_id.as_ref().unwrap())
                    .await,
            ) {
                match job.status {
                    JobStatus::Failed | JobStatus::Cancelled => {
                        let _ = pipeline_manager.retry_job(job_id).await;
                    }
                    JobStatus::Completed => {
                        let _ = dag_scheduler
                            .on_job_completed(
                                &step.id,
                                &job.outputs,
                                job.streamer_name.as_deref(),
                                job.session_title.as_deref(),
                                job.platform.as_deref(),
                                job.session_start,
                            )
                            .await;
                    }
                    _ => {}
                }
            }
        }
        retried_count += 1;
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "count": retried_count,
        "message": format!("Successfully retried {} failed or cancelled DAGs", retried_count)
    })))
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
    State(state): State<AppState>,
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
    State(state): State<AppState>,
    Path(dag_id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let pipeline_manager = &state.pipeline_manager;

    let dag_scheduler = pipeline_manager
        .dag_scheduler()
        .ok_or_else(|| ApiError::service_unavailable("DAG scheduler not available"))?;

    // Delete the DAG
    dag_scheduler
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
    State(state): State<AppState>,
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
