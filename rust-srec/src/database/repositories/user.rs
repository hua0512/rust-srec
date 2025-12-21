//! User repository for database operations.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::Result;
use crate::database::models::UserDbModel;

/// User repository trait for user data access operations.
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Create a new user in the database.
    async fn create(&self, user: &UserDbModel) -> Result<()>;

    /// Find a user by their unique ID.
    async fn find_by_id(&self, id: &str) -> Result<Option<UserDbModel>>;

    /// Find a user by their username.
    async fn find_by_username(&self, username: &str) -> Result<Option<UserDbModel>>;

    /// Find a user by their email address.
    async fn find_by_email(&self, email: &str) -> Result<Option<UserDbModel>>;

    /// Update an existing user.
    async fn update(&self, user: &UserDbModel) -> Result<()>;

    /// Delete a user by their ID.
    async fn delete(&self, id: &str) -> Result<()>;

    /// List users with pagination.
    async fn list(&self, limit: i64, offset: i64) -> Result<Vec<UserDbModel>>;

    /// Update the last login timestamp for a user.
    async fn update_last_login(&self, id: &str, time: DateTime<Utc>) -> Result<()>;

    /// Update a user's password hash.
    async fn update_password(
        &self,
        id: &str,
        password_hash: &str,
        clear_must_change: bool,
    ) -> Result<()>;

    /// Count total number of users.
    async fn count(&self) -> Result<i64>;
}

/// SQLx implementation of UserRepository.
pub struct SqlxUserRepository {
    pool: SqlitePool,
}

impl SqlxUserRepository {
    /// Create a new SqlxUserRepository with the given connection pool.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn create(&self, user: &UserDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO users (
                id, username, password_hash, email, roles, is_active,
                must_change_password, last_login_at, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&user.id)
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.email)
        .bind(&user.roles)
        .bind(user.is_active)
        .bind(user.must_change_password)
        .bind(&user.last_login_at)
        .bind(&user.created_at)
        .bind(&user.updated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn find_by_id(&self, id: &str) -> Result<Option<UserDbModel>> {
        let user = sqlx::query_as::<_, UserDbModel>("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_username(&self, username: &str) -> Result<Option<UserDbModel>> {
        let user = sqlx::query_as::<_, UserDbModel>("SELECT * FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn find_by_email(&self, email: &str) -> Result<Option<UserDbModel>> {
        let user = sqlx::query_as::<_, UserDbModel>("SELECT * FROM users WHERE email = ?")
            .bind(email)
            .fetch_optional(&self.pool)
            .await?;
        Ok(user)
    }

    async fn update(&self, user: &UserDbModel) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"
            UPDATE users SET
                username = ?,
                password_hash = ?,
                email = ?,
                roles = ?,
                is_active = ?,
                must_change_password = ?,
                last_login_at = ?,
                updated_at = ?
            WHERE id = ?
            "#,
        )
        .bind(&user.username)
        .bind(&user.password_hash)
        .bind(&user.email)
        .bind(&user.roles)
        .bind(user.is_active)
        .bind(user.must_change_password)
        .bind(&user.last_login_at)
        .bind(&now)
        .bind(&user.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn list(&self, limit: i64, offset: i64) -> Result<Vec<UserDbModel>> {
        let users = sqlx::query_as::<_, UserDbModel>(
            "SELECT * FROM users ORDER BY created_at DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;
        Ok(users)
    }

    async fn update_last_login(&self, id: &str, time: DateTime<Utc>) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = ?, updated_at = ? WHERE id = ?")
            .bind(time.to_rfc3339())
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn update_password(
        &self,
        id: &str,
        password_hash: &str,
        clear_must_change: bool,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        if clear_must_change {
            sqlx::query(
                "UPDATE users SET password_hash = ?, must_change_password = FALSE, updated_at = ? WHERE id = ?",
            )
            .bind(password_hash)
            .bind(&now)
            .bind(id)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
                .bind(password_hash)
                .bind(&now)
                .bind(id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn count(&self) -> Result<i64> {
        let result: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await?;
        Ok(result.0)
    }
}
