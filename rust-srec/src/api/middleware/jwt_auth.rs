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
    /// Forwarded to `AuthService::authorize_access_token`. Routes wrapped by
    /// a `password_remediation` layer stay reachable for users whose
    /// `must_change_password` flag is set; token validation itself is
    /// unaffected.
    allow_password_remediation: bool,
}

impl JwtAuthLayer {
    /// Create a JWT auth layer that also enforces the forced-password-change
    /// state (`AuthError::PasswordChangeRequired` for flagged users).
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: false,
        }
    }

    /// Create a JWT auth layer for the password-remediation routes
    /// (`routes::auth::password_remediation_router`): tokens are validated as
    /// usual, but a user with `must_change_password` set is let through so
    /// they can actually remediate.
    pub fn password_remediation(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: true,
        }
    }
}

impl<S> tower::Layer<S> for JwtAuthLayer {
    type Service = JwtAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JwtAuthService {
            inner,
            auth_service: self.auth_service.clone(),
            allow_password_remediation: self.allow_password_remediation,
        }
    }
}

/// JWT authentication service.
#[derive(Clone)]
pub struct JwtAuthService<S> {
    inner: S,
    auth_service: Arc<AuthService>,
    allow_password_remediation: bool,
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

    fn call(&mut self, mut request: axum::http::Request<B>) -> Self::Future {
        let auth_service = self.auth_service.clone();
        let allow_password_remediation = self.allow_password_remediation;
        // The future must capture the instance `poll_ready` was called on;
        // leave the fresh clone in `self.inner` for the next `call` (the
        // standard tower pattern for cloning a service into a future).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let token = match extract_bearer_token(&request) {
                Ok(token) => token,
                Err(error) => return Ok(error.into_response()),
            };

            let claims = match auth_service
                .authorize_access_token(token, allow_password_remediation)
                .await
            {
                Ok(claims) => claims,
                Err(error) => return Ok(ApiError::from(error).into_response()),
            };

            request.extensions_mut().insert(claims);

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

    async fn call_layer(layer: JwtAuthLayer, path: &str, authorization: Option<&str>) -> Response {
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
        let service = layer.layer(inner);
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

        let missing =
            call_layer(JwtAuthLayer::new(auth_service.clone()), "/api/config", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let invalid = call_layer(
            JwtAuthLayer::new(auth_service),
            "/api/config",
            Some("Bearer invalid"),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn non_bearer_scheme_and_empty_bearer_are_unauthorized() {
        let user = active_user();
        let (auth_service, _) = test_services(UserLookup::Found(user));

        let basic = call_layer(
            JwtAuthLayer::new(auth_service.clone()),
            "/api/config",
            Some("Basic xyz"),
        )
        .await;
        assert_eq!(basic.status(), StatusCode::UNAUTHORIZED);

        let empty_bearer = call_layer(
            JwtAuthLayer::new(auth_service),
            "/api/config",
            Some("Bearer "),
        )
        .await;
        assert_eq!(empty_bearer.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn expired_and_tampered_tokens_are_unauthorized() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, jwt_service) = test_services(UserLookup::Found(user));

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should be past the Unix epoch")
            .as_secs();
        // An hour-old exp clears the 60s leeway Validation::default() applies
        // inside JwtService::validate_token.
        let expired_claims = Claims {
            sub: user_id.clone(),
            roles: vec!["user".to_string()],
            iss: "test-issuer".to_string(),
            aud: "test-audience".to_string(),
            exp: now - 3600,
            iat: now - 7200,
        };
        let expired = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &expired_claims,
            &jsonwebtoken::EncodingKey::from_secret("test-secret-key-32-chars-long!!".as_bytes()),
        )
        .expect("token encoding should succeed");
        let response = call_layer(
            JwtAuthLayer::new(auth_service.clone()),
            "/api/config",
            Some(&format!("Bearer {expired}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let token = jwt_service
            .generate_token(&user_id, vec!["user".to_string()])
            .expect("token generation should succeed");
        // Flip one mid-signature character (still base64url) so validation
        // fails on the signature check rather than on decoding.
        let (head, signature) = token
            .rsplit_once('.')
            .expect("JWT should have a signature segment");
        let mut signature_bytes = signature.as_bytes().to_vec();
        let mid = signature_bytes.len() / 2;
        signature_bytes[mid] = if signature_bytes[mid] == b'A' {
            b'B'
        } else {
            b'A'
        };
        let tampered = format!(
            "{head}.{}",
            String::from_utf8(signature_bytes).expect("signature should remain ASCII")
        );
        let response = call_layer(
            JwtAuthLayer::new(auth_service),
            "/api/config",
            Some(&format!("Bearer {tampered}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
            JwtAuthLayer::new(auth_service),
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
            JwtAuthLayer::new(auth_service),
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
    async fn forced_password_change_passes_password_remediation_layer() {
        // Which routes actually carry the `password_remediation` layer is
        // pinned by the drift-guard tests in `routes::auth`; this test only
        // covers the layer behavior for a `must_change_password` user.
        let user = UserDbModel::new("test-user", "hash", vec!["user".to_string()]);
        let user_id = user.id.clone();
        let (auth_service, jwt_service) = test_services(UserLookup::Found(user));
        let token = jwt_service
            .generate_token(&user_id, vec!["user".to_string()])
            .expect("token generation should succeed");

        let response = call_layer(
            JwtAuthLayer::password_remediation(auth_service),
            "/api/auth/change-password",
            Some(&format!("Bearer {token}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn token_for_missing_user_fails_closed() {
        let (auth_service, jwt_service) = test_services(UserLookup::Missing);
        let token = jwt_service
            .generate_token("deleted-user", vec!["user".to_string()])
            .expect("token generation should succeed");

        let response = call_layer(
            JwtAuthLayer::new(auth_service),
            "/api/config",
            Some(&format!("Bearer {token}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
