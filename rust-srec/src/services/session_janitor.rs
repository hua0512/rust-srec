//! Periodic GC for "empty" recording sessions.
//!
//! Sessions whose `total_size_bytes` stayed at 0 through `end_time` set
//! never produced retained segments — every file the engine wrote was
//! below `min_segment_size_bytes` and was deleted by the small-segment
//! guard in [`crate::services::container`].
//!
//! Layered defense against empty sessions:
//!
//! 1. The actor-side fix at `streamer_actor::handle_download_ended` for
//!    [`crate::scheduler::actor::DownloadEndPolicy::Completed`] keeps the
//!    session in Hysteresis on clean engine disconnect, so connection
//!    blips no longer mint new sessions.
//! 2. [`crate::database::models::SessionFilters::include_empty`] hides
//!    residual 0-byte rows from `GET /sessions` by default — instant
//!    UI cleanup.
//! 3. **This janitor** deletes those rows from DB after a grace period,
//!    so `session_events` audit rows and any orphan `danmu_statistics`
//!    go away with them via `ON DELETE CASCADE`.
//!
//! Why a janitor and not an inline DELETE in `enter_ended_state`:
//!
//! - Preserves the `Started → Ending → (Resumed | Ended)` broadcast
//!   contract — pipeline manager, notification, and container handlers
//!   see `Ended` exactly as today; the row just disappears later.
//! - Decouples from the `DbWritePath::Skip` path in
//!   `lifecycle::on_offline_detected` where streamer-state side effects
//!   (set_offline + StreamerOffline outbox event) commit *before*
//!   `enter_ended_state` runs. Inline DELETE there would orphan the
//!   outbox event tied to a now-vanished session row.
//! - Crash-safe: the SELECT predicate is the source of truth. A missed
//!   tick across a process restart is recovered by the next tick.
//!
//! Retention defends against:
//!
//! - API queries that just observed the `Ended` broadcast and may load
//!   the row briefly (frontend dashboard refresh, session-detail link).
//! - Pipeline manager DAG creation racing the janitor — DAG INSERT
//!   happens immediately on `Ended`; janitor waits ≥ retention.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::Result;
use crate::database::DbPool;

/// Default grace period between session-end and DELETE eligibility.
///
/// 5 minutes is well above pipeline DAG creation latency (< 1 s) and any
/// reasonable UI poll cadence (< 30 s), so consumers that just observed
/// `SessionTransition::Ended` and may load the row briefly are safe.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(5 * 60);

/// Default cadence for the janitor sweep loop.
///
/// 30 minutes amortizes the DELETE cost across infrequent sweeps while
/// keeping the upper bound on row residency (retention + interval) at
/// ~35 minutes — well below any meaningful storage cost.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Minimum allowed retention window. Tighter than this defeats the
/// purpose of the grace period (consumers may still be reading the row).
const MIN_RETENTION: Duration = Duration::from_secs(60);

/// Periodic GC task that DELETEs ended sessions whose
/// `total_size_bytes == 0`.
///
/// CASCADE-on-delete handles:
/// - `media_outputs` (none for empty sessions, by definition)
/// - `danmu_statistics` (any orphan rows from a brief danmu collection)
/// - `session_segments` (none for empty sessions)
/// - `session_events` (audit rows including `hysteresis_entered`)
///
/// `job.session_id` and `dag_execution.session_id` are plain TEXT (no
/// FK), so they retain their string references — but for empty sessions,
/// no jobs/DAGs would have been created in the first place (no segments
/// to process).
pub struct SessionJanitor {
    pool: DbPool,
    retention: Duration,
    interval: Duration,
    cancellation: CancellationToken,
}

impl SessionJanitor {
    /// Construct with defaults appropriate for production
    /// ([`DEFAULT_RETENTION`], [`DEFAULT_INTERVAL`]).
    pub fn new(pool: DbPool, cancellation: CancellationToken) -> Self {
        Self::with_config(pool, DEFAULT_RETENTION, DEFAULT_INTERVAL, cancellation)
    }

    /// Construct with explicit retention + interval. Reserved for tests
    /// that need sub-second windows; production uses [`Self::new`].
    pub fn with_config(
        pool: DbPool,
        retention: Duration,
        interval: Duration,
        cancellation: CancellationToken,
    ) -> Self {
        let retention = retention.max(MIN_RETENTION);
        Self {
            pool,
            retention,
            interval,
            cancellation,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        pool: DbPool,
        retention: Duration,
        interval: Duration,
        cancellation: CancellationToken,
    ) -> Self {
        // Tests need to bypass the production MIN_RETENTION floor so
        // we can drive the predicate with sub-second windows.
        Self {
            pool,
            retention,
            interval,
            cancellation,
        }
    }

    /// Run a single sweep. Returns the number of `live_sessions` rows
    /// deleted (excludes CASCADE-deleted child rows).
    pub async fn sweep_once(&self) -> Result<u64> {
        let cutoff_ms = chrono::Utc::now().timestamp_millis()
            - i64::try_from(self.retention.as_millis()).unwrap_or(i64::MAX);

        let result = sqlx::query(
            "DELETE FROM live_sessions \
             WHERE total_size_bytes = 0 \
               AND end_time IS NOT NULL \
               AND end_time < ?",
        )
        .bind(cutoff_ms)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Spawn the periodic sweep loop. The task observes the cancellation
    /// token for graceful shutdown.
    pub fn spawn(self) -> JoinHandle<()> {
        let janitor = Arc::new(self);
        let janitor_for_task = Arc::clone(&janitor);
        tokio::spawn(async move {
            info!(
                retention_secs = janitor_for_task.retention.as_secs(),
                interval_secs = janitor_for_task.interval.as_secs(),
                "SessionJanitor started"
            );
            // Run an immediate first sweep on startup to catch rows left
            // behind by the previous process — e.g., the binary crashed
            // between the `Ended` broadcast and the next-scheduled sweep.
            janitor_for_task.run_sweep_logged().await;
            loop {
                tokio::select! {
                    _ = janitor_for_task.cancellation.cancelled() => {
                        debug!("SessionJanitor shutting down");
                        return;
                    }
                    _ = tokio::time::sleep(janitor_for_task.interval) => {
                        janitor_for_task.run_sweep_logged().await;
                    }
                }
            }
        })
    }

    async fn run_sweep_logged(&self) {
        match self.sweep_once().await {
            Ok(0) => {
                debug!("SessionJanitor sweep: no empty sessions to delete");
            }
            Ok(count) => {
                info!(deleted = count, "SessionJanitor sweep: deleted empty sessions");
            }
            Err(e) => {
                warn!(error = %e, "SessionJanitor sweep failed; will retry on next tick");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{init_pool, run_migrations};

    async fn setup_pool() -> DbPool {
        let pool = init_pool("sqlite::memory:")
            .await
            .expect("Failed to create test pool");
        run_migrations(&pool)
            .await
            .expect("Failed to run migrations");
        pool
    }

    async fn setup_streamer(pool: &DbPool) -> String {
        let platform_id = uuid::Uuid::new_v4().to_string();
        let platform_name = format!("test_platform_{}", uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO platform_config (id, platform_name, fetch_delay_ms, download_delay_ms)
             VALUES (?, ?, 60000, 1000)",
        )
        .bind(&platform_id)
        .bind(&platform_name)
        .execute(pool)
        .await
        .expect("Failed to insert platform config");

        let streamer_id = uuid::Uuid::new_v4().to_string();
        let streamer_url = format!("https://example.com/test_{}", uuid::Uuid::new_v4());
        sqlx::query(
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority)
             VALUES (?, 'TestStreamer', ?, ?, 'NOT_LIVE', 'NORMAL')",
        )
        .bind(&streamer_id)
        .bind(&streamer_url)
        .bind(&platform_id)
        .execute(pool)
        .await
        .expect("Failed to insert streamer");

        streamer_id
    }

    async fn insert_session(
        pool: &DbPool,
        streamer_id: &str,
        total_size_bytes: i64,
        end_time_ms: Option<i64>,
    ) -> String {
        let session_id = uuid::Uuid::new_v4().to_string();
        let start_ms = chrono::Utc::now().timestamp_millis() - 1000;
        sqlx::query(
            "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, total_size_bytes)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&session_id)
        .bind(streamer_id)
        .bind(start_ms)
        .bind(end_time_ms)
        .bind(total_size_bytes)
        .execute(pool)
        .await
        .expect("Failed to insert session");
        session_id
    }

    #[tokio::test]
    async fn sweep_deletes_empty_ended_session_past_retention() {
        let pool = setup_pool().await;
        let streamer_id = setup_streamer(&pool).await;

        let now_ms = chrono::Utc::now().timestamp_millis();
        // Empty + ended 1 hour ago — past retention (10 ms).
        let stale_empty =
            insert_session(&pool, &streamer_id, 0, Some(now_ms - 60 * 60 * 1000)).await;
        // Empty + ended just now — within retention.
        let fresh_empty = insert_session(&pool, &streamer_id, 0, Some(now_ms)).await;
        // Real recording, ended 1 hour ago — past retention but has bytes.
        let real_recording =
            insert_session(&pool, &streamer_id, 1_500_000_000, Some(now_ms - 60 * 60 * 1000))
                .await;

        let janitor = SessionJanitor::for_test(
            pool.clone(),
            Duration::from_millis(10),
            Duration::from_secs(60),
            CancellationToken::new(),
        );
        let deleted = janitor.sweep_once().await.expect("sweep failed");
        assert_eq!(deleted, 1, "only the stale empty session must be deleted");

        // Verify exactly the stale empty row is gone, others remain.
        let surviving: Vec<String> =
            sqlx::query_scalar("SELECT id FROM live_sessions WHERE streamer_id = ?")
                .bind(&streamer_id)
                .fetch_all(&pool)
                .await
                .expect("Failed to read sessions");
        assert!(!surviving.contains(&stale_empty));
        assert!(surviving.contains(&fresh_empty));
        assert!(surviving.contains(&real_recording));
    }

    #[tokio::test]
    async fn sweep_keeps_active_empty_session() {
        let pool = setup_pool().await;
        let streamer_id = setup_streamer(&pool).await;

        // Active (end_time IS NULL) with zero bytes — still in the brief
        // window between LIVE detection and the first retained segment.
        let active = insert_session(&pool, &streamer_id, 0, None).await;

        let janitor = SessionJanitor::for_test(
            pool.clone(),
            Duration::from_millis(10),
            Duration::from_secs(60),
            CancellationToken::new(),
        );
        let deleted = janitor.sweep_once().await.expect("sweep failed");
        assert_eq!(deleted, 0, "active sessions must never be deleted");

        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM live_sessions WHERE id = ?)")
                .bind(&active)
                .fetch_one(&pool)
                .await
                .expect("Failed to query existence");
        assert!(exists);
    }

    #[tokio::test]
    async fn sweep_cascade_deletes_session_events() {
        let pool = setup_pool().await;
        let streamer_id = setup_streamer(&pool).await;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let stale_empty =
            insert_session(&pool, &streamer_id, 0, Some(now_ms - 60 * 60 * 1000)).await;

        // Attach a session_events row. `id` is INTEGER PRIMARY KEY
        // AUTOINCREMENT, so we don't bind it.
        sqlx::query(
            "INSERT INTO session_events \
                 (session_id, streamer_id, kind, payload, occurred_at) \
             VALUES (?, ?, 'session_ended', '{}', ?)",
        )
        .bind(&stale_empty)
        .bind(&streamer_id)
        .bind(now_ms - 60 * 60 * 1000)
        .execute(&pool)
        .await
        .expect("Failed to insert session_event");

        let janitor = SessionJanitor::for_test(
            pool.clone(),
            Duration::from_millis(10),
            Duration::from_secs(60),
            CancellationToken::new(),
        );
        let deleted = janitor.sweep_once().await.expect("sweep failed");
        assert_eq!(deleted, 1);

        let event_rows: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_events WHERE session_id = ?")
                .bind(&stale_empty)
                .fetch_one(&pool)
                .await
                .expect("Failed to count events");
        assert_eq!(
            event_rows, 0,
            "ON DELETE CASCADE must remove session_events alongside the parent row"
        );
    }

    /// Repeated sweeps are idempotent: a second sweep right after the
    /// first does nothing (no rows match).
    #[tokio::test]
    async fn sweep_is_idempotent() {
        let pool = setup_pool().await;
        let streamer_id = setup_streamer(&pool).await;

        let now_ms = chrono::Utc::now().timestamp_millis();
        insert_session(&pool, &streamer_id, 0, Some(now_ms - 60 * 60 * 1000)).await;

        let janitor = SessionJanitor::for_test(
            pool.clone(),
            Duration::from_millis(10),
            Duration::from_secs(60),
            CancellationToken::new(),
        );
        assert_eq!(janitor.sweep_once().await.unwrap(), 1);
        assert_eq!(janitor.sweep_once().await.unwrap(), 0);
    }
}
