use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("Generic error: {0}")]
    Generic(#[from] anyhow::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parsing error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("URL parsing error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("Extractor error: {0}")]
    Extractor(#[from] platforms_parser::extractor::error::ExtractorError),

    #[error("Semaphore acquire error: {0}")]
    Semaphore(#[from] tokio::sync::AcquireError),

    #[error("Platform extraction error: {0}")]
    Extraction(String),

    #[error("No streams available for the provided URL")]
    NoStreamsAvailable,

    #[error("Stream selection cancelled by user")]
    SelectionCancelled,

    #[error("Invalid stream filter: {0}")]
    InvalidFilter(String),

    #[error("Timeout error: Operation timed out after {seconds} seconds")]
    Timeout { seconds: u64 },
}

impl CliError {
    pub fn invalid_filter(msg: impl Into<String>) -> Self {
        Self::InvalidFilter(msg.into())
    }

    pub fn timeout() -> Self {
        Self::Timeout { seconds: 30 }
    }

    pub fn no_streams_found() -> Self {
        Self::NoStreamsAvailable
    }

    pub fn user_cancelled() -> Self {
        Self::SelectionCancelled
    }

    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::Extraction(msg.into())
    }

    pub fn no_matching_stream() -> Self {
        Self::InvalidFilter("No streams match the specified filters".into())
    }
}

pub type Result<T> = std::result::Result<T, CliError>; 