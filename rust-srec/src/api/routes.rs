//! API route modules.
//!
//! Organizes routes by resource type.

pub mod auth;
pub mod baidupcs;
pub mod config;
pub mod credential_profiles;
pub mod downloads;
pub mod engines;
pub mod export_import;
pub mod filters;
pub mod health;
pub mod job;
pub mod logging;
pub mod media;
pub mod notifications;
pub mod parse;
pub mod pipeline;
pub mod proxies;
pub mod sessions;
pub mod stream_proxy;
pub mod streamers;
pub mod templates;

use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::api::auth_service::AuthPrincipal;
use crate::api::error::{ApiError, ApiResult};
use crate::api::middleware::AuthLayer;
use crate::api::openapi::ApiDoc;
use crate::api::server::AppState;

/// Owner of playback contexts and login sessions while authentication is
/// disabled. Parse stores contexts under it and the stream proxy looks them up
/// under the same name, so both must use this constant.
pub(crate) const LOCAL_ANONYMOUS_PRINCIPAL: &str = "local-anonymous";

/// The caller's principal; anonymous callers are accepted only while
/// authentication is disabled.
pub(crate) fn request_principal(
    auth_enabled: bool,
    identity: Option<axum::Extension<AuthPrincipal>>,
) -> ApiResult<String> {
    match identity {
        Some(axum::Extension(principal)) => Ok(principal.claims.sub),
        None if !auth_enabled => Ok(LOCAL_ANONYMOUS_PRINCIPAL.into()),
        None => Err(ApiError::unauthorized("Authentication required")),
    }
}

/// Headers for responses that carry account-scoped data or playback URLs.
pub(crate) fn private_response_headers() -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    headers.insert(
        axum::http::header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("no-referrer"),
    );
    headers
}

/// Create the main API router with all routes.
///
/// Routes are organized as:
/// - Public routes: `/api/auth/*` (login), `/api/health/live`
/// - Password-remediation routes: `/api/auth/change-password`, `/api/auth/logout-all`
///   (JWT required, but reachable while a password change is being forced;
///   they carry their own `AuthLayer` via `auth::password_remediation_router`)
/// - Protected routes: All other `/api/*` routes (require a JWT or API key)
/// - MCP endpoint: `/api/mcp` (Streamable HTTP; JWT or API key via
///   `AuthLayer::mcp`, which defers read-only enforcement to the tools)
/// - Documentation: `/api/docs` (Swagger UI), `/api/docs/openapi.json` (OpenAPI spec)
pub fn create_router(state: AppState) -> Router {
    // Build protected routes with state first
    let protected_routes: Router<AppState> = Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/streamers/{streamer_id}/filters", filters::router())
        .nest("/api/config", config::router())
        .nest("/api/config/backup", export_import::router())
        .nest("/api/credentials", credential_profiles::router())
        .nest("/api/proxies", proxies::router())
        .nest("/api/templates", templates::router())
        .nest("/api/engines", engines::router())
        .nest("/api/job", job::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/sessions", sessions::router())
        .nest("/api/notifications", notifications::router())
        .nest("/api/parse", parse::router())
        .nest("/api/tools/baidupcs", baidupcs::router())
        .nest("/api/auth", auth::protected_router());

    // Apply the auth layer to protected routes if authentication is enabled.
    // The layer wraps the router, so we need to handle the type conversion
    let protected_routes: Router<AppState> = if let Some(auth_service) = &state.auth_service {
        protected_routes.layer(AuthLayer::new(auth_service.clone()))
    } else {
        protected_routes
    };

    // MCP endpoint (Streamable HTTP). Uses `AuthLayer::mcp`: same JWT/API key
    // validation as the REST routes, but read-only keys are allowed to POST
    // because scope is enforced per tool via
    // `mcp::SrecMcpServer::require_full_access`.
    let mcp_routes: Router<AppState> = Router::new().nest_service(
        "/api/mcp",
        crate::mcp::streamable_http_service(state.clone()),
    );
    let mcp_routes = if let Some(auth_service) = &state.auth_service {
        mcp_routes.layer(AuthLayer::mcp(auth_service.clone()))
    } else {
        mcp_routes
    };

    // Build the main router with public routes first, then merge protected routes
    Router::new()
        // Swagger UI for API documentation
        .merge(SwaggerUi::new("/api/docs").url("/api/docs/openapi.json", ApiDoc::openapi()))
        // Public routes (no authentication required)
        .nest("/api/health", health::router())
        .nest("/api/auth", auth::public_router())
        // Password-remediation routes carry their own AuthLayer, so they
        // must sit outside the shared layer above (which would 403 the
        // forced-change users they exist for).
        .nest(
            "/api/auth",
            auth::password_remediation_router(state.auth_service.as_ref()),
        )
        // WebSocket route with JWT auth via query parameter (not middleware)
        .nest("/api/downloads", downloads::router())
        // Logging routes with WebSocket (JWT auth via query param)
        .nest("/api/logging", logging::router())
        // Media route with optional query param auth (not middleware)
        .nest("/api/media", media::router())
        // Stream proxy route with query-param auth (not middleware)
        .nest("/api/stream-proxy", stream_proxy::router::<AppState>())
        // Merge protected routes
        .merge(protected_routes)
        // MCP endpoint with its own auth layer
        .merge(mcp_routes)
        // Apply state to all routes
        .with_state(state)
}
