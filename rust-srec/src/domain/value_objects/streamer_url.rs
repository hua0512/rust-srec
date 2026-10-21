//! Streamer URL value object.

use platforms_parser::extractor::factory::platform_name_for_url;
use serde::{Deserialize, Serialize};

use crate::Error;

/// `platform_config.platform_name` of the pseudo-platform, seeded as
/// `platform-streamlink`, that hosts every URL no built-in extractor claims but
/// the `streamlink` CLI can handle.
pub const STREAMLINK_PLATFORM: &str = "streamlink";

/// Whether `platform_name` names the Streamlink pseudo-platform. Callers pass
/// both stored lowercase names and the capitalised names URL detection reports.
pub fn is_streamlink_platform(platform_name: &str) -> bool {
    platform_name.eq_ignore_ascii_case(STREAMLINK_PLATFORM)
}

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
        platform_name_for_url(&self.0)
    }

    /// Extract the channel/room identifier from the URL.
    pub fn channel_id(&self) -> Option<String> {
        // For simple cases, we can try to extract from the URL path.
        // However, some platforms might have complex URLs.
        // Ideally, we'd use the platform regexes to capture the ID if they have capture groups.
        // But for now, we'll strip trailing slashes and take the last segment,
        // which works for Twitch, Huya, etc.
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
    fn test_url_normalization() {
        let url = StreamerUrl::new("https://WWW.TWITCH.TV/Streamer123/").unwrap();
        assert_eq!(url.as_str(), "https://www.twitch.tv/Streamer123");
    }

    /// The name is matched against `platform_config.platform_name` and decides which row a new
    /// streamer is stored under, so a URL must keep resolving to the same name.
    #[test]
    fn each_builtin_platform_url_reports_its_platform_name() {
        let cases = [
            ("https://www.huya.com/660000", Some("Huya")),
            ("https://live.douyin.com/123456789", Some("Douyin")),
            ("https://www.douyu.com/9999", Some("Douyu")),
            ("https://www.pandalive.co.kr/play/someone", Some("PandaTV")),
            ("https://weibo.com/u/1234567890", Some("Weibo")),
            ("https://www.twitch.tv/someone", Some("Twitch")),
            ("https://xhslink.com/m/844vKmW30jz", Some("RedBook")),
            ("https://live.bilibili.com/4063253", Some("Bilibili")),
            ("https://picarto.tv/someone", Some("Picarto")),
            ("https://www.tiktok.com/@someone/live", Some("TikTok")),
            ("https://twitcasting.tv/someone", Some("Twitcasting")),
            ("https://live.acfun.cn/live/123456", Some("Acfun")),
            ("https://play.sooplive.co.kr/someone/123", Some("SOOP")),
            ("https://www.bigo.tv/123456", Some("Bigo")),
            ("https://WWW.TWITCH.TV/Someone/", Some("Twitch")),
            ("https://LIVE.BILIBILI.COM/4063253", Some("Bilibili")),
            ("https://Play.SoopLive.co.kr/someone", Some("SOOP")),
            ("https://XHSLINK.COM/o/share123", Some("RedBook")),
            // A link to another platform in the query keeps the URL's own platform.
            (
                "https://www.douyu.com/9999?from=https://live.bilibili.com/4063253",
                Some("Douyu"),
            ),
            ("https://www.xiaohongshu.com/user/profile/6260c44f", None),
            ("https://example.com/someone", None),
        ];

        for (url, expected) in cases {
            let parsed = StreamerUrl::new(url).unwrap();
            assert_eq!(parsed.platform(), expected, "{url}");
        }
    }
}
