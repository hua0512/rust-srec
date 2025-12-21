//! API middleware.
//!
//! Provides middleware for authentication, logging, and request handling.

pub mod jwt_auth;
pub mod password_change;

pub use jwt_auth::{
    JwtAuthError, JwtAuthLayer, JwtAuthService, extract_claims, jwt_auth_middleware,
};
pub use password_change::{
    PasswordChangeLayer, PasswordChangeRequiredError, PasswordChangeService,
};
