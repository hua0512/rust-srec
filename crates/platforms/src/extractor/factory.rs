use std::sync::LazyLock;

use super::error::ExtractorError;
use super::platform_extractor::PlatformExtractor;
use super::streamlink_extractor::StreamlinkExtractor;
use crate::extractor::platforms::{
    self, acfun::Acfun, bigo::Bigo, bilibili::Bilibili, douyin::Douyin, douyu::Douyu, huya::Huya,
    pandatv::PandaTV, picarto::Picarto, redbook::RedBook, soop::Soop, tiktok::TikTok,
    twitcasting::Twitcasting, twitch::Twitch, weibo::Weibo,
};
use regex::Regex;
use reqwest::Client;

/// Which extractor resolves a stream URL.
///
/// [`Self::Auto`] dispatches on the URL regex registry and falls through to
/// [`StreamlinkExtractor`] only when no built-in platform claims the URL.
/// [`Self::Streamlink`] forces the CLI extractor even for a URL a built-in platform would
/// otherwise handle, which is the escape hatch when a native extractor breaks upstream.
///
/// This is independent of the download engine: the extractor resolves the stream URL, the engine
/// downloads it. Selecting the streamlink *engine* leaves extraction with the native extractor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtractorSelection {
    #[default]
    Auto,
    Streamlink,
}

impl ExtractorSelection {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Streamlink => "streamlink",
        }
    }
}

impl std::str::FromStr for ExtractorSelection {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "streamlink" => Ok(Self::Streamlink),
            _ => Err(format!("Unknown extractor: {s}")),
        }
    }
}

impl std::fmt::Display for ExtractorSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

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
    platforms::soop::URL_REGEX => Soop::new,
    platforms::bigo::URL_REGEX => Bigo::new,
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
        selection: ExtractorSelection,
    ) -> Result<Box<dyn PlatformExtractor>, ExtractorError> {
        // Checked before the RedBook guard below: that guard steers users toward share links for
        // the native RedBook extractor, which is moot once streamlink is doing the extraction.
        if selection == ExtractorSelection::Streamlink {
            return StreamlinkExtractor::new(url.to_string(), self.client.clone(), cookies, extras)
                .map(|e| Box::new(e) as Box<dyn PlatformExtractor>)
                .or(Err(ExtractorError::UnsupportedExtractor));
        }

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
                ExtractorSelection::Auto,
            )
            .err()
            .expect("expected error");

        assert!(matches!(err, ExtractorError::ValidationError(_)));
    }

    /// A URL a built-in platform claims must still go to that platform under `Auto`.
    #[test]
    fn auto_prefers_the_builtin_platform() {
        let factory = ExtractorFactory::new(default_client());
        let extractor = factory
            .create_extractor(
                "https://www.twitch.tv/someone",
                None,
                None,
                ExtractorSelection::Auto,
            )
            .expect("twitch is a built-in platform");

        assert_eq!(extractor.get_extractor().platform_name, "Twitch");
    }

    /// Forcing streamlink bypasses the registry, which is the whole point of the setting: it is
    /// the only recourse when a native extractor breaks upstream.
    ///
    /// Skipped when the `streamlink` CLI is absent, since `StreamlinkExtractor::new` probes for it.
    #[test]
    fn streamlink_selection_overrides_the_builtin_platform() {
        if !StreamlinkExtractor::is_available() {
            return;
        }

        let factory = ExtractorFactory::new(default_client());
        let extractor = factory
            .create_extractor(
                "https://www.twitch.tv/someone",
                None,
                None,
                ExtractorSelection::Streamlink,
            )
            .expect("streamlink is available");

        assert_eq!(extractor.get_extractor().platform_name, "Streamlink");
    }

    #[test]
    fn extractor_selection_round_trips_through_strings() {
        assert_eq!(
            "streamlink".parse::<ExtractorSelection>().unwrap(),
            ExtractorSelection::Streamlink
        );
        assert_eq!(
            "auto".parse::<ExtractorSelection>().unwrap(),
            ExtractorSelection::Auto
        );
        assert_eq!(ExtractorSelection::default(), ExtractorSelection::Auto);
        assert!("ffmpeg".parse::<ExtractorSelection>().is_err());
    }
}
