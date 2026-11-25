//! Recorder subsystem error types.

use std::path::Path;
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

    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

    #[error(transparent)]
    FilterSnapshot(#[from] srec_sqlite::filter_store::FilterSnapshotError),

    #[error("Database error: {0}")]
    Database(String),

    #[error(transparent)]
    Db(srec_sqlite::DbError),

    #[error(transparent)]
    Core(srec_core::CoreError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid state transition: cannot transition from {from} to {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Entity not found: {entity_type} with id {id}")]
    NotFound { entity_type: String, id: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("{0}")]
    Conflict(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("IO error while {op} '{path}': {source}")]
    IoPath {
        op: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Monitor error: {0}")]
    Monitor(String),

    /// Retain provider evidence for credential handling without displaying
    /// upstream bodies or signed URLs in recorder and API diagnostics.
    #[error("Extraction failed ({})", .0.category())]
    Extractor(#[from] platforms_parser::extractor::error::ExtractorError),

    #[error(transparent)]
    CredentialProfile(#[from] srec_core::domain::credential::ProfileError),

    #[error(transparent)]
    CredentialUnavailable(srec_core::domain::credential::CredentialUnavailable),

    #[error(transparent)]
    Proxy(#[from] srec_core::domain::proxy::ProxyError),

    #[error("Danmaku crate error: {0}")]
    DanmakuError(#[from] platforms_parser::danmaku::DanmakuError),

    #[error("Duplicate URL: a streamer with URL '{0}' already exists")]
    DuplicateUrl(String),

    #[error("{0}")]
    Other(String),
}

impl RecorderError {
    pub fn not_found(entity_type: impl Into<String>, id: impl Into<String>) -> Self {
        Self::NotFound {
            entity_type: entity_type.into(),
            id: id.into(),
        }
    }

    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    pub fn duplicate_url(url: impl Into<String>) -> Self {
        Self::DuplicateUrl(url.into())
    }

    pub fn io_path(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::IoPath {
            op,
            path: path.display().to_string(),
            source,
        }
    }
}

impl From<srec_sqlite::DbError> for RecorderError {
    fn from(error: srec_sqlite::DbError) -> Self {
        match error {
            srec_sqlite::DbError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_sqlite::DbError::FilterSnapshot(e) => Self::FilterSnapshot(e),
            srec_sqlite::DbError::Database(msg) => Self::Database(msg),
            srec_sqlite::DbError::Serialization(e) => Self::Serialization(e),
            srec_sqlite::DbError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_sqlite::DbError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_sqlite::DbError::Validation(msg) => Self::Validation(msg),
            srec_sqlite::DbError::Conflict(msg) => Self::Conflict(msg),
            srec_sqlite::DbError::Configuration(msg) => Self::Configuration(msg),
            srec_sqlite::DbError::Io(e) => Self::Io(e),
            srec_sqlite::DbError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_sqlite::DbError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_sqlite::DbError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            srec_sqlite::DbError::Proxy(e) => Self::Proxy(e),
            srec_sqlite::DbError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_sqlite::DbError::Other(msg) => Self::Other(msg),
            other => Self::Db(other),
        }
    }
}

impl From<srec_core::CoreError> for RecorderError {
    fn from(error: srec_core::CoreError) -> Self {
        match error {
            srec_core::CoreError::ManualSplit(e) => Self::ManualSplit(e),
            srec_core::CoreError::RecorderFeedbackBusy { streamer_id } => {
                Self::RecorderFeedbackBusy { streamer_id }
            }
            srec_core::CoreError::Database(msg) => Self::Database(msg),
            srec_core::CoreError::Serialization(e) => Self::Serialization(e),
            srec_core::CoreError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_core::CoreError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_core::CoreError::Validation(msg) => Self::Validation(msg),
            srec_core::CoreError::Conflict(msg) => Self::Conflict(msg),
            srec_core::CoreError::Configuration(msg) => Self::Configuration(msg),
            srec_core::CoreError::Io(e) => Self::Io(e),
            srec_core::CoreError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_core::CoreError::Monitor(msg) => Self::Monitor(msg),
            srec_core::CoreError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_core::CoreError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            srec_core::CoreError::Proxy(e) => Self::Proxy(e),
            srec_core::CoreError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_core::CoreError::Other(msg) => Self::Other(msg),
            other => Self::Core(other),
        }
    }
}

impl srec_sqlite::retry::SqlxErrorRef for RecorderError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::DatabaseSqlx(err) => Some(err),
            Self::Db(srec_sqlite::DbError::DatabaseSqlx(err)) => Some(err),
            _ => None,
        }
    }
}
