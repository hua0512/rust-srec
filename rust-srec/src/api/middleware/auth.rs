//! API authentication middleware.
//!
//! Provides API key authentication for protected endpoints.

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// API key authentication configuration.
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    /// Valid API keys
    api_keys: Arc<Vec<String>>,
    /// Header name for API key
    header_name: String,
}

impl ApiKeyAuth {
    /// Create a new API key authenticator.
    pub fn new(api_keys: Vec<String>) -> Self {
        Self {
            api_keys: Arc::new(api_keys),
            header_name: "X-API-Key".to_string(),
        }
    }

    /// Create with a custom header name.
    pub fn with_header_name(mut self, name: impl Into<String>) -> Self {
        self.header_name = name.into();
        self
    }

    /// Check if an API key is valid.
    pub fn is_valid(&self, key: &str) -> bool {
        self.api_keys.iter().any(|k| k == key)
    }

    /// Get the header name.
    pub fn header_name(&self) -> &str {
        &self.header_name
    }
}

impl Default for ApiKeyAuth {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// Middleware function for API key authentication.
pub async fn api_key_auth(
    auth: ApiKeyAuth,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip auth if no API keys are configured
    if auth.api_keys.is_empty() {
        return Ok(next.run(request).await);
    }

    // Get API key from header
    let api_key = request
        .headers()
        .get(&auth.header_name)
        .and_then(|v| v.to_str().ok());

    match api_key {
        Some(key) if auth.is_valid(key) => Ok(next.run(request).await),
        Some(_) => {
            tracing::warn!("Invalid API key provided");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::warn!("Missing API key in request");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Layer for API key authentication.
#[derive(Clone)]
pub struct ApiKeyAuthLayer {
    auth: ApiKeyAuth,
}

impl ApiKeyAuthLayer {
    /// Create a new API key auth layer.
    pub fn new(api_keys: Vec<String>) -> Self {
        Self {
            auth: ApiKeyAuth::new(api_keys),
        }
    }
}

impl<S> tower::Layer<S> for ApiKeyAuthLayer {
    type Service = ApiKeyAuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ApiKeyAuthService {
            inner,
            auth: self.auth.clone(),
        }
    }
}

/// Service for API key authentication.
#[derive(Clone)]
pub struct ApiKeyAuthService<S> {
    inner: S,
    auth: ApiKeyAuth,
}

impl<S, B> tower::Service<axum::http::Request<B>> for ApiKeyAuthService<S>
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
        // Skip auth if no API keys are configured
        if self.auth.api_keys.is_empty() {
            let future = self.inner.call(request);
            return Box::pin(async move { future.await });
        }

        // Get API key from header
        let api_key = request
            .headers()
            .get(&self.auth.header_name)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let is_valid = api_key
            .as_ref()
            .map(|k| self.auth.is_valid(k))
            .unwrap_or(false);

        if is_valid {
            let future = self.inner.call(request);
            Box::pin(async move { future.await })
        } else {
            Box::pin(async move {
                let response = Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(axum::body::Body::from("Unauthorized"))
                    .unwrap();
                Ok(response.into_response())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_auth_creation() {
        let auth = ApiKeyAuth::new(vec!["key1".to_string(), "key2".to_string()]);
        
        assert!(auth.is_valid("key1"));
        assert!(auth.is_valid("key2"));
        assert!(!auth.is_valid("key3"));
    }

    #[test]
    fn test_api_key_auth_empty() {
        let auth = ApiKeyAuth::new(Vec::new());
        
        assert!(!auth.is_valid("any_key"));
    }

    #[test]
    fn test_custom_header_name() {
        let auth = ApiKeyAuth::new(vec!["key".to_string()])
            .with_header_name("Authorization");
        
        assert_eq!(auth.header_name(), "Authorization");
    }
}
