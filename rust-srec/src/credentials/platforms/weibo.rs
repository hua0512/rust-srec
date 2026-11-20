//! Weibo account provider.
//!
//! A QR login holds the cookies a browser sends to `weibo.com`, which the
//! extractor uses, and, as its refresh token, an account token with the
//! account's uid and every cookie the sign-in set on Weibo's and Sina's
//! sites. The check asks Weibo's post timeline whether the cookies are still
//! signed in. The provider calls get a client that does not follow
//! redirects: the sign-in keeps the cookies each redirect sets, and the check
//! tells a sign-in redirect from a block.

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, instrument};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::{
    AccountStatus, CredentialProvider, ProviderCapabilities, QrLoginPoll, QrLoginStart,
};

use platforms_parser::extractor::platforms::weibo::passport::{
    self, AccountState, PassportError, QrStatus,
};

pub struct WeiboProvider;

fn map_error(error: PassportError) -> CredentialError {
    match error {
        PassportError::Response(error) => error.into(),
        PassportError::Network(error) => CredentialError::Network(error),
        PassportError::Parse(what) => CredentialError::ParseError(what.to_owned()),
        // Weibo documents none of these codes, so a refusal is not taken to
        // mean the login was revoked.
        PassportError::Api(code) => {
            CredentialError::RefreshFailed(format!("Weibo passport error {code}"))
        }
    }
}

#[async_trait]
impl CredentialProvider for WeiboProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            refresh_token: true,
            qr_login: true,
            check: true,
            ..ProviderCapabilities::default()
        }
    }

    fn follows_redirects(&self) -> bool {
        false
    }

    /// Cookies pasted without a QR login have no account token, so their uid
    /// comes from the `weibo.com` page they open.
    #[instrument(skip_all)]
    async fn check(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        let state =
            passport::check_account(client, &material.cookies, material.refresh_token.as_deref())
                .await
                .map_err(map_error)?;
        debug!(?state, "Weibo account checked");
        Ok(match state {
            AccountState::Valid => AccountStatus::Valid,
            AccountState::SignedOut => AccountStatus::Revoked,
            AccountState::Unknown => AccountStatus::Unverifiable,
        })
    }

    #[instrument(skip_all)]
    async fn start_qr_login(&self, client: &Client) -> Result<QrLoginStart, CredentialError> {
        let qr = passport::generate_qr(client).await.map_err(map_error)?;
        Ok(QrLoginStart {
            url: qr.url,
            auth_code: qr.qrid,
            expires_in: None,
        })
    }

    #[instrument(skip_all)]
    async fn poll_qr_login(
        &self,
        client: &Client,
        auth_code: &str,
    ) -> Result<QrLoginPoll, CredentialError> {
        let status = passport::poll_qr(client, auth_code)
            .await
            .map_err(map_error)?;
        Ok(match status {
            QrStatus::NotScanned => QrLoginPoll::Waiting { scanned: false },
            QrStatus::Scanned => QrLoginPoll::Waiting { scanned: true },
            QrStatus::Expired => QrLoginPoll::Expired,
            QrStatus::Confirmed(session) => QrLoginPoll::Completed(CredentialMaterial {
                cookies: session.cookies,
                refresh_token: Some(session.account_token),
                access_token: None,
                reauth_config: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_passport_refusal_is_not_a_revoked_login() {
        let refused = map_error(PassportError::Api(50_114_099));
        assert!(matches!(refused, CredentialError::RefreshFailed(_)));
        assert!(!refused.requires_relogin());
    }
}
