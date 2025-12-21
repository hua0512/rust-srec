//! Transactional operations for streamers.
//!
//! This module provides transaction-aware operations for streamer state management.
//! Use these when you need to update streamer state as part of a larger transaction
//! (e.g., session management, outbox events).

use chrono::{DateTime, Utc};
use sqlx::SqliteConnection;

use crate::Result;

/// Transactional operations for streamers.
///
/// These methods operate within an existing transaction and do NOT commit.
/// The caller is responsible for committing or rolling back the transaction.
pub struct StreamerTxOps;

impl StreamerTxOps {
    /// Update streamer state within a transaction.
    pub async fn update_state(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        state: &str,
    ) -> Result<u64> {
        let result = sqlx::query("UPDATE streamers SET state = ? WHERE id = ?")
            .bind(state)
            .bind(streamer_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// Set streamer to LIVE state and record success (clear errors, update last_live_time).
    pub async fn set_live(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        last_live_time: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query(
            r#"
            UPDATE streamers
            SET state = 'LIVE',
                consecutive_error_count = 0,
                disabled_until = NULL,
                last_error = NULL,
                last_live_time = ?
            WHERE id = ?
            "#,
        )
        .bind(last_live_time.to_rfc3339())
        .bind(streamer_id)
        .execute(tx)
        .await?;

        Ok(result.rows_affected())
    }

    /// Set streamer to NOT_LIVE state.
    pub async fn set_offline(tx: &mut SqliteConnection, streamer_id: &str) -> Result<u64> {
        let result = sqlx::query("UPDATE streamers SET state = 'NOT_LIVE' WHERE id = ?")
            .bind(streamer_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// Update streamer avatar within a transaction.
    pub async fn update_avatar(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        avatar_url: &str,
    ) -> Result<u64> {
        let result = sqlx::query("UPDATE streamers SET avatar = ? WHERE id = ?")
            .bind(avatar_url)
            .bind(streamer_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// Increment error count and set last_error.
    ///
    /// Returns the new error count.
    pub async fn increment_error(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        error_message: &str,
    ) -> Result<i32> {
        // Increment error count and set last_error
        let affected = sqlx::query(
            r#"
            UPDATE streamers
            SET consecutive_error_count = COALESCE(consecutive_error_count, 0) + 1,
                last_error = ?
            WHERE id = ?
            "#,
        )
        .bind(error_message)
        .bind(streamer_id)
        .execute(&mut *tx)
        .await?;

        if affected.rows_affected() == 0 {
            return Err(crate::Error::not_found("streamer", streamer_id));
        }

        // Get the new error count
        let row = sqlx::query(
            "SELECT COALESCE(consecutive_error_count, 0) AS cnt FROM streamers WHERE id = ?",
        )
        .bind(streamer_id)
        .fetch_one(tx)
        .await?;

        let new_count: i32 = sqlx::Row::get(&row, "cnt");

        Ok(new_count)
    }

    /// Set disabled_until for temporary backoff.
    ///
    /// When a timestamp is provided, also sets state to TEMPORAL_DISABLED.
    /// When clearing (None), sets state back to NOT_LIVE.
    pub async fn set_disabled_until(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        disabled_until: Option<DateTime<Utc>>,
    ) -> Result<u64> {
        let disabled_until_str = disabled_until.map(|dt| dt.to_rfc3339());
        let state = if disabled_until.is_some() {
            "TEMPORAL_DISABLED"
        } else {
            "NOT_LIVE"
        };

        let result = sqlx::query("UPDATE streamers SET disabled_until = ?, state = ? WHERE id = ?")
            .bind(disabled_until_str.as_deref())
            .bind(state)
            .bind(streamer_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }

    /// Clear error state (reset consecutive_error_count, disabled_until, last_error).
    pub async fn clear_error_state(tx: &mut SqliteConnection, streamer_id: &str) -> Result<u64> {
        let result = sqlx::query(
            r#"
            UPDATE streamers
            SET consecutive_error_count = 0,
                disabled_until = NULL,
                last_error = NULL
            WHERE id = ?
            "#,
        )
        .bind(streamer_id)
        .execute(tx)
        .await?;

        Ok(result.rows_affected())
    }

    /// Set a fatal error state (NOT_FOUND, FATAL_ERROR, etc.).
    pub async fn set_fatal_error(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        state: &str,
    ) -> Result<u64> {
        let result = sqlx::query("UPDATE streamers SET state = ? WHERE id = ?")
            .bind(state)
            .bind(streamer_id)
            .execute(tx)
            .await?;

        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        sqlx::query(
            r#"
            CREATE TABLE streamers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                url TEXT NOT NULL,
                platform_config_id TEXT NOT NULL,
                template_config_id TEXT,
                state TEXT NOT NULL DEFAULT 'NOT_LIVE',
                priority TEXT NOT NULL DEFAULT 'NORMAL',
                avatar TEXT,
                consecutive_error_count INTEGER DEFAULT 0,
                last_error TEXT,
                disabled_until TEXT,
                last_live_time TEXT
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        // Insert test streamer
        sqlx::query(
            r#"
            INSERT INTO streamers (id, name, url, platform_config_id, state)
            VALUES ('test-1', 'Test Streamer', 'https://example.com/test', 'twitch', 'NOT_LIVE')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        pool
    }

    #[tokio::test]
    async fn test_update_state() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let affected = StreamerTxOps::update_state(&mut tx, "test-1", "LIVE")
            .await
            .unwrap();
        assert_eq!(affected, 1);

        tx.commit().await.unwrap();

        // Verify
        let row: (String,) = sqlx::query_as("SELECT state FROM streamers WHERE id = 'test-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.0, "LIVE");
    }

    #[tokio::test]
    async fn test_set_live() {
        let pool = setup_test_db().await;

        // First set some error state
        sqlx::query(
            "UPDATE streamers SET consecutive_error_count = 3, last_error = 'some error' WHERE id = 'test-1'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let mut tx = pool.begin().await.unwrap();

        let now = Utc::now();
        let affected = StreamerTxOps::set_live(&mut tx, "test-1", now)
            .await
            .unwrap();
        assert_eq!(affected, 1);

        tx.commit().await.unwrap();

        // Verify state, error count cleared, last_live_time set
        let row: (String, i32, Option<String>) = sqlx::query_as(
            "SELECT state, consecutive_error_count, last_error FROM streamers WHERE id = 'test-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, "LIVE");
        assert_eq!(row.1, 0);
        assert!(row.2.is_none());
    }

    #[tokio::test]
    async fn test_increment_error() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let count = StreamerTxOps::increment_error(&mut tx, "test-1", "Test error")
            .await
            .unwrap();
        assert_eq!(count, 1);

        tx.commit().await.unwrap();

        // Verify
        let row: (i32, String) = sqlx::query_as(
            "SELECT consecutive_error_count, last_error FROM streamers WHERE id = 'test-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 1);
        assert_eq!(row.1, "Test error");
    }

    #[tokio::test]
    async fn test_set_disabled_until() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let until = Utc::now() + chrono::Duration::hours(1);
        StreamerTxOps::set_disabled_until(&mut tx, "test-1", Some(until))
            .await
            .unwrap();

        tx.commit().await.unwrap();

        // Verify disabled_until set AND state changed to TEMPORAL_DISABLED
        let row: (String, Option<String>) =
            sqlx::query_as("SELECT state, disabled_until FROM streamers WHERE id = 'test-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "TEMPORAL_DISABLED");
        assert!(row.1.is_some());

        // Test clearing disabled_until resets state to NOT_LIVE
        let mut tx2 = pool.begin().await.unwrap();
        StreamerTxOps::set_disabled_until(&mut tx2, "test-1", None)
            .await
            .unwrap();
        tx2.commit().await.unwrap();

        let row2: (String, Option<String>) =
            sqlx::query_as("SELECT state, disabled_until FROM streamers WHERE id = 'test-1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row2.0, "NOT_LIVE");
        assert!(row2.1.is_none());
    }
}
