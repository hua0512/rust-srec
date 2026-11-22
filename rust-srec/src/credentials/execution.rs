//! One bounded credential operation owns selection, repair and typed failover.

mod management;
mod pool;
mod repair;
mod state;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;
use std::sync::{Arc, Weak};
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

use parking_lot::Mutex;
#[cfg(test)]
use platforms_parser::extractor::error::ExtractorError;
use serde::Serialize;
use tokio::sync::Mutex as AsyncMutex;

#[cfg(test)]
use self::state::{AttemptStep, USE_RECORD_WINDOW};
use self::state::{Cursor, Rest};
pub use self::state::{Exclusions, next_renewal_at};
#[cfg(test)]
use super::{
    AccountStatus, CredentialError, CredentialIdentity, CredentialProfile, CredentialSelection,
    CredentialValidity, HealthReason, OperationDeadline, PoolStrategy, ProfileError,
    RefreshedCredentials, ResolvedCredentialPolicy, UnavailableReason,
};
use super::{
    CredentialBinding, CredentialMaterial, CredentialProfileHealth, CredentialProfileSummary,
    CredentialProviderRegistry, CredentialUnavailable,
};
use crate::database::repositories::CredentialProfileRepository;
use crate::proxies::ResolvedRoute;
#[cfg(test)]
use crate::proxies::RouteKey;
#[cfg(test)]
use crate::{Error, Result};

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

pub struct CredentialExecutionService {
    repository: Arc<CredentialProfileRepository>,
    providers: Arc<CredentialProviderRegistry>,
    cursors: Mutex<HashMap<String, Cursor>>,
    locks: dashmap::DashMap<String, Weak<AsyncMutex<()>>>,
    unavailable: Mutex<HashMap<String, (CredentialUnavailable, Instant)>>,
    /// When this process last wrote each account's last use.
    used: Mutex<HashMap<String, Instant>>,
    /// Accounts resting after a rejection their provider may lift by itself.
    resting: Mutex<HashMap<String, Rest>>,
}

impl CredentialExecutionService {
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
            resting: Mutex::new(HashMap::new()),
        }
    }

    pub fn repository(&self) -> &Arc<CredentialProfileRepository> {
        &self.repository
    }
}
