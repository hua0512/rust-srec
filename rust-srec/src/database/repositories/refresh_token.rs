//! Refresh token repository for database operations.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::models::RefreshTokenDbModel;

/// Refresh token repository trait for token data access operations.
#[async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    /// Create a new refresh token in the database.
    async fn create(&self, token: &RefreshTokenDbModel) -> Result<()>;

    /// Find a refresh token by its hash.
    async fn find_by_token_hash(&self, hash: &str) -> Result<Option<RefreshTokenDbModel>>;

    /// Find all active (non-revoked, non-expired) tokens for a user.
    async fn find_active_by_user(&self, user_id: &str) -> Result<Vec<RefreshTokenDbModel>>;

    /// Revoke a specific token by its ID.
    async fn revoke(&self, id: &str) -> Result<()>;

    /// Revoke all tokens for a specific user.
    async fn revoke_all_for_user(&self, user_id: &str) -> Result<()>;

    /// Clean up expired tokens from the database.
    /// Returns the number of tokens deleted.
    async fn cleanup_expired(&self) -> Result<u64>;

    /// Count active tokens for a user.
    async fn count_active_by_user(&self, user_id: &str) -> Result<i64>;
}

/// SQLx implementation of RefreshTokenRepository.
pub struct SqlxRefreshTokenRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxRefreshTokenRepository {
    /// Create a new SqlxRefreshTokenRepository with the given connection pool.
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl RefreshTokenRepository for SqlxRefreshTokenRepository {
    async fn create(&self, token: &RefreshTokenDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (
                id, user_id, token_hash, expires_at, created_at, revoked_at, device_info
            ) VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&token.id)
        .bind(&token.user_id)
        .bind(&token.token_hash)
        .bind(token.expires_at)
        .bind(token.created_at)
        .bind(token.revoked_at)
        .bind(&token.device_info)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn find_by_token_hash(&self, hash: &str) -> Result<Option<RefreshTokenDbModel>> {
        let token = sqlx::query_as::<_, RefreshTokenDbModel>(
            "SELECT * FROM refresh_tokens WHERE token_hash = ?",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(token)
    }

    async fn find_active_by_user(&self, user_id: &str) -> Result<Vec<RefreshTokenDbModel>> {
        let now = crate::database::time::now_ms();

        let tokens = sqlx::query_as::<_, RefreshTokenDbModel>(
            r#"
            SELECT * FROM refresh_tokens
            WHERE user_id = ?
              AND revoked_at IS NULL
              AND expires_at > ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(user_id)
        .bind(now)
        .fetch_all(&self.pool)
        .await?;
        Ok(tokens)
    }

    async fn revoke(&self, id: &str) -> Result<()> {
        let now = crate::database::time::now_ms();
        sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE id = ?")
            .bind(now)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }

    async fn revoke_all_for_user(&self, user_id: &str) -> Result<()> {
        let now = crate::database::time::now_ms();
        sqlx::query(
            "UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(user_id)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn cleanup_expired(&self) -> Result<u64> {
        let now = crate::database::time::now_ms();
        let result = sqlx::query("DELETE FROM refresh_tokens WHERE expires_at < ?")
            .bind(now)
            .execute(&self.write_pool)
            .await?;
        Ok(result.rows_affected())
    }

    async fn count_active_by_user(&self, user_id: &str) -> Result<i64> {
        let now = crate::database::time::now_ms();
        let result: (i64,) = sqlx::query_as(
            r#"
            SELECT COUNT(*) FROM refresh_tokens
            WHERE user_id = ?
              AND revoked_at IS NULL
              AND expires_at > ?
            "#,
        )
        .bind(user_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        Ok(result.0)
    }
}
