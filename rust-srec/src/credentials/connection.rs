//! Clients for the provider calls made for accounts.
//!
//! Each call goes through the route resolved for it, so a check, refresh or
//! sign-in reaches the platform from the address the account uses.

use super::CredentialProvider;
use crate::domain::proxy::ProxyTarget;
use crate::proxies::ProxyError;
use crate::utils::http_client::{ClientCache, apply_proxy, build_client, install_rustls_provider};

/// The client provider calls use on `proxy`. It follows redirects.
pub(crate) fn provider_client(proxy: &ProxyTarget) -> Result<reqwest::Client, ProxyError> {
    client(proxy, true)
}

/// The client `provider`'s calls use on `proxy`: one that follows redirects,
/// unless the provider follows them itself.
pub(crate) fn provider_client_for(
    provider: &dyn CredentialProvider,
    proxy: &ProxyTarget,
) -> Result<reqwest::Client, ProxyError> {
    client(proxy, provider.follows_redirects())
}

/// reqwest has no per-request redirect setting, so each policy has its own
/// client per proxy.
fn client(proxy: &ProxyTarget, follow_redirects: bool) -> Result<reqwest::Client, ProxyError> {
    static CLIENTS: ClientCache<(ProxyTarget, bool)> = ClientCache::new(64);
    CLIENTS.get_or_try_build(&(proxy.clone(), follow_redirects), || {
        install_rustls_provider();
        let builder = apply_proxy(reqwest::Client::builder(), proxy);
        build_client(if follow_redirects {
            builder
        } else {
            builder.redirect(reqwest::redirect::Policy::none())
        })
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// Answers `/` with a redirect to `/next`, and `/next` with 200.
    async fn redirecting_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match socket.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => head.extend_from_slice(&buf[..read]),
                    }
                }
                let response = if head.starts_with(b"GET /next ") {
                    "HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 302 Found\r\nlocation: /next\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        base
    }

    #[tokio::test]
    async fn only_providers_that_follow_redirects_themselves_see_them() {
        let base = redirecting_server().await;
        let status = |platform: &str| {
            let client =
                provider_client_for(super::super::provider(platform), &ProxyTarget::Direct)
                    .unwrap();
            let base = base.clone();
            async move {
                tokio::time::timeout(std::time::Duration::from_secs(10), client.get(base).send())
                    .await
                    .expect("local request timed out")
                    .unwrap()
                    .status()
            }
        };
        assert_eq!(status("weibo").await, reqwest::StatusCode::FOUND);
        assert_eq!(status("douyu").await, reqwest::StatusCode::OK);
        assert_eq!(status("weibo").await, reqwest::StatusCode::FOUND);
    }
}
