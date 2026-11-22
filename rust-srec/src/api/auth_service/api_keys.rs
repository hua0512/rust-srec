//! API key creation, listing, revocation, and cached authorization.

use std::sync::atomic::Ordering;
use std::time::Instant;

use tracing::{info, warn};

use super::AuthService;
use super::cache::{API_KEY_LAST_USED_WRITE_INTERVAL, CachedApiKeyState, USER_STATE_CACHE_TTL};
use super::config::{API_KEY_PREFIX, AuthError, AuthPrincipal, CredentialKind};
use crate::api::jwt::Claims;
use crate::database::models::{ApiKeyAccessLevel, ApiKeyDbModel};

impl AuthService {
    /// Generate a raw API key: `API_KEY_PREFIX` + 256 bits of randomness in hex.
    fn generate_api_key() -> String {
        use rand::RngExt;
        let bytes: [u8; 32] = rand::rng().random();
        format!("{API_KEY_PREFIX}{}", hex::encode(bytes))
    }

    /// Create a new API key for `user_id`.
    pub async fn create_api_key(
        &self,
        user_id: &str,
        name: &str,
        access_level: ApiKeyAccessLevel,
        expires_at: Option<i64>,
    ) -> Result<(ApiKeyDbModel, String), AuthError> {
        let raw_key = Self::generate_api_key();
        let key_hash = Self::hash_token(&raw_key);
        let key_prefix = &raw_key[..API_KEY_PREFIX.len() + 8];

        let model = ApiKeyDbModel::new(
            user_id,
            name,
            key_hash,
            key_prefix,
            access_level,
            expires_at,
        );
        self.api_key_repo
            .create(&model)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        info!(
            user_id = %user_id,
            api_key_id = %model.id,
            key_prefix = %model.key_prefix,
            access_level = %model.access_level,
            expires_at = ?model.expires_at,
            "API key created"
        );

        Ok((model, raw_key))
    }

    /// List all API keys (including revoked/expired) owned by `user_id`.
    pub async fn list_api_keys(&self, user_id: &str) -> Result<Vec<ApiKeyDbModel>, AuthError> {
        self.api_key_repo
            .list_by_user(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))
    }

    /// Revoke an API key owned by `user_id`. Returns false when the key does
    /// not exist, belongs to another user, or is already revoked.
    pub async fn revoke_api_key(&self, user_id: &str, key_id: &str) -> Result<bool, AuthError> {
        let revoked = self
            .api_key_repo
            .revoke(user_id, key_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        if revoked {
            self.api_key_cache_generation.fetch_add(1, Ordering::AcqRel);
            self.api_key_state_cache
                .retain(|_, state| state.key_id != key_id);
            info!(user_id = %user_id, api_key_id = %key_id, "API key revoked");
        }

        Ok(revoked)
    }

    /// Validate a raw API key and produce the request principal.
    pub(crate) async fn authorize_api_key(
        &self,
        raw_key: &str,
    ) -> Result<AuthPrincipal, AuthError> {
        let raw_key = raw_key.trim();
        if raw_key.is_empty() {
            return Err(AuthError::InvalidToken);
        }

        let key_hash = Self::hash_token(raw_key);

        let state = loop {
            let generation = self.api_key_cache_generation.load(Ordering::Acquire);
            if let Some(cached) = self
                .api_key_state_cache
                .get(&key_hash)
                .filter(|entry| {
                    entry.generation == generation
                        && entry.fetched_at.elapsed() < USER_STATE_CACHE_TTL
                })
                .map(|entry| entry.clone())
            {
                break cached;
            }

            let key = self
                .api_key_repo
                .find_by_key_hash(&key_hash)
                .await
                .map_err(|e| AuthError::Database(e.to_string()))?;
            let Some(key) = key else {
                self.api_key_state_cache.remove(&key_hash);
                return Err(AuthError::InvalidToken);
            };

            let user = self
                .user_repo
                .find_by_id(&key.user_id)
                .await
                .map_err(|e| AuthError::Database(e.to_string()))?;
            let Some(user) = user else {
                self.api_key_state_cache.remove(&key_hash);
                warn!(api_key_id = %key.id, "API key rejected: owning user not found");
                return Err(AuthError::UserNotFound);
            };

            let state = CachedApiKeyState {
                generation,
                fetched_at: Instant::now(),
                key_id: key.id.clone(),
                user_id: key.user_id.clone(),
                roles: user.get_roles(),
                user_is_active: user.is_active,
                must_change_password: user.must_change_password,
                access_level: key.get_access_level(),
                expires_at: key.expires_at,
                revoked: key.is_revoked(),
            };

            self.api_key_state_cache
                .insert(key_hash.clone(), state.clone());
            if self.api_key_cache_generation.load(Ordering::Acquire) == generation {
                break state;
            }

            // A revocation or user-state invalidation completed while this state
            // was being fetched. Discard the stale fill and retry.
            self.api_key_state_cache.remove(&key_hash);
        };

        if state.revoked {
            warn!(api_key_id = %state.key_id, "API key rejected: revoked");
            return Err(AuthError::TokenRevoked);
        }

        if state
            .expires_at
            .is_some_and(|at| at < crate::database::time::now_ms())
        {
            info!(api_key_id = %state.key_id, "API key rejected: expired");
            return Err(AuthError::TokenExpired);
        }

        if !state.user_is_active {
            warn!(user_id = %state.user_id, api_key_id = %state.key_id, "API key rejected: account disabled");
            return Err(AuthError::AccountDisabled);
        }

        // In local desktop mode the flag means "no password has been set" and
        // only in-process sessions can act for the account, so any key it has
        // was created by a session that was allowed to act; it is honoured.
        if state.must_change_password && !self.is_local_desktop_mode() {
            warn!(user_id = %state.user_id, api_key_id = %state.key_id, "API key rejected: password change required");
            return Err(AuthError::PasswordChangeRequired);
        }

        self.touch_api_key(&state.key_id).await;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| AuthError::Internal(e.to_string()))?
            .as_secs();
        let claims = Claims {
            sub: state.user_id.clone(),
            roles: state.roles.clone(),
            iss: "rust-srec".to_string(),
            aud: "rust-srec-api".to_string(),
            exp: now.saturating_add(self.config.access_token_expiration_secs),
            iat: now,
            sid: None,
        };

        Ok(AuthPrincipal {
            claims,
            credential: CredentialKind::ApiKey,
            access: state.access_level,
            api_key_id: Some(state.key_id),
        })
    }

    async fn touch_api_key(&self, key_id: &str) {
        let due = self
            .api_key_last_used_writes
            .get(key_id)
            .is_none_or(|last| last.elapsed() >= API_KEY_LAST_USED_WRITE_INTERVAL);
        if !due {
            return;
        }
        self.api_key_last_used_writes
            .insert(key_id.to_string(), Instant::now());

        let now = crate::database::time::now_ms();
        if let Err(error) = self.api_key_repo.update_last_used(key_id, now).await {
            warn!(error = %error, api_key_id = %key_id, "Failed to update API key last_used_at");
        }
    }
}
