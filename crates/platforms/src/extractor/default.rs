
use crate::extractor::platforms::{douyin, douyu, huya, twitch};

use super::factory::ExtractorFactory;
use reqwest::Client;
use rustls::{crypto::ring, ClientConfig};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;


pub(crate)  const DEFAULT_UA : &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

pub fn default_client() -> Client {
        let provider = Arc::new(ring::default_provider());
        let tls_config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("Failed to configure default TLS protocol versions")
            .with_platform_verifier()
            .unwrap()
            .with_no_client_auth();

        return Client::builder()
            .use_preconfigured_tls(tls_config)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");
    }

/// Returns a new `ExtractorFactory` populated with all the supported platforms.
pub fn default_factory() -> ExtractorFactory {
    let client = default_client();
    let mut factory = ExtractorFactory::new(client);

    factory
        .register(
            r"^(?:https?://)?(?:www\.)?huya\.com/(\d+|[a-zA-Z0-9_-]+)",
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