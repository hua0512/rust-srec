//! Application-wide error types.

use std::path::Path;
use thiserror::Error;

/// Application-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Application-wide error type.
#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    ManualSplit(#[from] pipeline_common::ManualSplitError),
    #[error("Scheduler feedback capacity is busy for {streamer_id}; retry startup")]
    SchedulerFeedbackBusy { streamer_id: String },
    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

    #[error(transparent)]
    FilterSnapshot(#[from] crate::database::filter_store::FilterSnapshotError),

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

    /// A DAG operation that only applies to a live execution was attempted on
    /// one that already reached a terminal status.
    ///
    /// Distinct from [`Error::Validation`] so `From<Error> for ApiError` can
    /// give it a dedicated code: callers branch their UI on this condition
    /// (a cancel racing a DAG that just finished is expected, not a failure
    /// to report), and a shared `VALIDATION_ERROR` code leaves them matching
    /// on message text.
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

    #[error("Monitor error: {0}")]
    Monitor(String),

    /// Retain provider evidence for credential handling without displaying
    /// upstream bodies or signed URLs in scheduler and API diagnostics.
    #[error("Extraction failed ({})", .0.category())]
    Extractor(#[from] platforms_parser::extractor::error::ExtractorError),

    #[error(transparent)]
    CredentialProfile(#[from] crate::domain::credential::ProfileError),

    #[error(transparent)]
    CredentialUnavailable(crate::domain::credential::CredentialUnavailable),

    #[error(transparent)]
    Proxy(#[from] crate::domain::proxy::ProxyError),

    #[error("Pipeline error: {0}")]
    PipelineError(String),

    /// A pipeline processor failure that retrying with the same inputs and
    /// configuration cannot resolve (for example invalid remote credentials,
    /// a missing remote directory, or files permanently rejected by the remote).
    #[error("{0}")]
    NonRetryablePipeline(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Danmaku crate error: {0}")]
    DanmakuError(#[from] platforms_parser::danmaku::DanmakuError),

    #[error("{0}")]
    Other(String),

    #[error("{channel} notification delivery failed: {message}")]
    NotificationDelivery {
        channel: &'static str,
        message: String,
    },

    #[error("Duplicate URL: a streamer with URL '{0}' already exists")]
    DuplicateUrl(String),

    /// A job preset or workflow cannot be deleted or renamed while a stored
    /// pipeline names it, because pipelines reference steps by name and would
    /// fail to resolve the step at run time.
    #[error(
        "Cannot {change} {kind} '{name}': it is used by {}",
        crate::domain::step_routes::summarize_users(.users)
    )]
    PresetInUse {
        change: crate::domain::step_routes::PresetChange,
        kind: crate::domain::step_routes::StepRef,
        name: String,
        /// Every user, in a stable order.
        users: Vec<crate::domain::step_routes::StepUser>,
    },
}

impl Error {
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

    /// Whether this error represents a permanent or invalid-input failure that
    /// a workflow step's automatic retry policy should not re-run.
    pub fn is_non_retryable_step_error(&self) -> bool {
        matches!(
            self,
            Self::Validation(_)
                | Self::Configuration(_)
                | Self::InvalidStateTransition { .. }
                | Self::NonRetryablePipeline(_)
                | Self::Proxy(
                    crate::domain::proxy::ProxyError::Missing(_)
                        | crate::domain::proxy::ProxyError::Invalid(_)
                        | crate::domain::proxy::ProxyError::GlobalInherit
                        | crate::domain::proxy::ProxyError::ConfigReplaced
                )
        )
    }

    pub(crate) fn into_core_error(self) -> srec_core::CoreError {
        match self {
            Self::ManualSplit(e) => srec_core::CoreError::ManualSplit(e),
            Self::SchedulerFeedbackBusy { streamer_id } => {
                srec_core::CoreError::SchedulerFeedbackBusy { streamer_id }
            }
            Self::DatabaseSqlx(e) => srec_core::CoreError::Database(e.to_string()),
            Self::FilterSnapshot(e) => srec_core::CoreError::Database(e.to_string()),
            Self::Database(msg) => srec_core::CoreError::Database(msg),
            Self::Serialization(e) => srec_core::CoreError::Serialization(e),
            Self::InvalidStateTransition { from, to } => {
                srec_core::CoreError::InvalidStateTransition { from, to }
            }
            Self::NotFound { entity_type, id } => {
                srec_core::CoreError::NotFound { entity_type, id }
            }
            Self::Validation(msg) => srec_core::CoreError::Validation(msg),
            Self::Conflict(msg) => srec_core::CoreError::Conflict(msg),
            Self::DagAlreadyTerminal { dag_id } => {
                srec_core::CoreError::DagAlreadyTerminal { dag_id }
            }
            Self::Configuration(msg) => srec_core::CoreError::Configuration(msg),
            Self::Io(e) => srec_core::CoreError::Io(e),
            Self::IoPath { op, path, source } => srec_core::CoreError::IoPath { op, path, source },
            Self::Monitor(msg) => srec_core::CoreError::Monitor(msg),
            Self::Extractor(e) => srec_core::CoreError::Other(e.to_string()),
            Self::CredentialProfile(e) => srec_core::CoreError::CredentialProfile(e),
            Self::CredentialUnavailable(e) => srec_core::CoreError::CredentialUnavailable(e),
            Self::Proxy(e) => srec_core::CoreError::Proxy(e),
            Self::PipelineError(msg) => srec_core::CoreError::PipelineError(msg),
            Self::NonRetryablePipeline(msg) => srec_core::CoreError::NonRetryablePipeline(msg),
            Self::ApiError(msg) => srec_core::CoreError::ApiError(msg),
            Self::DanmakuError(e) => srec_core::CoreError::Other(e.to_string()),
            Self::Other(msg) => srec_core::CoreError::Other(msg),
            Self::NotificationDelivery { channel, message } => {
                srec_core::CoreError::NotificationDelivery { channel, message }
            }
            Self::DuplicateUrl(url) => srec_core::CoreError::DuplicateUrl(url),
            Self::PresetInUse {
                change,
                kind,
                name,
                users,
            } => srec_core::CoreError::PresetInUse {
                change,
                kind,
                name,
                users,
            },
        }
    }
}

impl From<srec_core::CoreError> for Error {
    fn from(error: srec_core::CoreError) -> Self {
        match error {
            srec_core::CoreError::ManualSplit(e) => Self::ManualSplit(e),
            srec_core::CoreError::SchedulerFeedbackBusy { streamer_id } => {
                Self::SchedulerFeedbackBusy { streamer_id }
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
            srec_core::CoreError::DagAlreadyTerminal { dag_id } => {
                Self::DagAlreadyTerminal { dag_id }
            }
            srec_core::CoreError::Configuration(msg) => Self::Configuration(msg),
            srec_core::CoreError::Io(e) => Self::Io(e),
            srec_core::CoreError::IoPath { op, path, source } => Self::IoPath { op, path, source },
            srec_core::CoreError::Monitor(msg) => Self::Monitor(msg),
            srec_core::CoreError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_core::CoreError::CredentialUnavailable(e) => Self::CredentialUnavailable(e),
            srec_core::CoreError::Proxy(e) => Self::Proxy(e),
            srec_core::CoreError::PipelineError(msg) => Self::PipelineError(msg),
            srec_core::CoreError::NonRetryablePipeline(msg) => Self::NonRetryablePipeline(msg),
            srec_core::CoreError::ApiError(msg) => Self::ApiError(msg),
            srec_core::CoreError::Other(msg) => Self::Other(msg),
            srec_core::CoreError::NotificationDelivery { channel, message } => {
                Self::NotificationDelivery { channel, message }
            }
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
        }
    }
}

impl From<srec_sqlite::DbError> for Error {
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
            srec_sqlite::DbError::PresetInUse {
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
            srec_sqlite::DbError::Other(msg) => Self::Other(msg),
        }
    }
}

impl srec_sqlite::retry::SqlxErrorRef for Error {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::DatabaseSqlx(err) => Some(err),
            _ => None,
        }
    }
}
