//! User repository for authentication database operations.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::repositories::row_write::{Mutation, RowWrite, WriteMode};

use super::models::UserDbModel;

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

/// SQLx implementation of [`UserRepository`].
pub struct SqlxUserRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxUserRepository {
    /// Create a new `SqlxUserRepository` with the given connection pools.
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

pub(crate) async fn write_user(
    connection: &mut sqlx::SqliteConnection,
    model: &UserDbModel,
    mode: WriteMode,
    updated_at: i64,
) -> std::result::Result<(), sqlx::Error> {
    let mutation = match mode {
        WriteMode::Insert => Mutation::Insert,
        WriteMode::Update => Mutation::Update,
        WriteMode::Import => Mutation::Upsert,
    };
    let mut row = RowWrite::new("users", mutation, &model.id)?;
    row.field("username", &model.username, true)?;
    row.field("password_hash", &model.password_hash, true)?;
    row.field("email", &model.email, true)?;
    row.field("roles", &model.roles, true)?;
    row.field("is_active", model.is_active, true)?;
    row.field("must_change_password", model.must_change_password, true)?;
    row.field("last_login_at", model.last_login_at, true)?;
    row.field("created_at", model.created_at, false)?;
    row.field("updated_at", updated_at, true)?;
    row.execute(connection).await
}

pub(crate) async fn import_user(
    connection: &mut sqlx::SqliteConnection,
    model: &UserDbModel,
) -> std::result::Result<(), sqlx::Error> {
    write_user(connection, model, WriteMode::Import, model.updated_at).await
}

pub(crate) enum EmailSlots<'a> {
    All,
    User(&'a str),
}

pub(crate) async fn release_email_slots(
    connection: &mut sqlx::SqliteConnection,
    slots: EmailSlots<'_>,
) -> std::result::Result<(), sqlx::Error> {
    match slots {
        EmailSlots::All => {
            sqlx::query("UPDATE users SET email = NULL WHERE email IS NOT NULL")
                .execute(connection)
                .await?;
        }
        EmailSlots::User(id) => {
            sqlx::query("UPDATE users SET email = NULL WHERE id = ? AND email IS NOT NULL")
                .bind(id)
                .execute(connection)
                .await?;
        }
    }
    Ok(())
}

pub(crate) async fn delete_user(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}

#[async_trait]
impl UserRepository for SqlxUserRepository {
    async fn create(&self, user: &UserDbModel) -> Result<()> {
        write_user(
            &mut *self.write_pool.acquire().await?,
            user,
            WriteMode::Insert,
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
        write_user(
            &mut *self.write_pool.acquire().await?,
            user,
            WriteMode::Update,
            now,
        )
        .await?;
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        delete_user(&mut *self.write_pool.acquire().await?, id).await?;
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
