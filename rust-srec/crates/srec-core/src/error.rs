//! Storage-free core error types shared across layers.

use std::path::Path;
use thiserror::Error;

/// Result type for storage-free core operations and ports.
pub type CoreResult<T> = std::result::Result<T, CoreError>;

/// Storage-free error type for domain logic, utilities, and cross-layer ports.
#[derive(Error, Debug)]
pub enum CoreError {
    #[error(transparent)]
    ManualSplit(#[from] pipeline_common::ManualSplitError),

    #[error("Recorder feedback capacity is busy for {streamer_id}; retry startup")]
    RecorderFeedbackBusy { streamer_id: String },

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

impl CoreError {
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
}
