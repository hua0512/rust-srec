//! Test doubles shared by the tests that drive the credential refresh flow.

use async_trait::async_trait;

use super::error::CredentialError;
use super::manager::{CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials};

/// A [`CredentialManager`] that reports the stored credentials as needing a refresh and hands back
/// a fixed [`RefreshedCredentials`].
///
/// Lets a test drive `CredentialRefreshService::check_and_refresh_source` — and therefore
/// `CredentialStore::update_credentials` — to a successful refresh without a platform API.
/// `platform_id` must match the `platform_name` of the config layer under test, lowercased, since
/// `CredentialRefreshService::register_manager` keys managers by it.
pub(crate) struct StubCredentialManager {
    platform_id: &'static str,
    cookies: String,
    refresh_token: String,
}

impl StubCredentialManager {
    pub(crate) fn new(platform_id: &'static str, cookies: &str, refresh_token: &str) -> Self {
        Self {
            platform_id,
            cookies: cookies.to_string(),
            refresh_token: refresh_token.to_string(),
        }
    }
}

#[async_trait]
impl CredentialManager for StubCredentialManager {
    fn platform_id(&self) -> &'static str {
        self.platform_id
    }

    async fn check_status(&self, _cookies: &str) -> Result<CredentialStatus, CredentialError> {
        Ok(CredentialStatus::NeedsRefresh {
            refresh_deadline: None,
        })
    }

    async fn refresh(
        &self,
        _state: &RefreshState,
    ) -> Result<RefreshedCredentials, CredentialError> {
        Ok(RefreshedCredentials {
            cookies: self.cookies.clone(),
            refresh_token: Some(self.refresh_token.clone()),
            access_token: None,
            expires_at: None,
        })
    }

    async fn validate(&self, _cookies: &str) -> Result<bool, CredentialError> {
        Ok(true)
    }
}
