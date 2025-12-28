//! Credential error types.

use thiserror::Error;

/// Errors that can occur during credential operations.
#[derive(Debug, Error)]
pub enum CredentialError {
    /// Missing required cookie.
    #[error("Missing required cookie: {0}")]
    MissingCookie(&'static str),

    /// Missing refresh token - re-login required.
    #[error("Missing refresh token - re-login required")]
    MissingRefreshToken,

    /// Invalid refresh token - re-login required.
    #[error("Invalid refresh token - re-login required")]
    InvalidRefreshToken,

    /// Invalid credentials.
    #[error("Invalid credentials: {0}")]
    InvalidCredentials(String),

    /// Refresh failed.
    #[error("Refresh failed: {0}")]
    RefreshFailed(String),

    /// Crypto error.
    #[error("Crypto error: {0}")]
    CryptoError(String),

    /// Network error.
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Parse error.
    #[error("Parse error: {0}")]
    ParseError(String),

    /// JSON parse error.
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Unsupported platform.
    #[error("Unsupported platform: {0}")]
    UnsupportedPlatform(String),

    /// Database error.
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    /// Rate limited - try again later.
    #[error("Rate limited - try again later")]
    RateLimited,

    /// No credentials configured.
    #[error("No credentials configured for this scope")]
    NoCredentials,

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// Application error (from crate::Error).
    #[error("Application error: {0}")]
    Application(String),
}

impl CredentialError {
    /// Check if this error requires manual re-login.
    pub fn requires_relogin(&self) -> bool {
        matches!(
            self,
            Self::MissingRefreshToken | Self::InvalidRefreshToken | Self::InvalidCredentials(_)
        )
    }

    /// Check if this error is transient and may be retried.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::RateLimited | Self::ParseError(_)
        )
    }
}

impl From<crate::Error> for CredentialError {
    fn from(err: crate::Error) -> Self {
        match err {
            crate::Error::DatabaseSqlx(e) => CredentialError::Database(e),
            crate::Error::NotFound { entity_type, id } => {
                CredentialError::Internal(format!("{} not found: {}", entity_type, id))
            }
            _ => CredentialError::Application(err.to_string()),
        }
    }
}
