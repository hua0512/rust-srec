//! Refresh token database model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

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
        expires_at: DateTime<Utc>,
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

#[derive(Debug, Clone, FromRow)]
pub struct AuthSessionDbModel {
    pub id: String,
    pub user_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}
