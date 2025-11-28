//! API route modules.
//!
//! Organizes routes by resource type.

pub mod config;
pub mod health;
pub mod pipeline;
pub mod sessions;
pub mod streamers;
pub mod templates;

use axum::Router;

use crate::api::server::AppState;

/// Create the main API router with all routes.
pub fn create_router(state: AppState) -> Router {
    Router::new()
        .nest("/api/streamers", streamers::router())
        .nest("/api/config", config::router())
        .nest("/api/templates", templates::router())
        .nest("/api/pipeline", pipeline::router())
        .nest("/api/sessions", sessions::router())
        .nest("/health", health::router())
        .with_state(state)
}
