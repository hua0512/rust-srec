//! Transactional operations for monitor event outbox.
//!
//! This module provides transaction-aware operations for the monitor event outbox.
//! The outbox pattern ensures that database changes and event emissions are atomic.

use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::Result;
use crate::monitor::MonitorEvent;

/// Transactional operations for monitor event outbox.
///
/// These methods operate within an existing transaction and do NOT commit.
/// The caller is responsible for committing or rolling back the transaction.
pub struct MonitorOutboxTxOps;

impl MonitorOutboxTxOps {
    /// Enqueue a monitor event into the outbox within a transaction.
    pub async fn enqueue_event(
        tx: &mut SqliteConnection,
        streamer_id: &str,
        event: &MonitorEvent,
    ) -> Result<()> {
        let payload = serde_json::to_string(event).map_err(|e| {
            crate::Error::Other(format!("Failed to serialize monitor event: {}", e))
        })?;

        let event_type = match event {
            MonitorEvent::StreamerLive { .. } => "StreamerLive",
            MonitorEvent::StreamerOffline { .. } => "StreamerOffline",
            MonitorEvent::FatalError { .. } => "FatalError",
            MonitorEvent::TransientError { .. } => "TransientError",
            MonitorEvent::StateChanged { .. } => "StateChanged",
        };

        sqlx::query(
            r#"
            INSERT INTO monitor_event_outbox (streamer_id, event_type, payload, created_at)
            VALUES (?, ?, ?, ?)
            "#,
        )
        .bind(streamer_id)
        .bind(event_type)
        .bind(payload)
        .bind(crate::database::time::now_ms())
        .execute(tx)
        .await?;

        Ok(())
    }
}

/// Non-transactional outbox operations (for the publisher).
pub struct MonitorOutboxOps;

impl MonitorOutboxOps {
    /// Fetch undelivered events from the outbox.
    pub async fn fetch_undelivered(pool: &SqlitePool, limit: i32) -> Result<Vec<OutboxEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT id, payload, created_at, attempts
            FROM monitor_event_outbox
            WHERE delivered_at IS NULL
            ORDER BY id
            LIMIT ?
            "#,
        )
        .bind(limit)
        .fetch_all(pool)
        .await?;

        let entries = rows
            .into_iter()
            .map(|row| OutboxEntry {
                id: row.get("id"),
                payload: row.get("payload"),
                created_at: row.get("created_at"),
                attempts: row.get("attempts"),
            })
            .collect();

        Ok(entries)
    }

    /// Mark an event as delivered.
    pub async fn mark_delivered(pool: &SqlitePool, id: i64) -> Result<()> {
        let now = crate::database::time::now_ms();

        sqlx::query(
            "UPDATE monitor_event_outbox SET delivered_at = ?, attempts = attempts + 1, last_error = NULL WHERE id = ?",
        )
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Record a delivery failure.
    pub async fn record_failure(pool: &SqlitePool, id: i64, error: &str) -> Result<()> {
        sqlx::query(
            "UPDATE monitor_event_outbox SET attempts = attempts + 1, last_error = ? WHERE id = ?",
        )
        .bind(error)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Mark multiple events as delivered in a single transaction.
    pub async fn mark_delivered_batch(pool: &SqlitePool, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let now = crate::database::time::now_ms();
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE monitor_event_outbox SET delivered_at = ?, attempts = attempts + 1, last_error = NULL WHERE id IN ({})",
            placeholders
        );

        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(now);
        for id in ids {
            query = query.bind(id);
        }
        query.execute(pool).await?;

        Ok(())
    }

    pub async fn record_failure_batch(pool: &SqlitePool, failures: &[(i64, String)]) -> Result<()> {
        if failures.is_empty() {
            return Ok(());
        }

        let when_clauses = failures
            .iter()
            .map(|_| " WHEN ? THEN ?")
            .collect::<Vec<_>>()
            .join("");
        let id_placeholders = failures.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!(
            "UPDATE monitor_event_outbox SET attempts = attempts + 1, last_error = CASE id{} ELSE last_error END WHERE id IN ({})",
            when_clauses, id_placeholders
        );

        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql));
        for (id, error) in failures {
            query = query.bind(id).bind(error);
        }

        for (id, _) in failures {
            query = query.bind(id);
        }

        query.execute(pool).await?;

        Ok(())
    }
}

/// An entry from the outbox table.
pub struct OutboxEntry {
    pub id: i64,
    pub payload: String,
    pub created_at: i64,
    pub attempts: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::StreamerDbModel;
    use crate::database::repositories::{SqlxStreamerRepository, StreamerRepository as _};
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::domain::StreamerState;
    use sqlx::SqlitePool;

    async fn setup_test_db() -> SqlitePool {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        for id in ["test-1", "s1", "s2", "s3", "s0"] {
            insert_streamer(&pool, id).await;
        }
        pool
    }

    async fn insert_streamer(pool: &SqlitePool, id: &str) {
        let mut streamer = StreamerDbModel::new(
            format!("Streamer {id}"),
            format!("https://example.com/{id}"),
            "platform-twitch",
        );
        streamer.id = id.to_string();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_enqueue_event() {
        let pool = setup_test_db().await;
        let mut tx = pool.begin().await.unwrap();

        let event = MonitorEvent::StateChanged {
            streamer_id: "test-1".to_string(),
            streamer_name: "Test".to_string(),
            old_state: StreamerState::NotLive,
            new_state: StreamerState::Live,
            reason: None,
            timestamp: chrono::Utc::now(),
        };

        MonitorOutboxTxOps::enqueue_event(&mut tx, "test-1", &event)
            .await
            .unwrap();

        tx.commit().await.unwrap();

        // Verify
        let entries = MonitorOutboxOps::fetch_undelivered(&pool, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].payload.contains("StateChanged"));
    }

    #[tokio::test]
    async fn test_mark_delivered() {
        let pool = setup_test_db().await;

        // Insert directly
        sqlx::query(
            "INSERT INTO monitor_event_outbox (streamer_id, event_type, payload, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind("test-1")
        .bind("StateChanged")
        .bind("{}")
        .bind(crate::database::time::now_ms())
        .execute(&pool)
        .await
        .unwrap();

        // Should have 1 undelivered
        let entries = MonitorOutboxOps::fetch_undelivered(&pool, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 1);
        let id = entries[0].id;

        // Mark delivered
        MonitorOutboxOps::mark_delivered(&pool, id).await.unwrap();

        // Should have 0 undelivered
        let entries = MonitorOutboxOps::fetch_undelivered(&pool, 10)
            .await
            .unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[tokio::test]
    async fn test_batch_delivery_and_failure_updates() {
        let pool = setup_test_db().await;

        for i in 0..3 {
            sqlx::query(
                "INSERT INTO monitor_event_outbox (streamer_id, event_type, payload, created_at) VALUES (?, ?, ?, ?)",
            )
            .bind(format!("s{i}"))
            .bind("StateChanged")
            .bind("{}")
            .bind(crate::database::time::now_ms())
            .execute(&pool)
            .await
            .unwrap();
        }

        let rows: Vec<(i64,)> = sqlx::query_as("SELECT id FROM monitor_event_outbox ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        let ids: Vec<i64> = rows.into_iter().map(|(id,)| id).collect();

        MonitorOutboxOps::mark_delivered_batch(&pool, &ids[..2])
            .await
            .unwrap();

        let delivered_rows: Vec<(i64, Option<i64>, i64, Option<String>)> = sqlx::query_as(
            "SELECT id, delivered_at, attempts, last_error FROM monitor_event_outbox WHERE id IN (?, ?) ORDER BY id",
        )
        .bind(ids[0])
        .bind(ids[1])
        .fetch_all(&pool)
        .await
        .unwrap();

        assert_eq!(delivered_rows.len(), 2);
        for (_, delivered_at, attempts, last_error) in delivered_rows {
            assert!(delivered_at.is_some());
            assert_eq!(attempts, 1);
            assert_eq!(last_error, None);
        }

        let failures = vec![(ids[2], "no receivers".to_string())];
        MonitorOutboxOps::record_failure_batch(&pool, &failures)
            .await
            .unwrap();

        let failed_row: (i64, Option<i64>, i64, Option<String>) = sqlx::query_as(
            "SELECT id, delivered_at, attempts, last_error FROM monitor_event_outbox WHERE id = ?",
        )
        .bind(ids[2])
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(failed_row.0, ids[2]);
        assert!(failed_row.1.is_none());
        assert_eq!(failed_row.2, 1);
        assert_eq!(failed_row.3.as_deref(), Some("no receivers"));
    }
}
