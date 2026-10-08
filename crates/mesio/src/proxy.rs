use std::fmt;

use reqwest::Proxy;
use url::Url;

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
#[derive(Clone)]
pub struct ProxyAuth {
    /// Username for proxy authentication
    pub username: String,
    /// Password for proxy authentication
    pub password: String,
}

impl fmt::Debug for ProxyAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyAuth")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// Proxy configuration
#[derive(Clone)]
pub struct ProxyConfig {
    /// Proxy server URL (for example, <http://proxy.example.com:8080>)
    pub url: String,
    /// Type of proxy (HTTP, HTTPS, SOCKS5).
    ///
    /// Note: this describes how the client connects to the proxy server, not which URL schemes
    /// (http/https) are proxied. Both HTTP and HTTPS requests should follow the configured proxy.
    pub proxy_type: ProxyType,
    /// Authentication for the proxy (optional)
    pub auth: Option<ProxyAuth>,
}

impl ProxyConfig {
    /// The proxy URL with any embedded credentials masked, for logging.
    pub fn redacted_url(&self) -> String {
        match Url::parse(&normalize_proxy_url(&self.url, self.proxy_type)) {
            Ok(mut url) => {
                if (!url.username().is_empty() || url.password().is_some())
                    && (url.set_username("***").is_err() || url.set_password(None).is_err())
                {
                    return "<invalid proxy URL>".to_string();
                }
                url.to_string()
            }
            // An unparseable URL may still contain credentials; show none of it.
            Err(_) => "<invalid proxy URL>".to_string(),
        }
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfig")
            .field("url", &self.redacted_url())
            .field("proxy_type", &self.proxy_type)
            .field("auth", &self.auth)
            .finish()
    }
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

    let proxy_url = match &config.auth {
        Some(auth) => url_with_login(&proxy_url, auth)?,
        None => proxy_url,
    };

    // Use `all` so both http and https requests follow the configured proxy.
    Proxy::all(&proxy_url).map_err(|e| format!("Invalid proxy URL: {}", e.without_url()))
}

/// `proxy_url` with `auth` written into it, replacing any login it carries.
///
/// reqwest percent-decodes the login it finds in a proxy URL, for HTTP and
/// SOCKS alike, and `Proxy::basic_auth` stores its login in that URL without
/// escaping `%`, so the login goes in encoded: everything except ASCII
/// letters, digits and `*-._` is escaped.
fn url_with_login(proxy_url: &str, auth: &ProxyAuth) -> Result<String, String> {
    let encode = |value: &str| {
        url::form_urlencoded::byte_serialize(value.as_bytes())
            .collect::<String>()
            .replace('+', "%20")
    };
    let mut url = Url::parse(proxy_url).map_err(|e| format!("Invalid proxy URL: {e}"))?;
    if url.set_username(&encode(&auth.username)).is_err()
        || url.set_password(Some(&encode(&auth.password))).is_err()
    {
        return Err("Invalid proxy URL: it cannot carry a login".to_owned());
    }
    Ok(url.into())
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

    #[test]
    fn redacted_url_masks_embedded_credentials() {
        let config = ProxyConfig {
            url: "http://user:secret@proxy.example.com:8080".to_string(),
            proxy_type: ProxyType::Http,
            auth: Some(ProxyAuth {
                username: "user".to_string(),
                password: "hunter2".to_string(),
            }),
        };

        assert_eq!(config.redacted_url(), "http://***@proxy.example.com:8080/");
        let debug = format!("{config:?}");
        assert!(!debug.contains("secret"), "{debug}");
        assert!(!debug.contains("hunter2"), "{debug}");
    }

    #[test]
    fn redacted_url_keeps_credential_free_urls() {
        let config = ProxyConfig {
            url: "proxy.example.com:1080".to_string(),
            proxy_type: ProxyType::Socks5,
            auth: None,
        };

        assert_eq!(config.redacted_url(), "socks5://proxy.example.com:1080");
    }

    /// A plain-HTTP forward proxy answering 204 to requests whose Basic
    /// proxy credentials are `expected` (base64) and 407 otherwise.
    async fn login_checking_proxy(expected: &'static str) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 1024];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let request = String::from_utf8_lossy(&request);
                    let allowed = request.lines().any(|line| {
                        line.split_once(':').is_some_and(|(name, value)| {
                            name.eq_ignore_ascii_case("proxy-authorization")
                                && value.trim() == format!("Basic {expected}")
                        })
                    });
                    let response: &[u8] = if allowed {
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                    } else {
                        b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n"
                    };
                    let _ = stream.write_all(response).await;
                });
            }
        });
        address
    }

    #[tokio::test]
    async fn the_login_reaches_the_proxy_as_written() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            // base64 of `us@r%41:p%41 @:x/%`.
            let address = login_checking_proxy("dXNAciU0MTpwJTQxIEA6eC8l").await;
            let send = |auth: Option<ProxyAuth>, url: String| async move {
                let config = crate::DownloaderConfig {
                    proxy: Some(ProxyConfig {
                        url,
                        proxy_type: ProxyType::Http,
                        auth,
                    }),
                    ..crate::DownloaderConfig::default()
                };
                crate::downloader::create_client(&config)
                    .unwrap()
                    .get("http://site.example/")
                    .send()
                    .await
                    .unwrap()
                    .status()
            };
            let login = || {
                Some(ProxyAuth {
                    username: "us@r%41".to_owned(),
                    password: "p%41 @:x/%".to_owned(),
                })
            };
            assert_eq!(
                send(login(), format!("http://{address}")).await,
                reqwest::StatusCode::NO_CONTENT
            );
            // The address may omit its scheme, and the login replaces one
            // the address carries.
            assert_eq!(
                send(login(), format!("old:login@{address}")).await,
                reqwest::StatusCode::NO_CONTENT
            );
            assert_eq!(
                send(None, format!("http://{address}")).await,
                reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
            );
        })
        .await
        .unwrap();
    }
}
