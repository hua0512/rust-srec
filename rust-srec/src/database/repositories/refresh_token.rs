//! Refresh token repository for database operations.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::begin_immediate;
use crate::database::models::RefreshTokenDbModel;
use crate::database::retry::retry_on_sqlite_busy;

/// The predecessor's state when an atomic rotation obtains the database write lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenRotation {
    Rotated,
    Revoked { revoked_at: i64 },
    Expired,
    NotFound,
}

/// Refresh token repository trait for token data access operations.
#[async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    /// Create a new refresh token in the database.
    async fn create(&self, token: &RefreshTokenDbModel) -> Result<()>;

    /// Consume an active predecessor and create its replacement in one transaction.
    /// A revoked predecessor never issues another replacement, including concurrent callers.
    /// An insertion error leaves the predecessor unchanged.
    async fn rotate(
        &self,
        predecessor_id: &str,
        replacement: &RefreshTokenDbModel,
    ) -> Result<RefreshTokenRotation>;

    /// Find a refresh token by its hash.
    async fn find_by_token_hash(&self, hash: &str) -> Result<Option<RefreshTokenDbModel>>;

    /// Find all active (non-revoked, non-expired) tokens for a user.
    async fn find_active_by_user(&self, user_id: &str) -> Result<Vec<RefreshTokenDbModel>>;

    /// Revoke a specific token by its ID.
    async fn revoke(&self, id: &str) -> Result<()>;

    /// Revoke all tokens for a specific user.
    async fn revoke_all_for_user(&self, user_id: &str) -> Result<()>;

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
    async fn rotate(
        &self,
        predecessor_id: &str,
        replacement: &RefreshTokenDbModel,
    ) -> Result<RefreshTokenRotation> {
        retry_on_sqlite_busy("rotate_refresh_token", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();
            if replacement.revoked_at.is_some() || replacement.expires_at <= now {
                return Err(crate::Error::Validation(
                    "Refresh token replacement must be active and unexpired".to_string(),
                ));
            }
            let consumed = sqlx::query(
                "UPDATE refresh_tokens SET revoked_at = ? \
                 WHERE id = ? AND user_id = ? AND revoked_at IS NULL AND expires_at > ?",
            )
            .bind(now)
            .bind(predecessor_id)
            .bind(&replacement.user_id)
            .bind(now)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if consumed == 0 {
                let predecessor = sqlx::query_as::<_, RefreshTokenDbModel>(
                    "SELECT * FROM refresh_tokens WHERE id = ?",
                )
                .bind(predecessor_id)
                .fetch_optional(&mut *tx)
                .await?;
                let outcome = match predecessor {
                    None => RefreshTokenRotation::NotFound,
                    Some(token) if token.user_id != replacement.user_id => {
                        return Err(crate::Error::Validation(
                            "Refresh token replacement belongs to a different user".to_string(),
                        ));
                    }
                    Some(RefreshTokenDbModel {
                        revoked_at: Some(revoked_at),
                        ..
                    }) => RefreshTokenRotation::Revoked { revoked_at },
                    Some(_) => RefreshTokenRotation::Expired,
                };
                tx.rollback().await?;
                return Ok(outcome);
            }
            sqlx::query(
                "INSERT INTO refresh_tokens \
                 (id, user_id, token_hash, expires_at, created_at, revoked_at, device_info) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&replacement.id)
            .bind(&replacement.user_id)
            .bind(&replacement.token_hash)
            .bind(replacement.expires_at)
            .bind(replacement.created_at)
            .bind(replacement.revoked_at)
            .bind(&replacement.device_info)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(RefreshTokenRotation::Rotated)
        })
        .await
    }

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
        sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
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
