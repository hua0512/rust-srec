use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tracing::warn;

use futures::{Stream, StreamExt};
use hls::HlsData;
use reqwest::Client;
use tokio_stream::wrappers::ReceiverStream;
use tracing::debug;

use crate::{
    BoxMediaStream, DownloadError, downloader::create_client_pool, hls::HlsDownloaderError,
};
use crate::{
    DownloadEvent, DownloadRequest, DownloadSession, EventSink, MediaEngine, ProtocolSelection,
    ProtocolType,
};
use crate::{DownloadHandle, DownloadTerminal};
use tokio_util::sync::CancellationToken;

use super::engine::{self, EngineHandles, Terminal};
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

    pub async fn start_session(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<HlsData>, DownloadError> {
        let token = request.cancel.unwrap_or_default();
        let engine_token = token.child_token();
        let event_capacity =
            (self.config.scheduler_config.download_concurrency.max(1) * 64).max(256);
        let (events, event_stream) = EventSink::channel(event_capacity);

        events.emit(DownloadEvent::Started {
            protocol: ProtocolType::Hls,
            url: Arc::from(request.url.as_str()),
        });

        let mut config = self.config.clone();
        if let ProtocolSelection::Hls(options) = &request.protocol
            && let Some(policy) = options.variant_selection_policy.clone()
        {
            config.playlist_config.variant_selection_policy = policy;
        }
        let config = Arc::new(config);
        let (client_event_rx, handles) = engine::start_with_events(
            request.url.to_string(),
            config,
            Arc::clone(&self.clients),
            request.cache,
            engine_token.clone(),
            Some(events.clone()),
        )
        .await?;

        let stream_events = events.clone();
        let stream = ReceiverStream::new(client_event_rx).filter_map(move |event| {
            let events = stream_events.clone();
            async move {
                match event {
                    Ok(event) => match event {
                        HlsStreamEvent::Data(data) => Some(Ok(*data)),
                        HlsStreamEvent::DiscontinuityTagEncountered { .. } => Some(Ok(
                            HlsData::end_marker_with_reason(hls::SplitReason::Discontinuity),
                        )),
                        HlsStreamEvent::EndlistEncountered => None,
                        HlsStreamEvent::StreamEnded => Some(Ok(HlsData::end_marker_with_reason(
                            hls::SplitReason::EndOfStream,
                        ))),
                        HlsStreamEvent::PlaylistRefreshed {
                            media_sequence_base,
                            target_duration,
                        } => {
                            events.emit(DownloadEvent::PlaylistRefreshed {
                                media_sequence_base,
                                target_duration,
                            });
                            None
                        }
                        HlsStreamEvent::SegmentTimeout {
                            sequence_number,
                            waited_duration,
                        } => {
                            events.emit(DownloadEvent::SegmentTimeout {
                                sequence_number,
                                waited: waited_duration,
                            });
                            None
                        }
                        HlsStreamEvent::GapSkipped {
                            from_sequence,
                            to_sequence,
                            reason,
                        } => {
                            events.emit(DownloadEvent::GapSkipped {
                                from_sequence,
                                to_sequence,
                                reason,
                            });
                            None
                        }
                    },
                    Err(e) => Some(Err(e)),
                }
            }
        });

        let metrics = handles.performance_metrics.clone();
        let lifecycle = tokio::spawn(async move {
            let EngineHandles {
                watcher,
                reactor,
                assembler,
                ..
            } = handles;

            if let Err(e) = watcher.await {
                warn!("Playlist watcher task finished with error: {:?}", e);
            }
            let terminal = match reactor.await {
                Ok(terminal) => {
                    debug!(?terminal, "Reactor task finished");
                    terminal.into()
                }
                Err(e) => {
                    warn!("Reactor task finished with error: {:?}", e);
                    DownloadTerminal::PipelineError(Arc::from(format!("reactor task failed: {e}")))
                }
            };
            if let Err(e) = assembler.await {
                warn!("Assembler task finished with error: {:?}", e);
            }

            debug!("HLS pipeline tasks finished.");
            terminal
        });

        let stream: BoxMediaStream<HlsData, DownloadError> = Box::pin(CancelOnDropStream::new(
            stream.boxed(),
            engine_token.clone(),
        ));

        Ok(DownloadSession {
            items: stream,
            events: event_stream,
            handle: DownloadHandle::new(
                engine_token,
                metrics,
                events.dropped_counter(),
                Some(lifecycle),
            ),
        })
    }
}

impl From<Terminal> for DownloadTerminal {
    fn from(value: Terminal) -> Self {
        match value {
            Terminal::AuthoritativeEnd => Self::AuthoritativeEnd,
            Terminal::Cancelled => Self::Cancelled,
            Terminal::DownstreamClosed => Self::DownstreamClosed,
            Terminal::PipelineError(reason) => Self::PipelineError(reason),
        }
    }
}

impl MediaEngine for HlsDownloader {
    type Item = HlsData;

    async fn start(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<Self::Item>, DownloadError> {
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Hls(Default::default());
        }
        self.start_session(request).await
    }
}
