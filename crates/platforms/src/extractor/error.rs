use std::time::Duration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExtractorError {
    /// A confirmed account authentication failure. `code` is a provider reason
    /// code, never a response body, cookie, or credential value.
    #[error("account authentication required ({code})")]
    Authentication { code: String },
    /// The provider throttled the request. A throttle belongs to the network
    /// path the request took, not to the account it carried.
    #[error("provider rate limited")]
    RateLimited {
        code: Option<String>,
        retry_after: Option<Duration>,
    },
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

impl ExtractorError {
    /// Safe diagnostic category which does not render upstream URLs or bodies.
    pub fn category(&self) -> &'static str {
        match self {
            Self::Authentication { .. } => "authentication",
            Self::RateLimited { .. } => "rate_limited",
            Self::InvalidUrl(_) => "invalid_url",
            Self::HttpError(_) => "http",
            Self::UnsupportedExtractor => "unsupported_extractor",
            Self::JsonError(_) => "json",
            Self::LiveStreamNotSupported => "live_stream_not_supported",
            Self::AgeRestrictedContent => "age_restricted",
            Self::PrivateContent => "private",
            Self::RegionLockedContent => "region_locked",
            Self::StreamerNotFound => "not_found",
            Self::StreamerBanned => "banned",
            Self::NoStreamsFound => "no_streams",
            Self::ValidationError(_) => "validation",
            Self::JsError(_) => "javascript",
            Self::HlsPlaylistError(_) => "hls_playlist",
            Self::Other(_) => "other",
        }
    }

    /// Preserve throttling before decoding a response body. Generic 401/403
    /// responses remain HTTP errors; content restrictions are not proof of a
    /// revoked account. Provider-specific responses can establish that later.
    pub fn check_response(response: reqwest::Response) -> Result<reqwest::Response, Self> {
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| parse_retry_after(value, chrono::Utc::now()));
            return Err(Self::RateLimited {
                code: Some("429".to_owned()),
                retry_after,
            });
        }
        response.error_for_status().map_err(Self::HttpError)
    }
}

fn parse_retry_after(value: &str, now: chrono::DateTime<chrono::Utc>) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    chrono::DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|deadline| {
            (deadline.with_timezone(&chrono::Utc) - now)
                .to_std()
                .unwrap_or_default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_honors_seconds_dates_and_long_provider_delays() {
        let now = chrono::DateTime::parse_from_rfc2822("Wed, 21 Oct 2015 07:28:00 GMT")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            parse_retry_after("7200", now),
            Some(Duration::from_secs(7200))
        );
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:29:00 GMT", now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:27:00 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("invalid", now), None);
    }
}
