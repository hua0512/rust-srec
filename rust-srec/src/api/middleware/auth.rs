//! Request authentication middleware.
//!
//! Validates JWT access tokens and API keys for protected endpoints. On
//! success both the `Claims` (read by existing handlers) and the full
//! `AuthPrincipal` (credential kind + access scope) are inserted into the
//! request extensions.

use std::sync::Arc;

use axum::{
    http::{HeaderName, Method, Request, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};

use crate::api::auth_service::{API_KEY_PREFIX, AuthPrincipal, AuthService, CredentialKind};
use crate::api::error::ApiError;
use crate::database::models::ApiKeyAccessLevel;

/// Alternative header carrying an API key for clients that cannot set
/// `Authorization: Bearer` (both are accepted; `Authorization` wins).
const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// Authentication error response.
#[derive(Debug)]
pub enum AuthLayerError {
    /// Missing Authorization / X-Api-Key header
    MissingToken,
    /// Invalid credential format (not Bearer / non-ASCII header)
    InvalidFormat,
}

impl IntoResponse for AuthLayerError {
    fn into_response(self) -> Response {
        let message = match self {
            AuthLayerError::MissingToken => "Missing authorization token",
            AuthLayerError::InvalidFormat => "Invalid token format",
        };
        ApiError::unauthorized(message).into_response()
    }
}

/// The raw credential a request presented, before validation.
enum RequestCredential<'a> {
    Jwt(&'a str),
    ApiKey(&'a str),
}

fn extract_credential<B>(request: &Request<B>) -> Result<RequestCredential<'_>, AuthLayerError> {
    if let Some(auth_header) = request.headers().get(AUTHORIZATION) {
        let auth_str = auth_header
            .to_str()
            .map_err(|_| AuthLayerError::InvalidFormat)?;
        let token = auth_str
            .strip_prefix("Bearer ")
            .ok_or(AuthLayerError::InvalidFormat)?;
        // API keys are routed by their `srec_` prefix (`API_KEY_PREFIX`);
        // anything else is treated as a JWT access token.
        return Ok(if token.starts_with(API_KEY_PREFIX) {
            RequestCredential::ApiKey(token)
        } else {
            RequestCredential::Jwt(token)
        });
    }

    if let Some(key_header) = request.headers().get(&X_API_KEY) {
        let key = key_header
            .to_str()
            .map_err(|_| AuthLayerError::InvalidFormat)?;
        return Ok(RequestCredential::ApiKey(key));
    }

    Err(AuthLayerError::MissingToken)
}

/// Methods a read-only API key may use on REST routes. MCP requests are all
/// POST, so the MCP layer (`AuthLayer::mcp`) disables this check and relies
/// on `mcp::require_write` inside each tool instead.
fn is_safe_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn read_only_key_response() -> Response {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "API_KEY_READ_ONLY",
        "This API key is read-only and cannot perform write operations",
    )
    .into_response()
}

fn api_key_not_allowed_response() -> Response {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "API_KEY_NOT_ALLOWED",
        "API keys cannot access this endpoint",
    )
    .into_response()
}

/// Authentication layer for use with axum's layer system.
#[derive(Clone)]
pub struct AuthLayer {
    auth_service: Arc<AuthService>,
    /// Forwarded to `AuthService::authorize_access_token`. Routes wrapped by
    /// a `password_remediation` layer stay reachable for users whose
    /// `must_change_password` flag is set; token validation itself is
    /// unaffected. API keys are rejected outright on these routes.
    allow_password_remediation: bool,
    /// When set, read-only API keys are limited to `is_safe_method`
    /// requests. Disabled by `AuthLayer::mcp` because MCP transports POST
    /// for every call and scope is enforced per tool instead.
    enforce_read_only_methods: bool,
}

impl AuthLayer {
    /// Create an auth layer that also enforces the forced-password-change
    /// state (`AuthError::PasswordChangeRequired` for flagged users).
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: false,
            enforce_read_only_methods: true,
        }
    }

    /// Create an auth layer for the password-remediation routes
    /// (`routes::auth::password_remediation_router`): tokens are validated as
    /// usual, but a user with `must_change_password` set is let through so
    /// they can actually remediate. API keys are rejected on these routes.
    pub fn password_remediation(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: true,
            enforce_read_only_methods: true,
        }
    }

    /// Create an auth layer for the MCP endpoint: identical credential
    /// validation, but read-only scope is enforced by the MCP tools
    /// themselves (`mcp::require_write`) rather than by HTTP method.
    pub fn mcp(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: false,
            enforce_read_only_methods: false,
        }
    }
}

impl<S> tower::Layer<S> for AuthLayer {
    type Service = AuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthMiddleware {
            inner,
            auth_service: self.auth_service.clone(),
            allow_password_remediation: self.allow_password_remediation,
            enforce_read_only_methods: self.enforce_read_only_methods,
        }
    }
}

/// Authentication middleware service.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    auth_service: Arc<AuthService>,
    allow_password_remediation: bool,
    enforce_read_only_methods: bool,
}

impl<S, B> tower::Service<axum::http::Request<B>> for AuthMiddleware<S>
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
        let enforce_read_only_methods = self.enforce_read_only_methods;
        // The future must capture the instance `poll_ready` was called on;
        // leave the fresh clone in `self.inner` for the next `call` (the
        // standard tower pattern for cloning a service into a future).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let principal = match extract_credential(&request) {
                Ok(RequestCredential::Jwt(token)) => {
                    match auth_service
                        .authorize_access_token(token, allow_password_remediation)
                        .await
                    {
                        Ok(claims) => AuthPrincipal {
                            claims,
                            credential: CredentialKind::Jwt,
                            access: ApiKeyAccessLevel::Full,
                        },
                        Err(error) => return Ok(ApiError::from(error).into_response()),
                    }
                }
                Ok(RequestCredential::ApiKey(key)) => {
                    // Remediation routes manage the account itself
                    // (change-password / logout-all); only an interactive
                    // JWT session may reach them.
                    if allow_password_remediation {
                        return Ok(api_key_not_allowed_response());
                    }
                    match auth_service.authorize_api_key(key).await {
                        Ok(principal) => principal,
                        Err(error) => return Ok(ApiError::from(error).into_response()),
                    }
                }
                Err(error) => return Ok(error.into_response()),
            };

            if enforce_read_only_methods
                && principal.access == ApiKeyAccessLevel::ReadOnly
                && !is_safe_method(request.method())
            {
                return Ok(read_only_key_response());
            }

            request.extensions_mut().insert(principal.claims.clone());
            request.extensions_mut().insert(principal);

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
    use crate::database::models::{ApiKeyAccessLevel, RefreshTokenDbModel, UserDbModel};
    use crate::database::repositories::{
        InMemoryApiKeyRepository, RefreshTokenRepository, UserRepository,
    };

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
            Arc::new(InMemoryApiKeyRepository::default()),
            jwt_service.clone(),
            AuthConfig::default(),
        ));
        (auth_service, jwt_service)
    }

    async fn call_layer(layer: AuthLayer, path: &str, authorization: Option<&str>) -> Response {
        call_layer_with(layer, Method::GET, path, authorization, None).await
    }

    async fn call_layer_with(
        layer: AuthLayer,
        method: Method,
        path: &str,
        authorization: Option<&str>,
        x_api_key: Option<&str>,
    ) -> Response {
        let inner = service_fn(|request: Request<Body>| async move {
            let claims = request
                .extensions()
                .get::<Claims>()
                .expect("authorized requests should contain claims");
            let principal = request
                .extensions()
                .get::<AuthPrincipal>()
                .expect("authorized requests should contain the principal");
            assert_eq!(principal.claims, *claims);
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
            .method(method)
            .uri(path)
            .body(Body::empty())
            .expect("test request should build");
        if let Some(value) = authorization {
            request.headers_mut().insert(
                AUTHORIZATION,
                value.parse().expect("authorization header should be valid"),
            );
        }
        if let Some(value) = x_api_key {
            request.headers_mut().insert(
                X_API_KEY,
                value.parse().expect("x-api-key header should be valid"),
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

        let missing = call_layer(AuthLayer::new(auth_service.clone()), "/api/config", None).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let invalid = call_layer(
            AuthLayer::new(auth_service),
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
            AuthLayer::new(auth_service.clone()),
            "/api/config",
            Some("Basic xyz"),
        )
        .await;
        assert_eq!(basic.status(), StatusCode::UNAUTHORIZED);

        let empty_bearer =
            call_layer(AuthLayer::new(auth_service), "/api/config", Some("Bearer ")).await;
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
            AuthLayer::new(auth_service.clone()),
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
            AuthLayer::new(auth_service),
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
            AuthLayer::new(auth_service),
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
            AuthLayer::new(auth_service),
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
            AuthLayer::password_remediation(auth_service),
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
            AuthLayer::new(auth_service),
            "/api/config",
            Some(&format!("Bearer {token}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    async fn create_key(
        auth_service: &Arc<AuthService>,
        user_id: &str,
        access_level: ApiKeyAccessLevel,
        expires_at: Option<i64>,
    ) -> (String, String) {
        let (model, raw_key) = auth_service
            .create_api_key(user_id, "test key", access_level, expires_at)
            .await
            .expect("key creation should succeed");
        (model.id, raw_key)
    }

    #[tokio::test]
    async fn valid_api_key_reaches_handler_via_bearer_and_header() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) = create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, None).await;

        let bearer = call_layer(
            AuthLayer::new(auth_service.clone()),
            "/api/config",
            Some(&format!("Bearer {raw_key}")),
        )
        .await;
        assert_eq!(bearer.status(), StatusCode::OK);
        assert_eq!(bearer.headers()["x-user-id"], user_id);

        let header = call_layer_with(
            AuthLayer::new(auth_service),
            Method::GET,
            "/api/config",
            None,
            Some(&raw_key),
        )
        .await;
        assert_eq!(header.status(), StatusCode::OK);
        assert_eq!(header.headers()["x-user-id"], user_id);
    }

    #[tokio::test]
    async fn unknown_api_key_is_unauthorized() {
        let user = active_user();
        let (auth_service, _) = test_services(UserLookup::Found(user));

        let response = call_layer(
            AuthLayer::new(auth_service),
            "/api/config",
            Some("Bearer srec_0000000000000000000000000000000000000000000000000000000000000000"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn revoked_and_expired_api_keys_are_unauthorized() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));

        let (key_id, raw_key) =
            create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, None).await;
        let revoked = auth_service
            .revoke_api_key(&user_id, &key_id)
            .await
            .expect("revoke should succeed");
        assert!(revoked);
        let response = call_layer(
            AuthLayer::new(auth_service.clone()),
            "/api/config",
            Some(&format!("Bearer {raw_key}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let past = crate::database::time::now_ms() - 1000;
        let (_, expired_key) =
            create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, Some(past)).await;
        let response = call_layer(
            AuthLayer::new(auth_service),
            "/api/config",
            Some(&format!("Bearer {expired_key}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn read_only_api_key_is_limited_to_safe_methods() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) =
            create_key(&auth_service, &user_id, ApiKeyAccessLevel::ReadOnly, None).await;

        let get = call_layer_with(
            AuthLayer::new(auth_service.clone()),
            Method::GET,
            "/api/config",
            Some(&format!("Bearer {raw_key}")),
            None,
        )
        .await;
        assert_eq!(get.status(), StatusCode::OK);

        let post = call_layer_with(
            AuthLayer::new(auth_service),
            Method::POST,
            "/api/config",
            Some(&format!("Bearer {raw_key}")),
            None,
        )
        .await;
        assert_eq!(post.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(post.into_body(), usize::MAX)
            .await
            .expect("error body should be readable");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("error body should be JSON");
        assert_eq!(body["code"], "API_KEY_READ_ONLY");
    }

    #[tokio::test]
    async fn full_api_key_can_write() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) = create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, None).await;

        let post = call_layer_with(
            AuthLayer::new(auth_service),
            Method::POST,
            "/api/streamers",
            Some(&format!("Bearer {raw_key}")),
            None,
        )
        .await;
        assert_eq!(post.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn mcp_layer_lets_read_only_keys_post() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) =
            create_key(&auth_service, &user_id, ApiKeyAccessLevel::ReadOnly, None).await;

        let post = call_layer_with(
            AuthLayer::mcp(auth_service),
            Method::POST,
            "/api/mcp",
            Some(&format!("Bearer {raw_key}")),
            None,
        )
        .await;
        assert_eq!(post.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_keys_are_rejected_on_password_remediation_routes() {
        let user = active_user();
        let user_id = user.id.clone();
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) = create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, None).await;

        let response = call_layer_with(
            AuthLayer::password_remediation(auth_service),
            Method::POST,
            "/api/auth/change-password",
            Some(&format!("Bearer {raw_key}")),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("error body should be readable");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("error body should be JSON");
        assert_eq!(body["code"], "API_KEY_NOT_ALLOWED");
    }

    #[tokio::test]
    async fn api_key_of_disabled_user_is_rejected() {
        let mut user = active_user();
        let user_id = user.id.clone();
        user.is_active = false;
        let (auth_service, _) = test_services(UserLookup::Found(user));
        let (_, raw_key) = create_key(&auth_service, &user_id, ApiKeyAccessLevel::Full, None).await;

        let response = call_layer(
            AuthLayer::new(auth_service),
            "/api/config",
            Some(&format!("Bearer {raw_key}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
