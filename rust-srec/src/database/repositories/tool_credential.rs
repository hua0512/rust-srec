//! `tool_credentials` table access.
//!
//! Writes come from a tool's login routes (save on remembered login, delete
//! on logout); reads serve `has_stored_credentials` and the automatic
//! re-login path (`crate::baidupcs::StoredLoginAuthenticator`). Single row
//! per `(tool, account_key)`.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::WritePool;
use crate::database::models::ToolCredentialDbModel;
use crate::database::retry::retry_on_sqlite_busy;

const UPSERT_SQL: &str = r#"
    INSERT INTO tool_credentials (
        tool, account_key, payload, created_at, updated_at
    )
    VALUES (?, ?, ?, ?, ?)
    ON CONFLICT (tool, account_key) DO UPDATE SET
        payload    = excluded.payload,
        updated_at = excluded.updated_at
"#;

const GET_SQL: &str = r#"
    SELECT tool, account_key, payload, created_at, updated_at
    FROM tool_credentials
    WHERE tool = ? AND account_key = ?
"#;

const DELETE_SQL: &str = "DELETE FROM tool_credentials WHERE tool = ? AND account_key = ?";

/// Repository for opt-in external-tool login material.
#[async_trait]
pub trait ToolCredentialRepository: Send + Sync {
    /// Stored payload for one `(tool, account_key)`, if any.
    async fn get(&self, tool: &str, account_key: &str) -> Result<Option<ToolCredentialDbModel>>;

    /// Insert or replace the payload for the model's key, keeping the
    /// original `created_at` on replace.
    async fn upsert(&self, model: &ToolCredentialDbModel) -> Result<()>;

    /// Remove the payload for one key (no-op when absent).
    async fn delete(&self, tool: &str, account_key: &str) -> Result<()>;
}

/// Sqlx implementation backed by separate read / write pools (same pattern
/// as [`crate::database::repositories::SqlxUploadRecordRepository`]).
pub struct SqlxToolCredentialRepository {
    pool: SqlitePool,
    write_pool: WritePool,
}

impl SqlxToolCredentialRepository {
    pub fn new(pool: SqlitePool, write_pool: WritePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl ToolCredentialRepository for SqlxToolCredentialRepository {
    async fn get(&self, tool: &str, account_key: &str) -> Result<Option<ToolCredentialDbModel>> {
        let row = sqlx::query_as::<_, ToolCredentialDbModel>(GET_SQL)
            .bind(tool)
            .bind(account_key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row)
    }

    async fn upsert(&self, model: &ToolCredentialDbModel) -> Result<()> {
        retry_on_sqlite_busy("upsert_tool_credentials", || async {
            sqlx::query(UPSERT_SQL)
                .bind(&model.tool)
                .bind(&model.account_key)
                .bind(&model.payload)
                .bind(model.created_at)
                .bind(model.updated_at)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn delete(&self, tool: &str, account_key: &str) -> Result<()> {
        retry_on_sqlite_busy("delete_tool_credentials", || async {
            sqlx::query(DELETE_SQL)
                .bind(tool)
                .bind(account_key)
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> SqlxToolCredentialRepository {
        let pool = crate::database::init_pool_with_size("sqlite::memory:", 1)
            .await
            .unwrap();
        crate::database::run_migrations(&pool).await.unwrap();
        SqlxToolCredentialRepository::new(pool.clone(), pool)
    }

    fn model(tool: &str, account_key: &str) -> ToolCredentialDbModel {
        ToolCredentialDbModel {
            tool: tool.to_string(),
            account_key: account_key.to_string(),
            payload: r#"{"cookies":"BDUSS=abc; STOKEN=DEF"}"#.to_string(),
            created_at: 1_000,
            updated_at: 1_000,
        }
    }

    #[tokio::test]
    async fn upsert_get_delete_roundtrip() {
        let repo = setup().await;

        assert!(repo.get("baidupcs", "").await.unwrap().is_none());

        repo.upsert(&model("baidupcs", "")).await.unwrap();
        let stored = repo.get("baidupcs", "").await.unwrap().expect("row stored");
        assert!(stored.payload.contains("BDUSS=abc"));
        assert_eq!(stored.created_at, 1_000);

        // Replace keeps created_at, updates the payload.
        let mut replacement = model("baidupcs", "");
        replacement.payload = r#"{"bduss":"newbduss","stoken":"NEWSTOKEN"}"#.to_string();
        replacement.created_at = 2_000;
        replacement.updated_at = 2_000;
        repo.upsert(&replacement).await.unwrap();
        let stored = repo
            .get("baidupcs", "")
            .await
            .unwrap()
            .expect("row still there");
        assert!(stored.payload.contains("newbduss"));
        assert_eq!(stored.created_at, 1_000, "created_at survives replace");
        assert_eq!(stored.updated_at, 2_000);

        repo.delete("baidupcs", "").await.unwrap();
        assert!(repo.get("baidupcs", "").await.unwrap().is_none());
        // Deleting again is a no-op.
        repo.delete("baidupcs", "").await.unwrap();
    }

    #[tokio::test]
    async fn rows_are_keyed_by_tool_and_account() {
        let repo = setup().await;
        repo.upsert(&model("baidupcs", "")).await.unwrap();
        repo.upsert(&model("baidupcs", "/app/config/BaiduPCS-Go"))
            .await
            .unwrap();
        // A different tool may reuse the same account key.
        repo.upsert(&model("othertool", "")).await.unwrap();

        repo.delete("baidupcs", "").await.unwrap();
        assert!(repo.get("baidupcs", "").await.unwrap().is_none());
        assert!(
            repo.get("baidupcs", "/app/config/BaiduPCS-Go")
                .await
                .unwrap()
                .is_some(),
            "delete only touches its own account key"
        );
        assert!(
            repo.get("othertool", "").await.unwrap().is_some(),
            "delete only touches its own tool"
        );
    }
}
