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

    /// Set whether to accept invalid certificates
    ///
    /// # Warning
    /// This is unsafe and should only be used for testing or in controlled environments.
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.config.danger_accept_invalid_certs = accept;
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
    use crate::ProxyAuth;

    use super::*;
    use std::time::Duration;

    #[test]
    fn test_builder_defaults() {
        let config = DownloaderConfigBuilder::new().build();
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.connect_timeout, Duration::from_secs(10));
        assert!(config.follow_redirects);
        assert!(config.use_system_proxy);
        assert!(!config.danger_accept_invalid_certs);
    }

    #[test]
    fn test_builder_customization() {
        let config = DownloaderConfigBuilder::new()
            .with_timeout(Duration::from_secs(60))
            .with_connect_timeout(Duration::from_secs(20))
            .with_follow_redirects(false)
            .with_user_agent("CustomUserAgent/1.0")
            .with_header("X-Custom-Header", "CustomValue")
            .with_system_proxy(false)
            .build();

        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.connect_timeout, Duration::from_secs(20));
        assert!(!config.follow_redirects);
        assert_eq!(config.user_agent, "CustomUserAgent/1.0");
        assert!(!config.use_system_proxy);

        // Verify custom header
        let header_value = config.headers.get("X-Custom-Header").unwrap();
        assert_eq!(header_value.to_str().unwrap(), "CustomValue");
    }

    #[test]
    fn test_caching_options() {
        // Test with caching enabled
        let config_with_cache = DownloaderConfigBuilder::new()
            .with_caching_enabled(true)
            .build();
        assert!(config_with_cache.cache_config.is_some());

        // Test with caching disabled
        let config_without_cache = DownloaderConfigBuilder::new()
            .with_caching_enabled(false)
            .build();
        assert!(config_without_cache.cache_config.is_none());
    }

    #[test]
    fn test_proxy_configuration() {
        let proxy_config = ProxyConfig {
            url: "http://proxy.example.com:8080".to_string(),
            proxy_type: crate::ProxyType::Http,
            auth: Some(ProxyAuth {
                username: "user".to_string(),
                password: "pass".to_string(),
            }),
        };

        // Test with explicit proxy
        let config_with_proxy = DownloaderConfigBuilder::new()
            .with_proxy(proxy_config.clone())
            .build();

        assert!(config_with_proxy.proxy.is_some());
        assert!(!config_with_proxy.use_system_proxy);

        let stored_proxy = config_with_proxy.proxy.unwrap();
        assert_eq!(stored_proxy.url, proxy_config.url);
        assert_eq!(stored_proxy.auth.as_ref().unwrap().username, "user");
        assert_eq!(stored_proxy.auth.as_ref().unwrap().password, "pass");
        assert_eq!(stored_proxy.proxy_type, proxy_config.proxy_type);
    }
}
