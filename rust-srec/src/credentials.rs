//! Credential management module.
//!
//! This module provides platform-agnostic credential management,
//! including automatic cookie refresh for platforms like Bilibili.
//!
//! # Architecture
//!
//! - [`CredentialScope`]: Identifies which config layer provides credentials
//! - [`CredentialSource`]: Complete credential info with source tracking
//! - [`CredentialManager`]: Platform-specific refresh trait
//! - [`CredentialRefreshService`]: Orchestrates the refresh flow
//!
//! Credential sources are resolved alongside recording configuration by
//! [`crate::config::ConfigService`].

mod admission;
pub mod conversion;
mod error;
mod execution;
pub mod login_sessions;
mod manager;
mod profile;
mod resolution;
mod selection;
mod service;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
mod tracker;
mod types;

// Platform-specific implementations
pub mod platforms;

pub use admission::{OperationDeadline, PlatformAdmission};
pub use error::CredentialError;
pub use execution::{
    CredentialExecution, CredentialExecutionService, CredentialSnapshot, Exclusions, Extracted,
    ProfileActionResult,
};
pub use manager::{CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials};
pub use profile::{
    CredentialMaterial, CredentialProfile, CredentialProfileHealth, CredentialProfileSummary,
    CredentialUnavailable, ProfileError,
};
pub(crate) use resolution::legacy_account_extras;
pub(crate) use resolution::resolve_authentication;
pub use resolution::{
    isolate_authentication_extras, isolate_platform_authentication_extras,
    managed_authentication_extras, merge_cookie_updates,
};
pub use selection::{
    CredentialBinding, CredentialIdentity, CredentialOwner, CredentialSelection, PoolStrategy,
    ResolvedCredentialPolicy,
};
pub(crate) use selection::{
    canonical_document_text, canonical_overrides_text, canonical_selection_text,
};
pub use service::CredentialRefreshService;
pub use store::CredentialStore;
pub use tracker::{DailyCheckTracker, RefreshFailureTracker};
pub use types::{CredentialEvent, CredentialScope, CredentialSource};
pub(crate) use types::{extractor_platform_extras, platform_reauth_extra};
