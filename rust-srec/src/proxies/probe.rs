//! Checking that a proxy reaches a site.

use std::error::Error as _;
use std::time::{Duration, Instant};

use platforms_parser::proxy::ProxyTarget;
use serde::Serialize;

use super::ProxyEndpoint;

/// Bound on one check, connection included.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Why a check failed. Error text is not returned: it can quote addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// The proxy refused the login (HTTP 407).
    ProxyAuthenticationRequired,
    /// The proxy, or the site through it, could not be reached.
    ConnectFailed,
    /// No answer within the time limit.
    Timeout,
    /// The TLS handshake with the site failed.
    Tls,
    /// Any other failure.
    Failed,
}

/// The result of one check.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProbeOutcome {
    /// The site answered through the proxy. Any HTTP status except 407
    /// counts: the check is about the connection, not the page.
    pub ok: bool,
    pub status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<ProbeErrorKind>,
}

/// The home page a platform check requests.
pub fn platform_homepage(platform: &str) -> Option<&'static str> {
    Some(match platform.to_ascii_lowercase().as_str() {
        "acfun" => "https://live.acfun.cn",
        "bigo" => "https://www.bigo.tv",
        "bilibili" => "https://live.bilibili.com",
        "douyin" => "https://live.douyin.com",
        "douyu" => "https://www.douyu.com",
        "huya" => "https://www.huya.com",
        "pandatv" => "https://www.pandalive.co.kr",
        "picarto" => "https://picarto.tv",
        "redbook" => "https://www.xiaohongshu.com",
        "soop" => "https://www.sooplive.co.kr",
        "tiktok" => "https://www.tiktok.com",
        "twitcasting" => "https://twitcasting.tv",
        "twitch" => "https://www.twitch.tv",
        "weibo" => "https://weibo.com",
        _ => return None,
    })
}

/// Requests `target` through `endpoint` once, without following redirects.
pub async fn probe(endpoint: &ProxyEndpoint, target: &url::Url) -> ProbeOutcome {
    crate::utils::http_client::install_rustls_provider();
    let started = Instant::now();
    let client = crate::utils::http_client::apply_proxy(
        reqwest::Client::builder()
            .timeout(PROBE_TIMEOUT)
            .connect_timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none()),
        &ProxyTarget::Explicit(endpoint.clone()),
    )
    .build();
    let client = match client {
        Ok(client) => client,
        Err(_) => {
            return ProbeOutcome {
                ok: false,
                status: None,
                latency_ms: 0,
                error: Some(ProbeErrorKind::Failed),
            };
        }
    };
    let response = client
        .get(target.clone())
        .header(
            reqwest::header::USER_AGENT,
            platforms_parser::extractor::DEFAULT_UA,
        )
        .send()
        .await;
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    match response {
        Ok(response) => {
            let status = response.status();
            let auth = status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED;
            ProbeOutcome {
                ok: !auth,
                status: Some(status.as_u16()),
                latency_ms,
                error: auth.then_some(ProbeErrorKind::ProxyAuthenticationRequired),
            }
        }
        Err(error) => ProbeOutcome {
            ok: false,
            status: None,
            latency_ms,
            error: Some(classify(&error)),
        },
    }
}

fn classify(error: &reqwest::Error) -> ProbeErrorKind {
    let mut chain = String::new();
    let mut source: Option<&dyn std::error::Error> = error.source();
    while let Some(cause) = source {
        chain.push_str(&cause.to_string().to_ascii_lowercase());
        chain.push(' ');
        source = cause.source();
    }
    if chain.contains("407") || chain.contains("proxy authentication") {
        ProbeErrorKind::ProxyAuthenticationRequired
    } else if error.is_timeout() {
        ProbeErrorKind::Timeout
    } else if chain.contains("certificate") || chain.contains("tls") || chain.contains("handshake")
    {
        ProbeErrorKind::Tls
    } else if error.is_connect() {
        ProbeErrorKind::ConnectFailed
    } else {
        ProbeErrorKind::Failed
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    /// A plain-HTTP forward proxy that answers every request itself with
    /// `status`, after checking the login when `login` is set.
    async fn fixture_proxy(login: Option<&'static str>) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
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
                    let request = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let allowed = login.is_none_or(|login| {
                        use base64::Engine as _;
                        let expected = base64::prelude::BASE64_STANDARD.encode(login);
                        request.contains(&format!(
                            "proxy-authorization: basic {}",
                            expected.to_ascii_lowercase()
                        ))
                    });
                    let response: &[u8] = if allowed {
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"
                    } else {
                        b"HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic\r\nContent-Length: 0\r\n\r\n"
                    };
                    let _ = stream.write_all(response).await;
                });
            }
        });
        address
    }

    #[tokio::test]
    async fn a_check_reports_reachability_and_a_refused_login() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let target = url::Url::parse("http://site.example/").unwrap();
            let open = fixture_proxy(None).await;
            let outcome = probe(&ProxyEndpoint::new(format!("http://{open}"), None), &target).await;
            assert!(outcome.ok);
            assert_eq!(outcome.status, Some(204));

            let guarded = fixture_proxy(Some("user:secret")).await;
            let refused = probe(
                &ProxyEndpoint::new(format!("http://{guarded}"), None),
                &target,
            )
            .await;
            assert!(!refused.ok);
            assert_eq!(
                refused.error,
                Some(ProbeErrorKind::ProxyAuthenticationRequired)
            );
            let accepted = probe(
                &ProxyEndpoint::new(
                    format!("http://{guarded}"),
                    Some(("user".into(), "secret".into())),
                ),
                &target,
            )
            .await;
            assert!(accepted.ok, "{accepted:?}");

            // The login reaches the proxy as written, escapes included.
            let awkward = fixture_proxy(Some("us@r:p%41 @:x/%")).await;
            let accepted = probe(
                &ProxyEndpoint::new(
                    format!("http://{awkward}"),
                    Some(("us@r".into(), "p%41 @:x/%".into())),
                ),
                &target,
            )
            .await;
            assert!(accepted.ok, "{accepted:?}");

            // Nothing listens on a port that was just released.
            let closed = TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap()
                .local_addr()
                .unwrap();
            let unreachable = probe(
                &ProxyEndpoint::new(format!("http://{closed}"), None),
                &target,
            )
            .await;
            assert_eq!(unreachable.error, Some(ProbeErrorKind::ConnectFailed));
        })
        .await
        .unwrap();
    }

    #[test]
    fn known_platforms_have_a_home_page() {
        for platform in ["bilibili", "Twitch", "douyin"] {
            assert!(platform_homepage(platform).is_some(), "{platform}");
        }
        assert!(platform_homepage("unknown").is_none());
    }
}
