//! REST API server module.
//!
//! Provides HTTP endpoints for managing streamers, configurations,
//! templates, and monitoring pipeline jobs.

pub mod auth_request;
pub mod auth_service;
#[doc(hidden)]
pub mod auth_store;
pub mod backup;
pub mod batch_lookup;
pub mod bootstrap;
pub(crate) mod cors;
pub mod error;
pub mod extract;
pub mod jwt;
pub(crate) mod live_danmu;
pub(crate) mod middleware;
pub mod models;
pub mod openapi;
pub mod playback_context;
#[doc(hidden)]
pub mod proto;
#[doc(hidden)]
pub mod queries;
pub mod rate_limit;
pub mod routes;
pub(crate) mod schemas;
pub mod server;
