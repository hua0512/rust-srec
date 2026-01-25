use std::{sync::OnceLock, time::Duration};

use tracing::{debug, warn};

use crate::domain::ProxyConfig;

pub fn install_rustls_provider() {
    static PROVIDER_INSTALLED: OnceLock<()> = OnceLock::new();
    PROVIDER_INSTALLED.get_or_init(|| {
        if let Err(e) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
            // Safe to ignore: can happen if another crate installed it first.
            debug!(existing_provider = ?e, "rustls CryptoProvider already installed");
        }
    });
}

/// Apply `proxy_config` to an existing `reqwest::ClientBuilder`.
///
/// Behavior matches rust-srec's download proxy semantics:
/// - `enabled = false` => disable all proxy (including env/system)
/// - `enabled = true` + `url = Some(..)` => use explicit proxy (optionally with auth)
/// - `enabled = true` + `url = None` + `use_system_proxy = true` => use system/env proxy defaults
/// - `enabled = true` + `url = None` + `use_system_proxy = false` => disable all proxy
pub fn apply_proxy_config(
    mut builder: reqwest::ClientBuilder,
    proxy_config: &ProxyConfig,
) -> reqwest::ClientBuilder {
    if !proxy_config.enabled {
        return builder.no_proxy();
    }

    if let Some(url) = proxy_config.url.as_deref() {
        match reqwest::Proxy::all(url) {
            Ok(mut proxy) => {
                if let (Some(username), Some(password)) = (
                    proxy_config.username.as_ref(),
                    proxy_config.password.as_ref(),
                ) {
                    proxy = proxy.basic_auth(username, password);
                }
                builder = builder.proxy(proxy);
            }
            Err(error) => {
                warn!(
                    proxy_url = %url,
                    error = %error,
                    "Invalid proxy URL; disabling proxy"
                );
                builder = builder.no_proxy();
            }
        }
        return builder;
    }

    if proxy_config.use_system_proxy {
        // reqwest default behavior (no `no_proxy()` call) uses system/env proxy settings.
        return builder;
    }

    // Proxy "enabled" but neither explicit URL nor system proxy => no proxy.
    builder.no_proxy()
}

/// Build a `reqwest::Client` configured like `platforms-parser`'s default client,
/// but with rust-srec proxy semantics applied.
pub fn build_platforms_client(
    proxy_config: &ProxyConfig,
    request_timeout: Duration,
    pool_max_idle_per_host: usize,
) -> reqwest::Client {
    install_rustls_provider();

    let mut builder = platforms_parser::extractor::create_client_builder(None);

    if request_timeout > Duration::ZERO {
        builder = builder.timeout(request_timeout);
    }

    if pool_max_idle_per_host > 0 {
        builder = builder.pool_max_idle_per_host(pool_max_idle_per_host);
    }

    builder = apply_proxy_config(builder, proxy_config);

    builder.build().unwrap_or_else(|error| {
        warn!(
            error = %error,
            "Failed to create HTTP client via platforms-parser; falling back to reqwest defaults"
        );

        // Best-effort: preserve "no proxy" semantics when requested.
        if !proxy_config.enabled || (!proxy_config.use_system_proxy && proxy_config.url.is_none()) {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        } else {
            reqwest::Client::new()
        }
    })
}
