use crate::extractor::platforms::{
    douyin, douyu, huya, pandatv::builder::PandaTV, twitch::Twitch, weibo::builder::Weibo,
};

use super::factory::ExtractorFactory;
use reqwest::Client;
use rustls::{ClientConfig, crypto::ring};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;

pub(crate) const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
pub(crate) const DEFAULT_MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6.1 Mobile/15E148 Safari/604.1";

pub fn default_client() -> Client {
    let provider = Arc::new(ring::default_provider());
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("Failed to configure default TLS protocol versions")
        .with_platform_verifier()
        .unwrap()
        .with_no_client_auth();

    Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("Failed to create HTTP client")
}

/// Returns a new `ExtractorFactory` populated with all the supported platforms.
pub fn default_factory() -> ExtractorFactory {
    let client = default_client();
    let mut factory = ExtractorFactory::new(client);

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?huya\.com/(\d+|[a-zA-Z0-9_-]+)",
            Arc::new(|url, client, cookies| {
                Box::new(huya::HuyaExtractor::new(url, client, cookies))
            }),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?live.douyin\.com/([a-zA-Z0-9_-]+)",
            Arc::new(|url, client, cookies| {
                Box::new(douyin::DouyinExtractorBuilder::new(url, client, cookies).build())
            }),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?douyu\.com/(\d+)",
            Arc::new(|url, client, cookies| {
                Box::new(douyu::DouyuExtractorBuilder::new(url, client, cookies).build(None))
            }),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?pandalive\.co\.kr/play/([a-zA-Z0-9_-]+)",
            Arc::new(|url, client, cookies| Box::new(PandaTV::new(url, client, cookies))),
        )
        .unwrap();

    factory
        .register(
            r"(?:https://(?:www\.)?weibo\.com/(u/\d+|l/wblive/p/show/\d+:[a-zA-Z0-9]+))(?:[?#].*)?$",
            Arc::new(|url, client, cookies| Box::new(Weibo::new(url, client, cookies))),
        )
        .unwrap();

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?twitch\.tv/([a-zA-Z0-9_]+)",
            Arc::new(|url, client, cookies| Box::new(Twitch::new(url, client, cookies))),
        )
        .unwrap();

    factory
}
