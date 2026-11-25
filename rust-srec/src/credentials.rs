//! Credential management module.
//!
//! Pure credential types, selection policies, site normalization, and
//! authentication-field isolation live in [`crate::domain::credential`].
//! This module provides platform providers, execution/repair services,
//! admission control, QR login sessions, and notification events.

mod admission;
mod blocks;
mod connection;
mod error;
mod execution;
pub mod login_sessions;
mod provider;
mod resolution;
mod service;
#[cfg(test)]
pub(crate) mod test_support;
mod types;

// Platform-specific implementations
pub mod platforms;

pub use crate::domain::credential::{
    ATTENTION_REFRESH_FAILURES, AUTHENTICATION_FIELDS, AttentionReason, CredentialAttention,
    CredentialBinding, CredentialIdentity, CredentialMaterial, CredentialOwner, CredentialProfile,
    CredentialProfileHealth, CredentialProfileSummary, CredentialScope, CredentialSelection,
    CredentialUnavailable, CredentialValidity, HealthReason, MAX_SITES, PoolStrategy, ProfileError,
    ProfileReferences, RecordingReference, RefreshedCredentials, ResolvedCredentialPolicy,
    SelectionReference, StreamerSite, UnavailableReason, covering_sites,
    isolate_platform_authentication_extras, login_fields, merge_cookie_updates, normalize_site,
    normalize_sites, site_host,
};
pub(crate) use crate::domain::credential::{
    NESTED_EXTRAS, platform_reauth_extra, remove_authentication_fields, resolve_authentication,
};
pub use admission::{OperationDeadline, PlatformAdmission};
pub use blocks::{CredentialBlock, CredentialBlockChange, CredentialBlocks};
pub(crate) use connection::{provider_client, provider_client_for};
pub use error::CredentialError;
pub use execution::{
    CredentialExecution, CredentialExecutionService, CredentialSnapshot, Exclusions, Extracted,
    ProfileActionResult, next_renewal_at,
};
pub use provider::{
    AccountStatus, CookieProvider, CredentialProvider, ProviderCapabilities, QrLoginPoll,
    QrLoginStart, provider,
};
#[doc(hidden)]
pub use provider::{DuplicateProvider, register_provider_for_process};
pub use resolution::managed_authentication_extras;
pub use service::CredentialProviderRegistry;
pub use types::CredentialEvent;
