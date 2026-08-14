//! Streamer and filter MCP tools.
//!
//! Thin wrappers over the `api::routes::{streamers,filters}` handlers.

use axum::Json;
use axum::extract::{FromRef, Path, Query, State};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};

use super::config_tools::{IdParams, PageParams};
use super::{SrecMcpServer, invalid_json_param, tool_json};
use crate::api::models::{
    CreateFilterRequest, CreateStreamerRequest, StreamerFilterParams, UpdateFilterRequest,
    UpdateStreamerRequest,
};
use crate::api::routes::filters::{self, FilterRouteState};
use crate::api::routes::streamers::{self, CheckHistoryParams, StreamerRouteState};

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct StreamerListParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
    /// Filter by platform name (e.g. "bilibili", "douyu", "twitch")
    pub platform: Option<String>,
    /// Filter by state (comma-separated: NotLive, Live, InspectingLive, OutOfSchedule, Cancelled, FatalError, Unknown)
    pub state: Option<String>,
    /// Filter by template ID
    pub template: Option<String>,
    /// Filter by enabled status
    pub enabled: Option<bool>,
    /// Search query (matches name or URL)
    pub search: Option<String>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct StreamerCreateParams {
    /// Display name for the streamer
    pub name: String,
    /// Live room URL; the platform is derived from it
    pub url: String,
    /// Optional template ID to attach
    pub template_id: Option<String>,
    /// Whether recording is enabled (default true)
    pub enabled: Option<bool>,
    /// Optional streamer-specific configuration override object (same shape
    /// as the streamer_specific_config field returned by streamer_get)
    pub streamer_specific_config: Option<serde_json::Value>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct StreamerUpdateParams {
    /// Streamer ID
    pub id: String,
    /// Partial update object. Same shape as `PUT /api/streamers/{id}`
    /// (UpdateStreamerRequest in the OpenAPI spec): optional name, url,
    /// template_id, priority, enabled, streamer_specific_config, ...
    pub updates: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CheckHistoryToolParams {
    /// Streamer ID
    pub id: String,
    /// Maximum rows to return (default 60)
    pub limit: Option<i64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct StreamerIdParams {
    /// Streamer ID
    pub streamer_id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct FilterCreateParams {
    /// Streamer ID the filter belongs to
    pub streamer_id: String,
    /// Filter type (e.g. "title_include", "title_exclude", "time_range")
    pub filter_type: String,
    /// Filter configuration object (shape depends on filter_type; see
    /// existing filters via filter_list)
    pub config: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct FilterUpdateParams {
    /// Streamer ID the filter belongs to
    pub streamer_id: String,
    /// Filter ID
    pub filter_id: String,
    /// New filter type (optional)
    pub filter_type: Option<String>,
    /// New filter configuration object (optional)
    pub config: Option<serde_json::Value>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct FilterIdParams {
    /// Streamer ID the filter belongs to
    pub streamer_id: String,
    /// Filter ID
    pub filter_id: String,
}

#[tool_router(router = streamer_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "streamer_list",
        description = "List monitored streamers with their state (live/offline), platform, priority, and configuration references (paginated, filterable). Requires a full-access API key because streamer overrides can contain credentials."
    )]
    pub async fn streamer_list(
        &self,
        Parameters(params): Parameters<StreamerListParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        let pagination = PageParams {
            limit: params.limit,
            offset: params.offset,
        }
        .to_pagination();
        let filters = StreamerFilterParams {
            platform: params.platform,
            template: params.template,
            template_unassigned: None,
            state: params.state,
            priority: None,
            enabled: params.enabled,
            sort_by: None,
            sort_dir: None,
            search: params.search,
        };
        tool_json(streamers::list_streamers(State(state), Query(pagination), Query(filters)).await)
    }

    #[tool(
        name = "streamer_get",
        description = "Get one streamer by ID, including its resolved configuration references and current state. Requires a full-access API key because streamer overrides can contain credentials."
    )]
    pub async fn streamer_get(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(streamers::get_streamer(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "streamer_create",
        description = "Add a streamer to monitor/record by live room URL. Requires a full-access API key."
    )]
    pub async fn streamer_create(
        &self,
        Parameters(params): Parameters<StreamerCreateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request = CreateStreamerRequest {
            name: params.name,
            url: params.url,
            template_id: params.template_id,
            priority: Default::default(),
            enabled: params.enabled.unwrap_or(true),
            streamer_specific_config: params.streamer_specific_config,
        };
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(streamers::create_streamer(State(state), Json(request)).await)
    }

    #[tool(
        name = "streamer_update",
        description = "Update a streamer (name, url, enabled, template assignment, streamer-specific config override, ...). Requires a full-access API key."
    )]
    pub async fn streamer_update(
        &self,
        Parameters(params): Parameters<StreamerUpdateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request: UpdateStreamerRequest = serde_json::from_value(params.updates)
            .map_err(|error| invalid_json_param("updates", error))?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(streamers::update_streamer(State(state), Path(params.id), Json(request)).await)
    }

    #[tool(
        name = "streamer_delete",
        description = "Delete a streamer and stop monitoring it. Requires a full-access API key."
    )]
    pub async fn streamer_delete(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(streamers::delete_streamer(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "streamer_clear_error",
        description = "Clear a streamer's fatal-error state so monitoring resumes. Requires a full-access API key."
    )]
    pub async fn streamer_clear_error(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(streamers::clear_error(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "streamer_check_history",
        description = "Get the recent live-status poll history for a streamer (per-check outcome and latency). Requires a full-access API key."
    )]
    pub async fn streamer_check_history(
        &self,
        Parameters(params): Parameters<CheckHistoryToolParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = StreamerRouteState::from_ref(&self.app_state);
        tool_json(
            streamers::get_check_history(
                State(state),
                Path(params.id),
                Query(CheckHistoryParams {
                    limit: params.limit,
                }),
            )
            .await,
        )
    }

    #[tool(
        name = "filter_list",
        description = "List recording filters attached to a streamer (title/time filters controlling when recording triggers). Requires a full-access API key."
    )]
    pub async fn filter_list(
        &self,
        Parameters(params): Parameters<StreamerIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = FilterRouteState::from_ref(&self.app_state);
        tool_json(filters::list_filters(State(state), Path(params.streamer_id)).await)
    }

    #[tool(
        name = "filter_create",
        description = "Create a recording filter for a streamer. Requires a full-access API key."
    )]
    pub async fn filter_create(
        &self,
        Parameters(params): Parameters<FilterCreateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request = CreateFilterRequest {
            streamer_id: params.streamer_id.clone(),
            filter_type: params.filter_type,
            config: params.config,
        };
        let state = FilterRouteState::from_ref(&self.app_state);
        tool_json(
            filters::create_filter(State(state), Path(params.streamer_id), Json(request)).await,
        )
    }

    #[tool(
        name = "filter_update",
        description = "Update a recording filter. Requires a full-access API key."
    )]
    pub async fn filter_update(
        &self,
        Parameters(params): Parameters<FilterUpdateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request = UpdateFilterRequest {
            filter_type: params.filter_type,
            config: params.config,
        };
        let state = FilterRouteState::from_ref(&self.app_state);
        tool_json(
            filters::update_filter(
                State(state),
                Path((params.streamer_id, params.filter_id)),
                Json(request),
            )
            .await,
        )
    }

    #[tool(
        name = "filter_delete",
        description = "Delete a recording filter. Requires a full-access API key."
    )]
    pub async fn filter_delete(
        &self,
        Parameters(params): Parameters<FilterIdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = FilterRouteState::from_ref(&self.app_state);
        tool_json(
            filters::delete_filter(State(state), Path((params.streamer_id, params.filter_id)))
                .await,
        )
    }
}
