//! Password change enforcement middleware.
//!
//! Middleware that checks if a user must change their password before accessing
//! protected resources. Returns 403 Forbidden if the user has must_change_password=true.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::sync::Arc;

use crate::api::jwt::Claims;
use crate::database::repositories::UserRepository;

/// Password change required error response.
#[derive(Debug)]
pub struct PasswordChangeRequiredError;

impl IntoResponse for PasswordChangeRequiredError {
    fn into_response(self) -> Response {
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": "password_change_required",
                "message": "You must change your password before accessing this resource"
            })),
        )
            .into_response()
    }
}

/// Password change enforcement layer for use with axum's layer system.
#[derive(Clone)]
pub struct PasswordChangeLayer<R: UserRepository + Clone> {
    user_repo: Arc<R>,
}

impl<R: UserRepository + Clone> PasswordChangeLayer<R> {
    /// Create a new password change enforcement layer.
    pub fn new(user_repo: Arc<R>) -> Self {
        Self { user_repo }
    }
}

impl<S, R: UserRepository + Clone + 'static> tower::Layer<S> for PasswordChangeLayer<R> {
    type Service = PasswordChangeService<S, R>;

    fn layer(&self, inner: S) -> Self::Service {
        PasswordChangeService {
            inner,
            user_repo: self.user_repo.clone(),
        }
    }
}

/// Password change enforcement service.
#[derive(Clone)]
pub struct PasswordChangeService<S, R: UserRepository + Clone> {
    inner: S,
    user_repo: Arc<R>,
}

impl<S, B, R> tower::Service<axum::http::Request<B>> for PasswordChangeService<S, R>
where
    S: tower::Service<axum::http::Request<B>, Response = Response> + Clone + Send + 'static,
    S::Future: Send,
    B: Send + 'static,
    R: UserRepository + Clone + 'static,
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
        let user_repo = self.user_repo.clone();
        let mut inner = self.inner.clone();

        Box::pin(async move {
            // Extract claims from request extensions (set by JWT middleware)
            let claims = request.extensions().get::<Claims>().cloned();

            if let Some(claims) = claims {
                // Look up user to check must_change_password flag
                match user_repo.find_by_id(&claims.sub).await {
                    Ok(Some(user)) => {
                        if user.must_change_password {
                            // User must change password - return 403
                            return Ok(PasswordChangeRequiredError.into_response());
                        }
                    }
                    Ok(None) => {
                        // User not found - this shouldn't happen with valid JWT
                        // Let the request proceed, other middleware will handle it
                        tracing::warn!("User {} from JWT not found in database", claims.sub);
                    }
                    Err(e) => {
                        // Database error - log and let request proceed
                        // Don't block users due to transient DB issues
                        tracing::error!("Database error checking must_change_password: {}", e);
                    }
                }
            }

            // User doesn't need to change password or no claims - proceed
            inner.call(request).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_change_required_error_response() {
        let error = PasswordChangeRequiredError;
        let response = error.into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
