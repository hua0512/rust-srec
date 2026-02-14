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

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Row, Sqlite};
use std::str::FromStr;
use std::time::Duration;

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
    sqlx::query(&format!(
        "PRAGMA wal_autocheckpoint = {}",
        DEFAULT_WAL_AUTOCHECKPOINT_PAGES
    ))
    .execute(&mut *conn)
    .await?;

    // Cap WAL/journal size growth to reduce disk usage under write-heavy workloads.
    sqlx::query(&format!(
        "PRAGMA journal_size_limit = {}",
        DEFAULT_JOURNAL_SIZE_LIMIT_BYTES
    ))
    .execute(&mut *conn)
    .await?;

    // Set cache size (64MB)
    sqlx::query(&format!("PRAGMA cache_size = {}", DEFAULT_CACHE_SIZE_KB))
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
    let connect_options = SqliteConnectOptions::from_str(database_url)?
        // Enable WAL mode for concurrent reads during writes
        .journal_mode(SqliteJournalMode::Wal)
        // NORMAL synchronous mode - balance between safety and performance
        .synchronous(SqliteSynchronous::Normal)
        // Set busy timeout to wait for locks
        .busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))
        // Enable foreign key constraints
        .foreign_keys(true)
        // Create database if it doesn't exist
        .create_if_missing(true);

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
    let connect_options = SqliteConnectOptions::from_str(database_url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_millis(DEFAULT_BUSY_TIMEOUT_MS))
        .foreign_keys(true)
        .create_if_missing(true);

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

pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    tracing::info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(pool).await?;
    tracing::info!("Database migrations completed");
    Ok(())
}

pub async fn begin_immediate(pool: &WritePool) -> Result<ImmediateTransaction, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    Ok(ImmediateTransaction::new(conn))
}

/// Wrapper for a manual immediate transaction.
///
/// This wrapper ensures that the transaction determines the write lock immediately (BEGIN IMMEDIATE),
/// preventing deadlocks that occur with deferred transactions (default) when multiple readers
/// try to upgrade to writers simultaneously.
pub struct ImmediateTransaction {
    conn: sqlx::pool::PoolConnection<Sqlite>,
    finished: bool,
}

impl ImmediateTransaction {
    pub fn new(conn: sqlx::pool::PoolConnection<Sqlite>) -> Self {
        Self {
            conn,
            finished: false,
        }
    }
}

impl ImmediateTransaction {
    /// Commit the transaction.
    pub async fn commit(mut self) -> Result<(), sqlx::Error> {
        sqlx::query("COMMIT").execute(&mut *self.conn).await?;
        self.finished = true;
        Ok(())
    }

    pub async fn rollback(mut self) -> Result<(), sqlx::Error> {
        sqlx::query("ROLLBACK").execute(&mut *self.conn).await?;
        self.finished = true;
        Ok(())
    }
}

impl std::ops::Deref for ImmediateTransaction {
    type Target = sqlx::SqliteConnection;

    fn deref(&self) -> &Self::Target {
        &self.conn
    }
}

impl std::ops::DerefMut for ImmediateTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.conn
    }
}

impl Drop for ImmediateTransaction {
    fn drop(&mut self) {
        if !self.finished {
            self.conn.close_on_drop();
        }
    }
}

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
}
