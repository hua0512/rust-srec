use pipeline_common::WriterError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Pipeline error: {0}")]
    Pipeline(#[from] pipeline_common::PipelineError),

    #[error("Download error: {0}")]
    Download(#[from] mesio_engine::DownloadError),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Processor error: {0}")]
    Processor(#[from] Box<dyn std::error::Error>),

    #[error("Writer error: {0}")]
    Writer(String),

    #[error("Broken pipe: consumer closed the connection")]
    BrokenPipe,
}

/// Check if an error string indicates a broken pipe error
pub fn is_broken_pipe_error(err_str: &str) -> bool {
    err_str.contains("Broken pipe")
        || err_str.contains("broken pipe")
        || err_str.contains("os error 109")  // Windows broken pipe error code
        || err_str.contains("EPIPE")
}

impl<StrategyError: std::error::Error + Send + Sync + 'static> From<WriterError<StrategyError>>
    for AppError
{
    fn from(error: WriterError<StrategyError>) -> Self {
        AppError::Writer(error.to_string())
    }
}
