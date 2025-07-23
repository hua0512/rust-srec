use std::sync::Arc;

use crate::media_protocol::{Cacheable, MultiSource};
use futures::StreamExt;
use hls::HlsData;
use reqwest::Client;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use crate::{
    BoxMediaStream, CacheManager, Download, DownloadError, ProtocolBase, SourceManager,
    create_client, hls::HlsDownloaderError,
};

use super::{HlsConfig, HlsStreamCoordinator, HlsStreamEvent};

pub struct HlsDownloader {
    client: Client,
    config: HlsConfig,
}

impl HlsDownloader {
    pub fn new(config: HlsConfig) -> Result<Self, DownloadError> {
        Self::with_config(config)
    }

    /// Create a new HlsDownloader with custom configuration
    pub fn with_config(config: HlsConfig) -> Result<Self, DownloadError> {
        let downloader_config = config.base.clone();
        let client = create_client(&downloader_config)?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &HlsConfig {
        &self.config
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub async fn perform_download(
        &self,
        url: &str,
        _source_manager: Option<&mut SourceManager>,
        cache_manager: Option<Arc<CacheManager>>,
    ) -> Result<BoxMediaStream<HlsData, HlsDownloaderError>, DownloadError> {
        let config = Arc::new(self.config.clone());
        let coordinator = HlsStreamCoordinator::setup_and_spawn(
            url.to_string(),
            config.clone(),
            self.client.clone(),
            cache_manager,
        )
        .await;

        let (client_event_rx, _shutdown_tx, _handles) = coordinator.unwrap();

        let stream = ReceiverStream::new(client_event_rx);

        // map receiver stream to BoxMediaStream
        let stream = stream.filter_map(|event| async move {
            match event {
                Ok(event) => match event {
                    HlsStreamEvent::Data(data) => Some(Ok(*data)),
                    HlsStreamEvent::DiscontinuityTagEncountered { .. } => {
                        debug!("Discontinuity tag encountered");
                        Some(Ok(HlsData::EndMarker))
                    }
                    _ => None,
                },
                Err(e) => Some(Err(e)),
            }
        });

        // Box the stream and return
        Ok(stream.boxed())
    }
}

impl ProtocolBase for HlsDownloader {
    type Config = HlsConfig;

    fn new(config: Self::Config) -> Result<Self, DownloadError> {
        Self::with_config(config)
    }
}

impl Download for HlsDownloader {
    type Data = HlsData;
    type Error = HlsDownloaderError;
    type Stream = BoxMediaStream<Self::Data, Self::Error>;

    async fn download(&self, url: &str) -> Result<Self::Stream, DownloadError> {
        self.perform_download(url, None, None).await
    }
}

impl MultiSource for HlsDownloader {
    async fn download_with_sources(
        &self,
        url: &str, // This is the primary/initial URL
        source_manager: &mut SourceManager,
    ) -> Result<Self::Stream, DownloadError> {
        // Attempt to download using the initial url.
        match self.perform_download(url, Some(source_manager), None).await {
            Ok(stream) => Ok(stream),
            Err(mut last_error) => {
                // If the initial attempt fails, iterate through sources obtained from
                // the source_manager.select_source() method.
                // The loop should continue until a download is successful or
                // source_manager.select_source() returns None.
                while let Some(content_source) = source_manager.select_source() {
                    // Assuming select_source() returns Option<ContentSource> where ContentSource has a url: String.
                    match self.perform_download(&content_source.url, None, None).await {
                        Ok(stream) => return Ok(stream), // Return the Result<Self::Stream, DownloadError> from the first successful download
                        Err(err) => {
                            last_error = err; // Update to the latest error and try next source
                        }
                    }
                }
                // Or an appropriate error if all sources fail.
                Err(last_error)
            }
        }
    }
}

impl Cacheable for HlsDownloader {
    async fn download_with_cache(
        &self,
        url: &str,
        cache_manager: Arc<CacheManager>,
    ) -> Result<Self::Stream, DownloadError> {
        self.perform_download(url, None, Some(cache_manager)).await
    }
}
