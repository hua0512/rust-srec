//! Accounts that need the user before they work again.

use serde::Serialize;

use super::{CredentialProfileHealth, CredentialProfileSummary, CredentialValidity};

/// Consecutive failed refreshes after which a rejected account is reported.
/// The repository restarts the count when a failure follows the previous one
/// by more than six hours. The third failure is also when the refresh-failure
/// notification first repeats, so one transient failure lists nothing.
pub const ATTENTION_REFRESH_FAILURES: i64 = 3;

/// What the user has to do about an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    /// Only a new login or new cookies make the account usable again.
    LoginRequired,
    /// The platform rejected the account and its automatic refresh keeps
    /// failing; a new login is the likely fix.
    RefreshFailing,
}

impl AttentionReason {
    /// The reason an account's health calls for, if it calls for one.
    pub fn for_health(health: &CredentialProfileHealth) -> Option<Self> {
        match health.validity {
            CredentialValidity::Invalid => Some(Self::LoginRequired),
            CredentialValidity::NeedsRefresh
                if health.refresh_failure_count >= ATTENTION_REFRESH_FAILURES =>
            {
                Some(Self::RefreshFailing)
            }
            _ => None,
        }
    }
}

/// An enabled account that needs the user, with its platform's name.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CredentialAttention {
    pub profile: CredentialProfileSummary,
    pub platform_name: String,
    pub reason: AttentionReason,
    /// Health at the account's current revision.
    pub health: CredentialProfileHealth,
}
