//! API middleware.
//!
//! Provides middleware for authentication, logging, and request handling.

pub mod jwt_auth;

pub use jwt_auth::{extract_claims, jwt_auth_middleware, JwtAuthError, JwtAuthLayer, JwtAuthService};
