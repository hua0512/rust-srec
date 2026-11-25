//! Account preparation, proactive renewal, reactive repair, and failure classification.

use std::collections::HashSet;
use std::time::Instant;

use platforms_parser::extractor::error::ExtractorError;

use super::state::{
    AttemptStep, Attempts, Candidate, Rest, fail_over_throttle, next_renewal_at, provider_error,
};
use super::{CredentialExecutionService, CredentialSnapshot};
use crate::credentials::{
    AccountStatus, CredentialEvent, CredentialProfile, CredentialValidity, HealthReason,
    OperationDeadline, ProfileError, RefreshedCredentials, provider_client_for,
};
use crate::proxies::ResolvedRoute;
use crate::{Error, Result};

impl CredentialExecutionService {
    /// Refreshes the account with its provider through `route`.
    pub(super) async fn repair(
        &self,
        profile: &CredentialProfile,
        platform: &str,
        route: &ResolvedRoute,
        deadline: OperationDeadline,
    ) -> Result<CredentialProfile> {
        let provider = self.providers.provider(platform);
        if !provider.capabilities().refresh {
            return Err(ProfileError::InvalidMaterial(
                "profile cannot be refreshed by this provider",
            )
            .into());
        }
        self.providers
            .admission()
            .admit(&profile.platform_config_id, &route.key, deadline)
            .await
            .map_err(provider_error)?;
        match provider
            .refresh(
                &provider_client_for(provider, &route.target)?,
                &profile.material()?,
            )
            .await
        {
            Ok(replacement) => {
                let updated = self.repository.refreshed(profile, &replacement).await?;
                if updated.revision != profile.revision {
                    self.providers
                        .maybe_notify_credential_event(CredentialEvent::Refreshed {
                            profile_id: Some(updated.id.clone()),
                            profile_label: Some(updated.label.clone()),
                            scope: self.display_scope(&updated.owner()).await,
                            platform: platform.to_owned(),
                            expires_at: replacement.expires_at,
                            timestamp: jiff::Timestamp::now(),
                        });
                }
                Ok(updated)
            }
            Err(error) => {
                let requires_login = error.requires_relogin();
                self.repository
                    .publish_health(
                        profile,
                        if requires_login {
                            CredentialValidity::Invalid
                        } else {
                            CredentialValidity::NeedsRefresh
                        },
                        Some(if requires_login {
                            HealthReason::LoginRequired
                        } else {
                            HealthReason::RefreshFailed
                        }),
                        false,
                    )
                    .await?;
                let (count, notify) = self.repository.record_refresh_failure(profile).await?;
                if notify {
                    tracing::warn!(profile_id = %profile.id, platform, failure_count = count, requires_login, "Credential profile refresh failed");
                    self.providers
                        .maybe_notify_credential_event(CredentialEvent::RefreshFailed {
                            profile_id: Some(profile.id.clone()),
                            profile_label: Some(profile.label.clone()),
                            scope: self.display_scope(&profile.owner()).await,
                            platform: platform.to_owned(),
                            error: if requires_login {
                                HealthReason::LoginRequired
                            } else {
                                HealthReason::RefreshFailed
                            }
                            .as_str()
                            .to_owned(),
                            requires_relogin: requires_login,
                            failure_count: count.min(u32::MAX as i64) as u32,
                            timestamp: jiff::Timestamp::now(),
                        });
                }
                if requires_login {
                    Err(Error::Extractor(ExtractorError::Authentication {
                        code: "login_required".into(),
                    }))
                } else {
                    let error = provider_error(error);
                    self.defer_throttle(&profile.platform_config_id, route, &error);
                    Err(error)
                }
            }
        }
    }

    pub(super) async fn renew(
        &self,
        profile: CredentialProfile,
        platform: &str,
        route: &ResolvedRoute,
        deadline: OperationDeadline,
    ) -> Result<CredentialProfile> {
        match self.repair(&profile, platform, route, deadline).await {
            Err(Error::Monitor(_)) => {
                tracing::warn!(
                    profile_id = %profile.id,
                    platform,
                    "Could not renew the credential profile; using its current material"
                );
                Ok(profile)
            }
            result => result,
        }
    }

    pub(super) async fn prepare(
        &self,
        step: &AttemptStep<'_>,
        platform: &str,
        checked: &mut HashSet<String>,
        repaired: &mut HashSet<String>,
    ) -> Result<CredentialProfile> {
        let (id, route, deadline) = (step.id, step.route, step.deadline);
        let lock = self.refresh_lock(id);
        let _guard = lock.lock().await;
        let (profile, health) = self.repository.get_with_health(id).await?;
        if !profile.enabled {
            return Err(ProfileError::SourceChanged.into());
        }
        let force_repair = step.repair == Some(profile.revision);
        if !force_repair
            && health
                .as_ref()
                .is_some_and(|health| health.validity == CredentialValidity::Invalid)
        {
            return Err(Error::Extractor(ExtractorError::Authentication {
                code: "login_required".into(),
            }));
        }
        if force_repair {
            if !repaired.insert(id.to_owned()) {
                return Err(Error::Extractor(ExtractorError::Authentication {
                    code: "login_required".into(),
                }));
            }
            return self.repair(&profile, platform, route, deadline).await;
        }
        let provider = self.providers.provider(platform);
        let capabilities = provider.capabilities();
        if next_renewal_at(provider, &profile, health.as_ref())?
            .is_some_and(|due| crate::database::time::now_ms() >= due)
            && repaired.insert(id.to_owned())
        {
            return self.renew(profile, platform, route, deadline).await;
        }
        if !capabilities.check {
            return Ok(profile);
        }
        let today = jiff::Timestamp::now()
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date();
        let status = if let Some(health) = health.as_ref().filter(|health| {
            health.validity == CredentialValidity::Invalid
                || health
                    .last_check_at
                    .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
                    .is_some_and(|time| time.to_zoned(jiff::tz::TimeZone::UTC).date() == today)
        }) {
            match health.validity {
                CredentialValidity::Valid => return Ok(profile),
                CredentialValidity::Invalid => {
                    return Err(Error::Extractor(ExtractorError::Authentication {
                        code: "login_required".into(),
                    }));
                }
                CredentialValidity::Unknown | CredentialValidity::NeedsRefresh => {
                    AccountStatus::Repairable
                }
            }
        } else if checked.insert(id.to_owned()) {
            match self
                .provider_check(provider, &profile, route, deadline)
                .await?
            {
                Ok(status) => status,
                Err(error)
                    if provider.accounts_optional()
                        && !matches!(
                            error,
                            Error::Extractor(ExtractorError::RateLimited { .. })
                        ) =>
                {
                    tracing::debug!(
                        profile_id = %profile.id,
                        platform,
                        %error,
                        "Could not check the credential profile; using it unchecked"
                    );
                    AccountStatus::Unverifiable
                }
                Err(error) => return Err(error),
            }
        } else {
            return Ok(profile);
        };
        match status {
            AccountStatus::Valid => {
                self.repository
                    .publish_health(&profile, CredentialValidity::Valid, None, true)
                    .await?;
                Ok(profile)
            }
            AccountStatus::Unverifiable => Ok(profile),
            AccountStatus::Repairable if capabilities.refresh && repaired.insert(id.to_owned()) => {
                self.repository
                    .publish_health(
                        &profile,
                        CredentialValidity::NeedsRefresh,
                        Some(HealthReason::RepairRequired),
                        true,
                    )
                    .await?;
                self.repair(&profile, platform, route, deadline).await
            }
            AccountStatus::Repairable | AccountStatus::Revoked => {
                self.mark_invalid(&profile, platform).await?;
                Err(Error::Extractor(ExtractorError::Authentication {
                    code: "login_required".into(),
                }))
            }
        }
    }

    pub(super) async fn store_session(
        &self,
        profile: &CredentialProfile,
        platform: &str,
        cookies: String,
        snapshot: &mut CredentialSnapshot,
    ) -> Result<()> {
        let replacement = RefreshedCredentials {
            cookies: cookies.clone(),
            refresh_token: None,
            access_token: None,
            expires_at: None,
        };
        match self.repository.refreshed(profile, &replacement).await {
            Ok(updated) => {
                self.providers.session_saved(&profile.id);
                snapshot.material = updated.material()?;
                snapshot.binding.revision = updated.revision as u64;
            }
            Err(error) => {
                snapshot.material.cookies = cookies;
                if matches!(
                    error,
                    srec_sqlite::DbError::CredentialProfile(
                        ProfileError::SourceChanged | ProfileError::Disabled
                    )
                ) {
                    tracing::debug!(
                        profile_id = %profile.id,
                        "Credential profile changed during extraction; minted session cookies not stored"
                    );
                } else {
                    let scope = self.display_scope(&profile.owner()).await;
                    let error = Error::from(error);
                    self.providers
                        .report_session_save_failure(profile, scope, platform, &error);
                }
            }
        }
        Ok(())
    }

    pub(super) async fn confirm_health(&self, profile: &CredentialProfile) -> Result<()> {
        let health = self.repository.health(&profile.id).await?;
        let validity = health
            .as_ref()
            .map_or(CredentialValidity::Unknown, |health| health.validity);
        if validity != CredentialValidity::Invalid {
            self.repository
                .publish_health(profile, validity, None, false)
                .await?;
        }
        Ok(())
    }

    pub(super) async fn classify_failure(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
        profile: &CredentialProfile,
        error: Error,
    ) -> Result<()> {
        let id = step.id;
        match error {
            Error::Extractor(ExtractorError::Authentication { .. }) => {
                let provider = self.providers.provider(&attempts.platform);
                if !attempts.repaired.contains(id)
                    && provider.capabilities().refresh
                    && provider.refreshable(&profile.material()?)
                {
                    tracing::info!(
                        profile_id = %id,
                        platform_id = %step.policy.platform_id,
                        attempt = step.attempt + 1,
                        "Credential profile rejected; repairing it before the next attempt"
                    );
                    self.repository
                        .publish_health(
                            profile,
                            CredentialValidity::NeedsRefresh,
                            Some(HealthReason::AuthenticationFailed),
                            false,
                        )
                        .await?;
                    attempts.candidates.push_front(Candidate {
                        id: id.to_owned(),
                        preloaded: Some((profile.clone(), None)),
                        repair: Some(profile.revision),
                        route: step.route.clone(),
                    });
                } else if let Some(rest) = provider.rejection_rest() {
                    self.resting.lock().insert(
                        id.to_owned(),
                        Rest {
                            revision: profile.revision,
                            until: Instant::now() + rest,
                        },
                    );
                    tracing::warn!(
                        profile_id = %id,
                        platform = %attempts.platform,
                        rest_secs = rest.as_secs(),
                        "Credential profile turned away by the platform; resting it before trying it again"
                    );
                    attempts.exclusions.resting = true;
                    attempts.previous = Some((id.to_owned(), "resting"));
                } else {
                    self.mark_invalid(profile, &attempts.platform).await?;
                    attempts.exclusions.login_required = true;
                    attempts.previous = Some((id.to_owned(), "authentication_failed"));
                }
                Ok(())
            }
            error @ Error::Extractor(ExtractorError::RateLimited { .. }) => {
                self.defer_throttle(&step.policy.platform_id, step.route, &error);
                fail_over_throttle(step, attempts, error)
            }
            error => Err(error),
        }
    }
}
