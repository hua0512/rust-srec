//! API error handling.
//!
//! Provides consistent error responses for the API.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

use crate::error::Error;

/// API error response body.
#[derive(Debug, Serialize)]
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

    /// Add details to the error.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// Create a 400 Bad Request error.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "BAD_REQUEST", message)
    }

    /// Create a 401 Unauthorized error.
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", message)
    }

    /// Create a 403 Forbidden error.
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "FORBIDDEN", message)
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
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR", message)
    }

    /// Create a 500 Internal Server Error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR", message)
    }

    /// Create a 503 Service Unavailable error.
    pub fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "SERVICE_UNAVAILABLE", message)
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
            Error::Io(e) => {
                tracing::error!("IO error: {}", e);
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
    fn test_api_error_with_details() {
        let err = ApiError::validation("Invalid input")
            .with_details(serde_json::json!({"field": "email", "reason": "invalid format"}));
        
        assert!(err.details.is_some());
    }

    #[test]
    fn test_from_domain_error() {
        let domain_err = Error::not_found("Streamer", "123");
        let api_err: ApiError = domain_err.into();
        
        assert_eq!(api_err.status, StatusCode::NOT_FOUND);
        assert!(api_err.message.contains("123"));
    }
}
