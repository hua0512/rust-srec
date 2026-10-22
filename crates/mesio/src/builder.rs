//! # Builder for DownloaderConfig
//!
//! This module provides a builder pattern implementation for creating and customizing
//! DownloaderConfig instances with a fluent API.
//!
//! # Example
//!
//! ```
//! use std::time::Duration;
//! use mesio_engine::DownloaderConfig;
//! use mesio_engine::proxy::ProxyConfig;
//!
//! // Create a config with the builder
//! let config = DownloaderConfig::builder()
//!     .with_timeout(Duration::from_secs(60))
//!     .with_connect_timeout(Duration::from_secs(15))
//!     .with_user_agent("MyApp/1.0")
//!     .with_header("X-Api-Key", "my-secret-key")
//!     .with_follow_redirects(true)
//!     .with_caching_enabled(true)
//!     .build();
//!
//! // Or with an explicit proxy configuration
//! let config_with_proxy = DownloaderConfig::builder()
//!     .with_proxy(ProxyConfig {
//!         url: "http://proxy.example.com:8080".to_string(),
//!         proxy_type: mesio_engine::proxy::ProxyType::Http,
//!         auth: Some(mesio_engine::proxy::ProxyAuth {
//!             username: "user".to_string(),
//!             password: "pass".to_string(),
//!         }),
//!     })
//!     .build();
//! ```

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue};

use crate::{CacheConfig, DownloaderConfig, proxy::ProxyConfig};

/// Builder for creating DownloaderConfig instances with a fluent API
#[derive(Debug, Clone)]
pub struct DownloaderConfigBuilder {
    /// Internal config being built
    config: DownloaderConfig,
}

impl DownloaderConfigBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: DownloaderConfig::default(),
        }
    }

    /// Set the cache configuration
    pub fn with_cache_config(mut self, cache_config: CacheConfig) -> Self {
        self.config.cache_config = Some(cache_config);
        self
    }

    /// Enable or disable caching
    pub fn with_caching_enabled(mut self, enabled: bool) -> Self {
        if enabled {
            if self.config.cache_config.is_none() {
                self.config.cache_config = Some(CacheConfig::default());
            }
        } else {
            self.config.cache_config = None;
        }
        self
    }

    /// Set the overall timeout for the entire HTTP request
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.config.timeout = timeout;
        self
    }

    /// Set the connection timeout (time to establish initial connection)
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.config.connect_timeout = timeout;
        self
    }

    /// Set the read timeout (maximum time between receiving data chunks)
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.config.read_timeout = timeout;
        self
    }

    /// Set the write timeout (maximum time for sending request data)
    ///
    /// Note: `reqwest` does not currently expose a dedicated write-timeout setting on the
    /// `ClientBuilder`; this field is reserved for future use.
    pub fn with_write_timeout(mut self, timeout: Duration) -> Self {
        self.config.write_timeout = timeout;
        self
    }

    /// Set whether to follow redirects
    pub fn with_follow_redirects(mut self, follow: bool) -> Self {
        self.config.follow_redirects = follow;
        self
    }

    /// Set the user agent string
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.config.user_agent = user_agent.into();
        self
    }

    /// Add a custom HTTP header
    pub fn with_header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        if let (Ok(name), Ok(value)) = (
            name.as_ref().parse::<reqwest::header::HeaderName>(),
            HeaderValue::from_str(value.as_ref()),
        ) {
            self.config.headers.insert(name, value);
        }
        self
    }

    /// Set all HTTP headers, replacing any existing headers
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.config.headers = headers;
        self
    }

    /// Set custom parameters for requests
    pub fn with_params(mut self, params: Vec<(String, String)>) -> Self {
        self.config.params = params;
        self
    }

    /// Set the proxy configuration
    pub fn with_proxy(mut self, proxy: ProxyConfig) -> Self {
        self.config.proxy = Some(proxy);
        self.config.use_system_proxy = false; // Explicit proxy overrides system proxy
        self
    }

    /// Set whether to use system proxy settings if available
    pub fn with_system_proxy(mut self, use_system_proxy: bool) -> Self {
        // Only set system proxy if no explicit proxy is configured
        if self.config.proxy.is_none() {
            self.config.use_system_proxy = use_system_proxy;
        }
        self
    }

    /// Set whether to force IPv4
    pub fn with_force_ipv4(mut self, force: bool) -> Self {
        self.config.force_ipv4 = force;
        self
    }

    /// Set whether to force IPv6
    pub fn with_force_ipv6(mut self, force: bool) -> Self {
        self.config.force_ipv6 = force;
        self
    }

    /// Set whether to accept invalid certificates
    ///
    /// # Warning
    /// This is unsafe and should only be used for testing or in controlled environments.
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.config.danger_accept_invalid_certs = accept;
        self
    }

    // --- HTTP/2 Configuration Methods ---

    /// Set the HTTP version preference
    ///
    /// - `Auto`: Let ALPN negotiate (HTTP/2 preferred with rustls-tls)
    /// - `Http2Only`: Prefer HTTP/2 (ALPN will negotiate)
    /// - `Http1Only`: Force HTTP/1.1 only
    pub fn with_http_version(mut self, version: crate::config::HttpVersionPreference) -> Self {
        self.config.http_version = version;
        self
    }

    /// Force HTTP/1.1 only (disable HTTP/2)
    pub fn with_http1_only(mut self) -> Self {
        self.config.http_version = crate::config::HttpVersionPreference::Http1Only;
        self
    }

    /// Set the TCP keep-alive interval for maintaining long-lived HTTP/2 connections
    ///
    /// This helps keep HTTP/2 connections alive for multiplexing benefits.
    /// Recommended: 15-30 seconds for media streaming workloads.
    pub fn with_http2_keep_alive_interval(mut self, interval: Duration) -> Self {
        self.config.http2_keep_alive_interval = Some(interval);
        self
    }

    /// Configure settings optimized for HLS media streaming
    ///
    /// This sets:
    /// - Keep-alive interval: 20s (maintains HTTP/2 connections for multiplexing)
    pub fn with_http2_hls_optimized(mut self) -> Self {
        self.config.http2_keep_alive_interval = Some(Duration::from_secs(20));
        self
    }

    /// Build the DownloaderConfig instance
    pub fn build(self) -> DownloaderConfig {
        self.config
    }
}

impl Default for DownloaderConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::ProxyType;

    use super::*;

    #[test]
    fn an_explicit_proxy_overrides_the_system_proxy() {
        let proxy = ProxyConfig {
            url: "http://proxy.example.com:8080".to_string(),
            proxy_type: ProxyType::Http,
            auth: None,
        };

        let config = DownloaderConfigBuilder::new()
            .with_system_proxy(true)
            .with_proxy(proxy)
            .with_system_proxy(true)
            .build();

        assert!(config.proxy.is_some());
        assert!(!config.use_system_proxy);
    }
}
