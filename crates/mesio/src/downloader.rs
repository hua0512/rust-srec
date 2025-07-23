use reqwest::Client;
use rustls::{ClientConfig, crypto::ring};
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;
use tracing::{debug, info};

use crate::{
    Cacheable, Download, DownloaderConfig, MultiSource, ProtocolBase, RawDownload, RawResumable,
    Resumable,
};
use crate::{DownloadError, proxy::build_proxy_from_config};

/// Create a reqwest Client with the provided configuration
pub fn create_client(config: &DownloaderConfig) -> Result<Client, DownloadError> {
    // Create the crypto provider
    let provider = Arc::new(ring::default_provider());

    // Build platform default TLS configuration
    let tls_config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("Failed to configure default TLS protocol versions")
        .with_platform_verifier()
        .unwrap()
        .with_no_client_auth();

    let mut client_builder = Client::builder()
        .pool_max_idle_per_host(5) // Allow multiple connections to same host
        .user_agent(&config.user_agent)
        .default_headers(config.headers.clone())
        .use_preconfigured_tls(tls_config)
        .redirect(if config.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        });

    if !config.timeout.is_zero() {
        client_builder = client_builder.timeout(config.timeout);
    }

    if !config.connect_timeout.is_zero() {
        client_builder = client_builder.connect_timeout(config.connect_timeout);
    }

    if !config.read_timeout.is_zero() {
        client_builder = client_builder.pool_idle_timeout(config.read_timeout);
    }

    // Set up proxy configuration
    if let Some(proxy_config) = &config.proxy {
        // Explicit proxy configuration takes precedence
        let proxy = match build_proxy_from_config(proxy_config) {
            Ok(p) => p,
            Err(e) => return Err(DownloadError::ProxyError(e)),
        };
        client_builder = client_builder.proxy(proxy);
        info!(proxy_url = %proxy_config.url, "Using explicitly configured proxy for downloads");
    } else if config.use_system_proxy {
        // No explicit proxy but system proxy enabled
        // reqwest will use system proxy settings by default when we don't call no_proxy()
        info!("Using system proxy settings for downloads");
    } else {
        // Explicitly disable proxy
        client_builder = client_builder.no_proxy();
        debug!("Proxy disabled for downloads");
    }

    client_builder.build().map_err(DownloadError::from)
}

use crate::{
    cache::{CacheConfig, CacheManager},
    source::{ContentSource, SourceManager, SourceSelectionStrategy},
};

/// Configuration for the DownloadManager
#[derive(Debug, Clone)]
pub struct DownloadManagerConfig {
    /// Cache configuration
    pub cache_config: Option<CacheConfig>,
    /// Source selection strategy
    pub source_strategy: SourceSelectionStrategy,
    /// Maximum number of source retry attempts
    pub max_retry_count: usize,
    /// Whether to enforce SSL certificate validation
    pub enforce_certificate_validation: bool,
}

impl Default for DownloadManagerConfig {
    fn default() -> Self {
        Self {
            cache_config: Some(CacheConfig::default()),
            source_strategy: SourceSelectionStrategy::default(),
            max_retry_count: 3,
            enforce_certificate_validation: true,
        }
    }
}

/// Modern download manager implementation that uses capability traits
pub struct DownloadManager<P> {
    /// The media protocol handler with its capabilities
    protocol: P,
    /// Manager for multiple content sources
    source_manager: SourceManager,
    /// Optional cache manager
    cache_manager: Option<Arc<CacheManager>>,
    /// Configuration for this download manager
    #[allow(dead_code)]
    config: DownloadManagerConfig,
}

impl<P> DownloadManager<P>
where
    P: ProtocolBase,
{
    /// Create a new download manager with the given protocol handler and default configuration
    pub async fn new(protocol: P) -> Result<Self, DownloadError> {
        Self::with_config(protocol, DownloadManagerConfig::default()).await
    }

    /// Create a new download manager with custom configuration
    pub async fn with_config(
        protocol: P,
        config: DownloadManagerConfig,
    ) -> Result<Self, DownloadError> {
        // Initialize cache if enabled
        let cache_manager = if let Some(cache_config) = &config.cache_config {
            Some(Arc::new(CacheManager::new(cache_config.clone()).await?))
        } else {
            None
        };

        // Create source manager with the specified strategy
        let source_manager = SourceManager::with_strategy(config.source_strategy.clone());

        Ok(Self {
            protocol,
            source_manager,
            cache_manager,
            config,
        })
    }

    /// Add a source URL to the download manager
    pub fn add_source(&mut self, url: impl Into<String>, priority: u8) {
        self.source_manager.add_url(url, priority);
    }

    /// Add a content source with metadata
    pub fn add_content_source(&mut self, source: ContentSource) {
        self.source_manager.add_source(source);
    }
}

// Basic download capability implementation
impl<P> DownloadManager<P>
where
    P: Download,
{
    /// Start a simple download
    pub async fn download(&self, url: &str) -> Result<P::Stream, DownloadError> {
        self.protocol.download(url).await
    }
}

// Implementation when both Download and Resumable capabilities are available
impl<P> DownloadManager<P>
where
    P: Download + Resumable,
{
    /// Resume a download from the specified range
    pub async fn resume(
        &self,
        url: &str,
        range: (u64, Option<u64>),
    ) -> Result<P::Stream, DownloadError> {
        self.protocol.resume(url, range).await
    }

    /// Download with automatic resume if range is provided
    pub async fn download_with_resume(
        &self,
        url: &str,
        range: Option<(u64, Option<u64>)>,
    ) -> Result<P::Stream, DownloadError> {
        match range {
            Some(r) => self.resume(url, r).await,
            None => self.download(url).await,
        }
    }
}

// Implementation when both Download and MultiSource capabilities are available
impl<P> DownloadManager<P>
where
    P: Download + MultiSource,
{
    /// Download with source management
    pub async fn download_with_sources(&mut self, url: &str) -> Result<P::Stream, DownloadError> {
        // If no sources configured yet, use the provided URL
        if !self.source_manager.has_sources() {
            self.source_manager.add_url(url, 0);
        }

        self.protocol
            .download_with_sources(url, &mut self.source_manager)
            .await
    }
}

// Implementation for combined Multi-Source and Cacheable capabilities
impl<P> DownloadManager<P>
where
    P: Download + MultiSource + Cacheable,
{
    /// Download with sources and caching
    pub async fn download_with_sources_and_cache(
        &mut self,
        url: &str,
    ) -> Result<P::Stream, DownloadError> {
        // If no sources configured yet, use the provided URL
        if !self.source_manager.has_sources() {
            self.source_manager.add_url(url, 0);
        }

        // Try cache first if available
        if let Some(cache_manager) = &self.cache_manager {
            match self
                .protocol
                .download_with_cache(url, cache_manager.clone())
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(_) => {
                    // Cache miss, fall through to source download
                }
            }
        }

        // Cache miss or no cache, try sources
        self.protocol
            .download_with_sources(url, &mut self.source_manager)
            .await
    }
}

// Implementation for RawDownload capability
impl<P> DownloadManager<P>
where
    P: RawDownload,
{
    /// Download raw bytes
    pub async fn download_raw(&self, url: &str) -> Result<P::RawStream, DownloadError> {
        self.protocol.download_raw(url).await
    }
}

// Implementation when RawDownload and RawResumable capabilities are available
impl<P> DownloadManager<P>
where
    P: RawDownload + RawResumable,
{
    /// Resume a raw download from the specified range
    pub async fn resume_raw(
        &self,
        url: &str,
        range: (u64, Option<u64>),
    ) -> Result<P::RawStream, DownloadError> {
        self.protocol.resume_raw(url, range).await
    }

    /// Download raw with automatic resume if range is provided
    pub async fn download_raw_with_resume(
        &self,
        url: &str,
        range: Option<(u64, Option<u64>)>,
    ) -> Result<P::RawStream, DownloadError> {
        match range {
            Some(r) => self.resume_raw(url, r).await,
            None => self.download_raw(url).await,
        }
    }
}
