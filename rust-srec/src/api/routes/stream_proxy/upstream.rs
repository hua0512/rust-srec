//! Upstream HTTP fetching, redirect checking, response header sanitization, and streaming relay.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::{Bytes, BytesMut};
use futures::{StreamExt, TryStreamExt};

use super::hls::{
    RelayContext, is_hls_content_type, looks_like_hls_manifest, media_type_essence,
    rewrite_hls_manifest,
};
use super::ssrf::validate_target_url;
use super::{
    HLS_MAGIC_SCAN_BYTES, MAX_MANIFEST_BYTES, MAX_REDIRECTS, StreamProxyState, USER_AGENT,
};
use crate::api::error::{ApiError, ApiResult};

/// Headers that describe the client-to-proxy hop rather than the upstream
/// request, so a caller-supplied value is never forwarded.
pub(super) fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

pub(super) async fn fetch_upstream(
    client: &reqwest::Client,
    initial_target: url::Url,
    headers: &reqwest::header::HeaderMap,
    allow_private_targets: bool,
) -> ApiResult<reqwest::Response> {
    fetch_upstream_bound(client, initial_target, headers, allow_private_targets, None).await
}

pub(super) async fn fetch_upstream_bound(
    client: &reqwest::Client,
    initial_target: url::Url,
    headers: &reqwest::header::HeaderMap,
    allow_private_targets: bool,
    authentication: Option<(&crate::services::playback_context::PlaybackData, usize)>,
) -> ApiResult<reqwest::Response> {
    let mut target = initial_target;

    for redirect_count in 0..=MAX_REDIRECTS {
        validate_target_url(&target, allow_private_targets).await?;
        let request_headers = if authentication
            .is_some_and(|(data, index)| !data.permits_authentication(&target, index))
        {
            let mut safe = reqwest::header::HeaderMap::new();
            safe.insert(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(USER_AGENT),
            );
            if let Some(range) = headers.get(reqwest::header::RANGE) {
                safe.insert(reqwest::header::RANGE, range.clone());
            }
            safe
        } else {
            headers.clone()
        };
        let response = client
            .get(target.clone())
            .headers(request_headers)
            .send()
            .await
            .map_err(|error| {
                // `without_url` because reqwest errors embed the full target
                // URL, which may carry signed query parameters.
                tracing::debug!(
                    scheme = target.scheme(),
                    host = target.host_str().unwrap_or_default(),
                    error = %error.without_url(),
                    "stream proxy upstream request failed"
                );
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "BAD_GATEWAY",
                    "Proxy request failed",
                )
            })?;

        if !response.status().is_redirection() {
            return Ok(response);
        }

        let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
            return Ok(response);
        };
        if redirect_count == MAX_REDIRECTS {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Too many upstream redirects",
            ));
        }

        let location = location.to_str().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Invalid upstream redirect",
            )
        })?;
        target = target.join(location).map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Invalid upstream redirect",
            )
        })?;
    }

    Err(ApiError::new(
        StatusCode::BAD_GATEWAY,
        "BAD_GATEWAY",
        "Too many upstream redirects",
    ))
}

/// The `Content-Type` to relay for an upstream response.
///
/// The relay answers on the API origin with whatever the target returns, so
/// an HTML, SVG or other XML page served under its own type would run as a
/// page on that origin. Only media, image, HLS and JSON types are kept, as the
/// bare parsed type: forwarding the raw value would let `video/mp4;,text/html`
/// through, since browsers use the last type in a comma list. Anything else, a
/// missing type included, becomes an opaque download.
fn relayed_content_type(value: Option<&HeaderValue>) -> HeaderValue {
    let opaque = HeaderValue::from_static("application/octet-stream");
    let Some(raw) = value.and_then(|value| value.to_str().ok()) else {
        return opaque;
    };
    if raw.contains(',') {
        return opaque;
    }
    let media_type = media_type_essence(raw);
    let Some((top, sub)) = media_type.split_once('/') else {
        return opaque;
    };
    let plain_subtype = !sub.is_empty()
        && !sub.ends_with("+xml")
        && sub
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+.-".contains(&byte));
    let allowed = matches!(top, "video" | "audio" | "image")
        || matches!(
            media_type.as_str(),
            "application/vnd.apple.mpegurl" | "application/x-mpegurl" | "application/json"
        );
    if plain_subtype && allowed {
        HeaderValue::from_str(&media_type).unwrap_or(opaque)
    } else {
        opaque
    }
}

/// Whether an upstream `Content-Type` names an HTML page or a JSON document,
/// which a provider sends for error pages and API answers rather than media.
pub(super) fn is_page_or_api_content_type(value: &HeaderValue) -> bool {
    let Ok(raw) = value.to_str() else {
        return true;
    };
    raw.split(',').any(|item| {
        let media_type = media_type_essence(item);
        let Some((top, sub)) = media_type.split_once('/') else {
            return false;
        };
        matches!(
            (top, sub),
            ("text", "html") | ("application", "xhtml+xml") | ("application" | "text", "json")
        ) || sub.ends_with("+json")
    })
}

pub(super) async fn relay_upstream(
    state: &StreamProxyState,
    headers_in: &HeaderMap,
    upstream: reqwest::Response,
    context: RelayContext<'_>,
) -> ApiResult<Response> {
    let status = upstream.status();
    let final_url = upstream.url().clone();
    let hls_content_type = is_hls_content_type(upstream.headers());
    let authenticated = state.auth_service.is_some()
        || context.token.is_some()
        || context.headers.is_some()
        || context.playback.is_some()
        || headers_in.contains_key(axum::http::header::AUTHORIZATION)
        || headers_in.contains_key(axum::http::header::COOKIE);

    // Build response headers.
    let mut out_headers = HeaderMap::new();
    out_headers.insert(
        axum::http::header::CONTENT_TYPE,
        relayed_content_type(upstream.headers().get(reqwest::header::CONTENT_TYPE)),
    );
    out_headers.insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    out_headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox"),
    );
    let allowed = [
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

    if authenticated {
        out_headers.insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("private, no-store"),
        );
    }

    state
        .cors
        .apply_allow_origin(headers_in.get(axum::http::header::ORIGIN), &mut out_headers);
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

    let mut upstream_stream = upstream.bytes_stream();
    let mut initial_chunks = Vec::new();
    let mut prefix = BytesMut::new();
    while prefix.len() < HLS_MAGIC_SCAN_BYTES {
        let next_chunk = tokio::select! {
            _ = state.cancel_token.cancelled() => {
                return Err(ApiError::service_unavailable("Server is shutting down"));
            }
            chunk = upstream_stream.try_next() => chunk,
        };
        let Some(chunk) = next_chunk.map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Proxy response failed",
            )
        })?
        else {
            break;
        };
        prefix.extend_from_slice(&chunk);
        initial_chunks.push(chunk);
    }

    let body = if hls_content_type || looks_like_hls_manifest(&prefix) {
        let mut manifest_bytes = prefix;
        if manifest_bytes.len() > MAX_MANIFEST_BYTES {
            return Err(ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Upstream HLS manifest is too large",
            ));
        }
        loop {
            let next_chunk = tokio::select! {
                _ = state.cancel_token.cancelled() => {
                    return Err(ApiError::service_unavailable("Server is shutting down"));
                }
                chunk = upstream_stream.try_next() => chunk,
            };
            let Some(chunk) = next_chunk.map_err(|_| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "BAD_GATEWAY",
                    "Proxy response failed",
                )
            })?
            else {
                break;
            };
            if manifest_bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
                return Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "BAD_GATEWAY",
                    "Upstream HLS manifest is too large",
                ));
            }
            manifest_bytes.extend_from_slice(&chunk);
        }

        if looks_like_hls_manifest(&manifest_bytes) {
            let manifest = std::str::from_utf8(&manifest_bytes).map_err(|_| {
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "BAD_GATEWAY",
                    "Upstream HLS manifest is not UTF-8",
                )
            })?;
            let rewritten = rewrite_hls_manifest(manifest, &final_url, context);
            // A manifest recognised by its content may arrive under any type,
            // and native HLS players only accept it under the HLS one.
            out_headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.apple.mpegurl"),
            );
            out_headers.remove(axum::http::header::CONTENT_LENGTH);
            out_headers.remove(axum::http::header::CONTENT_RANGE);
            out_headers.remove(axum::http::header::ACCEPT_RANGES);
            out_headers.remove(axum::http::header::ETAG);
            out_headers.remove(axum::http::header::LAST_MODIFIED);
            out_headers.insert(
                axum::http::header::CACHE_CONTROL,
                HeaderValue::from_static("private, no-store"),
            );
            axum::body::Body::from(rewritten)
        } else {
            axum::body::Body::from(manifest_bytes.freeze())
        }
    } else {
        let prefix_stream =
            futures::stream::iter(initial_chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
        let remaining_stream = upstream_stream
            .map_err(std::io::Error::other)
            .take_until(state.cancel_token.clone().cancelled_owned());
        axum::body::Body::from_stream(prefix_stream.chain(remaining_stream))
    };

    let mut response = (status, body).into_response();
    *response.headers_mut() = out_headers;
    Ok(response)
}
