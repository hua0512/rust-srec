use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use hls::HlsData;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::dash::config::DashConfig;
use crate::dash::engine;
use crate::downloader::{ClientPool, create_client_pool};
use crate::{
    BoxMediaStream, DownloadError, DownloadEvent, DownloadHandle, DownloadRequest, DownloadSession,
    EventSink, MediaEngine, ProtocolSelection, ProtocolType,
};

/// Cancels the engine when the consumer drops the item stream.
struct CancelOnDropStream {
    inner: BoxMediaStream<HlsData, DownloadError>,
    token: CancellationToken,
}

impl Stream for CancelOnDropStream {
    type Item = Result<HlsData, DownloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for CancelOnDropStream {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

/// Records MPEG-DASH presentations as one fragmented MP4 stream.
///
/// Items are [`HlsData`] so DASH output shares the fMP4 processing and
/// writing path with HLS: an init segment, then media items, with end
/// markers where the init changes. A static or ended presentation finishes
/// with an `EndOfStream` end marker and [`crate::DownloadTerminal::AuthoritativeEnd`].
/// A live stream that restarts its segment numbering finishes the same way
/// once everything before the restart is written, but with
/// [`crate::DownloadTerminal::Restarted`]: a new session records what follows.
pub struct DashDownloader {
    clients: Arc<ClientPool>,
    config: DashConfig,
}

impl DashDownloader {
    pub fn new(config: DashConfig) -> Result<Self, DownloadError> {
        Self::with_config(config)
    }

    pub fn with_config(config: DashConfig) -> Result<Self, DownloadError> {
        config.validate()?;
        let clients = Arc::new(create_client_pool(&config.base)?);
        Ok(Self { clients, config })
    }

    pub fn config(&self) -> &DashConfig {
        &self.config
    }

    pub async fn start_session(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<HlsData>, DownloadError> {
        let token = request.cancel.unwrap_or_default();
        let engine_token = token.child_token();
        let event_capacity = (self.config.scheduler.download_concurrency.max(1) * 128).max(256);
        let (events, event_stream) = EventSink::channel(event_capacity);
        events.emit(DownloadEvent::Started {
            protocol: ProtocolType::Dash,
            url: Arc::from(request.url.as_str()),
        });

        let mut config = self.config.clone();
        if let ProtocolSelection::Dash(options) = &request.protocol
            && let Some(selection) = options.selection.clone()
        {
            config.selection = selection;
        }
        let started = engine::start(
            request.url,
            Arc::new(config),
            Arc::clone(&self.clients),
            request.cache,
            engine_token.clone(),
            Some(events.clone()),
        )
        .await;
        let (items, handles) = match started {
            Ok(started) => started,
            Err(error) => {
                engine_token.cancel();
                return Err(error);
            }
        };

        let metrics = handles.performance_metrics.clone();
        let lifecycle = tokio::spawn(engine::join(handles, engine_token.clone()));
        let stream: BoxMediaStream<HlsData, DownloadError> = Box::pin(CancelOnDropStream {
            inner: ReceiverStream::new(items).boxed(),
            token: engine_token.clone(),
        });
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

impl MediaEngine for DashDownloader {
    type Item = HlsData;

    async fn start(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<Self::Item>, DownloadError> {
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Dash(request.options.dash.clone());
        }
        self.start_session(request).await
    }
}
