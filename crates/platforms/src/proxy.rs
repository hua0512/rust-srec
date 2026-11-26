//! How extractors, their helper processes and danmu connections reach a
//! platform: directly, through the environment's proxy, or through an
//! explicit proxy.

pub use proxy_target::{
    PROXY_ENVIRONMENT, ProxyEndpoint, ProxyTarget, UnsupportedProxyScheme, UnusableProxy,
    redacted_url, unroutable_proxy,
};

/// A plain-HTTP forward proxy for tests. It answers every request itself:
/// 204 when the request carries `login` (`user:password`) as its Basic proxy
/// credentials and 407 otherwise.
#[cfg(test)]
pub(crate) async fn login_checking_proxy(login: &'static str) -> std::net::SocketAddr {
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let expected = format!("Basic {}", base64::prelude::BASE64_STANDARD.encode(login));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let expected = expected.clone();
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
                        name.eq_ignore_ascii_case("proxy-authorization") && value.trim() == expected
                    })
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A login with escapes, delimiters and spaces; the proxy must receive
    /// it as written.
    const AWKWARD_LOGIN: (&str, &str) = ("us@r", "p%41 @:x/%");

    #[tokio::test]
    async fn a_client_sends_the_login_as_written() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let (username, password) = AWKWARD_LOGIN;
            let proxy = login_checking_proxy("us@r:p%41 @:x/%").await;
            let send = |login: Option<(String, String)>| async move {
                let endpoint = ProxyEndpoint::new(format!("http://{proxy}"), login);
                crate::extractor::create_client_builder(Some(endpoint))
                    .build()
                    .unwrap()
                    .get("http://site.example/")
                    .send()
                    .await
                    .unwrap()
                    .status()
            };
            assert_eq!(
                send(Some((username.to_owned(), password.to_owned()))).await,
                reqwest::StatusCode::NO_CONTENT
            );
            assert_eq!(
                send(None).await,
                reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_unusable_proxy_fails_requests_instead_of_skipping_the_proxy() {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let target = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = target.local_addr().unwrap();
            for proxy in ["http://[invalid", "sock5://proxy.invalid:1080"] {
                let client =
                    crate::extractor::create_client_builder(Some(ProxyEndpoint::new(proxy, None)))
                        .build()
                        .unwrap();
                assert!(
                    client
                        .get(format!("http://{address}/"))
                        .send()
                        .await
                        .is_err(),
                    "{proxy}"
                );
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(100), target.accept())
                        .await
                        .is_err(),
                    "the request reached the target directly through {proxy}"
                );
            }
        })
        .await
        .unwrap();
    }
}
