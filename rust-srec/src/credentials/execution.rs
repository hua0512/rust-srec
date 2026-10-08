//! One bounded credential operation owns selection, repair and typed failover.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use platforms_parser::extractor::error::ExtractorError;
use serde::Serialize;
use tokio::sync::Mutex as AsyncMutex;

use crate::database::repositories::CredentialProfileRepository;
use crate::proxies::{ResolvedRoute, RouteKey};
use crate::{Error, Result};

use super::{
    AccountStatus, CredentialBinding, CredentialError, CredentialIdentity, CredentialMaterial,
    CredentialProfile, CredentialProfileHealth, CredentialProfileSummary,
    CredentialProviderRegistry, CredentialSelection, CredentialUnavailable, CredentialValidity,
    HealthReason, OperationDeadline, PoolStrategy, ProfileError, RefreshedCredentials,
    ResolvedCredentialPolicy, UnavailableReason,
};

#[derive(Clone)]
pub struct CredentialSnapshot {
    pub binding: CredentialBinding,
    pub material: CredentialMaterial,
    /// The route every request using this snapshot takes: extraction,
    /// downloads, danmu and playback. Platforms may tie what they hand out to
    /// the address that asked for it.
    pub route: ResolvedRoute,
}

impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSnapshot")
            .field("binding", &self.binding)
            .field("material", &"[redacted]")
            .field("route", &self.route.kind())
            .finish()
    }
}

pub struct Extracted<T> {
    pub value: T,
    pub session_cookies: Option<String>,
    /// A positive offline/content result says nothing about account validity.
    pub preserve_health: bool,
}

pub struct CredentialExecution<T> {
    pub value: T,
    pub snapshot: CredentialSnapshot,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProfileActionResult {
    pub supported: bool,
    pub profile: CredentialProfileSummary,
    pub health: Option<CredentialProfileHealth>,
}

struct Cursor {
    next: usize,
    used: Instant,
    owner: super::CredentialOwner,
    platform_id: String,
}

/// An account still to try in one operation.
struct Candidate {
    id: String,
    /// Repair the account before extracting with it.
    repair: bool,
    /// The route the account's requests take.
    route: ResolvedRoute,
}

/// Per-operation state carried across attempts.
struct Attempts {
    candidates: VecDeque<Candidate>,
    exclusions: Exclusions,
    /// Accounts whose status was checked by this operation.
    checked: HashSet<String>,
    /// Accounts this operation already repaired; each is repaired at most once.
    repaired: HashSet<String>,
    /// Accounts already retried after changing mid-attempt.
    source_retries: HashSet<String>,
    /// The last failed account and why, logged when failover moves on.
    previous: Option<(String, &'static str)>,
    platform: String,
    /// The operation's route, which accounts without their own follow.
    operation: ResolvedRoute,
}

impl Attempts {
    /// Requeues an account that changed mid-attempt, once per operation.
    fn retry_source(&mut self, id: &str, route: ResolvedRoute) -> bool {
        let first = self.source_retries.insert(id.to_owned());
        if first {
            self.candidates.push_front(Candidate {
                id: id.to_owned(),
                repair: false,
                route,
            });
        }
        first
    }
}

/// The inputs of one attempt that stay fixed while it runs.
struct AttemptStep<'a> {
    policy: &'a ResolvedCredentialPolicy,
    binding: Option<&'a CredentialBinding>,
    deadline: OperationDeadline,
    attempt: u8,
    id: &'a str,
    repair: bool,
    route: &'a ResolvedRoute,
}

/// Why candidates were passed over, so exhaustion can say what would help.
#[derive(Debug, Default)]
pub struct Exclusions {
    disabled: bool,
    login_required: bool,
}

impl Exclusions {
    /// Records why a stored profile cannot be selected now; true when it can.
    pub fn admits(&mut self, enabled: bool, health: Option<&CredentialProfileHealth>) -> bool {
        if !enabled {
            self.disabled = true;
            return false;
        }
        if health.is_some_and(|health| health.validity == CredentialValidity::Invalid) {
            self.login_required = true;
            return false;
        }
        true
    }

    pub fn reason(&self) -> Option<UnavailableReason> {
        if self.login_required {
            Some(UnavailableReason::LoginRequired)
        } else if self.disabled {
            Some(UnavailableReason::ProfilesDisabled)
        } else {
            None
        }
    }
}

pub struct CredentialExecutionService {
    repository: Arc<CredentialProfileRepository>,
    providers: Arc<CredentialProviderRegistry>,
    cursors: Mutex<HashMap<String, Cursor>>,
    locks: dashmap::DashMap<String, Weak<AsyncMutex<()>>>,
    unavailable: Mutex<HashMap<String, (CredentialUnavailable, Instant)>>,
    /// When this process last wrote each account's last use.
    used: Mutex<HashMap<String, Instant>>,
}

impl CredentialExecutionService {
    async fn mark_invalid(&self, profile: &CredentialProfile, platform: &str) -> Result<()> {
        if self.repository.mark_invalid(profile).await? {
            tracing::warn!(profile_id = %profile.id, platform, "Credential profile needs a new login");
            self.providers
                .maybe_notify_credential_event(super::CredentialEvent::Invalid {
                    profile_id: Some(profile.id.clone()),
                    profile_label: Some(profile.label.clone()),
                    scope: self.display_scope(&profile.owner()).await,
                    platform: platform.to_owned(),
                    reason: "login_required".into(),
                    error_code: None,
                    timestamp: Utc::now(),
                });
        }
        Ok(())
    }
    pub fn new(
        repository: Arc<CredentialProfileRepository>,
        providers: Arc<CredentialProviderRegistry>,
    ) -> Self {
        Self {
            repository,
            providers,
            cursors: Mutex::new(HashMap::new()),
            locks: dashmap::DashMap::new(),
            unavailable: Mutex::new(HashMap::new()),
            used: Mutex::new(HashMap::new()),
        }
    }

    pub fn repository(&self) -> &Arc<CredentialProfileRepository> {
        &self.repository
    }

    fn refresh_lock(&self, id: &str) -> Arc<AsyncMutex<()>> {
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

    async fn unavailable(
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
                .maybe_notify_credential_event(super::CredentialEvent::Unavailable {
                    scope: self.display_scope(&policy.owner).await,
                    platform,
                    reason_code: reason,
                    timestamp: Utc::now(),
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

    async fn unavailable_after(
        &self,
        policy: &ResolvedCredentialPolicy,
        exclusions: &Exclusions,
        fallback: UnavailableReason,
    ) -> Error {
        self.unavailable(policy, exclusions.reason().unwrap_or(fallback))
            .await
    }

    /// Owner display names for notifications; a failed lookup keeps the ID.
    async fn display_scope(&self, owner: &super::CredentialOwner) -> super::CredentialScope {
        match self.repository.scope(owner).await {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(%error, owner_id = owner.id(), "Could not load the credential owner name for a notification");
                super::CredentialScope::for_owner(owner, owner.id().to_owned())
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
            // Health and material writes reject disabled profiles, so a provider
            // call here would only discard its result (including rotated tokens).
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
                .await?;
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

    /// Checks the account with its provider through `route`.
    async fn provider_check(
        &self,
        provider: &dyn super::CredentialProvider,
        profile: &CredentialProfile,
        route: &ResolvedRoute,
        deadline: OperationDeadline,
    ) -> Result<AccountStatus> {
        self.providers
            .admission()
            .admit(&profile.platform_config_id, &route.key, deadline)
            .await
            .map_err(provider_error)?;
        provider
            .check(
                &super::provider_client(&route.target)?,
                &profile.material()?,
            )
            .await
            .map_err(|error| {
                let error = provider_error(error);
                self.defer_throttle(&profile.platform_config_id, route, &error);
                error
            })
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

    /// Refreshes the account with its provider through `route`.
    async fn repair(
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
                &super::provider_client(&route.target)?,
                &profile.material()?,
            )
            .await
        {
            Ok(replacement) => {
                let updated = self.repository.refreshed(profile, &replacement).await?;
                if updated.revision != profile.revision {
                    self.providers.maybe_notify_credential_event(
                        super::CredentialEvent::Refreshed {
                            profile_id: Some(updated.id.clone()),
                            profile_label: Some(updated.label.clone()),
                            scope: self.display_scope(&updated.owner()).await,
                            platform: platform.to_owned(),
                            expires_at: replacement.expires_at,
                            timestamp: Utc::now(),
                        },
                    );
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
                    self.providers.maybe_notify_credential_event(
                        super::CredentialEvent::RefreshFailed {
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
                            timestamp: Utc::now(),
                        },
                    );
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

    /// Refreshes an account whose material is due for renewal. A failure that
    /// does not demand a new login keeps the current material, which works
    /// until it lapses; the renewal is retried after [`RENEWAL_RETRY`].
    async fn renew(
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

    async fn prepare(
        &self,
        step: &AttemptStep<'_>,
        platform: &str,
        checked: &mut HashSet<String>,
        repaired: &mut HashSet<String>,
    ) -> Result<CredentialProfile> {
        let (id, route, deadline, force_repair) = (step.id, step.route, step.deadline, step.repair);
        let lock = self.refresh_lock(id);
        let _guard = lock.lock().await;
        let profile = self.repository.get(id).await?;
        if !profile.enabled {
            return Err(ProfileError::SourceChanged.into());
        }
        let health = self.repository.health(id).await?;
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
        let today = Utc::now().date_naive();
        let status = if let Some(health) = health.as_ref().filter(|health| {
            health.validity == CredentialValidity::Invalid
                || health
                    .last_check_at
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .is_some_and(|time| time.date_naive() == today)
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
            self.provider_check(provider, &profile, route, deadline)
                .await?
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
            // Health stays unchecked; recording proceeds as it would anonymously.
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

    /// A bound poll cannot switch accounts. Startup/recovery may pass allow_switch
    /// only when the caller owns the session and will commit the replacement epoch.
    ///
    /// `route` is the operation's route; an account with its own route uses
    /// that instead, for every request made with it.
    pub async fn execute<T, F, Fut>(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        deadline: OperationDeadline,
        route: &ResolvedRoute,
        extract: F,
    ) -> Result<CredentialExecution<T>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        let operation =
            self.execute_within(policy, binding, allow_switch, deadline, route, extract);
        tokio::time::timeout_at(deadline.instant(), operation)
            .await
            .map_err(|_| Error::Monitor("Credential operation deadline exceeded".into()))?
    }

    async fn execute_within<T, F, Fut>(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        deadline: OperationDeadline,
        route: &ResolvedRoute,
        mut extract: F,
    ) -> Result<CredentialExecution<T>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        self.repository.validate_selection(policy).await?;
        if matches!(
            policy.selection,
            CredentialSelection::Inherit | CredentialSelection::None
        ) {
            return self
                .execute_anonymous(policy, binding, deadline, route, &mut extract)
                .await;
        }
        let mut exclusions = Exclusions::default();
        let candidates = self
            .select_candidates(policy, binding, allow_switch, route, &mut exclusions)
            .await?;
        let platform = self.repository.platform_name(&policy.platform_id).await?;
        let mut attempts = Attempts {
            candidates,
            exclusions,
            checked: HashSet::new(),
            repaired: HashSet::new(),
            source_retries: HashSet::new(),
            previous: None,
            platform,
            operation: route.clone(),
        };
        for attempt in 0..attempt_budget(policy, allow_switch).0 {
            let Some(Candidate { id, repair, route }) = attempts.candidates.pop_front() else {
                break;
            };
            if let Some((failed, outcome)) =
                attempts.previous.take().filter(|(failed, _)| *failed != id)
            {
                tracing::info!(
                    platform_id = %policy.platform_id,
                    strategy = policy.selection.strategy_name(),
                    attempt = attempt + 1,
                    from_profile_id = %failed,
                    to_profile_id = %id,
                    outcome,
                    "Credential failover to the next eligible account"
                );
            }
            let step = AttemptStep {
                policy,
                binding,
                deadline,
                attempt,
                id: &id,
                repair,
                route: &route,
            };
            if let Some(execution) = self.attempt(&step, &mut attempts, &mut extract).await? {
                return Ok(execution);
            }
        }
        // A remaining candidate means the attempt budget, not the accounts, ran out.
        Err(if attempts.candidates.is_empty() {
            self.unavailable_after(
                policy,
                &attempts.exclusions,
                UnavailableReason::AttemptsExhausted,
            )
            .await
        } else {
            self.unavailable(policy, UnavailableReason::AttemptsExhausted)
                .await
        })
    }

    /// `Inherit`/`None` run once without an account, under platform admission.
    async fn execute_anonymous<T, F, Fut>(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        deadline: OperationDeadline,
        route: &ResolvedRoute,
        extract: &mut F,
    ) -> Result<CredentialExecution<T>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        self.providers
            .admission()
            .admit(&policy.platform_id, &route.key, deadline)
            .await
            .map_err(provider_error)?;
        let mut snapshot = CredentialSnapshot {
            binding: CredentialBinding {
                identity: CredentialIdentity::Anonymous,
                revision: 0,
                policy: policy.clone(),
                epoch: binding.map_or(0, |binding| binding.epoch),
            },
            material: CredentialMaterial {
                cookies: String::new(),
                refresh_token: None,
                access_token: None,
                reauth_config: None,
            },
            route: route.clone(),
        };
        let result = extract(snapshot.clone())
            .await
            .inspect_err(|error| self.defer_throttle(&policy.platform_id, route, error))?;
        if let Some(cookies) = result.session_cookies {
            snapshot.material.cookies = cookies;
        }
        self.unavailable.lock().remove(&policy.generation);
        Ok(CredentialExecution {
            value: result.value,
            snapshot,
        })
    }

    /// Orders the selectable accounts for one operation: a bound session's
    /// account (or its saved successor during recovery) first, otherwise the
    /// round-robin start. With failover, accounts whose route is paused by a
    /// throttle move behind the others; without it only the first account is
    /// kept.
    async fn select_candidates(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        route: &ResolvedRoute,
        exclusions: &mut Exclusions,
    ) -> Result<VecDeque<Candidate>> {
        let paths = self.eligible_profiles(policy, route, exclusions).await?;
        let mut eligible: Vec<String> = paths.iter().map(|(id, _)| id.clone()).collect();
        if eligible.is_empty() {
            return Err(self
                .unavailable_after(policy, exclusions, UnavailableReason::AttemptsExhausted)
                .await);
        }
        let bound_id = binding
            .filter(|binding| binding.policy.generation == policy.generation)
            .and_then(|binding| match &binding.identity {
                CredentialIdentity::Profile { profile_id } => Some(profile_id.as_str()),
                _ => None,
            });
        if let Some(id) = bound_id {
            if let Some(index) = eligible.iter().position(|candidate| candidate == id) {
                eligible.rotate_left(index);
            } else if !allow_switch
                || matches!(
                    policy.selection,
                    CredentialSelection::Pool {
                        failover: false,
                        ..
                    }
                )
            {
                return Err(self
                    .unavailable(policy, UnavailableReason::BoundProfileUnavailable)
                    .await);
            } else {
                start_after_bound(policy, id, &mut eligible);
                tracing::info!(
                    platform_id = %policy.platform_id,
                    from_profile_id = %id,
                    to_profile_id = %eligible[0],
                    strategy = policy.selection.strategy_name(),
                    "Bound credential profile is unavailable; recovering with the next saved account"
                );
            }
        } else if binding.is_some() && !allow_switch {
            return Err(self
                .unavailable(policy, UnavailableReason::BindingPolicyChanged)
                .await);
        } else if matches!(
            policy.selection,
            CredentialSelection::Pool {
                strategy: PoolStrategy::RoundRobin,
                ..
            }
        ) {
            let Some(start) = self.round_robin_start(policy, &eligible) else {
                return Err(self
                    .unavailable_after(policy, exclusions, UnavailableReason::AttemptsExhausted)
                    .await);
            };
            if let Some(index) = eligible.iter().position(|id| *id == start) {
                eligible.rotate_left(index);
            }
        }
        let route_of = |id: &str| {
            paths
                .iter()
                .find(|(candidate, _)| candidate == id)
                .map_or_else(|| route.clone(), |(_, route)| route.clone())
        };
        if attempt_budget(policy, allow_switch).1 {
            let admission = self.providers.admission();
            eligible
                .sort_by_key(|id| admission.backing_off(&policy.platform_id, &route_of(id).key));
        } else {
            eligible.truncate(1);
        }
        Ok(eligible
            .into_iter()
            .map(|id| Candidate {
                route: route_of(&id),
                id,
                repair: false,
            })
            .collect())
    }

    /// Saved accounts that are enabled and not invalid, in saved order, with
    /// the route each one takes: its own, else the operation's `route`.
    async fn eligible_profiles(
        &self,
        policy: &ResolvedCredentialPolicy,
        route: &ResolvedRoute,
        exclusions: &mut Exclusions,
    ) -> Result<Vec<(String, ResolvedRoute)>> {
        let mut eligible = Vec::new();
        for id in policy.selection.profile_ids() {
            let profile = self.repository.get(id).await?;
            let health = self.repository.health(id).await?;
            if exclusions.admits(profile.enabled, health.as_ref()) {
                let own = self.repository.account_route(&profile, Some(route)).await?;
                eligible.push((id.to_owned(), own));
            }
        }
        Ok(eligible)
    }

    /// Advances the policy generation's cursor to the next saved account that
    /// is eligible now. Cursors idle for an hour, or superseded by a newer
    /// generation of the same owner and platform, are dropped.
    fn round_robin_start(
        &self,
        policy: &ResolvedCredentialPolicy,
        eligible: &[String],
    ) -> Option<String> {
        let all_ids = policy.selection.profile_ids();
        let mut cursors = self.cursors.lock();
        cursors.retain(|generation, cursor| {
            cursor.used.elapsed() < Duration::from_secs(3600)
                && (generation == &policy.generation
                    || cursor.owner != policy.owner
                    || cursor.platform_id != policy.platform_id)
        });
        if cursors.len() >= 4096
            && !cursors.contains_key(&policy.generation)
            && let Some(oldest) = cursors
                .iter()
                .min_by_key(|(_, cursor)| cursor.used)
                .map(|(key, _)| key.clone())
        {
            cursors.remove(&oldest);
        }
        let cursor = cursors.entry(policy.generation.clone()).or_insert(Cursor {
            next: 0,
            used: Instant::now(),
            owner: policy.owner.clone(),
            platform_id: policy.platform_id.clone(),
        });
        let selected = (0..all_ids.len())
            .map(|offset| (cursor.next + offset) % all_ids.len())
            .find(|index| eligible.iter().any(|id| id == all_ids[*index]))?;
        cursor.next = (selected + 1) % all_ids.len();
        cursor.used = Instant::now();
        Some(all_ids[selected].to_owned())
    }

    /// One attempt with one account. `None` moves on to the next candidate;
    /// an error ends the operation.
    async fn attempt<T, F, Fut>(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
        extract: &mut F,
    ) -> Result<Option<CredentialExecution<T>>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        let Some(profile) = self.prepare_attempt(step, attempts).await? else {
            return Ok(None);
        };
        let Some(snapshot) = self.confirm_attempt(step, attempts, &profile).await? else {
            return Ok(None);
        };
        self.record_use(step.id).await;
        tracing::debug!(
            profile_id = %step.id,
            attempt = step.attempt + 1,
            platform_id = %step.policy.platform_id,
            strategy = step.policy.selection.strategy_name(),
            repair = step.repair,
            "Executing credential-bound extraction"
        );
        match extract(snapshot.clone()).await {
            Ok(result) => {
                self.settle_success(step, attempts, &profile, snapshot, result)
                    .await
            }
            Err(error) => {
                self.classify_failure(step, attempts, &profile, error)
                    .await?;
                Ok(None)
            }
        }
    }

    /// Readies the account: skips it when it became unselectable, and runs
    /// status checks or repair.
    async fn prepare_attempt(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
    ) -> Result<Option<CredentialProfile>> {
        let id = step.id;
        let health = self.repository.health(id).await?;
        if !attempts.exclusions.admits(true, health.as_ref()) {
            return Ok(None);
        }
        let prepared = self
            .prepare(
                step,
                &attempts.platform,
                &mut attempts.checked,
                &mut attempts.repaired,
            )
            .await;
        match prepared {
            Ok(profile) => Ok(Some(profile)),
            Err(Error::Extractor(ExtractorError::Authentication { .. })) => {
                attempts.exclusions.login_required = true;
                attempts.previous = Some((id.to_owned(), "login_required"));
                Ok(None)
            }
            Err(Error::CredentialProfile(ProfileError::SourceChanged))
                if attempts.retry_source(id, step.route.clone()) =>
            {
                Ok(None)
            }
            // The provider call already paused the route it was throttled on.
            Err(error @ Error::Extractor(ExtractorError::RateLimited { .. })) => {
                fail_over_throttle(step, attempts, error)?;
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Builds the snapshot, waits for platform admission, then re-reads the
    /// account: a change while waiting retries it once from its new state.
    async fn confirm_attempt(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
        profile: &CredentialProfile,
    ) -> Result<Option<CredentialSnapshot>> {
        let id = step.id;
        let snapshot = CredentialSnapshot {
            binding: CredentialBinding {
                identity: CredentialIdentity::Profile {
                    profile_id: id.to_owned(),
                },
                revision: profile.revision as u64,
                policy: step.policy.clone(),
                epoch: step.binding.map_or(0, |binding| binding.epoch),
            },
            material: profile.material()?,
            route: step.route.clone(),
        };
        self.providers
            .admission()
            .admit(&step.policy.platform_id, &step.route.key, step.deadline)
            .await
            .map_err(provider_error)?;
        let latest = self.repository.get(id).await?;
        let latest_health = self.repository.health(id).await?;
        if !latest.enabled || latest.revision != profile.revision {
            attempts.exclusions.disabled |= !latest.enabled;
            let route = self
                .repository
                .account_route(&latest, Some(&attempts.operation))
                .await?;
            attempts.retry_source(id, route);
            return Ok(None);
        }
        if !attempts.exclusions.admits(true, latest_health.as_ref()) {
            return Ok(None);
        }
        self.repository.validate_current(profile).await?;
        Ok(Some(snapshot))
    }

    /// Accepts a successful extraction unless the account changed meanwhile,
    /// storing a minted session or closing the account's health check.
    async fn settle_success<T>(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
        profile: &CredentialProfile,
        mut snapshot: CredentialSnapshot,
        result: Extracted<T>,
    ) -> Result<Option<CredentialExecution<T>>> {
        let current = self.repository.get(step.id).await?;
        if !current.enabled || current.revision != profile.revision {
            let route = self
                .repository
                .account_route(&current, Some(&attempts.operation))
                .await?;
            if attempts.retry_source(step.id, route) {
                return Ok(None);
            }
            return Err(ProfileError::SourceChanged.into());
        }
        if let Some(cookies) = result
            .session_cookies
            .filter(|cookies| cookies != &profile.cookies)
        {
            self.store_session(profile, &attempts.platform, cookies, &mut snapshot)
                .await?;
        } else if !result.preserve_health {
            self.confirm_health(profile).await?;
        }
        self.unavailable.lock().remove(&step.policy.generation);
        Ok(Some(CredentialExecution {
            value: result.value,
            snapshot,
        }))
    }

    /// The live result stands even when the minted session cannot be stored;
    /// it then serves this attempt only, at the old revision.
    async fn store_session(
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
                    Error::CredentialProfile(ProfileError::SourceChanged | ProfileError::Disabled)
                ) {
                    tracing::debug!(
                        profile_id = %profile.id,
                        "Credential profile changed during extraction; minted session cookies not stored"
                    );
                } else {
                    let scope = self.display_scope(&profile.owner()).await;
                    self.providers
                        .report_session_save_failure(profile, scope, platform, &error);
                }
            }
        }
        Ok(())
    }

    /// Records that the account worked. A concurrent invalid conclusion stands.
    async fn confirm_health(&self, profile: &CredentialProfile) -> Result<()> {
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

    /// A rejected account is queued for one repair (or marked invalid), a
    /// throttle pauses the account's route and hands over to an account on
    /// another one, and every other failure ends the operation.
    async fn classify_failure(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
        profile: &CredentialProfile,
        error: Error,
    ) -> Result<()> {
        let id = step.id;
        match error {
            Error::Extractor(ExtractorError::Authentication { .. }) => {
                if !attempts.repaired.contains(id)
                    && self
                        .providers
                        .provider(&attempts.platform)
                        .capabilities()
                        .refresh
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
                        repair: true,
                        route: step.route.clone(),
                    });
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

    /// Notes that an operation received the account's material. Each
    /// account is written at most once per [`USE_RECORD_WINDOW`] by this
    /// process, and the stored time only moves on once it is that old, so
    /// frequent polls cost no writes. The time is informational: a failed
    /// write is logged and the operation proceeds.
    async fn record_use(&self, id: &str) {
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
            .record_use(id, crate::database::time::now_ms(), USE_RECORD_WINDOW)
            .await
        {
            tracing::warn!(profile_id = %id, %error, "Could not record the credential profile's last use");
        }
    }

    fn defer_throttle(&self, platform_id: &str, route: &ResolvedRoute, error: &Error) {
        if let Error::Extractor(error) = error {
            self.providers
                .admission()
                .observe(platform_id, route, error);
        }
    }
}

/// How long a failed renewal waits before the next one.
const RENEWAL_RETRY: Duration = Duration::from_secs(60 * 60);

/// How long a recorded use stands before the next use is written.
pub(crate) const USE_RECORD_WINDOW: Duration = Duration::from_secs(5 * 60);

/// When the account is next renewed before use, in epoch milliseconds, or
/// `None` when its provider never renews it: the provider has no renewal age,
/// cannot refresh this account's material, or the account needs a new login.
///
/// The age counts from the last refresh at the current revision, else from
/// the profile's last change, which a refresh or new material also updates.
/// A failed renewal waits an hour first. A time in the past means
/// the next operation using the account renews it.
pub fn next_renewal_at(
    provider: &dyn super::CredentialProvider,
    profile: &CredentialProfile,
    health: Option<&CredentialProfileHealth>,
) -> Result<Option<i64>> {
    let Some(age) = provider.renew_after() else {
        return Ok(None);
    };
    if !provider.capabilities().refresh
        || health.is_some_and(|health| health.validity == CredentialValidity::Invalid)
        || !provider.refreshable(&profile.material()?)
    {
        return Ok(None);
    }
    let after = |since: i64, wait: Duration| {
        since.saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX))
    };
    let since = health
        .and_then(|health| health.last_refresh_at)
        .unwrap_or(profile.updated_at);
    let retry = health
        .and_then(|health| health.last_failure_at)
        .map_or(i64::MIN, |failed| after(failed, RENEWAL_RETRY));
    Ok(Some(after(since, age).max(retry)))
}

/// After a throttle on the step's route, the operation continues only with
/// accounts on other routes: the same route would meet the same throttle.
fn fail_over_throttle(step: &AttemptStep<'_>, attempts: &mut Attempts, error: Error) -> Result<()> {
    let throttled: &RouteKey = &step.route.key;
    attempts
        .candidates
        .retain(|candidate| candidate.route.key != *throttled);
    if attempts.candidates.is_empty() {
        return Err(error);
    }
    tracing::info!(
        profile_id = %step.id,
        platform_id = %step.policy.platform_id,
        attempt = step.attempt + 1,
        "Credential profile's route throttled; trying an account on another route"
    );
    attempts.previous = Some((step.id.to_owned(), "throttled"));
    Ok(())
}

/// The attempt budget, and whether a failed account may hand over to the next.
fn attempt_budget(policy: &ResolvedCredentialPolicy, allow_switch: bool) -> (u8, bool) {
    match policy.selection {
        CredentialSelection::Pool {
            max_attempts,
            failover,
            ..
        } => (max_attempts, failover && allow_switch),
        _ => (2, false),
    }
}

/// Recovery without the bound account starts at the next saved account after
/// it that is still eligible, so the saved order is kept.
fn start_after_bound(policy: &ResolvedCredentialPolicy, bound: &str, eligible: &mut [String]) {
    let saved = policy.selection.profile_ids();
    if let Some(bound_position) = saved.iter().position(|candidate| *candidate == bound)
        && let Some(next) = (1..=saved.len())
            .map(|offset| saved[(bound_position + offset) % saved.len()])
            .find(|candidate| eligible.iter().any(|id| id == candidate))
        && let Some(index) = eligible.iter().position(|id| id == next)
    {
        eligible.rotate_left(index);
    }
}

/// A manual validation or refresh reports provider trouble as retryable
/// unavailability, not as an internal error.
fn management_error(error: Error) -> Error {
    match error {
        Error::Monitor(_) => ProfileError::ProviderUnavailable("provider_failed").into(),
        error => error,
    }
}

fn provider_error(error: CredentialError) -> Error {
    match error {
        CredentialError::RateLimited { retry_after } => {
            Error::Extractor(ExtractorError::RateLimited {
                code: None,
                retry_after,
            })
        }
        CredentialError::DeadlineExceeded => {
            Error::Monitor("Credential operation deadline exceeded".into())
        }
        _ => Error::Monitor("Credential provider request failed".into()),
    }
}

#[cfg(test)]
mod tests;
