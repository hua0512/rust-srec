use super::factory::ExtractorFactory;
use reqwest::Client;
use rustls::{ClientConfig, crypto::ring};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;

pub(crate) const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
pub(crate) const DEFAULT_MOBILE_UA: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_6_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.6.1 Mobile/15E148 Safari/604.1";

#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}

pub fn default_client() -> Client {
    create_client(None)
}

pub fn create_client(proxy_config: Option<ProxyConfig>) -> Client {
    let provider = Arc::new(ring::default_provider());
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("Failed to configure default TLS protocol versions")
        .with_platform_verifier()
        .unwrap()
        .with_no_client_auth();

    let mut builder = Client::builder()
        .use_preconfigured_tls(tls_config)
        .timeout(std::time::Duration::from_secs(30));

    if let Some(config) = proxy_config {
        match reqwest::Proxy::all(&config.url) {
            Ok(mut proxy) => {
                if let (Some(username), Some(password)) = (config.username, config.password) {
                    proxy = proxy.basic_auth(&username, &password);
                }
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                eprintln!("Warning: Failed to configure proxy '{}': {}", config.url, e);
            }
        }
    }

    builder.build().expect("Failed to create HTTP client")
}

/// Returns a new `ExtractorFactory` populated with all the supported platforms.
pub fn default_factory() -> ExtractorFactory {
    let client = default_client();
    ExtractorFactory::new(client)
}

/// Returns a new `ExtractorFactory` with proxy support.
pub fn factory_with_proxy(proxy_config: Option<ProxyConfig>) -> ExtractorFactory {
    let client = create_client(proxy_config);
    ExtractorFactory::new(client)
}
