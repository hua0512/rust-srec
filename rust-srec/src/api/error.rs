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

/// Code returned when a cancel or retry targets a DAG that already reached a
/// terminal status. Clients match on this to treat the race as a no-op rather
/// than an error; keep it in sync with the frontend's
/// `DAG_ALREADY_TERMINAL_CODE` in `lib/api-error.ts`.
pub const DAG_ALREADY_TERMINAL_CODE: &str = "DAG_ALREADY_TERMINAL";

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
    /// Seconds emitted as the `Retry-After` response header. Only set by
    /// [`ApiError::too_many_requests`]; every other constructor leaves it
    /// `None` so the header is omitted.
    pub retry_after_secs: Option<u64>,
}

impl ApiError {
    /// Create a new API error.
    pub fn new(status: StatusCode, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            message: message.into(),
            details: None,
            retry_after_secs: None,
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

    /// Create a 429 Too Many Requests error carrying a `Retry-After` header.
    pub fn too_many_requests(message: impl Into<String>, retry_after_secs: u64) -> Self {
        Self {
            retry_after_secs: Some(retry_after_secs),
            ..Self::new(StatusCode::TOO_MANY_REQUESTS, "TOO_MANY_REQUESTS", message)
        }
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
        let retry_after = self
            .retry_after_secs
            .and_then(|secs| axum::http::HeaderValue::from_str(&secs.to_string()).ok());
        let body = ApiErrorResponse {
            code: self.code,
            message: self.message,
            details: self.details,
        };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(retry_after) = retry_after {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, retry_after);
        }
        response
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::Serialization(error) => ApiError::from(error),
            Error::NotFound { entity_type, id } => {
                ApiError::not_found(format!("{} with id '{}' not found", entity_type, id))
            }
            Error::Validation(msg) => ApiError::validation(msg),
            Error::DagAlreadyTerminal { ref dag_id } => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                DAG_ALREADY_TERMINAL_CODE,
                format!("DAG {} is already in a terminal state", dag_id),
            ),
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

impl From<serde_json::Error> for ApiError {
    fn from(error: serde_json::Error) -> Self {
        // Data errors can include the stored value itself, including credentials.
        tracing::error!(category = ?error.classify(), line = error.line(), column = error.column(),
            "Internal JSON operation failed");
        Self::internal("Internal JSON operation failed")
    }
}

impl From<std::io::Error> for ApiError {
    fn from(error: std::io::Error) -> Self {
        Self::from(Error::Io(error))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::from(Error::DatabaseSqlx(error))
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(error: tokio::task::JoinError) -> Self {
        tracing::error!(
            cancelled = error.is_cancelled(),
            panicked = error.is_panic(),
            "API background operation failed"
        );
        Self::internal("Background operation failed")
    }
}

impl From<zip::result::ZipError> for ApiError {
    fn from(error: zip::result::ZipError) -> Self {
        match error {
            zip::result::ZipError::Io(error) => Self::from(error),
            _ => Self::internal("Archive operation failed"),
        }
    }
}

impl From<axum::http::header::InvalidHeaderValue> for ApiError {
    fn from(_: axum::http::header::InvalidHeaderValue) -> Self {
        Self::internal("Response header could not be encoded")
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
            AuthError::UsernameTooLong { max } => {
                ApiError::bad_request(format!("Username must be at most {max} characters"))
            }
            // Deliberately identical whether or not the username exists, so a
            // throttled caller cannot use the 429 to enumerate accounts.
            AuthError::TooManyAttempts { retry_after_secs } => ApiError::too_many_requests(
                "Too many failed login attempts; try again later",
                retry_after_secs,
            ),
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
    fn internal_sources_do_not_reach_error_responses_but_validation_stays_actionable() {
        let stored_json = serde_json::from_str::<u64>("\"stored-secret-value\"").unwrap_err();
        assert!(stored_json.to_string().contains("stored-secret-value"));
        let errors = [
            ApiError::from(stored_json),
            ApiError::from(std::io::Error::other("private-filesystem-detail")),
            ApiError::from(Error::Database("private-database-detail".to_string())),
            ApiError::from(Error::Other("private-service-detail".to_string())),
        ];
        for error in errors {
            assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(!error.message.contains("secret"));
            assert!(!error.message.contains("private"));
        }
        let validation =
            ApiError::from(Error::Validation("At most 100 IDs are allowed".to_string()));
        assert_eq!(validation.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(validation.message, "At most 100 IDs are allowed");
    }

    #[test]
    fn route_internal_errors_use_literal_messages_or_typed_mapping() {
        // Keep stored/backend values out of ad-hoc 500 responses. Request validation remains
        // free to explain the caller's own invalid fields.
        let routes = [
            include_str!("routes/config.rs"),
            include_str!("routes/engines.rs"),
            include_str!("routes/templates.rs"),
            include_str!("routes/credentials.rs"),
            include_str!("routes/notifications.rs"),
            include_str!("routes/export_import.rs"),
            include_str!("routes/filters.rs"),
            include_str!("routes/media.rs"),
            include_str!("routes/logging.rs"),
            include_str!("routes/logging/archive.rs"),
            include_str!("routes/sessions.rs"),
            include_str!("routes/stream_proxy.rs"),
            include_str!("routes/baidupcs.rs"),
        ];
        for source in routes {
            for call in source.split("ApiError::internal(").skip(1) {
                assert!(
                    call.trim_start().starts_with('"'),
                    "internal responses must not interpolate backend details"
                );
            }
        }
    }

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
    fn dag_already_terminal_has_its_own_code() {
        let api_err = ApiError::from(Error::DagAlreadyTerminal {
            dag_id: "dag-1".to_string(),
        });

        // The frontend branches its cancel UX on this code. A plain
        // `Error::Validation` would collapse it into VALIDATION_ERROR and
        // leave callers matching on message text.
        assert_eq!(api_err.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(api_err.code, DAG_ALREADY_TERMINAL_CODE);
        assert_ne!(api_err.code, ApiError::validation("x").code);
        assert!(api_err.message.contains("dag-1"));
    }

    #[test]
    fn test_auth_database_error_is_generic_service_unavailable() {
        let api_err = ApiError::from(AuthError::Database("sensitive details".to_string()));

        assert_eq!(api_err.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(api_err.code, "SERVICE_UNAVAILABLE");
        assert!(!api_err.message.contains("sensitive"));
    }
}
