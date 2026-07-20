//! API error handling.
//!
//! Provides consistent error responses for the API.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

use crate::api::auth_service::AuthError;
use crate::error::Error;

/// API error response body.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ApiErrorResponse {
    /// Error code for programmatic handling
    pub code: String,
    /// Human-readable error message
    pub message: String,
    /// Additional error details (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// API error type that can be converted to HTTP responses.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    /// Create a new API error.
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    /// Create a 400 Bad Request error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "BAD_REQUEST", message)
    }

    /// Create a 401 Unauthorized error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", message)
    }

    /// Create a 404 Not Found error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }

    /// Create a 409 Conflict error.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "CONFLICT", message)
    }

    /// Create a 422 Unprocessable Entity error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            message,
        )
    }

    /// Create a 500 Internal Server Error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
    }

    /// Create a 503 Service Unavailable error.
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
            message,
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ApiErrorResponse {
            code: self.code,
            message: self.message,
            details: self.details,
        };
        (self.status, Json(body)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::NotFound { entity_type, id } => {
                ApiError::not_found(format!("{} with id '{}' not found", entity_type, id))
            }
            Error::Validation(msg) => ApiError::validation(msg),
            Error::Configuration(msg) => ApiError::bad_request(msg),
            Error::DatabaseSqlx(e) => {
                tracing::error!("Database error: {}", e);
                ApiError::internal("Database error occurred")
            }
            Error::Database(msg) => {
                tracing::error!("Database error: {}", msg);
                ApiError::internal("Database error occurred")
            }
            Error::InvalidStateTransition { from, to } => {
                ApiError::conflict(format!("Cannot transition from {} to {}", from, to))
            }
            Error::DuplicateUrl(url) => {
                ApiError::conflict(format!("A streamer with URL '{}' already exists", url))
            }
            Error::Io(e) => {
                tracing::error!("IO error: {}", e);
                ApiError::internal("IO error occurred")
            }
            Error::IoPath { op, path, source } => {
                tracing::error!("IO error while {} '{}': {}", op, path, source);
                ApiError::internal("IO error occurred")
            }
            Error::ApiError(msg) => ApiError::bad_request(msg),
            _ => {
                tracing::error!("Unexpected error: {}", err);
                ApiError::internal("An unexpected error occurred")
            }
        }
    }
}

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        match err {
            AuthError::InvalidCredentials => ApiError::unauthorized("Invalid username or password"),
            AuthError::AccountDisabled => ApiError::new(
                StatusCode::FORBIDDEN,
                "ACCOUNT_DISABLED",
                "Account is disabled",
            ),
            AuthError::PasswordChangeRequired => ApiError::new(
                StatusCode::FORBIDDEN,
                "PASSWORD_CHANGE_REQUIRED",
                "Password change is required",
            ),
            AuthError::TokenExpired => ApiError::unauthorized("Token has expired"),
            AuthError::TokenRevoked => ApiError::unauthorized("Token has been revoked"),
            AuthError::InvalidToken => ApiError::unauthorized("Invalid token"),
            AuthError::WeakPassword(message) => {
                ApiError::bad_request(format!("Weak password: {message}"))
            }
            AuthError::IncorrectCurrentPassword => {
                ApiError::bad_request("Current password is incorrect")
            }
            AuthError::UserNotFound => ApiError::unauthorized("Invalid credentials"),
            AuthError::Database(error) => {
                tracing::error!(error = %error, "Authentication database error");
                ApiError::service_unavailable("Authentication service unavailable")
            }
            AuthError::Internal(error) => {
                tracing::error!(error = %error, "Authentication internal error");
                ApiError::internal("Authentication failed due to an internal error")
            }
        }
    }
}

/// Result type for API handlers.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_error_creation() {
        let err = ApiError::not_found("User not found");
        assert_eq!(err.status, StatusCode::NOT_FOUND);
        assert_eq!(err.code, "NOT_FOUND");
        assert_eq!(err.message, "User not found");
    }

    #[test]
    fn test_from_domain_error() {
        let domain_err = Error::not_found("Streamer", "123");
        let api_err: ApiError = domain_err.into();

        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
        assert!(api_err.message.contains("123"));
    }

    #[test]
    fn test_password_change_required_error_has_stable_code() {
        let api_err = ApiError::from(AuthError::PasswordChangeRequired);

        assert_eq!(api_err.status, StatusCode::FORBIDDEN);
        assert_eq!(api_err.code, "PASSWORD_CHANGE_REQUIRED");
    }

    #[test]
    fn test_auth_database_error_is_generic_service_unavailable() {
        let api_err = ApiError::from(AuthError::Database("sensitive details".to_string()));

        assert_eq!(api_err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(api_err.code, "SERVICE_UNAVAILABLE");
        assert!(!api_err.message.contains("sensitive"));
    }
}
