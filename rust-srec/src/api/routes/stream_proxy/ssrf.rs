//! Outbound target validation, DNS-rebinding defense, and HTTP client construction.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::lookup_host;

use crate::api::error::{ApiError, ApiResult};
use crate::proxies::{ProxyTarget, SystemProxy};
use crate::utils::http_client::ClientCache;

pub(super) fn stream_proxy_client(
    allow_private_targets: bool,
    upstream_proxy: &ProxyTarget,
) -> ApiResult<reqwest::Client> {
    // Bounded: different streamers may use different proxies. reqwest clients
    // are cheap to clone and old streams keep their own pool alive.
    static CLIENTS: ClientCache<(bool, ProxyTarget)> = ClientCache::new(16);
    CLIENTS.get_or_try_build(&(allow_private_targets, upstream_proxy.clone()), || {
        crate::utils::http_client::install_rustls_provider();
        // Never impose a whole-response timeout on continuous FLV/MPEG-TS streams.
        // HTTP/1.1 like the other backend clients (see `build_client`); a
        // continuous stream relayed over HTTP/2 would also need adaptive flow
        // control to keep up.
        let mut builder = reqwest::Client::builder()
            .http1_only()
            .connect_timeout(Duration::from_secs(10))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(20)
            .redirect(reqwest::redirect::Policy::none());
        if let ProxyTarget::Explicit(endpoint) = upstream_proxy {
            // Playback fails closed rather than send media outside the
            // chosen route.
            let proxy = endpoint
                .reqwest_proxy()
                .map_err(|_| ApiError::bad_request("Invalid upstream proxy configuration"))?;
            builder = builder.proxy(proxy);
        } else if connects_directly(upstream_proxy, SystemProxy::current()) {
            builder = builder.no_proxy();
            if !allow_private_targets {
                builder = builder.dns_resolver(Arc::new(PublicAddressResolver));
            }
        }
        // An operator-configured proxy is a trusted egress endpoint and may
        // itself live on a private network. Target/redirect validation still
        // happens before every request; remote DNS and routing are enforced
        // by that proxy.
        builder
            .build()
            .map_err(|_| ApiError::internal("Stream proxy HTTP client is unavailable"))
    })
}

/// Whether the stream proxy connects without a proxy, and so must resolve
/// targets through `PublicAddressResolver` itself.
///
/// `reqwest` is built without the `system-proxy` feature, so `ProxyTarget::System`
/// only consults the environment variables tracked by [`SystemProxy`]; when none
/// are set, connections dial targets directly on every OS and must use
/// `PublicAddressResolver`.
pub(super) fn connects_directly(upstream_proxy: &ProxyTarget, system: &SystemProxy) -> bool {
    match upstream_proxy {
        ProxyTarget::Direct => true,
        ProxyTarget::System => !system.detected(),
        ProxyTarget::Explicit(_) => false,
    }
}

pub(super) fn is_public_ipv4(address: Ipv4Addr) -> bool {
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

/// Global unicast (`2000::/3`) outside the documentation range. A 6to4 or
/// Teredo address embeds the IPv4 addresses a host with such a tunnel sends
/// its packets to, so it is only as public as those addresses. The other
/// forms that embed an IPv4 address (IPv4-mapped, IPv4-compatible and NAT64)
/// lie outside `2000::/3` and are refused outright.
pub(super) fn is_public_ipv6(address: Ipv6Addr) -> bool {
    let [s0, s1, s2, s3, _, _, s6, s7] = address.segments();
    let ipv4 = |high: u16, low: u16| Ipv4Addr::from((u32::from(high) << 16) | u32::from(low));
    match (s0, s1) {
        (0x2001, 0x0db8) => false,
        // Teredo: the server's address, then the client's, inverted.
        (0x2001, 0x0000) => is_public_ipv4(ipv4(s2, s3)) && is_public_ipv4(ipv4(!s6, !s7)),
        // 6to4: the tunnel endpoint's address follows the prefix.
        (0x2002, _) => is_public_ipv4(ipv4(s1, s2)),
        _ => (s0 & 0xe000) == 0x2000,
    }
}

pub(super) fn is_public_ip(address: IpAddr) -> bool {
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
pub(super) async fn resolve_public_addresses(
    host: &str,
) -> std::io::Result<Vec<std::net::SocketAddr>> {
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

pub(in crate::api::routes) async fn validate_target_url(
    target: &url::Url,
    allow_private_targets: bool,
) -> ApiResult<()> {
    match target.scheme() {
        "http" | "https" => {}
        _ => return Err(ApiError::bad_request("Only http/https URLs are allowed")),
    }
    if !target.username().is_empty() || target.password().is_some() {
        return Err(ApiError::bad_request("URL credentials are not allowed"));
    }

    let host = target
        .host()
        .ok_or_else(|| ApiError::bad_request("Target host is required"))?;
    if allow_private_targets {
        // `stream_proxy_allow_private_targets` opts the operator into LAN and
        // localhost sources, so only the scheme/credential checks above apply.
        return Ok(());
    }

    // The connector dials an IP literal without a lookup, so the literal's
    // own address is the one to check. The URL parser has already decoded
    // alternative IPv4 spellings such as `127.1` into `Host::Ipv4`.
    let domain = match host {
        url::Host::Domain(domain) => domain,
        url::Host::Ipv4(address) if is_public_ipv4(address) => return Ok(()),
        url::Host::Ipv6(address) if is_public_ipv6(address) => return Ok(()),
        url::Host::Ipv4(_) | url::Host::Ipv6(_) => {
            return Err(ApiError::bad_request("Target host is not allowed"));
        }
    };

    let normalized_host = domain.trim_end_matches('.');
    if normalized_host.eq_ignore_ascii_case("localhost")
        || normalized_host.to_ascii_lowercase().ends_with(".localhost")
    {
        return Err(ApiError::bad_request("Target host is not allowed"));
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
