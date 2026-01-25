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
mod property_tests {
    use super::*;
    use chrono::Duration;
    use proptest::prelude::*;

    // Property 17: Logout revokes token with timestamp
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_logout_revokes_token_with_timestamp(
            user_id in "[a-zA-Z0-9-]{8,36}",
            token_hash in "[a-f0-9]{64}",
            days_until_expiry in 1i64..30i64
        ) {
            let expires = Utc::now() + Duration::days(days_until_expiry);
            let mut token = RefreshTokenDbModel::new(&user_id, &token_hash, expires, None);

            // Property: Token should be valid before revocation
            prop_assert!(token.is_valid(), "Token should be valid before revocation");
            prop_assert!(token.revoked_at.is_none(), "revoked_at should be None before revocation");

            // Revoke the token
            token.revoke();

            // Property: Token should be revoked after revocation
            prop_assert!(token.is_revoked(), "Token should be revoked after revocation");
            prop_assert!(!token.is_valid(), "Token should not be valid after revocation");

            // Property: revoked_at should have a timestamp
            prop_assert!(token.revoked_at.is_some(), "revoked_at should have timestamp");

            // Property: revoked_at should be parseable as DateTime
            let revoked_time = token.get_revoked_at();
            prop_assert!(revoked_time.is_some(), "revoked_at should be parseable");
        }
    }

    // Property 19: Active sessions exclude revoked and expired
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_active_sessions_exclude_revoked_and_expired(
            user_id in "[a-zA-Z0-9-]{8,36}",
            token_hash in "[a-f0-9]{64}"
        ) {
            // Test 1: Valid token (not expired, not revoked)
            let valid_expires = Utc::now() + Duration::days(7);
            let valid_token = RefreshTokenDbModel::new(&user_id, &token_hash, valid_expires, None);
            prop_assert!(valid_token.is_valid(), "Non-expired, non-revoked token should be valid");

            // Test 2: Expired token
            let expired_expires = Utc::now() - Duration::hours(1);
            let expired_token = RefreshTokenDbModel::new(&user_id, &token_hash, expired_expires, None);
            prop_assert!(!expired_token.is_valid(), "Expired token should not be valid");
            prop_assert!(expired_token.is_expired(), "Expired token should report as expired");

            // Test 3: Revoked token
            let revoked_expires = Utc::now() + Duration::days(7);
            let mut revoked_token = RefreshTokenDbModel::new(&user_id, &token_hash, revoked_expires, None);
            revoked_token.revoke();
            prop_assert!(!revoked_token.is_valid(), "Revoked token should not be valid");
            prop_assert!(revoked_token.is_revoked(), "Revoked token should report as revoked");
        }
    }

    // Property 15: Token validation checks all conditions
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_token_validation_checks_all_conditions(
            user_id in "[a-zA-Z0-9-]{8,36}",
            token_hash in "[a-f0-9]{64}",
            device_info in prop::option::of("[a-zA-Z0-9 ]{0,50}")
        ) {
            let expires = Utc::now() + Duration::days(7);
            let token = RefreshTokenDbModel::new(&user_id, &token_hash, expires, device_info.clone());

            // Property: is_valid should be equivalent to (!is_expired && !is_revoked)
            let expected_valid = !token.is_expired() && !token.is_revoked();
            prop_assert_eq!(
                token.is_valid(),
                expected_valid,
                "is_valid should equal (!is_expired && !is_revoked)"
            );

            // Property: Token fields should be preserved
            prop_assert_eq!(&token.user_id, &user_id, "user_id should be preserved");
            prop_assert_eq!(&token.token_hash, &token_hash, "token_hash should be preserved");
            prop_assert_eq!(&token.device_info, &device_info, "device_info should be preserved");
        }
    }
}
