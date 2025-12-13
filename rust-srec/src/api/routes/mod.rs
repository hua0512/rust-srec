//! API route modules.
//!
//! Organizes routes by resource type.

pub mod auth;
pub mod config;
pub mod downloads;
pub mod engines;
pub mod filters;
pub mod health;
pub mod job;
pub mod media;
pub mod parse;
pub mod pipeline;
pub mod sessions;
pub mod streamers;
pub mod templates;

use axum::Router;
use std::sync::Arc;

use crate::api::middleware::JwtAuthLayer;
use crate::api::server::AppState;

pub use auth::{LoginRequest, LoginResponse};

/// Create the main API router with all routes.
///
/// Routes are organized as:
/// - Public routes: `/api/auth/*` (login), `/health/*`
/// - Protected routes: All other `/api/*` routes (require JWT authentication)
pub fn create_router(state: AppState) -> Router {
    // Build protected routes with state first
    let protected_routes: Router<AppState> = Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/streamers/{streamer_id}/filters", filters::router())
        .nest("/api/config", config::router())
        .nest("/api/templates", templates::router())
        .nest("/api/engines", engines::router())
        .nest("/api/job", job::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/sessions", sessions::router())
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
        // Public routes (no authentication required)
        .nest("/api/health", health::router())
        .nest("/api/auth", auth::public_router())
        .nest("/api/media", media::router())
        // WebSocket route with JWT auth via query parameter (not middleware)
        .nest("/api/downloads", downloads::router())
        // Merge protected routes
        .merge(protected_routes)
        // Apply state to all routes
        .with_state(state)
}
