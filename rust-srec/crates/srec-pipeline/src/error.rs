//! Pipeline subsystem error types.

use std::path::Path;
use thiserror::Error;

/// Result type for pipeline orchestration, scheduling, and queue operations.
pub type PipelineResult<T> = std::result::Result<T, PipelineError>;

/// Error type for pipeline orchestration, scheduling, and queue operations.
#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

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

    /// A DAG operation that only applies to a live execution was attempted on
    /// one that already reached a terminal status.
    #[error("DAG {dag_id} is already in a terminal state")]
    DagAlreadyTerminal { dag_id: String },

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

    #[error(transparent)]
    Proxy(#[from] srec_core::domain::proxy::ProxyError),

    #[error("Pipeline error: {0}")]
    PipelineError(String),

    /// A pipeline processor failure that retrying with the same inputs and
    /// configuration cannot resolve.
    #[error("{0}")]
    NonRetryablePipeline(String),

    #[error("{0}")]
    Other(String),
}

impl PipelineError {
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

    pub fn io_path(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::IoPath {
            op,
            path: path.display().to_string(),
            source,
        }
    }

    /// Whether this error represents a permanent or invalid-input failure that
    /// a workflow step's automatic retry policy should not re-run.
    pub fn is_non_retryable_step_error(&self) -> bool {
        match self {
            Self::Validation(_)
            | Self::Configuration(_)
            | Self::InvalidStateTransition { .. }
            | Self::NonRetryablePipeline(_)
            | Self::Proxy(
                srec_core::domain::proxy::ProxyError::Missing(_)
                | srec_core::domain::proxy::ProxyError::Invalid(_)
                | srec_core::domain::proxy::ProxyError::GlobalInherit
                | srec_core::domain::proxy::ProxyError::ConfigReplaced,
            ) => true,
            Self::Core(err) => err.is_non_retryable_step_error(),
            _ => false,
        }
    }
}

impl From<srec_sqlite::DbError> for PipelineError {
    fn from(error: srec_sqlite::DbError) -> Self {
        match error {
            srec_sqlite::DbError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
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
            srec_sqlite::DbError::Proxy(e) => Self::Proxy(e),
            srec_sqlite::DbError::Other(msg) => Self::Other(msg),
            other => Self::Db(other),
        }
    }
}

impl From<srec_core::CoreError> for PipelineError {
    fn from(error: srec_core::CoreError) -> Self {
        match error {
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
            srec_core::CoreError::DagAlreadyTerminal { dag_id } => {
                Self::DagAlreadyTerminal { dag_id }
            }
            srec_core::CoreError::Configuration(msg) => Self::Configuration(msg),
            srec_core::CoreError::Io(e) => Self::Io(e),
            srec_core::CoreError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_core::CoreError::Proxy(e) => Self::Proxy(e),
            srec_core::CoreError::PipelineError(msg) => Self::PipelineError(msg),
            srec_core::CoreError::NonRetryablePipeline(msg) => Self::NonRetryablePipeline(msg),
            srec_core::CoreError::Other(msg) => Self::Other(msg),
            other => Self::Core(other),
        }
    }
}

impl srec_sqlite::retry::SqlxErrorRef for PipelineError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::DatabaseSqlx(err) => Some(err),
            Self::Db(srec_sqlite::DbError::DatabaseSqlx(err)) => Some(err),
            _ => None,
        }
    }
}
