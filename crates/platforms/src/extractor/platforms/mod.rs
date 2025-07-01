use super::factory::ExtractorFactory;
use reqwest::Client;
use std::sync::Arc;

pub mod douyin;
pub mod douyu;
pub mod huya;
pub mod twitch;

/// Returns a new `ExtractorFactory` populated with all the supported platforms.
pub fn default_factory() -> ExtractorFactory {
    let client = Client::new();
    let mut factory = ExtractorFactory::new(client);

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?huya\.com/(\d+)",
            Arc::new(|url, client| Box::new(huya::HuyaExtractor::new(url, client))),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?douyin\.com/([a-zA-Z0-9_-]+)",
            Arc::new(|url, client| Box::new(douyin::DouyinExtractor::new(url, client))),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?douyu\.com/(\d+)",
            Arc::new(|url, client| Box::new(douyu::DouyuExtractor::new(url, client))),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?twitch\.tv/([a-zA-Z0-9_]+)",
            Arc::new(|url, client| Box::new(twitch::TwitchExtractor::new(url, client))),
        )
        .unwrap();

    factory
}