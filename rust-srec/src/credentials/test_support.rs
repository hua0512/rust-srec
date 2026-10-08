//! Test doubles shared by the tests that drive the credential refresh flow.

use std::sync::Arc;

use async_trait::async_trait;

use super::error::CredentialError;
use super::{
    AccountStatus, CredentialMaterial, CredentialProvider, PlatformAdmission, ProviderCapabilities,
    RefreshedCredentials,
};

/// A [`CredentialProvider`] that reports every account as needing a refresh
/// and hands back fixed [`RefreshedCredentials`].
///
/// Lets a test drive a profile refresh to success without a platform API.
/// Register it with `CredentialProviderRegistry::register_provider` under the
/// `platform_name` of the config layer under test.
pub(crate) struct StubCredentialProvider {
    cookies: String,
    refresh_token: String,
}

impl StubCredentialProvider {
    pub(crate) fn new(cookies: &str, refresh_token: &str) -> Self {
        Self {
            cookies: cookies.to_string(),
            refresh_token: refresh_token.to_string(),
        }
    }
}

#[async_trait]
impl CredentialProvider for StubCredentialProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            check: true,
            refresh: true,
            ..ProviderCapabilities::default()
        }
    }

    async fn check(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> Result<AccountStatus, CredentialError> {
        Ok(AccountStatus::Repairable)
    }

    async fn refresh(
        &self,
        _client: &reqwest::Client,
        _material: &CredentialMaterial,
    ) -> Result<RefreshedCredentials, CredentialError> {
        Ok(RefreshedCredentials {
            cookies: self.cookies.clone(),
            refresh_token: Some(self.refresh_token.clone()),
            access_token: None,
            expires_at: None,
        })
    }
}

/// Admission with enough tokens that tests never wait on the rate limiter.
pub(crate) fn unthrottled_admission() -> Arc<PlatformAdmission> {
    Arc::new(PlatformAdmission::new(
        crate::monitor::RateLimiterManager::with_config(crate::monitor::RateLimiterConfig {
            max_tokens: 100,
            initial_tokens: 100,
            refill_rate: 100.0,
        }),
    ))
}
