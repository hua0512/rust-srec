//! Per-operation cursor, candidate, exclusion, and renewal timing state.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use platforms_parser::extractor::error::ExtractorError;

use crate::credentials::{
    CredentialBinding, CredentialError, CredentialOwner, CredentialProfile,
    CredentialProfileHealth, CredentialProvider, CredentialSelection, CredentialValidity,
    OperationDeadline, PoolStrategy, ProfileError, ResolvedCredentialPolicy, UnavailableReason,
};
use crate::proxies::{ResolvedRoute, RouteKey};
use crate::{Error, Result};

pub(super) struct Cursor {
    pub(super) next: usize,
    pub(super) used: Instant,
    pub(super) owner: CredentialOwner,
    pub(super) platform_id: String,
}

/// An account still to try in one operation.
pub(super) struct Candidate {
    pub(super) id: String,
    pub(super) preloaded: Option<(CredentialProfile, Option<CredentialProfileHealth>)>,
    /// Repair the account before extracting with it, if it is still at this
    /// revision, the one the platform rejected.
    pub(super) repair: Option<i64>,
    /// The route the account's requests take.
    pub(super) route: ResolvedRoute,
}

/// Per-operation state carried across attempts.
pub(super) struct Attempts {
    pub(super) candidates: VecDeque<Candidate>,
    pub(super) exclusions: Exclusions,
    /// Accounts whose status was checked by this operation.
    pub(super) checked: HashSet<String>,
    /// Accounts this operation already repaired; each is repaired at most once.
    pub(super) repaired: HashSet<String>,
    /// Accounts already retried after changing mid-attempt.
    pub(super) source_retries: HashSet<String>,
    /// The last failed account and why, logged when failover moves on.
    pub(super) previous: Option<(String, &'static str)>,
    pub(super) platform: String,
    /// The operation's route, which accounts without their own follow.
    pub(super) operation: ResolvedRoute,
}

impl Attempts {
    /// Requeues an account that changed mid-attempt, once per operation.
    pub(super) fn retry_source(
        &mut self,
        id: &str,
        preloaded: Option<(CredentialProfile, Option<CredentialProfileHealth>)>,
        route: ResolvedRoute,
    ) -> bool {
        let first = self.source_retries.insert(id.to_owned());
        if first {
            self.candidates.push_front(Candidate {
                id: id.to_owned(),
                preloaded,
                repair: None,
                route,
            });
        }
        first
    }
}

/// The inputs of one attempt that stay fixed while it runs.
pub(super) struct AttemptStep<'a> {
    pub(super) policy: &'a ResolvedCredentialPolicy,
    pub(super) binding: Option<&'a CredentialBinding>,
    pub(super) deadline: OperationDeadline,
    pub(super) attempt: u8,
    pub(super) id: &'a str,
    pub(super) preloaded: Option<&'a (CredentialProfile, Option<CredentialProfileHealth>)>,
    pub(super) repair: Option<i64>,
    pub(super) route: &'a ResolvedRoute,
}

/// Why candidates were passed over, so exhaustion can say what would help.
#[derive(Debug, Default)]
pub struct Exclusions {
    pub(super) disabled: bool,
    pub(super) login_required: bool,
    /// An account is resting after the platform turned it away; it needs
    /// no new login, so it reports as the attempts running out.
    pub(super) resting: bool,
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
        } else if self.resting {
            Some(UnavailableReason::AttemptsExhausted)
        } else {
            None
        }
    }
}

/// A rejected account's rest, at the revision that was rejected.
pub(super) struct Rest {
    pub(super) revision: i64,
    pub(super) until: Instant,
}

/// How long a failed renewal waits before the next one.
pub(super) const RENEWAL_RETRY: Duration = Duration::from_secs(60 * 60);

/// How long a recorded use stands before the next use is written.
pub(crate) const USE_RECORD_WINDOW: Duration = Duration::from_secs(5 * 60);

/// When the account is next renewed before use, in epoch milliseconds, or
/// `None` when its provider never renews it.
pub fn next_renewal_at(
    provider: &dyn CredentialProvider,
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

/// Whether the account's last refresh failed less than [`RENEWAL_RETRY`] ago
/// and has not succeeded since.
pub(super) fn refresh_failed_recently(health: Option<&CredentialProfileHealth>) -> bool {
    health.is_some_and(|health| {
        health.validity == CredentialValidity::NeedsRefresh
            && health.last_failure_at.is_some_and(|failed| {
                crate::database::time::now_ms()
                    < failed.saturating_add(RENEWAL_RETRY.as_millis() as i64)
            })
    })
}

/// After a throttle on the step's route, the operation continues only with
/// accounts on other routes: the same route would meet the same throttle.
pub(super) fn fail_over_throttle(
    step: &AttemptStep<'_>,
    attempts: &mut Attempts,
    error: Error,
) -> Result<()> {
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
pub(super) fn attempt_budget(policy: &ResolvedCredentialPolicy, allow_switch: bool) -> (u8, bool) {
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
pub(super) fn start_after_bound(
    policy: &ResolvedCredentialPolicy,
    bound: &str,
    eligible: &mut [String],
) {
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

/// Check whether `policy` uses round-robin pool selection.
pub(super) fn is_round_robin(policy: &ResolvedCredentialPolicy) -> bool {
    matches!(
        policy.selection,
        CredentialSelection::Pool {
            strategy: PoolStrategy::RoundRobin,
            ..
        }
    )
}

/// A manual validation or refresh reports provider trouble as retryable
/// unavailability, not as an internal error.
pub(super) fn management_error(error: Error) -> Error {
    match error {
        Error::Monitor(_) => ProfileError::ProviderUnavailable("provider_failed").into(),
        error => error,
    }
}

pub(super) fn provider_error(error: CredentialError) -> Error {
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
