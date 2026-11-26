//! Pipeline subsystem error types.

use srec_core::CoreError;
use srec_sqlite::DbError;
use thiserror::Error;

/// Result type for pipeline orchestration, scheduling, and queue operations.
pub type PipelineResult<T> = std::result::Result<T, PipelineError>;

/// Error type for pipeline orchestration, scheduling, and queue operations.
#[derive(Error, Debug)]
pub enum PipelineError {
    /// A DAG operation that only applies to a live execution was attempted on
    /// one that already reached a terminal status.
    ///
    /// Distinct from a validation error so the API can give it a dedicated
    /// code: callers branch their UI on this condition (a cancel racing a DAG
    /// that just finished is expected, not a failure to report), and a shared
    /// `VALIDATION_ERROR` code would leave them matching on message text.
    #[error("DAG {dag_id} is already in a terminal state")]
    DagAlreadyTerminal { dag_id: String },

    /// A database failure. Build it with `From<DbError>`, which moves a
    /// wrapped shared kind to [`PipelineError::Core`] instead.
    #[error(transparent)]
    Db(DbError),

    #[error(transparent)]
    Core(#[from] CoreError),
}

srec_core::impl_core_error_wrapper!(PipelineError);

impl PipelineError {
    /// Whether this error represents a permanent or invalid-input failure that
    /// a workflow step's automatic retry policy should not re-run.
    pub fn is_non_retryable_step_error(&self) -> bool {
        matches!(self, Self::Core(error) if error.is_non_retryable_step_error())
    }
}

impl From<DbError> for PipelineError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Core(error) => Self::Core(error),
            error => Self::Db(error),
        }
    }
}

impl From<sqlx::Error> for PipelineError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

impl srec_sqlite::retry::SqlxErrorRef for PipelineError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::Db(error) => error.as_sqlx_error(),
            _ => None,
        }
    }
}
