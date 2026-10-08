//! Clients for the provider calls made for accounts.
//!
//! Each call goes through the route resolved for it, so a check, refresh or
//! sign-in reaches the platform from the address the account uses.

use platforms_parser::proxy::ProxyTarget;

use crate::proxies::ProxyError;
use crate::utils::http_client::{ClientCache, apply_proxy, build_client, install_rustls_provider};

/// The client provider calls use on `proxy`.
pub(crate) fn provider_client(proxy: &ProxyTarget) -> Result<reqwest::Client, ProxyError> {
    static CLIENTS: ClientCache<ProxyTarget> = ClientCache::new(64);
    CLIENTS.get_or_try_build(proxy, || {
        install_rustls_provider();
        build_client(apply_proxy(reqwest::Client::builder(), proxy))
    })
}
