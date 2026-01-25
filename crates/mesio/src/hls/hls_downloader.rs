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
    BoxMediaStream, CacheManager, Download, DownloadError, ProtocolBase, SourceManager,
    downloader::create_client_pool, hls::HlsDownloaderError,
};
use tokio_util::sync::CancellationToken;

use super::{HlsConfig, HlsStreamCoordinator, HlsStreamEvent, coordinator::AllTaskHandles};

pub struct HlsDownloader {
    clients: Arc<crate::downloader::ClientPool>,
    config: HlsConfig,
}

impl HlsDownloader {
    pub fn new(config: HlsConfig) -> Result<Self, DownloadError> {
        Self::with_config(config)
    }

    /// Create a new HlsDownloader with custom configuration
    pub fn with_config(config: HlsConfig) -> Result<Self, DownloadError> {
        let downloader_config = config.base.clone();
        let clients = Arc::new(create_client_pool(&downloader_config)?);
        Ok(Self { clients, config })
    }

    pub fn config(&self) -> &HlsConfig {
        &self.config
    }

    pub fn client(&self) -> &Client {
        self.clients.default_client()
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

        // Capture current span for HLS segment downloads to be children
        let parent_span = tracing::Span::current();
        let parent_span = if parent_span.is_none() {
            None
        } else {
            Some(parent_span)
        };

        let (client_event_rx, handles) = HlsStreamCoordinator::setup_and_spawn(
            url.to_string(),
            config.clone(),
            Arc::clone(&self.clients),
            cache_manager,
            token,
            parent_span,
        )
        .await
        .map_err(DownloadError::HlsError)?;

        let stream = ReceiverStream::new(client_event_rx);

        // Spawn a separate task to await the completion of all pipeline components.
        // This ensures that graceful shutdown logic is fully executed.
        tokio::spawn(async move {
            let AllTaskHandles {
                playlist_engine_handle,
                scheduler_handle,
                output_manager_handle,
                http2_stats,
            } = handles;

            // It's important to await all handles to ensure cleanup.
            if let Some(handle) = playlist_engine_handle
                && let Err(e) = handle.await
            {
                warn!("Playlist engine task finished with error: {:?}", e);
            }

            if let Err(e) = scheduler_handle.await {
                warn!("Scheduler task finished with error: {:?}", e);
            }
            if let Err(e) = output_manager_handle.await {
                warn!("Output manager task finished with error: {:?}", e);
            }

            // Log HTTP/2 connection statistics
            http2_stats.log_summary();

            debug!("All HLS pipeline tasks have completed.");
        });

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
