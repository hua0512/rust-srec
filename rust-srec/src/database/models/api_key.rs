//! API key database model.

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Access level granted to an API key.
///
/// Enforced by the auth middleware for REST requests (read-only keys may only
/// issue GET/HEAD/OPTIONS) and by `mcp::require_write` for MCP tool calls.
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

    /// Check if the key is expired.
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .is_some_and(|at| at < crate::database::time::now_ms())
    }

    /// Check if the key is revoked.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Check if the key is valid (not expired and not revoked).
    pub fn is_valid(&self) -> bool {
        !self.is_expired() && !self.is_revoked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_key_is_valid_and_preserves_fields() {
        let key = ApiKeyDbModel::new(
            "user-123",
            "my ai key",
            "hashed",
            "srec_ab12cd34",
            ApiKeyAccessLevel::ReadOnly,
            None,
        );

        assert_eq!(key.user_id, "user-123");
        assert_eq!(key.name, "my ai key");
        assert_eq!(key.key_hash, "hashed");
        assert_eq!(key.key_prefix, "srec_ab12cd34");
        assert_eq!(key.get_access_level(), ApiKeyAccessLevel::ReadOnly);
        assert!(key.expires_at.is_none());
        assert!(key.is_valid());
    }

    #[test]
    fn expired_key_is_invalid() {
        let past = crate::database::time::now_ms() - 1000;
        let key = ApiKeyDbModel::new(
            "user-123",
            "old",
            "hash",
            "srec_old",
            ApiKeyAccessLevel::Full,
            Some(past),
        );

        assert!(key.is_expired());
        assert!(!key.is_valid());
    }

    #[test]
    fn revoked_key_is_invalid() {
        let mut key = ApiKeyDbModel::new(
            "user-123",
            "gone",
            "hash",
            "srec_gone",
            ApiKeyAccessLevel::Full,
            None,
        );
        key.revoked_at = Some(crate::database::time::now_ms());

        assert!(key.is_revoked());
        assert!(!key.is_valid());
    }

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
