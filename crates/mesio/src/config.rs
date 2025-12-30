use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};

use crate::{CacheConfig, proxy::ProxyConfig};

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";

/// HTTP version preference for connections
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HttpVersionPreference {
    /// Let ALPN negotiate the best version (default)
    #[default]
    Auto,
    /// Prefer HTTP/2 when available.
    ///
    /// Note: with `reqwest` + `rustls-tls`, HTTP/2 is negotiated via ALPN and
    /// fallback behavior is not strictly controllable at the client-builder level.
    Http2Only,
    /// Force HTTP/1.1 only (disable HTTP/2)
    Http1Only,
}

/// Configurable options for the downloader
#[derive(Debug, Clone)]
pub struct DownloaderConfig {
    /// Cache configuration
    pub cache_config: Option<CacheConfig>,

    /// Overall timeout for the entire HTTP request
    pub timeout: Duration,

    /// Connection timeout (time to establish initial connection)
    pub connect_timeout: Duration,

    /// Read timeout (maximum time between receiving data chunks)
    pub read_timeout: Duration,

    /// Write timeout (maximum time for sending request data).
    ///
    /// Note: `reqwest` does not currently expose a dedicated write-timeout setting on the
    /// `ClientBuilder`; this field is reserved for future use.
    pub write_timeout: Duration,

    /// Whether to follow redirects
    pub follow_redirects: bool,

    /// User agent string
    pub user_agent: String,

    /// Custom HTTP headers for requests
    pub headers: HeaderMap,

    /// Custom parameters for requests
    pub params: Vec<(String, String)>,

    /// Proxy configuration (optional)
    pub proxy: Option<ProxyConfig>,

    /// Whether to use system proxy settings if available
    pub use_system_proxy: bool,

    pub danger_accept_invalid_certs: bool, // For reqwest's `danger_accept_invalid_certs`

    pub force_ipv4: bool,

    pub force_ipv6: bool,

    // --- HTTP/2 Configuration ---
    /// HTTP version preference (Auto, Http2Only, Http1Only)
    /// Note: With rustls-tls, HTTP/2 is automatically negotiated via ALPN
    pub http_version: HttpVersionPreference,

    /// TCP keep-alive interval for maintaining long-lived connections
    /// This helps keep HTTP/2 connections alive for multiplexing benefits
    /// Recommended: 15-30 seconds for media streaming
    pub http2_keep_alive_interval: Option<Duration>,

    // --- Connection Pool Configuration ---
    /// Maximum idle connections to keep per host
    /// Higher values improve HTTP/2 multiplexing for HLS segment downloads
    /// Default: 10
    pub pool_max_idle_per_host: usize,

    /// Duration to keep idle connections alive before closing
    /// Longer timeouts improve connection reuse for streaming
    /// Default: 30 seconds
    pub pool_idle_timeout: Duration,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            cache_config: None,
            timeout: Duration::from_secs(0),
            connect_timeout: Duration::from_secs(30),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            follow_redirects: true,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            headers: DownloaderConfig::get_default_headers(),
            params: Vec::new(),
            proxy: None,
            use_system_proxy: true,
            danger_accept_invalid_certs: false,
            force_ipv4: false,
            force_ipv6: false,
            // HTTP/2 defaults - optimized for media streaming
            http_version: HttpVersionPreference::Auto,
            http2_keep_alive_interval: Some(Duration::from_secs(20)),
            // Connection pool defaults - optimized for HLS segment downloads
            pool_max_idle_per_host: 10,
            pool_idle_timeout: Duration::from_secs(30),
        }
    }
}

impl DownloaderConfig {
    pub fn builder() -> crate::builder::DownloaderConfigBuilder {
        crate::builder::DownloaderConfigBuilder::new()
    }

    pub fn with_config(config: DownloaderConfig) -> Self {
        let mut headers = DownloaderConfig::get_default_headers();

        if !config.headers.is_empty() {
            // If custom headers are provided, merge them with defaults
            // Custom headers take precedence over defaults for the same fields
            for (name, value) in config.headers.iter() {
                headers.insert(name.clone(), value.clone());
            }
        }

        Self {
            cache_config: config.cache_config,
            timeout: config.timeout,
            connect_timeout: config.connect_timeout,
            read_timeout: config.read_timeout,
            write_timeout: config.write_timeout,
            follow_redirects: config.follow_redirects,
            user_agent: config.user_agent,
            headers,
            params: config.params,
            proxy: config.proxy,
            use_system_proxy: config.use_system_proxy,
            danger_accept_invalid_certs: config.danger_accept_invalid_certs,
            force_ipv4: config.force_ipv4,
            force_ipv6: config.force_ipv6,
            // HTTP/2 settings
            http_version: config.http_version,
            http2_keep_alive_interval: config.http2_keep_alive_interval,
            // Connection pool settings
            pool_max_idle_per_host: config.pool_max_idle_per_host,
            pool_idle_timeout: config.pool_idle_timeout,
        }
    }

    pub fn get_default_headers() -> HeaderMap {
        let mut default_headers = HeaderMap::new();

        default_headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br"),
        );

        default_headers.insert(
            reqwest::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );

        default_headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
        );

        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.5,zh-CN;q=0.3,zh;q=0.2"),
        );
        default_headers
    }
}
