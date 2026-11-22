//! Authentication service for user login, token management, and password operations.
//!
//! This module provides the core authentication functionality including:
//! - User authentication with password verification
//! - Refresh token generation and rotation
//! - Password change with validation
//! - Session management (logout, logout-all)

mod api_keys;
mod cache;
mod config;
mod local_desktop;
mod sessions;
#[cfg(test)]
mod tests;
mod users;

#[cfg(test)]
mod login_privacy_tests;
#[cfg(test)]
mod rotation_tests;
#[cfg(test)]
mod session_tests;

#[cfg(test)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicU64};
use std::time::Instant;

#[cfg(test)]
use argon2::password_hash::phc::PasswordHash;
use dashmap::DashMap;
use tokio::sync::Semaphore;

use self::cache::{CachedApiKeyState, CachedUserState};
#[cfg(test)]
use self::config::LOGIN_DUMMY_PASSWORD_HASH;
pub use self::config::{
    API_KEY_PREFIX, AuthConfig, AuthError, AuthPrincipal, AuthResponse, CredentialKind,
    MAX_USERNAME_LENGTH, SessionInfo,
};
pub use self::local_desktop::LocalDesktopAccount;
use self::local_desktop::LocalDesktopSessionRecord;
use self::users::password_work_permits;
use crate::api::jwt::JwtService;
use crate::api::rate_limit::LoginRateLimiter;
#[cfg(test)]
use crate::database::models::{AuthSessionDbModel, RefreshTokenDbModel};
use crate::database::repositories::{ApiKeyRepository, RefreshTokenRepository, UserRepository};

/// Authentication service for managing user authentication and tokens.
pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    token_repo: Arc<dyn RefreshTokenRepository>,
    api_key_repo: Arc<dyn ApiKeyRepository>,
    jwt_service: Arc<JwtService>,
    config: AuthConfig,
    /// User-state cache read by `authorize_access_token`. Entries older than
    /// `USER_STATE_CACHE_TTL` are treated exactly like a missing entry, so a
    /// stale entry can never back an Ok decision.
    user_state_cache: DashMap<String, CachedUserState>,
    /// Key-hash -> state cache read by `authorize_api_key`; same TTL rules
    /// as `user_state_cache`. Cache fills are tagged with
    /// `api_key_cache_generation` so an in-flight fill cannot survive a
    /// completed revocation or user-state invalidation.
    api_key_state_cache: DashMap<String, CachedApiKeyState>,
    /// Incremented after every API-key revocation or user-state invalidation.
    /// A cache entry from an older generation is never used for authorization.
    api_key_cache_generation: AtomicU64,
    /// Per-key timestamp of the last `update_last_used` write, consulted by
    /// `touch_api_key` to throttle to `API_KEY_LAST_USED_WRITE_INTERVAL`.
    api_key_last_used_writes: DashMap<String, Instant>,
    /// Failed-login counters consulted and updated by `authenticate`.
    login_rate_limiter: LoginRateLimiter,
    /// Caps concurrent Argon2id work started by `verify_password_blocking`
    /// and `hash_password_blocking`.
    password_work_permits: Arc<Semaphore>,
    /// Sessions issued by `issue_local_desktop_session`, keyed by session id.
    /// Only the desktop shell's in-process backend ever fills this.
    local_desktop_sessions: DashMap<String, LocalDesktopSessionRecord>,
    /// Set for the desktop app's built-in backend, where an account that
    /// still has `must_change_password` is reachable only through sessions
    /// the shell issues in-process; see [`Self::with_local_desktop_mode`].
    local_desktop_mode: bool,
    #[cfg(test)]
    password_verify_calls: Arc<AtomicU64>,
}

impl AuthService {
    /// Create a new AuthService.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        token_repo: Arc<dyn RefreshTokenRepository>,
        api_key_repo: Arc<dyn ApiKeyRepository>,
        jwt_service: Arc<JwtService>,
        config: AuthConfig,
    ) -> Self {
        Self {
            user_repo,
            token_repo,
            api_key_repo,
            jwt_service,
            config,
            user_state_cache: DashMap::new(),
            api_key_state_cache: DashMap::new(),
            api_key_cache_generation: AtomicU64::new(0),
            api_key_last_used_writes: DashMap::new(),
            login_rate_limiter: LoginRateLimiter::from_env(),
            password_work_permits: Arc::new(Semaphore::new(password_work_permits())),
            local_desktop_sessions: DashMap::new(),
            local_desktop_mode: false,
            #[cfg(test)]
            password_verify_calls: Arc::new(AtomicU64::new(0)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_login_rate_limiter(mut self, login_rate_limiter: LoginRateLimiter) -> Self {
        self.login_rate_limiter = login_rate_limiter;
        self
    }

    pub fn user_repository(&self) -> Arc<dyn UserRepository> {
        self.user_repo.clone()
    }
}
