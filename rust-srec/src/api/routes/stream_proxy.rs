//! Stream proxy routes.
//!
//! Desktop builds do not run a TanStack Start server, so the frontend cannot rely on
//! `/stream-proxy` server handlers. This route provides an authenticated proxy under
//! `/api/stream-proxy` that can forward media requests with custom headers and Range
//! support.

use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header::AUTHORIZATION};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures::TryStreamExt;
use serde::Deserialize;
use std::sync::OnceLock;
use std::time::Duration;

use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;

fn stream_proxy_client() -> ApiResult<&'static reqwest::Client> {
    // The platforms-parser default client sets a 30s request timeout.
    // That breaks long-lived streaming responses (e.g. mpegts/flv) and manifests
    // as `UnrecoverableEarlyEof` after ~30s.
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(20)
            .build()
            .map_err(|e| e.to_string())
    });

    match client {
        Ok(client) => Ok(client),
        Err(message) => Err(ApiError::internal(message.clone())),
    }
}

#[derive(Debug, Deserialize)]
pub struct StreamProxyQuery {
    pub url: String,
    pub headers: Option<String>,
    pub token: Option<String>,
}

/// Create the stream proxy router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Mounted under `/api/stream-proxy` by the main router.
        .route("/", get(stream_proxy_get).options(stream_proxy_options))
}

async fn stream_proxy_options() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Range, Authorization"),
    );
    headers.insert(
        axum::http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Length, Content-Range, Accept-Ranges"),
    );

    (StatusCode::NO_CONTENT, headers)
}

pub async fn stream_proxy_get(
    State(state): State<AppState>,
    Query(query): Query<StreamProxyQuery>,
    req: Request,
) -> ApiResult<Response> {
    // Auth: support token query param (media elements can't send Authorization easily).
    let jwt_service = state
        .jwt_service
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("Authentication not configured"))?;

    let headers_in = req.headers();

    let token = if let Some(t) = query.token.clone() {
        t
    } else if let Some(t) = headers_in
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(String::from)
    {
        t
    } else {
        return Err(ApiError::unauthorized(
            "Missing or invalid Authorization header or token query",
        ));
    };

    jwt_service
        .validate_token(&token)
        .map_err(|_| ApiError::unauthorized("Invalid or expired token"))?;

    // Validate target URL.
    let target = url::Url::parse(&query.url)
        .map_err(|e| ApiError::bad_request(format!("Invalid url: {e}")))?;
    match target.scheme() {
        "http" | "https" => {}
        _ => return Err(ApiError::bad_request("Only http/https URLs are allowed")),
    }

    // Basic SSRF guardrails: disallow loopback and private IP literals.
    if let Some(host) = target.host_str() {
        if host.eq_ignore_ascii_case("localhost") {
            return Err(ApiError::bad_request("localhost is not allowed"));
        }

        // Allow loopback/private IP literals in tests so we can stand up an in-process upstream.
        if !cfg!(test)
            && let Ok(ip) = host.parse::<std::net::IpAddr>()
        {
            if ip.is_loopback() {
                return Err(ApiError::bad_request("loopback is not allowed"));
            }
            if matches!(ip, std::net::IpAddr::V4(v4) if v4.is_private()) {
                return Err(ApiError::bad_request("private ip is not allowed"));
            }
        }
    }

    // Parse custom headers JSON.
    let mut custom_headers: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    if let Some(raw) = &query.headers {
        custom_headers =
            serde_json::from_str(raw).map_err(|_| ApiError::bad_request("Invalid headers JSON"))?;
    }

    // Build upstream headers.
    let mut upstream_headers = reqwest::header::HeaderMap::new();
    // Default UA (helps with anti-hotlink).
    upstream_headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36",
        ),
    );

    for (k, v) in custom_headers {
        let lower = k.to_ascii_lowercase();
        if lower == "host" || lower == "connection" {
            continue;
        }
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| ApiError::bad_request("Invalid header name"))?;
        let value =
            HeaderValue::from_str(&v).map_err(|_| ApiError::bad_request("Invalid header value"))?;
        upstream_headers.insert(name, value);
    }

    // Forward Range header.
    if let Some(range) = headers_in.get(axum::http::header::RANGE)
        && let Ok(val) = range.to_str()
        && let Ok(value) = HeaderValue::from_str(val)
    {
        upstream_headers.insert(reqwest::header::RANGE, value);
    }

    let client = stream_proxy_client()?;

    let upstream = client
        .get(target)
        .headers(upstream_headers)
        .send()
        .await
        .map_err(|e| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                format!("Proxy request failed: {e}"),
            )
        })?;

    let status = upstream.status();

    // Build response headers.
    let mut out_headers = HeaderMap::new();
    let allowed = [
        axum::http::header::CONTENT_TYPE,
        axum::http::header::CONTENT_LENGTH,
        axum::http::header::CONTENT_RANGE,
        axum::http::header::ACCEPT_RANGES,
        axum::http::header::CACHE_CONTROL,
        axum::http::header::ETAG,
        axum::http::header::LAST_MODIFIED,
        axum::http::header::DATE,
    ];

    for key in allowed {
        if let Some(value) = upstream.headers().get(key.as_str()) {
            out_headers.insert(key, value.clone());
        }
    }

    // Enable CORS for the player (desktop uses tauri:// origin).
    out_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    out_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    out_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Range, Authorization"),
    );
    out_headers.insert(
        axum::http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("Content-Length, Content-Range, Accept-Ranges"),
    );

    // Stream upstream body to client.
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let body = axum::body::Body::from_stream(stream);

    let mut response = (status, body).into_response();
    *response.headers_mut() = out_headers;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request as HttpRequest, header};
    use axum::response::IntoResponse;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    use crate::api::jwt::JwtService;

    async fn upstream_handler(req: HttpRequest<Body>) -> impl IntoResponse {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

        let status = if let Some(range) = req.headers().get(header::RANGE) {
            if let Ok(range_str) = range.to_str() {
                let _ = range_str;
                let value = HeaderValue::from_static("bytes 0-1/3");
                headers.insert(header::CONTENT_RANGE, value);
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            }
        } else {
            StatusCode::OK
        };

        (status, headers, "abc")
    }

    fn build_query(pairs: &[(&str, &str)]) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in pairs {
            ser.append_pair(k, v);
        }
        ser.finish()
    }

    #[tokio::test]
    async fn proxy_forwards_range_and_sets_cors_headers() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new().route("/stream", get(upstream_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let jwt = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        ));
        let token = jwt.generate_token("user", vec![]).unwrap();

        let mut state = AppState::new();
        state.jwt_service = Some(jwt);

        let app = Router::new()
            .nest("/api/stream-proxy", super::router())
            .with_state(state);

        let target = format!("http://{upstream_addr}/stream");
        let headers_json = r#"{"Referer":"https://example.com/"}"#;
        let query = build_query(&[
            ("url", &target),
            ("headers", headers_json),
            ("token", &token),
        ]);

        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .header(header::RANGE, "bytes=0-1")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "*"
        );
        assert!(response.headers().get(header::CONTENT_TYPE).is_some());
        assert!(response.headers().get(header::CONTENT_RANGE).is_some());
    }

    #[tokio::test]
    async fn proxy_rejects_non_http_schemes() {
        let jwt = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(3600),
        ));
        let token = jwt.generate_token("user", vec![]).unwrap();

        let mut state = AppState::new();
        state.jwt_service = Some(jwt);

        let app = Router::new()
            .nest("/api/stream-proxy", super::router())
            .with_state(state);

        let query = build_query(&[("url", "file:///etc/passwd"), ("token", &token)]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
