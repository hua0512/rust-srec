//! Retry helpers for database operations.

use rand::random;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;

use crate::DbError;

const SQLITE_BUSY_MAX_RETRIES: usize = 12;
const SQLITE_BUSY_BASE_DELAY_MS: u64 = 10;
const SQLITE_BUSY_MAX_DELAY_MS: u64 = 2000;

/// Extracts an underlying [`sqlx::Error`] for SQLite busy/locked retry checks.
pub trait SqlxErrorRef {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error>;
}

impl SqlxErrorRef for DbError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::DatabaseSqlx(err) => Some(err),
            _ => None,
        }
    }
}

impl SqlxErrorRef for sqlx::Error {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        Some(self)
    }
}

fn is_sqlite_busy_error(err: &impl SqlxErrorRef) -> bool {
    let Some(sqlx_err) = err.as_sqlx_error() else {
        return false;
    };

    let sqlx::Error::Database(db_err) = sqlx_err else {
        let msg = sqlx_err.to_string().to_ascii_lowercase();
        return msg.contains("database is locked") || msg.contains("database is busy");
    };

    let code = db_err.code().map(Cow::into_owned);
    if matches!(code.as_deref(), Some("5") | Some("6")) {
        return true;
    }

    let msg = db_err.message().to_ascii_lowercase();
    msg.contains("database is locked") || msg.contains("database is busy")
}

pub async fn retry_on_sqlite_busy<T, E, F, Fut>(op_name: &'static str, mut op: F) -> Result<T, E>
where
    E: SqlxErrorRef,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut attempt = 0usize;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if !is_sqlite_busy_error(&err) || attempt >= SQLITE_BUSY_MAX_RETRIES {
                    return Err(err);
                }

                let exp_backoff_ms = SQLITE_BUSY_BASE_DELAY_MS.saturating_mul(1u64 << attempt);
                let capped_ms = exp_backoff_ms.min(SQLITE_BUSY_MAX_DELAY_MS);
                let jitter_ms =
                    (random::<u64>() % (capped_ms / 4 + 1)).min(SQLITE_BUSY_MAX_DELAY_MS);
                let delay =
                    Duration::from_millis((capped_ms + jitter_ms).min(SQLITE_BUSY_MAX_DELAY_MS));

                debug!(
                    "SQLite busy during {}, retrying in {:?} (attempt {}/{})",
                    op_name,
                    delay,
                    attempt + 1,
                    SQLITE_BUSY_MAX_RETRIES
                );

                sleep(delay).await;
                attempt += 1;
            }
        }
    }
}
