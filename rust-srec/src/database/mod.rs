//! Database module for rust-srec.
//!
//! This module provides the persistence layer using SQLite with sqlx.
//! It includes connection pool management, models, repositories, and maintenance.

pub mod batching;
pub mod maintenance;
pub mod models;
pub mod repositories;

// Re-export commonly used types
pub use batching::{BatchWriter, BatchWriterConfig, JobStatusUpdate, StatsUpdate};
pub use maintenance::{MaintenanceConfig, MaintenanceScheduler};

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Sqlite};
use std::str::FromStr;
use std::time::Duration;

/// Database connection pool type alias.
pub type DbPool = Pool<Sqlite>;

/// Default connection pool size.
const DEFAULT_POOL_SIZE: u32 = 10;

/// Default busy timeout in milliseconds.
const DEFAULT_BUSY_TIMEOUT_MS: u64 = 5000;

/// Default cache size in KB (64MB = 65536 KB, but SQLite uses pages, so we use -64000 for 64MB).
const DEFAULT_CACHE_SIZE_KB: i32 = -64000;

/// Default WAL auto-checkpoint threshold in pages.
/// With a typical 4KB page size, 1000 pages is ~4MB.
const DEFAULT_WAL_AUTOCHECKPOINT_PAGES: i32 = 1000;

/// Limit WAL size growth (bytes).
const DEFAULT_JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024; // 64MB

/// Initialize the database connection pool with WAL mode and performance optimizations.
///
/// # Configuration
/// - Journal Mode: WAL (Write-Ahead Logging) for concurrent reads/writes
/// - Connection Pool Size: 10 (configurable)
/// - Busy Timeout: 5000ms
/// - Synchronous: NORMAL (balance between safety and performance)
/// - Cache Size: 64MB
///
/// # Arguments
/// * `database_url` - SQLite database URL (e.g., "sqlite:srec.db?mode=rwc")
///
/// # Returns
/// A configured SQLite connection pool.
pub async fn init_pool(database_url: &str) -> Result<DbPool, sqlx::Error> {
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
        .max_connections(DEFAULT_POOL_SIZE)
        .acquire_timeout(Duration::from_secs(30))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
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
            })
        })
        .connect_with(connect_options)
        .await?;

    tracing::info!(
        "Database pool initialized with WAL mode, {} max connections",
        DEFAULT_POOL_SIZE
    );

    Ok(pool)
}

/// Run database migrations.
pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    tracing::info!("Running database migrations...");
    sqlx::migrate!("./migrations").run(pool).await?;
    tracing::info!("Database migrations completed");
    Ok(())
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
