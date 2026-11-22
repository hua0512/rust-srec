//! Configuration, principal, error, and response types for authentication.

use tracing::warn;

use crate::api::jwt::Claims;
use crate::database::models::ApiKeyAccessLevel;

/// Prefix of every raw API key. The auth middleware routes `Bearer` tokens
/// starting with this prefix to `AuthService::authorize_api_key` instead of
/// JWT validation, so it must never be a valid JWT prefix.
pub const API_KEY_PREFIX: &str = "srec_";

pub(super) fn session_expiration(access_seconds: u64, refresh_ms: i64) -> Result<i64, AuthError> {
    let access_ms = access_seconds
        .checked_mul(1000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| {
            AuthError::Internal("Access token expiration exceeds supported range".to_owned())
        })?;
    Ok(access_ms.max(refresh_ms))
}

// A valid Argon2id PHC with the same work parameters as newly stored passwords.
// Its digest is a fixed dummy value: even a matching result never authenticates a missing user.
pub(super) const LOGIN_DUMMY_PASSWORD_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c3JlYy1sb2dpbi1kdW1teQ$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// Maximum stored/logged device-description length; longer user agents are truncated.
pub const MAX_DEVICE_INFO_LENGTH: usize = 256;

pub(super) fn bounded_device_info(value: Option<String>) -> Option<String> {
    value.map(|value| value.chars().take(MAX_DEVICE_INFO_LENGTH).collect())
}

/// Longest username `authenticate` will look up, counted in `char`s so the
/// bound is the same for ASCII and CJK names.
pub const MAX_USERNAME_LENGTH: usize = 128;

/// Which credential authenticated the request.
///
/// API key management endpoints (`routes::auth::api_keys`) only accept
/// `Jwt` so a leaked API key cannot mint or revoke keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialKind {
    /// JWT access token from an interactive login session.
    Jwt,
    /// Long-lived API key (`api_keys` table).
    ApiKey,
}

/// Authenticated request principal inserted into request extensions by the
/// auth middleware, alongside the plain `Claims` that existing handlers read.
#[derive(Debug, Clone)]
pub struct AuthPrincipal {
    pub claims: Claims,
    pub credential: CredentialKind,
    /// `Full` for JWT sessions; the key's stored level for API keys. The
    /// middleware enforces this for REST routes and `mcp` tools enforce it
    /// per tool.
    pub access: ApiKeyAccessLevel,
    /// Stable key identity for bounded revalidation without retaining the raw secret.
    pub api_key_id: Option<String>,
}

/// Authentication configuration.
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Access token expiration in seconds (default: 3600 = 1 hour)
    pub access_token_expiration_secs: u64,
    /// Refresh token expiration in seconds (default: 604800 = 7 days)
    pub refresh_token_expiration_secs: u64,
    /// Grace window that suppresses session revocation on recent token replay (default: 0).
    pub refresh_token_reuse_grace_secs: u64,
    /// Whether to revoke all refresh tokens for a user when a revoked token is presented.
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
            revoke_all_on_refresh_token_reuse: true,
            min_password_length: 8,
        }
    }
}

impl AuthConfig {
    /// Create AuthConfig from environment variables.
    pub fn from_env() -> crate::Result<Self> {
        let access_token_expiration_secs =
            crate::utils::env::parse("ACCESS_TOKEN_EXPIRATION_SECS")?.unwrap_or(3600);

        let refresh_token_expiration_secs =
            crate::utils::env::parse("REFRESH_TOKEN_EXPIRATION_SECS")?.unwrap_or(604800);

        let refresh_token_reuse_grace_secs =
            crate::utils::env::parse("REFRESH_TOKEN_REUSE_GRACE_SECS")?.unwrap_or(0);

        let revoke_all_on_refresh_token_reuse = Self::parse_reuse_revocation(
            std::env::var("REVOKE_ALL_ON_REFRESH_TOKEN_REUSE")
                .ok()
                .as_deref(),
        );

        let min_password_length = crate::utils::env::parse("MIN_PASSWORD_LENGTH")?.unwrap_or(8);

        Ok(Self {
            access_token_expiration_secs,
            refresh_token_expiration_secs,
            refresh_token_reuse_grace_secs,
            revoke_all_on_refresh_token_reuse,
            min_password_length,
        })
    }

    fn parse_reuse_revocation(value: Option<&str>) -> bool {
        value
            .map(|v| match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    warn!(
                        value = %v.trim(),
                        "Unrecognized REVOKE_ALL_ON_REFRESH_TOKEN_REUSE value; treating as true"
                    );
                    true
                }
            })
            .unwrap_or(true)
    }
}

/// Authentication errors.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Account is disabled")]
    AccountDisabled,

    #[error("Password change is required")]
    PasswordChangeRequired,

    #[error("Token has expired")]
    TokenExpired,

    #[error("Token has been revoked")]
    TokenRevoked,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Password does not meet requirements: {0}")]
    WeakPassword(String),

    #[error("Current password is incorrect")]
    IncorrectCurrentPassword,

    #[error("User not found")]
    UserNotFound,

    #[error("Too many failed login attempts")]
    TooManyAttempts { retry_after_secs: u64 },

    #[error("Username must be at most {max} characters")]
    UsernameTooLong { max: usize },

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
    pub created_at: i64,
    /// When the session expires
    pub expires_at: i64,
}
