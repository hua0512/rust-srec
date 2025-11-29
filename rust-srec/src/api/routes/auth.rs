//! Authentication routes.
//!
//! Provides endpoints for user authentication and token management.

use axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::error::{ApiError, ApiResult};
use crate::api::jwt::JwtService;
use crate::api::server::AppState;

/// Login request body.
#[derive(Debug, Clone, Deserialize)]
pub struct LoginRequest {
    /// Username for authentication
    pub username: String,
    /// Password for authentication
    pub password: String,
}

/// Login response body.
#[derive(Debug, Clone, Serialize)]
pub struct LoginResponse {
    /// JWT access token
    pub access_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Token expiration time in seconds
    pub expires_in: u64,
    /// User roles
    pub roles: Vec<String>,
}

/// Shared state for auth routes (kept for backward compatibility).
#[derive(Clone)]
pub struct AuthState {
    /// JWT service for token operations
    pub jwt_service: Arc<JwtService>,
}

/// Create the auth router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
}

/// POST /api/auth/login
///
/// Authenticate user and return JWT token.
///
/// # Request Body
/// - `username`: User's username
/// - `password`: User's password
///
/// # Response
/// - `access_token`: JWT token for authentication
/// - `token_type`: "Bearer"
/// - `expires_in`: Token expiration time in seconds
/// - `roles`: User's roles
async fn login(
    State(state): State<AppState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    // Get JWT service from state
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("JWT authentication not configured"))?;

    // Validate credentials (placeholder implementation)
    // In a real implementation, this would check against a user database
    let (user_id, roles) = validate_credentials(&request.username, &request.password)?;

    // Generate JWT token
    let token = jwt_service
        .generate_token(&user_id, roles.clone())
        .map_err(|e| ApiError::internal(format!("Failed to generate token: {}", e)))?;

    Ok(Json(LoginResponse {
        access_token: token,
        token_type: "Bearer".to_string(),
        expires_in: jwt_service.expiration_secs(),
        roles,
    }))
}

/// Validate user credentials.
///
/// This is a placeholder implementation. In production, this should:
/// - Query a user database
/// - Verify password hash
/// - Return user ID and roles
///
/// For now, it accepts a hardcoded admin user for testing.
fn validate_credentials(username: &str, password: &str) -> Result<(String, Vec<String>), ApiError> {
    // Placeholder: Accept admin/admin for testing
    // TODO: Replace with actual user authentication
    if username == "admin" && password == "admin" {
        return Ok(("admin".to_string(), vec!["admin".to_string(), "user".to_string()]));
    }

    // Placeholder: Accept any user with password "password" as a regular user
    // TODO: Replace with actual user authentication
    if password == "password" {
        return Ok((username.to_string(), vec!["user".to_string()]));
    }

    Err(ApiError::unauthorized("Invalid username or password"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_credentials_admin() {
        let result = validate_credentials("admin", "admin");
        assert!(result.is_ok());
        let (user_id, roles) = result.unwrap();
        assert_eq!(user_id, "admin");
        assert!(roles.contains(&"admin".to_string()));
    }

    #[test]
    fn test_validate_credentials_user() {
        let result = validate_credentials("testuser", "password");
        assert!(result.is_ok());
        let (user_id, roles) = result.unwrap();
        assert_eq!(user_id, "testuser");
        assert!(roles.contains(&"user".to_string()));
        assert!(!roles.contains(&"admin".to_string()));
    }

    #[test]
    fn test_validate_credentials_invalid() {
        let result = validate_credentials("admin", "wrongpassword");
        assert!(result.is_err());
    }

    #[test]
    fn test_login_request_deserialize() {
        let json = r#"{"username": "test", "password": "secret"}"#;
        let request: LoginRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.username, "test");
        assert_eq!(request.password, "secret");
    }

    #[test]
    fn test_login_response_serialize() {
        let response = LoginResponse {
            access_token: "token123".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            roles: vec!["user".to_string()],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("token123"));
        assert!(json.contains("Bearer"));
    }
}
