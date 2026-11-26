//! Authentication routes.
//!
//! Provides endpoints for user authentication and token management.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};

use crate::api::auth_service::{
    AuthPrincipal, AuthResponse, AuthService, CredentialKind, SessionInfo,
};
use crate::api::auth_store::{ApiKeyAccessLevel, ApiKeyDbModel};
use crate::api::error::{ApiError, ApiResult};
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::jwt::Claims;
use crate::api::middleware::AuthLayer;
use crate::api::rate_limit::ClientIp;
use crate::api::server::AppState;

#[derive(Clone)]
pub struct AuthRouteState {
    auth_service: Option<Arc<AuthService>>,
}

impl FromRef<AppState> for AuthRouteState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
        }
    }
}

/// Login request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    /// Username for authentication
    pub username: String,
    /// Password for authentication
    pub password: String,
    /// Optional device information for session tracking; stored up to 256 Unicode characters.
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

/// Create API key request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateApiKeyRequest {
    /// Human-readable label for the key
    pub name: String,
    /// Access level: `read_only` or `full`
    pub access_level: ApiKeyAccessLevel,
    /// Optional expiration as Unix epoch milliseconds (UTC)
    pub expires_at: Option<i64>,
}

/// API key metadata (never contains the raw key).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ApiKeyResponse {
    /// Key ID
    pub id: String,
    /// Human-readable label
    pub name: String,
    /// First characters of the raw key for identification
    pub key_prefix: String,
    /// Access level: `read_only` or `full`
    pub access_level: ApiKeyAccessLevel,
    /// Expiration as Unix epoch milliseconds (None = never)
    pub expires_at: Option<i64>,
    /// Last authorized use as Unix epoch milliseconds (throttled updates)
    pub last_used_at: Option<i64>,
    /// Creation time as Unix epoch milliseconds
    pub created_at: i64,
    /// Revocation time as Unix epoch milliseconds (None = active)
    pub revoked_at: Option<i64>,
}

impl From<ApiKeyDbModel> for ApiKeyResponse {
    fn from(model: ApiKeyDbModel) -> Self {
        let access_level = model.get_access_level();
        Self {
            id: model.id,
            name: model.name,
            key_prefix: model.key_prefix,
            access_level,
            expires_at: model.expires_at,
            last_used_at: model.last_used_at,
            created_at: model.created_at,
            revoked_at: model.revoked_at,
        }
    }
}

/// Create API key response: the only place the raw key ever appears.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CreateApiKeyResponse {
    /// The raw API key. Shown exactly once; only its hash is stored.
    pub api_key: String,
    /// Metadata of the created key
    pub key: ApiKeyResponse,
}

/// Create the public auth router (no JWT required).
pub fn public_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    AuthRouteState: FromRef<S>,
{
    Router::new()
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
}

/// Create the protected auth router (JWT required).
///
/// The caller wraps these routes in `AuthLayer::new`, which also rejects
/// users whose `must_change_password` flag is set; routes such a user must
/// still reach belong in `password_remediation_router` instead.
///
/// The `/api-keys` routes additionally reject API key credentials in their
/// handlers (`require_jwt_credential`) so a leaked key cannot manage keys.
pub fn protected_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    AuthRouteState: FromRef<S>,
{
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/api-keys", get(list_api_keys).post(create_api_key))
        .route("/api-keys/{id}", delete(revoke_api_key))
}

/// Create the router for the password-remediation endpoints, the only routes
/// a user with `must_change_password` set may reach.
///
/// The `AuthLayer::password_remediation` exemption is attached here, at
/// route registration, so it cannot drift from the registered paths; renaming
/// or moving a route in this router moves its exemption with it. `None`
/// (authentication disabled) leaves the routes unwrapped, mirroring how
/// `routes::create_router` skips `AuthLayer::new` for the other protected
/// routes.
pub fn password_remediation_router<S>(auth_service: Option<&Arc<AuthService>>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    AuthRouteState: FromRef<S>,
{
    let router = Router::new()
        .route("/logout-all", post(logout_all))
        .route("/change-password", post(change_password));

    match auth_service {
        Some(auth_service) => router.layer(AuthLayer::password_remediation(auth_service.clone())),
        None => router,
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
        (status = 429, description = "Too many failed attempts; retry after the seconds in `Retry-After`", body = crate::api::error::ApiErrorResponse),
        (status = 503, description = "Authentication service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AuthRouteState>,
    ClientIp(client_ip): ClientIp,
    ApiJson(request): ApiJson<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;

    let response = auth_service
        .authenticate(
            &request.username,
            &request.password,
            request.device_info,
            client_ip,
        )
        .await?;

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
    State(state): State<AuthRouteState>,
    ApiJson(request): ApiJson<RefreshRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Token refresh not available"))?;

    let response = auth_service.refresh_tokens(&request.refresh_token).await?;

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
    State(state): State<AuthRouteState>,
    headers: axum::http::HeaderMap,
    ApiJson(request): ApiJson<LogoutRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Logout not available"))?;

    let principal = if headers.contains_key(axum::http::header::AUTHORIZATION) {
        Some(
            auth_service
                .authorize_credential(
                    crate::api::auth_request::request_credential(&headers, None)?,
                    true,
                )
                .await?,
        )
    } else {
        None
    };
    auth_service
        .logout_with_principal(&request.refresh_token, principal.as_ref())
        .await?;

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
    State(state): State<AuthRouteState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Logout not available"))?;

    auth_service.logout_all(&claims.sub).await?;

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
    State(state): State<AuthRouteState>,
    axum::Extension(claims): axum::Extension<Claims>,
    ApiJson(request): ApiJson<ChangePasswordRequest>,
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
        .await?;

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
    State(state): State<AuthRouteState>,
    axum::Extension(claims): axum::Extension<Claims>,
) -> ApiResult<Json<Vec<SessionInfo>>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Session listing not available"))?;

    let sessions = auth_service.list_active_sessions(&claims.sub).await?;

    Ok(Json(sessions))
}

/// Reject API key credentials on the key-management endpoints, so a leaked
/// key cannot list, mint, or revoke keys. The `AuthLayer` inserts the
/// principal; it is absent only when authentication is disabled, in which
/// case there is no `AuthService` to manage keys with anyway.
fn require_jwt_credential(principal: Option<&AuthPrincipal>) -> Result<&AuthPrincipal, ApiError> {
    let principal =
        principal.ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;
    if principal.credential != CredentialKind::Jwt {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "API_KEY_NOT_ALLOWED",
            "API keys cannot manage API keys; use an interactive session",
        ));
    }
    Ok(principal)
}

/// Maximum accepted length for an API key label.
const API_KEY_NAME_MAX_LEN: usize = 100;

#[utoipa::path(
    get,
    path = "/api/auth/api-keys",
    tag = "auth",
    security(("bearer_auth" = [])),
    responses(
        (status = 200, description = "List of the caller's API keys", body = Vec<ApiKeyResponse>),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 403, description = "API keys cannot manage API keys", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn list_api_keys(
    State(state): State<AuthRouteState>,
    principal: Option<axum::Extension<AuthPrincipal>>,
) -> ApiResult<Json<Vec<ApiKeyResponse>>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;
    let principal = require_jwt_credential(principal.as_deref())?;

    let keys = auth_service.list_api_keys(&principal.claims.sub).await?;

    Ok(Json(keys.into_iter().map(ApiKeyResponse::from).collect()))
}

#[utoipa::path(
    post,
    path = "/api/auth/api-keys",
    tag = "auth",
    security(("bearer_auth" = [])),
    request_body = CreateApiKeyRequest,
    responses(
        (status = 200, description = "API key created; the raw key is returned exactly once", body = CreateApiKeyResponse),
        (status = 400, description = "Invalid request", body = crate::api::error::ApiErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 403, description = "API keys cannot manage API keys", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn create_api_key(
    State(state): State<AuthRouteState>,
    principal: Option<axum::Extension<AuthPrincipal>>,
    ApiJson(request): ApiJson<CreateApiKeyRequest>,
) -> ApiResult<Json<CreateApiKeyResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;
    let principal = require_jwt_credential(principal.as_deref())?;

    let name = request.name.trim();
    if name.is_empty() {
        return Err(ApiError::bad_request("API key name must not be empty"));
    }
    if name.chars().count() > API_KEY_NAME_MAX_LEN {
        return Err(ApiError::bad_request(format!(
            "API key name must be at most {API_KEY_NAME_MAX_LEN} characters"
        )));
    }
    if let Some(expires_at) = request.expires_at
        && expires_at <= crate::database::time::now_ms()
    {
        return Err(ApiError::bad_request(
            "API key expiration must be in the future",
        ));
    }

    let (model, raw_key) = auth_service
        .create_api_key(
            &principal.claims.sub,
            name,
            request.access_level,
            request.expires_at,
        )
        .await?;

    Ok(Json(CreateApiKeyResponse {
        api_key: raw_key,
        key: model.into(),
    }))
}

#[utoipa::path(
    delete,
    path = "/api/auth/api-keys/{id}",
    tag = "auth",
    security(("bearer_auth" = [])),
    params(("id" = String, Path, description = "API key ID")),
    responses(
        (status = 200, description = "API key revoked", body = crate::api::openapi::MessageResponse),
        (status = 401, description = "Unauthorized", body = crate::api::error::ApiErrorResponse),
        (status = 403, description = "API keys cannot manage API keys", body = crate::api::error::ApiErrorResponse),
        (status = 404, description = "Unknown or already revoked key", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn revoke_api_key(
    State(state): State<AuthRouteState>,
    principal: Option<axum::Extension<AuthPrincipal>>,
    ApiPath(id): ApiPath<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;
    let principal = require_jwt_credential(principal.as_deref())?;

    let revoked = auth_service
        .revoke_api_key(&principal.claims.sub, &id)
        .await?;

    if !revoked {
        return Err(ApiError::not_found("API key not found or already revoked"));
    }

    Ok(Json(
        serde_json::json!({ "message": "API key revoked successfully" }),
    ))
}
