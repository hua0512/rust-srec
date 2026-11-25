//! Conversions from pure domain proxy types to `platforms-parser` proxy,
//! danmu tunnel, and extractor HTTP client types.

use std::time::Duration;

use platforms_parser::danmaku::{DanmakuError, DanmuProxy};
use tracing::warn;

use srec_core::domain::proxy::{
    ProxyEndpoint, ProxyError, ProxyTarget, ResolvedRoute, SystemProxy,
};
use srec_core::utils::http_client::{apply_proxy, build_client, install_rustls_provider};

/// Convert a domain [`ProxyEndpoint`] into a `platforms_parser::proxy::ProxyEndpoint`.
pub fn to_platforms_endpoint(endpoint: &ProxyEndpoint) -> platforms_parser::proxy::ProxyEndpoint {
    platforms_parser::proxy::ProxyEndpoint {
        url: endpoint.url.clone(),
        username: endpoint.username.clone(),
        password: endpoint.password.clone(),
    }
}

/// Convert a domain [`ProxyTarget`] into a `platforms_parser::proxy::ProxyTarget`.
pub fn to_platforms_target(target: &ProxyTarget) -> platforms_parser::proxy::ProxyTarget {
    match target {
        ProxyTarget::Direct => platforms_parser::proxy::ProxyTarget::Direct,
        ProxyTarget::System => platforms_parser::proxy::ProxyTarget::System,
        ProxyTarget::Explicit(endpoint) => {
            platforms_parser::proxy::ProxyTarget::Explicit(to_platforms_endpoint(endpoint))
        }
    }
}

/// The danmu tunnel for the environment's proxy; `None` when none is set.
pub fn system_danmu_proxy(
    system: &SystemProxy,
) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
    system
        .primary_url()
        .map(|url| {
            DanmuProxy::parse(url)
                .map(|proxy| proxy.with_no_proxy(system.no_proxy().map(str::to_owned)))
        })
        .transpose()
}

/// The tunnel raw connections (danmu, Bigo's native stream) take on `target`;
/// `None` connects directly.
pub fn danmu_proxy_for(
    system: &SystemProxy,
    target: &ProxyTarget,
) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
    match target {
        ProxyTarget::Direct => Ok(None),
        ProxyTarget::System => system_danmu_proxy(system),
        ProxyTarget::Explicit(endpoint) => {
            DanmuProxy::new(to_platforms_endpoint(endpoint)).map(Some)
        }
    }
}

/// The proxy danmu connections use for a resolved route.
pub fn danmu_proxy_for_route(
    route: &ResolvedRoute,
    system: &SystemProxy,
) -> std::result::Result<Option<DanmuProxy>, DanmakuError> {
    danmu_proxy_for(system, &route.target)
}

/// Build a `reqwest::Client` configured like `platforms-parser`'s default client,
/// with rust-srec proxy semantics applied. Fails when no client on the route
/// can be built: a default client would ignore the route.
pub fn build_platforms_client(
    proxy: &ProxyTarget,
    request_timeout: Duration,
    pool_max_idle_per_host: usize,
) -> std::result::Result<reqwest::Client, ProxyError> {
    install_rustls_provider();

    let mut builder = platforms_parser::extractor::create_client_builder(None);

    if request_timeout > Duration::ZERO {
        builder = builder.timeout(request_timeout);
    }

    if pool_max_idle_per_host > 0 {
        builder = builder.pool_max_idle_per_host(pool_max_idle_per_host);
    }

    builder = apply_proxy(builder, proxy);

    builder.build().or_else(|error| {
        warn!(
            error = %error,
            "Failed to create HTTP client via platforms-parser; falling back to reqwest defaults"
        );
        // The fallback keeps the route.
        build_client(apply_proxy(reqwest::Client::builder(), proxy))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use srec_core::domain::proxy::RouteSource;

    #[test]
    fn danmu_follows_the_route() {
        let none = SystemProxy::default();
        let set = SystemProxy::from_lookup(|name| match name {
            "HTTPS_PROXY" => Some("proxy.example:3128".to_owned()),
            "NO_PROXY" => Some("internal.example".to_owned()),
            _ => None,
        });
        assert!(
            danmu_proxy_for_route(&ResolvedRoute::direct(RouteSource::Global), &set)
                .unwrap()
                .is_none()
        );
        let system = ResolvedRoute::system(RouteSource::Global, &set);
        assert!(danmu_proxy_for_route(&system, &none).unwrap().is_none());
        let proxy = danmu_proxy_for_route(&system, &set).unwrap().unwrap();
        assert!(proxy.bypasses("chat.internal.example"));
        assert!(!proxy.bypasses("danmu.example"));

        let explicit = ResolvedRoute {
            target: ProxyTarget::Explicit(ProxyEndpoint::new(
                "socks5h://proxy.example:1080",
                Some(("user".into(), "p@ss".into())),
            )),
            ..ResolvedRoute::default()
        };
        let proxy = danmu_proxy_for_route(&explicit, &none).unwrap().unwrap();
        assert!(!proxy.bypasses("danmu.example"));
        assert_eq!(
            format!("{proxy:?}"),
            "DanmuProxy(socks5h://proxy.example:1080)"
        );
    }
}
