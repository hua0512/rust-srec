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

    /// Get last_login_at as `DateTime<Utc>`.
    pub fn get_last_login_at(&self) -> Option<DateTime<Utc>> {
        self.last_login_at
            .map(crate::database::time::ms_to_datetime)
    }

    /// Get created_at as `DateTime<Utc>`.
    pub fn get_created_at(&self) -> DateTime<Utc> {
        crate::database::time::ms_to_datetime(self.created_at)
    }

    /// Get updated_at as `DateTime<Utc>`.
    pub fn get_updated_at(&self) -> DateTime<Utc> {
        crate::database::time::ms_to_datetime(self.updated_at)
    }

    /// Update the updated_at timestamp to now.
    pub fn touch(&mut self) {
        self.updated_at = crate::database::time::now_ms();
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
mod new_user_defaults_tests {
    use super::*;

    #[test]
    fn test_new_user_must_change_password_short_username() {
        let username = "user";
        let password_hash = "$2b$12$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123";
        let roles = vec!["viewer".to_string()];
        let user = UserDbModel::new(username, password_hash, roles);

        assert!(
            user.must_change_password,
            "New users should have must_change_password=true"
        );
        assert!(user.is_active, "New users should be active by default");
        assert!(
            user.last_login_at.is_none(),
            "New users should have no last_login_at"
        );
    }

    #[test]
    fn test_new_user_must_change_password_long_username() {
        let username = "verylongusername1234";
        let password_hash = "$argon2id$v=19$m=65536,t=3,p=4$abcdefghijklmnopqrstuvwxyz$ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let roles = vec![
            "admin".to_string(),
            "editor".to_string(),
            "viewer".to_string(),
        ];
        let user = UserDbModel::new(username, password_hash, roles);

        assert!(
            user.must_change_password,
            "New users should have must_change_password=true"
        );
        assert!(user.is_active, "New users should be active by default");
        assert!(
            user.last_login_at.is_none(),
            "New users should have no last_login_at"
        );
    }

    #[test]
    fn test_new_user_defaults_single_role() {
        let username = "testuser";
        let password_hash = "$2y$10$N9qo8uLOickgx2ZMRZoMyeIjZAgcfl7p92ldGxad68LJZdL17lhWy";
        let roles = vec!["moderator".to_string()];
        let user = UserDbModel::new(username, password_hash, roles);

        assert!(user.must_change_password);
        assert!(user.is_active);
        assert!(user.last_login_at.is_none());
    }
}

#[cfg(test)]
mod user_creation_tests {
    use super::*;

    #[test]
    fn test_user_creation_preserves_fields_minimal() {
        let username = "alice";
        let password_hash = "$2b$12$LQv3c1yqBWVHxkd0LHAkCOYz6TtxMQJqhN8/LewY5GyYKKVgKKKKK";
        let roles = vec!["user".to_string()];
        let user = UserDbModel::new(username, password_hash, roles.clone());

        assert_eq!(&user.username, username, "Username should be preserved");
        assert_eq!(
            &user.password_hash, password_hash,
            "Password hash should be preserved"
        );

        let retrieved_roles = user.get_roles();
        assert_eq!(
            retrieved_roles.len(),
            roles.len(),
            "Role count should be preserved"
        );
        for role in &roles {
            assert!(
                retrieved_roles.contains(role),
                "Role {} should be preserved",
                role
            );
        }

        assert!(
            uuid::Uuid::parse_str(&user.id).is_ok(),
            "ID should be a valid UUID"
        );
    }

    #[test]
    fn test_user_creation_preserves_fields_multiple_roles() {
        let username = "bob_admin";
        let password_hash = "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHQ$iWh06vD8Fy27wf9npn6FXWiCX4K6pW6Ue1Bnzz07Z8A";
        let roles = vec![
            "admin".to_string(),
            "editor".to_string(),
            "moderator".to_string(),
        ];
        let user = UserDbModel::new(username, password_hash, roles.clone());

        assert_eq!(&user.username, username);
        assert_eq!(&user.password_hash, password_hash);

        let retrieved_roles = user.get_roles();
        assert_eq!(retrieved_roles.len(), roles.len());
        for role in &roles {
            assert!(retrieved_roles.contains(role));
        }

        assert!(uuid::Uuid::parse_str(&user.id).is_ok());
    }

    #[test]
    fn test_user_creation_uuid_uniqueness() {
        let username = "charlie";
        let password_hash = "$2y$10$abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let roles = vec!["guest".to_string()];

        let user1 = UserDbModel::new(username, password_hash, roles.clone());
        let user2 = UserDbModel::new(username, password_hash, roles);

        assert_ne!(user1.id, user2.id, "Each user should have unique UUID");
        assert!(uuid::Uuid::parse_str(&user1.id).is_ok());
        assert!(uuid::Uuid::parse_str(&user2.id).is_ok());
    }
}

#[cfg(test)]
mod user_update_tests {
    use super::*;

    #[test]
    fn test_update_roles_preserves_other_fields() {
        let username = "dave";
        let password_hash = "$2b$12$xyz123abc456def789ghi012jkl345mno678pqr901stu234vwx567";
        let roles = vec!["viewer".to_string()];
        let new_roles = vec!["editor".to_string()];

        let mut user = UserDbModel::new(username, password_hash, roles);
        let original_id = user.id.clone();
        let original_username = user.username.clone();
        let original_password_hash = user.password_hash.clone();
        let original_created_at = user.created_at;

        user.set_roles(new_roles.clone());

        assert_eq!(&user.id, &original_id, "ID should not change on update");
        assert_eq!(
            &user.username, &original_username,
            "Username should not change"
        );
        assert_eq!(
            &user.password_hash, &original_password_hash,
            "Password hash should not change"
        );
        assert_eq!(
            &user.created_at, &original_created_at,
            "created_at should not change"
        );

        let updated_roles = user.get_roles();
        assert_eq!(
            updated_roles.len(),
            new_roles.len(),
            "Roles should be updated"
        );
    }

    #[test]
    fn test_update_roles_from_single_to_multiple() {
        let username = "eve";
        let password_hash = "$argon2id$v=19$m=65536,t=3,p=4$saltsaltsalt$hashhashhashhashhashhash";
        let roles = vec!["basic".to_string()];
        let new_roles = vec!["admin".to_string(), "moderator".to_string()];

        let mut user = UserDbModel::new(username, password_hash, roles);
        let original_id = user.id.clone();
        let original_created_at = user.created_at;

        user.set_roles(new_roles.clone());

        assert_eq!(&user.id, &original_id);
        assert_eq!(&user.created_at, &original_created_at);

        let updated_roles = user.get_roles();
        assert_eq!(updated_roles.len(), new_roles.len());
    }

    #[test]
    fn test_update_roles_empty_to_populated() {
        let username = "frank";
        let password_hash = "$2y$10$EmptyRolesTestHashValue123456789012345678901234567890";
        let roles = vec!["temp".to_string()];
        let new_roles = vec!["permanent".to_string(), "verified".to_string()];

        let mut user = UserDbModel::new(username, password_hash, roles);
        let original_username = user.username.clone();
        let original_password_hash = user.password_hash.clone();

        user.set_roles(new_roles.clone());

        assert_eq!(&user.username, &original_username);
        assert_eq!(&user.password_hash, &original_password_hash);

        let updated_roles = user.get_roles();
        assert_eq!(updated_roles.len(), new_roles.len());
    }
}

#[cfg(test)]
mod role_checking_tests {
    use super::*;

    #[test]
    fn test_has_role_returns_true_for_assigned_roles() {
        let username = "grace";
        let password_hash = "$2b$12$RoleCheckingTestHashValue1234567890123456789012345678";
        let roles = vec![
            "admin".to_string(),
            "editor".to_string(),
            "viewer".to_string(),
        ];
        let user = UserDbModel::new(username, password_hash, roles.clone());

        for role in &roles {
            assert!(
                user.has_role(role),
                "has_role should return true for assigned role: {}",
                role
            );
        }
    }

    #[test]
    fn test_has_role_returns_false_for_nonexistent_role() {
        let username = "henry";
        let password_hash = "$argon2id$v=19$m=19456,t=2,p=1$NonExistentRoleTest$HashValue123";
        let roles = vec!["user".to_string(), "guest".to_string()];
        let user = UserDbModel::new(username, password_hash, roles);

        assert!(
            !user.has_role("nonexistent_role_xyz"),
            "has_role should return false for non-assigned role"
        );
        assert!(!user.has_role("admin"));
        assert!(!user.has_role("superuser"));
    }

    #[test]
    fn test_is_admin_consistency_with_admin_role() {
        let username = "admin_user";
        let password_hash = "$2y$10$AdminConsistencyTestHashValue123456789012345678901234";
        let roles = vec!["admin".to_string(), "moderator".to_string()];
        let user = UserDbModel::new(username, password_hash, roles);

        assert_eq!(
            user.is_admin(),
            user.has_role("admin"),
            "is_admin should equal has_role('admin')"
        );
        assert!(user.is_admin());
    }

    #[test]
    fn test_is_admin_consistency_without_admin_role() {
        let username = "regular_user";
        let password_hash = "$2b$12$RegularUserTestHashValue1234567890123456789012345678";
        let roles = vec!["editor".to_string(), "viewer".to_string()];
        let user = UserDbModel::new(username, password_hash, roles);

        assert_eq!(
            user.is_admin(),
            user.has_role("admin"),
            "is_admin should equal has_role('admin')"
        );
        assert!(!user.is_admin());
    }
}
