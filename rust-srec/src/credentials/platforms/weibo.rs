//! Weibo account provider.
//!
//! A QR login holds the `.weibo.com` cookies, which the extractor uses,
//! and, as its refresh token, an account token with the
//! account's uid and every cookie the sign-in set on Weibo's and Sina's
//! sites. The check asks Weibo's post timeline whether the cookies are still
//! signed in. An account Weibo signed out is renewed from its account token,
//! which only a QR login holds: a pasted Cookie header names no domains, so
//! it cannot rebuild the per-site cookies a renewal sends. The provider
//! calls get a client that does not follow redirects: the sign-in keeps the
//! cookies each redirect sets, and the check tells a sign-in redirect from a
//! block.

use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, instrument};

use crate::credentials::CredentialMaterial;
use crate::credentials::error::CredentialError;
use crate::credentials::provider::{
    AccountStatus, CredentialProvider, ProviderCapabilities, QrLoginPoll, QrLoginStart,
    RefreshedCredentials,
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
        PassportError::LoginRequired => CredentialError::InvalidRefreshToken,
        // A spent account token and Weibo's request checks both answer 403,
        // so a refusal is retried later instead of asking for a new login.
        error @ (PassportError::Refused | PassportError::AccountMismatch) => {
            CredentialError::RefreshFailed(error.to_string())
        }
    }
}

/// A signed-out account that can renew is repaired; only one that cannot
/// needs a new login.
fn account_status(state: AccountState, material: &CredentialMaterial) -> AccountStatus {
    match state {
        AccountState::Valid => AccountStatus::Valid,
        AccountState::SignedOut if account_token(material).is_some() => AccountStatus::Repairable,
        AccountState::SignedOut => AccountStatus::Revoked,
        AccountState::Unknown => AccountStatus::Unverifiable,
    }
}

/// The account token of a QR login, while it can still renew the account.
fn account_token(material: &CredentialMaterial) -> Option<&str> {
    material
        .refresh_token
        .as_deref()
        .filter(|token| passport::renewable(token))
}

#[async_trait]
impl CredentialProvider for WeiboProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            refresh_token: true,
            qr_login: true,
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    fn refreshable(&self, material: &CredentialMaterial) -> bool {
        account_token(material).is_some()
    }

    /// Renews an account Weibo signed out. The renewal replaces the account
    /// token's 30-day cookies; once the new ones expire, no renewal can
    /// work, so that is the material's expiry.
    #[instrument(skip_all)]
    async fn refresh(
        &self,
        client: &Client,
        material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        let token = account_token(material).ok_or(CredentialError::MissingRefreshToken)?;
        let session = passport::renew_session(client, token)
            .await
            .inspect_err(|error| debug!(%error, "Weibo account renewal failed"))
            .map_err(map_error)?;
        debug!("Weibo account renewed");
        Ok(RefreshedCredentials {
            cookies: session.cookies,
            refresh_token: Some(session.account_token),
            access_token: None,
            expires_at: session.renewable_until,
        })
    }

    fn follows_redirects(&self) -> bool {
        false
    }

    /// User links resolve through a guest session; the account is only a
    /// fallback.
    fn accounts_optional(&self) -> bool {
        true
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
                // Passport errors name no cookie, token or address.
                .inspect_err(|error| debug!(%error, "Weibo account check failed"))
                .map_err(map_error)?;
        debug!(?state, "Weibo account checked");
        Ok(account_status(state, material))
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

    fn material(refresh_token: Option<&str>) -> CredentialMaterial {
        CredentialMaterial {
            cookies: "SUB=com-sub".into(),
            refresh_token: refresh_token.map(str::to_owned),
            access_token: None,
            reauth_config: None,
        }
    }

    #[test]
    fn only_a_qr_login_that_can_renew_is_repaired_when_signed_out() {
        let renewable = r#"{"v":1,"uid":"1000000002","jar":[{"domain":".weibo.com","name":"ALF","value":"a","expires":4102444800}]}"#;
        let spent = r#"{"v":1,"uid":"1000000002","jar":[{"domain":".weibo.com","name":"SCF","value":"s"}]}"#;
        assert!(WeiboProvider.refreshable(&material(Some(renewable))));
        assert_eq!(
            account_status(AccountState::SignedOut, &material(Some(renewable))),
            AccountStatus::Repairable
        );
        for unrenewable in [material(None), material(Some(spent))] {
            assert!(!WeiboProvider.refreshable(&unrenewable));
            assert_eq!(
                account_status(AccountState::SignedOut, &unrenewable),
                AccountStatus::Revoked
            );
        }
    }

    #[test]
    fn only_the_sign_in_page_asks_for_a_new_login() {
        assert!(matches!(
            map_error(PassportError::LoginRequired),
            CredentialError::InvalidRefreshToken
        ));
        for refused in [
            PassportError::Api(50_114_099),
            PassportError::Refused,
            PassportError::AccountMismatch,
        ] {
            let refused = map_error(refused);
            assert!(matches!(refused, CredentialError::RefreshFailed(_)));
            assert!(!refused.requires_relogin());
        }
    }
}
