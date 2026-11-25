//! Profile management actions (`validate_profile`, `refresh_profile`), locks, and unavailability notifications.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use super::state::{Exclusions, USE_RECORD_WINDOW, management_error, provider_error};
use super::{CredentialExecutionService, ProfileActionResult};
use crate::credentials::{
    AccountStatus, CredentialEvent, CredentialOwner, CredentialProfile, CredentialProvider,
    CredentialScope, CredentialUnavailable, CredentialValidity, HealthReason, OperationDeadline,
    ProfileError, ResolvedCredentialPolicy, UnavailableReason, provider_client_for,
};
use crate::proxies::ResolvedRoute;
use crate::{Error, Result};

impl CredentialExecutionService {
    pub(super) async fn mark_invalid(
        &self,
        profile: &CredentialProfile,
        platform: &str,
    ) -> Result<()> {
        if self.repository.mark_invalid(profile).await? {
            tracing::warn!(profile_id = %profile.id, platform, "Credential profile needs a new login");
            self.providers
                .maybe_notify_credential_event(CredentialEvent::Invalid {
                    profile_id: Some(profile.id.clone()),
                    profile_label: Some(profile.label.clone()),
                    scope: self.display_scope(&profile.owner()).await,
                    platform: platform.to_owned(),
                    reason: "login_required".into(),
                    error_code: None,
                    timestamp: jiff::Timestamp::now(),
                });
        }
        Ok(())
    }

    pub(super) fn refresh_lock(&self, id: &str) -> Arc<AsyncMutex<()>> {
        self.locks.retain(|_, lock| lock.strong_count() > 0);
        let mut entry = self.locks.entry(id.to_owned()).or_default();
        if let Some(lock) = entry.upgrade() {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        *entry = Arc::downgrade(&lock);
        lock
    }

    pub fn unavailable_status(
        &self,
        policy: &ResolvedCredentialPolicy,
    ) -> Option<CredentialUnavailable> {
        self.unavailable
            .lock()
            .get(&policy.generation)
            .map(|(status, _)| status.clone())
    }

    pub(super) async fn unavailable(
        &self,
        policy: &ResolvedCredentialPolicy,
        reason: UnavailableReason,
    ) -> Error {
        let status = CredentialUnavailable {
            reason,
            policy_generation: policy.generation.clone(),
        };
        let notify = {
            let mut cache = self.unavailable.lock();
            let notify = cache
                .get(&policy.generation)
                .is_none_or(|(_, time)| time.elapsed() >= Duration::from_secs(600));
            if cache.len() >= 4096
                && !cache.contains_key(&policy.generation)
                && let Some(oldest) = cache
                    .iter()
                    .min_by_key(|(_, (_, time))| *time)
                    .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
            let notified = if notify {
                Instant::now()
            } else {
                cache[&policy.generation].1
            };
            cache.insert(policy.generation.clone(), (status.clone(), notified));
            notify
        };
        if notify {
            tracing::warn!(
                platform_id = %policy.platform_id,
                owner_kind = policy.owner.kind(),
                owner_id = policy.owner.id(),
                strategy = policy.selection.strategy_name(),
                reason = reason.as_str(),
                "Credential selection exhausted"
            );
            let platform = match self.repository.platform_name(&policy.platform_id).await {
                Ok(name) => name,
                Err(error) => {
                    tracing::warn!(%error, platform_id = %policy.platform_id, "Could not load the platform name for a credential notification");
                    policy.platform_id.clone()
                }
            };
            self.providers
                .maybe_notify_credential_event(CredentialEvent::Unavailable {
                    scope: self.display_scope(&policy.owner).await,
                    platform,
                    reason_code: reason,
                    timestamp: jiff::Timestamp::now(),
                });
        } else {
            tracing::debug!(
                platform_id = %policy.platform_id,
                strategy = policy.selection.strategy_name(),
                reason = reason.as_str(),
                "Credential selection still exhausted"
            );
        }
        Error::CredentialUnavailable(status)
    }

    pub(super) async fn unavailable_after(
        &self,
        policy: &ResolvedCredentialPolicy,
        exclusions: &Exclusions,
        fallback: UnavailableReason,
        anonymous_fallback: bool,
    ) -> Error {
        self.exhausted(
            policy,
            exclusions.reason().unwrap_or(fallback),
            anonymous_fallback,
        )
        .await
    }

    pub(super) async fn exhausted(
        &self,
        policy: &ResolvedCredentialPolicy,
        reason: UnavailableReason,
        anonymous_fallback: bool,
    ) -> Error {
        if anonymous_fallback {
            return Error::CredentialUnavailable(CredentialUnavailable {
                reason,
                policy_generation: policy.generation.clone(),
            });
        }
        self.unavailable(policy, reason).await
    }

    /// Owner display names for notifications; a failed lookup keeps the ID.
    pub(super) async fn display_scope(&self, owner: &CredentialOwner) -> CredentialScope {
        match self.repository.scope(owner).await {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(%error, owner_id = owner.id(), "Could not load the credential owner name for a notification");
                CredentialScope::for_owner(owner, owner.id().to_owned())
            }
        }
    }

    pub async fn validate_profile(
        &self,
        id: &str,
        deadline: OperationDeadline,
    ) -> Result<ProfileActionResult> {
        tokio::time::timeout_at(deadline.instant(), async {
            let lock = self.refresh_lock(id);
            let _guard = lock.lock().await;
            let profile = self.repository.get(id).await?;
            if !profile.enabled {
                return Err(ProfileError::Disabled.into());
            }
            let platform = self
                .repository
                .platform_name(&profile.platform_config_id)
                .await?;
            let provider = self.providers.provider(&platform);
            if !provider.capabilities().check {
                return self.action(profile, false).await;
            }
            let route = self.repository.account_route(&profile, None).await?;
            let status = self
                .provider_check(provider, &profile, &route, deadline)
                .await??;
            let validity = match status {
                AccountStatus::Valid => CredentialValidity::Valid,
                AccountStatus::Repairable => CredentialValidity::NeedsRefresh,
                AccountStatus::Revoked => CredentialValidity::Invalid,
                AccountStatus::Unverifiable => {
                    return Err(ProfileError::InvalidMaterial(
                        "this account has nothing the provider can check",
                    )
                    .into());
                }
            };
            self.repository
                .publish_health(
                    &profile,
                    validity,
                    Some(HealthReason::ManualValidation),
                    true,
                )
                .await?;
            self.action(profile, true).await
        })
        .await
        .map_err(|_| Error::from(ProfileError::ProviderUnavailable("deadline_exceeded")))?
        .map_err(management_error)
    }

    pub async fn refresh_profile(
        &self,
        id: &str,
        deadline: OperationDeadline,
    ) -> Result<ProfileActionResult> {
        tokio::time::timeout_at(deadline.instant(), async {
            let lock = self.refresh_lock(id);
            let _guard = lock.lock().await;
            let profile = self.repository.get(id).await?;
            if !profile.enabled {
                return Err(ProfileError::Disabled.into());
            }
            let platform = self
                .repository
                .platform_name(&profile.platform_config_id)
                .await?;
            let provider = self.providers.provider(&platform);
            if !provider.capabilities().refresh || !provider.refreshable(&profile.material()?) {
                return self.action(profile, false).await;
            }
            let route = self.repository.account_route(&profile, None).await?;
            let profile = self.repair(&profile, &platform, &route, deadline).await?;
            self.action(profile, true).await
        })
        .await
        .map_err(|_| Error::from(ProfileError::ProviderUnavailable("deadline_exceeded")))?
        .map_err(management_error)
    }

    pub(super) async fn provider_check(
        &self,
        provider: &dyn CredentialProvider,
        profile: &CredentialProfile,
        route: &ResolvedRoute,
        deadline: OperationDeadline,
    ) -> Result<Result<AccountStatus>> {
        self.providers
            .admission()
            .admit(&profile.platform_config_id, &route.key, deadline)
            .await
            .map_err(provider_error)?;
        let client = provider_client_for(provider, &route.target)?;
        let material = profile.material()?;
        Ok(provider.check(&client, &material).await.map_err(|error| {
            let error = provider_error(error);
            self.defer_throttle(&profile.platform_config_id, route, &error);
            error
        }))
    }

    async fn action(
        &self,
        profile: CredentialProfile,
        supported: bool,
    ) -> Result<ProfileActionResult> {
        Ok(ProfileActionResult {
            supported,
            health: self.repository.health(&profile.id).await?,
            profile: profile.summary(),
        })
    }

    pub(super) async fn record_use(&self, id: &str) {
        let now = Instant::now();
        {
            let mut recorded = self.used.lock();
            if recorded
                .get(id)
                .is_some_and(|at| now.duration_since(*at) < USE_RECORD_WINDOW)
            {
                return;
            }
            recorded.retain(|_, at| now.duration_since(*at) < USE_RECORD_WINDOW);
            recorded.insert(id.to_owned(), now);
        }
        if let Err(error) = self
            .repository
            .record_use(id, srec_sqlite::time::now_ms(), USE_RECORD_WINDOW)
            .await
        {
            tracing::warn!(profile_id = %id, %error, "Could not record the credential profile's last use");
        }
    }

    pub(super) fn excluded_while_resting(
        &self,
        profile: &CredentialProfile,
        exclusions: &mut Exclusions,
    ) -> bool {
        let resting =
            self.resting.lock().get(&profile.id).is_some_and(|rest| {
                rest.revision == profile.revision && Instant::now() < rest.until
            });
        exclusions.resting |= resting;
        resting
    }

    pub(super) fn defer_throttle(&self, platform_id: &str, route: &ResolvedRoute, error: &Error) {
        if let Error::Extractor(error) = error {
            self.providers
                .admission()
                .observe(platform_id, route, error);
        }
    }
}
