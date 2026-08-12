//! Built-in MCP (Model Context Protocol) server.
//!
//! Exposes the application's REST capabilities as MCP tools over the
//! Streamable HTTP transport, mounted at `/api/mcp` by
//! `api::routes::create_router`. Authentication is handled in front of the
//! transport by `api::middleware::auth::AuthLayer::mcp` (JWT or API key);
//! per-tool write access is enforced here via [`SrecMcpServer::require_write`]
//! because every MCP request is a POST and the middleware's method-based
//! read-only check cannot apply.
//!
//! Tools do not duplicate business logic: each one builds the corresponding
//! route state via `FromRef<AppState>` and calls the same `api::routes`
//! handler the REST endpoint uses, so validation, cache invalidation, and
//! `ConfigUpdateEvent` broadcasting behave identically for both surfaces.

use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::router::tool::ToolRouter,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool_handler,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};

use crate::api::auth_service::AuthPrincipal;
use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::database::models::ApiKeyAccessLevel;

mod config_tools;
mod notification_tools;
mod pipeline_tools;
mod session_tools;
mod streamer_tools;
mod system_tools;

/// MCP server handler backed by the shared [`AppState`].
#[derive(Clone)]
pub struct SrecMcpServer {
    app_state: AppState,
    /// Mirrors `AppState.auth_service.is_some()`. When authentication is
    /// disabled (`AUTH_DISABLED`, loopback only) no `AuthPrincipal` exists
    /// and `require_write` allows everything, matching the unauthenticated
    /// REST surface in that mode.
    auth_enabled: bool,
    tool_router: ToolRouter<Self>,
}

impl SrecMcpServer {
    pub fn new(app_state: AppState) -> Self {
        let auth_enabled = app_state.auth_service.is_some();
        Self {
            app_state,
            auth_enabled,
            tool_router: Self::config_tools()
                + Self::streamer_tools()
                + Self::session_tools()
                + Self::pipeline_tools()
                + Self::notification_tools()
                + Self::system_tools(),
        }
    }

    /// Enforce write access for mutating tools.
    ///
    /// Reads the `AuthPrincipal` that `AuthLayer` stored in the request
    /// extensions (propagated into the tool context by
    /// `StreamableHttpService` via `http::request::Parts`). Fails closed
    /// when authentication is enabled but no principal is present.
    fn require_write(&self, context: &RequestContext<RoleServer>) -> Result<(), ErrorData> {
        let principal = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<AuthPrincipal>());
        check_write_access(self.auth_enabled, principal)
    }
}

/// Core write-access rule shared by every mutating tool.
///
/// `auth_enabled == false` (AUTH_DISABLED loopback mode) allows everything,
/// mirroring the unauthenticated REST surface. Otherwise a `Full` principal
/// is required; a missing principal fails closed because `AuthLayer::mcp`
/// always inserts one for authenticated requests.
fn check_write_access(
    auth_enabled: bool,
    principal: Option<&AuthPrincipal>,
) -> Result<(), ErrorData> {
    if !auth_enabled {
        return Ok(());
    }
    match principal {
        Some(principal) if principal.access == ApiKeyAccessLevel::Full => Ok(()),
        Some(_) => Err(ErrorData::invalid_request(
            "This API key is read-only; this tool requires a full-access API key",
            None,
        )),
        None => Err(ErrorData::invalid_request(
            "Missing authenticated principal for a write tool",
            None,
        )),
    }
}

/// Convert a REST handler result into an MCP tool result.
///
/// Success bodies are serialized to a JSON text block. `ApiError`s become
/// tool execution errors (`is_error: true`) rather than protocol errors so
/// the model can read the failure and adjust its next call.
pub(crate) fn tool_json<T: serde::Serialize>(
    result: ApiResult<axum::Json<T>>,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(axum::Json(value)) => Ok(CallToolResult::success(vec![ContentBlock::json(&value)?])),
        Err(error) => Ok(api_error_result(error)),
    }
}

/// Convert a status-only REST handler result (e.g. 204 handlers) into an MCP
/// tool result.
pub(crate) fn tool_status(
    result: Result<axum::http::StatusCode, ApiError>,
    success_message: &str,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(_) => Ok(CallToolResult::success(vec![ContentBlock::text(
            success_message,
        )])),
        Err(error) => Ok(api_error_result(error)),
    }
}

/// Convert a body-less REST handler result (`ApiResult<()>`) into an MCP
/// tool result.
pub(crate) fn tool_unit(
    result: ApiResult<()>,
    success_message: &str,
) -> Result<CallToolResult, ErrorData> {
    match result {
        Ok(()) => Ok(CallToolResult::success(vec![ContentBlock::text(
            success_message,
        )])),
        Err(error) => Ok(api_error_result(error)),
    }
}

pub(crate) fn api_error_result(error: ApiError) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {}",
        error.code, error.message
    ))])
}

/// Map a serde deserialization failure of a JSON tool argument into a
/// protocol-level invalid-params error naming the offending parameter.
pub(crate) fn invalid_json_param(param: &str, error: serde_json::Error) -> ErrorData {
    ErrorData::invalid_params(format!("Invalid '{param}' payload: {error}"), None)
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SrecMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("rust-srec", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "rust-srec live stream recorder control server. Tool groups: \
                 config_* / template_* / engine_* (recording configuration; global -> platform -> \
                 template -> streamer override hierarchy), streamer_* / filter_* (recorded \
                 streamer management), session_* (recording sessions, segments, and danmu/chat \
                 statistics and XML content), pipeline_* / job_preset_* (post-processing jobs and \
                 DAGs), notification_* (notification channels), system_* and parse_url \
                 (diagnostics and URL extraction). Write tools require a full-access API key; \
                 read-only keys can only call read tools. JSON-object arguments follow the same \
                 schemas as the REST API documented at /api/docs.",
            )
    }
}

/// Build the Streamable HTTP service that `routes::create_router` mounts at
/// `/api/mcp`.
///
/// Stateless (`legacy_session_mode: false`): every tool call is served on its
/// own POST, which keeps per-request authentication meaningful and avoids
/// server-side session state. Host validation is disabled to match the rest
/// of the API surface (rust-srec commonly runs behind reverse proxies or on
/// non-localhost binds; `/api/mcp` is protected by `AuthLayer::mcp` instead).
pub fn streamable_http_service(
    state: AppState,
) -> StreamableHttpService<SrecMcpServer, LocalSessionManager> {
    let server = SrecMcpServer::new(state);
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts();
    StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth_service::CredentialKind;
    use crate::api::jwt::Claims;

    fn full_router() -> ToolRouter<SrecMcpServer> {
        SrecMcpServer::config_tools()
            + SrecMcpServer::streamer_tools()
            + SrecMcpServer::session_tools()
            + SrecMcpServer::pipeline_tools()
            + SrecMcpServer::notification_tools()
            + SrecMcpServer::system_tools()
    }

    /// Building the tool list exercises every generated parameter schema, so
    /// a schema that fails to generate would panic here rather than at the
    /// first `tools/list` request in production.
    #[test]
    fn tool_router_lists_all_groups() {
        let tools = full_router().list_all();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();

        for expected in [
            "config_get_global",
            "config_update_global",
            "config_list_platforms",
            "template_list",
            "engine_list",
            "streamer_list",
            "streamer_create",
            "filter_list",
            "session_list",
            "session_danmu_statistics",
            "session_list_danmu_files",
            "session_read_danmu",
            "pipeline_stats",
            "pipeline_list_jobs",
            "pipeline_list_dags",
            "notification_list_channels",
            "system_health",
            "parse_url",
        ] {
            assert!(names.contains(&expected), "missing tool: {expected}");
        }

        // Every tool name must be unique; duplicate registration would make
        // ToolRouter route to only one of them silently.
        let mut deduped = names.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(deduped.len(), names.len(), "duplicate tool names");
    }

    fn principal(access: ApiKeyAccessLevel) -> AuthPrincipal {
        AuthPrincipal {
            claims: Claims {
                sub: "user-1".to_string(),
                roles: vec!["user".to_string()],
                iss: "rust-srec".to_string(),
                aud: "rust-srec-api".to_string(),
                exp: 0,
                iat: 0,
            },
            credential: CredentialKind::ApiKey,
            access,
        }
    }

    #[test]
    fn write_access_rules() {
        // AUTH_DISABLED mode: everything allowed, principal or not.
        assert!(check_write_access(false, None).is_ok());

        // Auth enabled: full key writes, read-only key rejected, missing
        // principal fails closed.
        let full = principal(ApiKeyAccessLevel::Full);
        assert!(check_write_access(true, Some(&full)).is_ok());

        let read_only = principal(ApiKeyAccessLevel::ReadOnly);
        let error = check_write_access(true, Some(&read_only))
            .expect_err("read-only key must not pass the write gate");
        assert!(error.message.contains("read-only"));

        assert!(check_write_access(true, None).is_err());
    }
}
