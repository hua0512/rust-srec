//! API middleware.
//!
//! Provides middleware for authentication, logging, and request handling.

pub mod auth;

pub use auth::{ApiKeyAuth, ApiKeyAuthLayer};
