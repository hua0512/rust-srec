//! Configuration, template, and engine MCP tools.
//!
//! Thin wrappers over the `api::routes::{config,templates,engines}` handlers.

use axum::Json;
use axum::extract::{FromRef, Path, Query, State};
use rmcp::{
    ErrorData, RoleServer, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    service::RequestContext, tool, tool_router,
};

use super::{SrecMcpServer, invalid_json_param, tool_json};
use crate::api::models::{
    CreateTemplateRequest, PaginationParams, UpdateGlobalConfigRequest, UpdateTemplateRequest,
};
use crate::api::routes::config::{self, ConfigRouteState};
use crate::api::routes::engines::{self, EngineRouteState, UpdateEngineRequest};
use crate::api::routes::templates::{self, TemplateRouteState};

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct IdParams {
    /// Resource ID
    pub id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PageParams {
    /// Maximum number of items to return (default 20, max 100)
    pub limit: Option<u32>,
    /// Number of items to skip (default 0)
    pub offset: Option<u32>,
}

impl PageParams {
    pub fn to_pagination(&self) -> PaginationParams {
        let default = PaginationParams::default();
        PaginationParams {
            limit: self.limit.unwrap_or(default.limit),
            offset: self.offset.unwrap_or(default.offset),
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateGlobalConfigParams {
    /// Partial update object. Same fields as `PATCH /api/config/global`
    /// (UpdateGlobalConfigRequest in the OpenAPI spec at /api/docs), e.g.
    /// {"record_danmu": true, "max_concurrent_downloads": 4}
    pub updates: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdatePlatformConfigParams {
    /// Platform config ID (see config_list_platforms)
    pub id: String,
    /// Full platform configuration object. Same shape as the response of
    /// config_get_platform / `PUT /api/config/platforms/{id}`. Fetch the
    /// current config first, modify it, and send the whole object back;
    /// omitted optional fields are cleared. The `id` field may be omitted
    /// (it is filled from the `id` parameter).
    pub config: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct CreateTemplateParams {
    /// Template object. Same shape as `POST /api/templates`
    /// (CreateTemplateRequest in the OpenAPI spec): requires "name";
    /// optional overrides like output_folder, record_danmu, cookies,
    /// platform_overrides, engines_override, ...
    pub template: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateTemplateParams {
    /// Template ID
    pub id: String,
    /// Partial update object. Same shape as `PUT /api/templates/{id}`
    /// (UpdateTemplateRequest in the OpenAPI spec).
    pub updates: serde_json::Value,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct UpdateEngineParams {
    /// Engine configuration ID (see engine_list)
    pub id: String,
    /// Fields to update: optional "name" (string), "engine_type"
    /// ("ffmpeg" | "streamlink" | "mesio"), and "config" (engine-specific
    /// JSON object, same shape as returned by engine_get).
    pub updates: serde_json::Value,
}

#[tool_router(router = config_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "config_get_global",
        description = "Get the global recording configuration (output folder/format, concurrency limits, danmu recording, default engine/extractor, pipelines, retention, ...). Requires a full-access API key."
    )]
    pub async fn config_get_global(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = ConfigRouteState::from_ref(&self.app_state);
        tool_json(config::get_global_config(State(state)).await)
    }

    #[tool(
        name = "config_update_global",
        description = "Partially update the global recording configuration. Only the provided fields change. Requires a full-access API key."
    )]
    pub async fn config_update_global(
        &self,
        Parameters(params): Parameters<UpdateGlobalConfigParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request: UpdateGlobalConfigRequest = serde_json::from_value(params.updates)
            .map_err(|error| invalid_json_param("updates", error))?;
        let state = ConfigRouteState::from_ref(&self.app_state);
        tool_json(config::update_global_config(State(state), Json(request)).await)
    }

    #[tool(
        name = "config_list_platforms",
        description = "List all platform-level configurations (per-platform overrides such as cookies, proxy, danmu, engine selection). Requires a full-access API key."
    )]
    pub async fn config_list_platforms(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = ConfigRouteState::from_ref(&self.app_state);
        tool_json(config::list_platform_configs(State(state)).await)
    }

    #[tool(
        name = "config_get_platform",
        description = "Get one platform configuration by ID. Requires a full-access API key."
    )]
    pub async fn config_get_platform(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = ConfigRouteState::from_ref(&self.app_state);
        tool_json(config::get_platform_config(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "config_update_platform",
        description = "Replace a platform configuration (full PUT semantics: fetch with config_get_platform, modify, send the whole object back). Requires a full-access API key."
    )]
    pub async fn config_update_platform(
        &self,
        Parameters(params): Parameters<UpdatePlatformConfigParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let mut config_value = params.config;
        // `replace_platform_config` rejects a body whose id differs from the
        // path id; fill it in so callers only pass the id once.
        if let Some(object) = config_value.as_object_mut() {
            object.insert(
                "id".to_string(),
                serde_json::Value::String(params.id.clone()),
            );
        }
        let request: crate::api::models::PlatformConfigResponse =
            serde_json::from_value(config_value)
                .map_err(|error| invalid_json_param("config", error))?;
        let state = ConfigRouteState::from_ref(&self.app_state);
        tool_json(
            config::replace_platform_config(State(state), Path(params.id), Json(request)).await,
        )
    }

    #[tool(
        name = "template_list",
        description = "List reusable configuration templates (paginated). Requires a full-access API key."
    )]
    pub async fn template_list(
        &self,
        Parameters(params): Parameters<PageParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = TemplateRouteState::from_ref(&self.app_state);
        tool_json(templates::list_templates(State(state), Query(params.to_pagination())).await)
    }

    #[tool(
        name = "template_get",
        description = "Get one template by ID. Requires a full-access API key."
    )]
    pub async fn template_get(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = TemplateRouteState::from_ref(&self.app_state);
        tool_json(templates::get_template(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "template_create",
        description = "Create a configuration template. Requires a full-access API key."
    )]
    pub async fn template_create(
        &self,
        Parameters(params): Parameters<CreateTemplateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request: CreateTemplateRequest = serde_json::from_value(params.template)
            .map_err(|error| invalid_json_param("template", error))?;
        let state = TemplateRouteState::from_ref(&self.app_state);
        tool_json(templates::create_template(State(state), Json(request)).await)
    }

    #[tool(
        name = "template_update",
        description = "Update a configuration template. Requires a full-access API key."
    )]
    pub async fn template_update(
        &self,
        Parameters(params): Parameters<UpdateTemplateParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request: UpdateTemplateRequest = serde_json::from_value(params.updates)
            .map_err(|error| invalid_json_param("updates", error))?;
        let state = TemplateRouteState::from_ref(&self.app_state);
        tool_json(templates::update_template(State(state), Path(params.id), Json(request)).await)
    }

    #[tool(
        name = "template_delete",
        description = "Delete a configuration template (fails if streamers still use it). Requires a full-access API key."
    )]
    pub async fn template_delete(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let state = TemplateRouteState::from_ref(&self.app_state);
        tool_json(templates::delete_template(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "engine_list",
        description = "List download engine configurations (ffmpeg / streamlink / mesio) and their settings. Requires a full-access API key."
    )]
    pub async fn engine_list(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = EngineRouteState::from_ref(&self.app_state);
        tool_json(engines::list_engines(State(state)).await)
    }

    #[tool(
        name = "engine_get",
        description = "Get one engine configuration by ID. Requires a full-access API key."
    )]
    pub async fn engine_get(
        &self,
        Parameters(params): Parameters<IdParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = EngineRouteState::from_ref(&self.app_state);
        tool_json(engines::get_engine(State(state), Path(params.id)).await)
    }

    #[tool(
        name = "engine_update",
        description = "Update an engine configuration. Requires a full-access API key."
    )]
    pub async fn engine_update(
        &self,
        Parameters(params): Parameters<UpdateEngineParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_write(&context)?;
        let request: UpdateEngineRequest = serde_json::from_value(params.updates)
            .map_err(|error| invalid_json_param("updates", error))?;
        let state = EngineRouteState::from_ref(&self.app_state);
        tool_json(engines::update_engine(State(state), Path(params.id), Json(request)).await)
    }
}
