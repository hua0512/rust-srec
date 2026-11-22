//! Session authorization, login, refresh-token rotation, and logout operations.

use std::net::IpAddr;
use std::time::Instant;

use jiff::{SignedDuration, Timestamp};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use super::AuthService;
use super::cache::{CachedUserState, USER_STATE_CACHE_TTL};
use super::config::{
    API_KEY_PREFIX, AuthError, AuthPrincipal, AuthResponse, CredentialKind,
    LOGIN_DUMMY_PASSWORD_HASH, MAX_USERNAME_LENGTH, SessionInfo, bounded_device_info,
    session_expiration,
};
use crate::api::jwt::{Claims, JwtError};
use crate::api::rate_limit::LoginRateKey;
use crate::database::models::{ApiKeyAccessLevel, AuthSessionDbModel, RefreshTokenDbModel};
use crate::database::repositories::refresh_token::RefreshTokenRotation;

impl AuthService {
    /// Validate an access token and enforce durable session and user security state.
    pub(crate) async fn authorize_access_token(
        &self,
        token: &str,
        allow_password_remediation: bool,
    ) -> Result<Claims, AuthError> {
        let claims = self.jwt_service.validate_token(token).map_err(|error| {
            if matches!(error, JwtError::TokenExpired) {
                AuthError::TokenExpired
            } else {
                AuthError::InvalidToken
            }
        })?;

        self.authorize_session_claims(claims, allow_password_remediation, false)
            .await
    }

    pub(super) async fn authorize_session_claims(
        &self,
        claims: Claims,
        allow_password_remediation: bool,
        fresh_user: bool,
    ) -> Result<Claims, AuthError> {
        let now = crate::database::time::now_ms();
        if claims.exp <= u64::try_from(now / 1000).map_err(|_| AuthError::InvalidToken)? {
            return Err(AuthError::TokenExpired);
        }
        let session_id = claims
            .sid
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or(AuthError::InvalidToken)?;
        let session = self
            .token_repo
            .find_session(&claims.sub, session_id)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?
            .ok_or(AuthError::TokenRevoked)?;
        if session.revoked_at.is_some() {
            return Err(AuthError::TokenRevoked);
        }
        if session.expires_at <= now {
            return Err(AuthError::TokenExpired);
        }

        let cached = self
            .user_state_cache
            .get(&claims.sub)
            .filter(|entry| {
                !fresh_user
                    && entry.fetched_at.elapsed() < USER_STATE_CACHE_TTL
                    && (!entry.is_active
                        || (entry.must_change_password && !allow_password_remediation))
            })
            .map(|entry| *entry);

        let state = match cached {
            Some(state) => state,
            None => {
                let user = self
                    .user_repo
                    .find_by_id(&claims.sub)
                    .await
                    .map_err(|error| AuthError::Database(error.to_string()))?;
                let Some(user) = user else {
                    self.user_state_cache.remove(&claims.sub);
                    return Err(AuthError::UserNotFound);
                };
                let state = CachedUserState {
                    fetched_at: Instant::now(),
                    is_active: user.is_active,
                    must_change_password: user.must_change_password,
                };
                self.user_state_cache.insert(claims.sub.clone(), state);
                state
            }
        };

        if !state.is_active {
            warn!(user_id = %claims.sub, "Access denied: account disabled");
            return Err(AuthError::AccountDisabled);
        }

        if state.must_change_password && !allow_password_remediation {
            warn!(user_id = %claims.sub, "Access denied: password change required");
            return Err(AuthError::PasswordChangeRequired);
        }

        Ok(claims)
    }

    pub(crate) async fn authorize_credential(
        &self,
        raw: &str,
        allow_password_remediation: bool,
    ) -> Result<AuthPrincipal, AuthError> {
        let raw = raw.trim();
        if raw.starts_with(API_KEY_PREFIX) {
            self.authorize_api_key(raw).await
        } else {
            Ok(AuthPrincipal {
                claims: self
                    .authorize_access_token(raw, allow_password_remediation)
                    .await?,
                credential: CredentialKind::Jwt,
                access: ApiKeyAccessLevel::Full,
                api_key_id: None,
            })
        }
    }

    /// Revalidate a server-held principal against current durable credential state.
    pub(crate) async fn revalidate_principal(
        &self,
        principal: &AuthPrincipal,
    ) -> Result<AuthPrincipal, AuthError> {
        match principal.credential {
            CredentialKind::Jwt => Ok(AuthPrincipal {
                claims: self
                    .authorize_session_claims(principal.claims.clone(), false, true)
                    .await?,
                credential: CredentialKind::Jwt,
                access: ApiKeyAccessLevel::Full,
                api_key_id: None,
            }),
            CredentialKind::ApiKey => {
                let id = principal
                    .api_key_id
                    .as_deref()
                    .ok_or(AuthError::InvalidToken)?;
                let key = self
                    .api_key_repo
                    .find_by_id(&principal.claims.sub, id)
                    .await
                    .map_err(|error| AuthError::Database(error.to_string()))?
                    .ok_or(AuthError::TokenRevoked)?;
                if key.is_revoked() {
                    return Err(AuthError::TokenRevoked);
                }
                if key
                    .expires_at
                    .is_some_and(|expires| expires <= crate::database::time::now_ms())
                {
                    return Err(AuthError::TokenExpired);
                }
                let user = self
                    .user_repo
                    .find_by_id(&principal.claims.sub)
                    .await
                    .map_err(|error| AuthError::Database(error.to_string()))?
                    .ok_or(AuthError::UserNotFound)?;
                if !user.is_active {
                    return Err(AuthError::AccountDisabled);
                }
                if user.must_change_password {
                    return Err(AuthError::PasswordChangeRequired);
                }
                Ok(AuthPrincipal {
                    claims: Claims {
                        roles: user.get_roles(),
                        ..principal.claims.clone()
                    },
                    credential: CredentialKind::ApiKey,
                    access: key.get_access_level(),
                    api_key_id: Some(key.id),
                })
            }
        }
    }

    /// Generate a cryptographically secure refresh token (256 bits).
    pub(super) fn generate_refresh_token() -> String {
        use rand::RngExt;
        let bytes: [u8; 32] = rand::rng().random();
        hex::encode(bytes)
    }

    /// Hash an opaque token (refresh token or API key) using SHA-256.
    pub(super) fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn token_hash_prefix(token_hash: &str) -> &str {
        const PREFIX_LEN: usize = 10;
        let end = std::cmp::min(PREFIX_LEN, token_hash.len());
        &token_hash[..end]
    }

    /// Authenticate a user with username and password.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        device_info: Option<String>,
        client_ip: Option<IpAddr>,
    ) -> Result<AuthResponse, AuthError> {
        let device_info = bounded_device_info(device_info);
        debug!(
            device_info = ?device_info.as_deref(),
            "Login attempt"
        );

        if username.chars().count() > MAX_USERNAME_LENGTH {
            return Err(AuthError::UsernameTooLong {
                max: MAX_USERNAME_LENGTH,
            });
        }

        let mut rate_keys = vec![LoginRateKey::username(username)];
        if let Some(ip) = client_ip {
            rate_keys.push(LoginRateKey::Ip(ip));
        }

        let reserved_at = match self.login_rate_limiter.try_begin(&rate_keys) {
            Ok(reserved_at) => reserved_at,
            Err(retry_after) => {
                warn!(
                    client_ip = ?client_ip,
                    retry_after_secs = retry_after.as_secs(),
                    "Login throttled: failed-attempt budget exhausted"
                );
                let retry_after_secs =
                    retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
                return Err(AuthError::TooManyAttempts {
                    retry_after_secs: retry_after_secs.max(1),
                });
            }
        };

        let user = match self.user_repo.find_by_username(username).await {
            Ok(user) => user,
            Err(e) => {
                self.login_rate_limiter.release(&rate_keys, reserved_at);
                return Err(AuthError::Database(e.to_string()));
            }
        };
        let password_hash = user.as_ref().map_or(LOGIN_DUMMY_PASSWORD_HASH, |user| {
            user.password_hash.as_str()
        });
        let password_matches = match self.verify_password_blocking(password, password_hash).await {
            Ok(password_matches) => password_matches,
            Err(error) => {
                self.login_rate_limiter.release(&rate_keys, reserved_at);
                return Err(error);
            }
        };
        let Some(user) = user.filter(|_| password_matches) else {
            warn!("Login failed: invalid credentials");
            return Err(AuthError::InvalidCredentials);
        };
        if !user.is_active {
            warn!(user_id = %user.id, "Login blocked: account disabled");
            return Err(AuthError::AccountDisabled);
        }

        let now = Timestamp::now();
        let roles = user.get_roles();
        let session_id = uuid::Uuid::new_v4().to_string();
        let (access_token, access_expires_at) = self
            .jwt_service
            .generate_session_token(&user.id, roles.clone(), &session_id)
            .map_err(|error| {
                self.login_rate_limiter.release(&rate_keys, reserved_at);
                AuthError::Internal(error.to_string())
            })?;
        let refresh_token = Self::generate_refresh_token();
        let refresh_expires_at = self.refresh_expiration(now).inspect_err(|_| {
            self.login_rate_limiter.release(&rate_keys, reserved_at);
        })?;
        let mut token_model = RefreshTokenDbModel::new(
            &user.id,
            Self::hash_token(&refresh_token),
            refresh_expires_at,
            device_info,
        );
        token_model.session_id = Some(session_id.clone());
        let session = AuthSessionDbModel {
            id: session_id,
            user_id: user.id.clone(),
            created_at: now.as_millisecond(),
            expires_at: session_expiration(access_expires_at, token_model.expires_at).inspect_err(
                |_| {
                    self.login_rate_limiter.release(&rate_keys, reserved_at);
                },
            )?,
            revoked_at: None,
        };
        if !self
            .token_repo
            .create_session(&session, &token_model, &user.password_hash)
            .await
            .map_err(|error| {
                self.login_rate_limiter.release(&rate_keys, reserved_at);
                AuthError::Database(error.to_string())
            })?
        {
            return Err(AuthError::InvalidCredentials);
        }
        self.login_rate_limiter
            .commit_success(&rate_keys, reserved_at);
        self.invalidate_user_state(&user.id);
        info!(
            user_id = %user.id,
            username = %username,
            refresh_token_id = %token_model.id,
            refresh_expires_at = %token_model.expires_at,
            device_info = ?token_model.device_info.as_deref(),
            "Login successful (refresh token issued)"
        );

        Ok(AuthResponse {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.config.access_token_expiration_secs,
            refresh_expires_in: self.config.refresh_token_expiration_secs,
            roles,
            must_change_password: user.must_change_password,
        })
    }

    /// Refresh tokens using a valid refresh token.
    pub async fn refresh_tokens(&self, refresh_token: &str) -> Result<AuthResponse, AuthError> {
        let refresh_token = refresh_token.trim();
        if refresh_token.is_empty() {
            warn!("Empty refresh token presented");
            return Err(AuthError::InvalidToken);
        }

        let token_hash = Self::hash_token(refresh_token);
        let token_hash_prefix = Self::token_hash_prefix(&token_hash);
        debug!(token_hash_prefix = %token_hash_prefix, "Refresh token request received");

        let mut stored_token = self
            .token_repo
            .find_by_token_hash(&token_hash)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or_else(|| {
                warn!(token_hash_prefix = %token_hash_prefix, "Refresh token not found");
                AuthError::InvalidToken
            })?;
        stored_token.device_info = bounded_device_info(stored_token.device_info.take());
        let session_id = stored_token
            .session_id
            .as_deref()
            .unwrap_or(&stored_token.id)
            .to_owned();
        if self
            .token_repo
            .find_session(&stored_token.user_id, &session_id)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?
            .is_some_and(|session| session.revoked_at.is_some())
        {
            return Err(AuthError::TokenRevoked);
        }

        if let Some(revoked_at) = stored_token.revoked_at {
            self.revoke_sessions_on_refresh_reuse(&stored_token.user_id, &session_id, revoked_at)
                .await?;
            return Err(AuthError::TokenRevoked);
        }

        if stored_token.is_expired() {
            info!(
                user_id = %stored_token.user_id,
                refresh_token_id = %stored_token.id,
                refresh_expires_at = %stored_token.expires_at,
                device_info = ?stored_token.device_info.as_deref(),
                token_hash_prefix = %token_hash_prefix,
                "Expired refresh token presented"
            );
            return Err(AuthError::TokenExpired);
        }

        let user = self
            .user_repo
            .find_by_id(&stored_token.user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or(AuthError::UserNotFound)?;

        if !user.is_active {
            warn!(
                user_id = %user.id,
                "Token refresh blocked: account disabled"
            );
            return Err(AuthError::AccountDisabled);
        }

        let roles = user.get_roles();
        let (access_token, access_expires_at) = self
            .jwt_service
            .generate_session_token(&user.id, roles.clone(), &session_id)
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let new_refresh_token = Self::generate_refresh_token();
        let new_refresh_token_hash = Self::hash_token(&new_refresh_token);
        let now = Timestamp::now();
        let refresh_expires_at = self.refresh_expiration(now)?;

        let mut token_model = RefreshTokenDbModel::new(
            &user.id,
            new_refresh_token_hash,
            refresh_expires_at,
            stored_token.device_info,
        );
        token_model.session_id = Some(session_id.clone());
        match self
            .token_repo
            .rotate_session(
                &stored_token.id,
                &token_model,
                session_expiration(access_expires_at, token_model.expires_at)?,
            )
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
        {
            RefreshTokenRotation::Rotated => {}
            RefreshTokenRotation::SessionRevoked => return Err(AuthError::TokenRevoked),
            RefreshTokenRotation::Revoked { revoked_at } => {
                self.revoke_sessions_on_refresh_reuse(
                    &stored_token.user_id,
                    &session_id,
                    revoked_at,
                )
                .await?;
                return Err(AuthError::TokenRevoked);
            }
            RefreshTokenRotation::Expired => return Err(AuthError::TokenExpired),
            RefreshTokenRotation::NotFound => return Err(AuthError::InvalidToken),
        }

        info!(
            user_id = %user.id,
            old_refresh_token_id = %stored_token.id,
            new_refresh_token_id = %token_model.id,
            refresh_expires_at = %token_model.expires_at,
            device_info = ?token_model.device_info.as_deref(),
            token_hash_prefix = %token_hash_prefix,
            "Token refresh succeeded (refresh token rotated)"
        );

        Ok(AuthResponse {
            access_token,
            refresh_token: new_refresh_token,
            token_type: "Bearer".to_string(),
            expires_in: self.config.access_token_expiration_secs,
            refresh_expires_in: self.config.refresh_token_expiration_secs,
            roles,
            must_change_password: user.must_change_password,
        })
    }

    fn refresh_expiration(&self, now: Timestamp) -> Result<Timestamp, AuthError> {
        let duration = i64::try_from(self.config.refresh_token_expiration_secs)
            .ok()
            .map(SignedDuration::from_secs)
            .ok_or_else(|| {
                AuthError::Internal("Refresh token expiration exceeds supported range".to_owned())
            })?;
        now.checked_add(duration).map_err(|_| {
            AuthError::Internal("Refresh token expiration exceeds supported range".to_owned())
        })
    }

    async fn revoke_sessions_on_refresh_reuse(
        &self,
        user_id: &str,
        session_id: &str,
        revoked_at: i64,
    ) -> Result<(), AuthError> {
        let elapsed = crate::database::time::now_ms().saturating_sub(revoked_at);
        let within_grace = self.config.refresh_token_reuse_grace_secs > 0
            && u64::try_from(elapsed).is_ok_and(|elapsed| {
                elapsed
                    <= self
                        .config
                        .refresh_token_reuse_grace_secs
                        .saturating_mul(1000)
            });
        if self.config.revoke_all_on_refresh_token_reuse && !within_grace {
            self.token_repo
                .revoke_all_for_active_session(user_id, session_id)
                .await
                .map_err(|e| AuthError::Database(e.to_string()))?;
            warn!(
                user_id,
                "Revoked all refresh tokens after refresh token reuse"
            );
        } else {
            debug!(
                user_id,
                within_grace, "Refresh token reuse rejected without revoking other sessions"
            );
        }
        Ok(())
    }

    /// Logout closes only the refresh token's session lineage, including issued access tokens.
    #[cfg(test)]
    pub async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        self.logout_with_principal(refresh_token, None).await
    }

    pub(crate) async fn logout_with_principal(
        &self,
        refresh_token: &str,
        principal: Option<&AuthPrincipal>,
    ) -> Result<(), AuthError> {
        let authenticated = if let Some(principal) = principal {
            if principal.credential != CredentialKind::Jwt {
                return Err(AuthError::InvalidToken);
            }
            Some(
                self.authorize_session_claims(principal.claims.clone(), true, true)
                    .await?,
            )
        } else {
            None
        };
        let token_hash = Self::hash_token(refresh_token.trim());
        let stored = self
            .token_repo
            .find_by_token_hash(&token_hash)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?;
        let (user_id, session_id) = match stored {
            Some(token) => {
                if authenticated
                    .as_ref()
                    .is_some_and(|claims| claims.sub != token.user_id)
                {
                    return Err(AuthError::InvalidToken);
                }
                let session = token.session_id.unwrap_or(token.id);
                (token.user_id, session)
            }
            None => {
                let claims = authenticated.ok_or(AuthError::InvalidToken)?;
                (claims.sub, claims.sid.ok_or(AuthError::InvalidToken)?)
            }
        };
        self.token_repo
            .revoke_session(&user_id, &session_id)
            .await
            .map_err(|error| AuthError::Database(error.to_string()))?;
        info!(%user_id, %session_id, "Logout successful (session revoked)");
        Ok(())
    }

    /// Logout from all sessions by revoking all refresh tokens for a user.
    pub async fn logout_all(&self, user_id: &str) -> Result<(), AuthError> {
        self.token_repo
            .revoke_all_for_user(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;
        self.invalidate_user_state(user_id);

        info!(user_id = %user_id, "Logout-all successful (all refresh tokens revoked)");

        Ok(())
    }

    /// List active sessions for a user.
    pub async fn list_active_sessions(&self, user_id: &str) -> Result<Vec<SessionInfo>, AuthError> {
        let tokens = self
            .token_repo
            .find_active_by_user(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        let sessions = tokens
            .into_iter()
            .map(|t| SessionInfo {
                id: t.id,
                device_info: bounded_device_info(t.device_info),
                created_at: t.created_at,
                expires_at: t.expires_at,
            })
            .collect();

        Ok(sessions)
    }
}
