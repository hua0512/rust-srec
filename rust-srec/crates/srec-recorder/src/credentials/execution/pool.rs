//! Credential pool candidate selection, round-robin cursors, and execution loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::time::{Duration, Instant};

use platforms_parser::extractor::error::ExtractorError;
use srec_core::CoreError;

use super::state::{
    AttemptStep, Attempts, Candidate, Cursor, Exclusions, attempt_budget, fail_over_throttle,
    is_round_robin, provider_error, refresh_failed_recently, start_after_bound,
};
use super::{CredentialExecution, CredentialExecutionService, CredentialSnapshot, Extracted};
use crate::credentials::{
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialProfile,
    CredentialProfileHealth, CredentialSelection, OperationDeadline, ProfileError,
    ResolvedCredentialPolicy, UnavailableReason,
};
use crate::proxies::ResolvedRoute;
use crate::{Error, Result};

impl CredentialExecutionService {
    /// A bound poll cannot switch accounts. Startup/recovery may pass allow_switch
    /// only when the caller owns the session and will commit the replacement epoch.
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
        let platform = self.repository.validate_selection_platform(policy).await?;
        let bound_anonymous = binding.is_some_and(|binding| {
            binding.policy.generation == policy.generation
                && matches!(binding.identity, CredentialIdentity::Anonymous)
        });
        if matches!(
            policy.selection,
            CredentialSelection::Inherit | CredentialSelection::None
        ) {
            return self
                .execute_anonymous(policy, binding, deadline, route, &mut extract)
                .await;
        }
        let accounts_optional = self.providers.provider(&platform).accounts_optional();
        let falls_back = policy.selection.anonymous_fallback() || accounts_optional;
        if falls_back && bound_anonymous && !allow_switch {
            return self
                .execute_anonymous(policy, binding, deadline, route, &mut extract)
                .await;
        }
        let anonymous_fallback =
            falls_back && (binding.is_none() || allow_switch || accounts_optional);
        match self
            .execute_pool(
                policy,
                binding,
                allow_switch,
                deadline,
                route,
                &mut extract,
                platform,
                anonymous_fallback,
            )
            .await
        {
            Err(Error::Core(CoreError::CredentialUnavailable(unavailable)))
                if anonymous_fallback =>
            {
                tracing::debug!(
                    platform_id = %policy.platform_id,
                    strategy = policy.selection.strategy_name(),
                    reason = unavailable.reason.as_str(),
                    "No account in the pool can be used; checking without one"
                );
                match self
                    .execute_anonymous(policy, binding, deadline, route, &mut extract)
                    .await
                {
                    Err(Error::Extractor(ExtractorError::Authentication { .. })) => {
                        Err(self.unavailable(policy, unavailable.reason).await)
                    }
                    result => result,
                }
            }
            result => result,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn execute_pool<T, F, Fut>(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        deadline: OperationDeadline,
        route: &ResolvedRoute,
        extract: &mut F,
        platform: String,
        anonymous_fallback: bool,
    ) -> Result<CredentialExecution<T>>
    where
        F: FnMut(CredentialSnapshot) -> Fut,
        Fut: Future<Output = Result<Extracted<T>>>,
    {
        let mut exclusions = Exclusions::default();
        let candidates = self
            .select_candidates(
                policy,
                binding,
                allow_switch,
                route,
                &mut exclusions,
                anonymous_fallback,
            )
            .await?;
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
            let Some(Candidate {
                id,
                preloaded,
                repair,
                route,
            }) = attempts.candidates.pop_front()
            else {
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
                preloaded: preloaded.as_ref(),
                repair,
                route: &route,
            };
            if let Some(execution) = self.attempt(&step, &mut attempts, extract).await? {
                return Ok(execution);
            }
        }
        Err(if attempts.candidates.is_empty() {
            self.unavailable_after(
                policy,
                &attempts.exclusions,
                UnavailableReason::AttemptsExhausted,
                anonymous_fallback,
            )
            .await
        } else {
            self.exhausted(
                policy,
                UnavailableReason::AttemptsExhausted,
                anonymous_fallback,
            )
            .await
        })
    }

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

    async fn select_candidates(
        &self,
        policy: &ResolvedCredentialPolicy,
        binding: Option<&CredentialBinding>,
        allow_switch: bool,
        route: &ResolvedRoute,
        exclusions: &mut Exclusions,
        anonymous_fallback: bool,
    ) -> Result<VecDeque<Candidate>> {
        let mut loaded = self.eligible_profiles(policy, route, exclusions).await?;
        let mut eligible: Vec<String> = loaded.iter().map(|(p, _, _)| p.id.clone()).collect();
        if eligible.is_empty() {
            return Err(self
                .unavailable_after(
                    policy,
                    exclusions,
                    UnavailableReason::AttemptsExhausted,
                    anonymous_fallback,
                )
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
                    .exhausted(
                        policy,
                        UnavailableReason::BoundProfileUnavailable,
                        anonymous_fallback,
                    )
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
                .exhausted(
                    policy,
                    UnavailableReason::BindingPolicyChanged,
                    anonymous_fallback,
                )
                .await);
        } else if is_round_robin(policy) {
            let Some(start) = self.round_robin_start(policy, &eligible) else {
                return Err(self
                    .unavailable_after(
                        policy,
                        exclusions,
                        UnavailableReason::AttemptsExhausted,
                        anonymous_fallback,
                    )
                    .await);
            };
            if let Some(index) = eligible.iter().position(|id| *id == start) {
                eligible.rotate_left(index);
            }
        }

        let mut by_id: HashMap<
            String,
            (
                CredentialProfile,
                Option<CredentialProfileHealth>,
                ResolvedRoute,
            ),
        > = loaded
            .drain(..)
            .map(|(profile, health, resolved_route)| {
                (profile.id.clone(), (profile, health, resolved_route))
            })
            .collect();

        if attempt_budget(policy, allow_switch).1 {
            let admission = self.providers.admission();
            eligible.sort_by_key(|id| {
                let key = by_id.get(id).map_or(&route.key, |(_, _, own)| &own.key);
                admission.backing_off(&policy.platform_id, key)
            });
        } else {
            eligible.truncate(1);
        }
        Ok(eligible
            .into_iter()
            .filter_map(|id| {
                let (profile, health, resolved_route) = by_id.remove(&id)?;
                Some(Candidate {
                    id,
                    preloaded: Some((profile, health)),
                    repair: None,
                    route: resolved_route,
                })
            })
            .collect())
    }

    async fn eligible_profiles(
        &self,
        policy: &ResolvedCredentialPolicy,
        route: &ResolvedRoute,
        exclusions: &mut Exclusions,
    ) -> Result<
        Vec<(
            CredentialProfile,
            Option<CredentialProfileHealth>,
            ResolvedRoute,
        )>,
    > {
        let ids = policy.selection.profile_ids();
        let candidates = self.repository.load_pool_candidates(&ids, route).await?;
        let mut eligible = Vec::with_capacity(candidates.len());
        for (profile, health, own) in candidates {
            if exclusions.admits(profile.enabled, health.as_ref())
                && !self.excluded_while_resting(&profile, exclusions)
            {
                eligible.push((profile, health, own));
            }
        }
        Ok(eligible)
    }

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
            repair = step.repair.is_some(),
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

    async fn prepare_attempt(
        &self,
        step: &AttemptStep<'_>,
        attempts: &mut Attempts,
    ) -> Result<Option<CredentialProfile>> {
        let id = step.id;
        let (profile, health) = match step.preloaded {
            Some((profile, health)) => (profile.clone(), health.clone()),
            None => self.repository.get_with_health(id).await?,
        };
        if !attempts.exclusions.admits(profile.enabled, health.as_ref()) {
            return Ok(None);
        }
        if self.excluded_while_resting(&profile, &mut attempts.exclusions) {
            return Ok(None);
        }
        let accounts_optional = self
            .providers
            .provider(&attempts.platform)
            .accounts_optional();
        if accounts_optional && refresh_failed_recently(health.as_ref()) {
            attempts.previous = Some((id.to_owned(), "refresh_failed"));
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
            Err(Error::Core(CoreError::CredentialProfile(ProfileError::SourceChanged)))
                if attempts.retry_source(id, None, step.route.clone()) =>
            {
                Ok(None)
            }
            Err(error @ Error::Extractor(ExtractorError::RateLimited { .. })) => {
                fail_over_throttle(step, attempts, error)?;
                Ok(None)
            }
            Err(error @ Error::Monitor(_))
                if accounts_optional
                    && refresh_failed_recently(self.repository.health(id).await?.as_ref()) =>
            {
                tracing::warn!(
                    profile_id = %id,
                    platform = %attempts.platform,
                    %error,
                    "Could not repair the credential profile; checking without it"
                );
                attempts.previous = Some((id.to_owned(), "refresh_failed"));
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

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
        let (latest, latest_health) = self.repository.get_with_health(id).await?;
        if !latest.enabled || latest.revision != profile.revision {
            attempts.exclusions.disabled |= !latest.enabled;
            let route = self
                .repository
                .account_route(&latest, Some(&attempts.operation))
                .await?;
            attempts.retry_source(id, Some((latest, latest_health)), route);
            return Ok(None);
        }
        if !attempts.exclusions.admits(true, latest_health.as_ref())
            || self.excluded_while_resting(&latest, &mut attempts.exclusions)
        {
            return Ok(None);
        }
        self.repository.validate_current(profile).await?;
        Ok(Some(snapshot))
    }

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
            if attempts.retry_source(step.id, Some((current, None)), route) {
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
        self.resting.lock().remove(step.id);
        self.unavailable.lock().remove(&step.policy.generation);
        Ok(Some(CredentialExecution {
            value: result.value,
            snapshot,
        }))
    }
}
