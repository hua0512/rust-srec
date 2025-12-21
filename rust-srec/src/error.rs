//! Application-wide error types.

use thiserror::Error;

/// Application-wide result type.
pub type Result<T> = std::result::Result<T, Error>;

/// Application-wide error type.
#[derive(Error, Debug)]
pub enum Error {
    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

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

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Monitor error: {0}")]
    Monitor(String),

    #[error("Download error: {0}")]
    DownloadError(String),

    #[error("Pipeline error: {0}")]
    PipelineError(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("Danmaku crate error: {0}")]
    DanmakuError(#[from] platforms_parser::danmaku::DanmakuError),

    #[error("{0}")]
    Other(String),

    #[error("Duplicate URL: a streamer with URL '{0}' already exists")]
    DuplicateUrl(String),
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

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    pub fn duplicate_url(url: impl Into<String>) -> Self {
        Self::DuplicateUrl(url.into())
    }
}
