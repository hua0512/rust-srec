//! Credential error types.

use thiserror::Error;

/// Errors that can occur during credential operations.
#[derive(Debug, Error)]
pub enum CredentialError {
    #[error(
        "Credential operation deadline exceeded while waiting for platform admission or provider work"
    )]
    DeadlineExceeded,
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

    /// Network error.
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    /// Parse error.
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Rate limited - try again later.
    #[error("Rate limited - try again later")]
    RateLimited {
        retry_after: Option<std::time::Duration>,
    },

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl CredentialError {
    /// Check if this error requires manual re-login.
    pub fn requires_relogin(&self) -> bool {
        matches!(
            self,
            Self::MissingRefreshToken | Self::InvalidRefreshToken | Self::InvalidCredentials(_)
        )
    }
}

impl From<platforms_parser::extractor::error::ExtractorError> for CredentialError {
    fn from(error: platforms_parser::extractor::error::ExtractorError) -> Self {
        use platforms_parser::extractor::error::ExtractorError;
        match error {
            ExtractorError::RateLimited { retry_after, .. } => Self::RateLimited { retry_after },
            ExtractorError::HttpError(error) => Self::Network(error.without_url()),
            _ => Self::RefreshFailed("Credential provider request failed".into()),
        }
    }
}
