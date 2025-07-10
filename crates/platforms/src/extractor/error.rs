use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtractorError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("http error: {0}")]
    HttpError(#[from] reqwest::Error),
    #[error("unsupported extractor")]
    UnsupportedExtractor,
    #[error("json error: {0}")]
    JsonError(#[from] serde_json::Error),
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
    #[error("validation error: {0}")]
    ValidationError(String),
    #[error("js error: {0}")]
    JsError(String),
    #[error("hls playlist error: {0}")]
    HlsPlaylistError(String),
    #[error("other error: {0}")]
    Other(String),
}
