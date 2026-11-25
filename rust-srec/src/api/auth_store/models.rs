//! Authentication database row models.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::utils::json::{self, JsonContext};

/// User database model.
/// Represents a user account for authentication.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct UserDbModel {
    /// Unique identifier (UUID)
    pub id: String,
    /// Unique username for login
    pub username: String,
    /// Argon2id password hash
    pub password_hash: String,
    /// Optional email address (unique if provided)
    pub email: Option<String>,
    /// JSON array of roles (e.g., ["admin", "user"])
    pub roles: String,
    /// Whether the user account is active
    pub is_active: bool,
    /// Whether the user must change password on next login
    pub must_change_password: bool,
    /// Unix epoch milliseconds (UTC) of last successful login.
    pub last_login_at: Option<i64>,
    /// Unix epoch milliseconds (UTC) when the user was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the user was last updated.
    pub updated_at: i64,
}

impl UserDbModel {
    /// Create a new user with default values.
    /// Note: Password should be hashed before calling this.
    pub fn new(
        username: impl Into<String>,
        password_hash: impl Into<String>,
        roles: Vec<String>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id: id.clone(),
            username: username.into(),
            password_hash: password_hash.into(),
            email: None,
            roles: json::to_string_or_fallback(
                &roles,
                r#"["user"]"#,
                JsonContext::UserField {
                    user_id: &id,
                    field: "roles",
                },
                "Failed to serialize user roles; storing default",
            ),
            is_active: true,
            must_change_password: true,
            last_login_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Get the roles as a `Vec<String>`.
    pub fn get_roles(&self) -> Vec<String> {
        json::parse_or_default(
            &self.roles,
            JsonContext::UserField {
                user_id: &self.id,
                field: "roles",
            },
            "Invalid user roles JSON; treating as empty",
        )
    }
}

/// Access level granted to an API key.
///
/// Enforced by the auth middleware for REST requests (read-only keys may only
/// access approved non-sensitive REST queries) and by MCP's per-tool
/// full-access checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyAccessLevel {
    /// Key may only perform read operations.
    ReadOnly,
    /// Key acts with the owning user's full permissions.
    Full,
}

impl ApiKeyAccessLevel {
    /// Stable string form stored in `api_keys.access_level`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiKeyAccessLevel::ReadOnly => "read_only",
            ApiKeyAccessLevel::Full => "full",
        }
    }

    /// Parse the stored string form; unknown values fail closed to `None`.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read_only" => Some(ApiKeyAccessLevel::ReadOnly),
            "full" => Some(ApiKeyAccessLevel::Full),
            _ => None,
        }
    }
}

/// API key database model.
///
/// Only the SHA-256 hash of the raw key is persisted (`key_hash`); the raw
/// key is returned once by `AuthService::create_api_key` and never stored.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ApiKeyDbModel {
    /// Unique identifier (UUID)
    pub id: String,
    /// Foreign key to the user who owns this key
    pub user_id: String,
    /// Human-readable label chosen by the user
    pub name: String,
    /// SHA-256 hash (hex) of the raw key value
    pub key_hash: String,
    /// First characters of the raw key, kept for display purposes
    pub key_prefix: String,
    /// Stored form of `ApiKeyAccessLevel` ('read_only' | 'full')
    pub access_level: String,
    /// Unix epoch milliseconds (UTC) when the key expires (None = never).
    pub expires_at: Option<i64>,
    /// Unix epoch milliseconds (UTC) of the last authorized use (throttled).
    pub last_used_at: Option<i64>,
    /// Unix epoch milliseconds (UTC) when the key was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the key was revoked (None if still valid).
    pub revoked_at: Option<i64>,
}

impl ApiKeyDbModel {
    /// Create a new API key record.
    /// Note: `key_hash` must be the SHA-256 hex digest of the raw key.
    pub fn new(
        user_id: impl Into<String>,
        name: impl Into<String>,
        key_hash: impl Into<String>,
        key_prefix: impl Into<String>,
        access_level: ApiKeyAccessLevel,
        expires_at: Option<i64>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.into(),
            name: name.into(),
            key_hash: key_hash.into(),
            key_prefix: key_prefix.into(),
            access_level: access_level.as_str().to_string(),
            expires_at,
            last_used_at: None,
            created_at: now,
            revoked_at: None,
        }
    }

    /// Parsed access level; unrecognized stored values fail closed to `ReadOnly`.
    pub fn get_access_level(&self) -> ApiKeyAccessLevel {
        ApiKeyAccessLevel::parse(&self.access_level).unwrap_or(ApiKeyAccessLevel::ReadOnly)
    }

    /// Check if the key is revoked.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

/// Refresh token database model.
/// Represents a refresh token for JWT authentication with token rotation.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct RefreshTokenDbModel {
    /// Unique identifier (UUID)
    pub id: String,
    /// Foreign key to the user who owns this token
    pub user_id: String,
    /// SHA-256 hash of the token value (never store raw token)
    pub token_hash: String,
    /// Unix epoch milliseconds (UTC) when the token expires.
    pub expires_at: i64,
    /// Unix epoch milliseconds (UTC) when the token was created.
    pub created_at: i64,
    /// Unix epoch milliseconds (UTC) when the token was revoked (None if still valid).
    pub revoked_at: Option<i64>,
    /// Optional device/client information for audit purposes
    pub device_info: Option<String>,
    /// Stable login-session lineage, retained across refresh rotation.
    #[serde(default)]
    pub session_id: Option<String>,
}

impl RefreshTokenDbModel {
    /// Create a new refresh token.
    /// Note: token_hash should be the SHA-256 hash of the actual token.
    pub fn new(
        user_id: impl Into<String>,
        token_hash: impl Into<String>,
        expires_at: Timestamp,
        device_info: Option<String>,
    ) -> Self {
        let now = crate::database::time::now_ms();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.into(),
            token_hash: token_hash.into(),
            expires_at: crate::database::time::datetime_to_ms(expires_at),
            created_at: now,
            revoked_at: None,
            device_info,
            session_id: None,
        }
    }

    /// Check if the token is expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at < crate::database::time::now_ms()
    }

    /// Check if the token is revoked.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}

/// Persisted login-session lineage backing JWT access tokens and refresh rotation.
#[derive(Debug, Clone, FromRow)]
pub struct AuthSessionDbModel {
    pub id: String,
    pub user_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_level_round_trips_and_fails_closed() {
        assert_eq!(
            ApiKeyAccessLevel::parse("read_only"),
            Some(ApiKeyAccessLevel::ReadOnly)
        );
        assert_eq!(
            ApiKeyAccessLevel::parse("full"),
            Some(ApiKeyAccessLevel::Full)
        );
        assert_eq!(ApiKeyAccessLevel::parse("admin"), None);

        let mut key = ApiKeyDbModel::new("u", "n", "h", "p", ApiKeyAccessLevel::Full, None);
        key.access_level = "bogus".to_string();
        assert_eq!(key.get_access_level(), ApiKeyAccessLevel::ReadOnly);
    }
}
