//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub mod error;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod server;

pub use server::ApiServer;
