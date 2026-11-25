//! SQLite persistence error types.

use std::path::Path;
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

    #[error("Database error: {0}")]
    Database(String),

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

    #[error(transparent)]
    CredentialProfile(#[from] srec_core::domain::credential::ProfileError),

    #[error(transparent)]
    CredentialUnavailable(srec_core::domain::credential::CredentialUnavailable),

    #[error(transparent)]
    Proxy(#[from] srec_core::domain::proxy::ProxyError),

    #[error("Duplicate URL: a streamer with URL '{0}' already exists")]
    DuplicateUrl(String),

    /// A job preset or workflow cannot be deleted or renamed while a stored
    /// pipeline names it, because pipelines reference steps by name and would
    /// fail to resolve the step at run time.
    #[error(
        "Cannot {change} {kind} '{name}': it is used by {}",
        srec_core::domain::step_routes::summarize_users(.users)
    )]
    PresetInUse {
        change: srec_core::domain::step_routes::PresetChange,
        kind: srec_core::domain::step_routes::StepRef,
        name: String,
        /// Every user, in a stable order.
        users: Vec<srec_core::domain::step_routes::StepUser>,
    },

    #[error("{0}")]
    Other(String),
}

impl DbError {
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

impl From<srec_core::CoreError> for DbError {
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
            srec_core::CoreError::Configuration(msg) => Self::Configuration(msg),
            srec_core::CoreError::Io(e) => Self::Io(e),
            srec_core::CoreError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_core::CoreError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_core::CoreError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            srec_core::CoreError::Proxy(e) => Self::Proxy(e),
            srec_core::CoreError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_core::CoreError::PresetInUse {
                change,
                kind,
                name,
                users,
            } => Self::PresetInUse {
                change,
                kind,
                name,
                users,
            },
            srec_core::CoreError::Other(msg) => Self::Other(msg),
            other => Self::Other(other.to_string()),
        }
    }
}

impl From<DbError> for srec_core::CoreError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::DatabaseSqlx(e) => Self::Database(e.to_string()),
            DbError::FilterSnapshot(e) => Self::Database(e.to_string()),
            DbError::Database(msg) => Self::Database(msg),
            DbError::Serialization(e) => Self::Serialization(e),
            DbError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            DbError::NotFound { entity_type, id } => Self::NotFound { entity_type, id },
            DbError::Validation(msg) => Self::Validation(msg),
            DbError::Conflict(msg) => Self::Conflict(msg),
            DbError::Configuration(msg) => Self::Configuration(msg),
            DbError::Io(e) => Self::Io(e),
            DbError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            DbError::CredentialProfile(e) => Self::CredentialProfile(e),
            DbError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            DbError::Proxy(e) => Self::Proxy(e),
            DbError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            DbError::PresetInUse {
                change,
                kind,
                name,
                users,
            } => Self::PresetInUse {
                change,
                kind,
                name,
                users,
            },
            DbError::Other(msg) => Self::Other(msg),
        }
    }
}
