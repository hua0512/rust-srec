//! Notification subsystem error types.

use srec_core::CoreError;
use srec_sqlite::DbError;
use thiserror::Error;

/// Result type for notification channel, service, Web Push, and outbox operations.
pub type NotifyResult<T> = std::result::Result<T, NotifyError>;

/// Error type for notification delivery, channel management, Web Push, and outbox draining.
#[derive(Error, Debug)]
pub enum NotifyError {
    #[error("{channel} notification delivery failed: {message}")]
    NotificationDelivery {
        channel: &'static str,
        message: String,
    },

    /// A database failure. Build it with `From<DbError>`, which moves a
    /// wrapped shared kind to [`NotifyError::Core`] instead.
    #[error(transparent)]
    Db(DbError),

    #[error(transparent)]
    Core(#[from] CoreError),
}

srec_core::impl_core_error_wrapper!(NotifyError);

impl From<DbError> for NotifyError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Core(error) => Self::Core(error),
            error => Self::Db(error),
        }
    }
}

impl From<sqlx::Error> for NotifyError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}
