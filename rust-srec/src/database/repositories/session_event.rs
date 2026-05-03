//! `session_events` table access.
//!
//! Two entry points:
//!
//! - [`SessionEventTxOps::insert`] — transaction-scoped helper, called from
//!   inside [`crate::database::repositories::SessionTxOps`] /
//!   [`crate::session::repository::SessionLifecycleRepository`] so the event
//!   row is written in the same `BEGIN IMMEDIATE` boundary as the
//!   `live_sessions` row it describes.
//! - [`SessionEventRepository`] — standalone trait for non-tx writes
//!   (best-effort persistence of in-memory transitions like
//!   `hysteresis_entered`) and for the read path used by the API.
//!
//! The two paths share [`SessionEventDbModel`] and the same single SQL
//! statement template, so divergence is impossible by construction.
//!
//! The `kind` column is constrained at the table level
//! (`CHECK (kind IN (...))`) — typos crash at insert time rather than
//! producing rows the deserializer can't read.

use async_trait::async_trait;
use sqlx::{SqliteConnection, SqlitePool};

use crate::Result;
use crate::database::WritePool;
use crate::database::models::SessionEventDbModel;
use crate::database::retry::retry_on_sqlite_busy;

const INSERT_SQL: &str = r#"
    INSERT INTO session_events (session_id, streamer_id, kind, occurred_at, payload)
    VALUES (?, ?, ?, ?, ?)
"#;

const LIST_BY_SESSION_SQL: &str = r#"
    SELECT id, session_id, streamer_id, kind, occurred_at, payload
    FROM session_events
    WHERE session_id = ?
    ORDER BY occurred_at ASC, id ASC
"#;

/// Transaction-scoped writes. Must be called from inside an existing
/// `BEGIN IMMEDIATE` block — does not commit on its own.
pub struct SessionEventTxOps;

impl SessionEventTxOps {
    /// Insert one event row. The caller is responsible for committing the
    /// outer transaction.
    pub async fn insert(
        tx: &mut SqliteConnection,
        row: &SessionEventDbModel,
    ) -> Result<()> {
        sqlx::query(INSERT_SQL)
            .bind(&row.session_id)
            .bind(&row.streamer_id)
            .bind(&row.kind)
            .bind(row.occurred_at)
            .bind(row.payload.as_deref())
            .execute(tx)
            .await?;
        Ok(())
    }
}

/// Standalone repository for session-event writes that aren't part of an
/// existing transaction (best-effort hysteresis/resumed persistence) and for
/// reads served by the API.
#[async_trait]
pub trait SessionEventRepository: Send + Sync {
    /// Insert one event row. Best-effort: callers wrap this and log on
    /// failure rather than propagating the error, because the audit log
    /// must never block the lifecycle's in-memory transition.
    async fn insert(&self, row: &SessionEventDbModel) -> Result<()>;

    /// All events for a session, oldest first. Used by the
    /// `GET /api/sessions/{id}` handler to build the timeline payload.
    async fn list_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionEventDbModel>>;
}

/// Sqlx implementation backed by separate read / write pools (matches the
/// pattern used by [`crate::database::repositories::session::SqlxSessionRepository`]).
pub struct SqlxSessionEventRepository {
    pool: SqlitePool,
    write_pool: WritePool,
}

impl SqlxSessionEventRepository {
    pub fn new(pool: SqlitePool, write_pool: WritePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl SessionEventRepository for SqlxSessionEventRepository {
    async fn insert(&self, row: &SessionEventDbModel) -> Result<()> {
        retry_on_sqlite_busy("insert_session_event", || async {
            sqlx::query(INSERT_SQL)
                .bind(&row.session_id)
                .bind(&row.streamer_id)
                .bind(&row.kind)
                .bind(row.occurred_at)
                .bind(row.payload.as_deref())
                .execute(&self.write_pool)
                .await?;
            Ok(())
        })
        .await
    }

    async fn list_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionEventDbModel>> {
        let rows = sqlx::query_as::<_, SessionEventDbModel>(LIST_BY_SESSION_SQL)
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Minimal schema: just `live_sessions` (so the FK target exists) and
        // `session_events`. Mirrors the columns defined in
        // `migrations/20260428001500_add_session_events.sql`.
        sqlx::query(
            r#"CREATE TABLE live_sessions (
                id TEXT PRIMARY KEY,
                streamer_id TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                end_time INTEGER,
                titles TEXT,
                danmu_statistics_id TEXT,
                total_size_bytes INTEGER DEFAULT 0
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                streamer_id TEXT NOT NULL,
                kind TEXT NOT NULL CHECK (kind IN (
                    'session_started',
                    'hysteresis_entered',
                    'session_resumed',
                    'session_ended'
                )),
                occurred_at INTEGER NOT NULL,
                payload TEXT,
                FOREIGN KEY (session_id) REFERENCES live_sessions(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO live_sessions (id, streamer_id, start_time)
               VALUES ('s1', 'streamer-1', 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn row(kind: &str, occurred_at: i64, payload: Option<&str>) -> SessionEventDbModel {
        SessionEventDbModel {
            id: 0,
            session_id: "s1".to_string(),
            streamer_id: "streamer-1".to_string(),
            kind: kind.to_string(),
            occurred_at,
            payload: payload.map(|s| s.to_string()),
        }
    }

    #[tokio::test]
    async fn insert_and_list_round_trip() {
        let pool = setup_pool().await;
        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());

        repo.insert(&row("session_started", 100, Some(r#"{"kind":"session_started","from_hysteresis":false,"title":"hi"}"#)))
            .await
            .unwrap();
        repo.insert(&row("hysteresis_entered", 200, Some(r#"{"kind":"hysteresis_entered"}"#)))
            .await
            .unwrap();
        repo.insert(&row("session_ended", 300, Some(r#"{"kind":"session_ended"}"#)))
            .await
            .unwrap();

        let events = repo.list_for_session("s1").await.unwrap();
        assert_eq!(events.len(), 3, "all rows returned");
        assert_eq!(events[0].kind, "session_started", "ordered ASC");
        assert_eq!(events[2].kind, "session_ended");
        assert_eq!(events[1].occurred_at, 200);
    }

    #[tokio::test]
    async fn insert_rejects_unknown_kind() {
        let pool = setup_pool().await;
        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());

        // CHECK constraint must reject typos at insert time — defends the
        // discriminated union from drift.
        let err = repo.insert(&row("session_typo", 100, None)).await;
        assert!(err.is_err(), "CHECK constraint must reject unknown kinds");
    }

    #[tokio::test]
    async fn tx_ops_insert_uses_outer_transaction() {
        let pool = setup_pool().await;

        let mut tx = pool.begin().await.unwrap();
        SessionEventTxOps::insert(
            &mut tx,
            &row("session_started", 50, Some(r#"{"kind":"session_started"}"#)),
        )
        .await
        .unwrap();
        // Roll back — assert the row is gone (proves the helper participates
        // in the caller's tx instead of auto-committing).
        tx.rollback().await.unwrap();

        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());
        let events = repo.list_for_session("s1").await.unwrap();
        assert!(events.is_empty(), "rolled-back row must not be visible");
    }

    #[tokio::test]
    async fn cascade_delete_when_session_removed() {
        let pool = setup_pool().await;
        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());

        // Seed an event, enable foreign keys (SQLite needs this per-connection),
        // then delete the session and verify the cascade fires.
        repo.insert(&row("session_started", 10, None))
            .await
            .unwrap();

        // SQLite requires `PRAGMA foreign_keys = ON` per connection for the
        // cascade to be enforced. The production pool sets this at connect
        // time; the test pool needs it explicitly here.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM live_sessions WHERE id = 's1'")
            .execute(&pool)
            .await
            .unwrap();

        let events = repo.list_for_session("s1").await.unwrap();
        assert!(events.is_empty(), "child rows must cascade on session delete");
    }
}
