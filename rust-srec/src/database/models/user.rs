//! User database model.

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
