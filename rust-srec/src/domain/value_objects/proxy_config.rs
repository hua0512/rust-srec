//! Proxy configuration value object.

use serde::{Deserialize, Serialize};

/// Proxy configuration for network requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Whether proxy is enabled.
    pub enabled: bool,
    /// Proxy URL (e.g., "http://proxy.example.com:8080").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Username for proxy authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Password for proxy authentication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Use system proxy settings.
    #[serde(default)]
    pub use_system_proxy: bool,
}

impl ProxyConfig {
    /// Create a disabled proxy config.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            url: None,
            username: None,
            password: None,
            use_system_proxy: false,
        }
    }

    /// Create a proxy config with a URL.
    pub fn with_url(url: impl Into<String>) -> Self {
        Self {
            enabled: true,
            url: Some(url.into()),
            username: None,
            password: None,
            use_system_proxy: false,
        }
    }

    /// Create a proxy config that uses system settings.
    pub fn system() -> Self {
        Self {
            enabled: true,
            url: None,
            username: None,
            password: None,
            use_system_proxy: true,
        }
    }

    /// Add authentication credentials.
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }

    /// Check if this config has authentication.
    pub fn has_auth(&self) -> bool {
        self.username.is_some() && self.password.is_some()
    }

    /// Get the effective proxy URL with authentication if present.
    pub fn effective_url(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }

        self.url.as_ref().map(|url| {
            if let (Some(user), Some(pass)) = (&self.username, &self.password) {
                // Insert auth into URL
                if let Some(pos) = url.find("://") {
                    let (scheme, rest) = url.split_at(pos + 3);
                    format!("{}{}:{}@{}", scheme, user, pass, rest)
                } else {
                    url.clone()
                }
            } else {
                url.clone()
            }
        })
    }
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proxy_disabled() {
        let config = ProxyConfig::disabled();
        assert!(!config.enabled);
        assert!(config.url.is_none());
    }

    #[test]
    fn test_proxy_with_url() {
        let config = ProxyConfig::with_url("http://proxy.example.com:8080");
        assert!(config.enabled);
        assert_eq!(config.url, Some("http://proxy.example.com:8080".to_string()));
    }

    #[test]
    fn test_proxy_with_auth() {
        let config = ProxyConfig::with_url("http://proxy.example.com:8080")
            .with_auth("user", "pass");
        assert!(config.has_auth());
        assert_eq!(
            config.effective_url(),
            Some("http://user:pass@proxy.example.com:8080".to_string())
        );
    }

    #[test]
    fn test_proxy_system() {
        let config = ProxyConfig::system();
        assert!(config.enabled);
        assert!(config.use_system_proxy);
    }

    #[test]
    fn test_proxy_serialization() {
        let config = ProxyConfig::with_url("http://proxy.example.com:8080");
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ProxyConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, config);
    }
}
