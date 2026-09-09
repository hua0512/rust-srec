//! User repository for database operations.

mod writes;
pub(crate) use writes::{EmailSlots, delete_user, import_user, release_email_slots};

use async_trait::async_trait;
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

    /// Update the last login timestamp (epoch ms) for a user.
    async fn update_last_login(&self, id: &str, time_ms: i64) -> Result<()>;

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
    write_pool: SqlitePool,
}

impl SqlxUserRepository {
    /// Create a new SqlxUserRepository with the given connection pool.
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn create(&self, user: &UserDbModel) -> Result<()> {
        writes::write_user(
            &mut *self.write_pool.acquire().await?,
            user,
            super::row_write::WriteMode::Insert,
            user.updated_at,
        )
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
        let now = crate::database::time::now_ms();
        writes::write_user(
            &mut *self.write_pool.acquire().await?,
            user,
            super::row_write::WriteMode::Update,
            now,
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        writes::delete_user(&mut *self.write_pool.acquire().await?, id).await?;
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

    async fn update_last_login(&self, id: &str, time_ms: i64) -> Result<()> {
        sqlx::query("UPDATE users SET last_login_at = ?, updated_at = ? WHERE id = ?")
            .bind(time_ms)
            .bind(crate::database::time::now_ms())
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn update_password(
        &self,
        id: &str,
        password_hash: &str,
        clear_must_change: bool,
    ) -> Result<()> {
        let now = crate::database::time::now_ms();
        if clear_must_change {
            sqlx::query(
                "UPDATE users SET password_hash = ?, must_change_password = FALSE, updated_at = ? WHERE id = ?",
            )
            .bind(password_hash)
            .bind(now)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        } else {
            sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
                .bind(password_hash)
                .bind(now)
                .bind(id)
                .execute(&self.write_pool)
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
