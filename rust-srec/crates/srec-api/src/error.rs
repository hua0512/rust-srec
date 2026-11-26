//! Error types for the `srec-api` crate.

use srec_core::CoreError;
use srec_notify::NotifyError;
use srec_pipeline::PipelineError;
use srec_recorder::RecorderError;
use srec_sqlite::DbError;
use thiserror::Error;

/// Result type for API server, authentication store, logging, and query operations.
pub type ServerResult<T> = std::result::Result<T, ServerError>;

/// Crate-internal alias matching `crate::error::Result`.
pub(crate) type Result<T> = ServerResult<T>;

/// Crate-internal alias matching `crate::error::Error`.
pub(crate) type Error = ServerError;

/// Error type surfaced across the `srec-api` crate boundary; the root
/// `rust-srec` crate uses it as its application-wide error.
///
/// The `From` impls for the lower crate errors keep every kind in one place: a
/// shared kind is always in [`ServerError::Core`] and a database failure in
/// [`ServerError::Db`], so the `Pipeline`, `Recorder` and `Notify` wrappers
/// hold only failures that originate in those crates.
#[derive(Error, Debug)]
pub enum ServerError {
    #[error("API error: {0}")]
    ApiError(String),

    #[error(transparent)]
    Pipeline(PipelineError),

    #[error(transparent)]
    Recorder(RecorderError),

    #[error(transparent)]
    Notify(NotifyError),

    #[error(transparent)]
    Db(DbError),

    #[error(transparent)]
    Core(#[from] CoreError),
}

srec_core::impl_core_error_wrapper!(ServerError);

impl From<DbError> for ServerError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Core(error) => Self::Core(error),
            error => Self::Db(error),
        }
    }
}

impl From<PipelineError> for ServerError {
    fn from(error: PipelineError) -> Self {
        match error {
            PipelineError::Core(error) => Self::Core(error),
            PipelineError::Db(error) => Self::from(error),
            error => Self::Pipeline(error),
        }
    }
}

impl From<RecorderError> for ServerError {
    fn from(error: RecorderError) -> Self {
        match error {
            RecorderError::Core(error) => Self::Core(error),
            RecorderError::Db(error) => Self::from(error),
            error => Self::Recorder(error),
        }
    }
}

impl From<NotifyError> for ServerError {
    fn from(error: NotifyError) -> Self {
        match error {
            NotifyError::Core(error) => Self::Core(error),
            NotifyError::Db(error) => Self::from(error),
            error => Self::Notify(error),
        }
    }
}

impl From<sqlx::Error> for ServerError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

impl From<srec_sqlite::filter_store::FilterSnapshotError> for ServerError {
    fn from(error: srec_sqlite::filter_store::FilterSnapshotError) -> Self {
        Self::Db(error.into())
    }
}

impl srec_sqlite::retry::SqlxErrorRef for ServerError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::Db(error) => error.as_sqlx_error(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use srec_sqlite::retry::SqlxErrorRef;

    use super::*;

    /// `retry_on_sqlite_busy` only retries errors whose `as_sqlx_error` finds
    /// the busy `sqlx::Error`, so every wrapper must keep it reachable.
    #[test]
    fn sqlx_errors_stay_visible_to_busy_retries_through_every_wrapper() {
        let sqlx = || sqlx::Error::PoolTimedOut;
        assert!(RecorderError::from(sqlx()).as_sqlx_error().is_some());
        assert!(PipelineError::from(sqlx()).as_sqlx_error().is_some());
        for error in [
            ServerError::from(sqlx()),
            ServerError::from(RecorderError::from(sqlx())),
            ServerError::from(PipelineError::from(sqlx())),
            ServerError::from(NotifyError::from(sqlx())),
        ] {
            assert!(error.as_sqlx_error().is_some(), "{error:?}");
        }
    }
}
