//! Platform account providers.
//!
//! A provider holds everything platform-specific about accounts: which
//! material a profile accepts, what the extractor receives from it, and the
//! provider calls that check, refresh or sign in an account. Platforms
//! without a dedicated provider use [`CookieProvider`]: cookies only, with no
//! provider calls.
//!
//! Provider calls receive the HTTP client for the account's network path, so
//! a check or refresh leaves through the same proxy as the account's
//! recordings.

use async_trait::async_trait;
use reqwest::Client;
use serde::Serialize;

use chrono::{DateTime, Utc};

use super::platforms::{BilibiliProvider, DouyuProvider, SoopProvider, TwitchProvider};
use super::{CredentialError, CredentialMaterial};

/// What accounts on a platform support. Profile validation, the extractor
/// handoff and the account UI all follow these.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ProviderCapabilities {
    /// Profiles may hold a refresh token.
    pub refresh_token: bool,
    /// Profiles may hold an access token.
    pub access_token: bool,
    /// An access token alone authenticates, without cookies.
    pub token_only: bool,
    /// Profiles may hold a username and password for automatic sign-in.
    pub reauth_login: bool,
    /// Accounts can be added by scanning a QR code.
    pub qr_login: bool,
    /// The provider can check whether an account still works.
    pub check: bool,
    /// The provider can repair a rejected account without the user.
    pub refresh: bool,
}

/// The outcome of checking an account with its provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    Valid,
    /// Rejected, but a refresh may recover it without the user.
    Repairable,
    /// Rejected; only signing in again recovers it.
    Revoked,
    /// The account holds nothing the provider can check, so requests using it
    /// are no different from anonymous ones.
    Unverifiable,
}

/// Replacement material from a successful refresh. Absent tokens keep the
/// profile's current ones.
#[derive(Clone)]
pub struct RefreshedCredentials {
    pub cookies: String,
    pub refresh_token: Option<String>,
    pub access_token: Option<String>,
    /// Expected expiration time, if known.
    pub expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for RefreshedCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshedCredentials")
            .field("material", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A started QR login: the code to show and the handle to poll it with.
pub struct QrLoginStart {
    pub url: String,
    pub auth_code: String,
    /// Provider lifetime of the code in seconds, when it states one.
    pub expires_in: Option<u64>,
}

impl std::fmt::Debug for QrLoginStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QrLoginStart")
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum QrLoginPoll {
    Waiting { scanned: bool },
    Expired,
    Completed(CredentialMaterial),
}

#[async_trait]
pub trait CredentialProvider: Send + Sync {
    fn capabilities(&self) -> ProviderCapabilities;

    /// Extractor settings the account supplies besides its cookies.
    fn extractor_authentication(
        &self,
        _material: &CredentialMaterial,
    ) -> serde_json::Map<String, serde_json::Value> {
        serde_json::Map::new()
    }

    /// Checks the account with the provider. Only called when
    /// [`ProviderCapabilities::check`] is set.
    async fn check(
        &self,
        _client: &Client,
        _material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        Ok(AccountStatus::Unverifiable)
    }

    /// Whether [`refresh`](Self::refresh) can renew this account. Only
    /// consulted when [`ProviderCapabilities::refresh`] is set; accounts it
    /// rejects are offered no refresh.
    fn refreshable(&self, _material: &CredentialMaterial) -> bool {
        true
    }

    /// Age after which a refreshable account is refreshed before it is used,
    /// for platforms whose sessions lapse on a schedule without the extractor
    /// noticing. The age counts from the account's last refresh, or from the
    /// last change to its profile when it has not been refreshed since.
    fn renew_after(&self) -> Option<std::time::Duration> {
        None
    }

    /// Repairs a rejected account. Only called when
    /// [`ProviderCapabilities::refresh`] is set.
    async fn refresh(
        &self,
        _client: &Client,
        _material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        Err(unsupported())
    }

    /// Only called when [`ProviderCapabilities::qr_login`] is set.
    async fn start_qr_login(&self, _client: &Client) -> Result<QrLoginStart, CredentialError> {
        Err(unsupported())
    }

    async fn poll_qr_login(
        &self,
        _client: &Client,
        _auth_code: &str,
    ) -> Result<QrLoginPoll, CredentialError> {
        Err(unsupported())
    }
}

fn unsupported() -> CredentialError {
    CredentialError::Internal("operation not supported by this provider".into())
}

/// Accounts are cookies passed to the extractor as they are.
pub struct CookieProvider;

#[async_trait]
impl CredentialProvider for CookieProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }
}

/// The provider for accounts on `platform` (a platform name, any case).
pub fn provider(platform: &str) -> &'static dyn CredentialProvider {
    if platform.eq_ignore_ascii_case("bilibili") {
        &BilibiliProvider
    } else if platform.eq_ignore_ascii_case("douyu") {
        &DouyuProvider
    } else if platform.eq_ignore_ascii_case("soop") {
        &SoopProvider
    } else if platform.eq_ignore_ascii_case("twitch") {
        &TwitchProvider
    } else {
        &CookieProvider
    }
}

/// Configuration keys that are account login on `platform` rather than
/// content settings. Elsewhere `username`/`password` are room passwords.
pub fn login_fields(platform: &str) -> &'static [&'static str] {
    if provider(platform).capabilities().reauth_login {
        &["username", "password"]
    } else {
        &[]
    }
}

/// The automatic sign-in login in `config`, when `platform` supports one and
/// both fields are present.
pub(crate) fn platform_reauth_extra(
    platform: &str,
    config: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    if !provider(platform).capabilities().reauth_login {
        return None;
    }
    let field = |name: &str| {
        config?
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    Some(serde_json::json!({
        "username": field("username")?,
        "password": field("password")?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_platforms_get_cookie_accounts_without_provider_calls() {
        assert_eq!(
            provider("huya").capabilities(),
            ProviderCapabilities::default()
        );
        assert!(provider("SOOP").capabilities().reauth_login);
        assert_eq!(login_fields("Soop"), ["username", "password"]);
        assert!(login_fields("bigo").is_empty());
    }

    #[test]
    fn reauth_login_needs_both_fields_on_a_platform_that_supports_it() {
        let login = serde_json::json!({"username": " user ", "password": "pass"});
        assert_eq!(
            platform_reauth_extra("soop", Some(&login)),
            Some(serde_json::json!({"username": "user", "password": "pass"}))
        );
        assert_eq!(platform_reauth_extra("bigo", Some(&login)), None);
        assert_eq!(
            platform_reauth_extra("soop", Some(&serde_json::json!({"username": "user"}))),
            None
        );
    }
}
