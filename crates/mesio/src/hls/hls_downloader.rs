use std::sync::Arc;

use futures::StreamExt;
use hls::HlsData;
use reqwest::Client;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, warn};

use crate::cancel_stream::CancelOnDropStream;
use crate::segment_engine::assembler::AssemblerExit;
use crate::{
    BoxMediaStream, DownloadError, DownloadEvent, DownloadHandle, DownloadRequest, DownloadSession,
    DownloadTerminal, EventSink, MediaEngine, ProtocolSelection, ProtocolType,
    downloader::create_client_pool,
};

use super::engine::{self, EngineHandles, Terminal};
use super::{HlsConfig, HlsStreamEvent};

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

    pub(crate) fn with_clients(
        config: HlsConfig,
        clients: Arc<crate::downloader::ClientPool>,
    ) -> Self {
        Self { clients, config }
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
                warn!(
                    "Playlist watcher task finished with error: {}",
                    crate::redact::redact_diagnostic(&e)
                );
            }
            let mut terminal = match reactor.await {
                Ok(terminal) => {
                    debug!(?terminal, "Reactor task finished");
                    terminal.into()
                }
                Err(e) => {
                    warn!(
                        "Reactor task finished with error: {}",
                        crate::redact::redact_diagnostic(&e)
                    );
                    DownloadTerminal::PipelineError(Arc::from(format!("reactor task failed: {e}")))
                }
            };
            match assembler.await {
                Ok(AssemblerExit::Done) => {}
                Ok(AssemblerExit::Failed(reason)) => {
                    if !matches!(terminal, DownloadTerminal::PipelineError(_)) {
                        terminal = DownloadTerminal::PipelineError(reason);
                    }
                }
                Err(e) => {
                    warn!(
                        "Assembler task finished with error: {}",
                        crate::redact::redact_diagnostic(&e)
                    );
                    if !e.is_cancelled() && !matches!(terminal, DownloadTerminal::PipelineError(_))
                    {
                        terminal = DownloadTerminal::PipelineError(Arc::from(format!(
                            "assembler task failed: {}",
                            crate::redact::redact_diagnostic(&e)
                        )));
                    }
                }
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
