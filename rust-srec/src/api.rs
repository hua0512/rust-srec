//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub(crate) mod auth_request;
pub mod auth_service;
pub(crate) mod batch_lookup;
pub mod cors;
pub mod error;
pub mod jwt;
pub mod middleware;
pub mod models;
pub mod openapi;
pub mod proto;
pub mod rate_limit;
pub mod routes;
pub mod server;
