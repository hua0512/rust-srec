//! `streamer_check_history` table access.
//!
//! Append-only ring buffer of monitor poll outcomes. The writer task
//! (assembled in `crate::services`) feeds rows here best-effort — a failed
//! insert logs and is dropped, never propagated, because diagnostic
//! telemetry must not block the polling hot path.
//!
//! The retention trim runs in the same statement batch as the insert so the
//! per-streamer row count converges to `KEEP_PER_STREAMER` even under
//! restarts, without needing a separate maintenance job.

use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::Result;
use crate::database::WritePool;
use crate::database::models::StreamerCheckHistoryDbModel;
use crate::database::retry::retry_on_sqlite_busy;

/// Per-streamer retention cap. The writer trims to this many most-recent
/// rows on every insert. The API caps `?limit=` at the same value, so a
/// request that asks for everything sees exactly what's persisted.
///
/// Sized for the screenshot's "HISTORY (60PTS)" UI plus a 3× debug headroom:
/// operators occasionally want more context when investigating extractor
/// flakiness, but per-streamer rows × ~200 B × N streamers stays well under
/// a few MB at typical deployment sizes.
pub const KEEP_PER_STREAMER: i64 = 200;

const INSERT_SQL: &str = r#"
    INSERT INTO streamer_check_history (
        streamer_id,
        checked_at,
        duration_ms,
        outcome,
        fatal_kind,
        filter_reason,
        error_message,
        streams_extracted,
        stream_selected,
        streams_extracted_json,
        title,
        category,
        viewer_count
    )
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

/// Trim to the most-recent `KEEP_PER_STREAMER` rows for a given streamer.
///
/// The subquery pins the IDs we want to keep (newest first); the outer
/// DELETE removes everything else. Cheap because `idx_streamer_check_history
/// _streamer_id_checked_at` covers both the SELECT and the DELETE filter.
///
/// Run after every insert so the steady-state row count is bounded without
/// needing a periodic maintenance task.
const TRIM_SQL: &str = r#"
    DELETE FROM streamer_check_history
    WHERE streamer_id = ?1
      AND id NOT IN (
          SELECT id FROM streamer_check_history
          WHERE streamer_id = ?1
          ORDER BY checked_at DESC, id DESC
          LIMIT ?2
      )
"#;

const LIST_RECENT_SQL: &str = r#"
    SELECT id, streamer_id, checked_at, duration_ms, outcome,
           fatal_kind, filter_reason, error_message,
           streams_extracted, stream_selected, streams_extracted_json,
           title, category, viewer_count
    FROM streamer_check_history
    WHERE streamer_id = ?
    ORDER BY checked_at DESC, id DESC
    LIMIT ?
"#;

/// Repository for the check-history table.
///
/// Reads serve the API (`GET /api/streamers/:id/check-history`); writes are
/// best-effort and called only from the writer task.
#[async_trait]
pub trait StreamerCheckHistoryRepository: Send + Sync {
    /// Insert one row and trim the streamer's history to
    /// [`KEEP_PER_STREAMER`] in the same logical write. Best-effort: the
    /// caller wraps this and logs on failure rather than propagating, because
    /// the diagnostic strip must never block the lifecycle's polling loop.
    async fn insert(&self, row: &StreamerCheckHistoryDbModel) -> Result<()>;

    /// Most-recent `limit` rows for a streamer, newest first. The handler
    /// reverses to oldest-first before returning to the client so the UI
    /// renders left → right = past → now without re-sorting.
    async fn list_recent(
        &self,
        streamer_id: &str,
        limit: i64,
    ) -> Result<Vec<StreamerCheckHistoryDbModel>>;
}

/// Sqlx implementation backed by separate read / write pools (matches the
/// pattern used by [`crate::database::repositories::SqlxSessionEventRepository`]).
pub struct SqlxStreamerCheckHistoryRepository {
    pool: SqlitePool,
    write_pool: WritePool,
}

impl SqlxStreamerCheckHistoryRepository {
    pub fn new(pool: SqlitePool, write_pool: WritePool) -> Self {
        Self { pool, write_pool }
    }
}

#[async_trait]
impl StreamerCheckHistoryRepository for SqlxStreamerCheckHistoryRepository {
    async fn insert(&self, row: &StreamerCheckHistoryDbModel) -> Result<()> {
        retry_on_sqlite_busy("insert_streamer_check_history", || async {
            // Use a tx so the insert + trim are atomic — otherwise a reader
            // snapping in between would briefly see KEEP_PER_STREAMER+1 rows.
            // Cheap because both statements hit the same covering index.
            let mut tx = self.write_pool.begin().await?;
            sqlx::query(INSERT_SQL)
                .bind(&row.streamer_id)
                .bind(row.checked_at)
                .bind(row.duration_ms)
                .bind(&row.outcome)
                .bind(row.fatal_kind.as_deref())
                .bind(row.filter_reason.as_deref())
                .bind(row.error_message.as_deref())
                .bind(row.streams_extracted)
                .bind(row.stream_selected.as_deref())
                .bind(row.streams_extracted_json.as_deref())
                .bind(row.title.as_deref())
                .bind(row.category.as_deref())
                .bind(row.viewer_count)
                .execute(&mut *tx)
                .await?;
            sqlx::query(TRIM_SQL)
                .bind(&row.streamer_id)
                .bind(KEEP_PER_STREAMER)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(())
        })
        .await
    }

    async fn list_recent(
        &self,
        streamer_id: &str,
        limit: i64,
    ) -> Result<Vec<StreamerCheckHistoryDbModel>> {
        // Server-side guard: even if the handler forgets to clamp, never let
        // a single query pull unbounded rows.
        let clamped = limit.clamp(1, KEEP_PER_STREAMER);
        let rows = sqlx::query_as::<_, StreamerCheckHistoryDbModel>(LIST_RECENT_SQL)
            .bind(streamer_id)
            .bind(clamped)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::models::streamer_check_history::outcome;
    use sqlx::SqlitePool;

    async fn setup_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        // Minimal schema: just `streamers` (so the FK target exists) and
        // `streamer_check_history`. Mirrors the columns defined in
        // `migrations/20260503120000_add_streamer_check_history.sql`.
        sqlx::query(
            r#"CREATE TABLE streamers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"CREATE TABLE streamer_check_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                streamer_id TEXT NOT NULL,
                checked_at INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                outcome TEXT NOT NULL CHECK (outcome IN (
                    'live','offline','filtered','transient_error','fatal_error'
                )),
                fatal_kind TEXT,
                filter_reason TEXT,
                error_message TEXT,
                streams_extracted INTEGER NOT NULL DEFAULT 0,
                stream_selected TEXT,
                streams_extracted_json TEXT,
                title TEXT,
                category TEXT,
                viewer_count INTEGER,
                FOREIGN KEY (streamer_id) REFERENCES streamers(id) ON DELETE CASCADE
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO streamers (id, name) VALUES ('s1','Alice')")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    fn row(outcome_str: &str, checked_at: i64) -> StreamerCheckHistoryDbModel {
        StreamerCheckHistoryDbModel {
            id: 0,
            streamer_id: "s1".to_string(),
            checked_at,
            duration_ms: 42,
            outcome: outcome_str.to_string(),
            fatal_kind: None,
            filter_reason: None,
            error_message: None,
            streams_extracted: 0,
            stream_selected: None,
            streams_extracted_json: None,
            title: None,
            category: None,
            viewer_count: None,
        }
    }

    #[tokio::test]
    async fn insert_and_list_round_trip_each_outcome() {
        let pool = setup_pool().await;
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        // Insert one row per outcome to assert the CHECK constraint accepts
        // every documented variant — guards against the migration and the
        // `outcome` constants drifting apart.
        for (idx, kind) in outcome::ALL.iter().enumerate() {
            repo.insert(&row(kind, 100 + idx as i64)).await.unwrap();
        }

        let rows = repo.list_recent("s1", 10).await.unwrap();
        assert_eq!(rows.len(), outcome::ALL.len());
        // Newest-first ordering.
        assert_eq!(rows[0].outcome, outcome::FATAL_ERROR);
        assert_eq!(rows.last().unwrap().outcome, outcome::LIVE);
    }

    #[tokio::test]
    async fn insert_rejects_unknown_outcome() {
        let pool = setup_pool().await;
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        // CHECK constraint must reject typos at insert time — defends the
        // discriminated union from drift.
        let err = repo.insert(&row("typo", 100)).await;
        assert!(
            err.is_err(),
            "CHECK constraint must reject unknown outcomes"
        );
    }

    #[tokio::test]
    async fn insert_trims_to_keep_per_streamer() {
        let pool = setup_pool().await;
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        // Insert KEEP_PER_STREAMER + 5 rows; the trim should converge to
        // exactly KEEP_PER_STREAMER, keeping the most-recent ones.
        let total = KEEP_PER_STREAMER + 5;
        for i in 0..total {
            repo.insert(&row(outcome::LIVE, i)).await.unwrap();
        }

        let rows = repo
            .list_recent("s1", KEEP_PER_STREAMER + 50)
            .await
            .unwrap();
        assert_eq!(
            rows.len() as i64,
            KEEP_PER_STREAMER,
            "writer must trim to KEEP_PER_STREAMER on insert"
        );
        // Newest first. The five oldest rows (checked_at 0..5) must be gone.
        assert_eq!(rows[0].checked_at, total - 1);
        assert_eq!(
            rows.last().unwrap().checked_at,
            total - KEEP_PER_STREAMER,
            "the oldest 5 rows must have been trimmed"
        );
    }

    #[tokio::test]
    async fn list_recent_clamps_limit_to_cap() {
        let pool = setup_pool().await;
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        for i in 0..10 {
            repo.insert(&row(outcome::OFFLINE, i)).await.unwrap();
        }

        // Asking for more than the cap is allowed; the handler may forget
        // to clamp, so the repo enforces an upper bound itself.
        let rows = repo
            .list_recent("s1", KEEP_PER_STREAMER * 10)
            .await
            .unwrap();
        assert_eq!(rows.len(), 10);

        // Asking for zero clamps up to one — the handler shouldn't pass 0,
        // but the repo must not return an unbounded query either way.
        let rows = repo.list_recent("s1", 0).await.unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[tokio::test]
    async fn isolated_per_streamer() {
        let pool = setup_pool().await;
        sqlx::query("INSERT INTO streamers (id, name) VALUES ('s2','Bob')")
            .execute(&pool)
            .await
            .unwrap();
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        repo.insert(&row(outcome::LIVE, 100)).await.unwrap();
        let mut s2 = row(outcome::OFFLINE, 200);
        s2.streamer_id = "s2".to_string();
        repo.insert(&s2).await.unwrap();

        let s1_rows = repo.list_recent("s1", 50).await.unwrap();
        assert_eq!(s1_rows.len(), 1);
        assert_eq!(s1_rows[0].outcome, outcome::LIVE);

        let s2_rows = repo.list_recent("s2", 50).await.unwrap();
        assert_eq!(s2_rows.len(), 1);
        assert_eq!(s2_rows[0].outcome, outcome::OFFLINE);
    }

    #[tokio::test]
    async fn cascade_delete_when_streamer_removed() {
        let pool = setup_pool().await;
        let repo = SqlxStreamerCheckHistoryRepository::new(pool.clone(), pool.clone());

        repo.insert(&row(outcome::LIVE, 100)).await.unwrap();

        // SQLite needs `PRAGMA foreign_keys = ON` per connection for the
        // cascade to fire. Production sets it at connect time; the in-memory
        // test pool needs it explicitly.
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM streamers WHERE id = 's1'")
            .execute(&pool)
            .await
            .unwrap();

        let rows = repo.list_recent("s1", 50).await.unwrap();
        assert!(
            rows.is_empty(),
            "child rows must cascade on streamer delete"
        );
    }
}
