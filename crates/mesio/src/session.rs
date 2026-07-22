use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use std::time::Instant;

use futures::{Stream, StreamExt};
use hls::HlsData;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use url::Url;

use crate::cache::CacheManager;
use crate::flv::{FlvDownloader, FlvProtocolConfig};
use crate::hls::config::HlsVariantSelectionPolicy;
use crate::hls::engine::identity::SegmentKey;
use crate::hls::{GapSkipReason, MetricsSnapshot, PerformanceMetrics};
use crate::hls::{HlsConfig, HlsDownloader};
use crate::source::{ContentSource, SourceManager};
use crate::{BoxMediaStream, DownloadError};

pub type DownloadEventStream = Pin<Box<dyn Stream<Item = DownloadEvent> + Send + 'static>>;

/// Protocol type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolType {
    /// FLV protocol.
    Flv,
    /// HLS protocol.
    Hls,
    /// Auto-detect from URL.
    Auto,
}

#[derive(Clone)]
pub struct DownloadRequest {
    pub url: Url,
    pub protocol: ProtocolSelection,
    pub sources: Vec<ContentSource>,
    pub cache: Option<Arc<CacheManager>>,
    pub cancel: Option<CancellationToken>,
    pub options: DownloadOptions,
}

impl DownloadRequest {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            protocol: ProtocolSelection::Auto,
            sources: Vec::new(),
            cache: None,
            cancel: None,
            options: DownloadOptions::default(),
        }
    }

    pub fn from_url(url: &str) -> Result<Self, DownloadError> {
        let url = Url::parse(url).map_err(|e| DownloadError::invalid_url(url, e.to_string()))?;
        Ok(Self::new(url))
    }

    pub fn with_protocol(mut self, protocol: ProtocolSelection) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn with_cache(mut self, cache: Arc<CacheManager>) -> Self {
        self.cache = Some(cache);
        self
    }

    pub fn add_source(mut self, source: ContentSource) -> Self {
        self.sources.push(source);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct DownloadOptions {
    pub hls: HlsRequestOptions,
    pub flv: FlvRequestOptions,
}

#[derive(Debug, Clone, Default)]
pub enum ProtocolSelection {
    #[default]
    Auto,
    Hls(HlsRequestOptions),
    Flv(FlvRequestOptions),
}

#[derive(Debug, Clone, Default)]
pub struct HlsRequestOptions {
    pub variant_selection_policy: Option<HlsVariantSelectionPolicy>,
}

#[derive(Debug, Clone)]
pub struct FlvRequestOptions {
    pub reconnect: FlvReconnect,
}

impl Default for FlvRequestOptions {
    fn default() -> Self {
        Self {
            reconnect: FlvReconnect::FailTerminal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvReconnect {
    FailTerminal,
    ReconnectSameSourceWithDiscontinuity,
    SwitchSourceWithDiscontinuity,
}

pub struct DownloadSession<T> {
    pub items: BoxMediaStream<T, DownloadError>,
    pub events: DownloadEventStream,
    pub handle: DownloadHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadTerminal {
    AuthoritativeEnd,
    Cancelled,
    DownstreamClosed,
    PipelineError(Arc<str>),
}

pub enum DownloaderSession {
    Flv(DownloadSession<flv::data::FlvData>),
    Hls(DownloadSession<hls::HlsData>),
}

impl DownloaderSession {
    pub fn into_hls(self) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        match self {
            Self::Hls(session) => Ok(session),
            Self::Flv(_) => Err(DownloadError::UnsupportedProtocol {
                protocol: "expected HLS session, got FLV".to_string(),
            }),
        }
    }

    pub fn into_flv(self) -> Result<DownloadSession<flv::data::FlvData>, DownloadError> {
        match self {
            Self::Flv(session) => Ok(session),
            Self::Hls(_) => Err(DownloadError::UnsupportedProtocol {
                protocol: "expected FLV session, got HLS".to_string(),
            }),
        }
    }
}

#[derive(Clone)]
pub struct DownloadHandle {
    cancel: CancellationToken,
    metrics: Arc<Mutex<Option<Arc<PerformanceMetrics>>>>,
    dropped_events: Arc<AtomicU64>,
    lifecycle: Arc<Mutex<Option<JoinHandle<DownloadTerminal>>>>,
}

impl DownloadHandle {
    pub(crate) fn new(
        cancel: CancellationToken,
        metrics: Option<Arc<PerformanceMetrics>>,
        dropped_events: Arc<AtomicU64>,
        lifecycle: Option<JoinHandle<DownloadTerminal>>,
    ) -> Self {
        Self {
            cancel,
            metrics: Arc::new(Mutex::new(metrics)),
            dropped_events,
            lifecycle: Arc::new(Mutex::new(lifecycle)),
        }
    }

    pub(crate) fn new_with_metrics_slot(
        cancel: CancellationToken,
        metrics: Arc<Mutex<Option<Arc<PerformanceMetrics>>>>,
        dropped_events: Arc<AtomicU64>,
        lifecycle: Option<JoinHandle<DownloadTerminal>>,
    ) -> Self {
        Self {
            cancel,
            metrics,
            dropped_events,
            lifecycle: Arc::new(Mutex::new(lifecycle)),
        }
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn metrics(&self) -> Option<MetricsSnapshot> {
        self.metrics
            .lock()
            .as_ref()
            .map(|metrics| metrics.snapshot())
    }

    pub(crate) fn metrics_source(&self) -> Option<Arc<PerformanceMetrics>> {
        self.metrics.lock().clone()
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped_events.load(Ordering::Relaxed)
    }

    pub async fn join(&self) -> Option<Result<DownloadTerminal, DownloadError>> {
        let lifecycle = self.lifecycle.lock().take()?;
        Some(lifecycle.await.map_err(|e| DownloadError::Internal {
            reason: format!("download lifecycle task failed: {e}"),
        }))
    }
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Started {
        protocol: ProtocolType,
        url: Arc<str>,
    },
    SourceSelected {
        url: Arc<str>,
        priority: u8,
        attempt: u32,
    },
    ResourceStarted {
        resource: ResourceId,
        display_url: Arc<str>,
        content_length: Option<u64>,
    },
    Progress {
        resource: ResourceId,
        bytes_delta: u64,
        bytes_total: u64,
    },
    ResourceFinished {
        resource: ResourceId,
        bytes: u64,
        from_cache: bool,
    },
    RetryScheduled {
        resource: Option<ResourceId>,
        attempt: u32,
        delay: Duration,
        reason: Arc<str>,
    },
    GapSkipped {
        from_sequence: u64,
        to_sequence: u64,
        reason: GapSkipReason,
    },
    SegmentTimeout {
        sequence_number: u64,
        waited: Duration,
    },
    PlaylistRefreshed {
        media_sequence_base: u64,
        target_duration: f64,
    },
    Lagged {
        dropped: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceId {
    HlsPlaylist { url: Arc<str> },
    HlsSegment { key: SegmentKey },
    HlsKey { uri: Arc<str> },
    FlvStream { url: Arc<str> },
}

#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<DownloadEvent>,
    dropped: Arc<AtomicU64>,
    unreported_dropped: Arc<AtomicU64>,
}

impl EventSink {
    pub fn channel(capacity: usize) -> (Self, DownloadEventStream) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let unreported_dropped = Arc::new(AtomicU64::new(0));
        (
            Self {
                tx,
                dropped,
                unreported_dropped,
            },
            Box::pin(ReceiverStream::new(rx)),
        )
    }

    pub fn emit(&self, event: DownloadEvent) {
        if !matches!(event, DownloadEvent::Lagged { .. }) {
            let dropped = self.unreported_dropped.swap(0, Ordering::Relaxed);
            if dropped > 0 && self.tx.try_send(DownloadEvent::Lagged { dropped }).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                self.unreported_dropped
                    .fetch_add(dropped.saturating_add(1), Ordering::Relaxed);
            }
        }

        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            self.unreported_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub(crate) fn dropped_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.dropped)
    }
}

pub trait MediaEngine: Send + Sync + 'static {
    type Item: Send + 'static;

    fn start(
        &self,
        request: DownloadRequest,
    ) -> impl Future<Output = Result<DownloadSession<Self::Item>, DownloadError>> + Send;
}

#[derive(Debug, Clone)]
pub struct MesioConfig {
    pub flv: FlvProtocolConfig,
    pub hls: HlsConfig,
    pub token: CancellationToken,
}

impl Default for MesioConfig {
    fn default() -> Self {
        Self {
            flv: FlvProtocolConfig::default(),
            hls: HlsConfig::default(),
            token: CancellationToken::new(),
        }
    }
}

pub struct MesioDownloader {
    config: MesioConfig,
    cache: Option<Arc<CacheManager>>,
}

impl MesioDownloader {
    pub fn new(config: MesioConfig) -> Self {
        Self {
            config,
            cache: None,
        }
    }

    pub fn with_cache(mut self, cache: Arc<CacheManager>) -> Self {
        self.cache = Some(cache);
        self
    }

    pub fn detect_protocol(url: &str) -> Result<ProtocolType, DownloadError> {
        let url = Url::parse(url).map_err(|e| {
            DownloadError::invalid_url(
                format!("Failed to parse URL for protocol detection: {url}"),
                e.to_string(),
            )
        })?;

        let path = url.path().to_lowercase();
        if path.ends_with(".m3u8") || path.ends_with(".m3u") || path.contains("playlist") {
            return Ok(ProtocolType::Hls);
        }

        if path.ends_with(".flv") {
            return Ok(ProtocolType::Flv);
        }

        if let Some(query) = url.query() {
            let query = query.to_lowercase();
            if query.contains("playlist") || query.contains("manifest") || query.contains("hls") {
                return Ok(ProtocolType::Hls);
            }
        }

        if matches!(url.scheme(), "rtmp" | "rtmps" | "rtsp") {
            return Ok(ProtocolType::Flv);
        }

        Err(DownloadError::ProtocolDetectionFailed {
            url: url.to_string(),
        })
    }

    pub async fn start(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloaderSession, DownloadError> {
        self.apply_defaults(&mut request);

        let protocol = match &request.protocol {
            ProtocolSelection::Auto => Self::detect_protocol(request.url.as_str())?,
            ProtocolSelection::Hls(_) => ProtocolType::Hls,
            ProtocolSelection::Flv(_) => ProtocolType::Flv,
        };

        match protocol {
            ProtocolType::Hls => {
                let mut request = request;
                if matches!(request.protocol, ProtocolSelection::Auto) {
                    request.protocol = ProtocolSelection::Hls(request.options.hls.clone());
                }
                Ok(DownloaderSession::Hls(self.start_hls(request).await?))
            }
            ProtocolType::Flv => {
                let mut request = request;
                if matches!(request.protocol, ProtocolSelection::Auto) {
                    request.protocol = ProtocolSelection::Flv(request.options.flv.clone());
                }
                Ok(DownloaderSession::Flv(self.start_flv(request).await?))
            }
            ProtocolType::Auto => unreachable!(),
        }
    }

    pub async fn start_hls(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        self.apply_defaults(&mut request);
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Hls(request.options.hls.clone());
        }
        if request.sources.is_empty() {
            return self.start_single_hls(request).await;
        }
        self.start_hls_with_sources(request).await
    }

    async fn start_single_hls(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        let downloader = HlsDownloader::with_config(self.config.hls.clone())?;
        downloader.start(request).await
    }

    pub async fn start_flv(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<flv::data::FlvData>, DownloadError> {
        self.apply_defaults(&mut request);
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Flv(request.options.flv.clone());
        }
        let reconnect = match &request.protocol {
            ProtocolSelection::Flv(options) => options.reconnect,
            _ => request.options.flv.reconnect,
        };
        if reconnect != FlvReconnect::FailTerminal {
            return Err(DownloadError::Configuration {
                reason: format!("FLV reconnect mode {reconnect:?} is declared but not implemented"),
            });
        }
        let selected_source = Self::select_initial_source(&mut request)?;
        if let Some(source) = &selected_source {
            request.url = source.url.clone();
        }
        let downloader = FlvDownloader::with_config(self.config.flv.clone())?;
        let session = downloader.start(request).await?;
        if let Some(source) = selected_source {
            return wrap_flv_source_session(session, source);
        }
        Ok(session)
    }

    fn apply_defaults(&self, request: &mut DownloadRequest) {
        if request.cancel.is_none() {
            request.cancel = Some(self.config.token.clone());
        }
        if request.cache.is_none() {
            request.cache = self.cache.clone();
        }
    }

    fn select_initial_source(
        request: &mut DownloadRequest,
    ) -> Result<Option<SelectedSource>, DownloadError> {
        if request.sources.is_empty() {
            return Ok(None);
        }

        let mut manager = SourceManager::new();
        for source in request.sources.iter().cloned() {
            manager.add_source(source);
        }

        select_source_attempt(&mut manager, &HashSet::new()).and_then(|source| {
            source
                .ok_or_else(|| {
                    DownloadError::source_exhausted("no healthy download sources are available")
                })
                .map(Some)
        })
    }

    async fn start_hls_with_sources(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        let parent_token = request
            .cancel
            .clone()
            .unwrap_or_else(|| self.config.token.clone());
        let session_token = parent_token.child_token();
        let event_capacity =
            (self.config.hls.scheduler_config.download_concurrency.max(1) * 64).max(256);
        let (events, event_stream) = EventSink::channel(event_capacity);
        let dropped_counter = events.dropped_counter();
        let (item_tx, item_rx) = mpsc::channel(32);
        let config = self.config.clone();
        let request_cache = request.cache.clone();
        let active_metrics = Arc::new(Mutex::new(None));
        let lifecycle_token = session_token.clone();
        let request_for_task = request;
        let events_for_task = events.clone();
        let metrics_for_task = Arc::clone(&active_metrics);

        let lifecycle = tokio::spawn(async move {
            run_hls_source_failover(
                config,
                request_for_task,
                request_cache,
                lifecycle_token,
                events_for_task,
                item_tx,
                metrics_for_task,
            )
            .await
        });

        let stream: BoxMediaStream<HlsData, DownloadError> = Box::pin(SessionCancelOnDropStream {
            inner: Box::pin(ReceiverStream::new(item_rx)),
            token: session_token.clone(),
        });

        let handle = DownloadHandle::new_with_metrics_slot(
            session_token,
            active_metrics,
            dropped_counter,
            Some(lifecycle),
        );

        Ok(DownloadSession {
            items: stream,
            events: event_stream,
            handle,
        })
    }
}

struct SelectedSource {
    url: Url,
    original_url: String,
    priority: u8,
}

async fn run_hls_source_failover(
    config: MesioConfig,
    request: DownloadRequest,
    request_cache: Option<Arc<CacheManager>>,
    token: CancellationToken,
    events: EventSink,
    item_tx: mpsc::Sender<Result<HlsData, DownloadError>>,
    active_metrics: Arc<Mutex<Option<Arc<PerformanceMetrics>>>>,
) -> DownloadTerminal {
    let mut manager = SourceManager::new();
    for source in request.sources.iter().cloned() {
        manager.add_source(source);
    }

    let mut attempted = HashSet::new();
    let mut last_error: Option<DownloadError> = None;
    let mut delivered_media = false;
    let mut pending_discontinuity = false;
    let mut attempt = 0_u32;

    loop {
        if token.is_cancelled() {
            return DownloadTerminal::Cancelled;
        }
        *active_metrics.lock() = None;

        let selected = match select_source_attempt(&mut manager, &attempted) {
            Ok(Some(source)) => source,
            Ok(None) => {
                let reason = last_error
                    .map(|err| err.to_string())
                    .unwrap_or_else(|| "all HLS sources failed".to_string());
                let error = DownloadError::source_exhausted(reason.clone());
                if item_tx.send(Err(error)).await.is_err() {
                    debug!("HLS error receiver closed after source exhaustion");
                }
                return DownloadTerminal::PipelineError(Arc::from(reason));
            }
            Err(err) => {
                let reason = err.to_string();
                if item_tx.send(Err(err)).await.is_err() {
                    debug!("HLS error receiver closed after source selection failure");
                }
                return DownloadTerminal::PipelineError(Arc::from(reason));
            }
        };
        attempted.insert(selected.original_url.clone());
        attempt = attempt.saturating_add(1);

        if pending_discontinuity {
            if item_tx
                .send(Ok(HlsData::end_marker_with_reason(
                    hls::SplitReason::Discontinuity,
                )))
                .await
                .is_err()
            {
                return DownloadTerminal::DownstreamClosed;
            }
            pending_discontinuity = false;
        }

        events.emit(DownloadEvent::SourceSelected {
            url: Arc::from(selected.original_url.as_str()),
            priority: selected.priority,
            attempt,
        });

        let mut attempt_request = request.clone();
        attempt_request.url = selected.url;
        attempt_request.sources.clear();
        attempt_request.cache = request_cache.clone();
        attempt_request.cancel = Some(token.clone());
        attempt_request.protocol = match &attempt_request.protocol {
            ProtocolSelection::Auto => ProtocolSelection::Hls(Default::default()),
            other => other.clone(),
        };

        let downloader = match HlsDownloader::with_config(config.hls.clone()) {
            Ok(downloader) => downloader,
            Err(err) => {
                let reason = err.to_string();
                if item_tx.send(Err(err)).await.is_err() {
                    debug!("HLS error receiver closed after downloader initialization failure");
                }
                return DownloadTerminal::PipelineError(Arc::from(reason));
            }
        };

        let started_at = Instant::now();
        let session = match downloader.start(attempt_request).await {
            Ok(session) => session,
            Err(err) => {
                manager.record_failure(&selected.original_url, &err, started_at.elapsed());
                last_error = Some(err);
                continue;
            }
        };

        *active_metrics.lock() = session.handle.metrics_source();
        let attempt_events = session.events;
        let forward_events = events.clone();
        let event_task = tokio::spawn(forward_event_stream(attempt_events, forward_events));
        let mut items = session.items;
        let handle = session.handle;
        let mut terminal = DownloadTerminal::Cancelled;
        let mut failed = None;

        while let Some(item) = items.next().await {
            match item {
                Ok(item) => {
                    if !item.is_end_marker() {
                        delivered_media = true;
                    }
                    if item_tx.send(Ok(item)).await.is_err() {
                        handle.cancel();
                        if let Err(error) = event_task.await {
                            warn!(%error, "HLS event forwarding task failed during cancellation");
                        }
                        return DownloadTerminal::DownstreamClosed;
                    }
                }
                Err(err) => {
                    failed = Some(err);
                    break;
                }
            }
        }

        drop(items);
        handle.cancel();
        if let Some(joined) = handle.join().await {
            match joined {
                Ok(joined_terminal) => terminal = joined_terminal,
                Err(err) => {
                    failed = Some(err);
                }
            }
        }
        if let Err(error) = event_task.await {
            warn!(%error, "HLS event forwarding task failed");
        }

        if let Some(err) = failed {
            manager.record_failure(&selected.original_url, &err, started_at.elapsed());
            last_error = Some(err);
            pending_discontinuity = delivered_media;
            continue;
        }

        match terminal {
            DownloadTerminal::AuthoritativeEnd | DownloadTerminal::Cancelled => return terminal,
            DownloadTerminal::DownstreamClosed => return DownloadTerminal::DownstreamClosed,
            DownloadTerminal::PipelineError(reason) => {
                let err = DownloadError::Protocol {
                    reason: reason.to_string(),
                };
                manager.record_failure(&selected.original_url, &err, started_at.elapsed());
                last_error = Some(err);
                pending_discontinuity = delivered_media;
            }
        }
    }
}

fn wrap_flv_source_session(
    session: DownloadSession<flv::data::FlvData>,
    source: SelectedSource,
) -> Result<DownloadSession<flv::data::FlvData>, DownloadError> {
    let (events, event_stream) = EventSink::channel(256);
    events.emit(DownloadEvent::SourceSelected {
        url: Arc::from(source.original_url.as_str()),
        priority: source.priority,
        attempt: 1,
    });
    let dropped_counter = events.dropped_counter();
    let forward_events = events.clone();
    tokio::spawn(forward_event_stream(session.events, forward_events));

    Ok(DownloadSession {
        items: session.items,
        events: event_stream,
        handle: DownloadHandle::new_with_metrics_slot(
            session.handle.cancel.clone(),
            session.handle.metrics.clone(),
            dropped_counter,
            None,
        ),
    })
}

async fn forward_event_stream(mut stream: DownloadEventStream, sink: EventSink) {
    while let Some(event) = stream.next().await {
        sink.emit(event);
    }
}

fn select_source_attempt(
    manager: &mut SourceManager,
    attempted: &HashSet<String>,
) -> Result<Option<SelectedSource>, DownloadError> {
    let Some(source) = manager.select_source_excluding(attempted) else {
        return Ok(None);
    };
    let url = Url::parse(&source.url)
        .map_err(|e| DownloadError::invalid_url(source.url.clone(), e.to_string()))?;
    Ok(Some(SelectedSource {
        url,
        original_url: source.url,
        priority: source.priority,
    }))
}

struct SessionCancelOnDropStream<T> {
    inner: BoxMediaStream<T, DownloadError>,
    token: CancellationToken,
}

impl<T> Stream for SessionCancelOnDropStream<T> {
    type Item = Result<T, DownloadError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<T> Drop for SessionCancelOnDropStream<T> {
    fn drop(&mut self) {
        self.token.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_sink_counts_dropped_events() {
        let (sink, _events) = EventSink::channel(1);
        sink.emit(DownloadEvent::Lagged { dropped: 0 });
        sink.emit(DownloadEvent::Lagged { dropped: 1 });
        assert_eq!(sink.dropped(), 1);
    }

    #[tokio::test]
    async fn event_sink_emits_coalesced_lagged_event_after_drop() {
        let (sink, mut events) = EventSink::channel(2);
        sink.emit(DownloadEvent::Started {
            protocol: ProtocolType::Hls,
            url: Arc::from("https://example.test/live.m3u8"),
        });
        sink.emit(DownloadEvent::Started {
            protocol: ProtocolType::Hls,
            url: Arc::from("https://example.test/live.m3u8"),
        });
        sink.emit(DownloadEvent::Progress {
            resource: ResourceId::HlsPlaylist {
                url: Arc::from("https://example.test/live.m3u8"),
            },
            bytes_delta: 1,
            bytes_total: 1,
        });

        assert!(matches!(
            events.next().await,
            Some(DownloadEvent::Started { .. })
        ));
        sink.emit(DownloadEvent::ResourceFinished {
            resource: ResourceId::HlsPlaylist {
                url: Arc::from("https://example.test/live.m3u8"),
            },
            bytes: 1,
            from_cache: false,
        });

        assert!(matches!(
            events.next().await,
            Some(DownloadEvent::Started { .. })
        ));
        assert!(matches!(
            events.next().await,
            Some(DownloadEvent::Lagged { dropped: 1 })
        ));
    }

    #[test]
    fn handle_cancel_cancels_token() {
        let token = CancellationToken::new();
        let handle = DownloadHandle::new(token.clone(), None, Arc::new(AtomicU64::new(0)), None);
        handle.cancel();
        assert!(token.is_cancelled());
    }
}
