//! One bounded credential operation owns selection, repair and typed failover.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use platforms_parser::extractor::error::{ExtractorError, ThrottleScope};
use serde::Serialize;
use tokio::sync::Mutex as AsyncMutex;

use crate::database::repositories::CredentialProfileRepository;
use crate::{Error, Result};

use super::{
    CredentialBinding, CredentialError, CredentialIdentity, CredentialMaterial, CredentialProfile,
    CredentialProfileHealth, CredentialProfileSummary, CredentialRefreshService,
    CredentialSelection, CredentialStatus, CredentialUnavailable, OperationDeadline, PoolStrategy,
    ProfileError, RefreshState, RefreshedCredentials, ResolvedCredentialPolicy,
};

#[derive(Clone)]
pub struct CredentialSnapshot {
    pub binding: CredentialBinding,
    pub material: CredentialMaterial,
}

impl std::fmt::Debug for CredentialSnapshot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialSnapshot")
            .field("binding", &self.binding)
            .field("material", &"[redacted]")
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

struct Probe {
    id: String,
    probes: Arc<Mutex<HashSet<String>>>,
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.probes.lock().remove(&self.id);
    }
}

/// Why candidates were passed over, so exhaustion can say what would help.
#[derive(Debug, Default)]
pub struct Exclusions {
    disabled: bool,
    login_required: bool,
    /// Another operation holds the half-open probe after a cooldown.
    probing: bool,
    retry_at: Option<i64>,
}

impl Exclusions {
    /// Records why a stored profile cannot be selected now; true when it can.
    pub fn admits(
        &mut self,
        enabled: bool,
        health: Option<&CredentialProfileHealth>,
        now: i64,
    ) -> bool {
        if !enabled {
            self.disabled = true;
            return false;
        }
        if health.is_some_and(|health| health.validity == "invalid") {
            self.login_required = true;
            return false;
        }
        if let Some(until) = health
            .and_then(|health| health.cooldown_until)
            .filter(|until| *until > now)
        {
            self.cooling(until);
            return false;
        }
        true
    }

    fn cooling(&mut self, until: i64) {
        self.retry_at = Some(self.retry_at.map_or(until, |current| current.min(until)));
    }

    pub fn retry_at(&self) -> Option<i64> {
        self.retry_at
    }

    /// A cooldown ends without user action, so it is reported ahead of
    /// conditions that need one.
    pub fn reason(&self) -> Option<&'static str> {
        if self.retry_at.is_some() || self.probing {
            Some("cooling_down")
        } else if self.login_required {
            Some("login_required")
        } else if self.disabled {
            Some("profiles_disabled")
        } else {
            None
        }
    }
}

pub struct CredentialExecutionService {
    repository: Arc<CredentialProfileRepository>,
    refresh: Arc<CredentialRefreshService>,
    cursors: Mutex<HashMap<String, Cursor>>,
    locks: dashmap::DashMap<String, Weak<AsyncMutex<()>>>,
    probes: Arc<Mutex<HashSet<String>>>,
    unavailable: Mutex<HashMap<String, (CredentialUnavailable, Instant)>>,
}

impl CredentialExecutionService {
    async fn mark_invalid(&self, profile: &CredentialProfile, platform: &str) -> Result<()> {
        if self.repository.mark_invalid(profile).await? {
            tracing::warn!(profile_id = %profile.id, platform, "Credential profile needs a new login");
            self.refresh
                .maybe_notify_credential_event(super::CredentialEvent::Invalid {
                    profile_id: Some(profile.id.clone()),
                    profile_label: Some(profile.label.clone()),
                    scope: self.profile_scope(profile).await?,
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
        refresh: Arc<CredentialRefreshService>,
    ) -> Self {
        Self {
            repository,
            refresh,
            cursors: Mutex::new(HashMap::new()),
            locks: dashmap::DashMap::new(),
            probes: Arc::new(Mutex::new(HashSet::new())),
            unavailable: Mutex::new(HashMap::new()),
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
        reason: &str,
        retry_at: Option<i64>,
    ) -> Error {
        let status = CredentialUnavailable {
            reason: reason.to_owned(),
            policy_generation: policy.generation.clone(),
            retry_at,
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
                reason,
                retry_at,
                "Credential selection exhausted"
            );
            let platform = match self.repository.platform_name(&policy.platform_id).await {
                Ok(name) => name,
                Err(error) => {
                    tracing::warn!(%error, platform_id = %policy.platform_id, "Could not load the platform name for a credential notification");
                    policy.platform_id.clone()
                }
            };
            self.refresh
                .maybe_notify_credential_event(super::CredentialEvent::Unavailable {
                    scope: self.display_scope(&policy.owner).await,
                    platform,
                    reason_code: reason.to_owned(),
                    retry_at,
                    timestamp: Utc::now(),
                });
        } else {
            tracing::debug!(
                platform_id = %policy.platform_id,
                strategy = policy.selection.strategy_name(),
                reason,
                retry_at,
                "Credential selection still exhausted"
            );
        }
        Error::CredentialUnavailable(status)
    }

    async fn unavailable_after(
        &self,
        policy: &ResolvedCredentialPolicy,
        exclusions: &Exclusions,
        fallback: &str,
    ) -> Error {
        self.unavailable(
            policy,
            exclusions.reason().unwrap_or(fallback),
            exclusions.retry_at,
        )
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

    async fn profile_scope(&self, profile: &CredentialProfile) -> Result<super::CredentialScope> {
        Ok(self.display_scope(&profile.owner()?).await)
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
            let Some(manager) = self.refresh.manager(&platform) else {
                return self.action(profile, false).await;
            };
            self.refresh
                .admission()
                .admit(&profile.platform_config_id, deadline)
                .await
                .map_err(provider_error)?;
            let status = manager
                .check_status(&profile.cookies)
                .await
                .map_err(|error| {
                    let error = provider_error(error);
                    self.defer_shared_throttle(&profile.platform_config_id, &error);
                    error
                })?;
            let validity = match status {
                CredentialStatus::Valid => "valid",
                CredentialStatus::NeedsRefresh { .. } => "needs_refresh",
                CredentialStatus::Invalid { .. } => "invalid",
            };
            self.repository
                .publish_health(&profile, validity, None, Some("manual_validation"), true)
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
            if self
                .refresh
                .manager(&platform)
                .is_none_or(|manager| !manager.supports_auto_refresh())
            {
                return self.action(profile, false).await;
            }
            let profile = self.repair(&profile, &platform, deadline).await?;
            self.action(profile, true).await
        })
        .await
        .map_err(|_| Error::from(ProfileError::ProviderUnavailable("deadline_exceeded")))?
        .map_err(management_error)
    }

    async fn action(
        &self,
        profile: CredentialProfile,
        supported: bool,
    ) -> Result<ProfileActionResult> {
        Ok(ProfileActionResult {
            supported,
            health: self.repository.health(&profile.id).await?,
            profile: profile.summary()?,
        })
    }

    async fn repair(
        &self,
        profile: &CredentialProfile,
        platform: &str,
        deadline: OperationDeadline,
    ) -> Result<CredentialProfile> {
        let Some(manager) = self
            .refresh
            .manager(platform)
            .filter(|manager| manager.supports_auto_refresh())
        else {
            return Err(ProfileError::InvalidMaterial(
                "profile cannot be refreshed by this provider",
            )
            .into());
        };
        self.refresh
            .admission()
            .admit(&profile.platform_config_id, deadline)
            .await
            .map_err(provider_error)?;
        let mut extra = profile
            .material()?
            .reauth_config
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(token) = &profile.access_token
            && let Some(fields) = extra.as_object_mut()
        {
            fields.insert(
                "access_token".into(),
                serde_json::Value::String(token.clone()),
            );
        }
        let state = RefreshState {
            cookies: profile.cookies.clone(),
            refresh_token: profile.refresh_token.clone(),
            extra: Some(extra),
        };
        match manager.refresh(&state).await {
            Ok(replacement) => {
                let updated = self.repository.refreshed(profile, &replacement).await?;
                if updated.revision != profile.revision {
                    self.refresh
                        .maybe_notify_credential_event(super::CredentialEvent::Refreshed {
                            profile_id: Some(updated.id.clone()),
                            profile_label: Some(updated.label.clone()),
                            scope: self.profile_scope(&updated).await?,
                            platform: platform.to_owned(),
                            expires_at: replacement.expires_at,
                            timestamp: Utc::now(),
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
                            "invalid"
                        } else {
                            "needs_refresh"
                        },
                        None,
                        Some(if requires_login {
                            "login_required"
                        } else {
                            "refresh_failed"
                        }),
                        false,
                    )
                    .await?;
                let (count, notify) = self.repository.record_refresh_failure(profile).await?;
                if notify {
                    tracing::warn!(profile_id = %profile.id, platform, failure_count = count, requires_login, "Credential profile refresh failed");
                    self.refresh.maybe_notify_credential_event(
                        super::CredentialEvent::RefreshFailed {
                            profile_id: Some(profile.id.clone()),
                            profile_label: Some(profile.label.clone()),
                            scope: self.profile_scope(profile).await?,
                            platform: platform.to_owned(),
                            error: if requires_login {
                                "login_required"
                            } else {
                                "refresh_failed"
                            }
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
                    self.defer_shared_throttle(&profile.platform_config_id, &error);
                    Err(error)
                }
            }
        }
    }

    async fn prepare(
        &self,
        id: &str,
        platform: &str,
        deadline: OperationDeadline,
        force_repair: bool,
        checked: &mut HashSet<String>,
        repaired: &mut HashSet<String>,
    ) -> Result<CredentialProfile> {
        let lock = self.refresh_lock(id);
        let _guard = lock.lock().await;
        let profile = self.repository.get(id).await?;
        if !profile.enabled {
            return Err(ProfileError::SourceChanged.into());
        }
        let health = self.repository.health(id).await?;
        if let Some(until) = health
            .as_ref()
            .and_then(|health| health.cooldown_until)
            .filter(|until| *until > crate::database::time::now_ms())
        {
            return Err(Error::CredentialUnavailable(CredentialUnavailable {
                reason: "profile_cooling_down".into(),
                policy_generation: String::new(),
                retry_at: Some(until),
            }));
        }
        if !force_repair
            && health
                .as_ref()
                .is_some_and(|health| health.validity == "invalid")
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
            return self.repair(&profile, platform, deadline).await;
        }
        let Some(manager) = self.refresh.manager(platform) else {
            return Ok(profile);
        };
        let today = Utc::now().date_naive();
        let status = if let Some(health) = health.as_ref().filter(|health| {
            health.validity == "invalid"
                || health
                    .last_check_at
                    .and_then(chrono::DateTime::from_timestamp_millis)
                    .is_some_and(|time| time.date_naive() == today)
        }) {
            match health.validity.as_str() {
                "valid" => return Ok(profile),
                "invalid" => {
                    return Err(Error::Extractor(ExtractorError::Authentication {
                        code: "login_required".into(),
                    }));
                }
                _ => CredentialStatus::NeedsRefresh {
                    refresh_deadline: None,
                },
            }
        } else if checked.insert(id.to_owned()) {
            self.refresh
                .admission()
                .admit(&profile.platform_config_id, deadline)
                .await
                .map_err(provider_error)?;
            manager
                .check_status(&profile.cookies)
                .await
                .map_err(provider_error)?
        } else {
            return Ok(profile);
        };
        match status {
            CredentialStatus::Valid => {
                self.repository
                    .publish_health(&profile, "valid", None, None, true)
                    .await?;
                Ok(profile)
            }
            CredentialStatus::NeedsRefresh { .. } | CredentialStatus::Invalid { .. } => {
                self.repository
                    .publish_health(
                        &profile,
                        "needs_refresh",
                        None,
                        Some("repair_required"),
                        true,
                    )
                    .await?;
                if manager.supports_auto_refresh() && repaired.insert(id.to_owned()) {
                    self.repair(&profile, platform, deadline).await
                } else {
                    self.mark_invalid(&profile, platform).await?;
                    Err(Error::Extractor(ExtractorError::Authentication {
                        code: "login_required".into(),
                    }))
                }
            }
        }
    }

    /// A bound poll cannot switch accounts. Startup/recovery may pass allow_switch
    /// only when the caller owns the session and will commit the replacement epoch.
    pub async fn execute<T, F, Fut>(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        deadline: OperationDeadline,
        mut extract: F,
    ) -> Result<CredentialExecution<T>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        tokio::time::timeout_at(deadline.instant(), async {
            self.repository.validate_selection(policy).await?;
            if matches!(policy.selection, CredentialSelection::Inherit | CredentialSelection::None) {
                self.refresh.admission().admit(&policy.platform_id, deadline).await.map_err(provider_error)?;
                let mut snapshot = CredentialSnapshot { binding: CredentialBinding { identity: CredentialIdentity::Anonymous, revision: 0, policy: policy.clone(), epoch: binding.map_or(0, |binding| binding.epoch) }, material: CredentialMaterial { cookies: String::new(), refresh_token: None, access_token: None, reauth_config: None } };
                let result = extract(snapshot.clone()).await.inspect_err(|error| self.defer_shared_throttle(&policy.platform_id, error))?;
                if let Some(cookies) = result.session_cookies { snapshot.material.cookies = cookies; }
                self.unavailable.lock().remove(&policy.generation);
                return Ok(CredentialExecution { value: result.value, snapshot });
            }
            let mut eligible = Vec::new(); let mut exclusions = Exclusions::default();
            let now = crate::database::time::now_ms();
            for id in policy.selection.profile_ids() {
                let profile = self.repository.get(id).await?;
                let health = self.repository.health(id).await?;
                if !exclusions.admits(profile.enabled, health.as_ref(), now) { continue; }
                if self.probes.lock().contains(id) { exclusions.probing = true; continue; }
                eligible.push(id.to_owned());
            }
            if eligible.is_empty() { return Err(self.unavailable_after(policy, &exclusions, "attempts_exhausted").await); }
            let bound_id = binding.filter(|binding| binding.policy.generation == policy.generation).and_then(|binding| match &binding.identity { CredentialIdentity::Profile { profile_id } => Some(profile_id), _ => None });
            if let Some(id) = bound_id {
                if let Some(index) = eligible.iter().position(|candidate| candidate == id) { eligible.rotate_left(index); }
                else if !allow_switch || matches!(policy.selection, CredentialSelection::Pool { failover: false, .. }) { return Err(self.unavailable(policy, "bound_profile_unavailable", exclusions.retry_at).await); }
                else {
                    let saved = policy.selection.profile_ids();
                    if let Some(bound_position) = saved.iter().position(|candidate| *candidate == id)
                        && let Some(next) = (1..=saved.len()).map(|offset| saved[(bound_position + offset) % saved.len()]).find(|candidate| eligible.iter().any(|id| id == candidate))
                        && let Some(index) = eligible.iter().position(|id| id == next) { eligible.rotate_left(index); }
                    tracing::info!(platform_id = %policy.platform_id, from_profile_id = %id, to_profile_id = %eligible[0], strategy = policy.selection.strategy_name(), "Bound credential profile is unavailable; recovering with the next saved account");
                }
            } else if binding.is_some() && !allow_switch {
                return Err(self.unavailable(policy, "binding_policy_changed", exclusions.retry_at).await);
            } else if matches!(policy.selection, CredentialSelection::Pool { strategy: PoolStrategy::RoundRobin, .. }) {
                let all_ids = policy.selection.profile_ids();
                let selected = {
                    let mut cursors = self.cursors.lock();
                    cursors.retain(|generation, cursor| cursor.used.elapsed() < Duration::from_secs(3600) && (generation == &policy.generation || cursor.owner != policy.owner || cursor.platform_id != policy.platform_id));
                    if cursors.len() >= 4096 && !cursors.contains_key(&policy.generation)
                        && let Some(oldest) = cursors.iter().min_by_key(|(_, cursor)| cursor.used).map(|(key, _)| key.clone()) { cursors.remove(&oldest); }
                    let cursor = cursors.entry(policy.generation.clone()).or_insert(Cursor { next: 0, used: Instant::now(), owner: policy.owner.clone(), platform_id: policy.platform_id.clone() });
                    let selected = (0..all_ids.len()).map(|offset| (cursor.next + offset) % all_ids.len()).find(|index| eligible.iter().any(|id| id == all_ids[*index]));
                    if let Some(selected) = selected { cursor.next = (selected + 1) % all_ids.len(); cursor.used = Instant::now(); }
                    selected
                };
                let Some(selected) = selected else { return Err(self.unavailable_after(policy, &exclusions, "attempts_exhausted").await); };
                if let Some(index) = eligible.iter().position(|id| id == all_ids[selected]) { eligible.rotate_left(index); }
            }
            let (max_attempts, failover) = match policy.selection { CredentialSelection::Pool { max_attempts, failover, .. } => (max_attempts, failover && allow_switch), _ => (2, false) };
            if !failover { eligible.truncate(1); }
            let mut candidates: VecDeque<_> = eligible.into_iter().map(|id| (id, false)).collect();
            let mut checked = HashSet::new(); let mut repaired = HashSet::new(); let mut source_retries = HashSet::new();
            let platform = self.repository.platform_name(&policy.platform_id).await?;
            let mut previous: Option<(String, &str)> = None;
            for attempt in 0..max_attempts {
                let Some((id, repair)) = candidates.pop_front() else { break; };
                if let Some((failed, outcome)) = previous.take().filter(|(failed, _)| *failed != id) {
                    tracing::info!(platform_id = %policy.platform_id, strategy = policy.selection.strategy_name(), attempt = attempt + 1, from_profile_id = %failed, to_profile_id = %id, outcome, "Credential failover to the next eligible account");
                }
                let health = self.repository.health(&id).await?;
                if !exclusions.admits(true, health.as_ref(), crate::database::time::now_ms()) { continue; }
                let probe_guard = if health.as_ref().and_then(|health| health.cooldown_until).is_some() {
                    if !self.probes.lock().insert(id.clone()) { exclusions.probing = true; continue; }
                    Some(Probe { id: id.clone(), probes: self.probes.clone() })
                } else { None };
                let prepared = self.prepare(&id, &platform, deadline, repair, &mut checked, &mut repaired).await;
                let profile = match prepared {
                    Ok(profile) => profile,
                    Err(Error::Extractor(ExtractorError::Authentication { .. })) => {
                        exclusions.login_required = true; previous = Some((id, "login_required")); continue;
                    }
                    Err(Error::CredentialUnavailable(status)) => {
                        if let Some(until) = status.retry_at { exclusions.cooling(until); }
                        previous = Some((id, "cooling_down")); continue;
                    }
                    Err(Error::CredentialProfile(ProfileError::SourceChanged)) if source_retries.insert(id.clone()) => { candidates.push_front((id, false)); continue; }
                    Err(error) => {
                        self.defer_shared_throttle(&policy.platform_id, &error);
                        return Err(error);
                    },
                };
                let mut snapshot = CredentialSnapshot { binding: CredentialBinding { identity: CredentialIdentity::Profile { profile_id: id.clone() }, revision: profile.revision as u64, policy: policy.clone(), epoch: binding.map_or(0, |binding| binding.epoch) }, material: profile.material()? };
                self.refresh.admission().admit(&policy.platform_id, deadline).await.map_err(provider_error)?;
                let latest = self.repository.get(&id).await?;
                let latest_health = self.repository.health(&id).await?;
                if !latest.enabled || latest.revision != profile.revision {
                    exclusions.disabled |= !latest.enabled;
                    if source_retries.insert(id.clone()) { candidates.push_front((id, false)); }
                    continue;
                }
                if !exclusions.admits(true, latest_health.as_ref(), crate::database::time::now_ms()) { continue; }
                self.repository.validate_current(&profile).await?;
                tracing::debug!(profile_id = %id, attempt = attempt + 1, platform_id = %policy.platform_id, strategy = policy.selection.strategy_name(), repair, "Executing credential-bound extraction");
                match extract(snapshot.clone()).await {
                    Ok(result) => {
                        let current = self.repository.get(&id).await?;
                        if !current.enabled || current.revision != profile.revision {
                            if source_retries.insert(id.clone()) { candidates.push_front((id, false)); continue; }
                            return Err(ProfileError::SourceChanged.into());
                        }
                        if let Some(cookies) = result.session_cookies.filter(|cookies| cookies != &profile.cookies) {
                            let updated = self.repository.refreshed(&profile, &RefreshedCredentials { cookies, refresh_token: None, access_token: None, expires_at: None }).await?;
                            snapshot.material = updated.material()?; snapshot.binding.revision = updated.revision as u64;
                        } else if !result.preserve_health || probe_guard.is_some() {
                            // An offline answer still ends the half-open probe: the account
                            // was not throttled, so its expired cooldown and count are cleared.
                            let health = self.repository.health(&profile.id).await?;
                            let validity = health.as_ref().map_or("unknown", |health| health.validity.as_str());
                            let preserve_concurrent_conclusion = health.as_ref().is_some_and(|health| health.validity == "invalid" || health.cooldown_until.is_some_and(|until| until > crate::database::time::now_ms() || probe_guard.is_none()));
                            if !preserve_concurrent_conclusion { self.repository.publish_health(&profile, validity, None, None, false).await?; }
                        }
                        self.unavailable.lock().remove(&policy.generation);
                        return Ok(CredentialExecution { value: result.value, snapshot });
                    }
                    Err(Error::Extractor(ExtractorError::Authentication { .. })) => {
                        if !repaired.contains(&id) && self.refresh.manager(&platform).is_some_and(|manager| manager.supports_auto_refresh()) {
                            tracing::info!(profile_id = %id, platform_id = %policy.platform_id, attempt = attempt + 1, "Credential profile rejected; repairing it before the next attempt");
                            self.repository.publish_health(&profile, "needs_refresh", None, Some("authentication_failed"), false).await?;
                            candidates.push_front((id, true));
                        } else {
                            self.mark_invalid(&profile, &platform).await?;
                            exclusions.login_required = true; previous = Some((id, "authentication_failed"));
                        }
                    }
                    Err(Error::Extractor(ExtractorError::RateLimited { scope: ThrottleScope::Account, retry_after, .. })) => {
                        let validity = health.as_ref().map_or("unknown", |health| health.validity.as_str()).to_string();
                        let count = health.map_or(0, |health| health.throttle_count);
                        let delay = retry_after.unwrap_or_else(|| Duration::from_secs((60_u64.saturating_mul(1_u64 << count.clamp(0, 4))).min(900)));
                        let until = crate::database::time::now_ms().saturating_add(delay.as_millis().min(i64::MAX as u128) as i64);
                        self.repository.publish_health(&profile, &validity, Some(until), Some("account_throttled"), false).await?;
                        tracing::info!(profile_id = %id, platform_id = %policy.platform_id, cooldown_ms = delay.as_millis() as u64, "Credential profile throttled; cooling it down");
                        exclusions.cooling(until); previous = Some((id, "account_throttled"));
                    }
                    Err(error @ Error::Extractor(ExtractorError::RateLimited { .. })) => {
                        self.defer_shared_throttle(&policy.platform_id, &error);
                        return Err(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            // A remaining candidate means the attempt budget, not the accounts, ran out.
            Err(if candidates.is_empty() { self.unavailable_after(policy, &exclusions, "attempts_exhausted").await } else { self.unavailable(policy, "attempts_exhausted", exclusions.retry_at).await })
        }).await.map_err(|_| Error::Monitor("Credential operation deadline exceeded".into()))?
    }

    fn defer_shared_throttle(&self, platform_id: &str, error: &Error) {
        if let Error::Extractor(error) = error {
            self.refresh.admission().observe(platform_id, error);
        }
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
        CredentialError::Database(error) => Error::DatabaseSqlx(error),
        CredentialError::SourceChanged => ProfileError::SourceChanged.into(),
        CredentialError::RateLimited { retry_after } => {
            Error::Extractor(ExtractorError::RateLimited {
                scope: ThrottleScope::Unknown,
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
