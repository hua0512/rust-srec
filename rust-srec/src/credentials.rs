//! Credential management module.
//!
//! This module provides platform-agnostic credential management,
//! including automatic cookie refresh for platforms like Bilibili.
//!
//! # Architecture
//!
//! - [`CredentialProfile`]: One account's material, owned by a platform
//! - [`CredentialSelection`]: Which profiles a platform, template or streamer uses
//! - [`CredentialProvider`]: Platform-specific account rules and provider calls
//! - [`CredentialExecutionService`]: Selects, repairs and fails over accounts
//!
//! Selection policies are resolved alongside recording configuration by
//! [`crate::config::ConfigService`].

mod admission;
mod attention;
mod blocks;
mod connection;
mod error;
mod execution;
pub mod login_sessions;
mod profile;
mod provider;
mod resolution;
mod selection;
mod service;
#[cfg(test)]
pub(crate) mod test_support;
mod types;

// Platform-specific implementations
pub mod platforms;

pub use admission::{OperationDeadline, PlatformAdmission};
pub use attention::{ATTENTION_REFRESH_FAILURES, AttentionReason, CredentialAttention};
pub use blocks::{CredentialBlock, CredentialBlockChange, CredentialBlocks};
pub(crate) use connection::provider_client;
pub use error::CredentialError;
pub use execution::{
    CredentialExecution, CredentialExecutionService, CredentialSnapshot, Exclusions, Extracted,
    ProfileActionResult, next_renewal_at,
};
pub use profile::{
    CredentialMaterial, CredentialProfile, CredentialProfileHealth, CredentialProfileSummary,
    CredentialUnavailable, CredentialValidity, HealthReason, ProfileError, ProfileReferences,
    RecordingReference, SelectionReference, UnavailableReason,
};
pub(crate) use provider::platform_reauth_extra;
pub use provider::{
    AccountStatus, CookieProvider, CredentialProvider, ProviderCapabilities, QrLoginPoll,
    QrLoginStart, RefreshedCredentials, login_fields, provider,
};
pub use resolution::{
    AUTHENTICATION_FIELDS, isolate_platform_authentication_extras, managed_authentication_extras,
    merge_cookie_updates,
};
pub(crate) use resolution::{
    NESTED_EXTRAS, carries_authentication_fields, remove_authentication_fields,
    resolve_authentication,
};
pub use selection::{
    CredentialBinding, CredentialIdentity, CredentialOwner, CredentialSelection, PoolStrategy,
    ResolvedCredentialPolicy,
};
pub use service::CredentialProviderRegistry;
pub use types::{CredentialEvent, CredentialScope};
