//! Stream proxy routes.
//!
//! Desktop builds do not run a TanStack Start server, so the frontend cannot rely on
//! `/stream-proxy` server handlers. This route provides an authenticated proxy under
//! `/api/stream-proxy` that can forward media requests with custom headers and Range
//! support.

mod hls;
mod managed;
mod ssrf;
#[cfg(test)]
mod tests;
mod upstream;

use std::sync::Arc;

use axum::Router;
use axum::extract::{FromRef, Request, State};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use self::hls::RelayContext;
use self::managed::managed_stream_proxy;
use self::ssrf::stream_proxy_client;
pub(super) use self::ssrf::validate_target_url;
use self::upstream::{fetch_upstream, is_hop_by_hop, relay_upstream};
use super::parse::{ParseRouteState, resolve_route_for_url};
use crate::api::auth_service::AuthService;
use crate::api::cors::CorsPolicy;
use crate::api::error::{ApiError, ApiResult};
use crate::api::extract::ApiQuery;
use crate::api::server::AppState;
use crate::proxies::ProxyTarget;

const MAX_REDIRECTS: usize = 5;
const MAX_MANIFEST_BYTES: usize = 8 * 1024 * 1024;
const HLS_MAGIC_SCAN_BYTES: usize = 10;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/123.0.0.0 Safari/537.36";

type SharedConfigService = Arc<
    crate::config::ConfigService<
        crate::database::repositories::config::SqlxConfigRepository,
        crate::database::repositories::streamer::SqlxStreamerRepository,
    >,
>;

#[derive(Clone)]
pub struct StreamProxyState {
    auth_service: Option<Arc<AuthService>>,
    source_config: Option<ParseRouteState>,
    /// Used only when no application config resolver is available (tests).
    upstream_proxy: ProxyTarget,
    /// Source of the `stream_proxy_allow_private_targets` global-config flag,
    /// read per request so UI changes apply without a restart. `None` (tests
    /// only) falls back to `allow_private_targets`.
    config_service: Option<SharedConfigService>,
    allow_private_targets: bool,
    /// Same policy the router-wide `CorsLayer` applies, so the CORS headers
    /// these handlers write themselves cannot be looser than it.
    cors: CorsPolicy,
    /// Server shutdown token used to close continuous media streams during graceful shutdown.
    cancel_token: CancellationToken,
}

impl FromRef<AppState> for StreamProxyState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            auth_service: state.auth_service.clone(),
            source_config: Some(ParseRouteState::from_ref(state)),
            upstream_proxy: ProxyTarget::Direct,
            config_service: Some(state.config_service.clone()),
            allow_private_targets: false,
            cors: state.cors_policy(),
            cancel_token: state.cancel_token.clone(),
        }
    }
}

impl StreamProxyState {
    /// The current `stream_proxy_allow_private_targets` setting. Fails closed:
    /// a config read error must not widen the target policy.
    async fn allow_private_targets(&self) -> bool {
        match &self.config_service {
            Some(config_service) => config_service
                .get_cached_global_config()
                .await
                .map(|config| config.stream_proxy_allow_private_targets)
                .unwrap_or(false),
            None => self.allow_private_targets,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamProxyQuery {
    pub url: Option<String>,
    pub headers: Option<String>,
    pub token: Option<String>,
    pub source_url: Option<String>,
    pub playback_handle: Option<String>,
    /// Index into the managed parse result's streams; selects whose headers apply.
    pub stream: Option<usize>,
    #[serde(rename = "_HLS_msn")]
    pub hls_msn: Option<u64>,
    #[serde(rename = "_HLS_part")]
    pub hls_part: Option<u64>,
    #[serde(rename = "_HLS_skip")]
    pub hls_skip: Option<HlsSkip>,
    /// Web BFF requests cookie-authenticated links without exposing its bearer token.
    #[serde(default)]
    pub web: bool,
}

#[derive(Clone, Copy, Deserialize)]
pub enum HlsSkip {
    #[serde(rename = "YES")]
    Yes,
    #[serde(rename = "v2")]
    VersionTwo,
}

/// Create the stream proxy router.
pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    StreamProxyState: FromRef<S>,
{
    Router::new()
        // Mounted under `/api/stream-proxy` by the main router. `OPTIONS` is
        // not routed: the router-wide `CorsLayer` answers every `OPTIONS`
        // itself without calling the inner service.
        .route("/", get(stream_proxy_get))
}

pub async fn stream_proxy_get(
    State(state): State<StreamProxyState>,
    ApiQuery(query): ApiQuery<StreamProxyQuery>,
    req: Request,
) -> ApiResult<Response> {
    let headers_in = req.headers();
    // Low-latency delivery hints are not forwarded upstream. Accepting their
    // typed forms keeps players that append them working.
    let _ = (query.hls_msn, query.hls_part, query.hls_skip);

    let principal = crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers_in,
        query.token.as_deref(),
        crate::api::auth_request::AccessPolicy::Full,
    )
    .await?;
    if query.playback_handle.is_some() || query.stream.is_some() {
        if query.headers.is_some() || query.source_url.is_some() {
            return Err(ApiError::bad_request(
                "Managed playback cannot contain client headers or a source URL",
            ));
        }
        let principal = principal
            .as_ref()
            .map(|principal| principal.claims.sub.as_str())
            .unwrap_or(super::LOCAL_ANONYMOUS_PRINCIPAL);
        return managed_stream_proxy(&state, &query, headers_in, principal).await;
    }
    let relay_token = if state.auth_service.is_some() {
        Some(crate::api::auth_request::request_credential(
            headers_in,
            query.token.as_deref(),
        )?)
    } else {
        query.token.as_deref()
    };

    let allow_private_targets = state.allow_private_targets().await;

    // Validated by fetch_upstream, which checks the initial target and every
    // redirect hop with validate_target_url before fetching it.
    let raw_url = query
        .url
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("URL is required"))?;
    let target =
        url::Url::parse(raw_url).map_err(|_| ApiError::bad_request("Invalid url parameter"))?;

    let mut custom_headers: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    if let Some(raw) = &query.headers {
        custom_headers =
            serde_json::from_str(raw).map_err(|_| ApiError::bad_request("Invalid headers JSON"))?;
    }
    if custom_headers.len() > 64 {
        return Err(ApiError::bad_request("Too many custom headers"));
    }

    let mut upstream_headers = reqwest::header::HeaderMap::new();
    upstream_headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static(USER_AGENT),
    );

    for (k, v) in custom_headers {
        if is_hop_by_hop(&k) {
            continue;
        }
        if k.len() > 256 || v.len() > 16_384 {
            return Err(ApiError::bad_request("Custom header is too large"));
        }
        let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| ApiError::bad_request("Invalid header name"))?;
        let value =
            HeaderValue::from_str(&v).map_err(|_| ApiError::bad_request("Invalid header value"))?;
        upstream_headers.insert(name, value);
    }

    if let Some(range) = headers_in.get(axum::http::header::RANGE)
        && let Ok(val) = range.to_str()
        && let Ok(value) = HeaderValue::from_str(val)
    {
        upstream_headers.insert(reqwest::header::RANGE, value);
    }

    let source_url = query.source_url.as_deref().unwrap_or(raw_url);
    let source =
        url::Url::parse(source_url).map_err(|_| ApiError::bad_request("Invalid source URL"))?;
    if !matches!(source.scheme(), "http" | "https")
        || !source.username().is_empty()
        || source.password().is_some()
    {
        return Err(ApiError::bad_request("Invalid source URL"));
    }
    let upstream_proxy = match &state.source_config {
        Some(config) => resolve_route_for_url(config, source_url).await?.target,
        None => state.upstream_proxy.clone(),
    };
    let client = stream_proxy_client(allow_private_targets, &upstream_proxy)?;
    let upstream =
        fetch_upstream(&client, target, &upstream_headers, allow_private_targets).await?;

    relay_upstream(
        &state,
        headers_in,
        upstream,
        RelayContext {
            headers: query.headers.as_deref(),
            token: relay_token,
            source_url: Some(source_url),
            web: query.web,
            playback: None,
        },
    )
    .await
}
