//! SQLite persistence error types.

use srec_core::CoreError;
use thiserror::Error;

/// Result type for SQLite persistence and repository operations.
pub type DbResult<T> = std::result::Result<T, DbError>;

/// Error type for SQLite persistence, migrations, and repositories.
#[derive(Error, Debug)]
pub enum DbError {
    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

    #[error(transparent)]
    FilterSnapshot(#[from] crate::filter_store::FilterSnapshotError),

    #[error(transparent)]
    Core(#[from] CoreError),
}

srec_core::impl_core_error_wrapper!(DbError);

/// Lets storage-free ports propagate a [`DbError`] with `?`; SQLite failures
/// become [`CoreError::Database`].
impl From<DbError> for CoreError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::DatabaseSqlx(e) => Self::Database(e.to_string()),
            DbError::FilterSnapshot(e) => Self::Database(e.to_string()),
            DbError::Core(e) => e,
        }
    }
}
