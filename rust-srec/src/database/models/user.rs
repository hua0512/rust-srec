//! User database model.

use chrono::{DateTime, Utc};
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
    /// Timestamp of last successful login
    pub last_login_at: Option<String>,
    /// Timestamp when the user was created
    pub created_at: String,
    /// Timestamp when the user was last updated
    pub updated_at: String,
}

impl UserDbModel {
    /// Create a new user with default values.
    /// Note: Password should be hashed before calling this.
    pub fn new(
        username: impl Into<String>,
        password_hash: impl Into<String>,
        roles: Vec<String>,
    ) -> Self {
        let now = Utc::now().to_rfc3339();
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
            created_at: now.clone(),
            updated_at: now,
        }
    }

    /// Get the roles as a Vec<String>.
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

    /// Set the roles from a Vec<String>.
    pub fn set_roles(&mut self, roles: Vec<String>) {
        self.roles = json::to_string_or_fallback(
            &roles,
            r#"["user"]"#,
            JsonContext::UserField {
                user_id: &self.id,
                field: "roles",
            },
            "Failed to serialize user roles; storing default",
        );
    }

    /// Check if the user has a specific role.
    pub fn has_role(&self, role: &str) -> bool {
        self.get_roles().iter().any(|r| r == role)
    }

    /// Check if the user is an admin.
    pub fn is_admin(&self) -> bool {
        self.has_role("admin")
    }

    /// Get last_login_at as DateTime<Utc>.
    pub fn get_last_login_at(&self) -> Option<DateTime<Utc>> {
        self.last_login_at
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Get created_at as DateTime<Utc>.
    pub fn get_created_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.created_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Get updated_at as DateTime<Utc>.
    pub fn get_updated_at(&self) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(&self.updated_at)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Update the updated_at timestamp to now.
    pub fn touch(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_new() {
        let user = UserDbModel::new("testuser", "hashed_password", vec!["user".to_string()]);
        assert_eq!(user.username, "testuser");
        assert_eq!(user.password_hash, "hashed_password");
        assert!(user.is_active);
        assert!(user.must_change_password);
        assert!(user.last_login_at.is_none());
        assert!(user.email.is_none());
    }

    #[test]
    fn test_user_roles() {
        let mut user = UserDbModel::new(
            "admin",
            "hashed_password",
            vec!["admin".to_string(), "user".to_string()],
        );
        assert!(user.has_role("admin"));
        assert!(user.has_role("user"));
        assert!(!user.has_role("superuser"));
        assert!(user.is_admin());

        user.set_roles(vec!["user".to_string()]);
        assert!(!user.is_admin());
        assert!(user.has_role("user"));
    }

    #[test]
    fn test_user_get_roles() {
        let user = UserDbModel::new(
            "testuser",
            "hashed_password",
            vec!["admin".to_string(), "user".to_string()],
        );
        let roles = user.get_roles();
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&"admin".to_string()));
        assert!(roles.contains(&"user".to_string()));
    }
}

#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    // Property 21: Must change password flag for new users
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_must_change_password_flag_for_new_users(
            username in "[a-zA-Z][a-zA-Z0-9_]{3,20}",
            password_hash in "[a-zA-Z0-9$./]{60,100}",
            roles in prop::collection::vec("[a-zA-Z]{3,10}", 1..5)
        ) {
            let user = UserDbModel::new(&username, &password_hash, roles.clone());

            // Property: New users should have must_change_password=true
            prop_assert!(
                user.must_change_password,
                "New users should have must_change_password=true"
            );

            // Property: New users should be active by default
            prop_assert!(user.is_active, "New users should be active by default");

            // Property: New users should have no last_login_at
            prop_assert!(
                user.last_login_at.is_none(),
                "New users should have no last_login_at"
            );
        }
    }

    // Property 2: User creation round-trip
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_user_creation_round_trip(
            username in "[a-zA-Z][a-zA-Z0-9_]{3,20}",
            password_hash in "[a-zA-Z0-9$./]{60,100}",
            roles in prop::collection::vec("[a-zA-Z]{3,10}", 1..5)
        ) {
            let user = UserDbModel::new(&username, &password_hash, roles.clone());

            // Property: Username should be preserved
            prop_assert_eq!(&user.username, &username, "Username should be preserved");

            // Property: Password hash should be preserved
            prop_assert_eq!(&user.password_hash, &password_hash, "Password hash should be preserved");

            // Property: Roles should be preserved (round-trip through JSON)
            let retrieved_roles = user.get_roles();
            prop_assert_eq!(
                retrieved_roles.len(),
                roles.len(),
                "Role count should be preserved"
            );
            for role in &roles {
                prop_assert!(
                    retrieved_roles.contains(role),
                    "Role {} should be preserved", role
                );
            }

            // Property: ID should be a valid UUID
            prop_assert!(
                uuid::Uuid::parse_str(&user.id).is_ok(),
                "ID should be a valid UUID"
            );
        }
    }

    // Property 4: User update preserves unmodified fields
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_user_update_preserves_unmodified_fields(
            username in "[a-zA-Z][a-zA-Z0-9_]{3,20}",
            password_hash in "[a-zA-Z0-9$./]{60,100}",
            roles in prop::collection::vec("[a-zA-Z]{3,10}", 1..3),
            new_roles in prop::collection::vec("[a-zA-Z]{3,10}", 1..3)
        ) {
            let mut user = UserDbModel::new(&username, &password_hash, roles);
            let original_id = user.id.clone();
            let original_username = user.username.clone();
            let original_password_hash = user.password_hash.clone();
            let original_created_at = user.created_at.clone();

            // Update roles
            user.set_roles(new_roles.clone());

            // Property: ID should not change
            prop_assert_eq!(&user.id, &original_id, "ID should not change on update");

            // Property: Username should not change
            prop_assert_eq!(&user.username, &original_username, "Username should not change");

            // Property: Password hash should not change
            prop_assert_eq!(&user.password_hash, &original_password_hash, "Password hash should not change");

            // Property: created_at should not change
            prop_assert_eq!(&user.created_at, &original_created_at, "created_at should not change");

            // Property: Roles should be updated
            let updated_roles = user.get_roles();
            prop_assert_eq!(updated_roles.len(), new_roles.len(), "Roles should be updated");
        }
    }

    // Property: Role checking consistency
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_role_checking_consistency(
            username in "[a-zA-Z][a-zA-Z0-9_]{3,20}",
            password_hash in "[a-zA-Z0-9$./]{60,100}",
            roles in prop::collection::vec("[a-zA-Z]{3,10}", 1..5)
        ) {
            let user = UserDbModel::new(&username, &password_hash, roles.clone());

            // Property: has_role should return true for all assigned roles
            for role in &roles {
                prop_assert!(
                    user.has_role(role),
                    "has_role should return true for assigned role: {}", role
                );
            }

            // Property: has_role should return false for non-assigned roles
            prop_assert!(
                !user.has_role("nonexistent_role_xyz"),
                "has_role should return false for non-assigned role"
            );

            // Property: is_admin should be consistent with has_role("admin")
            prop_assert_eq!(
                user.is_admin(),
                user.has_role("admin"),
                "is_admin should equal has_role('admin')"
            );
        }
    }
}
