//! Stream proxy routes.
//!
//! Desktop builds do not run a TanStack Start server, so the frontend cannot rely on
//! `/stream-proxy` server handlers. This route provides an authenticated proxy under
//! `/api/stream-proxy` that can forward media requests with custom headers and Range
//! support.

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
    let mut target = initial_target;

    for redirect_count in 0..=MAX_REDIRECTS {
        validate_target_url(&target, allow_private_targets).await?;
        let response = client
            .get(target.clone())
            .headers(headers.clone())
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
        if !matches!(preceding, Some(':') | Some(',')) {
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

#[derive(Debug, Deserialize)]
pub struct StreamProxyQuery {
    pub url: String,
    pub headers: Option<String>,
    pub token: Option<String>,
    pub source_url: Option<String>,
    /// Web BFF requests cookie-authenticated links without exposing its bearer token.
    #[serde(default)]
    pub web: bool,
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

    crate::api::auth_request::authorize_request(
        state.auth_service.as_ref(),
        headers_in,
        query.token.as_deref(),
        crate::api::auth_request::AccessPolicy::Full,
    )
    .await?;
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
    let target =
        url::Url::parse(&query.url).map_err(|_| ApiError::bad_request("Invalid url parameter"))?;

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

    let source_url = query.source_url.as_deref().unwrap_or(&query.url);
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
            let rewritten = rewrite_hls_manifest(
                manifest,
                &final_url,
                RelayContext {
                    headers: query.headers.as_deref(),
                    token: relay_token,
                    source_url: Some(source_url),
                    web: query.web,
                },
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
            auth_service: None,
            source_config: None,
            proxy_config: ProxyConfig::disabled(),
            config_service: None,
            allow_private_targets,
            cors: CorsPolicy::AnyOrigin,
        }
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
