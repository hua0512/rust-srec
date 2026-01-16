use reqwest::Proxy;

/// Proxy configuration types
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
#[cfg_attr(feature = "clap", derive(clap::ValueEnum))]
pub enum ProxyType {
    /// HTTP proxy
    Http,
    /// HTTPS proxy
    Https,
    /// SOCKS5 proxy
    Socks5,
}

/// Proxy authentication type
#[derive(Debug, Clone)]
pub struct ProxyAuth {
    /// Username for proxy authentication
    pub username: String,
    /// Password for proxy authentication
    pub password: String,
}

/// Proxy configuration
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Proxy server URL (e.g., "http://proxy.example.com:8080")
    pub url: String,
    /// Type of proxy (HTTP, HTTPS, SOCKS5).
    ///
    /// Note: this describes how the client connects to the proxy server, not which URL schemes
    /// (http/https) are proxied. Both HTTP and HTTPS requests should follow the configured proxy.
    pub proxy_type: ProxyType,
    /// Authentication for the proxy (optional)
    pub auth: Option<ProxyAuth>,
}

fn normalize_proxy_url(proxy_url: &str, proxy_type: ProxyType) -> String {
    if proxy_url.contains("://") {
        return proxy_url.to_string();
    }

    match proxy_type {
        ProxyType::Http => format!("http://{proxy_url}"),
        ProxyType::Https => format!("https://{proxy_url}"),
        ProxyType::Socks5 => format!("socks5://{proxy_url}"),
    }
}

/// Build a reqwest `Proxy` object from our proxy configuration.
pub fn build_proxy_from_config(config: &ProxyConfig) -> Result<Proxy, String> {
    let proxy_url = match config.proxy_type {
        ProxyType::Socks5 if config.url.starts_with("socks5h://") => config.url.clone(),
        proxy_type => normalize_proxy_url(&config.url, proxy_type),
    };

    // Use `all` so both http and https requests follow the configured proxy.
    let mut proxy = Proxy::all(&proxy_url).map_err(|e| format!("Invalid proxy URL: {e}"))?;

    // Add authentication if provided
    if let Some(auth) = &config.auth {
        proxy = proxy.basic_auth(&auth.username, &auth.password);
    }

    Ok(proxy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keeps_existing_scheme() {
        assert_eq!(
            normalize_proxy_url("https://proxy.example.com:443", ProxyType::Http),
            "https://proxy.example.com:443"
        );
    }

    #[test]
    fn normalize_adds_http_scheme() {
        assert_eq!(
            normalize_proxy_url("proxy.example.com:8080", ProxyType::Http),
            "http://proxy.example.com:8080"
        );
    }

    #[test]
    fn normalize_adds_https_scheme() {
        assert_eq!(
            normalize_proxy_url("proxy.example.com:443", ProxyType::Https),
            "https://proxy.example.com:443"
        );
    }

    #[test]
    fn normalize_adds_socks5_scheme() {
        assert_eq!(
            normalize_proxy_url("proxy.example.com:1080", ProxyType::Socks5),
            "socks5://proxy.example.com:1080"
        );
    }

    #[test]
    fn build_proxy_accepts_host_port_without_scheme() {
        let config = ProxyConfig {
            url: "proxy.example.com:8080".to_string(),
            proxy_type: ProxyType::Http,
            auth: None,
        };

        build_proxy_from_config(&config).expect("proxy should build with implicit scheme");
    }

    #[test]
    fn build_proxy_preserves_socks5h() {
        let config = ProxyConfig {
            url: "socks5h://proxy.example.com:1080".to_string(),
            proxy_type: ProxyType::Socks5,
            auth: None,
        };

        build_proxy_from_config(&config).expect("socks5h proxy should build");
    }
}
