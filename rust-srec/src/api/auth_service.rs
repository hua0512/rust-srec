//! Authentication service for user login, token management, and password operations.
//!
//! This module provides the core authentication functionality including:
//! - User authentication with password verification
//! - Refresh token generation and rotation
//! - Password change with validation
//! - Session management (logout, logout-all)

use std::sync::Arc;

use argon2::{
    Argon2, Params,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::database::models::RefreshTokenDbModel;
use crate::database::repositories::{RefreshTokenRepository, UserRepository};

use super::jwt::JwtService;

/// Authentication configuration.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Access token expiration in seconds (default: 3600 = 1 hour)
    pub access_token_expiration_secs: u64,
    /// Refresh token expiration in seconds (default: 604800 = 7 days)
    pub refresh_token_expiration_secs: u64,
    /// Grace window to tolerate replay of a recently-rotated refresh token (default: 0 = disabled).
    ///
    /// This helps with clients that accidentally send the old refresh token again due to
    /// concurrent refresh attempts or retries.
    pub refresh_token_reuse_grace_secs: u64,
    /// Whether to revoke all refresh tokens for a user when a revoked token is presented.
    ///
    /// Default is `false` to avoid logging out other sessions due to common client-side races.
    pub revoke_all_on_refresh_token_reuse: bool,
    /// Minimum password length
    pub min_password_length: usize,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            access_token_expiration_secs: 3600,    // 1 hour
            refresh_token_expiration_secs: 604800, // 7 days
            refresh_token_reuse_grace_secs: 0,
            revoke_all_on_refresh_token_reuse: false,
            min_password_length: 8,
        }
    }
}

impl AuthConfig {
    /// Create AuthConfig from environment variables.
    ///
    /// Environment variables:
    /// - `ACCESS_TOKEN_EXPIRATION_SECS`: Access token expiration in seconds (default: 3600 = 1 hour)
    /// - `REFRESH_TOKEN_EXPIRATION_SECS`: Refresh token expiration in seconds (default: 604800 = 7 days)
    /// - `REFRESH_TOKEN_REUSE_GRACE_SECS`: Grace window for recently rotated tokens (default: 0)
    /// - `REVOKE_ALL_ON_REFRESH_TOKEN_REUSE`: Whether to revoke all tokens on revoked-token reuse (default: false)
    /// - `MIN_PASSWORD_LENGTH`: Minimum password length (default: 8)
    pub fn from_env() -> Self {
        let access_token_expiration_secs = std::env::var("ACCESS_TOKEN_EXPIRATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3600);

        let refresh_token_expiration_secs = std::env::var("REFRESH_TOKEN_EXPIRATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(604800);

        let refresh_token_reuse_grace_secs = std::env::var("REFRESH_TOKEN_REUSE_GRACE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let revoke_all_on_refresh_token_reuse = std::env::var("REVOKE_ALL_ON_REFRESH_TOKEN_REUSE")
            .ok()
            .is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            });

        let min_password_length = std::env::var("MIN_PASSWORD_LENGTH")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(8);

        Self {
            access_token_expiration_secs,
            refresh_token_expiration_secs,
            refresh_token_reuse_grace_secs,
            revoke_all_on_refresh_token_reuse,
            min_password_length,
        }
    }
}

/// Authentication errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Account is disabled")]
    AccountDisabled,

    #[error("Token has expired")]
    TokenExpired,

    #[error("Token has been revoked")]
    TokenRevoked,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Password change required")]
    PasswordChangeRequired,

    #[error("Password does not meet requirements: {0}")]
    WeakPassword(String),

    #[error("Current password is incorrect")]
    IncorrectCurrentPassword,

    #[error("User not found")]
    UserNotFound,

    #[error("Database error: {0}")]
    Database(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

/// Authentication response returned on successful login or token refresh.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthResponse {
    /// JWT access token
    pub access_token: String,
    /// Opaque refresh token
    pub refresh_token: String,
    /// Token type (always "Bearer")
    pub token_type: String,
    /// Access token expiration in seconds
    pub expires_in: u64,
    /// Refresh token expiration in seconds
    pub refresh_expires_in: u64,
    /// User's roles
    pub roles: Vec<String>,
    /// Whether the user must change their password
    pub must_change_password: bool,
}

/// Session information for active sessions listing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct SessionInfo {
    /// Session ID (token ID)
    pub id: String,
    /// Device information if available
    pub device_info: Option<String>,
    /// When the session was created
    pub created_at: String,
    /// When the session expires
    pub expires_at: String,
}

/// Authentication service for managing user authentication and tokens.
pub struct AuthService {
    user_repo: Arc<dyn UserRepository>,
    token_repo: Arc<dyn RefreshTokenRepository>,
    jwt_service: Arc<JwtService>,
    config: AuthConfig,
}

impl AuthService {
    /// Create a new AuthService.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        token_repo: Arc<dyn RefreshTokenRepository>,
        jwt_service: Arc<JwtService>,
        config: AuthConfig,
    ) -> Self {
        Self {
            user_repo,
            token_repo,
            jwt_service,
            config,
        }
    }

    /// Hash a password using Argon2id with OWASP recommended parameters.
    pub fn hash_password(password: &str) -> Result<String, AuthError> {
        // OWASP recommended parameters: m=19456 (19 MiB), t=2, p=1
        let params = Params::new(19456, 2, 1, None)
            .map_err(|e| AuthError::Internal(format!("Invalid Argon2 params: {}", e)))?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let salt = SaltString::generate(&mut OsRng);
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AuthError::Internal(format!("Password hashing failed: {}", e)))?
            .to_string();

        Ok(password_hash)
    }

    /// Verify a password against a stored hash.
    pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| AuthError::Internal(format!("Invalid password hash format: {}", e)))?;

        // Use default Argon2 for verification (it reads params from the hash)
        let argon2 = Argon2::default();
        Ok(argon2
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// Generate a cryptographically secure refresh token (256 bits).
    fn generate_refresh_token() -> String {
        use rand::Rng;
        let bytes: [u8; 32] = rand::rng().random(); // 256 bits
        hex::encode(bytes)
    }

    /// Hash a refresh token using SHA-256.
    fn hash_refresh_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    fn token_hash_prefix(token_hash: &str) -> &str {
        const PREFIX_LEN: usize = 10;
        let end = std::cmp::min(PREFIX_LEN, token_hash.len());
        &token_hash[..end]
    }

    /// Validate password strength.
    pub fn validate_password_strength(&self, password: &str) -> Result<(), AuthError> {
        if password.len() < self.config.min_password_length {
            return Err(AuthError::WeakPassword(format!(
                "Password must be at least {} characters",
                self.config.min_password_length
            )));
        }

        let has_letter = password.chars().any(|c| c.is_alphabetic());
        let has_number = password.chars().any(|c| c.is_numeric());

        if !has_letter {
            return Err(AuthError::WeakPassword(
                "Password must contain at least one letter".to_string(),
            ));
        }

        if !has_number {
            return Err(AuthError::WeakPassword(
                "Password must contain at least one number".to_string(),
            ));
        }

        Ok(())
    }
}

impl AuthService {
    /// Authenticate a user with username and password.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
        device_info: Option<String>,
    ) -> Result<AuthResponse, AuthError> {
        debug!(
            username = %username,
            device_info = ?device_info.as_deref(),
            "Login attempt"
        );

        // Find user by username
        let user = self
            .user_repo
            .find_by_username(username)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;

        // Check if account is active
        if !user.is_active {
            warn!(user_id = %user.id, username = %username, "Login blocked: account disabled");
            return Err(AuthError::AccountDisabled);
        }

        // Verify password
        if !Self::verify_password(password, &user.password_hash)? {
            warn!(user_id = %user.id, username = %username, "Login failed: invalid credentials");
            return Err(AuthError::InvalidCredentials);
        }

        // Update last login timestamp
        let now = Utc::now();
        self.user_repo
            .update_last_login(&user.id, now)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        // Generate tokens
        let roles = user.get_roles();
        let access_token = self
            .jwt_service
            .generate_token(&user.id, roles.clone())
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let refresh_token = Self::generate_refresh_token();
        let refresh_token_hash = Self::hash_refresh_token(&refresh_token);
        let refresh_expires_at =
            now + Duration::seconds(self.config.refresh_token_expiration_secs as i64);

        // Store refresh token
        let token_model = RefreshTokenDbModel::new(
            &user.id,
            refresh_token_hash,
            refresh_expires_at,
            device_info,
        );
        self.token_repo
            .create(&token_model)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

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

        let token_hash = Self::hash_refresh_token(refresh_token);
        let token_hash_prefix = Self::token_hash_prefix(&token_hash);
        debug!(token_hash_prefix = %token_hash_prefix, "Refresh token request received");

        // Find the token
        let stored_token = self
            .token_repo
            .find_by_token_hash(&token_hash)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or_else(|| {
                warn!(token_hash_prefix = %token_hash_prefix, "Refresh token not found");
                AuthError::InvalidToken
            })?;

        // Check if token is revoked (potential reuse attack).
        // Note: tokens are revoked on every successful refresh (rotation), so clients that
        // retry/concurrently refresh can legitimately present a recently revoked token.
        let is_revoked = stored_token.is_revoked();
        let revoked_recently = if is_revoked && self.config.refresh_token_reuse_grace_secs > 0 {
            stored_token
                .get_revoked_at()
                .map(|revoked_at| {
                    let grace =
                        Duration::seconds(self.config.refresh_token_reuse_grace_secs as i64);
                    (Utc::now() - revoked_at) <= grace
                })
                .unwrap_or(false)
        } else {
            false
        };

        if is_revoked && revoked_recently {
            debug!(
                user_id = %stored_token.user_id,
                refresh_token_id = %stored_token.id,
                refresh_expires_at = %stored_token.expires_at,
                refresh_revoked_at = ?stored_token.revoked_at.as_deref(),
                device_info = ?stored_token.device_info.as_deref(),
                token_hash_prefix = %token_hash_prefix,
                grace_secs = %self.config.refresh_token_reuse_grace_secs,
                "Recently revoked refresh token presented; proceeding due to grace window"
            );
        } else if is_revoked {
            warn!(
                user_id = %stored_token.user_id,
                refresh_token_id = %stored_token.id,
                refresh_expires_at = %stored_token.expires_at,
                refresh_revoked_at = ?stored_token.revoked_at.as_deref(),
                device_info = ?stored_token.device_info.as_deref(),
                token_hash_prefix = %token_hash_prefix,
                "Revoked refresh token presented (possible token reuse)"
            );

            if self.config.revoke_all_on_refresh_token_reuse {
                // Security breach detection: revoke all tokens for this user
                self.token_repo
                    .revoke_all_for_user(&stored_token.user_id)
                    .await
                    .map_err(|e| AuthError::Database(e.to_string()))?;
                warn!(
                    user_id = %stored_token.user_id,
                    "Revoked all refresh tokens for user due to revoked token reuse attempt"
                );
            } else {
                warn!(
                    user_id = %stored_token.user_id,
                    "Revoked token reuse detected; not revoking other sessions (REVOKE_ALL_ON_REFRESH_TOKEN_REUSE=false)"
                );
            }
            return Err(AuthError::TokenRevoked);
        }

        // Check if token is expired
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

        // Revoke the old token (token rotation)
        self.token_repo
            .revoke(&stored_token.id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;
        debug!(
            user_id = %stored_token.user_id,
            refresh_token_id = %stored_token.id,
            token_hash_prefix = %token_hash_prefix,
            "Refresh token rotated (old token revoked)"
        );

        // Get user for roles
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

        // Generate new tokens
        let roles = user.get_roles();
        let access_token = self
            .jwt_service
            .generate_token(&user.id, roles.clone())
            .map_err(|e| AuthError::Internal(e.to_string()))?;

        let new_refresh_token = Self::generate_refresh_token();
        let new_refresh_token_hash = Self::hash_refresh_token(&new_refresh_token);
        let now = Utc::now();
        let refresh_expires_at =
            now + Duration::seconds(self.config.refresh_token_expiration_secs as i64);

        // Store new refresh token
        let token_model = RefreshTokenDbModel::new(
            &user.id,
            new_refresh_token_hash,
            refresh_expires_at,
            stored_token.device_info,
        );
        self.token_repo
            .create(&token_model)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

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

    /// Change a user's password.
    pub async fn change_password(
        &self,
        user_id: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        debug!(user_id = %user_id, "Password change attempt");
        // Get user
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or(AuthError::UserNotFound)?;

        // Verify current password
        if !Self::verify_password(current_password, &user.password_hash)? {
            warn!(user_id = %user_id, "Password change failed: incorrect current password");
            return Err(AuthError::IncorrectCurrentPassword);
        }

        // Prevent password reuse - new password must be different from current
        if current_password == new_password {
            return Err(AuthError::WeakPassword(
                "New password must be different from current password".to_string(),
            ));
        }

        // Validate new password strength
        self.validate_password_strength(new_password)?;

        // Hash new password
        let new_hash = Self::hash_password(new_password)?;

        // Update password and clear must_change_password flag
        self.user_repo
            .update_password(user_id, &new_hash, true)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        // Revoke all existing refresh tokens to invalidate all sessions
        // This ensures that if an attacker has a stolen token, they cannot
        // continue using it after the legitimate user changes their password
        self.token_repo
            .revoke_all_for_user(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        info!(
            user_id = %user_id,
            "Password changed; revoked all refresh tokens for user"
        );

        Ok(())
    }

    /// Logout by revoking a specific refresh token.
    pub async fn logout(&self, refresh_token: &str) -> Result<(), AuthError> {
        let token_hash = Self::hash_refresh_token(refresh_token);
        let token_hash_prefix = Self::token_hash_prefix(&token_hash);
        debug!(token_hash_prefix = %token_hash_prefix, "Logout request received");

        let stored_token = self
            .token_repo
            .find_by_token_hash(&token_hash)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?
            .ok_or_else(|| {
                warn!(token_hash_prefix = %token_hash_prefix, "Logout failed: refresh token not found");
                AuthError::InvalidToken
            })?;

        self.token_repo
            .revoke(&stored_token.id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

        info!(
            user_id = %stored_token.user_id,
            refresh_token_id = %stored_token.id,
            device_info = ?stored_token.device_info.as_deref(),
            "Logout successful (refresh token revoked)"
        );

        Ok(())
    }

    /// Logout from all sessions by revoking all refresh tokens for a user.
    pub async fn logout_all(&self, user_id: &str) -> Result<(), AuthError> {
        self.token_repo
            .revoke_all_for_user(user_id)
            .await
            .map_err(|e| AuthError::Database(e.to_string()))?;

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
                device_info: t.device_info,
                created_at: t.created_at,
                expires_at: t.expires_at,
            })
            .collect();

        Ok(sessions)
    }

    /// Get the authentication configuration.
    pub fn config(&self) -> &AuthConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::UserDbModel;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::Mutex;

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert_eq!(config.access_token_expiration_secs, 3600);
        assert_eq!(config.refresh_token_expiration_secs, 604800);
        assert_eq!(config.refresh_token_reuse_grace_secs, 0);
        assert!(!config.revoke_all_on_refresh_token_reuse);
        assert_eq!(config.min_password_length, 8);
    }

    #[test]
    fn test_hash_password() {
        let password = "testpassword123";
        let hash = AuthService::hash_password(password).expect("Hashing should succeed");

        // Hash should be a valid Argon2id hash
        assert!(hash.starts_with("$argon2id$"));
        // Hash should not equal the original password
        assert_ne!(hash, password);
    }

    #[test]
    fn test_verify_password_correct() {
        let password = "testpassword123";
        let hash = AuthService::hash_password(password).expect("Hashing should succeed");

        let result =
            AuthService::verify_password(password, &hash).expect("Verification should succeed");
        assert!(result);
    }

    #[test]
    fn test_verify_password_incorrect() {
        let password = "testpassword123";
        let wrong_password = "wrongpassword456";
        let hash = AuthService::hash_password(password).expect("Hashing should succeed");

        let result = AuthService::verify_password(wrong_password, &hash)
            .expect("Verification should succeed");
        assert!(!result);
    }

    #[test]
    fn test_generate_refresh_token() {
        let token1 = AuthService::generate_refresh_token();
        let token2 = AuthService::generate_refresh_token();

        // Tokens should be 64 hex characters (32 bytes = 256 bits)
        assert_eq!(token1.len(), 64);
        assert_eq!(token2.len(), 64);
        // Tokens should be unique
        assert_ne!(token1, token2);
    }

    #[test]
    fn test_hash_refresh_token() {
        let token = "test_refresh_token";
        let hash = AuthService::hash_refresh_token(token);

        // SHA-256 produces 64 hex characters
        assert_eq!(hash.len(), 64);
        // Hash should not equal the original token
        assert_ne!(hash, token);
        // Same input should produce same hash
        let hash2 = AuthService::hash_refresh_token(token);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_validate_password_strength_valid() {
        let config = AuthConfig::default();
        let service = create_test_service_minimal(config);

        assert!(service.validate_password_strength("password1").is_ok());
        assert!(service.validate_password_strength("MyP@ssw0rd").is_ok());
        assert!(service.validate_password_strength("12345678a").is_ok());
    }

    #[test]
    fn test_validate_password_strength_too_short() {
        let config = AuthConfig::default();
        let service = create_test_service_minimal(config);

        let result = service.validate_password_strength("pass1");
        assert!(matches!(result, Err(AuthError::WeakPassword(_))));
    }

    #[test]
    fn test_validate_password_strength_no_letter() {
        let config = AuthConfig::default();
        let service = create_test_service_minimal(config);

        let result = service.validate_password_strength("12345678");
        assert!(matches!(result, Err(AuthError::WeakPassword(_))));
    }

    #[test]
    fn test_validate_password_strength_no_number() {
        let config = AuthConfig::default();
        let service = create_test_service_minimal(config);

        let result = service.validate_password_strength("abcdefgh");
        assert!(matches!(result, Err(AuthError::WeakPassword(_))));
    }

    // Helper to create a minimal AuthService for testing password validation
    fn create_test_service_minimal(config: AuthConfig) -> AuthService {
        use std::sync::Arc;

        // Create mock repositories using a simple in-memory implementation
        let user_repo = Arc::new(MockUserRepository);
        let token_repo = Arc::new(MockRefreshTokenRepository);
        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        ));

        AuthService::new(user_repo, token_repo, jwt_service, config)
    }

    // Mock implementations for testing
    struct MockUserRepository;

    #[async_trait::async_trait]
    impl UserRepository for MockUserRepository {
        async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
            Ok(())
        }
        async fn find_by_id(&self, _id: &str) -> crate::Result<Option<UserDbModel>> {
            Ok(None)
        }
        async fn find_by_username(&self, _username: &str) -> crate::Result<Option<UserDbModel>> {
            Ok(None)
        }
        async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
            Ok(None)
        }
        async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
            Ok(())
        }
        async fn delete(&self, _id: &str) -> crate::Result<()> {
            Ok(())
        }
        async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
            Ok(vec![])
        }
        async fn update_last_login(
            &self,
            _id: &str,
            _time: chrono::DateTime<chrono::Utc>,
        ) -> crate::Result<()> {
            Ok(())
        }
        async fn update_password(&self, _id: &str, _hash: &str, _clear: bool) -> crate::Result<()> {
            Ok(())
        }
        async fn count(&self) -> crate::Result<i64> {
            Ok(0)
        }
    }

    struct MockRefreshTokenRepository;

    #[async_trait::async_trait]
    impl RefreshTokenRepository for MockRefreshTokenRepository {
        async fn create(&self, _token: &RefreshTokenDbModel) -> crate::Result<()> {
            Ok(())
        }
        async fn find_by_token_hash(
            &self,
            _hash: &str,
        ) -> crate::Result<Option<RefreshTokenDbModel>> {
            Ok(None)
        }
        async fn find_active_by_user(
            &self,
            _user_id: &str,
        ) -> crate::Result<Vec<RefreshTokenDbModel>> {
            Ok(vec![])
        }
        async fn revoke(&self, _id: &str) -> crate::Result<()> {
            Ok(())
        }
        async fn revoke_all_for_user(&self, _user_id: &str) -> crate::Result<()> {
            Ok(())
        }
        async fn cleanup_expired(&self) -> crate::Result<u64> {
            Ok(0)
        }
        async fn count_active_by_user(&self, _user_id: &str) -> crate::Result<i64> {
            Ok(0)
        }
    }

    struct SpyRefreshTokenRepository {
        token: Mutex<RefreshTokenDbModel>,
        revoke_all_called: AtomicBool,
        revoke_called: AtomicBool,
        create_called: AtomicBool,
    }

    impl SpyRefreshTokenRepository {
        fn new(token: RefreshTokenDbModel) -> Self {
            Self {
                token: Mutex::new(token),
                revoke_all_called: AtomicBool::new(false),
                revoke_called: AtomicBool::new(false),
                create_called: AtomicBool::new(false),
            }
        }
    }

    #[async_trait::async_trait]
    impl RefreshTokenRepository for SpyRefreshTokenRepository {
        async fn create(&self, _token: &RefreshTokenDbModel) -> crate::Result<()> {
            self.create_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn find_by_token_hash(
            &self,
            _hash: &str,
        ) -> crate::Result<Option<RefreshTokenDbModel>> {
            Ok(Some(self.token.lock().await.clone()))
        }

        async fn find_active_by_user(
            &self,
            _user_id: &str,
        ) -> crate::Result<Vec<RefreshTokenDbModel>> {
            Ok(vec![])
        }

        async fn revoke(&self, id: &str) -> crate::Result<()> {
            self.revoke_called.store(true, Ordering::SeqCst);
            let mut token = self.token.lock().await;
            if token.id == id {
                token.revoked_at = Some(Utc::now().to_rfc3339());
            }
            Ok(())
        }

        async fn revoke_all_for_user(&self, _user_id: &str) -> crate::Result<()> {
            self.revoke_all_called.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn cleanup_expired(&self) -> crate::Result<u64> {
            Ok(0)
        }

        async fn count_active_by_user(&self, _user_id: &str) -> crate::Result<i64> {
            Ok(0)
        }
    }

    struct SpyUserRepository {
        user: UserDbModel,
    }

    #[async_trait::async_trait]
    impl UserRepository for SpyUserRepository {
        async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
            Ok(())
        }
        async fn find_by_id(&self, id: &str) -> crate::Result<Option<UserDbModel>> {
            if self.user.id == id {
                Ok(Some(self.user.clone()))
            } else {
                Ok(None)
            }
        }
        async fn find_by_username(&self, _username: &str) -> crate::Result<Option<UserDbModel>> {
            Ok(None)
        }
        async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
            Ok(None)
        }
        async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
            Ok(())
        }
        async fn delete(&self, _id: &str) -> crate::Result<()> {
            Ok(())
        }
        async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
            Ok(vec![])
        }
        async fn update_last_login(
            &self,
            _id: &str,
            _time: chrono::DateTime<chrono::Utc>,
        ) -> crate::Result<()> {
            Ok(())
        }
        async fn update_password(&self, _id: &str, _hash: &str, _clear: bool) -> crate::Result<()> {
            Ok(())
        }
        async fn count(&self) -> crate::Result<i64> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn test_refresh_revoked_token_does_not_revoke_all_by_default() {
        let refresh_token = "abc123";
        let token_hash = AuthService::hash_refresh_token(refresh_token);

        let mut stored =
            RefreshTokenDbModel::new("user-1", token_hash, Utc::now() + Duration::days(7), None);
        stored.revoked_at = Some((Utc::now() - Duration::hours(2)).to_rfc3339());

        let token_repo = Arc::new(SpyRefreshTokenRepository::new(stored));
        let user_repo = Arc::new(MockUserRepository);
        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        ));

        let service = AuthService::new(
            user_repo,
            token_repo.clone(),
            jwt_service,
            AuthConfig::default(),
        );

        let result = service.refresh_tokens(refresh_token).await;
        assert!(matches!(result, Err(AuthError::TokenRevoked)));
        assert!(!token_repo.revoke_all_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_refresh_revoked_token_revokes_all_when_enabled() {
        let refresh_token = "abc123";
        let token_hash = AuthService::hash_refresh_token(refresh_token);

        let mut stored =
            RefreshTokenDbModel::new("user-1", token_hash, Utc::now() + Duration::days(7), None);
        stored.revoked_at = Some((Utc::now() - Duration::hours(2)).to_rfc3339());

        let token_repo = Arc::new(SpyRefreshTokenRepository::new(stored));
        let user_repo = Arc::new(MockUserRepository);
        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        ));

        let config = AuthConfig {
            revoke_all_on_refresh_token_reuse: true,
            ..Default::default()
        };

        let service = AuthService::new(user_repo, token_repo.clone(), jwt_service, config);

        let result = service.refresh_tokens(refresh_token).await;
        assert!(matches!(result, Err(AuthError::TokenRevoked)));
        assert!(token_repo.revoke_all_called.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_refresh_recently_revoked_token_succeeds_with_grace_window() {
        let refresh_token = "abc123";
        let token_hash = AuthService::hash_refresh_token(refresh_token);

        let mut stored = RefreshTokenDbModel::new(
            "user-1",
            token_hash,
            Utc::now() + Duration::days(7),
            Some("test-device".to_string()),
        );
        stored.revoked_at = Some((Utc::now() - Duration::seconds(1)).to_rfc3339());

        let token_repo = Arc::new(SpyRefreshTokenRepository::new(stored));

        let mut user = UserDbModel::new("u", "hash", vec!["admin".to_string()]);
        user.id = "user-1".to_string();
        user.must_change_password = false;
        let user_repo = Arc::new(SpyUserRepository { user });

        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        ));

        let config = AuthConfig {
            refresh_token_reuse_grace_secs: 10,
            ..Default::default()
        };

        let service = AuthService::new(user_repo, token_repo.clone(), jwt_service, config);

        let result = service.refresh_tokens(refresh_token).await;
        assert!(result.is_ok());
        assert!(token_repo.revoke_called.load(Ordering::SeqCst));
        assert!(token_repo.create_called.load(Ordering::SeqCst));
        assert!(!token_repo.revoke_all_called.load(Ordering::SeqCst));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // Property 1: Password hashing preserves security
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_password_hashing_preserves_security(
            password in "[a-zA-Z0-9!@#$%^&*]{8,64}"
        ) {
            let hash = AuthService::hash_password(&password)
                .expect("Hashing should succeed");

            // Property: Hash should be a valid Argon2id hash
            prop_assert!(hash.starts_with("$argon2id$"), "Hash should be Argon2id format");

            // Property: Hash should not equal the original password
            prop_assert_ne!(&hash, &password, "Hash should not equal password");

            // Property: Hash should be deterministically verifiable
            let verified = AuthService::verify_password(&password, &hash)
                .expect("Verification should succeed");
            prop_assert!(verified, "Correct password should verify");
        }
    }

    // Property 7: Password verification correctness
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_password_verification_correctness(
            password in "[a-zA-Z0-9!@#$%^&*]{8,32}",
            wrong_password in "[a-zA-Z0-9!@#$%^&*]{8,32}"
        ) {
            let hash = AuthService::hash_password(&password)
                .expect("Hashing should succeed");

            // Property: Correct password should verify
            let correct_result = AuthService::verify_password(&password, &hash)
                .expect("Verification should succeed");
            prop_assert!(correct_result, "Correct password should verify");

            // Property: Wrong password should not verify (if different)
            if password != wrong_password {
                let wrong_result = AuthService::verify_password(&wrong_password, &hash)
                    .expect("Verification should succeed");
                prop_assert!(!wrong_result, "Wrong password should not verify");
            }
        }
    }

    // Property 9: Refresh token hashing
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_refresh_token_hashing(
            token in "[a-zA-Z0-9]{32,128}"
        ) {
            let hash = AuthService::hash_refresh_token(&token);

            // Property: Hash should be SHA-256 (64 hex characters)
            prop_assert_eq!(hash.len(), 64, "SHA-256 hash should be 64 hex chars");

            // Property: Hash should not equal the original token
            prop_assert_ne!(&hash, &token, "Hash should not equal token");

            // Property: Same input should produce same hash (deterministic)
            let hash2 = AuthService::hash_refresh_token(&token);
            prop_assert_eq!(&hash, &hash2, "Hashing should be deterministic");
        }
    }

    // Property 13: Refresh token entropy
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_refresh_token_entropy(_seed in 0u64..1000000u64) {
            let token1 = AuthService::generate_refresh_token();
            let token2 = AuthService::generate_refresh_token();

            // Property: Tokens should be 64 hex characters (256 bits)
            prop_assert_eq!(token1.len(), 64, "Token should be 64 hex chars (256 bits)");
            prop_assert_eq!(token2.len(), 64, "Token should be 64 hex chars (256 bits)");

            // Property: Tokens should be unique (with overwhelming probability)
            prop_assert_ne!(&token1, &token2, "Generated tokens should be unique");

            // Property: Tokens should be valid hex
            prop_assert!(
                token1.chars().all(|c| c.is_ascii_hexdigit()),
                "Token should be valid hex"
            );
        }
    }

    // Property 25: Password validation rules
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10))]

        #[test]
        fn prop_password_validation_rules(
            // Valid passwords: 8+ chars with letter and number
            valid_password in "[a-zA-Z]{4,8}[0-9]{4,8}",
            // Too short passwords
            short_password in "[a-zA-Z0-9]{1,7}",
            // No letter passwords
            no_letter in "[0-9]{8,16}",
            // No number passwords
            no_number in "[a-zA-Z]{8,16}"
        ) {
            let config = AuthConfig::default();
            let service = create_test_service_for_props(config);

            // Property: Valid passwords should be accepted
            prop_assert!(
                service.validate_password_strength(&valid_password).is_ok(),
                "Valid password should be accepted: {}", valid_password
            );

            // Property: Too short passwords should be rejected
            prop_assert!(
                service.validate_password_strength(&short_password).is_err(),
                "Short password should be rejected: {}", short_password
            );

            // Property: Passwords without letters should be rejected
            prop_assert!(
                service.validate_password_strength(&no_letter).is_err(),
                "Password without letter should be rejected: {}", no_letter
            );

            // Property: Passwords without numbers should be rejected
            prop_assert!(
                service.validate_password_strength(&no_number).is_err(),
                "Password without number should be rejected: {}", no_number
            );
        }
    }

    // Helper to create AuthService for property tests
    fn create_test_service_for_props(config: AuthConfig) -> AuthService {
        use crate::database::models::UserDbModel;
        use std::sync::Arc;

        struct MockUserRepo;

        #[async_trait::async_trait]
        impl UserRepository for MockUserRepo {
            async fn create(&self, _user: &UserDbModel) -> crate::Result<()> {
                Ok(())
            }
            async fn find_by_id(&self, _id: &str) -> crate::Result<Option<UserDbModel>> {
                Ok(None)
            }
            async fn find_by_username(
                &self,
                _username: &str,
            ) -> crate::Result<Option<UserDbModel>> {
                Ok(None)
            }
            async fn find_by_email(&self, _email: &str) -> crate::Result<Option<UserDbModel>> {
                Ok(None)
            }
            async fn update(&self, _user: &UserDbModel) -> crate::Result<()> {
                Ok(())
            }
            async fn delete(&self, _id: &str) -> crate::Result<()> {
                Ok(())
            }
            async fn list(&self, _limit: i64, _offset: i64) -> crate::Result<Vec<UserDbModel>> {
                Ok(vec![])
            }
            async fn update_last_login(
                &self,
                _id: &str,
                _time: chrono::DateTime<chrono::Utc>,
            ) -> crate::Result<()> {
                Ok(())
            }
            async fn update_password(
                &self,
                _id: &str,
                _hash: &str,
                _clear: bool,
            ) -> crate::Result<()> {
                Ok(())
            }
            async fn count(&self) -> crate::Result<i64> {
                Ok(0)
            }
        }

        struct MockTokenRepo;

        #[async_trait::async_trait]
        impl RefreshTokenRepository for MockTokenRepo {
            async fn create(&self, _token: &RefreshTokenDbModel) -> crate::Result<()> {
                Ok(())
            }
            async fn find_by_token_hash(
                &self,
                _hash: &str,
            ) -> crate::Result<Option<RefreshTokenDbModel>> {
                Ok(None)
            }
            async fn find_active_by_user(
                &self,
                _user_id: &str,
            ) -> crate::Result<Vec<RefreshTokenDbModel>> {
                Ok(vec![])
            }
            async fn revoke(&self, _id: &str) -> crate::Result<()> {
                Ok(())
            }
            async fn revoke_all_for_user(&self, _user_id: &str) -> crate::Result<()> {
                Ok(())
            }
            async fn cleanup_expired(&self) -> crate::Result<u64> {
                Ok(0)
            }
            async fn count_active_by_user(&self, _user_id: &str) -> crate::Result<i64> {
                Ok(0)
            }
        }

        let user_repo = Arc::new(MockUserRepo);
        let token_repo = Arc::new(MockTokenRepo);
        let jwt_service = Arc::new(JwtService::new(
            "test-secret-key-32-chars-long!!",
            "test-issuer",
            "test-audience",
            Some(900),
        ));

        AuthService::new(user_repo, token_repo, jwt_service, config)
    }
}
