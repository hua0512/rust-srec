//! Authentication routes.
//!
//! Provides endpoints for user authentication and token management.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, State},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::api::auth_service::{AuthResponse, AuthService, SessionInfo};
use crate::api::error::{ApiError, ApiResult};
use crate::api::jwt::Claims;
use crate::api::middleware::JwtAuthLayer;
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
/// The caller wraps these routes in `JwtAuthLayer::new`, which also rejects
/// users whose `must_change_password` flag is set; routes such a user must
/// still reach belong in `password_remediation_router` instead.
pub fn protected_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    AuthRouteState: FromRef<S>,
{
    Router::new().route("/sessions", get(list_sessions))
}

/// Create the router for the password-remediation endpoints, the only routes
/// a user with `must_change_password` set may reach.
///
/// The `JwtAuthLayer::password_remediation` exemption is attached here, at
/// route registration, so it cannot drift from the registered paths; renaming
/// or moving a route in this router moves its exemption with it. `None`
/// (authentication disabled) leaves the routes unwrapped, mirroring how
/// `routes::create_router` skips `JwtAuthLayer::new` for the other protected
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
        Some(auth_service) => {
            router.layer(JwtAuthLayer::password_remediation(auth_service.clone()))
        }
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
        (status = 503, description = "Authentication service unavailable", body = crate::api::error::ApiErrorResponse)
    )
)]
pub async fn login(
    State(state): State<AuthRouteState>,
    Json(request): Json<LoginRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Authentication not configured"))?;

    let response = auth_service
        .authenticate(&request.username, &request.password, request.device_info)
        .await
        .map_err(ApiError::from)?;

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
    Json(request): Json<RefreshRequest>,
) -> ApiResult<Json<LoginResponse>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Token refresh not available"))?;

    let response = auth_service
        .refresh_tokens(&request.refresh_token)
        .await
        .map_err(ApiError::from)?;

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
    Json(request): Json<LogoutRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let auth_service = state
        .auth_service
        .as_ref()
        .ok_or_else(|| ApiError::service_unavailable("Logout not available"))?;

    auth_service
        .logout(&request.refresh_token)
        .await
        .map_err(ApiError::from)?;

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

    auth_service
        .logout_all(&claims.sub)
        .await
        .map_err(ApiError::from)?;

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
        .map_err(ApiError::from)?;

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

    let sessions = auth_service
        .list_active_sessions(&claims.sub)
        .await
        .map_err(ApiError::from)?;

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

    mod password_remediation_drift_guard {
        //! Pins the contract between `password_remediation_router`,
        //! `protected_router`, and the `JwtAuthLayer` wiring mirrored from
        //! `routes::create_router`: exactly the routes registered in
        //! `password_remediation_router` stay reachable for a user whose
        //! `must_change_password` flag is set, and only past JWT validation.

        use axum::body::{Body, to_bytes};
        use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        use super::super::*;
        use crate::api::auth_service::AuthConfig;
        use crate::api::jwt::JwtService;
        use crate::database::models::{RefreshTokenDbModel, UserDbModel};
        use crate::database::repositories::{RefreshTokenRepository, UserRepository};

        const CURRENT_PASSWORD: &str = "current-pass-1";

        /// Serves a single user by id so `AuthService::authorize_access_token`
        /// and `AuthService::change_password` see the same forced-change row.
        struct ForcedChangeUserRepository {
            user: UserDbModel,
        }

        #[async_trait::async_trait]
        impl UserRepository for ForcedChangeUserRepository {
            async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
                Ok(())
            }

            async fn find_by_id(&self, id: &str) -> crate::Result<Option<UserDbModel>> {
                Ok((self.user.id == id).then(|| self.user.clone()))
            }

            async fn find_by_username(
                &self,
                _username: &str,
            ) -> crate::Result<Option<UserDbModel>> {
                Ok(None)
            }

            async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
                Ok(None)
            }

            async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
                Ok(())
            }

            async fn delete(&self, _id: &str) -> crate::Result<()> {
                Ok(())
            }

            async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
                Ok(Vec::new())
            }

            async fn update_last_login(&self, _id: &str, _time_ms: i64) -> crate::Result<()> {
                Ok(())
            }

            async fn update_password(
                &self,
                _id: &str,
                _password_hash: &str,
                _clear_must_change: bool,
            ) -> crate::Result<()> {
                Ok(())
            }

            async fn count(&self) -> crate::Result<i64> {
                Ok(0)
            }
        }

        struct NoopRefreshTokenRepository;

        #[async_trait::async_trait]
        impl RefreshTokenRepository for NoopRefreshTokenRepository {
            async fn create(&self, _token: &RefreshTokenDbModel) -> crate::Result<()> {
                Ok(())
            }

            async fn find_by_token_hash(
                &self,
                _token_hash: &str,
            ) -> crate::Result<Option<RefreshTokenDbModel>> {
                Ok(None)
            }

            async fn find_active_by_user(
                &self,
                _user_id: &str,
            ) -> crate::Result<Vec<RefreshTokenDbModel>> {
                Ok(Vec::new())
            }

            async fn revoke(&self, _id: &str) -> crate::Result<()> {
                Ok(())
            }

            async fn revoke_all_for_user(&self, _user_id: &str) -> crate::Result<()> {
                Ok(())
            }

            async fn count_active_by_user(&self, _user_id: &str) -> crate::Result<i64> {
                Ok(0)
            }
        }

        #[derive(Clone)]
        struct TestState {
            auth_service: Option<Arc<AuthService>>,
        }

        impl FromRef<TestState> for AuthRouteState {
            fn from_ref(state: &TestState) -> Self {
                Self {
                    auth_service: state.auth_service.clone(),
                }
            }
        }

        /// Build the auth routers with the same nest shape as
        /// `routes::create_router`: `public_router` and
        /// `password_remediation_router` (which attaches its own layer) nested
        /// on the main chain, `protected_router` merged in behind
        /// `JwtAuthLayer::new`. Constructing this also proves the three
        /// routers register disjoint paths (axum panics on overlap). Returns
        /// the app plus an access token for a user whose
        /// `must_change_password` flag is set (`UserDbModel::new` leaves it
        /// set).
        fn forced_change_app() -> (Router, String) {
            let password_hash =
                AuthService::hash_password(CURRENT_PASSWORD).expect("hashing should succeed");
            let user = UserDbModel::new("forced-user", password_hash, vec!["user".to_string()]);
            let user_id = user.id.clone();

            let jwt_service = Arc::new(JwtService::new(
                "test-secret-key-32-chars-long!!",
                "test-issuer",
                "test-audience",
                Some(3600),
            ));
            let auth_service = Arc::new(AuthService::new(
                Arc::new(ForcedChangeUserRepository { user }),
                Arc::new(NoopRefreshTokenRepository),
                jwt_service.clone(),
                AuthConfig::default(),
            ));
            let token = jwt_service
                .generate_token(&user_id, vec!["user".to_string()])
                .expect("token generation should succeed");

            let protected: Router<TestState> = Router::new()
                .nest("/api/auth", protected_router())
                .layer(JwtAuthLayer::new(auth_service.clone()));
            let app = Router::new()
                .nest("/api/auth", public_router())
                .nest(
                    "/api/auth",
                    password_remediation_router(Some(&auth_service)),
                )
                .merge(protected)
                .with_state(TestState {
                    auth_service: Some(auth_service),
                });
            (app, token)
        }

        async fn send(app: &Router, request: Request<Body>) -> axum::response::Response {
            app.clone()
                .oneshot(request)
                .await
                .expect("router call should be infallible")
        }

        #[tokio::test]
        async fn forced_change_user_reaches_both_remediation_routes() {
            let (app, token) = forced_change_app();

            let logout_all = send(
                &app,
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/logout-all")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("test request should build"),
            )
            .await;
            assert_eq!(logout_all.status(), StatusCode::OK);

            let body = serde_json::json!({
                "current_password": CURRENT_PASSWORD,
                "new_password": "brand-new-pass-2",
            });
            let change_password = send(
                &app,
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/change-password")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("test request should build"),
            )
            .await;
            assert_eq!(change_password.status(), StatusCode::OK);
        }

        #[tokio::test]
        async fn remediation_routes_still_require_a_valid_token() {
            let (app, _token) = forced_change_app();

            for uri in ["/api/auth/logout-all", "/api/auth/change-password"] {
                let response = send(
                    &app,
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .body(Body::empty())
                        .expect("test request should build"),
                )
                .await;
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "uri: {uri}");
            }
        }

        #[tokio::test]
        async fn forced_change_user_is_denied_on_protected_auth_routes() {
            let (app, token) = forced_change_app();

            let response = send(
                &app,
                Request::builder()
                    .method("GET")
                    .uri("/api/auth/sessions")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .expect("test request should build"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);

            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("error body should be readable");
            let body: serde_json::Value =
                serde_json::from_slice(&body).expect("error body should be JSON");
            assert_eq!(body["code"], "PASSWORD_CHANGE_REQUIRED");
        }
    }
}
