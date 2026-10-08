//! Credential provider registry.
//!
//! Resolves each platform's [`CredentialProvider`], and holds the shared
//! platform admission and the notification sink for credential events. Account
//! selection, refresh and health live in [`super::CredentialExecutionService`].

#[cfg(test)]
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use tracing::warn;

use crate::notification::{NotificationEvent, NotificationService};

use super::provider::CredentialProvider;
use super::types::{CredentialEvent, CredentialScope};
use super::{CredentialProfile, PlatformAdmission};

pub struct CredentialProviderRegistry {
    admission: Arc<PlatformAdmission>,
    /// Whether platforms use their built-in providers. Without them every
    /// account is plain cookies and no provider is ever called.
    platform_providers: bool,
    /// Test replacements for the built-in providers, by lowercase platform name.
    #[cfg(test)]
    providers: HashMap<String, Arc<dyn CredentialProvider>>,
    notification_service: OnceLock<Arc<NotificationService>>,
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
            #[cfg(test)]
            providers: HashMap::new(),
            notification_service: OnceLock::new(),
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
    pub(crate) fn provider(&self, platform_name: &str) -> &dyn CredentialProvider {
        #[cfg(test)]
        if let Some(provider) = self.providers.get(&platform_name.to_ascii_lowercase()) {
            return provider.as_ref();
        }
        if self.platform_providers {
            super::provider(platform_name)
        } else {
            &super::CookieProvider
        }
    }

    /// Wire a NotificationService to emit CredentialEvents as NotificationEvents.
    pub fn set_notification_service(&self, service: Arc<NotificationService>) {
        self.admission.set_notification_service(service.clone());
        if self.notification_service.set(service).is_err() {
            warn!("Credential notification service is already configured");
        }
    }

    #[cfg(test)]
    pub(crate) fn has_notification_service(&self) -> bool {
        self.notification_service.get().is_some()
    }

    /// Replace the provider for a platform.
    #[cfg(test)]
    pub(crate) fn register_provider(
        &mut self,
        platform_name: &str,
        provider: Arc<dyn CredentialProvider>,
    ) {
        self.providers
            .insert(platform_name.to_ascii_lowercase(), provider);
    }

    pub(crate) fn maybe_notify_credential_event(&self, event: CredentialEvent) {
        let Some(service) = self.notification_service.get().cloned() else {
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

        service.dispatch_notification(NotificationEvent::Credential { event });
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
                timestamp: Utc::now(),
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

    #[tokio::test]
    async fn notification_service_is_installed_once() {
        let service = CredentialProviderRegistry::new();
        let first = Arc::new(NotificationService::new());

        service.set_notification_service(Arc::clone(&first));
        service.set_notification_service(Arc::new(NotificationService::new()));

        let installed = service
            .notification_service
            .get()
            .expect("notification service should be installed");
        assert!(Arc::ptr_eq(installed, &first));
    }

    #[tokio::test]
    async fn repeated_session_save_failures_notify_once_until_a_save_succeeds() {
        let service = CredentialProviderRegistry::new();
        let notifications = Arc::new(NotificationService::new());
        let mut received = notifications.subscribe();
        service.set_notification_service(notifications);
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
        assert!(matches!(
            first,
            NotificationEvent::Credential {
                event: CredentialEvent::SessionSaveFailed { ref profile_label, .. }
            } if profile_label.as_deref() == Some("Main")
        ));

        service.session_saved(&profile.id);
        service.report_session_save_failure(&profile, scope, "soop", &error);
        // Only the failure after the successful save is announced again.
        assert!(next().await.timestamp() >= first.timestamp());
    }
}
