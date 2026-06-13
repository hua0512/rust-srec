use crate::source::ContentSource;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tracing::warn;

use crate::media_protocol::{Cacheable, MultiSource};
use futures::{Stream, StreamExt};
use hls::HlsData;
use reqwest::Client;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use crate::{
    BoxMediaStream, CacheManager, Download, DownloadError, ProtocolBase, SourceManager,
    downloader::create_client_pool, hls::HlsDownloaderError,
};
use tokio_util::sync::CancellationToken;

use super::engine::{self, EngineHandles};
use super::{HlsConfig, HlsStreamEvent};

struct CancelOnDropStream {
    inner: BoxMediaStream<HlsData, HlsDownloaderError>,
    token: CancellationToken,
}

impl CancelOnDropStream {
    fn new(inner: BoxMediaStream<HlsData, HlsDownloaderError>, token: CancellationToken) -> Self {
        Self { inner, token }
    }
}

impl Stream for CancelOnDropStream {
    type Item = Result<HlsData, HlsDownloaderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for CancelOnDropStream {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

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
        let engine_token = token.child_token();

        let (client_event_rx, handles) = engine::start(
            url.to_string(),
            config,
            Arc::clone(&self.clients),
            cache_manager,
            engine_token.clone(),
        )
        .await?;

        let stream = ReceiverStream::new(client_event_rx);

        // Await the pipeline tasks off to the side so graceful-shutdown logic
        // (reactor drain, assembler flush) always runs to completion.
        tokio::spawn(async move {
            let EngineHandles {
                watcher,
                reactor,
                assembler,
                ..
            } = handles;

            if let Err(e) = watcher.await {
                warn!("Playlist watcher task finished with error: {:?}", e);
            }
            match reactor.await {
                Ok(terminal) => debug!(?terminal, "Reactor task finished"),
                Err(e) => warn!("Reactor task finished with error: {:?}", e),
            }
            if let Err(e) = assembler.await {
                warn!("Assembler task finished with error: {:?}", e);
            }

            debug!("HLS pipeline tasks finished.");
        });

        // map receiver stream to BoxMediaStream
        let stream = stream.filter_map(|event| async move {
            match event {
                Ok(event) => match event {
                    HlsStreamEvent::Data(data) => Some(Ok(*data)),
                    HlsStreamEvent::DiscontinuityTagEncountered { .. } => {
                        debug!("Discontinuity tag encountered");
                        Some(Ok(HlsData::end_marker_with_reason(
                            hls::SplitReason::Discontinuity,
                        )))
                    }
                    HlsStreamEvent::EndlistEncountered => {
                        debug!("ENDLIST encountered");
                        None
                    }
                    HlsStreamEvent::StreamEnded => {
                        debug!("HLS stream ended, emitting EndOfStream marker");
                        Some(Ok(HlsData::end_marker_with_reason(
                            hls::SplitReason::EndOfStream,
                        )))
                    }
                    _ => None,
                },
                Err(e) => Some(Err(e)),
            }
        });

        Ok(Box::pin(CancelOnDropStream::new(
            stream.boxed(),
            engine_token,
        )))
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
        Err(last_error.unwrap_or_else(|| DownloadError::source_exhausted("No source available")))
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
