//! Managed playback handle validation, credential header attachment, and relay.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;

use super::hls::{ManagedRelay, RelayContext};
use super::ssrf::stream_proxy_client;
use super::upstream::{
    fetch_upstream_bound, is_hop_by_hop, is_page_or_api_content_type, relay_upstream,
};
use super::{StreamProxyQuery, StreamProxyState, USER_AGENT};
use crate::api::error::{ApiError, ApiResult};

pub(super) async fn managed_stream_proxy(
    state: &StreamProxyState,
    query: &StreamProxyQuery,
    incoming: &HeaderMap,
    principal: &str,
) -> ApiResult<Response> {
    use crate::services::playback_context::PlaybackError;
    let (Some(handle), Some(stream), Some(url)) = (
        query.playback_handle.as_deref(),
        query.stream,
        query.url.as_deref(),
    ) else {
        return Err(ApiError::bad_request(
            "Managed playback requires a playback handle, stream and URL",
        ));
    };
    let config = state.source_config.as_ref().ok_or(PlaybackError::Expired)?;
    let data = super::super::parse::validate_playback(config, handle, principal).await?;
    let target = managed_target(&data, stream, url)?;
    let headers = managed_media_headers(&data, stream, incoming)?;
    let allow_private = state.allow_private_targets().await;
    // Media leaves through the route its URL was extracted on: platforms may
    // sign stream URLs for that address.
    let client = stream_proxy_client(allow_private, &data.snapshot.route.target)?;
    let upstream = fetch_upstream_bound(
        &client,
        target,
        &headers,
        allow_private,
        Some((&data, stream)),
    )
    .await?;
    if !upstream.status().is_success() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "PLAYBACK_UPSTREAM_UNAVAILABLE",
            "Upstream media is unavailable; renew playback",
        ));
    }
    // An authenticated provider error page or API response can describe the
    // account; only media is relayed.
    if upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .is_some_and(is_page_or_api_content_type)
    {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "PLAYBACK_UPSTREAM_UNAVAILABLE",
            "Upstream returned a non-media response",
        ));
    }
    let token = if state.auth_service.is_some() {
        Some(crate::api::auth_request::request_credential(
            incoming,
            query.token.as_deref(),
        )?)
    } else {
        None
    };
    let mut response = relay_upstream(
        state,
        incoming,
        upstream,
        RelayContext {
            headers: None,
            token,
            source_url: Some(&data.source.url),
            web: query.web,
            playback: Some(ManagedRelay {
                handle,
                stream,
                data: &data,
            }),
        },
    )
    .await?;
    response
        .headers_mut()
        .extend(super::super::private_response_headers());
    response.headers_mut().remove(axum::http::header::ETAG);
    response
        .headers_mut()
        .remove(axum::http::header::LAST_MODIFIED);
    Ok(response)
}

/// Validates a client-supplied target before any of the context's headers are
/// attached to it. Redirect hops are checked again in `fetch_upstream_bound`.
pub(super) fn managed_target(
    data: &crate::services::playback_context::PlaybackData,
    stream: usize,
    url: &str,
) -> Result<url::Url, crate::services::playback_context::PlaybackError> {
    use crate::services::playback_context::{PlaybackError, checked_url};
    if stream >= data.media.streams.len() {
        return Err(PlaybackError::InvalidResource);
    }
    let target = checked_url(url)?;
    if !data.permits_authentication(&target, stream) {
        return Err(PlaybackError::HostNotAllowed);
    }
    Ok(target)
}

pub(super) fn managed_media_headers(
    data: &crate::services::playback_context::PlaybackData,
    stream_index: usize,
    incoming: &HeaderMap,
) -> ApiResult<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static(USER_AGENT),
    );
    let common = data
        .media
        .headers
        .iter()
        .flat_map(|headers| headers.iter())
        .map(|(name, value)| (name.as_str(), value.as_str()));
    let specific = data
        .media
        .streams
        .get(stream_index)
        .and_then(|stream| stream.extras.as_ref())
        .and_then(|extras| extras.get("headers"))
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|headers| headers.iter())
        .filter_map(|(name, value)| value.as_str().map(|value| (name.as_str(), value)));
    let mut cookie_updates = Vec::new();
    for (index, (name, value)) in common.chain(specific).enumerate() {
        if index >= 64 {
            return Err(ApiError::bad_request("Too many media headers"));
        }
        if name.eq_ignore_ascii_case("cookie") {
            cookie_updates.push(value);
            continue;
        }
        if is_hop_by_hop(name) {
            continue;
        }
        if name.len() > 256 || value.len() > 16_384 {
            return Err(ApiError::bad_request("Media header is too large"));
        }
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| ApiError::bad_request("Invalid media header"))?;
        let value = HeaderValue::from_str(value)
            .map_err(|_| ApiError::bad_request("Invalid media header"))?;
        headers.insert(name, value);
    }
    // The post-extraction snapshot includes any committed reactive login. It
    // wins duplicate cookie names over stale extractor header copies.
    let cookie =
        crate::credentials::merge_cookie_updates(&data.snapshot.material.cookies, cookie_updates);
    if !cookie.is_empty() {
        headers.insert(
            reqwest::header::COOKIE,
            HeaderValue::from_str(&cookie)
                .map_err(|_| ApiError::bad_request("Invalid media authentication"))?,
        );
    }
    if let Some(range) = incoming.get(axum::http::header::RANGE) {
        headers.insert(reqwest::header::RANGE, range.clone());
    }
    Ok(headers)
}
