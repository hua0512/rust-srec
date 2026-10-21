//! Twitch account provider.
//!
//! Checks the account's OAuth token with Twitch's token validation endpoint.
//! Web player tokens have no refresh token and no expiry; a rejected token
//! was revoked and needs a new sign-in, so accounts are never refreshed.

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, instrument, warn};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::{AccountStatus, CredentialProvider, ProviderCapabilities};

use platforms_parser::extractor::platforms::twitch::{resolve_oauth_token, validate_oauth_token};

pub struct TwitchProvider;

#[async_trait]
impl CredentialProvider for TwitchProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            access_token: true,
            token_only: true,
            check: true,
            ..ProviderCapabilities::default()
        }
    }

    /// The extractor sends the access token as its OAuth token; without one
    /// it falls back to the browser's `auth-token` cookie.
    fn extractor_authentication(
        &self,
        material: &CredentialMaterial,
    ) -> serde_json::Map<String, serde_json::Value> {
        material
            .access_token
            .iter()
            .map(|token| ("oauth_token".to_owned(), token.clone().into()))
            .collect()
    }

    #[instrument(skip_all)]
    async fn check(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        // The same resolution as the extractor, so this checks what recording sends.
        let Some(token) =
            resolve_oauth_token(material.access_token.as_deref(), Some(&material.cookies))
        else {
            return Ok(AccountStatus::Unverifiable);
        };
        match validate_oauth_token(client, &token).await {
            Ok(true) => {
                debug!("Twitch OAuth token is valid");
                Ok(AccountStatus::Valid)
            }
            Ok(false) => Ok(AccountStatus::Revoked),
            Err(error) => {
                warn!(category = error.category(), "Twitch token check failed");
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use platforms_parser::extractor::platform_extractor::PlatformExtractor;
    use platforms_parser::extractor::platforms::twitch::Twitch;

    use super::*;

    fn material(cookies: &str, access_token: Option<&str>) -> CredentialMaterial {
        CredentialMaterial {
            cookies: cookies.into(),
            refresh_token: None,
            access_token: access_token.map(str::to_owned),
            reauth_config: None,
        }
    }

    /// The extras this provider hands the extractor are what the platforms
    /// crate's Twitch extractor reads its OAuth token from; a key renamed on
    /// either side would leave recordings unauthenticated.
    #[test]
    fn the_access_token_reaches_the_extractor_as_its_oauth_token() {
        let extras = TwitchProvider.extractor_authentication(&material("", Some("token")));
        let twitch = Twitch::new(
            "https://www.twitch.tv/someone".to_owned(),
            Client::new(),
            None,
            Some(serde_json::Value::Object(extras)),
        );
        assert_eq!(
            twitch
                .get_extractor()
                .get_platform_headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("OAuth token")
        );
    }

    /// Without a token there is nothing to validate, so the check reports
    /// the account unverifiable rather than failing. The client's proxy is a
    /// closed loopback port, so a check that did send a request fails here
    /// instead of reaching Twitch.
    #[tokio::test]
    async fn an_account_without_a_token_is_unverifiable_without_a_request() {
        let client = Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
            .build()
            .unwrap();
        for material in [material("", None), material("unique_id=a", Some("  "))] {
            assert_eq!(
                TwitchProvider.check(&client, &material).await.unwrap(),
                AccountStatus::Unverifiable
            );
        }
    }
}
