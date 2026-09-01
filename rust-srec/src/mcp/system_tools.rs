//! System diagnostics and URL parsing MCP tools.
//!
//! `system_health` reads `HealthChecker::current` directly instead of going
//! through `api::routes::health::health_check`, because that handler
//! re-validates a JWT bearer header (it lives on the public router) while
//! MCP requests were already authenticated by `AuthLayer::mcp`.

use axum::Json;
use axum::extract::{FromRef, State};
use rmcp::{
    ErrorData, RoleServer,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars,
    service::RequestContext,
    tool, tool_router,
};

use super::{SrecMcpServer, tool_json};
use crate::api::models::{ComponentHealth, HealthResponse, ParseUrlRequest};
use crate::api::routes::parse::{self, ParseRouteState};

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ParseUrlParams {
    /// Live room or video page URL to extract stream info from
    pub url: String,
    /// Optional cookies for authenticated extraction
    pub cookies: Option<String>,
}

#[tool_router(router = system_tools, vis = "pub(crate)")]
impl SrecMcpServer {
    #[tool(
        name = "system_health",
        description = "Get server health: overall status, uptime, per-component health, CPU and memory usage."
    )]
    pub async fn system_health(&self) -> Result<CallToolResult, ErrorData> {
        let uptime = self.app_state.start_time.elapsed().as_secs();
        let system_health = self.app_state.health_checker.current();
        let components: Vec<ComponentHealth> = system_health
            .components
            .iter()
            .map(|(name, health)| ComponentHealth {
                name: name.clone(),
                status: health.status.to_string(),
                message: health.message.clone(),
                last_check: health.last_check.clone(),
                check_duration_ms: health.check_duration_ms,
                disk: health.disk.clone(),
            })
            .collect();
        let response = HealthResponse {
            status: system_health.status.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: uptime,
            components,
            cpu_usage: system_health.cpu_usage,
            memory_usage: system_health.memory_usage,
        };
        Ok(CallToolResult::success(vec![ContentBlock::json(
            &response,
        )?]))
    }

    #[tool(
        name = "parse_url",
        description = "Parse a live room URL and extract stream metadata (title, live status, available qualities/streams). Makes an outbound request to the platform and requires a full-access API key."
    )]
    pub async fn parse_url_tool(
        &self,
        Parameters(params): Parameters<ParseUrlParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_full_access(&context)?;
        let state = ParseRouteState::from_ref(&self.app_state);
        let request = ParseUrlRequest {
            url: params.url,
            cookies: params.cookies,
        };
        tool_json(parse::parse_url(State(state), Json(request)).await)
    }
}
