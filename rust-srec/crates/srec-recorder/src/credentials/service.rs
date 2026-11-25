//! Credential provider registry.
//!
//! Resolves each platform's [`CredentialProvider`], and holds the shared
//! platform admission and the notification sink for credential events. Account
//! selection, refresh and health live in [`super::CredentialExecutionService`].

#[cfg(any(test, feature = "test-support"))]
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tracing::warn;

use srec_core::domain::notification::{RuntimeAlert, RuntimeAlertSink};

use super::provider::CredentialProvider;
use super::{CredentialEvent, CredentialProfile, CredentialScope, PlatformAdmission};

pub struct CredentialProviderRegistry {
    admission: Arc<PlatformAdmission>,
    /// Whether platforms use their built-in providers. Without them every
    /// account is plain cookies, and only providers registered for the
    /// process (`register_provider_for_process`) are called.
    platform_providers: bool,
    /// Test replacements for the built-in providers, by lowercase platform name.
    #[cfg(any(test, feature = "test-support"))]
    providers: HashMap<String, Arc<dyn CredentialProvider>>,
    alert_sink: OnceLock<Arc<dyn RuntimeAlertSink>>,
    /// Profiles whose latest session-cookie save failed. A failure repeats on
    /// every live check, so it is announced once until a save succeeds.
    session_save_failures: dashmap::DashSet<String>,
}

impl Default for CredentialProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialProviderRegistry {
    pub fn new() -> Self {
        Self {
            admission: Arc::new(PlatformAdmission::from_config(
                &crate::monitor::StreamMonitorConfig::default(),
            )),
            platform_providers: false,
            #[cfg(any(test, feature = "test-support"))]
            providers: HashMap::new(),
            alert_sink: OnceLock::new(),
            session_save_failures: dashmap::DashSet::new(),
        }
    }

    pub fn with_admission(mut self, admission: Arc<PlatformAdmission>) -> Self {
        self.admission = admission;
        self
    }

    pub fn admission(&self) -> Arc<PlatformAdmission> {
        self.admission.clone()
    }

    /// Enable the built-in provider of each platform (QR login aside, which
    /// always uses it).
    pub fn with_platform_providers(mut self) -> Self {
        self.platform_providers = true;
        self
    }

    /// The provider whose calls check and refresh accounts on the platform.
    pub fn provider(&self, platform_name: &str) -> &dyn CredentialProvider {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(provider) = self.providers.get(&platform_name.to_ascii_lowercase()) {
            return provider.as_ref();
        }
        if let Some(provider) = super::provider::registered(platform_name) {
            return provider;
        }
        if self.platform_providers {
            super::provider(platform_name)
        } else {
            &super::CookieProvider
        }
    }

    /// Wire a [`RuntimeAlertSink`] to emit credential and throttling alerts.
    pub fn set_notification_service(&self, sink: Arc<dyn RuntimeAlertSink>) {
        self.admission.set_notification_service(sink.clone());
        if self.alert_sink.set(sink).is_err() {
            warn!("Credential notification sink is already configured");
        }
    }

    /// Replace the provider for a platform.
    #[cfg(any(test, feature = "test-support"))]
    pub fn register_provider(
        &mut self,
        platform_name: &str,
        provider: Arc<dyn CredentialProvider>,
    ) {
        self.providers
            .insert(platform_name.to_ascii_lowercase(), provider);
    }

    pub(crate) fn maybe_notify_credential_event(&self, event: CredentialEvent) {
        let Some(sink) = self.alert_sink.get().cloned() else {
            return;
        };

        // Basic anti-spam gating for recurring failures.
        if let CredentialEvent::RefreshFailed {
            requires_relogin,
            failure_count,
            ..
        } = &event
        {
            let should_notify = *requires_relogin || *failure_count == 1 || *failure_count % 3 == 0;
            if !should_notify {
                return;
            }
        }

        sink.clone().emit(RuntimeAlert::Credential(event));
    }

    /// Session cookies minted during extraction (e.g. SOOP reactive login)
    /// could not be stored on their profile. The check keeps its result; the
    /// failure is announced once per profile until a save succeeds.
    pub(crate) fn report_session_save_failure(
        &self,
        profile: &CredentialProfile,
        scope: CredentialScope,
        platform: &str,
        error: &crate::Error,
    ) {
        warn!(
            %error,
            profile_id = %profile.id,
            platform,
            "Failed to persist session cookies from extract"
        );
        if self.session_save_failures.insert(profile.id.clone()) {
            self.maybe_notify_credential_event(CredentialEvent::SessionSaveFailed {
                profile_id: Some(profile.id.clone()),
                profile_label: Some(profile.label.clone()),
                scope,
                platform: platform.to_owned(),
                error: error.to_string(),
                timestamp: jiff::Timestamp::now(),
            });
        }
    }

    pub(crate) fn session_saved(&self, profile_id: &str) {
        self.session_save_failures.remove(profile_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::test_support::TestAlertSink;

    #[tokio::test]
    async fn repeated_session_save_failures_notify_once_until_a_save_succeeds() {
        let service = CredentialProviderRegistry::new();
        let (alerts, mut received) = TestAlertSink::new();
        service.set_notification_service(alerts);
        let profile = CredentialProfile {
            id: "profile-a".into(),
            platform_config_id: "platform-soop".into(),
            label: "Main".into(),
            enabled: true,
            cookies: "AuthTicket=a".into(),
            refresh_token: None,
            access_token: None,
            reauth_config: None,
            revision: 1,
            version: 1,
            created_at: 0,
            updated_at: 0,
            last_used_at: None,
            proxy_route: "inherit".into(),
            proxy_id: None,
        };
        let scope = CredentialScope::Platform {
            platform_id: "platform-soop".into(),
            platform_name: "soop".into(),
        };
        let error = crate::Error::DatabaseSqlx(sqlx::Error::PoolTimedOut);
        let mut next = async || {
            tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
                .await
                .unwrap()
                .unwrap()
        };

        service.report_session_save_failure(&profile, scope.clone(), "soop", &error);
        service.report_session_save_failure(&profile, scope.clone(), "soop", &error);
        let first = next().await;
        let first_ts = match first {
            RuntimeAlert::Credential(CredentialEvent::SessionSaveFailed {
                ref profile_label,
                timestamp,
                ..
            }) if profile_label.as_deref() == Some("Main") => timestamp,
            other => panic!("unexpected alert: {other:?}"),
        };

        service.session_saved(&profile.id);
        service.report_session_save_failure(&profile, scope, "soop", &error);
        // Only the failure after the successful save is announced again.
        let second_ts = match next().await {
            RuntimeAlert::Credential(CredentialEvent::SessionSaveFailed { timestamp, .. }) => {
                timestamp
            }
            other => panic!("unexpected alert: {other:?}"),
        };
        assert!(second_ts >= first_ts);
    }
}
