//! Error types for the `srec-api` crate.

use std::path::Path;
use thiserror::Error;

/// Result type for API server, authentication store, logging, and query operations.
pub type ServerResult<T> = std::result::Result<T, ServerError>;

/// Crate-internal alias matching `crate::error::Result`.
pub(crate) type Result<T> = ServerResult<T>;

/// Crate-internal alias matching `crate::error::Error`.
pub(crate) type Error = ServerError;

/// Error type surfaced across the `srec-api` crate boundary.
#[derive(Error, Debug)]
pub enum ServerError {
    #[error(transparent)]
    ManualSplit(#[from] pipeline_common::ManualSplitError),

    #[error("Scheduler feedback capacity is busy for {streamer_id}; retry startup")]
    SchedulerFeedbackBusy { streamer_id: String },

    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

    #[error(transparent)]
    FilterSnapshot(#[from] srec_sqlite::filter_store::FilterSnapshotError),

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
    CredentialProfile(#[from] srec_core::domain::credential::ProfileError),

    #[error(transparent)]
    CredentialUnavailable(srec_core::domain::credential::CredentialUnavailable),

    #[error(transparent)]
    Proxy(#[from] srec_core::domain::proxy::ProxyError),

    #[error("Pipeline error: {0}")]
    PipelineError(String),

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

    #[error(
        "Cannot {change} {kind} '{name}': it is used by {}",
        srec_core::domain::step_routes::summarize_users(.users)
    )]
    PresetInUse {
        change: srec_core::domain::step_routes::PresetChange,
        kind: srec_core::domain::step_routes::StepRef,
        name: String,
        users: Vec<srec_core::domain::step_routes::StepUser>,
    },
}

impl ServerError {
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

impl From<srec_core::CoreError> for ServerError {
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

impl From<srec_sqlite::DbError> for ServerError {
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

impl From<srec_notify::NotifyError> for ServerError {
    fn from(error: srec_notify::NotifyError) -> Self {
        match error {
            srec_notify::NotifyError::Database(e) => Self::from(e),
            srec_notify::NotifyError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_notify::NotifyError::Serialization(e) => Self::Serialization(e),
            srec_notify::NotifyError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_notify::NotifyError::Validation(msg) => Self::Validation(msg),
            srec_notify::NotifyError::Configuration(msg) => Self::Configuration(msg),
            srec_notify::NotifyError::Proxy(e) => Self::Proxy(e),
            srec_notify::NotifyError::NotificationDelivery { channel, message } => {
                Self::NotificationDelivery { channel, message }
            }
            srec_notify::NotifyError::Other(msg) => Self::Other(msg),
        }
    }
}

impl From<srec_pipeline::PipelineError> for ServerError {
    fn from(error: srec_pipeline::PipelineError) -> Self {
        match error {
            srec_pipeline::PipelineError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_pipeline::PipelineError::Database(msg) => Self::Database(msg),
            srec_pipeline::PipelineError::Db(e) => Self::from(e),
            srec_pipeline::PipelineError::Core(e) => Self::from(e),
            srec_pipeline::PipelineError::Serialization(e) => Self::Serialization(e),
            srec_pipeline::PipelineError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_pipeline::PipelineError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_pipeline::PipelineError::Validation(msg) => Self::Validation(msg),
            srec_pipeline::PipelineError::Conflict(msg) => Self::Conflict(msg),
            srec_pipeline::PipelineError::DagAlreadyTerminal { dag_id } => {
                Self::DagAlreadyTerminal { dag_id }
            }
            srec_pipeline::PipelineError::Configuration(msg) => Self::Configuration(msg),
            srec_pipeline::PipelineError::Io(e) => Self::Io(e),
            srec_pipeline::PipelineError::IoPath { op, path, source } => {
                Self::IoPath { op, path, source }
            }
            srec_pipeline::PipelineError::Proxy(e) => Self::Proxy(e),
            srec_pipeline::PipelineError::PipelineError(msg) => Self::PipelineError(msg),
            srec_pipeline::PipelineError::NonRetryablePipeline(msg) => {
                Self::NonRetryablePipeline(msg)
            }
            srec_pipeline::PipelineError::Other(msg) => Self::Other(msg),
        }
    }
}

impl From<srec_recorder::RecorderError> for ServerError {
    fn from(error: srec_recorder::RecorderError) -> Self {
        match error {
            srec_recorder::RecorderError::ManualSplit(e) => Self::ManualSplit(e),
            srec_recorder::RecorderError::SchedulerFeedbackBusy { streamer_id } => {
                Self::SchedulerFeedbackBusy { streamer_id }
            }
            srec_recorder::RecorderError::DatabaseSqlx(e) => Self::DatabaseSqlx(e),
            srec_recorder::RecorderError::FilterSnapshot(e) => Self::FilterSnapshot(e),
            srec_recorder::RecorderError::Database(msg) => Self::Database(msg),
            srec_recorder::RecorderError::Db(e) => Self::from(e),
            srec_recorder::RecorderError::Core(e) => Self::from(e),
            srec_recorder::RecorderError::Serialization(e) => Self::Serialization(e),
            srec_recorder::RecorderError::InvalidStateTransition { from, to } => {
                Self::InvalidStateTransition { from, to }
            }
            srec_recorder::RecorderError::NotFound { entity_type, id } => {
                Self::NotFound { entity_type, id }
            }
            srec_recorder::RecorderError::Validation(msg) => Self::Validation(msg),
            srec_recorder::RecorderError::Conflict(msg) => Self::Conflict(msg),
            srec_recorder::RecorderError::Configuration(msg) => Self::Configuration(msg),
            srec_recorder::RecorderError::Io(e) => Self::Io(e),
            srec_recorder::RecorderError::IoPath { op, path, source } => {
                Self::IoPath { op, path, source }
            }
            srec_recorder::RecorderError::Monitor(msg) => Self::Monitor(msg),
            srec_recorder::RecorderError::Extractor(e) => Self::Extractor(e),
            srec_recorder::RecorderError::CredentialProfile(e) => Self::CredentialProfile(e),
            srec_recorder::RecorderError::CredentialUnavailable(e) => {
                Self::CredentialUnavailable(e)
            }
            srec_recorder::RecorderError::Proxy(e) => Self::Proxy(e),
            srec_recorder::RecorderError::DanmakuError(e) => Self::DanmakuError(e),
            srec_recorder::RecorderError::DuplicateUrl(url) => Self::DuplicateUrl(url),
            srec_recorder::RecorderError::Other(msg) => Self::Other(msg),
        }
    }
}

impl srec_sqlite::retry::SqlxErrorRef for ServerError {
    fn as_sqlx_error(&self) -> Option<&sqlx::Error> {
        match self {
            Self::DatabaseSqlx(err) => Some(err),
            _ => None,
        }
    }
}
