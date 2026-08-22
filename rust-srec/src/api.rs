//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub mod auth_service;
pub mod error;
pub mod jwt;
pub mod middleware;
pub mod models;
pub mod openapi;
pub mod proto;
pub mod routes;
pub mod server;
