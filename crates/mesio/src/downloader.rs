use reqwest::Client;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, OnceLock};
use tracing::{debug, info};

use crate::DownloaderConfig;
use crate::config::HttpVersionPreference;
use crate::rate_limit::RateLimiter;
use crate::{DownloadError, proxy::build_proxy_from_config};

/// Create a reqwest Client with the provided configuration.
///
/// The HTTP version follows [`DownloaderConfig::http_version`]; see
/// [`HttpVersionPreference`] for what each choice means on the wire.
pub fn create_client(config: &DownloaderConfig) -> Result<Client, DownloadError> {
    create_client_with_backend(config, TlsBackend::Rustls)
}

pub(crate) const ENV_NATIVE_TLS_HOSTS: &str = "RUST_SREC_NATIVE_TLS_HOSTS";

fn install_rustls_provider() {
    // Select the same provider before creating shared reqwest and rustls clients.
    static PROVIDER_INSTALLED: OnceLock<()> = OnceLock::new();
    PROVIDER_INSTALLED.get_or_init(|| {
        if let Err(e) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
            debug!(existing_provider = ?e, "rustls CryptoProvider already installed");
        }
    });
}

fn default_native_tls_hosts() -> Vec<String> {
    // Douyu CDN endpoints frequently terminate TLS with RSA key exchange only.
    vec!["edgesrv.com".to_string()]
}

fn native_tls_hosts_from_env() -> Vec<String> {
    let Ok(raw) = std::env::var(ENV_NATIVE_TLS_HOSTS) else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn host_matches_entry(host: &str, entry: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }

    if host.eq_ignore_ascii_case(entry) {
        return true;
    }

    // Suffix match: "edgesrv.com" matches "stream-foo.edgesrv.com".
    // Also support leading-dot entries like ".edgesrv.com".
    let normalized = entry.strip_prefix('.').unwrap_or(entry);
    host.len() > normalized.len()
        && host
            .to_ascii_lowercase()
            .ends_with(&format!(".{normalized}").to_ascii_lowercase())
}

#[derive(Debug, Clone)]
pub struct ClientPool {
    rustls: Client,
    #[cfg(feature = "tls-native-fallback")]
    native: Client,
    native_hosts: Vec<String>,
    /// Shared by every request made through the pool, so rate limiting seen
    /// by one request slows all of them down (see [`crate::rate_limit`]).
    rate_limiter: Arc<RateLimiter>,
}

impl ClientPool {
    pub fn new(config: &DownloaderConfig) -> Result<Self, DownloadError> {
        let rustls = create_client_with_backend(config, TlsBackend::Rustls)?;

        let mut native_hosts = default_native_tls_hosts();
        native_hosts.extend(native_tls_hosts_from_env());

        #[cfg(feature = "tls-native-fallback")]
        let native = create_client_with_backend(config, TlsBackend::NativeTls)?;

        Ok(Self {
            rustls,
            #[cfg(feature = "tls-native-fallback")]
            native,
            native_hosts,
            rate_limiter: Arc::new(RateLimiter::default()),
        })
    }

    pub(crate) fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }

    pub fn default_client(&self) -> &Client {
        &self.rustls
    }

    pub fn client_for_host(&self, host: Option<&str>) -> &Client {
        let Some(host) = host else {
            return &self.rustls;
        };

        let wants_native = self
            .native_hosts
            .iter()
            .any(|entry| host_matches_entry(host, entry));

        if wants_native {
            #[cfg(feature = "tls-native-fallback")]
            {
                return &self.native;
            }

            #[cfg(not(feature = "tls-native-fallback"))]
            {
                debug!(%host, "native-tls fallback requested but not enabled (build with feature tls-native-fallback)");
            }
        }

        &self.rustls
    }

    pub fn client_for_url(&self, url: &url::Url) -> &Client {
        self.client_for_host(url.host_str())
    }
}

/// Applies the HTTP version preference to a client builder.
///
/// `Auto` offers HTTP/2 through TLS ALPN only on a direct route (no explicit
/// proxy and the system proxy turned off). Through a proxy every HTTP/2
/// request shares one tunnelled connection, and proxies commonly limit or
/// stall a single connection, while HTTP/1.1 spreads concurrent downloads
/// over separate tunnels; measured through an HTTP proxy, HTTP/2 was erratic
/// and sometimes stalled where HTTP/1.1 stayed steady. Cleartext `http://`
/// URLs use HTTP/1.1 regardless, as there is no negotiation without TLS.
///
/// `Http2Only` speaks HTTP/2 with prior knowledge, so it fails against
/// servers (and `http://` URLs) without HTTP/2.
///
/// HTTP/2 multiplexes a host's requests over one connection, so flow control
/// decides throughput: hyper's fixed 64 KiB default windows cap a stream at
/// one window per round trip, so adaptive windows sized from the measured
/// bandwidth-delay product are used instead.
///
/// The native-tls fallback client is built without ALPN support, so it stays
/// on HTTP/1.1 whatever the preference: forcing HTTP/2 there would send an
/// HTTP/2 preface the server never agreed to.
fn apply_http_version(
    builder: reqwest::ClientBuilder,
    config: &DownloaderConfig,
    backend: TlsBackend,
) -> reqwest::ClientBuilder {
    let builder = builder.http2_adaptive_window(true);
    match wire_version(config, backend) {
        WireVersion::Negotiate => builder,
        WireVersion::Http1 => builder.http1_only(),
        WireVersion::Http2 => builder.http2_prior_knowledge(),
    }
}

/// How a client speaks HTTP, as decided by [`apply_http_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WireVersion {
    /// Offer `h2` and `http/1.1` through ALPN.
    Negotiate,
    Http1,
    /// HTTP/2 with prior knowledge.
    Http2,
}

fn wire_version(config: &DownloaderConfig, backend: TlsBackend) -> WireVersion {
    let direct = config.proxy.is_none() && !config.use_system_proxy;
    match (config.http_version, backend) {
        (HttpVersionPreference::Http1Only, _) => WireVersion::Http1,
        #[cfg(feature = "tls-native-fallback")]
        (_, TlsBackend::NativeTls) => WireVersion::Http1,
        (HttpVersionPreference::Http2Only, TlsBackend::Rustls) => WireVersion::Http2,
        (HttpVersionPreference::Auto, TlsBackend::Rustls) if direct => WireVersion::Negotiate,
        (HttpVersionPreference::Auto, TlsBackend::Rustls) => WireVersion::Http1,
    }
}

#[derive(Debug, Clone, Copy)]
enum TlsBackend {
    Rustls,
    #[cfg(feature = "tls-native-fallback")]
    NativeTls,
}

fn create_client_with_backend(
    config: &DownloaderConfig,
    backend: TlsBackend,
) -> Result<Client, DownloadError> {
    install_rustls_provider();

    let mut client_builder = Client::builder()
        .pool_max_idle_per_host(config.pool_max_idle_per_host)
        .pool_idle_timeout(config.pool_idle_timeout)
        .user_agent(&config.user_agent)
        .default_headers(config.headers.clone())
        .redirect(if config.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        });

    debug!(
        pool_max_idle_per_host = config.pool_max_idle_per_host,
        pool_idle_timeout_secs = config.pool_idle_timeout.as_secs(),
        "HTTP connection pool configured"
    );

    match backend {
        TlsBackend::Rustls => {
            client_builder = client_builder.tls_backend_rustls();
        }
        #[cfg(feature = "tls-native-fallback")]
        TlsBackend::NativeTls => {
            client_builder = client_builder.tls_backend_native();
        }
    }

    client_builder = apply_http_version(client_builder, config, backend);

    // Keeps long-lived (HTTP/2) connections alive between segment requests.
    if let Some(interval) = config.http2_keep_alive_interval {
        client_builder = client_builder.tcp_keepalive(interval);
    }

    client_builder = match (config.force_ipv4, config.force_ipv6) {
        (true, false) => client_builder.local_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        (false, true) => client_builder.local_address(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        _ => client_builder,
    };

    if !config.timeout.is_zero() {
        client_builder = client_builder.timeout(config.timeout);
    }

    if !config.connect_timeout.is_zero() {
        client_builder = client_builder.connect_timeout(config.connect_timeout);
    }

    if !config.read_timeout.is_zero() {
        client_builder = client_builder.read_timeout(config.read_timeout);
    }

    client_builder =
        client_builder.tls_danger_accept_invalid_certs(config.danger_accept_invalid_certs);

    if let Some(proxy_config) = &config.proxy {
        let proxy = match build_proxy_from_config(proxy_config) {
            Ok(p) => p,
            Err(e) => return Err(DownloadError::proxy_configuration(e)),
        };
        client_builder = client_builder.proxy(proxy);
        info!(proxy_url = %proxy_config.redacted_url(), "Using explicitly configured proxy for downloads");
    } else if config.use_system_proxy {
        info!(?backend, "Using system proxy settings for downloads");
    } else {
        client_builder = client_builder.no_proxy();
        debug!("Proxy disabled for downloads");
    }

    client_builder.build().map_err(DownloadError::from)
}

pub(crate) fn create_client_pool(config: &DownloaderConfig) -> Result<ClientPool, DownloadError> {
    ClientPool::new(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{ProxyConfig, ProxyType};

    fn config(http_version: HttpVersionPreference, route: Route) -> DownloaderConfig {
        let mut config = DownloaderConfig {
            http_version,
            ..DownloaderConfig::default()
        };
        match route {
            Route::Direct => {
                config.proxy = None;
                config.use_system_proxy = false;
            }
            Route::System => {
                config.proxy = None;
                config.use_system_proxy = true;
            }
            Route::Explicit => {
                config.proxy = Some(ProxyConfig {
                    url: "http://proxy.example.com:8080".to_owned(),
                    proxy_type: ProxyType::Http,
                    auth: None,
                });
                config.use_system_proxy = false;
            }
        }
        config
    }

    #[derive(Clone, Copy)]
    enum Route {
        Direct,
        System,
        Explicit,
    }

    #[test]
    fn auto_negotiates_http2_only_on_a_direct_route() {
        let wire = |route| {
            wire_version(
                &config(HttpVersionPreference::Auto, route),
                TlsBackend::Rustls,
            )
        };
        assert_eq!(wire(Route::Direct), WireVersion::Negotiate);
        assert_eq!(wire(Route::System), WireVersion::Http1);
        assert_eq!(wire(Route::Explicit), WireVersion::Http1);
    }

    #[test]
    fn explicit_preferences_apply_on_every_route() {
        for route in [Route::Direct, Route::System, Route::Explicit] {
            assert_eq!(
                wire_version(
                    &config(HttpVersionPreference::Http2Only, route),
                    TlsBackend::Rustls
                ),
                WireVersion::Http2
            );
            assert_eq!(
                wire_version(
                    &config(HttpVersionPreference::Http1Only, route),
                    TlsBackend::Rustls
                ),
                WireVersion::Http1
            );
        }
    }

    #[cfg(feature = "tls-native-fallback")]
    #[test]
    fn the_native_tls_fallback_stays_on_http1() {
        for preference in [
            HttpVersionPreference::Auto,
            HttpVersionPreference::Http2Only,
            HttpVersionPreference::Http1Only,
        ] {
            assert_eq!(
                wire_version(&config(preference, Route::Direct), TlsBackend::NativeTls),
                WireVersion::Http1
            );
        }
    }
}
