//! Streamer URL value object.

use crate::Error;
use serde::{Deserialize, Serialize};

/// A validated streamer URL.
///
/// This value object ensures that streamer URLs are valid and provides
/// utilities for extracting platform information.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamerUrl(String);

impl StreamerUrl {
    /// Create a new StreamerUrl from a string, validating it.
    pub fn new(url: impl Into<String>) -> Result<Self, Error> {
        let url = url.into();
        Self::validate(&url)?;
        Ok(Self(Self::normalize(&url)))
    }

    /// Create a StreamerUrl without validation (for trusted sources like DB).
    pub fn from_trusted(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// Get the URL as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Get the platform name from the URL.
    pub fn platform(&self) -> Option<&'static str> {
        let url_lower = self.0.to_lowercase();

        if url_lower.contains("twitch.tv") {
            Some("Twitch")
        } else if url_lower.contains("huya.com") {
            Some("Huya")
        } else if url_lower.contains("douyu.com") {
            Some("Douyu")
        } else if url_lower.contains("bilibili.com") {
            Some("Bilibili")
        } else if url_lower.contains("youtube.com") || url_lower.contains("youtu.be") {
            Some("YouTube")
        } else {
            None
        }
    }

    /// Extract the channel/room identifier from the URL.
    pub fn channel_id(&self) -> Option<String> {
        // Extract the last path segment as the channel ID
        let url = self.0.trim_end_matches('/');
        url.rsplit('/').next().map(|s| s.to_string())
    }

    /// Validate a URL string.
    fn validate(url: &str) -> Result<(), Error> {
        if url.is_empty() {
            return Err(Error::validation("URL cannot be empty"));
        }

        // Basic URL validation
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(Error::validation("URL must start with http:// or https://"));
        }

        // Check for valid characters
        if url.contains(char::is_whitespace) {
            return Err(Error::validation("URL cannot contain whitespace"));
        }

        Ok(())
    }

    /// Normalize a URL (remove trailing slashes, lowercase domain).
    fn normalize(url: &str) -> String {
        let url = url.trim_end_matches('/');

        // Find the end of the domain part
        if let Some(pos) = url.find("://") {
            let (scheme, rest) = url.split_at(pos + 3);
            if let Some(path_start) = rest.find('/') {
                let (domain, path) = rest.split_at(path_start);
                format!("{}{}{}", scheme, domain.to_lowercase(), path)
            } else {
                format!("{}{}", scheme, rest.to_lowercase())
            }
        } else {
            url.to_string()
        }
    }
}

impl std::fmt::Display for StreamerUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for StreamerUrl {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<StreamerUrl> for String {
    fn from(url: StreamerUrl) -> Self {
        url.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_url() {
        let url = StreamerUrl::new("https://www.twitch.tv/streamer123").unwrap();
        assert_eq!(url.as_str(), "https://www.twitch.tv/streamer123");
    }

    #[test]
    fn test_url_normalization() {
        let url = StreamerUrl::new("https://WWW.TWITCH.TV/Streamer123/").unwrap();
        assert_eq!(url.as_str(), "https://www.twitch.tv/Streamer123");
    }

    #[test]
    fn test_invalid_url_empty() {
        let result = StreamerUrl::new("");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_url_no_scheme() {
        let result = StreamerUrl::new("www.twitch.tv/streamer");
        assert!(result.is_err());
    }

    #[test]
    fn test_platform_detection() {
        let twitch = StreamerUrl::new("https://www.twitch.tv/streamer").unwrap();
        assert_eq!(twitch.platform(), Some("Twitch"));

        let huya = StreamerUrl::new("https://www.huya.com/123456").unwrap();
        assert_eq!(huya.platform(), Some("Huya"));

        let unknown = StreamerUrl::new("https://unknown.com/streamer").unwrap();
        assert_eq!(unknown.platform(), None);
    }

    #[test]
    fn test_channel_id() {
        let url = StreamerUrl::new("https://www.twitch.tv/streamer123").unwrap();
        assert_eq!(url.channel_id(), Some("streamer123".to_string()));

        let url_with_slash = StreamerUrl::new("https://www.huya.com/123456/").unwrap();
        assert_eq!(url_with_slash.channel_id(), Some("123456".to_string()));
    }

    #[test]
    fn test_serialization() {
        let url = StreamerUrl::new("https://www.twitch.tv/streamer").unwrap();
        let json = serde_json::to_string(&url).unwrap();
        assert_eq!(json, "\"https://www.twitch.tv/streamer\"");

        let parsed: StreamerUrl = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, url);
    }
}
