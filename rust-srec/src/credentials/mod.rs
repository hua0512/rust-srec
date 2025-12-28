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
//! - [`CredentialResolver`]: Finds credential source for streamers
//! - [`CredentialRefreshService`]: Orchestrates the refresh flow

mod error;
mod manager;
mod resolver;
mod service;
mod store;
mod tracker;
mod types;

// Platform-specific implementations
pub mod platforms;

pub use error::CredentialError;
pub use manager::{CredentialManager, CredentialStatus, RefreshState, RefreshedCredentials};
pub use resolver::CredentialResolver;
pub use service::CredentialRefreshService;
pub use store::CredentialStore;
pub use tracker::{DailyCheckTracker, RefreshFailureTracker};
pub use types::{CredentialScope, CredentialSource};
