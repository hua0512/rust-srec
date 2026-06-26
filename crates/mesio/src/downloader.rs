use reqwest::Client;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;
use tracing::{debug, info};

use crate::DownloaderConfig;
use crate::{DownloadError, proxy::build_proxy_from_config};

/// Create a reqwest Client with the provided configuration
///
/// HTTP/2 is automatically negotiated via ALPN when using rustls-tls.
/// The connection pool settings help maximize HTTP/2 multiplexing benefits.
///
/// HTTP/2 Multiplexing Benefits for HLS:
/// - Single TCP connection handles multiple concurrent segment downloads
/// - Reduced connection overhead (no TCP handshake per segment)
/// - Better utilization of available bandwidth
/// - Header compression (HPACK) reduces overhead for repeated headers
pub fn create_client(config: &DownloaderConfig) -> Result<Client, DownloadError> {
    install_rustls_provider();
    use crate::config::HttpVersionPreference;

    let mut client_builder = Client::builder()
        .pool_max_idle_per_host(config.pool_max_idle_per_host) // Configurable: Keep connections warm for HLS segment downloads
        .pool_idle_timeout(config.pool_idle_timeout) // Configurable: Reuse connections for specified duration
        .user_agent(&config.user_agent)
        .default_headers(config.headers.clone())
        .use_rustls_tls()
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

    // --- HTTP Version Configuration ---
    // Note: reqwest with rustls-tls automatically negotiates HTTP/2 via ALPN
    // These options control fallback behavior
    match config.http_version {
        HttpVersionPreference::Http2Only => {
            // With rustls-tls, HTTP/2 is preferred via ALPN
            // We can't force it, but we log the preference
            debug!("HTTP/2 preferred mode (ALPN will negotiate)");
        }
        HttpVersionPreference::Http1Only => {
            client_builder = client_builder.http1_only();
            debug!("HTTP/1.1 only mode enabled");
        }
        HttpVersionPreference::Auto => {
            // Default: let ALPN negotiate (HTTP/2 preferred with rustls)
            debug!("HTTP version: Auto (ALPN negotiation, HTTP/2 preferred)");
        }
    }

    // --- HTTP/2 Configuration Notes ---
    // With rustls-tls backend, HTTP/2 is automatically negotiated via ALPN.
    // The flow control window sizes use hyper's defaults which are reasonable
    // for most use cases. The key optimizations we can apply are:
    // 1. Connection pooling (configured above)
    // 2. TCP keep-alive to maintain connections
    // 3. Proper timeout configuration
    //
    // Note: HTTP/2 specific methods like http2_adaptive_window() require
    // the native-tls backend. With rustls-tls, HTTP/2 works but with
    // default flow control settings.

    // --- TCP Keep-Alive for long-lived connections ---
    // This helps maintain HTTP/2 connections for multiplexing
    if let Some(interval) = config.http2_keep_alive_interval {
        client_builder = client_builder.tcp_keepalive(interval);
        debug!(
            ?interval,
            "TCP keep-alive configured for HTTP/2 connection reuse"
        );
    }

    // Force IP Version
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

    // reqwest does not currently expose a per-request "write timeout" builder API.
    // `timeout` (overall request timeout) is still applied above when configured.

    client_builder = client_builder.danger_accept_invalid_certs(config.danger_accept_invalid_certs);

    // Set up proxy configuration
    if let Some(proxy_config) = &config.proxy {
        // Explicit proxy configuration takes precedence
        let proxy = match build_proxy_from_config(proxy_config) {
            Ok(p) => p,
            Err(e) => return Err(DownloadError::proxy_configuration(e)),
        };
        client_builder = client_builder.proxy(proxy);
        info!(proxy_url = %proxy_config.url, "Using explicitly configured proxy for downloads");
    } else if config.use_system_proxy {
        // No explicit proxy but system proxy enabled
        // reqwest will use system proxy settings by default when we don't call no_proxy()
        info!("Using system proxy settings for downloads");
    } else {
        // Explicitly disable proxy
        client_builder = client_builder.no_proxy();
        debug!("Proxy disabled for downloads");
    }

    client_builder.build().map_err(DownloadError::from)
}

pub(crate) const ENV_NATIVE_TLS_HOSTS: &str = "RUST_SREC_NATIVE_TLS_HOSTS";

fn install_rustls_provider() {
    // `reqwest` is configured with `rustls-tls-*-no-provider`; install one globally.
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
        })
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
    use crate::config::HttpVersionPreference;

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

    match backend {
        TlsBackend::Rustls => {
            client_builder = client_builder.use_rustls_tls();
        }
        #[cfg(feature = "tls-native-fallback")]
        TlsBackend::NativeTls => {
            client_builder = client_builder.use_native_tls();
        }
    }

    match config.http_version {
        HttpVersionPreference::Http2Only => {
            debug!(?backend, "HTTP/2 preferred mode (ALPN will negotiate)");
        }
        HttpVersionPreference::Http1Only => {
            client_builder = client_builder.http1_only();
        }
        HttpVersionPreference::Auto => {}
    }

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

    client_builder = client_builder.danger_accept_invalid_certs(config.danger_accept_invalid_certs);

    if let Some(proxy_config) = &config.proxy {
        let proxy = match build_proxy_from_config(proxy_config) {
            Ok(p) => p,
            Err(e) => return Err(DownloadError::proxy_configuration(e)),
        };
        client_builder = client_builder.proxy(proxy);
        info!(proxy_url = %proxy_config.url, "Using explicitly configured proxy for downloads");
    } else if config.use_system_proxy {
        info!(?backend, "Using system proxy settings for downloads");
    } else {
        client_builder = client_builder.no_proxy();
    }

    client_builder.build().map_err(DownloadError::from)
}

pub(crate) fn create_client_pool(config: &DownloaderConfig) -> Result<ClientPool, DownloadError> {
    ClientPool::new(config)
}
