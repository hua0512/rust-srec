//! `session_events` table access.
//!
//! Two entry points:
//!
//! - [`SessionEventTxOps::insert`] — transaction-scoped helper, called from
//!   inside [`crate::database::repositories::SessionTxOps`] /
//!   [`crate::database::repositories::SessionLifecycleRepository`] so the event
//!   row is written in the same `BEGIN IMMEDIATE` boundary as the
//!   `live_sessions` row it describes.
//! - [`SessionEventRepository`] — standalone trait for non-tx writes
//!   (best-effort persistence of in-memory transitions like
//!   `hysteresis_entered`) and for the read path used by the API.
//!
//! The two write paths share [`SessionEventDbModel`] and the same single
//! SQL statement template, so divergence is impossible by construction.
//! Read paths map storage rows into [`SessionEvent`] before returning to
//! callers.
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
use crate::session::SessionEvent;

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

const LIST_BY_STREAMER_SQL: &str = r#"
    SELECT id, session_id, streamer_id, kind, occurred_at, payload
    FROM session_events
    WHERE streamer_id = ?
    ORDER BY occurred_at ASC, id ASC
"#;

/// Transaction-scoped writes. Must be called from inside an existing
/// `BEGIN IMMEDIATE` block — does not commit on its own.
pub struct SessionEventTxOps;

impl SessionEventTxOps {
    /// Insert one event row. The caller is responsible for committing the
    /// outer transaction.
    pub async fn insert(tx: &mut SqliteConnection, row: &SessionEventDbModel) -> Result<()> {
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
    async fn list_for_session(&self, session_id: &str) -> Result<Vec<SessionEvent>>;

    /// All events for a streamer, oldest first.
    async fn list_for_streamer(&self, streamer_id: &str) -> Result<Vec<SessionEvent>>;
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

    async fn list_for_session(&self, session_id: &str) -> Result<Vec<SessionEvent>> {
        let rows = sqlx::query_as::<_, SessionEventDbModel>(LIST_BY_SESSION_SQL)
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn list_for_streamer(&self, streamer_id: &str) -> Result<Vec<SessionEvent>> {
        let rows = sqlx::query_as::<_, SessionEventDbModel>(LIST_BY_STREAMER_SQL)
            .bind(streamer_id)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    use crate::database::models::{LiveSessionDbModel, StreamerDbModel};
    use crate::database::repositories::{
        SessionRepository as _, SqlxSessionRepository, SqlxStreamerRepository,
        StreamerRepository as _,
    };
    use crate::database::{init_pool_with_size, run_migrations};
    use crate::session::SessionEventPayload;

    async fn setup_pool() -> SqlitePool {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        let mut streamer = StreamerDbModel::new(
            "Streamer One",
            "https://example.com/streamer-1",
            "platform-twitch",
        );
        streamer.id = "streamer-1".to_string();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&streamer)
            .await
            .unwrap();

        let mut session = LiveSessionDbModel::new("streamer-1");
        session.id = "s1".to_string();
        session.start_time = 0;
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .create_session(&session)
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

        repo.insert(&row(
            "session_started",
            100,
            Some(r#"{"kind":"session_started","from_hysteresis":false,"title":"hi"}"#),
        ))
        .await
        .unwrap();
        repo.insert(&row(
            "hysteresis_entered",
            200,
            Some(r#"{"kind":"hysteresis_entered"}"#),
        ))
        .await
        .unwrap();
        repo.insert(&row(
            "session_ended",
            300,
            Some(r#"{"kind":"session_ended"}"#),
        ))
        .await
        .unwrap();

        let events = repo.list_for_session("s1").await.unwrap();
        assert_eq!(events.len(), 3, "all rows returned");
        assert_eq!(events[0].kind, "session_started", "ordered ASC");
        assert_eq!(events[2].kind, "session_ended");
        assert_eq!(events[1].occurred_at.timestamp_millis(), 200);
        assert!(matches!(
            events[0].payload.as_ref(),
            Some(SessionEventPayload::SessionStarted { .. })
        ));
    }

    #[tokio::test]
    async fn list_maps_malformed_payload_to_none() {
        let pool = setup_pool().await;
        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());

        repo.insert(&row("session_started", 100, Some("{bad json")))
            .await
            .unwrap();

        let events = repo.list_for_session("s1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "session_started");
        assert_eq!(events[0].occurred_at.timestamp_millis(), 100);
        assert!(
            events[0].payload.is_none(),
            "malformed JSON should degrade to a kind-only domain event"
        );
    }

    #[tokio::test]
    async fn list_for_streamer_filters_and_orders_rows() {
        let pool = setup_pool().await;
        let repo = SqlxSessionEventRepository::new(pool.clone(), pool.clone());

        let mut other_streamer = StreamerDbModel::new(
            "Streamer Two",
            "https://example.com/streamer-2",
            "platform-twitch",
        );
        other_streamer.id = "streamer-2".to_string();
        SqlxStreamerRepository::new(pool.clone(), pool.clone())
            .create_streamer(&other_streamer)
            .await
            .unwrap();

        let mut other_session = LiveSessionDbModel::new("streamer-2");
        other_session.id = "s2".to_string();
        other_session.start_time = 0;
        SqlxSessionRepository::new(pool.clone(), pool.clone())
            .create_session(&other_session)
            .await
            .unwrap();

        repo.insert(&row("session_ended", 300, None)).await.unwrap();
        repo.insert(&row("session_started", 100, None))
            .await
            .unwrap();
        repo.insert(&SessionEventDbModel {
            id: 0,
            session_id: "s2".to_string(),
            streamer_id: "streamer-2".to_string(),
            kind: "session_started".to_string(),
            occurred_at: 50,
            payload: None,
        })
        .await
        .unwrap();

        let events = repo.list_for_streamer("streamer-1").await.unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|row| row.streamer_id == "streamer-1"));
        assert_eq!(events[0].kind, "session_started");
        assert_eq!(events[1].kind, "session_ended");
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

        sqlx::query("DELETE FROM live_sessions WHERE id = 's1'")
            .execute(&pool)
            .await
            .unwrap();

        let events = repo.list_for_session("s1").await.unwrap();
        assert!(
            events.is_empty(),
            "child rows must cascade on session delete"
        );
    }
}
