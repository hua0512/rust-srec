//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub(crate) mod auth_request;
pub(crate) mod auth_service;
#[doc(hidden)]
pub mod auth_store;
pub(crate) mod backup;
pub(crate) mod batch_lookup;
pub(crate) mod bootstrap;
pub(crate) mod cors;
pub(crate) mod error;
pub(crate) mod extract;
pub(crate) mod jwt;
pub(crate) mod middleware;
pub(crate) mod models;
pub(crate) mod openapi;
pub(crate) mod playback_context;
pub(crate) mod proto;
#[doc(hidden)]
pub mod queries;
pub(crate) mod rate_limit;
pub(crate) mod routes;
pub(crate) mod schemas;
pub(crate) mod server;
