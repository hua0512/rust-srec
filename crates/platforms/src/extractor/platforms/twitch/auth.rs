//! Twitch OAuth token helpers.
//!
//! Shared by the extractor, which sends the token on GQL requests, and by the
//! app account provider, which checks the same token proactively.

use reqwest::Client;
use tracing::debug;

use crate::extractor::error::ExtractorError;
use crate::extractor::utils::parse_cookie_header;

const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
/// The browser cookie holding the web player's OAuth token.
const AUTH_TOKEN_COOKIE: &str = "auth-token";

/// The OAuth token requests should carry: an explicit token, else the
/// `auth-token` cookie copied from a signed-in browser. The `oauth:` prefix
/// that chat (IRC) tokens use is not part of the HTTP token.
pub fn resolve_oauth_token(explicit: Option<&str>, cookies: Option<&str>) -> Option<String> {
    fn bare(token: &str) -> Option<String> {
        let token = token.trim();
        let token = token
            .get(..6)
            .filter(|prefix| prefix.eq_ignore_ascii_case("oauth:"))
            .map_or(token, |_| token[6..].trim());
        (!token.is_empty()).then(|| token.to_owned())
    }

    explicit.and_then(bare).or_else(|| {
        parse_cookie_header(cookies?)
            .into_iter()
            .find(|(name, _)| name == AUTH_TOKEN_COOKIE)
            .and_then(|(_, value)| bare(&value))
    })
}

/// Returns whether Twitch still accepts the OAuth token. Web player tokens
/// carry no expiry, so a rejection means the session was revoked (sign-out,
/// password change) and only signing in again recovers it.
pub async fn validate_oauth_token(client: &Client, token: &str) -> Result<bool, ExtractorError> {
    validate_oauth_token_at(client, VALIDATE_URL, token).await
}

async fn validate_oauth_token_at(
    client: &Client,
    url: &str,
    token: &str,
) -> Result<bool, ExtractorError> {
    let response = client
        .get(url)
        .header(
            reqwest::header::AUTHORIZATION,
            format!("OAuth {}", token.trim()),
        )
        .send()
        .await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        debug!("Twitch rejected the OAuth token");
        return Ok(false);
    }
    ExtractorError::check_response(response)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn serve_once(status: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = "{}";
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        format!("http://{addr}/oauth2/validate")
    }

    async fn validate_against(status: &'static str) -> Result<bool, ExtractorError> {
        let url = serve_once(status).await;
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            validate_oauth_token_at(&crate::extractor::default::default_client(), &url, "token"),
        )
        .await
        .expect("local validate request timed out")
    }

    #[tokio::test]
    async fn validation_distinguishes_accepted_revoked_and_throttled_tokens() {
        assert!(validate_against("200 OK").await.unwrap());
        assert!(!validate_against("401 Unauthorized").await.unwrap());
        assert!(matches!(
            validate_against("429 Too Many Requests").await,
            Err(ExtractorError::RateLimited { .. })
        ));
        assert!(matches!(
            validate_against("503 Service Unavailable").await,
            Err(ExtractorError::HttpError(_))
        ));
    }

    #[test]
    fn explicit_token_wins_over_the_browser_cookie_and_loses_its_chat_prefix() {
        let cookies = Some("unique_id=a; auth-token=from-cookie; other=b");

        assert_eq!(
            resolve_oauth_token(Some(" explicit "), cookies).as_deref(),
            Some("explicit")
        );
        assert_eq!(
            resolve_oauth_token(Some("  "), cookies).as_deref(),
            Some("from-cookie")
        );
        assert_eq!(
            resolve_oauth_token(Some("OAuth:abc"), None).as_deref(),
            Some("abc")
        );
        assert_eq!(resolve_oauth_token(Some("oauth:"), None), None);
        assert_eq!(resolve_oauth_token(None, Some("unique_id=a")), None);
        assert_eq!(resolve_oauth_token(None, Some("auth-token=")), None);
        assert_eq!(resolve_oauth_token(None, None), None);
    }
}
