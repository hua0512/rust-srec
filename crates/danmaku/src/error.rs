//! Danmaku error types.

use thiserror::Error;

/// Crate-specific result type.
pub type Result<T> = std::result::Result<T, DanmakuError>;

/// Errors that can occur during danmu collection.
#[derive(Error, Debug)]
pub enum DanmakuError {
    /// Connection-related errors (WebSocket, IRC, etc.)
    #[error("Connection error: {0}")]
    Connection(String),

    /// Protocol parsing/encoding errors
    #[error("Protocol error: {0}")]
    Protocol(String),

    /// IO errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// TARS codec errors
    #[error("TARS error: {0}")]
    Tars(#[from] tars_codec::TarsError),

    /// Generic error
    #[error("{0}")]
    Other(String),
}

impl DanmakuError {
    /// Create a connection error.
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::Connection(msg.into())
    }

    /// Create a protocol error.
    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }

    /// Create a generic error.
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}
