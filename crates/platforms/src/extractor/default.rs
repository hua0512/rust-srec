use super::factory::ExtractorFactory;
use crate::proxy::{ProxyEndpoint, ProxyTarget};
use reqwest::Client;
use rustls::{ClientConfig, crypto::aws_lc_rs};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::{Arc, OnceLock};
use tracing::debug;

pub const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36";
pub(crate) const DEFAULT_MOBILE_UA: &str = "Mozilla/5.0 (iPhone17,1; CPU iPhone OS 18_2_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148 Mohegan Sun/4.7.4";

/// A proxy for [`create_client`] and [`factory_with_proxy`].
pub type ProxyConfig = ProxyEndpoint;

pub fn default_client() -> Client {
    create_client(None)
}

pub fn create_client(proxy_config: Option<ProxyConfig>) -> Client {
    create_client_builder(proxy_config)
        .build()
        .expect("Failed to create HTTP client")
}

pub fn create_client_builder(proxy_config: Option<ProxyConfig>) -> reqwest::ClientBuilder {
    static PROVIDER_INSTALLED: OnceLock<()> = OnceLock::new();
    PROVIDER_INSTALLED.get_or_init(|| {
        if let Err(e) = aws_lc_rs::default_provider().install_default() {
            debug!(existing_provider = ?e, "rustls CryptoProvider already installed");
        }
    });

    let provider = Arc::new(aws_lc_rs::default_provider());
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("Failed to configure default TLS protocol versions")
        .with_platform_verifier()
        .expect("Failed to configure platform TLS verifier")
        .with_no_client_auth();

    let mut builder = Client::builder()
        .use_preconfigured_tls(tls_config)
        // Reqwest auto-decompression is enabled by default when the corresponding
        // crate features are enabled (gzip/deflate/brotli).
        .timeout(std::time::Duration::from_secs(30));

    if let Some(config) = proxy_config {
        match config.reqwest_proxy() {
            Ok(proxy) => {
                builder = builder.proxy(proxy);
            }
            Err(e) => {
                eprintln!(
                    "Warning: Failed to configure proxy '{}': {}; requests through it will fail",
                    crate::proxy::redacted_url(&config.url),
                    e.without_url()
                );
                // Without a proxy the client would follow the environment,
                // sending requests outside the proxy that was asked for.
                builder = builder.proxy(crate::proxy::unroutable_proxy());
            }
        }
    }

    builder
}

/// Returns a new `ExtractorFactory` populated with all the supported platforms.
pub fn default_factory() -> ExtractorFactory {
    let client = default_client();
    ExtractorFactory::new(client)
}

/// Returns a new `ExtractorFactory` with proxy support. Without a proxy the
/// environment decides, as for [`default_factory`].
pub fn factory_with_proxy(proxy_config: Option<ProxyConfig>) -> ExtractorFactory {
    let proxy = proxy_config
        .clone()
        .map_or(ProxyTarget::System, ProxyTarget::Explicit);
    let client = create_client(proxy_config);
    ExtractorFactory::new(client).with_proxy(proxy)
}
