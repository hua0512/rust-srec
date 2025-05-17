//! # Cache Utilities
//!
//! Common utility functions for cache operations.

use reqwest::Response;

/// Extract common cache-related headers from an HTTP response
pub fn extract_cache_headers(
    response: &Response,
) -> (Option<String>, Option<String>, Option<String>) {
    let etag = response
        .headers()
        .get("ETag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let last_modified = response
        .headers()
        .get("Last-Modified")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let content_type = response
        .headers()
        .get("Content-Type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    (etag, last_modified, content_type)
}
