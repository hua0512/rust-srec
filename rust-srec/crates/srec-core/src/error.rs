//! Storage-free core error types shared across layers.
//!
//! Every backend crate has its own error type. It holds the failures that
//! originate in that crate, a `Core` variant for the shared kinds below, and
//! one wrapper variant for each lower crate's own failures that it propagates.
//! Converting a lower error moves its shared kind into the higher error's
//! `Core` and its database failure into the higher error's `Db`, so code that
//! inspects an error finds each kind in one place at every layer.

use std::path::Path;
use thiserror::Error;

/// Result type for storage-free core operations and ports.
pub type CoreResult<T> = std::result::Result<T, CoreError>;

/// Storage-free error type for domain logic, utilities, and cross-layer ports.
#[derive(Error, Debug)]
pub enum CoreError {
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

    #[error("{0}")]
    Other(String),

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

    pub fn database(msg: impl Into<String>) -> Self {
        Self::Database(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
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

#[doc(hidden)]
pub use serde_json::Error as __SerdeJsonError;

/// Gives a crate error type that wraps [`CoreError`] the same constructors as
/// [`CoreError`] (`not_found`, `validation`, `conflict`, `config`,
/// `duplicate_url`, `io_path`, `database`, `other`) and `From` impls for the
/// errors that [`CoreError`] absorbs (`serde_json::Error`, `std::io::Error`,
/// `ProfileError`, `ProxyError`).
///
/// Everything it generates builds the [`CoreError`] and converts it with the
/// type's `From<CoreError>` impl, so a shared kind always lands in that
/// type's `Core` variant.
#[macro_export]
macro_rules! impl_core_error_wrapper {
    ($error:ty) => {
        impl $error {
            pub fn not_found(entity_type: impl Into<String>, id: impl Into<String>) -> Self {
                $crate::CoreError::not_found(entity_type, id).into()
            }

            pub fn validation(msg: impl Into<String>) -> Self {
                $crate::CoreError::validation(msg).into()
            }

            pub fn conflict(msg: impl Into<String>) -> Self {
                $crate::CoreError::conflict(msg).into()
            }

            pub fn config(msg: impl Into<String>) -> Self {
                $crate::CoreError::config(msg).into()
            }

            pub fn duplicate_url(url: impl Into<String>) -> Self {
                $crate::CoreError::duplicate_url(url).into()
            }

            pub fn io_path(
                op: &'static str,
                path: &::std::path::Path,
                source: ::std::io::Error,
            ) -> Self {
                $crate::CoreError::io_path(op, path, source).into()
            }

            pub fn database(msg: impl Into<String>) -> Self {
                $crate::CoreError::database(msg).into()
            }

            pub fn other(msg: impl Into<String>) -> Self {
                $crate::CoreError::other(msg).into()
            }
        }

        impl From<$crate::error::__SerdeJsonError> for $error {
            fn from(error: $crate::error::__SerdeJsonError) -> Self {
                $crate::CoreError::from(error).into()
            }
        }

        impl From<::std::io::Error> for $error {
            fn from(error: ::std::io::Error) -> Self {
                $crate::CoreError::from(error).into()
            }
        }

        impl From<$crate::domain::credential::ProfileError> for $error {
            fn from(error: $crate::domain::credential::ProfileError) -> Self {
                $crate::CoreError::from(error).into()
            }
        }

        impl From<$crate::domain::proxy::ProxyError> for $error {
            fn from(error: $crate::domain::proxy::ProxyError) -> Self {
                $crate::CoreError::from(error).into()
            }
        }
    };
}
