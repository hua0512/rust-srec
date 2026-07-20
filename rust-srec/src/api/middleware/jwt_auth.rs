//! JWT authentication middleware.
//!
//! Provides JWT token-based authentication for protected endpoints.

use std::sync::Arc;

use axum::{
    http::{Request, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};

use crate::api::auth_service::AuthService;
use crate::api::error::ApiError;

/// JWT authentication error response.
#[derive(Debug)]
pub enum JwtAuthError {
    /// Missing Authorization header
    MissingToken,
    /// Invalid token format (not Bearer)
    InvalidFormat,
}

impl IntoResponse for JwtAuthError {
    fn into_response(self) -> Response {
        let message = match self {
            JwtAuthError::MissingToken => "Missing authorization token",
            JwtAuthError::InvalidFormat => "Invalid token format",
        };
        ApiError::unauthorized(message).into_response()
    }
}

fn extract_bearer_token<B>(request: &Request<B>) -> Result<&str, JwtAuthError> {
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .ok_or(JwtAuthError::MissingToken)?;

    let auth_str = auth_header
        .to_str()
        .map_err(|_| JwtAuthError::InvalidFormat)?;

    auth_str
        .strip_prefix("Bearer ")
        .ok_or(JwtAuthError::InvalidFormat)
}

/// JWT authentication layer for use with axum's layer system.
#[derive(Clone)]
pub struct JwtAuthLayer {
    auth_service: Arc<AuthService>,
}

impl JwtAuthLayer {
    /// Create a new JWT auth layer.
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self { auth_service }
    }
}

impl<S> tower::Layer<S> for JwtAuthLayer {
    type Service = JwtAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JwtAuthService {
            inner,
            auth_service: self.auth_service.clone(),
        }
    }
}

/// JWT authentication service.
#[derive(Clone)]
pub struct JwtAuthService<S> {
    inner: S,
    auth_service: Arc<AuthService>,
}

impl<S, B> tower::Service<axum::http::Request<B>> for JwtAuthService<S>
where
    S: tower::Service<axum::http::Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: axum::http::Request<B>) -> Self::Future {
        let auth_service = self.auth_service.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            let token = match extract_bearer_token(&request) {
                Ok(token) => token,
                Err(error) => return Ok(error.into_response()),
            };

            let allow_password_remediation = matches!(
                request.uri().path(),
                "/api/auth/change-password" | "/api/auth/logout-all"
            );
            let claims = match auth_service
                .authorize_access_token(token, allow_password_remediation)
                .await
            {
                Ok(claims) => claims,
                Err(error) => return Ok(ApiError::from(error).into_response()),
            };

            let (mut parts, body) = request.into_parts();
            parts.extensions.insert(claims);
            let request = axum::http::Request::from_parts(parts, body);

            inner.call(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::body::{Body, to_bytes};
    use axum::http::StatusCode;
    use tower::{Layer, ServiceExt, service_fn};

    use super::*;
    use crate::api::auth_service::{AuthConfig, AuthService};
    use crate::api::jwt::{Claims, JwtService};
    use crate::database::models::{RefreshTokenDbModel, UserDbModel};
    use crate::database::repositories::{RefreshTokenRepository, UserRepository};

    #[derive(Clone)]
    enum UserLookup {
        Found(UserDbModel),
        Missing,
    }

    struct TestUserRepository {
        lookup: UserLookup,
    }

    #[async_trait::async_trait]
    impl UserRepository for TestUserRepository {
        async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
            Ok(())
        }

        async fn find_by_id(&self, id: &str) -> crate::Result<Option<UserDbModel>> {
            match &self.lookup {
                UserLookup::Found(user) if user.id == id => Ok(Some(user.clone())),
                UserLookup::Found(_) | UserLookup::Missing => Ok(None),
            }
        }

        async fn find_by_username(&self, _username: &str) -> crate::Result<Option<UserDbModel>> {
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

    struct TestRefreshTokenRepository;

    #[async_trait::async_trait]
    impl RefreshTokenRepository for TestRefreshTokenRepository {
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

    fn test_services(lookup: UserLookup) -> (Arc<AuthService>, Arc<JwtService>) {
        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        ));
        let auth_service = Arc::new(AuthService::new(
            Arc::new(TestUserRepository { lookup }),
            Arc::new(TestRefreshTokenRepository),
            jwt_service.clone(),
            AuthConfig::default(),
        ));
        (auth_service, jwt_service)
    }

    async fn call_layer(
        auth_service: Arc<AuthService>,
        path: &str,
        authorization: Option<&str>,
    ) -> Response {
        let inner = service_fn(|request: Request<Body>| async move {
            let claims = request
                .extensions()
                .get::<Claims>()
                .expect("authorized requests should contain claims");
            Ok::<_, Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("x-user-id", claims.sub.as_str())
                    .body(Body::empty())
                    .expect("test response should build"),
            )
        });
        let service = JwtAuthLayer::new(auth_service).layer(inner);
        let mut request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("test request should build");
        if let Some(value) = authorization {
            request.headers_mut().insert(
                AUTHORIZATION,
                value.parse().expect("authorization header should be valid"),
            );
        }

        service
            .oneshot(request)
            .await
            .expect("authentication layer should be infallible")
    }

    fn active_user() -> UserDbModel {
        let mut user = UserDbModel::new("test-user", "hash", vec!["user".to_string()]);
        user.must_change_password = false;
        user
    }

    #[tokio::test]
    async fn missing_and_invalid_tokens_are_unauthorized() {
        let user = active_user();
        let (auth_service, _) = test_services(UserLookup::Found(user));

        let missing = call_layer(auth_service.clone(), "/api/config", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let invalid = call_layer(auth_service, "/api/config", Some("Bearer invalid")).await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_active_user_reaches_handler_with_claims() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, jwt_service) = test_services(UserLookup::Found(user));
        let token = jwt_service
            .generate_token(&user_id, vec!["user".to_string()])
            .expect("token generation should succeed");

        let response = call_layer(
            auth_service,
            "/api/config",
            Some(&format!("Bearer {token}")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-user-id"], user_id);
    }

    #[tokio::test]
    async fn forced_password_change_is_denied_on_normal_routes() {
        let user = UserDbModel::new("test-user", "hash", vec!["user".to_string()]);
        let user_id = user.id.clone();
        let (auth_service, jwt_service) = test_services(UserLookup::Found(user));
        let token = jwt_service
            .generate_token(&user_id, vec!["user".to_string()])
            .expect("token generation should succeed");

        let response = call_layer(
            auth_service,
            "/api/config",
            Some(&format!("Bearer {token}")),
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

    #[tokio::test]
    async fn forced_password_change_can_reach_only_remediation_routes() {
        for path in ["/api/auth/change-password", "/api/auth/logout-all"] {
            let user = UserDbModel::new("test-user", "hash", vec!["user".to_string()]);
            let user_id = user.id.clone();
            let (auth_service, jwt_service) = test_services(UserLookup::Found(user));
            let token = jwt_service
                .generate_token(&user_id, vec!["user".to_string()])
                .expect("token generation should succeed");

            let response = call_layer(auth_service, path, Some(&format!("Bearer {token}"))).await;
            assert_eq!(response.status(), StatusCode::OK, "path: {path}");
        }
    }

    #[tokio::test]
    async fn token_for_missing_user_fails_closed() {
        let (auth_service, jwt_service) = test_services(UserLookup::Missing);
        let token = jwt_service
            .generate_token("deleted-user", vec!["user".to_string()])
            .expect("token generation should succeed");

        let response = call_layer(
            auth_service,
            "/api/config",
            Some(&format!("Bearer {token}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
