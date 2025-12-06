//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub mod auth_service;
pub mod error;
pub mod jwt;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod server;

pub use auth_service::{AuthConfig, AuthError, AuthResponse, AuthService, SessionInfo};
pub use jwt::{Claims, JwtError, JwtService};
pub use server::ApiServer;
