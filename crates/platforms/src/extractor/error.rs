use thiserror::Error;

#[derive(Debug, Error)]
pub enum ExtractorError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("regex error: {0}")]
    RegexError(String),
    #[error("http error: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("unsupported extractor")]
    UnsupportedExtractor,
    #[error("json error: {0}")]
    JsonError(#[from] serde_json::Error),
    #[error("platform not supported")]
    PlatformNotSupported,
    #[error("live stream not supported")]
    LiveStreamNotSupported,
    #[error("age-restricted content")]
    AgeRestrictedContent,
    #[error("private content")]
    PrivateContent,
    #[error("region-locked content")]
    RegionLockedContent,
    #[error("streamer not found")]
    StreamerNotFound,
    #[error("streamer banned")]
    StreamerBanned,
    // #[error("video not found")]
    // VideoNotFound,
    // #[error("video unavailable")]
    // VideoUnavailable,
    #[error("no streams found")]
    NoStreamsFound,
    #[error("other: {0}")]
    Other(String),
}
