use std::sync::LazyLock;

use parking_lot::RwLock;
use regex::Regex;
use reqwest::Client;

use super::error::ExtractorError;
use super::platform_extractor::PlatformExtractor;
use super::streamlink_extractor::StreamlinkExtractor;
use crate::extractor::platforms::{
    self, acfun::Acfun, bigo::Bigo, bilibili::Bilibili, douyin::Douyin, douyu::Douyu, huya::Huya,
    pandatv::PandaTV, picarto::Picarto, redbook::RedBook, soop::Soop, tiktok::TikTok,
    twitcasting::Twitcasting, twitch::Twitch, weibo::Weibo,
};
use crate::proxy::ProxyTarget;

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

/// Builds a platform's extractor from the stream URL, HTTP client, cookies and platform extras.
#[doc(hidden)]
pub type ExtractorConstructor =
    fn(String, Client, Option<String>, Option<serde_json::Value>) -> Box<dyn PlatformExtractor>;

struct PlatformEntry {
    /// The name [`platform_name_for_url`] reports, matched case-insensitively against stored
    /// platform configuration names.
    name: &'static str,
    regex: &'static LazyLock<Regex>,
    constructor: ExtractorConstructor,
}

macro_rules! platform_registry {
    ( $( $name:literal: $regex:path => $builder:path ),+ $(,)? ) => {
        &[
            $(
                PlatformEntry {
                    name: $name,
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

// The first matching entry wins. The Bilibili and Acfun regexes are unanchored, so they also match
// another platform's URL that embeds a link of theirs (in a query string, say); the platforms whose
// URLs can carry one are listed before them so such a URL keeps its own platform and extractor.
static PLATFORMS: &[PlatformEntry] = platform_registry![
    "Huya": platforms::huya::URL_REGEX => Huya::new,
    "Douyin": platforms::douyin::URL_REGEX => Douyin::new,
    "Douyu": platforms::douyu::URL_REGEX => Douyu::new,
    "PandaTV": platforms::pandatv::URL_REGEX => PandaTV::new,
    "Weibo": platforms::weibo::URL_REGEX => Weibo::new,
    "Twitch": platforms::twitch::URL_REGEX => Twitch::new,
    "RedBook": platforms::redbook::URL_REGEX => RedBook::new,
    "Bilibili": platforms::bilibili::URL_REGEX => Bilibili::new,
    "Picarto": platforms::picarto::URL_REGEX => Picarto::new,
    "TikTok": platforms::tiktok::URL_REGEX => TikTok::new,
    "Twitcasting": platforms::twitcasting::URL_REGEX => Twitcasting::new,
    "Acfun": platforms::acfun::URL_REGEX => Acfun::new,
    "SOOP": platforms::soop::URL_REGEX => Soop::new,
    "Bigo": platforms::bigo::URL_REGEX => Bigo::new,
];

struct RegisteredPlatform {
    name: &'static str,
    regex: Regex,
    constructor: ExtractorConstructor,
}

static REGISTERED: RwLock<Vec<RegisteredPlatform>> = RwLock::new(Vec::new());

/// [`register_platform`] refused a name that a built-in or registered platform already uses.
#[doc(hidden)]
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[error("a platform named {0:?} already exists")]
pub struct DuplicatePlatformName(pub &'static str);

/// Adds a platform for the rest of the process, for tests that need an offline platform.
///
/// Registered platforms are consulted after every built-in one, so `regex` never takes a URL from
/// a built-in platform. Names are compared case-insensitively, as stored configuration names are.
#[doc(hidden)]
pub fn register_platform(
    name: &'static str,
    regex: Regex,
    constructor: ExtractorConstructor,
) -> Result<(), DuplicatePlatformName> {
    let mut registered = REGISTERED.write();
    let taken = PLATFORMS
        .iter()
        .map(|platform| platform.name)
        .chain(registered.iter().map(|platform| platform.name))
        .any(|existing| existing.eq_ignore_ascii_case(name));
    if taken {
        return Err(DuplicatePlatformName(name));
    }
    registered.push(RegisteredPlatform {
        name,
        regex,
        constructor,
    });
    Ok(())
}

/// The first platform whose URL regex matches: built-ins first, then registered ones.
fn find_platform(url: &str) -> Option<(&'static str, ExtractorConstructor)> {
    if let Some(platform) = PLATFORMS
        .iter()
        .find(|platform| platform.regex.is_match(url))
    {
        return Some((platform.name, platform.constructor));
    }
    REGISTERED
        .read()
        .iter()
        .find(|platform| platform.regex.is_match(url))
        .map(|platform| (platform.name, platform.constructor))
}

/// The name of the platform [`ExtractorFactory::create_extractor`] dispatches `url` to under
/// [`ExtractorSelection::Auto`], or `None` when no platform claims it.
///
/// The URL regexes are case-sensitive, so callers lowercase the host first.
pub fn platform_name_for_url(url: &str) -> Option<&'static str> {
    find_platform(url).map(|(name, _)| name)
}

/// A factory for creating platform-specific extractors.
pub struct ExtractorFactory {
    client: Client,
    proxy: ProxyTarget,
}

impl ExtractorFactory {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            proxy: ProxyTarget::System,
        }
    }

    /// The path `client` uses, for extractors that make requests from another
    /// process (Streamlink), which the client's proxy does not reach. Without
    /// one the environment decides, as it does for a client without a proxy.
    pub fn with_proxy(mut self, proxy: ProxyTarget) -> Self {
        self.proxy = proxy;
        self
    }

    fn streamlink(
        &self,
        url: &str,
        cookies: Option<String>,
        extras: Option<serde_json::Value>,
    ) -> Result<Box<dyn PlatformExtractor>, ExtractorError> {
        StreamlinkExtractor::new(url.to_string(), self.client.clone(), cookies, extras)
            .map(|extractor| {
                Box::new(extractor.with_proxy(self.proxy.clone())) as Box<dyn PlatformExtractor>
            })
            .or(Err(ExtractorError::UnsupportedExtractor))
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
            return self.streamlink(url, cookies, extras);
        }

        if REDBOOK_PROFILE_URL_REGEX.is_match(url) {
            return Err(ExtractorError::ValidationError(
                "RedBook profile URLs are not supported; use xhslink.com/m or xhslink.com/o share links".to_string(),
            ));
        }

        if let Some((_, constructor)) = find_platform(url) {
            return Ok(constructor(
                url.to_string(),
                self.client.clone(),
                cookies,
                extras,
            ));
        }

        // Automatic fallback: try Streamlink for anything not covered by built-in extractors.
        // If Streamlink isn't available or can't handle the URL, preserve the legacy behavior.
        self.streamlink(url, cookies, extras)
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
    fn registered_platforms_are_matched_after_the_builtins() {
        use crate::extractor::platform_extractor::Extractor;
        use crate::media::MediaInfo;

        struct Registered(Extractor);

        #[async_trait::async_trait]
        impl PlatformExtractor for Registered {
            fn get_extractor(&self) -> &Extractor {
                &self.0
            }

            async fn extract(&self) -> Result<MediaInfo, ExtractorError> {
                Err(ExtractorError::NoStreamsFound)
            }
        }

        let constructor: ExtractorConstructor =
            |url, client, _, _| Box::new(Registered(Extractor::new("RegistryTest", url, client)));
        // Also claims Twitch URLs, which must stay with the built-in Twitch platform.
        let regex = Regex::new(r"^https://(?:registry-test\.invalid|www\.twitch\.tv)/").unwrap();
        register_platform("RegistryTest", regex, constructor).unwrap();

        let room = "https://registry-test.invalid/room";
        assert_eq!(platform_name_for_url(room), Some("RegistryTest"));
        let extractor = ExtractorFactory::new(default_client())
            .create_extractor(room, None, None, ExtractorSelection::Auto)
            .expect("the registered platform claims the URL");
        assert_eq!(extractor.get_extractor().platform_name, "RegistryTest");

        assert_eq!(
            platform_name_for_url("https://www.twitch.tv/someone"),
            Some("Twitch")
        );

        let other = Regex::new(r"^https://other\.invalid/").unwrap();
        for name in ["registrytest", "SOOP", "soop"] {
            assert_eq!(
                register_platform(name, other.clone(), constructor),
                Err(DuplicatePlatformName(name))
            );
        }
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
