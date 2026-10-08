//! SOOP account provider.
//!
//! Checks session cookies via `get_private_info.php` and signs in again with
//! the account's username and password when the session is no longer valid.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::Client;
use tracing::{debug, instrument, warn};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::RefreshedCredentials;
use crate::credentials::provider::{AccountStatus, CredentialProvider, ProviderCapabilities};

use platforms_parser::extractor::platforms::soop::{login_for_cookies, validate_session};

pub struct SoopProvider;

fn login(material: &CredentialMaterial) -> Option<(&str, &str)> {
    let config = material.reauth_config.as_ref()?;
    let field = |name: &str| {
        config
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some((field("username")?, field("password")?))
}

#[async_trait]
impl CredentialProvider for SoopProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            reauth_login: true,
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    /// The extractor signs in by itself when a stream needs it and the
    /// session cookies are missing or rejected.
    fn extractor_authentication(
        &self,
        material: &CredentialMaterial,
    ) -> serde_json::Map<String, serde_json::Value> {
        login(material)
            .map(|(username, password)| {
                serde_json::Map::from_iter([
                    ("username".to_owned(), username.into()),
                    ("password".to_owned(), password.into()),
                ])
            })
            .unwrap_or_default()
    }

    #[instrument(skip_all)]
    async fn check(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        let session_valid = !material.cookies.trim().is_empty()
            && validate_session(client, &material.cookies)
                .await
                .inspect_err(|error| {
                    warn!(category = error.category(), "SOOP session check failed")
                })
                .map_err(map_provider_error)?;
        Ok(if session_valid {
            debug!("SOOP session cookies are valid");
            AccountStatus::Valid
        } else if login(material).is_some() {
            debug!("SOOP session is missing or invalid; signing in again");
            AccountStatus::Repairable
        } else {
            AccountStatus::Revoked
        })
    }

    #[instrument(skip_all)]
    async fn refresh(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        let (username, password) = login(material).ok_or_else(|| {
            CredentialError::InvalidCredentials(
                "SOOP re-login requires this account's username and password".to_string(),
            )
        })?;

        // A session that became valid again since the check is kept.
        if !material.cookies.trim().is_empty()
            && validate_session(client, &material.cookies)
                .await
                .map_err(map_provider_error)?
        {
            debug!("SOOP session still valid; skipping re-login");
            return Ok(RefreshedCredentials {
                cookies: material.cookies.clone(),
                refresh_token: None,
                access_token: None,
                expires_at: Some(Utc::now() + Duration::days(7)),
            });
        }

        debug!("SOOP re-login with the account's username/password");
        let cookies = login_for_cookies(client, username, password)
            .await
            .map_err(map_provider_error)?;

        Ok(RefreshedCredentials {
            cookies,
            refresh_token: None,
            access_token: None,
            expires_at: Some(Utc::now() + Duration::days(7)),
        })
    }
}

fn map_provider_error(
    error: platforms_parser::extractor::error::ExtractorError,
) -> CredentialError {
    use platforms_parser::extractor::error::ExtractorError;
    match error {
        ExtractorError::Authentication { .. } => {
            CredentialError::InvalidCredentials("login_required".to_string())
        }
        error => error.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::provider_client;

    fn material(cookies: &str, reauth: Option<serde_json::Value>) -> CredentialMaterial {
        CredentialMaterial {
            cookies: cookies.into(),
            refresh_token: None,
            access_token: None,
            reauth_config: reauth,
        }
    }

    #[tokio::test]
    async fn a_missing_session_is_repairable_only_with_a_login() {
        let login = serde_json::json!({"username": "user", "password": "pass"});
        assert_eq!(
            SoopProvider
                .check(
                    &provider_client(&platforms_parser::proxy::ProxyTarget::Direct).unwrap(),
                    &material(" ", Some(login.clone()))
                )
                .await
                .unwrap(),
            AccountStatus::Repairable
        );
        assert_eq!(
            SoopProvider
                .check(
                    &provider_client(&platforms_parser::proxy::ProxyTarget::Direct).unwrap(),
                    &material("", None)
                )
                .await
                .unwrap(),
            AccountStatus::Revoked
        );
        assert_eq!(
            SoopProvider
                .extractor_authentication(&material("", Some(login)))
                .get("username"),
            Some(&serde_json::json!("user"))
        );
    }
}
