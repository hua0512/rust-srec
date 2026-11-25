//! Notification subsystem error types.

use thiserror::Error;

/// Result type for notification channel, service, Web Push, and outbox operations.
pub type NotifyResult<T> = std::result::Result<T, NotifyError>;

/// Error type for notification delivery, channel management, Web Push, and outbox draining.
#[derive(Error, Debug)]
pub enum NotifyError {
    #[error(transparent)]
    Database(#[from] srec_sqlite::DbError),

    #[error("Database error: {0}")]
    DatabaseSqlx(#[from] sqlx::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Entity not found: {entity_type} with id {id}")]
    NotFound { entity_type: String, id: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error(transparent)]
    Proxy(#[from] srec_core::domain::proxy::ProxyError),

    #[error("{channel} notification delivery failed: {message}")]
    NotificationDelivery {
        channel: &'static str,
        message: String,
    },

    #[error("{0}")]
    Other(String),
}

impl NotifyError {
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
}
