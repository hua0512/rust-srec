use reqwest::Proxy;

/// Proxy configuration types
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ProxyType {
    /// HTTP proxy
    Http,
    /// HTTPS proxy
    Https,
    /// SOCKS5 proxy
    Socks5,
    /// All protocols proxy (use this for general-purpose proxies)
    All,
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
    /// Type of proxy (HTTP, HTTPS, SOCKS5)
    pub proxy_type: ProxyType,
    /// Authentication for the proxy (optional)
    pub auth: Option<ProxyAuth>,
}

/// Build a reqwest Proxy object from our proxy configuration
pub fn build_proxy_from_config(config: &ProxyConfig) -> Result<Proxy, String> {
    let proxy_url = &config.url;

    // Create the appropriate proxy based on type
    let mut proxy = match config.proxy_type {
        ProxyType::Http => {
            Proxy::http(proxy_url).map_err(|e| format!("Invalid HTTP proxy URL: {e}"))?
        }
        ProxyType::Https => {
            Proxy::https(proxy_url).map_err(|e| format!("Invalid HTTPS proxy URL: {e}"))?
        }
        ProxyType::Socks5 => {
            // Make sure URL starts with socks5://
            let url = if proxy_url.starts_with("socks5://") {
                proxy_url.to_string()
            } else {
                format!("socks5://{proxy_url}")
            };

            Proxy::all(&url).map_err(|e| format!("Invalid SOCKS5 proxy URL: {e}"))?
        }
        ProxyType::All => Proxy::all(proxy_url).map_err(|e| format!("Invalid proxy URL: {e}"))?,
    };

    // Add authentication if provided
    if let Some(auth) = &config.auth {
        proxy = proxy.basic_auth(&auth.username, &auth.password);
    }

    Ok(proxy)
}
