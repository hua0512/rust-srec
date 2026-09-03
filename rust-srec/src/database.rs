//! Database module for rust-srec.
//!
//! This module provides the persistence layer using SQLite with sqlx.
//! It includes connection pool management, models, repositories, and maintenance.

pub mod batching;
pub mod maintenance;
pub mod models;
pub mod repositories;
pub mod retry;
pub mod time;

// Re-export commonly used types
pub use batching::{BatchWriter, BatchWriterConfig, JobStatusUpdate, StatsUpdate};
pub use maintenance::{MaintenanceConfig, MaintenanceScheduler};

use std::str::FromStr;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, Pool, Row, Sqlite};

/// Database connection pool type alias.
pub type DbPool = Pool<Sqlite>;

/// Serialized write pool type alias (max_connections=1).
pub type WritePool = Pool<Sqlite>;

/// Default connection pool size.
const DEFAULT_POOL_SIZE: u32 = 10;

/// Default busy timeout in milliseconds.
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 30_000;

/// Default cache size in KB (64MB = 65536 KB, but SQLite uses pages, so we use -64000 for 64MB).
const DEFAULT_CACHE_SIZE_KB: i32 = -64000;

/// Default WAL auto-checkpoint threshold in pages.
/// With a typical 4KB page size, 1000 pages is ~4MB.
const DEFAULT_WAL_AUTOCHECKPOINT_PAGES: i32 = 1000;

/// Limit WAL size growth (bytes).
const DEFAULT_JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024; // 64MB

async fn apply_per_connection_pragmas(
    conn: &mut sqlx::SqliteConnection,
) -> Result<(), sqlx::Error> {
    // Ensure WAL auto-checkpoint is enabled to avoid unbounded WAL growth.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA wal_autocheckpoint = {}",
        DEFAULT_WAL_AUTOCHECKPOINT_PAGES
    )))
    .execute(&mut *conn)
    .await?;

    // Cap WAL/journal size growth to reduce disk usage under write-heavy workloads.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA journal_size_limit = {}",
        DEFAULT_JOURNAL_SIZE_LIMIT_BYTES
    )))
    .execute(&mut *conn)
    .await?;

    // Set cache size (64MB)
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA cache_size = {}",
        DEFAULT_CACHE_SIZE_KB
    )))
    .execute(&mut *conn)
    .await?;

    // Enable memory-mapped I/O for better performance
    sqlx::query("PRAGMA mmap_size = 268435456") // 256MB
        .execute(&mut *conn)
        .await?;

    // Set temp store to memory
    sqlx::query("PRAGMA temp_store = MEMORY")
        .execute(&mut *conn)
        .await?;

    Ok(())
}

async fn ensure_wal_mode(pool: &DbPool, pool_name: &str) -> Result<(), sqlx::Error> {
    let mut conn = pool.acquire().await?;
    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&mut *conn)
        .await?;
    let mode: String = row.get(0);
    if mode != "wal" && mode != "memory" {
        tracing::warn!(
            "{}_journal_mode was '{}', expected 'wal'; re-setting",
            pool_name,
            mode
        );
        sqlx::query("PRAGMA journal_mode = WAL")
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

fn sqlite_connect_options(database_url: &str) -> Result<SqliteConnectOptions, sqlx::Error> {
    Ok(SqliteConnectOptions::from_str(database_url)?
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))
        .foreign_keys(true)
        .create_if_missing(true))
}

async fn establish_wal_mode(database_url: &str) -> Result<(), sqlx::Error> {
    let options = sqlite_connect_options(database_url)?.journal_mode(SqliteJournalMode::Wal);
    let connection = sqlx::SqliteConnection::connect_with(&options).await?;
    connection.close().await
}

/// Compute a sensible default read pool size based on available CPU cores.
///
/// SQLite readers don't benefit much beyond ~10 connections, and on low-core
/// machines (e.g. 2-core desktop) a smaller pool avoids unnecessary overhead.
pub fn default_read_pool_size() -> u32 {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(2);
    (cores * 2).min(DEFAULT_POOL_SIZE)
}

async fn open_pool_with_size(
    database_url: &str,
    max_connections: u32,
) -> Result<DbPool, sqlx::Error> {
    let connect_options = sqlite_connect_options(database_url)?;

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(|conn, _meta| {
            Box::pin(async move { apply_per_connection_pragmas(&mut *conn).await })
        })
        .connect_with(connect_options)
        .await?;

    ensure_wal_mode(&pool, "read_pool").await?;

    tracing::info!(
        "Database pool initialized with WAL mode, {} max connections",
        max_connections
    );

    Ok(pool)
}

/// Initialize the database connection pool with WAL mode and performance optimizations.
///
/// # Arguments
/// * `database_url` - SQLite database URL (e.g., "sqlite:srec.db?mode=rwc")
/// * `max_connections` - Maximum number of connections in the pool
///
/// # Returns
/// A configured SQLite connection pool.
pub async fn init_pool_with_size(
    database_url: &str,
    max_connections: u32,
) -> Result<DbPool, sqlx::Error> {
    establish_wal_mode(database_url).await?;
    open_pool_with_size(database_url, max_connections).await
}

/// Initialize the database connection pool with default size.
pub async fn init_pool(database_url: &str) -> Result<DbPool, sqlx::Error> {
    init_pool_with_size(database_url, default_read_pool_size()).await
}

/// Initialize a serialized write pool with `max_connections = 1`.
///
/// All write operations that use `BEGIN IMMEDIATE` should go through this pool
/// to eliminate write contention at the source — only one connection ever attempts
/// to acquire the SQLite write lock.
///
/// # Configuration
/// - Max connections: 1 (serializes writes)
/// - Acquire timeout: 60s (writes queue through a single connection)
/// - Same WAL/pragma configuration as the read pool
///
/// # Arguments
/// * `database_url` - SQLite database URL (same as the read pool)
///
/// # Returns
/// A configured SQLite connection pool with a single connection.
pub async fn init_write_pool(database_url: &str) -> Result<WritePool, sqlx::Error> {
    establish_wal_mode(database_url).await?;
    open_write_pool(database_url).await
}

async fn open_write_pool(database_url: &str) -> Result<WritePool, sqlx::Error> {
    let connect_options = sqlite_connect_options(database_url)?;

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(60))
        .after_connect(|conn, _meta| {
            Box::pin(async move { apply_per_connection_pragmas(&mut *conn).await })
        })
        .connect_with(connect_options)
        .await?;

    ensure_wal_mode(&pool, "write_pool").await?;

    // Run a passive WAL checkpoint on startup to catch up any frames from a
    // previous crash without blocking readers (unlike TRUNCATE used in maintenance).
    {
        let mut conn = pool.acquire().await?;
        let row: (i32, i32, i32) = sqlx::query_as("PRAGMA wal_checkpoint(PASSIVE)")
            .fetch_one(&mut *conn)
            .await?;
        tracing::info!(
            "Write pool startup WAL checkpoint: busy={}, checkpointed={}, total={}",
            row.0,
            row.1,
            row.2
        );
    }

    tracing::info!("Write pool initialized with 1 max connection (serialized writes)");

    Ok(pool)
}

/// Initialize the read and write pools in the order required by SQLite.
///
/// Entering WAL mode requires an exclusive lock that SQLite's busy timeout cannot wait for.
/// A short-lived connection establishes the persistent mode before either pool opens the database.
pub async fn init_database_pools(database_url: &str) -> Result<(DbPool, WritePool), sqlx::Error> {
    establish_wal_mode(database_url).await?;
    let pool = open_pool_with_size(database_url, default_read_pool_size()).await?;
    let write_pool = match open_write_pool(database_url).await {
        Ok(pool) => pool,
        Err(error) => {
            pool.close().await;
            return Err(error);
        }
    };
    Ok((pool, write_pool))
}

pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    tracing::info!("Running database migrations...");

    // auto_vacuum must be selected before the first table is created. Existing
    // databases are converted by the guarded maintenance-window VACUUM path.
    {
        let mut connection = pool.acquire().await?;
        let (table_count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_one(&mut *connection)
        .await?;
        if table_count == 0 {
            sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
                .execute(&mut *connection)
                .await?;
            sqlx::query("VACUUM").execute(&mut *connection).await?;
        }
    }

    sqlx::migrate!("./migrations").run(pool).await?;
    tracing::info!("Database migrations completed");
    Ok(())
}

pub async fn begin_immediate(pool: &WritePool) -> Result<ImmediateTransaction, sqlx::Error> {
    pool.begin_with("BEGIN IMMEDIATE").await
}

/// An immediate SQLite transaction backed by SQLx's transaction lifecycle.
///
/// `BEGIN IMMEDIATE` acquires the write reservation at transaction start, and
/// SQLx queues a rollback before returning a dropped transaction to the pool.
pub type ImmediateTransaction = sqlx::Transaction<'static, Sqlite>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_pool() {
        let pool = init_pool("sqlite::memory:").await.unwrap();

        // Verify WAL mode is enabled
        let result: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();

        // In-memory databases use "memory" journal mode, not WAL
        // For file-based databases, this would be "wal"
        assert!(result.0 == "memory" || result.0 == "wal");
    }

    #[tokio::test]
    async fn fresh_file_database_pools_initialize_reliably() {
        let directory = tempfile::tempdir().unwrap();

        for attempt in 0..32 {
            let path = directory.path().join(format!("fresh-{attempt}.db"));
            let url = format!("sqlite:{}?mode=rwc", path.to_string_lossy());
            let (pool, write_pool) = init_database_pools(&url).await.unwrap();

            let read_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&pool)
                .await
                .unwrap();
            let write_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&write_pool)
                .await
                .unwrap();

            assert_eq!(read_mode, "wal");
            assert_eq!(write_mode, "wal");

            pool.close().await;
            write_pool.close().await;
        }
    }

    #[tokio::test]
    async fn dropped_immediate_transaction_rolls_back_on_the_same_connection() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE values_to_rollback (value INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();

        {
            let mut tx = begin_immediate(&pool).await.unwrap();
            sqlx::query("INSERT INTO values_to_rollback (value) VALUES (1)")
                .execute(&mut *tx)
                .await
                .unwrap();
        }

        let mut tx = tokio::time::timeout(Duration::from_secs(1), begin_immediate(&pool))
            .await
            .unwrap()
            .unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM values_to_rollback")
            .fetch_one(&mut *tx)
            .await
            .unwrap();

        assert_eq!(count, 0);
        tx.commit().await.unwrap();
    }

    /// `DROP TABLE` takes the dropped table's triggers with it: a migration that
    /// rebuilds `job` (create new table, copy, drop, rename) silently removes the two
    /// defined on `job`, and one that rebuilds `job_execution_progress` removes the
    /// `updated_at` trigger. Pin all three by name so a rebuild that does not reinstate
    /// one fails here rather than quietly degrading `job_execution_progress` upkeep.
    #[tokio::test]
    async fn migrated_schema_keeps_every_job_progress_trigger() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        for trigger in [
            "trg_job_execution_progress_touch_updated_at",
            "trg_job_reset_clears_progress",
            "trg_job_terminal_clears_progress",
        ] {
            let found: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = ?",
            )
            .bind(trigger)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(found, 1, "missing trigger {trigger}");
        }
    }

    /// A create-copy-drop-rename on `live_sessions` deletes every
    /// `media_outputs`, `danmu_statistics`, `session_segments`,
    /// `session_events` and `danmu_aggregator_state` row in the database unless
    /// foreign keys are genuinely off, and every other migration test starts
    /// from an empty database, where that is invisible. Populate the schema as
    /// it stands immediately before the rebuild, then apply the rest and assert
    /// nothing was lost.
    ///
    /// Any future `live_sessions` rebuild inherits this guard: raise
    /// `FIXTURE_BEFORE` to the new version so the fixture lands ahead of it.
    #[tokio::test]
    async fn live_sessions_rebuild_preserves_every_cascade_child() {
        /// Fixture is inserted just before this version, so the rebuild that
        /// follows has real rows to move.
        const FIXTURE_BEFORE: i64 = 20_260_903_000_000;

        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();

        let full = sqlx::migrate!("./migrations");
        let mut prefix = sqlx::migrate!("./migrations");
        prefix.migrations = full
            .migrations
            .iter()
            .filter(|migration| migration.version < FIXTURE_BEFORE)
            .cloned()
            .collect::<Vec<_>>()
            .into();
        assert!(
            prefix.migrations.len() < full.migrations.len(),
            "FIXTURE_BEFORE must sit before at least one migration"
        );
        prefix.run(&pool).await.unwrap();

        // `platform-twitch` is seeded by the initial schema migration.
        for statement in [
            "INSERT INTO streamers (id, name, url, platform_config_id, state, priority, \
             created_at, updated_at) \
             VALUES ('str-1', 'Alice', 'https://example.com/alice', 'platform-twitch', \
             'LIVE', 'NORMAL', 1700000000000, 1700000000000)",
            "INSERT INTO live_sessions (id, streamer_id, start_time, end_time, titles, \
             total_size_bytes, session_complete_dispatched) VALUES \
             ('sess-ended', 'str-1', 1700000100000, 1700003700000, '[]', 5000, 1), \
             ('sess-active', 'str-1', 1700010000000, NULL, '[]', 1500, 0)",
            "INSERT INTO media_outputs (id, session_id, file_path, file_type, size_bytes, \
             created_at) VALUES \
             ('mo-1', 'sess-ended', '/rec/1.mp4', 'VIDEO', 4000, 1700003600000), \
             ('mo-2', 'sess-ended', '/rec/1.jpg', 'THUMBNAIL', 1000, 1700003610000), \
             ('mo-3', 'sess-active', '/rec/2.mp4', 'VIDEO', 1500, 1700010500000)",
            "INSERT INTO session_segments (id, session_id, segment_index, file_path, \
             duration_secs, size_bytes, persisted_at) VALUES \
             ('seg-1', 'sess-ended', 0, '/rec/1.mp4', 3600.0, 5000, 1700003600000), \
             ('seg-2', 'sess-active', 0, '/rec/2.mp4', 500.0, 1500, 1700010500000)",
            "INSERT INTO danmu_statistics (id, session_id, total_danmus) \
             VALUES ('ds-1', 'sess-ended', 1200)",
            "INSERT INTO danmu_aggregator_state (session_id, version, updated_at, state) \
             VALUES ('sess-active', 1, 1700010500000, X'0102')",
            "INSERT INTO session_events (session_id, streamer_id, kind, occurred_at, payload) \
             VALUES ('sess-ended', 'str-1', 'session_started', 1700000100000, NULL), \
             ('sess-ended', 'str-1', 'session_ended', 1700003700000, NULL), \
             ('sess-active', 'str-1', 'session_started', 1700010000000, NULL)",
        ] {
            sqlx::query(statement).execute(&pool).await.unwrap();
        }

        let counts = |pool: DbPool| async move {
            let mut out = Vec::new();
            for table in [
                "live_sessions",
                "media_outputs",
                "session_segments",
                "danmu_statistics",
                "danmu_aggregator_state",
                "session_events",
            ] {
                let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                    "SELECT COUNT(*) FROM {table}"
                )))
                .fetch_one(&pool)
                .await
                .unwrap();
                out.push((table, count));
            }
            out
        };

        let before = counts(pool.clone()).await;
        assert_eq!(
            before,
            vec![
                ("live_sessions", 2),
                ("media_outputs", 3),
                ("session_segments", 2),
                ("danmu_statistics", 1),
                ("danmu_aggregator_state", 1),
                ("session_events", 3),
            ]
        );

        // Already-applied versions are skipped, so this runs only the rebuild
        // and anything after it.
        full.run(&pool).await.unwrap();

        assert_eq!(counts(pool.clone()).await, before, "the rebuild lost rows");

        let violations: Vec<String> = sqlx::query_scalar(
            "SELECT \"table\" || ' rowid=' || COALESCE(CAST(rowid AS TEXT), 'null') \
             FROM pragma_foreign_key_check",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert!(violations.is_empty(), "foreign_key_check: {violations:?}");

        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        // The backfill reaches rows that predate the column.
        let names: Vec<Option<String>> =
            sqlx::query_scalar("SELECT streamer_name FROM live_sessions ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            names,
            vec![Some("Alice".to_string()), Some("Alice".to_string())]
        );
    }

    /// The `live_sessions` rebuild that made `streamer_id` nullable must
    /// reinstate every index the dropped table carried plus
    /// `trg_live_session_orphan_ends`; `DROP TABLE` removes both. Pin them by
    /// name so a later rebuild that forgets one fails here.
    #[tokio::test]
    async fn migrated_schema_keeps_every_live_sessions_index_and_trigger() {
        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();

        for (kind, name) in [
            ("index", "live_sessions_one_active_per_streamer"),
            ("index", "idx_live_session_streamer_id"),
            ("index", "idx_live_session_streamer_time"),
            ("index", "idx_live_sessions_empty_ended"),
            ("index", "idx_live_sessions_start_time"),
            ("trigger", "trg_live_session_orphan_ends"),
        ] {
            let found: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type = ? AND name = ? AND tbl_name = 'live_sessions'",
            )
            .bind(kind)
            .bind(name)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(found, 1, "missing {kind} {name}");
        }
    }

    #[tokio::test]
    async fn reclaiming_a_processing_job_drops_its_progress_snapshot() {
        use crate::database::models::{JobDbModel, JobExecutionProgressDbModel, JobStatus};
        use crate::database::repositories::{JobRepository as _, SqlxJobRepository};

        let pool = init_pool_with_size("sqlite::memory:", 1).await.unwrap();
        run_migrations(&pool).await.unwrap();
        let repo = SqlxJobRepository::new(pool.clone(), pool.clone());

        let job = JobDbModel::new("remux", "{}");
        repo.create_job(&job).await.unwrap();
        repo.update_job_status(&job.id, JobStatus::Processing)
            .await
            .unwrap();
        // upsert_job_execution_progress only inserts while the job is PROCESSING.
        repo.upsert_job_execution_progress(&JobExecutionProgressDbModel {
            job_id: job.id.clone(),
            kind: "ffmpeg".to_string(),
            progress: r#"{"percent":42}"#.to_string(),
            updated_at: time::now_ms(),
        })
        .await
        .unwrap();
        assert!(
            repo.get_job_execution_progress(&job.id)
                .await
                .unwrap()
                .is_some()
        );

        assert_eq!(repo.reset_processing_jobs().await.unwrap(), 1);

        assert!(
            repo.get_job_execution_progress(&job.id)
                .await
                .unwrap()
                .is_none(),
            "trg_job_reset_clears_progress should drop the snapshot of a requeued job"
        );
    }
}
