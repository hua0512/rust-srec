//! Recorder subsystem error types.

use srec_core::CoreError;
use srec_sqlite::DbError;
use thiserror::Error;

/// Result type for recording control plane, monitoring, download, danmu,
/// credential, proxy, and configuration operations.
pub type RecorderResult<T> = std::result::Result<T, RecorderError>;

/// Error type for the `srec-recorder` domain crate.
#[derive(Error, Debug)]
pub enum RecorderError {
    #[error(transparent)]
    ManualSplit(#[from] pipeline_common::ManualSplitError),

    #[error("Recorder feedback capacity is busy for {streamer_id}; retry startup")]
    RecorderFeedbackBusy { streamer_id: String },

    #[error("Monitor error: {0}")]
    Monitor(String),

    /// Retain provider evidence for credential handling without displaying
    /// upstream bodies or signed URLs in recorder and API diagnostics.
    #[error("Extraction failed ({})", .0.category())]
    Extractor(#[from] platforms_parser::extractor::error::ExtractorError),

    #[error("Danmaku crate error: {0}")]
    DanmakuError(#[from] platforms_parser::danmaku::DanmakuError),

    /// A database failure. Build it with `From<DbError>`, which moves a
    /// wrapped shared kind to [`RecorderError::Core`] instead.
    #[error(transparent)]
    Db(DbError),

    #[error(transparent)]
    Core(#[from] CoreError),
}

srec_core::impl_core_error_wrapper!(RecorderError);

impl From<DbError> for RecorderError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Core(error) => Self::Core(error),
            error => Self::Db(error),
        }
    }
}

impl From<sqlx::Error> for RecorderError {
    fn from(error: sqlx::Error) -> Self {
        Self::Db(error.into())
    }
}

impl From<srec_sqlite::filter_store::FilterSnapshotError> for RecorderError {
    fn from(error: srec_sqlite::filter_store::FilterSnapshotError) -> Self {
        Self::Db(error.into())
    }
}

impl srec_sqlite::retry::SqlxErrorRef for RecorderError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::Db(error) => error.as_sqlx_error(),
            _ => None,
        }
    }
}
