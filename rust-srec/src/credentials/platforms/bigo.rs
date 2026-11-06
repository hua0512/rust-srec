//! Bigo account provider.
//!
//! Accounts are cookies pasted from a browser. Bigo's `getUserInfo` tells a
//! working login from a lapsed one and sets some of the login's cookies
//! again, as the website does on every page load, so the provider checks and
//! refreshes accounts with it, and renews them hourly before use.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use tracing::instrument;

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::{
    AccountStatus, CredentialProvider, ProviderCapabilities, RefreshedCredentials,
};

use platforms_parser::extractor::platforms::bigo::account::{self, AccountSession};

/// How long a Bigo account rests after a login wall.
const REJECTION_REST: Duration = Duration::from_secs(30 * 60);
/// How often a login's cookies are fetched again before use.
const RENEW_AFTER: Duration = Duration::from_secs(60 * 60);

pub struct BigoProvider;

#[async_trait]
impl CredentialProvider for BigoProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    /// Bigo answers with its login wall both for a lapsed login and, for a
    /// while, for a working account it has flagged as unusual traffic.
    fn rejection_rest(&self) -> Option<Duration> {
        Some(REJECTION_REST)
    }

    fn renew_after(&self) -> Option<Duration> {
        Some(RENEW_AFTER)
    }

    #[instrument(skip_all)]
    async fn check(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        Ok(match account::user_info(client, &material.cookies).await? {
            AccountSession::SignedIn { .. } => AccountStatus::Valid,
            AccountSession::SignedOut => AccountStatus::Revoked,
        })
    }

    #[instrument(skip_all)]
    async fn refresh(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        match account::user_info(client, &material.cookies).await? {
            AccountSession::SignedIn { cookies } => Ok(RefreshedCredentials {
                cookies,
                refresh_token: None,
                access_token: None,
                expires_at: None,
            }),
            AccountSession::SignedOut => {
                Err(CredentialError::InvalidCredentials("signed_out".into()))
            }
        }
    }
}
