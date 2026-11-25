//! User-state and API-key authorization caches and invalidation helpers.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::AuthService;
use crate::api::auth_store::ApiKeyAccessLevel;

/// TTL for cached user-state denials and API-key snapshots. Session authorization
/// always reloads any state that could grant access, including password remediation.
pub(super) const USER_STATE_CACHE_TTL: Duration = Duration::from_secs(5);

/// Minimum interval between `api_keys.last_used_at` writes for one key, so
/// `authorize_api_key` does not issue an UPDATE per authenticated request.
pub(super) const API_KEY_LAST_USED_WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// Snapshot of the per-user flags enforced by `authorize_access_token`.
///
/// Only the result of `user_repo.find_by_id` is cached;
/// `allow_password_remediation` is passed per call because it reflects which
/// route the request hit (`AuthLayer::password_remediation` vs
/// `AuthLayer::new`), not the user row.
#[derive(Clone, Copy)]
pub(super) struct CachedUserState {
    pub(super) fetched_at: Instant,
    pub(super) is_active: bool,
    pub(super) must_change_password: bool,
}

/// Snapshot of the key + owning-user state enforced by `authorize_api_key`,
/// cached under the key hash with the same TTL discipline as
/// `CachedUserState` (`USER_STATE_CACHE_TTL`). Expiry is evaluated against
/// the current time on every use, so caching `expires_at` cannot extend a
/// key's lifetime.
#[derive(Clone)]
pub(super) struct CachedApiKeyState {
    pub(super) generation: u64,
    pub(super) fetched_at: Instant,
    pub(super) key_id: String,
    pub(super) user_id: String,
    pub(super) roles: Vec<String>,
    pub(super) user_is_active: bool,
    pub(super) must_change_password: bool,
    pub(super) access_level: ApiKeyAccessLevel,
    pub(super) expires_at: Option<i64>,
    pub(super) revoked: bool,
}

impl AuthService {
    /// Drop cached user and API-key authorization state for one user.
    ///
    /// Advancing `api_key_cache_generation` before evicting matching entries
    /// ensures an in-flight `authorize_api_key` lookup that read pre-write user
    /// roles or flags cannot insert a stale positive grant afterwards.
    pub(crate) fn invalidate_user_state(&self, user_id: &str) {
        self.user_state_cache.remove(user_id);
        self.api_key_cache_generation.fetch_add(1, Ordering::AcqRel);
        self.api_key_state_cache
            .retain(|_, state| state.user_id != user_id);
    }

    /// Drop all cached user and API-key authorization state.
    ///
    /// Used by bulk user writes (such as configuration imports) that modify
    /// user rows outside `AuthService`.
    pub(crate) fn invalidate_user_cache(&self) {
        self.user_state_cache.clear();
        self.api_key_cache_generation.fetch_add(1, Ordering::AcqRel);
        self.api_key_state_cache.clear();
    }
}
