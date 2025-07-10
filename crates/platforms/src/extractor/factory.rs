use std::sync::LazyLock;

use super::error::ExtractorError;
use super::platform_extractor::PlatformExtractor;
use crate::extractor::platforms::{
    self, bilibili::Bilibili, douyin::DouyinExtractorBuilder, douyu::DouyuExtractorBuilder,
    huya::HuyaExtractor, pandatv::PandaTV, picarto::Picarto, redbook::RedBook, tiktok::TikTok,
    twitcasting::Twitcasting, twitch::Twitch, weibo::Weibo,
};
use regex::Regex;
use reqwest::Client;

// A type alias for a thread-safe constructor function.
type ExtractorConstructor =
    fn(String, Client, Option<String>, Option<serde_json::Value>) -> Box<dyn PlatformExtractor>;

struct PlatformEntry {
    regex: &'static LazyLock<Regex>,
    constructor: ExtractorConstructor,
}

// Static platform registry
// Macro to create a constructor function for a given platform
macro_rules! create_constructor {
    ($name:ident, $builder:expr) => {
        fn $name(
            url: String,
            client: Client,
            cookies: Option<String>,
            extras: Option<serde_json::Value>,
        ) -> Box<dyn PlatformExtractor> {
            Box::new($builder(url, client, cookies, extras))
        }
    };
}

// Create constructor functions using the macro
create_constructor!(new_huya, HuyaExtractor::new);
create_constructor!(new_douyin, |url, client, cookies, extras| {
    DouyinExtractorBuilder::new(url, client, cookies, extras).build()
});
#[cfg(feature = "douyu")]
create_constructor!(new_douyu, |url, client, cookies, extras| {
    DouyuExtractorBuilder::new(url, client, cookies, extras).build(None)
});
create_constructor!(new_pandatv, PandaTV::new);
create_constructor!(new_weibo, Weibo::new);
create_constructor!(new_twitch, Twitch::new);
create_constructor!(new_redbook, RedBook::new);
create_constructor!(new_bilibili, Bilibili::new);
create_constructor!(new_picarto, Picarto::new);
create_constructor!(new_tiktok, TikTok::new);
create_constructor!(new_twitcasting, Twitcasting::new);

// Static platform registry
static PLATFORMS: &[PlatformEntry] = &[
    PlatformEntry {
        regex: &platforms::huya::URL_REGEX,
        constructor: new_huya,
    },
    PlatformEntry {
        regex: &platforms::douyin::URL_REGEX,
        constructor: new_douyin,
    },
    #[cfg(feature = "douyu")]
    PlatformEntry {
        regex: &platforms::douyu::URL_REGEX,
        constructor: new_douyu,
    },
    PlatformEntry {
        regex: &platforms::pandatv::URL_REGEX,
        constructor: new_pandatv,
    },
    PlatformEntry {
        regex: &platforms::weibo::URL_REGEX,
        constructor: new_weibo,
    },
    PlatformEntry {
        regex: &platforms::twitch::URL_REGEX,
        constructor: new_twitch,
    },
    PlatformEntry {
        regex: &platforms::redbook::URL_REGEX,
        constructor: new_redbook,
    },
    PlatformEntry {
        regex: &platforms::bilibili::URL_REGEX,
        constructor: new_bilibili,
    },
    PlatformEntry {
        regex: &platforms::picarto::URL_REGEX,
        constructor: new_picarto,
    },
    PlatformEntry {
        regex: &platforms::tiktok::URL_REGEX,
        constructor: new_tiktok,
    },
    PlatformEntry {
        regex: &platforms::twitcasting::URL_REGEX,
        constructor: new_twitcasting,
    },
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
        Err(ExtractorError::UnsupportedExtractor)
    }
}
