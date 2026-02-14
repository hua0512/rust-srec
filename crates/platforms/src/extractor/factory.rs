use std::sync::LazyLock;

use super::error::ExtractorError;
use super::platform_extractor::PlatformExtractor;
use super::streamlink_extractor::StreamlinkExtractor;
use crate::extractor::platforms::{
    self, acfun::Acfun, bilibili::Bilibili, douyin::Douyin, douyu::Douyu, huya::Huya,
    pandatv::PandaTV, picarto::Picarto, redbook::RedBook, tiktok::TikTok, twitcasting::Twitcasting,
    twitch::Twitch, weibo::Weibo,
};
use regex::Regex;
use reqwest::Client;

static REDBOOK_PROFILE_URL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:https?://)?(?:www\.)?xiaohongshu\.com/user/profile/").unwrap()
});

// A type alias for a thread-safe constructor function.
type ExtractorConstructor =
    fn(String, Client, Option<String>, Option<serde_json::Value>) -> Box<dyn PlatformExtractor>;

struct PlatformEntry {
    regex: &'static LazyLock<Regex>,
    constructor: ExtractorConstructor,
}

macro_rules! platform_registry {
    ( $( $regex:path => $builder:path ),+ $(,)? ) => {
        &[
            $(
                PlatformEntry {
                    regex: &$regex,
                    constructor: |url, client, cookies, extras| {
                        Box::new($builder(url, client, cookies, extras))
                            as Box<dyn PlatformExtractor>
                    },
                },
            )+
        ]
    };
}

// Static platform registry.
static PLATFORMS: &[PlatformEntry] = platform_registry![
    platforms::huya::URL_REGEX => Huya::new,
    platforms::douyin::URL_REGEX => Douyin::new,
    platforms::douyu::URL_REGEX => Douyu::new,
    platforms::pandatv::URL_REGEX => PandaTV::new,
    platforms::weibo::URL_REGEX => Weibo::new,
    platforms::twitch::URL_REGEX => Twitch::new,
    platforms::redbook::URL_REGEX => RedBook::new,
    platforms::bilibili::URL_REGEX => Bilibili::new,
    platforms::picarto::URL_REGEX => Picarto::new,
    platforms::tiktok::URL_REGEX => TikTok::new,
    platforms::twitcasting::URL_REGEX => Twitcasting::new,
    platforms::acfun::URL_REGEX => Acfun::new,
];

/// A factory for creating platform-specific extractors.
pub struct ExtractorFactory {
    client: Client,
}

impl ExtractorFactory {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn create_extractor(
        &self,
        url: &str,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Result<Box<dyn PlatformExtractor>, ExtractorError> {
        if REDBOOK_PROFILE_URL_REGEX.is_match(url) {
            return Err(ExtractorError::ValidationError(
                "RedBook profile URLs are not supported; use xhslink.com/m share links".to_string(),
            ));
        }

        for platform in PLATFORMS {
            if platform.regex.is_match(url) {
                return Ok((platform.constructor)(
                    url.to_string(),
                    self.client.clone(),
                    cookies,
                    extras,
                ));
            }
        }

        // Automatic fallback: try Streamlink for anything not covered by built-in extractors.
        // If Streamlink isn't available or can't handle the URL, preserve the legacy behavior.
        StreamlinkExtractor::new(url.to_string(), self.client.clone(), cookies, extras)
            .map(|e| Box::new(e) as Box<dyn PlatformExtractor>)
            .or(Err(ExtractorError::UnsupportedExtractor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::default::default_client;

    #[test]
    fn profile_urls_fail_before_streamlink_fallback() {
        let factory = ExtractorFactory::new(default_client());
        let err = factory
            .create_extractor(
                "https://www.xiaohongshu.com/user/profile/6260c44f0000000010006079",
                None,
                None,
            )
            .err()
            .expect("expected error");

        assert!(matches!(err, ExtractorError::ValidationError(_)));
    }
}
