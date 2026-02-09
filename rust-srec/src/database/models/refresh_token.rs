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

    /// Check if the token is valid (not expired and not revoked).
    pub fn is_valid(&self) -> bool {
        !self.is_expired() && !self.is_revoked()
    }

    /// Get expires_at as DateTime<Utc>.
    pub fn get_expires_at(&self) -> DateTime<Utc> {
        crate::database::time::ms_to_datetime(self.expires_at)
    }

    /// Get created_at as DateTime<Utc>.
    pub fn get_created_at(&self) -> DateTime<Utc> {
        crate::database::time::ms_to_datetime(self.created_at)
    }

    /// Get revoked_at as DateTime<Utc>.
    pub fn get_revoked_at(&self) -> Option<DateTime<Utc>> {
        self.revoked_at.map(crate::database::time::ms_to_datetime)
    }

    /// Revoke the token.
    pub fn revoke(&mut self) {
        self.revoked_at = Some(crate::database::time::now_ms());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_refresh_token_new() {
        let expires = Utc::now() + Duration::days(7);
        let token = RefreshTokenDbModel::new(
            "user-123",
            "hashed_token_value",
            expires,
            Some("Chrome on Windows".to_string()),
        );

        assert_eq!(token.user_id, "user-123");
        assert_eq!(token.token_hash, "hashed_token_value");
        assert!(token.revoked_at.is_none());
        assert_eq!(token.device_info, Some("Chrome on Windows".to_string()));
    }

    #[test]
    fn test_refresh_token_is_valid() {
        let expires = Utc::now() + Duration::days(7);
        let token = RefreshTokenDbModel::new("user-123", "hash", expires, None);

        assert!(token.is_valid());
        assert!(!token.is_expired());
        assert!(!token.is_revoked());
    }

    #[test]
    fn test_refresh_token_expired() {
        let expires = Utc::now() - Duration::hours(1); // Already expired
        let token = RefreshTokenDbModel::new("user-123", "hash", expires, None);

        assert!(!token.is_valid());
        assert!(token.is_expired());
    }

    #[test]
    fn test_refresh_token_revoked() {
        let expires = Utc::now() + Duration::days(7);
        let mut token = RefreshTokenDbModel::new("user-123", "hash", expires, None);

        assert!(token.is_valid());

        token.revoke();

        assert!(!token.is_valid());
        assert!(token.is_revoked());
        assert!(token.revoked_at.is_some());
    }
}

#[cfg(test)]
mod revocation_tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_token_valid_before_revocation_short_expiry() {
        let user_id = "user12345678";
        let token_hash = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let expires = Utc::now() + Duration::days(1);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        assert!(token.is_valid(), "Token should be valid before revocation");
        assert!(
            token.revoked_at.is_none(),
            "revoked_at should be None before revocation"
        );
    }

    #[test]
    fn test_token_valid_before_revocation_long_expiry() {
        let user_id = "abc-def-123-456-789";
        let token_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let expires = Utc::now() + Duration::days(29);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        assert!(token.is_valid(), "Token should be valid before revocation");
        assert!(
            token.revoked_at.is_none(),
            "revoked_at should be None before revocation"
        );
    }

    #[test]
    fn test_token_revoked_after_revocation() {
        let user_id = "testuser";
        let token_hash = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
        let expires = Utc::now() + Duration::days(7);
        let mut token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        token.revoke();

        assert!(
            token.is_revoked(),
            "Token should be revoked after revocation"
        );
        assert!(
            !token.is_valid(),
            "Token should not be valid after revocation"
        );
    }

    #[test]
    fn test_revoked_at_has_timestamp() {
        let user_id = "user-abc-123";
        let token_hash = "1111111111111111222222222222222233333333333333334444444444444444";
        let expires = Utc::now() + Duration::days(15);
        let mut token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        token.revoke();

        assert!(
            token.revoked_at.is_some(),
            "revoked_at should have timestamp"
        );
        let revoked_time = token.get_revoked_at();
        assert!(revoked_time.is_some(), "revoked_at should be parseable");
    }

    #[test]
    fn test_revoked_at_parseable_as_datetime() {
        let user_id = "longuser123456789012345678901234";
        let token_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let expires = Utc::now() + Duration::days(20);
        let mut token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        token.revoke();

        let revoked_time = token.get_revoked_at();
        assert!(
            revoked_time.is_some(),
            "revoked_at should be parseable as DateTime"
        );
    }
}

#[cfg(test)]
mod token_state_tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_valid_token_not_expired_not_revoked() {
        let user_id = "validuser";
        let token_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let valid_expires = Utc::now() + Duration::days(7);
        let valid_token = RefreshTokenDbModel::new(user_id, token_hash, valid_expires, None);

        assert!(
            valid_token.is_valid(),
            "Non-expired, non-revoked token should be valid"
        );
    }

    #[test]
    fn test_expired_token_not_valid() {
        let user_id = "expireduser";
        let token_hash = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let expired_expires = Utc::now() - Duration::hours(1);
        let expired_token = RefreshTokenDbModel::new(user_id, token_hash, expired_expires, None);

        assert!(
            !expired_token.is_valid(),
            "Expired token should not be valid"
        );
        assert!(
            expired_token.is_expired(),
            "Expired token should report as expired"
        );
    }

    #[test]
    fn test_revoked_token_not_valid() {
        let user_id = "revokeduser";
        let token_hash = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let revoked_expires = Utc::now() + Duration::days(7);
        let mut revoked_token =
            RefreshTokenDbModel::new(user_id, token_hash, revoked_expires, None);
        revoked_token.revoke();

        assert!(
            !revoked_token.is_valid(),
            "Revoked token should not be valid"
        );
        assert!(
            revoked_token.is_revoked(),
            "Revoked token should report as revoked"
        );
    }

    #[test]
    fn test_expired_and_revoked_token_not_valid() {
        let user_id = "doublebadusr";
        let token_hash = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let expired_expires = Utc::now() - Duration::hours(2);
        let mut token = RefreshTokenDbModel::new(user_id, token_hash, expired_expires, None);
        token.revoke();

        assert!(
            !token.is_valid(),
            "Expired and revoked token should not be valid"
        );
        assert!(token.is_expired(), "Token should report as expired");
        assert!(token.is_revoked(), "Token should report as revoked");
    }
}

#[cfg(test)]
mod validation_logic_tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_is_valid_logic_without_device_info() {
        let user_id = "logicuser";
        let token_hash = "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff0";
        let expires = Utc::now() + Duration::days(7);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, None);

        let expected_valid = !token.is_expired() && !token.is_revoked();
        assert_eq!(
            token.is_valid(),
            expected_valid,
            "is_valid should equal (!is_expired && !is_revoked)"
        );
    }

    #[test]
    fn test_is_valid_logic_with_device_info() {
        let user_id = "deviceuser";
        let token_hash = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let device_info = Some("Chrome 120 on Linux".to_string());
        let expires = Utc::now() + Duration::days(14);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, device_info.clone());

        let expected_valid = !token.is_expired() && !token.is_revoked();
        assert_eq!(
            token.is_valid(),
            expected_valid,
            "is_valid should equal (!is_expired && !is_revoked)"
        );
    }

    #[test]
    fn test_token_fields_preserved_minimal() {
        let user_id = "minimalusr";
        let token_hash = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let device_info = None;
        let expires = Utc::now() + Duration::days(7);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, device_info.clone());

        assert_eq!(&token.user_id, user_id, "user_id should be preserved");
        assert_eq!(
            &token.token_hash, token_hash,
            "token_hash should be preserved"
        );
        assert_eq!(
            &token.device_info, &device_info,
            "device_info should be preserved"
        );
    }

    #[test]
    fn test_token_fields_preserved_with_device() {
        let user_id = "fulluser123";
        let token_hash = "9876543210fedcba9876543210fedcba9876543210fedcba9876543210fedcba";
        let device_info = Some("Firefox 115".to_string());
        let expires = Utc::now() + Duration::days(10);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, device_info.clone());

        assert_eq!(&token.user_id, user_id, "user_id should be preserved");
        assert_eq!(
            &token.token_hash, token_hash,
            "token_hash should be preserved"
        );
        assert_eq!(
            &token.device_info, &device_info,
            "device_info should be preserved"
        );
    }

    #[test]
    fn test_token_fields_preserved_empty_device() {
        let user_id = "emptydevice";
        let token_hash = "0000000000000000111111111111111122222222222222223333333333333333";
        let device_info = Some("".to_string());
        let expires = Utc::now() + Duration::days(5);
        let token = RefreshTokenDbModel::new(user_id, token_hash, expires, device_info.clone());

        assert_eq!(&token.user_id, user_id, "user_id should be preserved");
        assert_eq!(
            &token.token_hash, token_hash,
            "token_hash should be preserved"
        );
        assert_eq!(
            &token.device_info, &device_info,
            "device_info should be preserved"
        );
    }
}
