//! Refresh token repository for database operations.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::begin_immediate;
use crate::database::models::{AuthSessionDbModel, RefreshTokenDbModel};
use crate::database::retry::retry_on_sqlite_busy;

/// The predecessor's state when an atomic rotation obtains the database write lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenRotation {
    Rotated,
    Revoked {
        revoked_at: i64,
    },
    /// The lineage was explicitly closed; reject without escalating reuse to other sessions.
    SessionRevoked,
    Expired,
    NotFound,
}

/// Refresh token repository trait for token data access operations.
#[async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    /// Create a new refresh token in the database.
    async fn create(&self, token: &RefreshTokenDbModel) -> Result<()>;

    async fn create_session(
        &self,
        _session: &AuthSessionDbModel,
        _token: &RefreshTokenDbModel,
        _expected_password_hash: &str,
    ) -> Result<bool> {
        Err(crate::Error::Other(
            "Session-aware login is not supported by this repository".to_owned(),
        ))
    }

    async fn find_session(
        &self,
        _user_id: &str,
        _session_id: &str,
    ) -> Result<Option<AuthSessionDbModel>> {
        Ok(None)
    }

    async fn revoke_session(&self, _user_id: &str, _session_id: &str) -> Result<()> {
        Err(crate::Error::Other(
            "Session revocation is not supported by this repository".to_owned(),
        ))
    }

    async fn rotate_session(
        &self,
        _predecessor_id: &str,
        _replacement: &RefreshTokenDbModel,
        _session_expires_at: i64,
    ) -> Result<RefreshTokenRotation> {
        Err(crate::Error::Other(
            "Session-aware rotation is not supported by this repository".to_owned(),
        ))
    }

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

    /// Apply reuse policy only while the replayed lineage remains open. The session check
    /// and user-wide revocation must share a transaction so explicit logout cannot escalate.
    async fn revoke_all_for_active_session(&self, _user_id: &str, _session_id: &str) -> Result<()> {
        Err(crate::Error::Other(
            "Conditional session revocation is not supported by this repository".to_owned(),
        ))
    }

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

async fn insert_token(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    token: &RefreshTokenDbModel,
    session_id: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO refresh_tokens(id,user_id,token_hash,expires_at,created_at,revoked_at,device_info,session_id) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&token.id).bind(&token.user_id).bind(&token.token_hash).bind(token.expires_at)
        .bind(token.created_at).bind(token.revoked_at).bind(&token.device_info).bind(session_id)
        .execute(&mut **tx).await?;
    Ok(())
}

#[async_trait]
impl RefreshTokenRepository for SqlxRefreshTokenRepository {
    async fn rotate(
        &self,
        predecessor_id: &str,
        replacement: &RefreshTokenDbModel,
    ) -> Result<RefreshTokenRotation> {
        self.rotate_session(predecessor_id, replacement, replacement.expires_at)
            .await
    }

    async fn rotate_session(
        &self,
        predecessor_id: &str,
        replacement: &RefreshTokenDbModel,
        session_expires_at: i64,
    ) -> Result<RefreshTokenRotation> {
        retry_on_sqlite_busy("rotate_refresh_token", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();
            if replacement.revoked_at.is_some() || replacement.expires_at <= now {
                return Err(crate::Error::validation("Refresh token replacement must be active and unexpired"));
            }
            let Some(predecessor) = sqlx::query_as::<_, RefreshTokenDbModel>("SELECT * FROM refresh_tokens WHERE id = ?")
                .bind(predecessor_id).fetch_optional(&mut *tx).await? else {
                    return Ok(RefreshTokenRotation::NotFound);
                };
            if predecessor.user_id != replacement.user_id {
                return Err(crate::Error::validation("Refresh token replacement belongs to a different user"));
            }
            let session_id = predecessor.session_id.as_deref().unwrap_or(&predecessor.id);
            if replacement.session_id.as_deref().is_some_and(|id| id != session_id) {
                return Err(crate::Error::validation("Refresh token replacement changed session lineage"));
            }
            // Nullable legacy rows can be adopted only while their predecessor is active.
            if predecessor.session_id.is_none() && predecessor.revoked_at.is_none() && predecessor.expires_at > now {
                sqlx::query("INSERT INTO auth_sessions(id,user_id,created_at,expires_at) VALUES(?,?,?,?) ON CONFLICT(id) DO NOTHING")
                    .bind(session_id).bind(&predecessor.user_id).bind(predecessor.created_at).bind(predecessor.expires_at)
                    .execute(&mut *tx).await?;
                sqlx::query("UPDATE refresh_tokens SET session_id = ? WHERE id = ?")
                    .bind(session_id).bind(predecessor_id).execute(&mut *tx).await?;
            }
            let session = sqlx::query_as::<_, AuthSessionDbModel>("SELECT * FROM auth_sessions WHERE id = ? AND user_id = ?")
                .bind(session_id).bind(&predecessor.user_id).fetch_optional(&mut *tx).await?;
            let Some(session) = session else { return Ok(RefreshTokenRotation::SessionRevoked); };
            if session.revoked_at.is_some() { return Ok(RefreshTokenRotation::SessionRevoked); }
            if let Some(revoked_at) = predecessor.revoked_at { return Ok(RefreshTokenRotation::Revoked { revoked_at }); }
            if predecessor.expires_at <= now || session.expires_at <= now { return Ok(RefreshTokenRotation::Expired); }
            let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND is_active = 1)")
                .bind(&predecessor.user_id).fetch_one(&mut *tx).await?;
            if !active { return Ok(RefreshTokenRotation::SessionRevoked); }
            sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
                .bind(now).bind(predecessor_id).execute(&mut *tx).await?;
            insert_token(&mut tx, replacement, session_id).await?;
            sqlx::query("UPDATE auth_sessions SET expires_at = MAX(expires_at, ?) WHERE id = ?")
                .bind(session_expires_at.max(replacement.expires_at)).bind(session_id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(RefreshTokenRotation::Rotated)
        }).await
    }

    async fn create_session(
        &self,
        session: &AuthSessionDbModel,
        token: &RefreshTokenDbModel,
        expected_password_hash: &str,
    ) -> Result<bool> {
        retry_on_sqlite_busy("create_auth_session", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE id = ? AND password_hash = ? AND is_active = 1)")
                .bind(&session.user_id).bind(expected_password_hash).fetch_one(&mut *tx).await?;
            if !valid { return Ok(false); }
            if token.user_id != session.user_id || token.session_id.as_deref() != Some(session.id.as_str())
                || session.revoked_at.is_some() || token.revoked_at.is_some()
                || token.expires_at <= crate::database::time::now_ms() || session.expires_at < token.expires_at {
                return Err(crate::Error::validation("Invalid auth session issuance"));
            }
            sqlx::query("INSERT INTO auth_sessions(id,user_id,created_at,expires_at) VALUES(?,?,?,?)")
                .bind(&session.id).bind(&session.user_id).bind(session.created_at).bind(session.expires_at).execute(&mut *tx).await?;
            insert_token(&mut tx, token, &session.id).await?;
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE users SET last_login_at = ?, updated_at = ? WHERE id = ?")
                .bind(now).bind(now).bind(&session.user_id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(true)
        }).await
    }

    async fn create(&self, token: &RefreshTokenDbModel) -> Result<()> {
        retry_on_sqlite_busy("create_refresh_token", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let session_id = token.session_id.as_deref().unwrap_or(&token.id);
            sqlx::query("INSERT INTO auth_sessions(id,user_id,created_at,expires_at) VALUES(?,?,?,?) ON CONFLICT(id) DO NOTHING")
                .bind(session_id).bind(&token.user_id).bind(token.created_at).bind(token.expires_at).execute(&mut *tx).await?;
            let owner: String = sqlx::query_scalar("SELECT user_id FROM auth_sessions WHERE id = ?")
                .bind(session_id).fetch_one(&mut *tx).await?;
            if owner != token.user_id { return Err(crate::Error::validation("Session belongs to a different user")); }
            insert_token(&mut tx, token, session_id).await?;
            tx.commit().await?;
            Ok(())
        }).await
    }

    async fn find_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<Option<AuthSessionDbModel>> {
        Ok(
            sqlx::query_as("SELECT * FROM auth_sessions WHERE id = ? AND user_id = ?")
                .bind(session_id)
                .bind(user_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn revoke_session(&self, user_id: &str, session_id: &str) -> Result<()> {
        retry_on_sqlite_busy("revoke_auth_session", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE auth_sessions SET revoked_at = COALESCE(revoked_at, ?) WHERE id = ? AND user_id = ?")
                .bind(now).bind(session_id).bind(user_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE refresh_tokens SET revoked_at = COALESCE(revoked_at, ?) WHERE user_id = ? AND (session_id = ? OR (session_id IS NULL AND id = ?))")
                .bind(now).bind(user_id).bind(session_id).bind(session_id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        }).await
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
        retry_on_sqlite_busy("revoke_all_auth_sessions", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let now = crate::database::time::now_ms();
            sqlx::query(
                "UPDATE auth_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL",
            )
            .bind(now)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn revoke_all_for_active_session(&self, user_id: &str, session_id: &str) -> Result<()> {
        retry_on_sqlite_busy("revoke_sessions_on_refresh_reuse", || async {
            let mut tx = begin_immediate(&self.write_pool).await?;
            let open: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM auth_sessions WHERE id = ? AND user_id = ? AND revoked_at IS NULL)")
                .bind(session_id).bind(user_id).fetch_one(&mut *tx).await?;
            if !open { return Ok(()); }
            let now = crate::database::time::now_ms();
            sqlx::query("UPDATE auth_sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
                .bind(now).bind(user_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE refresh_tokens SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
                .bind(now).bind(user_id).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        }).await
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

/// Import invalidates both access-session authority and legacy refresh tokens in
/// its existing transaction. This deliberately deletes, rather than revokes, rows.
pub(crate) async fn invalidate_all_for_import(
    connection: &mut sqlx::SqliteConnection,
) -> std::result::Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM auth_sessions")
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM refresh_tokens")
        .execute(connection)
        .await?;
    Ok(())
}
