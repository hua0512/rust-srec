//! Request authentication middleware.
//!
//! Validates JWT access tokens and API keys for protected endpoints. On
//! success both the `Claims` (read by existing handlers) and the full
//! `AuthPrincipal` (credential kind + access scope) are inserted into the
//! request extensions.

use std::sync::Arc;

use axum::{
    http::{Method, Request, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
};

use crate::api::auth_service::{API_KEY_PREFIX, AuthService, CredentialKind};
use crate::api::error::ApiError;
use crate::database::models::ApiKeyAccessLevel;

/// Authentication error response.
#[derive(Debug)]
pub enum AuthLayerError {
    /// Missing Authorization header
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

fn extract_credential<B>(request: &Request<B>) -> Result<&str, AuthLayerError> {
    if !request.headers().contains_key(AUTHORIZATION) {
        return Err(AuthLayerError::MissingToken);
    }
    crate::api::auth_request::request_credential(request.headers(), None)
        .map_err(|_| AuthLayerError::InvalidFormat)
}
fn has_path_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// REST requests available to read-only API keys. This is intentionally an
/// allowlist: configuration and operational records can contain cookies,
/// credentials, command arguments, or private endpoints even when exposed by
/// a safe HTTP method. MCP applies the equivalent rule per tool.
fn is_read_only_request_allowed(method: &Method, path: &str) -> bool {
    if *method == Method::OPTIONS {
        return true;
    }
    if !matches!(*method, Method::GET | Method::HEAD) {
        return false;
    }

    has_path_prefix(path, "/api/sessions")
        || path == "/api/pipeline/stats"
        || path == "/api/notifications/event-types"
        || path == "/api/notifications/events"
}

fn read_only_key_response() -> Response {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "API_KEY_READ_ONLY",
        "This endpoint requires a full-access API key",
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
    /// When set, read-only API keys are limited to the explicit REST query
    /// allowlist. Disabled by `AuthLayer::mcp` because MCP transports POST for
    /// every call and scope is enforced per tool instead.
    enforce_read_only_access: bool,
}

impl AuthLayer {
    /// Create an auth layer that also enforces the forced-password-change
    /// state (`AuthError::PasswordChangeRequired` for flagged users).
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: false,
            enforce_read_only_access: true,
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
            enforce_read_only_access: true,
        }
    }

    /// Create an auth layer for the MCP endpoint: identical credential
    /// validation, but read-only scope is enforced by the MCP tools
    /// themselves (`mcp::require_full_access`) rather than by HTTP method.
    pub fn mcp(auth_service: Arc<AuthService>) -> Self {
        Self {
            auth_service,
            allow_password_remediation: false,
            enforce_read_only_access: false,
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
            enforce_read_only_access: self.enforce_read_only_access,
        }
    }
}

/// Authentication middleware service.
#[derive(Clone)]
pub struct AuthMiddleware<S> {
    inner: S,
    auth_service: Arc<AuthService>,
    allow_password_remediation: bool,
    enforce_read_only_access: bool,
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
        let enforce_read_only_access = self.enforce_read_only_access;
        // The future must capture the instance `poll_ready` was called on;
        // leave the fresh clone in `self.inner` for the next `call` (the
        // standard tower pattern for cloning a service into a future).
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            let token = match extract_credential(&request) {
                Ok(token) => token,
                Err(error) => return Ok(error.into_response()),
            };
            if allow_password_remediation && token.trim().starts_with(API_KEY_PREFIX) {
                return Ok(api_key_not_allowed_response());
            }
            let principal = match auth_service
                .authorize_credential(token, allow_password_remediation)
                .await
            {
                Ok(principal) => principal,
                Err(error) => return Ok(ApiError::from(error).into_response()),
            };
            if allow_password_remediation && principal.credential == CredentialKind::ApiKey {
                return Ok(api_key_not_allowed_response());
            }
            if enforce_read_only_access
                && principal.access == ApiKeyAccessLevel::ReadOnly
                && !is_read_only_request_allowed(request.method(), request.uri().path())
            {
                return Ok(read_only_key_response());
            }

            request.extensions_mut().insert(principal.claims.clone());
            request.extensions_mut().insert(principal);

            inner.call(request).await
        })
    }
}
