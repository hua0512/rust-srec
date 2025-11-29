//! JWT authentication middleware.
//!
//! Provides JWT token-based authentication for protected endpoints.

use axum::{
    body::Body,
    extract::Request,
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::api::jwt::{Claims, JwtError, JwtService};

/// JWT authentication error response.
#[derive(Debug)]
pub enum JwtAuthError {
    /// Missing Authorization header
    MissingToken,
    /// Invalid token format (not Bearer)
    InvalidFormat,
    /// Token validation failed
    InvalidToken(JwtError),
}

impl IntoResponse for JwtAuthError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            JwtAuthError::MissingToken => (StatusCode::UNAUTHORIZED, "Missing authorization token"),
            JwtAuthError::InvalidFormat => (StatusCode::UNAUTHORIZED, "Invalid token format"),
            JwtAuthError::InvalidToken(JwtError::TokenExpired) => {
                (StatusCode::UNAUTHORIZED, "Token has expired")
            }
            JwtAuthError::InvalidToken(_) => (StatusCode::UNAUTHORIZED, "Invalid token"),
        };

        Response::builder()
            .status(status)
            .body(Body::from(message))
            .unwrap()
    }
}

/// Extract Bearer token from Authorization header.
fn extract_bearer_token(request: &Request) -> Result<&str, JwtAuthError> {
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .ok_or(JwtAuthError::MissingToken)?;

    let auth_str = auth_header
        .to_str()
        .map_err(|_| JwtAuthError::InvalidFormat)?;

    if !auth_str.starts_with("Bearer ") {
        return Err(JwtAuthError::InvalidFormat);
    }

    Ok(&auth_str[7..])
}

/// JWT authentication middleware.
///
/// Extracts the Bearer token from the Authorization header, validates it,
/// and injects the claims into request extensions.
pub async fn jwt_auth_middleware(
    jwt_service: Arc<JwtService>,
    mut request: Request,
    next: Next,
) -> Result<Response, JwtAuthError> {
    let token = extract_bearer_token(&request)?;

    let claims = jwt_service
        .validate_token(token)
        .map_err(JwtAuthError::InvalidToken)?;

    // Inject claims into request extensions for downstream handlers
    request.extensions_mut().insert(claims);

    Ok(next.run(request).await)
}

/// Extract claims from request extensions.
///
/// Use this in handlers to access the authenticated user's claims.
pub fn extract_claims(request: &Request) -> Option<&Claims> {
    request.extensions().get::<Claims>()
}

/// JWT authentication layer for use with axum's layer system.
#[derive(Clone)]
pub struct JwtAuthLayer {
    jwt_service: Arc<JwtService>,
}

impl JwtAuthLayer {
    /// Create a new JWT auth layer.
    pub fn new(jwt_service: Arc<JwtService>) -> Self {
        Self { jwt_service }
    }
}

impl<S> tower::Layer<S> for JwtAuthLayer {
    type Service = JwtAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JwtAuthService {
            inner,
            jwt_service: self.jwt_service.clone(),
        }
    }
}

/// JWT authentication service.
#[derive(Clone)]
pub struct JwtAuthService<S> {
    inner: S,
    jwt_service: Arc<JwtService>,
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
        let jwt_service = self.jwt_service.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Extract token from Authorization header
            let auth_header = request.headers().get(AUTHORIZATION);

            let token = match auth_header {
                Some(header) => {
                    let header_str = match header.to_str() {
                        Ok(s) => s,
                        Err(_) => {
                            return Ok(JwtAuthError::InvalidFormat.into_response());
                        }
                    };

                    if !header_str.starts_with("Bearer ") {
                        return Ok(JwtAuthError::InvalidFormat.into_response());
                    }

                    &header_str[7..]
                }
                None => {
                    return Ok(JwtAuthError::MissingToken.into_response());
                }
            };

            // Validate token
            let claims = match jwt_service.validate_token(token) {
                Ok(claims) => claims,
                Err(e) => {
                    return Ok(JwtAuthError::InvalidToken(e).into_response());
                }
            };

            // Convert request to inject claims
            let (mut parts, body) = request.into_parts();
            parts.extensions.insert(claims);
            let request = axum::http::Request::from_parts(parts, body);

            inner.call(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn create_test_service() -> Arc<JwtService> {
        Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        ))
    }

    #[test]
    fn test_extract_bearer_token_valid() {
        let request = Request::builder()
            .header(AUTHORIZATION, "Bearer valid_token_here")
            .body(Body::empty())
            .unwrap();

        let result = extract_bearer_token(&request);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "valid_token_here");
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let request = Request::builder().body(Body::empty()).unwrap();

        let result = extract_bearer_token(&request);
        assert!(matches!(result, Err(JwtAuthError::MissingToken)));
    }

    #[test]
    fn test_extract_bearer_token_invalid_format() {
        let request = Request::builder()
            .header(AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(Body::empty())
            .unwrap();

        let result = extract_bearer_token(&request);
        assert!(matches!(result, Err(JwtAuthError::InvalidFormat)));
    }

    #[test]
    fn test_jwt_auth_layer_creation() {
        let jwt_service = create_test_service();
        let layer = JwtAuthLayer::new(jwt_service);

        // Just verify it can be created
        assert!(Arc::strong_count(&layer.jwt_service) >= 1);
    }
}


#[cfg(test)]
mod property_tests {
    use super::*;
    use crate::api::jwt::JwtService;
    use axum::http::Request;
    use proptest::prelude::*;

    fn create_test_service() -> Arc<JwtService> {
        Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        ))
    }

    /// Create an expired token directly using jsonwebtoken
    fn create_expired_token(user_id: &str, roles: Vec<String>) -> String {
        use jsonwebtoken::{encode, EncodingKey, Header};
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Create claims with expiration 2 minutes in the past (beyond default leeway)
        let claims = Claims {
            sub: user_id.to_string(),
            roles,
            iss: "test-issuer".to_string(),
            aud: "test-audience".to_string(),
            exp: now.saturating_sub(120), // 2 minutes ago
            iat: now.saturating_sub(180), // 3 minutes ago
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(b"test-secret-key-32-chars-long!!"),
        )
        .expect("Token encoding should succeed")
    }

    // **Feature: jwt-auth-and-api-implementation, Property 2: Valid JWT Token Authentication**
    // **Validates: Requirements 1.2**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_valid_jwt_token_authentication(
            user_id in "[a-zA-Z0-9_-]{1,50}",
            roles in prop::collection::vec("[a-zA-Z0-9_]{1,20}", 0..5),
        ) {
            let jwt_service = create_test_service();

            // Generate a valid token
            let token = jwt_service
                .generate_token(&user_id, roles.clone())
                .expect("Token generation should succeed");

            // Create request with valid Bearer token
            let request = Request::builder()
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap();

            // Extract and validate token
            let extracted_token = extract_bearer_token(&request)
                .expect("Token extraction should succeed");

            let claims = jwt_service
                .validate_token(extracted_token)
                .expect("Token validation should succeed");

            // Property: Valid tokens should authenticate and extract correct claims
            prop_assert_eq!(&claims.sub, &user_id, "User ID should match");
            prop_assert_eq!(&claims.roles, &roles, "Roles should match");
        }
    }

    // **Feature: jwt-auth-and-api-implementation, Property 3: Expired JWT Token Rejection**
    // **Validates: Requirements 1.3**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_expired_jwt_token_rejection(
            user_id in "[a-zA-Z0-9_-]{1,50}",
            roles in prop::collection::vec("[a-zA-Z0-9_]{1,20}", 0..5),
        ) {
            let jwt_service = create_test_service();

            // Create a token that is already expired (2 minutes in the past)
            let token = create_expired_token(&user_id, roles);

            // Create request with expired token
            let request = Request::builder()
                .header(AUTHORIZATION, format!("Bearer {}", token))
                .body(Body::empty())
                .unwrap();

            let extracted_token = extract_bearer_token(&request)
                .expect("Token extraction should succeed");

            // Property: Expired tokens should be rejected
            let result = jwt_service.validate_token(extracted_token);
            prop_assert!(
                matches!(result, Err(crate::api::jwt::JwtError::TokenExpired)),
                "Expired tokens should be rejected with TokenExpired error"
            );
        }
    }

    // **Feature: jwt-auth-and-api-implementation, Property 4: Invalid JWT Token Rejection**
    // **Validates: Requirements 1.4**
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_invalid_jwt_token_rejection(
            // Generate random strings that are NOT valid JWT tokens
            invalid_token in "[a-zA-Z0-9]{10,100}",
        ) {
            let jwt_service = create_test_service();

            // Create request with invalid token
            let request = Request::builder()
                .header(AUTHORIZATION, format!("Bearer {}", invalid_token))
                .body(Body::empty())
                .unwrap();

            let extracted_token = extract_bearer_token(&request)
                .expect("Token extraction should succeed");

            // Property: Invalid tokens should be rejected
            let result = jwt_service.validate_token(extracted_token);
            prop_assert!(
                result.is_err(),
                "Invalid tokens should be rejected"
            );
        }

        #[test]
        fn prop_tampered_jwt_token_rejection(
            user_id in "[a-zA-Z0-9_-]{1,50}",
            roles in prop::collection::vec("[a-zA-Z0-9_]{1,20}", 0..5),
            tamper_char in prop::sample::select(vec!['X', 'Y', 'Z', '0', '1', '2']),
            tamper_pos in 10usize..50usize,
        ) {
            let jwt_service = create_test_service();

            // Generate a valid token
            let token = jwt_service
                .generate_token(&user_id, roles)
                .expect("Token generation should succeed");

            // Tamper with the token (modify a character in the middle)
            let mut tampered_token: Vec<char> = token.chars().collect();
            if tamper_pos < tampered_token.len() {
                tampered_token[tamper_pos] = tamper_char;
            }
            let tampered_token: String = tampered_token.into_iter().collect();

            // Only test if we actually changed the token
            if tampered_token != token {
                // Property: Tampered tokens should be rejected
                let result = jwt_service.validate_token(&tampered_token);
                prop_assert!(
                    result.is_err(),
                    "Tampered tokens should be rejected"
                );
            }
        }
    }
}
