use crate::source::ContentSource;
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

use crate::media_protocol::{Cacheable, MultiSource};
use futures::StreamExt;
use hls::HlsData;
use reqwest::Client;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use crate::{
    create_client, hls::HlsDownloaderError, BoxMediaStream, CacheManager, Download, DownloadError,
    ProtocolBase, SourceManager,
};
use tokio_util::sync::CancellationToken;

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

    async fn try_download_from_source(
        &self,
        source: &ContentSource,
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<HlsData, HlsDownloaderError>, DownloadError> {
        let start_time = Instant::now();
        match self
            .perform_download(&source.url, Some(source_manager), None, token)
            .await
        {
            Ok(stream) => {
                let elapsed = start_time.elapsed();
                source_manager.record_success(&source.url, elapsed);
                Ok(stream)
            }
            Err(err) => {
                let elapsed = start_time.elapsed();
                source_manager.record_failure(&source.url, &err, elapsed);
                warn!(
                    url = %source.url,
                    error = %err,
                    "Failed to download from source"
                );
                Err(err)
            }
        }
    }

    pub async fn perform_download(
        &self,
        url: &str,
        _source_manager: Option<&mut SourceManager>,
        cache_manager: Option<Arc<CacheManager>>,
        token: CancellationToken,
    ) -> Result<BoxMediaStream<HlsData, HlsDownloaderError>, DownloadError> {
        let config = Arc::new(self.config.clone());
        let (client_event_rx, _shutdown_tx, _handles) = HlsStreamCoordinator::setup_and_spawn(
            url.to_string(),
            config.clone(),
            self.client.clone(),
            cache_manager,
            token,
        )
        .await
        .map_err(DownloadError::HlsError)?;

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

    async fn download(
        &self,
        url: &str,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        self.perform_download(url, None, None, token).await
    }
}

impl MultiSource for HlsDownloader {
    async fn download_with_sources(
        &self,
        url: &str,
        source_manager: &mut SourceManager,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        if !source_manager.has_sources() {
            source_manager.add_url(url, 0);
        }

        let mut last_error: Option<DownloadError> = None;

        while let Some(content_source) = source_manager.select_source() {
            match self
                .try_download_from_source(&content_source, source_manager, token.clone())
                .await
            {
                Ok(stream) => return Ok(stream),
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| DownloadError::NoSource("No source available".to_string())))
    }
}

impl Cacheable for HlsDownloader {
    async fn download_with_cache(
        &self,
        url: &str,
        cache_manager: Arc<CacheManager>,
        token: CancellationToken,
    ) -> Result<Self::Stream, DownloadError> {
        self.perform_download(url, None, Some(cache_manager), token)
            .await
    }
}
