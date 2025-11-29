//! API route modules.
//!
//! Organizes routes by resource type.

pub mod auth;
pub mod config;
pub mod health;
pub mod pipeline;
pub mod sessions;
pub mod streamers;
pub mod templates;

use axum::Router;
use std::sync::Arc;

use crate::api::middleware::JwtAuthLayer;
use crate::api::server::AppState;

pub use auth::{AuthState, LoginRequest, LoginResponse};

/// Create the main API router with all routes.
///
/// Routes are organized as:
/// - Public routes: `/api/auth/*` (login), `/health/*`
/// - Protected routes: All other `/api/*` routes (require JWT authentication)
pub fn create_router(state: AppState) -> Router {
    // Build protected routes with state first
    let protected_routes: Router<AppState> = Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/config", config::router())
        .nest("/api/templates", templates::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/sessions", sessions::router());

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
        .nest("/health", health::router())
        .nest("/api/auth", auth::router())
        // Merge protected routes
        .merge(protected_routes)
        // Apply state to all routes
        .with_state(state)
}
