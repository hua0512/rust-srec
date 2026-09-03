//! Pipeline (post-processing) MCP tools: jobs, DAGs, pipeline presets, and
//! job presets.
//!
//! Thin wrappers over the `api::routes::pipeline` and `api::routes::job`
//! handlers.

use axum::extract::{FromRef, Path, Query, State};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};

use super::config_tools::{IdParams, PageParams};
use super::{SrecMcpServer, tool_json};
use crate::api::models::JobFilterParams;
use crate::api::routes::job::{
    self, JobPresetRouteState, PresetFilterParams, PresetPaginationParams,
};
use crate::api::routes::pipeline::{
    DagFilterParams, DagPaginationParams, PipelinePresetFilterParams,
    PipelinePresetPaginationParams, PipelineRouteState, PresetRouteState, dag, jobs, presets,
};

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct JobListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Filter by status (PENDING, QUEUED, PROCESSING, COMPLETED, FAILED, CANCELLED, RETRYING)
    pub status: Option<String>,
    /// Filter by streamer ID
    pub streamer_id: Option<String>,
    /// Filter by session ID
    pub session_id: Option<String>,
    /// Filter by pipeline ID
    pub pipeline_id: Option<String>,
    /// Search query (matches job/streamer/session IDs)
    pub search: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct JobLogsParams {
    /// Job ID
    pub id: String,
    /// Maximum number of log entries to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of entries to skip (default 0)
    pub offset: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DagIdParams {
    /// DAG ID
    pub dag_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct DagListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Filter by DAG status (PENDING, PROCESSING, COMPLETED, FAILED, CANCELLED)
    pub status: Option<String>,
    /// Filter by session ID
    pub session_id: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PresetListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Search query (matches name or description)
    pub search: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct JobPresetListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Filter by category
    pub category: Option<String>,
    /// Filter by processor type
    pub processor: Option<String>,
    /// Exact preset name; returns at most one preset
    pub name: Option<String>,
    /// Search query (substring of name or description)
    pub search: Option<String>,
}

#[tool_router(router = pipeline_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "pipeline_stats",
        description = "Get post-processing pipeline statistics (job counts by status, queue depths)."
    )]
    pub async fn pipeline_stats(&self) -> Result<CallToolResult, ErrorData> {
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(jobs::get_stats(State(state)).await)
    }

    #[tool(
        name = "pipeline_list_jobs",
        description = "List post-processing jobs (paginated, filterable by status/streamer/session/pipeline). Requires a full-access API key because job records include processor configuration."
    )]
    pub async fn pipeline_list_jobs(
        &self,
        Parameters(params): Parameters<JobListParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        let pagination = PageParams {
            limit: params.limit,
            offset: params.offset,
        }
        .to_pagination();
        let status = match params.status.as_deref() {
            None => None,
            Some(raw) => match serde_json::from_value(serde_json::Value::String(raw.to_string())) {
                Ok(status) => Some(status),
                Err(error) => {
                    return Err(ErrorData::invalid_params(
                        format!("Invalid 'status' value '{raw}': {error}"),
                        None,
                    ));
                }
            },
        };
        let filters = JobFilterParams {
            status,
            streamer_id: params.streamer_id,
            session_id: params.session_id,
            pipeline_id: params.pipeline_id,
            from_date: None,
            to_date: None,
            search: params.search,
        };
        tool_json(jobs::list_jobs(State(state), Query(pagination), Query(filters)).await)
    }

    #[tool(
        name = "pipeline_get_job",
        description = "Get one post-processing job by ID. Requires a full-access API key because job records include processor configuration."
    )]
    pub async fn pipeline_get_job(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(jobs::get_job(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "pipeline_job_logs",
        description = "Get the log entries recorded for a post-processing job. Requires a full-access API key."
    )]
    pub async fn pipeline_job_logs(
        &self,
        Parameters(params): Parameters<JobLogsParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        let pagination = PageParams {
            limit: params.limit,
            offset: params.offset,
        }
        .to_pagination();
        tool_json(jobs::list_job_logs(State(state), Path(params.id), Query(pagination)).await)
    }

    #[tool(
        name = "pipeline_retry_job",
        description = "Retry a failed post-processing job. Requires a full-access API key."
    )]
    pub async fn pipeline_retry_job(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(jobs::retry_job(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "pipeline_cancel_job",
        description = "Cancel a pending or running post-processing job. Requires a full-access API key."
    )]
    pub async fn pipeline_cancel_job(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(jobs::cancel_job(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "pipeline_delete_job",
        description = "Delete a terminal (completed/failed/cancelled) post-processing job record. Requires a full-access API key."
    )]
    pub async fn pipeline_delete_job(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(jobs::delete_job(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "pipeline_list_dags",
        description = "List post-processing DAGs (multi-step pipelines) with status summaries. Requires a full-access API key because DAG records include processor configuration."
    )]
    pub async fn pipeline_list_dags(
        &self,
        Parameters(params): Parameters<DagListParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        let filters = DagFilterParams {
            status: params.status,
            session_id: params.session_id,
        };
        let pagination = DagPaginationParams {
            limit: params.limit.unwrap_or(20),
            offset: params.offset.unwrap_or(0),
        };
        tool_json(dag::list_dags(State(state), Query(filters), Query(pagination)).await)
    }

    #[tool(
        name = "pipeline_dag_status",
        description = "Get the status of one DAG by ID. Requires a full-access API key because DAG records include processor configuration."
    )]
    pub async fn pipeline_dag_status(
        &self,
        Parameters(params): Parameters<DagIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(dag::get_dag_status(State(state), Path(params.dag_id)).await)
    }

    #[tool(
        name = "pipeline_dag_graph",
        description = "Get the node/edge graph of one DAG (steps, dependencies, per-node status). Requires a full-access API key because nodes include processor configuration."
    )]
    pub async fn pipeline_dag_graph(
        &self,
        Parameters(params): Parameters<DagIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(dag::get_dag_graph(State(state), Path(params.dag_id)).await)
    }

    #[tool(
        name = "pipeline_retry_dag",
        description = "Retry the failed portion of a DAG. Requires a full-access API key."
    )]
    pub async fn pipeline_retry_dag(
        &self,
        Parameters(params): Parameters<DagIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(dag::retry_dag(State(state), Path(params.dag_id)).await)
    }

    #[tool(
        name = "pipeline_cancel_dag",
        description = "Cancel a running DAG and its jobs. Requires a full-access API key."
    )]
    pub async fn pipeline_cancel_dag(
        &self,
        Parameters(params): Parameters<DagIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = PipelineRouteState::from_ref(&self.app_state);
        tool_json(dag::cancel_dag(State(state), Path(params.dag_id)).await)
    }

    #[tool(
        name = "pipeline_list_presets",
        description = "List pipeline presets (reusable multi-step post-processing workflow definitions). Requires a full-access API key."
    )]
    pub async fn pipeline_list_presets(
        &self,
        Parameters(params): Parameters<PresetListParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PresetRouteState::from_ref(&self.app_state);
        let filters = PipelinePresetFilterParams {
            search: params.search,
        };
        let pagination = PipelinePresetPaginationParams {
            limit: params.limit.unwrap_or(20),
            offset: params.offset.unwrap_or(0),
        };
        tool_json(
            presets::list_pipeline_presets(State(state), Query(filters), Query(pagination)).await,
        )
    }

    #[tool(
        name = "pipeline_get_preset",
        description = "Get one pipeline preset by ID. Requires a full-access API key."
    )]
    pub async fn pipeline_get_preset(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = PresetRouteState::from_ref(&self.app_state);
        tool_json(presets::get_pipeline_preset_by_id(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "job_preset_list",
        description = "List job presets (reusable single-processor configurations referenced by pipelines). Requires a full-access API key."
    )]
    pub async fn job_preset_list(
        &self,
        Parameters(params): Parameters<JobPresetListParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = JobPresetRouteState::from_ref(&self.app_state);
        let filters = PresetFilterParams {
            category: params.category,
            processor: params.processor,
            name: params.name,
            search: params.search,
        };
        let pagination = PresetPaginationParams {
            limit: params.limit.unwrap_or(20),
            offset: params.offset.unwrap_or(0),
        };
        tool_json(job::list_presets(State(state), Query(filters), Query(pagination)).await)
    }

    #[tool(
        name = "job_preset_get",
        description = "Get one job preset by ID. Requires a full-access API key."
    )]
    pub async fn job_preset_get(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = JobPresetRouteState::from_ref(&self.app_state);
        tool_json(job::get_preset(State(state), Path(params.id)).await)
    }
}
