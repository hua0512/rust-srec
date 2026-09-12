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

mod error;
mod manager;
mod service;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
mod tracker;
mod types;

// Platform-specific implementations
pub mod platforms;

pub use error::CredentialError;
pub use manager::{CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials};
pub use service::CredentialRefreshService;
pub use store::CredentialStore;
pub use tracker::{DailyCheckTracker, RefreshFailureTracker};
pub use types::{CredentialEvent, CredentialScope, CredentialSource};
pub(crate) use types::{extractor_platform_extras, platform_reauth_extra};
