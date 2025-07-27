use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};

use crate::{CacheConfig, proxy::ProxyConfig};

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

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

    /// Write timeout (maximum time for sending request data)
    pub write_timeout: Duration,

    /// Whether to follow redirects
    pub follow_redirects: bool,

    /// User agent string
    pub user_agent: String,

    /// Custom HTTP headers for requests
    pub headers: HeaderMap,

    /// Proxy configuration (optional)
    pub proxy: Option<ProxyConfig>,

    /// Whether to use system proxy settings if available
    pub use_system_proxy: bool,

    pub danger_accept_invalid_certs: bool, // For reqwest's `danger_accept_invalid_certs`

    pub force_ipv4: bool,

    pub force_ipv6: bool,
}

impl Default for DownloaderConfig {
    fn default() -> Self {
        Self {
            cache_config: None,
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            follow_redirects: true,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            headers: DownloaderConfig::get_default_headers(),
            proxy: None,
            use_system_proxy: true, // Enable system proxy by default
            danger_accept_invalid_certs: false, // Default to not accepting invalid certs
            force_ipv4: false,
            force_ipv6: false,
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
            proxy: config.proxy,
            use_system_proxy: config.use_system_proxy,
            danger_accept_invalid_certs: config.danger_accept_invalid_certs,
            force_ipv4: config.force_ipv4,
            force_ipv6: config.force_ipv6,
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
