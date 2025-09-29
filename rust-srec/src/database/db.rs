use sqlx::migrate::MigrateError;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Failed to connect to the database: {0}")]
    ConnectionFailed(#[from] sqlx::Error),
    #[error("Failed to run migrations: {0}")]
    MigrationFailed(#[from] MigrateError),
}

/// Creates and returns a connection pool to the SQLite database.
///
/// # Arguments
///
/// * `database_url` - The URL of the SQLite database file.
///
/// # Returns
///
/// A `Result` containing the `SqlitePool` or a `DbError`.
pub async fn create_pool(database_url: &str) -> Result<SqlitePool, DbError> {
    let options = SqliteConnectOptions::from_str(database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(options)
        .await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}
