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
    use super::*;
    use crate::credentials::provider_client;

    fn material(cookies: &str, access_token: Option<&str>) -> CredentialMaterial {
        CredentialMaterial {
            cookies: cookies.into(),
            refresh_token: None,
            access_token: access_token.map(str::to_owned),
            reauth_config: None,
        }
    }

    #[tokio::test]
    async fn an_account_without_a_token_is_unverifiable_without_a_request() {
        for material in [material("", None), material("unique_id=a", Some("  "))] {
            assert_eq!(
                TwitchProvider
                    .check(
                        &provider_client(&platforms_parser::proxy::ProxyTarget::Direct).unwrap(),
                        &material
                    )
                    .await
                    .unwrap(),
                AccountStatus::Unverifiable
            );
        }
    }

    #[test]
    fn the_access_token_reaches_the_extractor_as_its_oauth_token() {
        assert_eq!(
            TwitchProvider
                .extractor_authentication(&material("", Some("token")))
                .get("oauth_token"),
            Some(&serde_json::json!("token"))
        );
        assert!(
            TwitchProvider
                .extractor_authentication(&material("auth-token=a", None))
                .is_empty()
        );
    }
}
