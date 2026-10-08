use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use platforms_parser::proxy::{ProxyTarget, redacted_url, unroutable_proxy};

use crate::proxies::ProxyError;
use tracing::{debug, warn};

pub fn install_rustls_provider() {
    static PROVIDER_INSTALLED: OnceLock<()> = OnceLock::new();
    PROVIDER_INSTALLED.get_or_init(|| {
        if let Err(e) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
            // Safe to ignore: can happen if another crate installed it first.
            debug!(existing_provider = ?e, "rustls CryptoProvider already installed");
        }
    });
}

/// Routes `builder`'s requests through `proxy`. A direct target ignores the
/// proxy environment variables too; the system target leaves reqwest's own
/// environment lookup on. An explicit proxy reqwest cannot use makes every
/// request fail rather than leave without it.
pub fn apply_proxy(builder: reqwest::ClientBuilder, proxy: &ProxyTarget) -> reqwest::ClientBuilder {
    match proxy {
        ProxyTarget::Direct => builder.no_proxy(),
        ProxyTarget::System => builder,
        ProxyTarget::Explicit(endpoint) => match endpoint.reqwest_proxy() {
            Ok(proxy) => builder.proxy(proxy),
            Err(error) => {
                // Only the address's scheme, host and port are logged.
                warn!(
                    error = %error.without_url(),
                    proxy = %redacted_url(&endpoint.url),
                    "Invalid proxy URL; requests through it will fail"
                );
                builder.proxy(unroutable_proxy())
            }
        },
    }
}

/// Clients kept per connection setting, oldest dropped first: each edited
/// proxy login leaves a client behind.
pub(crate) struct ClientCache<K> {
    capacity: usize,
    clients: Mutex<VecDeque<(K, reqwest::Client)>>,
}

impl<K: Clone + Eq> ClientCache<K> {
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            clients: Mutex::new(VecDeque::new()),
        }
    }

    /// The cached client for `key`, else the one `build` makes, which is kept
    /// when it builds.
    pub(crate) fn get_or_try_build<E>(
        &self,
        key: &K,
        build: impl FnOnce() -> Result<reqwest::Client, E>,
    ) -> Result<reqwest::Client, E> {
        if let Some(client) = self.find(key) {
            return Ok(client);
        }
        let client = build()?;
        let mut clients = self.lock();
        if !clients.iter().any(|(cached, _)| cached == key) {
            if clients.len() >= self.capacity {
                clients.pop_front();
            }
            clients.push_back((key.clone(), client.clone()));
        }
        Ok(client)
    }

    fn find(&self, key: &K) -> Option<reqwest::Client> {
        self.lock()
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, client)| client.clone())
    }

    /// A panic while the lock was held cannot leave an entry half written,
    /// so a poisoned cache stays usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<(K, reqwest::Client)>> {
        self.clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Build a `reqwest::Client` configured like `platforms-parser`'s default client,
/// but with rust-srec proxy semantics applied. Fails when no client on the
/// route can be built: a default client would ignore the route.
pub fn build_platforms_client(
    proxy: &ProxyTarget,
    request_timeout: Duration,
    pool_max_idle_per_host: usize,
) -> Result<reqwest::Client, ProxyError> {
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

/// Builds `builder`, reporting a failure without its details, which may
/// quote a proxy address.
pub(crate) fn build_client(builder: reqwest::ClientBuilder) -> Result<reqwest::Client, ProxyError> {
    builder.build().map_err(|error| {
        warn!(error = %error.without_url(), "Could not build an HTTP client");
        ProxyError::ClientUnavailable
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_keeps_the_newest_clients() {
        install_rustls_provider();
        let cache = ClientCache::new(2);
        let builds = std::cell::Cell::new(0);
        let client = |key: u8| {
            cache
                .get_or_try_build(&key, || {
                    builds.set(builds.get() + 1);
                    Ok::<_, ()>(reqwest::Client::new())
                })
                .unwrap()
        };
        client(1);
        client(2);
        client(1);
        assert_eq!(builds.get(), 2);
        client(3);
        client(2);
        assert_eq!(builds.get(), 3);
        client(1);
        assert_eq!(builds.get(), 4);
        let failed: Result<_, &str> = cache.get_or_try_build(&9, || Err("no client"));
        assert!(failed.is_err());
        client(9);
        assert_eq!(builds.get(), 5);
    }
}
