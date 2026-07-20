//! API route modules.
//!
//! Organizes routes by resource type.

pub mod auth;
pub mod config;
pub mod credentials;
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
pub mod sessions;
pub mod stream_proxy;
pub mod streamers;
pub mod tdl;
pub mod templates;

use axum::Router;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::api::middleware::JwtAuthLayer;
use crate::api::openapi::ApiDoc;
use crate::api::server::AppState;

/// Create the main API router with all routes.
///
/// Routes are organized as:
/// - Public routes: `/api/auth/*` (login), `/api/health/live`
/// - Password-remediation routes: `/api/auth/change-password`, `/api/auth/logout-all`
///   (JWT required, but reachable while a password change is being forced;
///   they carry their own `JwtAuthLayer` via `auth::password_remediation_router`)
/// - Protected routes: All other `/api/*` routes (require JWT authentication)
/// - Documentation: `/api/docs` (Swagger UI), `/api/docs/openapi.json` (OpenAPI spec)
pub fn create_router(state: AppState) -> Router {
    // Build protected routes with state first
    let protected_routes: Router<AppState> = Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/streamers/{streamer_id}/filters", filters::router())
        .nest("/api/config", config::router())
        .nest("/api/config/backup", export_import::router())
        .nest("/api/credentials", credentials::router())
        .nest("/api/templates", templates::router())
        .nest("/api/engines", engines::router())
        .nest("/api/job", job::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/tools/tdl", tdl::router())
        .nest("/api/sessions", sessions::router())
        .nest("/api/notifications", notifications::router())
        .nest("/api/parse", parse::router())
        .nest("/api/auth", auth::protected_router());

    // Apply JWT auth layer to protected routes if authentication is enabled.
    // The layer wraps the router, so we need to handle the type conversion
    let protected_routes: Router<AppState> = if let Some(auth_service) = &state.auth_service {
        protected_routes.layer(JwtAuthLayer::new(auth_service.clone()))
    } else {
        protected_routes
    };

    // Build the main router with public routes first, then merge protected routes
    Router::new()
        // Swagger UI for API documentation
        .merge(SwaggerUi::new("/api/docs").url("/api/docs/openapi.json", ApiDoc::openapi()))
        // Public routes (no authentication required)
        .nest("/api/health", health::router())
        .nest("/api/auth", auth::public_router())
        // Password-remediation routes carry their own JwtAuthLayer, so they
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
        // Apply state to all routes
        .with_state(state)
}
