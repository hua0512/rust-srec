//! Stream proxy routes.
//!
//! Desktop builds do not run a TanStack Start server, so the frontend cannot rely on
//! `/stream-proxy` server handlers. This route provides an authenticated proxy under
//! `/api/stream-proxy` that can forward media requests with custom headers and Range
//! support.

mod hls_variables;

use axum::Router;
use axum::extract::{FromRef, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::{Bytes, BytesMut};
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::net::lookup_host;

use super::parse::{ParseRouteState, resolve_proxy_config_for_url};
use crate::api::auth_service::AuthService;
use crate::api::cors::CorsPolicy;
use crate::api::error::{ApiError, ApiResult};
use crate::api::server::AppState;
use crate::domain::ProxyConfig;

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
    playback: Option<Arc<crate::services::playback_context::PlaybackContextService>>,
    auth_service: Option<Arc<AuthService>>,
    source_config: Option<ParseRouteState>,
    /// Used only when no application config resolver is available (tests).
    proxy_config: ProxyConfig,
    /// Source of the `stream_proxy_allow_private_targets` global-config flag,
    /// read per request so UI changes apply without a restart. `None` (tests
    /// only) falls back to `allow_private_targets`.
    config_service: Option<SharedConfigService>,
    allow_private_targets: bool,
    /// Same policy the router-wide `CorsLayer` applies, so the CORS headers
    /// these handlers write themselves cannot be looser than it.
    cors: CorsPolicy,
}

impl FromRef<AppState> for StreamProxyState {
    fn from_ref(state: &AppState) -> Self {
        Self {
            playback: Some(state.playback_contexts.clone()),
            auth_service: state.auth_service.clone(),
            source_config: Some(ParseRouteState::from_ref(state)),
            proxy_config: ProxyConfig::disabled(),
            config_service: Some(state.config_service.clone()),
            allow_private_targets: false,
            cors: state.cors_policy(),
        }
    }
}

fn stream_proxy_client(
    allow_private_targets: bool,
    proxy_config: &ProxyConfig,
) -> ApiResult<reqwest::Client> {
    // Bound the pool cache: different streamers may have different proxy credentials.
    // reqwest clients are cheap to clone and old streams keep their own pool alive.
    type CachedClient = (bool, ProxyConfig, reqwest::Client);
    static CLIENTS: OnceLock<Mutex<Vec<CachedClient>>> = OnceLock::new();
    let mut clients = CLIENTS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map_err(|_| ApiError::internal("Stream proxy HTTP client is unavailable"))?;
    if let Some((_, _, client)) = clients
        .iter()
        .find(|(private, config, _)| *private == allow_private_targets && config == proxy_config)
    {
        return Ok(client.clone());
    }
    crate::utils::http_client::install_rustls_provider();
    // Never impose a whole-response timeout on continuous FLV/MPEG-TS streams.
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .tcp_nodelay(true)
        .pool_max_idle_per_host(20)
        .redirect(reqwest::redirect::Policy::none());
    let uses_proxy =
        proxy_config.enabled && (proxy_config.url.is_some() || proxy_config.use_system_proxy);
    if proxy_config.enabled
        && let Some(url) = proxy_config.url.as_deref()
    {
        // The shared extractor helper falls back to direct on malformed proxy URLs.
        // Playback must fail closed rather than send media outside the chosen route.
        reqwest::Proxy::all(url)
            .map_err(|_| ApiError::bad_request("Invalid upstream proxy configuration"))?;
    }
    builder = crate::utils::http_client::apply_proxy_config(builder, proxy_config);
    if !allow_private_targets && !uses_proxy {
        builder = builder.dns_resolver(Arc::new(PublicAddressResolver));
    }
    // An operator-configured proxy is a trusted egress endpoint and may itself
    // live on a private network. Target/redirect validation still happens before
    // every request; remote DNS and routing are enforced by that proxy.
    let client = builder
        .build()
        .map_err(|_| ApiError::internal("Stream proxy HTTP client is unavailable"))?;
    if clients.len() >= 16 {
        clients.remove(0);
    }
    clients.push((allow_private_targets, proxy_config.clone(), client.clone()));
    Ok(client)
}

fn is_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !matches!(
        (a, b, c),
        (0, _, _)
            | (10, _, _)
            | (100, 64..=127, _)
            | (127, _, _)
            | (169, 254, _)
            | (172, 16..=31, _)
            | (192, 0, 0)
            | (192, 0, 2)
            | (192, 168, _)
            | (198, 18..=19, _)
            | (198, 51, 100)
            | (203, 0, 113)
            | (224..=255, _, _)
    )
}

fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let segments = address.segments();
    (segments[0] & 0xe000) == 0x2000 && !(segments[0] == 0x2001 && segments[1] == 0x0db8)
}

fn is_public_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_ipv4(address),
        IpAddr::V6(address) => is_public_ipv6(address),
    }
}

/// Resolve `host` and reject the result unless every address is public.
///
/// Backs `PublicAddressResolver`, so the addresses the strict client's
/// connector dials are exactly the addresses that passed `is_public_ip`.
/// `validate_target_url` runs the same check earlier for a friendly 400, but
/// the connection performs its own lookup afterwards; a DNS record that
/// changes between the two lookups (rebinding) is caught here.
async fn resolve_public_addresses(host: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
    let addresses: Vec<std::net::SocketAddr> = lookup_host((host, 0)).await?.collect();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(std::io::Error::other(
            "target host resolved to a non-public address",
        ));
    }
    Ok(addresses)
}

struct PublicAddressResolver;

impl reqwest::dns::Resolve for PublicAddressResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addresses = resolve_public_addresses(name.as_str()).await?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

async fn validate_target_url(target: &url::Url, allow_private_targets: bool) -> ApiResult<()> {
    match target.scheme() {
        "http" | "https" => {}
        _ => return Err(ApiError::bad_request("Only http/https URLs are allowed")),
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(ApiError::bad_request("URL credentials are not allowed"));
    }

    let host = target
        .host_str()
        .ok_or_else(|| ApiError::bad_request("Target host is required"))?;
    if allow_private_targets {
        // `stream_proxy_allow_private_targets` opts the operator into LAN and
        // localhost sources, so only the scheme/credential checks above apply.
        return Ok(());
    }

    let normalized_host = host.trim_end_matches('.');
    if normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host.to_ascii_lowercase().ends_with(".localhost")
    {
        return Err(ApiError::bad_request("Target host is not allowed"));
    }

    if let Ok(address) = normalized_host.parse::<IpAddr>() {
        if !is_public_ip(address) {
            return Err(ApiError::bad_request("Target host is not allowed"));
        }
        return Ok(());
    }

    let port = target
        .port_or_known_default()
        .ok_or_else(|| ApiError::bad_request("Target port is required"))?;
    let addresses = lookup_host((normalized_host, port))
        .await
        .map_err(|_| ApiError::bad_request("Target hostname could not be resolved"))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public_ip(address.ip())) {
        return Err(ApiError::bad_request("Target host is not allowed"));
    }

    Ok(())
}

async fn fetch_upstream(
    client: &reqwest::Client,
    initial_target: url::Url,
    headers: &reqwest::header::HeaderMap,
    allow_private_targets: bool,
) -> ApiResult<reqwest::Response> {
    fetch_upstream_bound(client, initial_target, headers, allow_private_targets, None).await
}

async fn fetch_upstream_bound(
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

#[derive(Clone, Copy, Default)]
struct RelayContext<'a> {
    headers: Option<&'a str>,
    token: Option<&'a str>,
    source_url: Option<&'a str>,
    web: bool,
}

fn build_proxy_url(target: &url::Url, context: RelayContext<'_>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("url", target.as_str());
    if let Some(headers) = context.headers {
        serializer.append_pair("headers", headers);
    }
    if let Some(token) = context.token.filter(|_| !context.web) {
        serializer.append_pair("token", token);
    }
    if let Some(source_url) = context.source_url {
        serializer.append_pair("source_url", source_url);
    }
    let path = if context.web {
        "/stream-proxy"
    } else {
        "/api/stream-proxy"
    };
    format!("{path}?{}", serializer.finish())
}

fn proxy_hls_uri(uri: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let Ok(target) = base_url.join(uri) else {
        return uri.to_string();
    };
    if !matches!(target.scheme(), "http" | "https") {
        return uri.to_string();
    }
    build_proxy_url(&target, context)
}

fn find_uri_attribute(line: &str, from: usize) -> Option<(usize, usize)> {
    let mut search_from = from;
    while let Some(relative_start) = line.get(search_from..)?.find("URI") {
        let attribute_start = search_from + relative_start;
        let preceding = line.get(..attribute_start)?.trim_end().chars().next_back();
        let server_uri = line
            .get(..attribute_start)
            .is_some_and(|prefix| prefix.ends_with("SERVER-"));
        if !matches!(preceding, Some(':') | Some(',')) && !server_uri {
            search_from = attribute_start + 3;
            continue;
        }

        let bytes = line.as_bytes();
        let mut cursor = attribute_start + 3;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            search_from = attribute_start + 3;
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            search_from = attribute_start + 3;
            continue;
        }

        let value_start = cursor + 1;
        let relative_end = line.get(value_start..)?.find('"')?;
        return Some((value_start, value_start + relative_end));
    }
    None
}

fn rewrite_uri_attributes(line: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let mut output = String::with_capacity(line.len());
    let mut copied_until = 0;
    let mut search_from = 0;

    while let Some((value_start, value_end)) = find_uri_attribute(line, search_from) {
        output.push_str(&line[copied_until..value_start]);
        output.push_str(&proxy_hls_uri(
            &line[value_start..value_end],
            base_url,
            context,
        ));
        copied_until = value_end;
        search_from = value_end + 1;
    }
    output.push_str(&line[copied_until..]);
    output
}

fn rewrite_hls_line(line: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return line.to_string();
    }
    if trimmed.starts_with('#') {
        return rewrite_uri_attributes(line, base_url, context);
    }

    let start = line.find(trimmed).unwrap_or(0);
    let end = start + trimmed.len();
    format!(
        "{}{}{}",
        &line[..start],
        proxy_hls_uri(trimmed, base_url, context),
        &line[end..]
    )
}

fn rewrite_hls_manifest(manifest: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let mut output = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        let (content, newline) = if let Some(content) = line.strip_suffix("\r\n") {
            (content, "\r\n")
        } else if let Some(content) = line.strip_suffix('\n') {
            (content, "\n")
        } else {
            (line, "")
        };
        output.push_str(&rewrite_hls_line(content, base_url, context));
        output.push_str(newline);
    }
    output
}

fn looks_like_hls_manifest(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    bytes.starts_with(b"#EXTM3U")
}

fn is_hls_content_type(headers: &reqwest::header::HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let media_type = content_type.split(';').next().unwrap_or_default().trim();
    matches!(
        media_type.to_ascii_lowercase().as_str(),
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamProxyQuery {
    pub url: Option<String>,
    pub headers: Option<String>,
    pub token: Option<String>,
    pub source_url: Option<String>,
    pub playback_handle: Option<String>,
    pub resource_id: Option<String>,
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
    Query(query): Query<StreamProxyQuery>,
    req: Request,
) -> ApiResult<Response> {
    let headers_in = req.headers();
    // These client delivery hints were historically ignored by this relay.
    // Accept their typed forms without allowing them to choose an upstream URL.
    let _ = (query.hls_msn, query.hls_part, query.hls_skip);

    let principal = crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers_in,
        query.token.as_deref(),
        crate::api::auth_request::AccessPolicy::Full,
    )
    .await?;
    if query.playback_handle.is_some() || query.resource_id.is_some() {
        if query.url.is_some() || query.headers.is_some() || query.source_url.is_some() {
            return Err(ApiError::bad_request(
                "Managed playback cannot contain raw URLs or headers",
            ));
        }
        let principal = principal
            .as_ref()
            .map(|principal| principal.claims.sub.as_str())
            .unwrap_or("local-anonymous");
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

    // Fail closed: a config read error must not widen the target policy.
    let allow_private_targets = match &state.config_service {
        Some(config_service) => config_service
            .get_cached_global_config()
            .await
            .map(|config| config.stream_proxy_allow_private_targets)
            .unwrap_or(false),
        None => state.allow_private_targets,
    };

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
        let lower = k.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
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
        ) {
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
    let proxy_config = match &state.source_config {
        Some(config) => resolve_proxy_config_for_url(config, source_url).await,
        None => state.proxy_config.clone(),
    };
    let client = stream_proxy_client(allow_private_targets, &proxy_config)?;
    let upstream =
        fetch_upstream(&client, target, &upstream_headers, allow_private_targets).await?;

    relay_upstream(
        &state,
        headers_in,
        upstream,
        false,
        |manifest, final_url| {
            Ok(rewrite_hls_manifest(
                manifest,
                final_url,
                RelayContext {
                    headers: query.headers.as_deref(),
                    token: relay_token,
                    source_url: Some(source_url),
                    web: query.web,
                },
            ))
        },
    )
    .await
}

async fn managed_stream_proxy(
    state: &StreamProxyState,
    query: &StreamProxyQuery,
    incoming: &HeaderMap,
    principal: &str,
) -> ApiResult<Response> {
    let handle = query
        .playback_handle
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("Playback handle is required"))?;
    let resource = query
        .resource_id
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("Playback resource ID is required"))?;
    let playback = state
        .playback
        .as_ref()
        .ok_or(crate::services::playback_context::PlaybackError::Expired)?;
    let config = state
        .source_config
        .as_ref()
        .ok_or(crate::services::playback_context::PlaybackError::Expired)?;
    let data = super::parse::validate_playback(config, handle, principal).await?;
    let resource = playback.resource(handle, principal, resource)?;
    let headers = managed_media_headers(&data, resource.stream_index, incoming)?;
    let allow_private = match &state.config_service {
        Some(service) => service
            .get_cached_global_config()
            .await
            .map(|config| config.stream_proxy_allow_private_targets)
            .unwrap_or(false),
        None => state.allow_private_targets,
    };
    let proxy_config = resolve_proxy_config_for_url(config, &data.source.url).await;
    let client = stream_proxy_client(allow_private, &proxy_config)?;
    let upstream = fetch_upstream_bound(
        &client,
        resource.target,
        &headers,
        allow_private,
        Some((&data, resource.stream_index)),
    )
    .await?;
    if !upstream.status().is_success() {
        return Err(ApiError::new(
            StatusCode::BAD_GATEWAY,
            "PLAYBACK_UPSTREAM_UNAVAILABLE",
            "Upstream media is unavailable; renew playback",
        ));
    }
    if upstream
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.starts_with("text/html") || value.starts_with("application/json")
        })
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
    let mut response = relay_upstream(state, incoming, upstream, true, |manifest, final_url| {
        rewrite_managed_manifest(
            manifest,
            final_url,
            &resource.variables,
            |target, variables| {
                let resource_id = playback.register_resource_with_variables(
                    handle,
                    principal,
                    target,
                    resource.stream_index,
                    variables,
                )?;
                let mut parameters = url::form_urlencoded::Serializer::new(String::new());
                parameters
                    .append_pair("playback_handle", handle)
                    .append_pair("resource_id", &resource_id);
                if let Some(token) = token.filter(|_| !query.web) {
                    parameters.append_pair("token", token);
                }
                let path = if query.web {
                    "/stream-proxy"
                } else {
                    "/api/stream-proxy"
                };
                Ok(format!("{path}?{}", parameters.finish()))
            },
        )
    })
    .await?;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response.headers_mut().insert(
        axum::http::header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().remove(axum::http::header::ETAG);
    response
        .headers_mut()
        .remove(axum::http::header::LAST_MODIFIED);
    Ok(response)
}

fn managed_media_headers(
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
        if matches!(
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
        ) {
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

fn rewrite_managed_manifest(
    manifest: &str,
    base: &url::Url,
    parent_variables: &crate::services::playback_context::PlaybackVariables,
    register: impl Fn(
        url::Url,
        Arc<crate::services::playback_context::PlaybackVariables>,
    ) -> ApiResult<String>,
) -> ApiResult<String> {
    let manifest = manifest.strip_prefix('\u{feff}').unwrap_or(manifest);
    let variables = hls_variables::definitions(manifest, base, parent_variables)?;
    let rewrite_uri = |uri: &str| -> ApiResult<String> {
        let uri = hls_variables::expand_uri(uri, &variables)?;
        let target = base
            .join(&uri)
            .map_err(|_| ApiError::bad_request("Invalid media manifest URI"))?;
        if !matches!(target.scheme(), "http" | "https")
            || !target.username().is_empty()
            || target.password().is_some()
        {
            return Err(ApiError::bad_request("Unsupported media manifest URI"));
        }
        register(target, variables.clone())
    };
    let mut result = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            result.push_str(line);
            continue;
        }
        if trimmed.starts_with("#EXT-X-DEFINE:")
            || (trimmed.starts_with('#') && !trimmed.starts_with("#EXT"))
        {
            continue;
        }
        let output_start = result.len();
        if trimmed.starts_with('#') {
            let ranges = managed_uri_attributes(line)?;
            if carries_upstream_url(line, &ranges) {
                let tag = trimmed.split(':').next().unwrap_or_default();
                // A segment needs its EXTINF, so only the free-text title goes.
                if tag == "#EXTINF" {
                    let duration = trimmed
                        .split_once(':')
                        .map_or("", |(_, value)| value.split(',').next().unwrap_or(value));
                    result.push_str("#EXTINF:");
                    result.push_str(duration);
                    result.push_str(",\n");
                    continue;
                }
                // Vendor tags such as Twitch prefetch hints and non-URI attributes
                // such as DATERANGE X-...-URL name upstream media directly.
                // Players ignore tags they do not need, so the line is dropped.
                tracing::debug!(
                    tag,
                    "Dropped a managed manifest tag that names upstream media"
                );
                continue;
            }
            let mut copied = 0;
            for (start, end) in ranges {
                result.push_str(&line[copied..start]);
                result.push_str(&rewrite_uri(&line[start..end])?);
                copied = end;
            }
            result.push_str(&line[copied..]);
            if result[output_start..].contains("{$") {
                return Err(ApiError::bad_request(
                    "HLS variables outside URI fields are not supported for managed playback",
                ));
            }
        } else {
            result.push_str(&rewrite_uri(trimmed)?);
            if line.ends_with("\r\n") {
                result.push_str("\r\n");
            } else if line.ends_with('\n') {
                result.push('\n');
            }
        }
        if result.len() > MAX_MANIFEST_BYTES {
            return Err(ApiError::bad_request(
                "Rewritten media manifest is too large",
            ));
        }
    }
    Ok(result)
}

/// Checks the text outside the rewritten URI attributes.
fn carries_upstream_url(line: &str, uri_ranges: &[(usize, usize)]) -> bool {
    // Twitch prefetch tags carry a bare URI, including relative signed paths.
    // They are optional hints, not the segment URI lines the player requires.
    if line.trim_start().starts_with("#EXT-X-TWITCH-PREFETCH:")
        || line.trim_start().starts_with("#EXT-X-PREFETCH:")
    {
        return true;
    }
    let names_url = |text: &str| {
        text.contains("://")
            || text.contains("\"//")
            || text.contains("=//")
            || text.split(',').any(|attribute| {
                attribute.split_once('=').is_some_and(|(key, _)| {
                    let key = key.rsplit(':').next().unwrap_or(key).trim();
                    key == "URL" || key.ends_with("-URL")
                })
            })
    };
    let mut outside = 0;
    for &(start, end) in uri_ranges {
        if names_url(&line[outside..start]) {
            return true;
        }
        outside = end;
    }
    names_url(&line[outside..])
}

fn managed_uri_attributes(line: &str) -> ApiResult<Vec<(usize, usize)>> {
    let Some(colon) = line.find(':') else {
        return Ok(Vec::new());
    };
    if line[..colon].trim() == "#EXTINF" {
        return Ok(Vec::new());
    }
    let mut ranges = Vec::new();
    let mut start = colon + 1;
    let mut quoted = false;
    for (index, byte) in line
        .bytes()
        .enumerate()
        .skip(start)
        .chain(std::iter::once((line.len(), b',')))
    {
        if byte == b'"' {
            quoted = !quoted;
        }
        if byte != b',' || quoted {
            continue;
        }
        let attribute = &line[start..index];
        if let Some(equal) = attribute.find('=') {
            let key = attribute[..equal].trim();
            if key == "URI" || key.ends_with("-URI") || key == "X-ASSET-LIST" {
                let value = attribute[equal + 1..].trim();
                let Some(value) = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                else {
                    return Err(ApiError::bad_request(
                        "Invalid media manifest URI attribute",
                    ));
                };
                if value.contains('"') {
                    return Err(ApiError::bad_request(
                        "Invalid media manifest URI attribute",
                    ));
                }
                let leading =
                    attribute[equal + 1..].len() - attribute[equal + 1..].trim_start().len();
                let value_start = start + equal + 1 + leading + 1;
                ranges.push((value_start, value_start + value.len()));
            }
        }
        start = index + 1;
    }
    if quoted {
        return Err(ApiError::bad_request(
            "Unterminated media manifest attribute",
        ));
    }
    Ok(ranges)
}

async fn relay_upstream(
    state: &StreamProxyState,
    headers_in: &HeaderMap,
    upstream: reqwest::Response,
    strict_manifest: bool,
    rewrite: impl Fn(&str, &url::Url) -> ApiResult<String>,
) -> ApiResult<Response> {
    let status = upstream.status();
    let final_url = upstream.url().clone();
    let hls_content_type = is_hls_content_type(upstream.headers());

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
        let Some(chunk) = upstream_stream.try_next().await.map_err(|_| {
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
        while let Some(chunk) = upstream_stream.try_next().await.map_err(|_| {
            ApiError::new(
                StatusCode::BAD_GATEWAY,
                "BAD_GATEWAY",
                "Proxy response failed",
            )
        })? {
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
            let rewritten = rewrite(manifest, &final_url)?;
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
            if strict_manifest {
                return Err(ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "PLAYBACK_INVALID_MANIFEST",
                    "Upstream returned an invalid media manifest",
                ));
            }
            axum::body::Body::from(manifest_bytes.freeze())
        }
    } else {
        let prefix_stream =
            futures::stream::iter(initial_chunks.into_iter().map(Ok::<Bytes, std::io::Error>));
        let remaining_stream = upstream_stream.map_err(std::io::Error::other);
        axum::body::Body::from_stream(prefix_stream.chain(remaining_stream))
    };

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
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    async fn upstream_handler(req: HttpRequest<Body>) -> impl IntoResponse {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp2t"));
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));

        let status = if req
            .headers()
            .get(header::RANGE)
            .and_then(|range| range.to_str().ok())
            .is_some()
        {
            let value = HeaderValue::from_static("bytes 0-1/3");
            headers.insert(header::CONTENT_RANGE, value);
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        };

        (status, headers, "abc")
    }

    async fn hls_manifest_handler() -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
            concat!(
                "#EXTM3U\n",
                "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio/index.m3u8\"\n",
                "#EXT-X-STREAM-INF:BANDWIDTH=1000\n",
                "video/index.m3u8\n"
            ),
        )
    }

    fn build_query(pairs: &[(&str, &str)]) -> String {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in pairs {
            ser.append_pair(k, v);
        }
        ser.finish()
    }

    fn test_state(allow_private_targets: bool) -> StreamProxyState {
        StreamProxyState {
            playback: None,
            auth_service: None,
            source_config: None,
            proxy_config: ProxyConfig::disabled(),
            config_service: None,
            allow_private_targets,
            cors: CorsPolicy::AnyOrigin,
        }
    }

    #[test]
    fn managed_manifest_rewrites_every_uri_form_without_upstream_material() {
        let service = crate::services::playback_context::PlaybackContextService::default();
        let (source, snapshot, media) = crate::services::playback_context::test_bundle(
            "https://cdn.test/master.m3u8?secret=master",
        );
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let manifest = concat!(
            "#EXTM3U\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio.m3u8?secret=audio\"\n",
            "#EXT-X-I-FRAME-STREAM-INF:URI=\"iframe.m3u8?secret=iframe\"\n",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"https://other.test/key?secret=key\"\n",
            "#EXT-X-MAP:URI=\"init.mp4?secret=init\"\n",
            "#EXT-X-PART:DURATION=1,URI=\"part.ts?secret=part\"\n",
            "#EXT-X-PRELOAD-HINT:TYPE=PART,URI=\"next.ts?secret=next\"\n",
            "#EXT-X-RENDITION-REPORT:URI=\"other.m3u8?secret=report\"\n",
            "#EXT-X-CONTENT-STEERING:SERVER-URI=\"steering?secret=steer\"\n",
            "#EXT-X-TWITCH-PREFETCH:https://cdn.test/prefetch.ts?secret=prefetch\n",
            "#EXT-X-DATERANGE:ID=\"ad\",START-DATE=\"2026-01-01T00:00:00Z\",X-TV-TWITCH-AD-URL=\"https://ads.test/?secret=ad\"\n",
            "#EXT-X-SESSION-DATA:DATA-ID=\"poster\",VALUE=\"//cdn.test/poster?secret=poster\"\n",
            "video/index.m3u8?secret=nested\n",
            "#EXTINF:2.000,https://cdn.test/title?secret=title\n",
            "segment.ts?secret=segment\n",
        );
        let rewritten = rewrite_managed_manifest(
            manifest,
            &url::Url::parse("https://cdn.test/master.m3u8?secret=master").unwrap(),
            &Default::default(),
            |target, variables| {
                let resource = service.register_resource_with_variables(
                    &context.handle,
                    "alice",
                    target,
                    0,
                    variables,
                )?;
                Ok(format!(
                    "/api/stream-proxy?playback_handle={}&resource_id={resource}",
                    context.handle
                ))
            },
        )
        .unwrap();
        assert!(!rewritten.contains("secret="));
        assert!(!rewritten.contains("cdn.test"));
        assert!(!rewritten.contains("other.test"));
        assert!(!rewritten.contains("headers="));
        assert!(!rewritten.contains("ads.test"));
        assert!(!rewritten.contains("TWITCH-PREFETCH"));
        assert!(rewritten.contains("#EXT-X-MEDIA:"));
        assert!(rewritten.contains("#EXTINF:2.000,\n/api/stream-proxy?"));
        assert_eq!(rewritten.matches("resource_id=").count(), 10);
        assert!(
            rewrite_managed_manifest(
                "#EXTM3U\n#EXT-X-KEY:URI=unquoted-secret\n",
                &url::Url::parse("https://cdn.test/").unwrap(),
                &Default::default(),
                |_, _| Ok("opaque".into())
            )
            .is_err()
        );
    }

    #[test]
    fn managed_vendor_hints_never_expose_relative_signed_urls() {
        let rewritten = rewrite_managed_manifest(
            concat!(
                "#EXTM3U\n",
                "#EXT-X-TWITCH-PREFETCH:../next.ts?sig=private\n",
                "#EXT-X-PREFETCH:/next.ts?sig=private\n",
                "#EXT-X-DATERANGE:ID=\"ad\",X-ASSET-URL=\"ads.json?sig=private\"\n",
                "#EXT-X-DATERANGE:ID=\"other\",X-ASSET-URL=\"//cdn.test/ads.json?sig=private\"\n",
                "#EXT-X-TARGETDURATION:2\n#EXTINF:2,\nsegment.ts?sig=private\n",
            ),
            &url::Url::parse("https://cdn.test/live/index.m3u8").unwrap(),
            &Default::default(),
            |_, _| Ok("/opaque-resource".into()),
        )
        .unwrap();
        assert_eq!(
            rewritten,
            "#EXTM3U\n#EXT-X-TARGETDURATION:2\n#EXTINF:2,\n/opaque-resource\n"
        );
    }

    #[test]
    fn managed_variables_stay_server_side_across_nested_playlists_and_bom() {
        let service = crate::services::playback_context::PlaybackContextService::default();
        let (source, snapshot, media) = crate::services::playback_context::test_bundle(
            "https://cdn.test/master.m3u8?token=secret-query",
        );
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let base = url::Url::parse("https://cdn.test/master.m3u8?token=secret-query").unwrap();
        let registered = std::cell::RefCell::new(Vec::new());
        let master = rewrite_managed_manifest(
            "\u{feff}#EXTM3U\n# provider comment with secret-query\n#EXT-X-DEFINE:QUERYPARAM=\"token\"\n#EXT-X-DEFINE:NAME=\"base\",VALUE=\"https://cdn.test\"\n#EXT-X-STREAM-INF:BANDWIDTH=1000\n{$base}/child.m3u8?sig={$token}\n",
            &base, &Default::default(), |target, variables| {
                let id = service.register_resource_with_variables(&context.handle, "alice", target, 0, variables)?;
                registered.borrow_mut().push(id.clone());
                Ok(format!("/stream-proxy?resource_id={id}"))
            },
        ).unwrap();
        assert!(master.starts_with("#EXTM3U\n"));
        for secret in ["DEFINE", "secret-query", "cdn.test", "{$"] {
            assert!(!master.contains(secret));
        }
        let child = service
            .resource(&context.handle, "alice", &registered.borrow()[0])
            .unwrap();
        assert_eq!(
            child.target.as_str(),
            "https://cdn.test/child.m3u8?sig=secret-query"
        );
        let nested = rewrite_managed_manifest(
            "#EXTM3U\n#EXT-X-DEFINE:IMPORT=\"token\"\n#EXT-X-KEY:METHOD=AES-128,URI=\"key?sig={$token}\"\nsegment.ts?sig={$token}\n",
            &child.target, &child.variables, |target, variables| {
                let id = service.register_resource_with_variables(&context.handle, "alice", target, 0, variables)?;
                Ok(format!("/stream-proxy?resource_id={id}"))
            },
        ).unwrap();
        assert_eq!(nested.matches("resource_id=").count(), 2);
        assert!(!nested.contains("secret-query"));
        assert!(!nested.contains("DEFINE"));
        assert!(
            rewrite_managed_manifest(
                "#EXTM3U\n{$missing}/segment.ts\n",
                &base,
                &Default::default(),
                |_, _| Ok("unused".into())
            )
            .is_err()
        );
        assert!(
            rewrite_managed_manifest(
                "#EXTM3U\n#EXT-X-MEDIA:URI=\"safe\",X-ASSET-URI=unquoted-secret\n",
                &base,
                &Default::default(),
                |_, _| Ok("opaque".into())
            )
            .is_err()
        );
        assert!(rewrite_managed_manifest("#EXTM3U\n#EXT-X-DEFINE:NAME=\"secret\",VALUE=\"private\"\n#EXT-X-MEDIA:NAME=\"{$secret}\",URI=\"a.m3u8\"\n", &base, &Default::default(), |_, _| Ok("opaque".into())).is_err());
    }

    #[test]
    fn managed_resource_headers_keep_each_cdns_cookie_without_changing_the_account() {
        let service = crate::services::playback_context::PlaybackContextService::default();
        let (source, snapshot, mut media) =
            crate::services::playback_context::test_bundle("https://cdn.test/a.m3u8");
        let mut other = media.streams[0].clone();
        media.streams[0].extras = Some(
            serde_json::json!({"headers":{"Cookie":"session=stale; edge=cdn-a","Authorization":"Bearer cdn-a"}}),
        );
        other.extras = Some(
            serde_json::json!({"headers":{"Cookie":"session=stale; edge=cdn-b","Authorization":"Bearer cdn-b"}}),
        );
        media.streams.push(other);
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let data = service.get(&context.handle, "alice").unwrap();
        let a = managed_media_headers(&data, 0, &HeaderMap::new()).unwrap();
        let b = managed_media_headers(&data, 1, &HeaderMap::new()).unwrap();
        assert!(a[header::COOKIE].to_str().unwrap().contains("edge=cdn-a"));
        assert!(b[header::COOKIE].to_str().unwrap().contains("edge=cdn-b"));
        assert!(
            a[header::COOKIE]
                .to_str()
                .unwrap()
                .contains("session=secret-cookie")
        );
        assert!(
            b[header::COOKIE]
                .to_str()
                .unwrap()
                .contains("session=secret-cookie")
        );
        assert_eq!(a[header::AUTHORIZATION], "Bearer cdn-a");
        assert_eq!(b[header::AUTHORIZATION], "Bearer cdn-b");
        assert_eq!(data.snapshot.material.cookies, "session=secret-cookie");
    }

    #[tokio::test]
    async fn low_latency_hls_delivery_hints_are_typed_and_do_not_select_resources() {
        use axum::extract::FromRequestParts;
        let (mut parts, _) = HttpRequest::builder()
            .uri(
                "/?playback_handle=opaque&resource_id=resource&_HLS_msn=7&_HLS_part=1&_HLS_skip=v2",
            )
            .body(Body::empty())
            .unwrap()
            .into_parts();
        let Query(query) = Query::<StreamProxyQuery>::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(query.hls_msn, Some(7));
        assert_eq!(query.hls_part, Some(1));
        assert_eq!(query.resource_id.as_deref(), Some("resource"));
        for query in [
            "_HLS_msn=-1",
            "_HLS_part=no",
            "_HLS_skip=arbitrary",
            "_HLS_msn=18446744073709551616",
        ] {
            let (mut parts, _) = HttpRequest::builder()
                .uri(format!("/?{query}"))
                .body(Body::empty())
                .unwrap()
                .into_parts();
            assert!(
                Query::<StreamProxyQuery>::from_request_parts(&mut parts, &())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn managed_proxy_rejects_raw_mixtures_and_never_falls_back_after_expiry() {
        let app = super::router::<StreamProxyState>().with_state(test_state(true));
        for pairs in [
            vec![
                ("playback_handle", "handle"),
                ("resource_id", "resource"),
                ("url", "http://127.0.0.1/"),
            ],
            vec![
                ("playback_handle", "handle"),
                ("resource_id", "resource"),
                ("headers", "{}"),
            ],
        ] {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(format!("/?{}", build_query(&pairs)))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        let response = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/?playback_handle=expired&resource_id=resource")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn managed_redirect_cannot_forward_authentication_to_another_variant_origin() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let destination_url = format!("http://{}/segment", destination.local_addr().unwrap());
            let other_variant_url = destination_url.clone();
            let destination_app = Router::new().route(
                "/segment",
                get(|headers: HeaderMap| async move {
                    assert!(headers.get(header::COOKIE).is_none());
                    assert!(headers.get(header::AUTHORIZATION).is_none());
                    assert!(headers.get("x-provider-token").is_none());
                    "segment"
                }),
            );
            let destination_task = tokio::spawn(async move {
                axum::serve(destination, destination_app).await.unwrap();
            });
            let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let source_url = format!("http://{}/master", source.local_addr().unwrap());
            let source_app = Router::new().route(
                "/master",
                get(move |headers: HeaderMap| {
                    let location = destination_url.clone();
                    async move {
                        assert_eq!(
                            headers.get(header::COOKIE).unwrap(),
                            "session=secret-cookie"
                        );
                        (StatusCode::FOUND, [(header::LOCATION, location)])
                    }
                }),
            );
            let source_task = tokio::spawn(async move {
                axum::serve(source, source_app).await.unwrap();
            });
            let service = crate::services::playback_context::PlaybackContextService::default();
            let (source, snapshot, mut media) =
                crate::services::playback_context::test_bundle(&source_url);
            let mut other_variant = media.streams[0].clone();
            other_variant.url = other_variant_url;
            media.streams.push(other_variant);
            let context = service.insert("alice", source, snapshot, media).unwrap();
            let data = service.get(&context.handle, "alice").unwrap();
            let client = stream_proxy_client(true, &ProxyConfig::disabled()).unwrap();
            let mut headers = HeaderMap::new();
            headers.insert(
                header::COOKIE,
                HeaderValue::from_static("session=secret-cookie"),
            );
            headers.insert(
                header::AUTHORIZATION,
                HeaderValue::from_static("Bearer secret"),
            );
            headers.insert("x-provider-token", HeaderValue::from_static("secret"));
            let response = fetch_upstream_bound(
                &client,
                url::Url::parse(&source_url).unwrap(),
                &headers,
                true,
                Some((&data, 0)),
            )
            .await
            .unwrap();
            assert_eq!(response.text().await.unwrap(), "segment");
            source_task.abort();
            destination_task.abort();
        })
        .await
        .expect("redirect fixture must finish");
    }

    #[tokio::test]
    async fn managed_manifest_rejects_non_manifest_body_without_changing_legacy_relay() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target = format!("http://{}/manifest", listener.local_addr().unwrap());
            let app = Router::new().route(
                "/manifest",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                        "private-provider-sentinel",
                    )
                }),
            );
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            crate::utils::http_client::install_rustls_provider();
            let client = reqwest::Client::new();
            let state = test_state(true);
            let incoming = HeaderMap::new();
            let invalid = client.get(&target).send().await.unwrap();
            let error = relay_upstream(&state, &incoming, invalid, true, |_, _| unreachable!())
                .await
                .unwrap_err();
            assert_eq!(error.code, "PLAYBACK_INVALID_MANIFEST");
            assert!(!format!("{error:?}").contains("private-provider-sentinel"));
            let legacy = client.get(&target).send().await.unwrap();
            let response = relay_upstream(&state, &incoming, legacy, false, |_, _| unreachable!())
                .await
                .unwrap();
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(body.as_ref(), b"private-provider-sentinel");
            server.abort();
        })
        .await
        .expect("manifest fixture must finish");
    }

    #[tokio::test]
    async fn proxy_router_requires_full_access_and_honors_header_precedence() {
        use crate::database::models::ApiKeyAccessLevel;
        let fixture = crate::api::auth_request::tests::fixture().await;
        let (_, read) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "proxy-read",
                ApiKeyAccessLevel::ReadOnly,
                None,
            )
            .await
            .unwrap();
        let (_, full) = fixture
            .service
            .create_api_key(
                &fixture.user_id,
                "proxy-full",
                ApiKeyAccessLevel::Full,
                None,
            )
            .await
            .unwrap();
        let mut state = test_state(false);
        state.auth_service = Some(fixture.service.clone());
        let app = super::router::<StreamProxyState>().with_state(state);
        // An invalid scheme stops before any upstream request after authorization succeeds.
        for (token, authorization, expected) in [
            (read.as_str(), None, StatusCode::FORBIDDEN),
            (full.as_str(), None, StatusCode::BAD_REQUEST),
            (
                full.as_str(),
                Some("Basic invalid".to_owned()),
                StatusCode::UNAUTHORIZED,
            ),
            (
                full.as_str(),
                Some(format!("Bearer {read}")),
                StatusCode::FORBIDDEN,
            ),
            (
                "invalid",
                Some(format!("Bearer {full}")),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let query = build_query(&[("url", "file:///not-an-upstream"), ("token", token)]);
            let mut request = HttpRequest::builder().uri(format!("/?{query}"));
            if let Some(header) = authorization {
                request = request.header("Authorization", header);
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
        fixture.pool.close().await;
    }

    #[tokio::test]
    async fn proxy_forwards_range_and_sets_cors_headers() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new().route("/stream", get(upstream_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = test_state(true);

        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(state);

        let target = format!("http://{upstream_addr}/stream");
        let headers_json = r#"{"Referer":"https://example.com/"}"#;
        let query = build_query(&[
            ("url", &target),
            ("headers", headers_json),
            ("token", "unused-in-no-auth-mode"),
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
        let state = test_state(false);

        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(state);

        let query = build_query(&[("url", "file:///etc/passwd")]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_rewrites_hls_manifest_responses() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new().route("/live/master.m3u8", get(hls_manifest_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let state = test_state(true);
        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(state);
        let target = format!("http://{upstream_addr}/live/master.m3u8");
        let query = build_query(&[
            ("url", &target),
            ("headers", r#"{"Referer":"https://source.example/"}"#),
            ("token", "desktop-token"),
        ]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::CONTENT_RANGE).is_none());
        assert!(response.headers().get(header::ETAG).is_none());
        let content_length = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());
        let body = axum::body::to_bytes(response.into_body(), MAX_MANIFEST_BYTES)
            .await
            .unwrap();
        assert_eq!(content_length, Some(body.len()));
        let body = std::str::from_utf8(&body).unwrap();
        assert_eq!(body.matches("/api/stream-proxy?").count(), 2);
        assert!(body.contains("token=desktop-token"));
        assert!(body.contains("headers=%7B%22Referer%22"));
        assert!(body.contains("%2Flive%2Fvideo%2Findex.m3u8"));
    }

    #[test]
    fn rewrites_hls_resource_uris_without_changing_line_endings() {
        let manifest = concat!(
            "#EXTM3U\r\n",
            "#EXT-X-MEDIA:TYPE=AUDIO,URI=\"audio/index.m3u8\"\r\n",
            "#EXT-X-STREAM-INF:BANDWIDTH=1000\r\n",
            "video/index.m3u8\r\n",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"../key.bin\"\r\n",
            "#EXT-X-MAP:URI = \"init.mp4\"\r\n",
            "#EXT-X-PART:DURATION=0.333,URI=\"part.ts\"\r\n",
            "segment.ts"
        );
        let base_url = url::Url::parse("https://media.example/live/master.m3u8").unwrap();
        let rewritten = rewrite_hls_manifest(
            manifest,
            &base_url,
            RelayContext {
                headers: Some(r#"{"Referer":"https://source.example/"}"#),
                token: Some("desktop-token"),
                source_url: Some("https://source.example/channel"),
                web: false,
            },
        );

        assert_eq!(rewritten.matches("/api/stream-proxy?").count(), 6);
        assert!(rewritten.contains("url=https%3A%2F%2Fmedia.example%2Flive%2Faudio%2Findex.m3u8"));
        assert!(rewritten.contains("url=https%3A%2F%2Fmedia.example%2Fkey.bin"));
        assert!(
            rewritten
                .contains("headers=%7B%22Referer%22%3A%22https%3A%2F%2Fsource.example%2F%22%7D")
        );
        assert!(rewritten.contains("token=desktop-token"));
        assert_eq!(rewritten.matches("\r\n").count(), 7);
        assert!(!rewritten.ends_with('\n'));
    }

    #[tokio::test]
    async fn rejects_private_and_link_local_targets() {
        for target in [
            "http://127.0.0.1/stream",
            "http://10.0.0.1/stream",
            "http://169.254.169.254/latest/meta-data",
            "http://[::1]/stream",
            "http://[fd00::1]/stream",
        ] {
            let target = url::Url::parse(target).unwrap();
            let error = validate_target_url(&target, false).await.unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn validate_allows_private_targets_when_enabled() {
        let target = url::Url::parse("http://localhost:8080/stream").unwrap();
        assert!(validate_target_url(&target, true).await.is_ok());
        assert!(validate_target_url(&target, false).await.is_err());
    }

    #[tokio::test]
    async fn public_address_resolver_rejects_private_hosts() {
        // "localhost" resolves to loopback everywhere, so the resolver must
        // refuse to hand any address to the connector.
        assert!(resolve_public_addresses("localhost").await.is_err());
    }

    async fn redirect_to_manifest_handler() -> impl IntoResponse {
        (StatusCode::FOUND, [(header::LOCATION, "/live/master.m3u8")])
    }

    async fn redirect_loop_handler() -> impl IntoResponse {
        (StatusCode::FOUND, [(header::LOCATION, "/redirect-loop")])
    }

    async fn redirect_with_credentials_handler() -> impl IntoResponse {
        (
            StatusCode::FOUND,
            [(header::LOCATION, "http://user:pass@media.example/stream")],
        )
    }

    #[tokio::test]
    async fn proxy_follows_redirects_and_rewrites_against_final_url() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new()
            .route("/redirect", get(redirect_to_manifest_handler))
            .route("/live/master.m3u8", get(hls_manifest_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(test_state(true));
        let target = format!("http://{upstream_addr}/redirect");
        let query = build_query(&[("url", &target)]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), MAX_MANIFEST_BYTES)
            .await
            .unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        // Relative manifest URIs must resolve against the post-redirect URL,
        // not the target the client originally requested.
        assert_eq!(body.matches("/api/stream-proxy?").count(), 2);
        assert!(body.contains("%2Flive%2Fvideo%2Findex.m3u8"));
    }

    #[tokio::test]
    async fn proxy_validates_every_redirect_hop() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new().route("/redirect", get(redirect_with_credentials_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        // The credential check applies even with allow_private_targets, so a
        // rejected hop proves validate_target_url ran on the redirect target.
        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(test_state(true));
        let target = format!("http://{upstream_addr}/redirect");
        let query = build_query(&[("url", &target)]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn proxy_caps_upstream_redirects() {
        let upstream_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let upstream_app = Router::new().route("/redirect-loop", get(redirect_loop_handler));
        tokio::spawn(async move {
            axum::serve(upstream_listener, upstream_app).await.unwrap();
        });

        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(test_state(true));
        let target = format!("http://{upstream_addr}/redirect-loop");
        let query = build_query(&[("url", &target)]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn web_manifest_links_keep_source_identity_without_the_session_token() {
        let base = url::Url::parse("https://cdn.example/live/master.m3u8").unwrap();
        let source = "https://live.bilibili.com/1";
        let output = rewrite_hls_manifest(
            "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\nchild/index.m3u8\nsegment.ts\n",
            &base,
            RelayContext {
                source_url: Some(source),
                token: Some("backend-session-secret"),
                web: true,
                ..Default::default()
            },
        );
        assert_eq!(output.matches("/stream-proxy?").count(), 3);
        assert_eq!(
            output
                .matches("source_url=https%3A%2F%2Flive.bilibili.com%2F1")
                .count(),
            3
        );
        assert!(!output.contains("backend-session-secret"));
        assert!(!output.contains("token="));
        assert!(!output.contains("/api/stream-proxy"));
    }

    async fn proxy_fixture(req: HttpRequest<Body>) -> impl IntoResponse {
        assert_eq!(
            req.headers().get(header::PROXY_AUTHORIZATION).unwrap(),
            "Basic dXNlcjpwYXNz"
        );
        assert_eq!(req.uri().host(), Some("8.8.8.8"));
        if req.uri().path() == "/redirect" {
            return (
                StatusCode::FOUND,
                [(header::LOCATION, "http://127.0.0.1/private")],
            )
                .into_response();
        }
        if req.uri().path().ends_with(".m3u8") {
            return (
                [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                "#EXTM3U\nsegment.ts\n",
            )
                .into_response();
        }
        assert_eq!(req.headers().get(header::RANGE).unwrap(), "bytes=0-1");
        (
            StatusCode::PARTIAL_CONTENT,
            [(header::CONTENT_RANGE, "bytes 0-1/3")],
            "ab",
        )
            .into_response()
    }

    #[tokio::test]
    async fn configured_proxy_routes_media_and_still_rejects_private_redirects() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(proxy_fixture))
                .await
                .unwrap();
        });
        let mut state = test_state(false);
        state.proxy_config =
            ProxyConfig::with_url(format!("http://{address}")).with_auth("user", "pass");
        let app = Router::new()
            .nest("/api/stream-proxy", super::router::<StreamProxyState>())
            .with_state(state);
        let query = build_query(&[
            ("url", "http://8.8.8.8/live.m3u8"),
            ("source_url", "https://live.bilibili.com/1"),
            ("web", "true"),
            ("token", "backend-secret"),
        ]);
        let request = HttpRequest::builder()
            .uri(format!("/api/stream-proxy?{query}"))
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), app.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "private, no-store"
        );
        let bytes = axum::body::to_bytes(response.into_body(), MAX_MANIFEST_BYTES)
            .await
            .unwrap();
        let manifest = std::str::from_utf8(&bytes).unwrap();
        assert!(!manifest.contains("backend-secret"));
        let segment = manifest
            .lines()
            .find(|line| line.starts_with("/stream-proxy?"))
            .unwrap();
        let segment_url = url::Url::parse(&format!("https://app.example{segment}")).unwrap();
        assert_eq!(
            segment_url
                .query_pairs()
                .find(|(key, _)| key == "source_url")
                .unwrap()
                .1,
            "https://live.bilibili.com/1"
        );
        let request = HttpRequest::builder()
            .uri(format!("/api{segment}&web=true"))
            .header(header::RANGE, "bytes=0-1")
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(5), app.clone().oneshot(request))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 0-1/3"
        );
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap(),
            "ab"
        );
        for target in ["http://127.0.0.1/private", "http://8.8.8.8/redirect"] {
            let query = build_query(&[("url", target)]);
            let request = HttpRequest::builder()
                .uri(format!("/api/stream-proxy?{query}"))
                .body(Body::empty())
                .unwrap();
            let response =
                tokio::time::timeout(Duration::from_secs(5), app.clone().oneshot(request))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn invalid_upstream_proxy_fails_without_falling_back_to_direct() {
        let error =
            stream_proxy_client(false, &ProxyConfig::with_url("http://[invalid")).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn changed_proxy_configuration_selects_a_new_client_pool() {
        let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_url = format!("http://{}", first.local_addr().unwrap());
        let second_url = format!("http://{}", second.local_addr().unwrap());
        let first_server = tokio::spawn(async move {
            axum::serve(first, Router::new().fallback(|| async { "first" }))
                .await
                .unwrap();
        });
        let second_server = tokio::spawn(async move {
            axum::serve(second, Router::new().fallback(|| async { "second" }))
                .await
                .unwrap();
        });
        for (url, expected) in [(first_url, "first"), (second_url, "second")] {
            let client = stream_proxy_client(false, &ProxyConfig::with_url(url)).unwrap();
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                fetch_upstream(
                    &client,
                    url::Url::parse("http://8.8.8.8/stream").unwrap(),
                    &HeaderMap::new(),
                    false,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(response.text().await.unwrap(), expected);
        }
        first_server.abort();
        second_server.abort();
        assert!(first_server.await.unwrap_err().is_cancelled());
        assert!(second_server.await.unwrap_err().is_cancelled());
    }
}
