//! Retry helpers for database operations.

use rand::random;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::debug;

use crate::{Error, Result};

const SQLITE_BUSY_MAX_RETRIES: usize = 12;
const SQLITE_BUSY_BASE_DELAY_MS: u64 = 10;
const SQLITE_BUSY_MAX_DELAY_MS: u64 = 2000;

fn is_sqlite_busy_error(err: &Error) -> bool {
    let Error::DatabaseSqlx(sqlx_err) = err else {
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

pub async fn retry_on_sqlite_busy<T, F, Fut>(op_name: &'static str, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
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
