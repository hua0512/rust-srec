//! Authentication routes.
//!
//! Provides endpoints for user authentication and token management.

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::api::auth_service::{AuthError, AuthResponse, SessionInfo};
use crate::api::error::{ApiError, ApiResult};
use crate::api::jwt::Claims;
use crate::api::server::AppState;

/// Login request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Username for authentication
    pub username: String,
    /// Password for authentication
    pub password: String,
    /// Optional device information for session tracking
    pub device_info: Option<String>,
}

/// Login response body with refresh token.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LoginResponse {
    /// JWT access token
    pub access_token: String,
    /// Opaque refresh token
    pub refresh_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Access token expiration time in seconds
    pub expires_in: u64,
    /// Refresh token expiration time in seconds
    pub refresh_expires_in: u64,
    /// User roles
    pub roles: Vec<String>,
    /// Whether the user must change their password
    pub must_change_password: bool,
}

impl From<AuthResponse> for LoginResponse {
    fn from(auth: AuthResponse) -> Self {
        Self {
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            token_type: auth.token_type,
            expires_in: auth.expires_in,
            refresh_expires_in: auth.refresh_expires_in,
            roles: auth.roles,
            must_change_password: auth.must_change_password,
        }
    }
}

/// Refresh token request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct RefreshRequest {
    /// The refresh token to use
    pub refresh_token: String,
}

/// Logout request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LogoutRequest {
    /// The refresh token to revoke
    pub refresh_token: String,
}

/// Change password request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ChangePasswordRequest {
    /// Current password for verification
    pub current_password: String,
    /// New password to set
    pub new_password: String,
}

/// Create the public auth router (no JWT required).
pub fn public_router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
}

/// Create the protected auth router (JWT required).
pub fn protected_router() -> Router<AppState> {
    Router::new()
        .route("/logout-all", post(logout_all))
        .route("/change-password", post(change_password))
        .route("/sessions", get(list_sessions))
}

/// Convert AuthError to ApiError.
fn auth_error_to_api_error(err: AuthError) -> ApiError {
    match err {
        AuthError::InvalidCredentials => ApiError::unauthorized("Invalid username or password"),
        AuthError::AccountDisabled => ApiError::unauthorized("Account is disabled"),
        AuthError::TokenExpired => ApiError::unauthorized("Token has expired"),
        AuthError::TokenRevoked => ApiError::unauthorized("Token has been revoked"),
        AuthError::InvalidToken => ApiError::unauthorized("Invalid token"),
        AuthError::PasswordChangeRequired => {
            ApiError::forbidden("Password change required before accessing resources")
        }
        AuthError::WeakPassword(msg) => ApiError::bad_request(format!("Weak password: {}", msg)),
        AuthError::IncorrectCurrentPassword => {
            ApiError::bad_request("Current password is incorrect")
        }
        AuthError::UserNotFound => ApiError::unauthorized("Invalid credentials"),
        AuthError::Database(msg) => ApiError::internal(format!("Database error: {}", msg)),
        AuthError::Internal(msg) => ApiError::internal(msg),
    }
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Invalid credentials", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Authentication service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;

    let response = auth_service
        .authenticate(&request.username, &request.password, request.device_info)
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(response.into()))
}

#[utoipa::path(
    post,
    path = "/api/auth/refresh",
    tag = "auth",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Token refreshed successfully", body = LoginResponse),
        (status = 401, description = "Invalid or expired refresh token", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Refresh service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn refresh(
    State(state): State<AppState>,
    Json(request): Json<RefreshRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Token refresh not available"))?;

    let response = auth_service
        .refresh_tokens(&request.refresh_token)
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(response.into()))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    request_body = LogoutRequest,
    responses(
        (status = 200, description = "Logout successful", body = crate::api::openapi::MessageResponse),
        (status = 401, description = "Invalid token", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Logout service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    Json(request): Json<LogoutRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Logout not available"))?;

    auth_service
        .logout(&request.refresh_token)
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(
        serde_json::json!({ "message": "Logged out successfully" }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/auth/logout-all",
    tag = "auth",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "All sessions logged out", body = crate::api::openapi::MessageResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Logout service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn logout_all(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Logout not available"))?;

    auth_service
        .logout_all(&claims.sub)
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(
        serde_json::json!({ "message": "All sessions logged out successfully" }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/auth/change-password",
    tag = "auth",
    security(("bearer_auth" = [])),
    request_body = ChangePasswordRequest,
    responses(
        (status = 200, description = "Password changed successfully", body = crate::api::openapi::MessageResponse),
        (status = 400, description = "Invalid password", body = crate::api::error::ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Password change service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
    Json(request): Json<ChangePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Password change not available"))?;

    auth_service
        .change_password(
            &claims.sub,
            &request.current_password,
            &request.new_password,
        )
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(
        serde_json::json!({ "message": "Password changed successfully" }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/auth/sessions",
    tag = "auth",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of active sessions", body = Vec<crate::api::auth_service::SessionInfo>),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Session listing unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn list_sessions(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResult<Json<Vec<SessionInfo>>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session listing not available"))?;

    let sessions = auth_service
        .list_active_sessions(&claims.sub)
        .await
        .map_err(auth_error_to_api_error)?;

    Ok(Json(sessions))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_request_deserialize() {
        let json = r#"{"username": "test", "password": "secret"}"#;
        let request: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.username, "test");
        assert_eq!(request.password, "secret");
        assert!(request.device_info.is_none());
    }

    #[test]
    fn test_login_request_with_device_info() {
        let json =
            r#"{"username": "test", "password": "secret", "device_info": "Chrome on Windows"}"#;
        let request: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.device_info, Some("Chrome on Windows".to_string()));
    }

    #[test]
    fn test_login_response_serialize() {
        let response = LoginResponse {
            access_token: "token123".to_string(),
            refresh_token: "refresh456".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 900,
            refresh_expires_in: 604800,
            roles: vec!["user".to_string()],
            must_change_password: false,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("token123"));
        assert!(json.contains("refresh456"));
        assert!(json.contains("Bearer"));
        assert!(json.contains("must_change_password"));
    }

    #[test]
    fn test_refresh_request_deserialize() {
        let json = r#"{"refresh_token": "abc123"}"#;
        let request: RefreshRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.refresh_token, "abc123");
    }

    #[test]
    fn test_change_password_request_deserialize() {
        let json = r#"{"current_password": "old", "new_password": "new123"}"#;
        let request: ChangePasswordRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.current_password, "old");
        assert_eq!(request.new_password, "new123");
    }
}
