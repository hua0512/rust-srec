//! API route modules.
//!
//! Organizes routes by resource type.

pub mod auth;
pub mod config;
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
pub mod streamers;
pub mod templates;

use axum::Router;
use std::sync::Arc;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::api::middleware::JwtAuthLayer;
use crate::api::openapi::ApiDoc;
use crate::api::server::AppState;

pub use auth::{LoginRequest, LoginResponse};

/// Create the main API router with all routes.
///
/// Routes are organized as:
/// - Public routes: `/api/auth/*` (login), `/api/health/live`
/// - Protected routes: All other `/api/*` routes (require JWT authentication)
/// - Documentation: `/api/docs` (Swagger UI), `/api/docs/openapi.json` (OpenAPI spec)
pub fn create_router(state: AppState) -> Router {
    // Build protected routes with state first
    let protected_routes: Router<AppState> = Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/streamers/{streamer_id}/filters", filters::router())
        .nest("/api/config", config::router())
        .nest("/api/config/backup", export_import::router())
        .nest("/api/templates", templates::router())
        .nest("/api/engines", engines::router())
        .nest("/api/job", job::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/sessions", sessions::router())
        .nest("/api/notifications", notifications::router())
        .nest("/api/parse", parse::router())
        .nest("/api/auth", auth::protected_router());

    // Apply JWT auth layer to protected routes if JWT service is configured
    // The layer wraps the router, so we need to handle the type conversion
    let protected_routes: Router<AppState> = if let Some(jwt_service) = &state.jwt_service {
        protected_routes.layer(JwtAuthLayer::new(Arc::clone(jwt_service)))
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
        // WebSocket route with JWT auth via query parameter (not middleware)
        .nest("/api/downloads", downloads::router())
        // Logging routes with WebSocket (JWT auth via query param)
        .nest("/api/logging", logging::router())
        // Media route with optional query param auth (not middleware)
        .nest("/api/media", media::router())
        // Merge protected routes
        .merge(protected_routes)
        // Apply state to all routes
        .with_state(state)
}
