//! API key repository for database operations.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::models::ApiKeyDbModel;

/// API key repository trait for key data access operations.
#[async_trait]
pub trait ApiKeyRepository: Send + Sync {
    /// Create a new API key in the database.
    async fn create(&self, key: &ApiKeyDbModel) -> Result<()>;

    /// Find an API key by the SHA-256 hash of its raw value.
    async fn find_by_key_hash(&self, hash: &str) -> Result<Option<ApiKeyDbModel>>;

    /// List all keys (including revoked/expired ones) for a user, newest first.
    async fn list_by_user(&self, user_id: &str) -> Result<Vec<ApiKeyDbModel>>;

    /// Revoke a key by its ID, scoped to the owning user. Returns whether a
    /// row was actually revoked (false = unknown id or already revoked).
    async fn revoke(&self, user_id: &str, id: &str) -> Result<bool>;

    /// Update `last_used_at`. Callers throttle this (see
    /// `AuthService::authorize_api_key`) so it is not a per-request write.
    async fn update_last_used(&self, id: &str, time_ms: i64) -> Result<()>;
}

/// SQLx implementation of ApiKeyRepository.
pub struct SqlxApiKeyRepository {
    pool: SqlitePool,
    write_pool: SqlitePool,
}

impl SqlxApiKeyRepository {
    /// Create a new SqlxApiKeyRepository with the given connection pools.
    pub fn new(pool: SqlitePool, write_pool: SqlitePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl ApiKeyRepository for SqlxApiKeyRepository {
    async fn create(&self, key: &ApiKeyDbModel) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO api_keys (
                id, user_id, name, key_hash, key_prefix, access_level,
                expires_at, last_used_at, created_at, revoked_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&key.id)
        .bind(&key.user_id)
        .bind(&key.name)
        .bind(&key.key_hash)
        .bind(&key.key_prefix)
        .bind(&key.access_level)
        .bind(key.expires_at)
        .bind(key.last_used_at)
        .bind(key.created_at)
        .bind(key.revoked_at)
        .execute(&self.write_pool)
        .await?;
        Ok(())
    }

    async fn find_by_key_hash(&self, hash: &str) -> Result<Option<ApiKeyDbModel>> {
        let key = sqlx::query_as::<_, ApiKeyDbModel>("SELECT * FROM api_keys WHERE key_hash = ?")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await?;
        Ok(key)
    }

    async fn list_by_user(&self, user_id: &str) -> Result<Vec<ApiKeyDbModel>> {
        let keys = sqlx::query_as::<_, ApiKeyDbModel>(
            "SELECT * FROM api_keys WHERE user_id = ? ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(keys)
    }

    async fn revoke(&self, user_id: &str, id: &str) -> Result<bool> {
        let now = crate::database::time::now_ms();
        let result = sqlx::query(
            "UPDATE api_keys SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(id)
        .bind(user_id)
        .execute(&self.write_pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_last_used(&self, id: &str, time_ms: i64) -> Result<()> {
        sqlx::query("UPDATE api_keys SET last_used_at = ? WHERE id = ?")
            .bind(time_ms)
            .bind(id)
            .execute(&self.write_pool)
            .await?;
        Ok(())
    }
}

/// In-memory `ApiKeyRepository` shared by unit tests across the crate
/// (`api::auth_service`, `api::middleware::auth`, `api::routes::auth`).
#[cfg(test)]
#[derive(Default)]
pub struct InMemoryApiKeyRepository {
    keys: std::sync::Mutex<Vec<ApiKeyDbModel>>,
}

#[cfg(test)]
impl InMemoryApiKeyRepository {
    pub fn with_keys(keys: Vec<ApiKeyDbModel>) -> Self {
        Self {
            keys: std::sync::Mutex::new(keys),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ApiKeyDbModel>> {
        self.keys.lock().expect("test repository mutex poisoned")
    }
}

#[cfg(test)]
#[async_trait]
impl ApiKeyRepository for InMemoryApiKeyRepository {
    async fn create(&self, key: &ApiKeyDbModel) -> Result<()> {
        self.lock().push(key.clone());
        Ok(())
    }

    async fn find_by_key_hash(&self, hash: &str) -> Result<Option<ApiKeyDbModel>> {
        Ok(self.lock().iter().find(|k| k.key_hash == hash).cloned())
    }

    async fn list_by_user(&self, user_id: &str) -> Result<Vec<ApiKeyDbModel>> {
        let mut keys: Vec<_> = self
            .lock()
            .iter()
            .filter(|k| k.user_id == user_id)
            .cloned()
            .collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.created_at));
        Ok(keys)
    }

    async fn revoke(&self, user_id: &str, id: &str) -> Result<bool> {
        let mut keys = self.lock();
        for key in keys.iter_mut() {
            if key.id == id && key.user_id == user_id && key.revoked_at.is_none() {
                key.revoked_at = Some(crate::database::time::now_ms());
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn update_last_used(&self, id: &str, time_ms: i64) -> Result<()> {
        let mut keys = self.lock();
        for key in keys.iter_mut() {
            if key.id == id {
                key.last_used_at = Some(time_ms);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::ApiKeyAccessLevel;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite should connect");
        sqlx::query(
            r#"
            CREATE TABLE users (
                id TEXT PRIMARY KEY NOT NULL,
                username TEXT NOT NULL UNIQUE,
                password_hash TEXT NOT NULL,
                email TEXT UNIQUE,
                roles TEXT NOT NULL DEFAULT '["user"]',
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                must_change_password BOOLEAN NOT NULL DEFAULT TRUE,
                last_login_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE api_keys (
                id TEXT PRIMARY KEY NOT NULL,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                key_hash TEXT NOT NULL UNIQUE,
                key_prefix TEXT NOT NULL,
                access_level TEXT NOT NULL,
                expires_at INTEGER,
                last_used_at INTEGER,
                created_at INTEGER NOT NULL,
                revoked_at INTEGER,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(&pool)
        .await
        .expect("schema should apply");
        for user_id in ["user-1", "user-2"] {
            sqlx::query(
                "INSERT INTO users (id, username, password_hash, created_at, updated_at) VALUES (?, ?, 'hash', 0, 0)",
            )
            .bind(user_id)
            .bind(user_id)
            .execute(&pool)
            .await
            .expect("user seed should apply");
        }
        pool
    }

    fn sample_key(user_id: &str, hash: &str) -> ApiKeyDbModel {
        ApiKeyDbModel::new(
            user_id,
            "test key",
            hash,
            "srec_ab12cd34",
            ApiKeyAccessLevel::Full,
            None,
        )
    }

    #[tokio::test]
    async fn create_and_find_by_hash() {
        let pool = setup_pool().await;
        let repo = SqlxApiKeyRepository::new(pool.clone(), pool);
        let key = sample_key("user-1", "hash-1");

        repo.create(&key).await.expect("create should succeed");

        let found = repo
            .find_by_key_hash("hash-1")
            .await
            .expect("query should succeed")
            .expect("key should exist");
        assert_eq!(found.id, key.id);
        assert_eq!(found.get_access_level(), ApiKeyAccessLevel::Full);

        let missing = repo
            .find_by_key_hash("nope")
            .await
            .expect("query should succeed");
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn list_by_user_returns_newest_first() {
        let pool = setup_pool().await;
        let repo = SqlxApiKeyRepository::new(pool.clone(), pool);

        let mut first = sample_key("user-1", "hash-a");
        first.created_at -= 10_000;
        repo.create(&first).await.expect("create should succeed");
        let second = sample_key("user-1", "hash-b");
        repo.create(&second).await.expect("create should succeed");
        let other = sample_key("user-2", "hash-c");
        repo.create(&other).await.expect("create should succeed");

        let keys = repo
            .list_by_user("user-1")
            .await
            .expect("list should succeed");
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].id, second.id);
        assert_eq!(keys[1].id, first.id);
    }

    #[tokio::test]
    async fn revoke_is_scoped_to_owner_and_idempotent() {
        let pool = setup_pool().await;
        let repo = SqlxApiKeyRepository::new(pool.clone(), pool);
        let key = sample_key("user-1", "hash-1");
        repo.create(&key).await.expect("create should succeed");

        // Wrong owner does not revoke.
        let revoked = repo
            .revoke("user-2", &key.id)
            .await
            .expect("revoke should succeed");
        assert!(!revoked);

        let revoked = repo
            .revoke("user-1", &key.id)
            .await
            .expect("revoke should succeed");
        assert!(revoked);

        // Second revoke reports no change.
        let revoked = repo
            .revoke("user-1", &key.id)
            .await
            .expect("revoke should succeed");
        assert!(!revoked);

        let found = repo
            .find_by_key_hash("hash-1")
            .await
            .expect("query should succeed")
            .expect("key should exist");
        assert!(found.is_revoked());
    }

    #[tokio::test]
    async fn update_last_used_persists() {
        let pool = setup_pool().await;
        let repo = SqlxApiKeyRepository::new(pool.clone(), pool);
        let key = sample_key("user-1", "hash-1");
        repo.create(&key).await.expect("create should succeed");

        repo.update_last_used(&key.id, 1234567890)
            .await
            .expect("update should succeed");

        let found = repo
            .find_by_key_hash("hash-1")
            .await
            .expect("query should succeed")
            .expect("key should exist");
        assert_eq!(found.last_used_at, Some(1234567890));
    }
}
