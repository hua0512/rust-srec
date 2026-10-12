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
use crate::dash::{DashConfig, DashDownloader, DashSelectionConfig, MediaTrack};
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
    /// MPEG-DASH protocol.
    Dash,
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
    pub dash: DashRequestOptions,
}

#[derive(Debug, Clone, Default)]
pub enum ProtocolSelection {
    #[default]
    Auto,
    Hls(HlsRequestOptions),
    Flv(FlvRequestOptions),
    Dash(DashRequestOptions),
}

#[derive(Debug, Clone, Default)]
pub struct HlsRequestOptions {
    pub variant_selection_policy: Option<HlsVariantSelectionPolicy>,
}

#[derive(Debug, Clone, Default)]
pub struct DashRequestOptions {
    /// Replaces the configured Representation selection for this request.
    pub selection: Option<DashSelectionConfig>,
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
    /// DASH output uses the HLS fMP4 item type; the variant keeps the
    /// protocol visible to consumers.
    Dash(DownloadSession<hls::HlsData>),
}

impl DownloaderSession {
    pub fn protocol(&self) -> ProtocolType {
        match self {
            Self::Flv(_) => ProtocolType::Flv,
            Self::Hls(_) => ProtocolType::Hls,
            Self::Dash(_) => ProtocolType::Dash,
        }
    }

    pub fn into_hls(self) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        match self {
            Self::Hls(session) => Ok(session),
            other => Err(other.mismatch("HLS")),
        }
    }

    pub fn into_dash(self) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        match self {
            Self::Dash(session) => Ok(session),
            other => Err(other.mismatch("DASH")),
        }
    }

    pub fn into_flv(self) -> Result<DownloadSession<flv::data::FlvData>, DownloadError> {
        match self {
            Self::Flv(session) => Ok(session),
            other => Err(other.mismatch("FLV")),
        }
    }

    fn mismatch(&self, expected: &str) -> DownloadError {
        let actual = match self {
            Self::Flv(_) => "FLV",
            Self::Hls(_) => "HLS",
            Self::Dash(_) => "DASH",
        };
        DownloadError::UnsupportedProtocol {
            protocol: format!("expected {expected} session, got {actual}"),
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
    /// A track of a separately delivered stream (DASH audio) fell behind;
    /// output stopped waiting for it, and its late data is dropped.
    TrackStalled {
        track: MediaTrack,
        waited: Duration,
    },
    /// A track stopped contributing to the output for the rest of the
    /// session; recording continues with the remaining track.
    TrackDropped {
        track: MediaTrack,
        reason: Arc<str>,
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
    DashManifest { url: Arc<str> },
    DashSegment { track: MediaTrack, key: SegmentKey },
}

#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<DownloadEvent>,
    dropped: Arc<AtomicU64>,
    unreported_dropped: Arc<AtomicU64>,
}

impl EventSink {
    pub fn channel(capacity: usize) -> (Self, DownloadEventStream) {
        Self::channel_counting_into(capacity, Arc::new(AtomicU64::new(0)))
    }

    /// A channel whose drops add to an existing counter, so a sink that
    /// re-emits another session's events reports one combined total.
    pub(crate) fn channel_counting_into(
        capacity: usize,
        dropped: Arc<AtomicU64>,
    ) -> (Self, DownloadEventStream) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
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
    pub dash: DashConfig,
    pub token: CancellationToken,
}

impl Default for MesioConfig {
    fn default() -> Self {
        Self {
            flv: FlvProtocolConfig::default(),
            hls: HlsConfig::default(),
            dash: DashConfig::default(),
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
            DownloadError::invalid_url(url, format!("protocol detection failed: {e}"))
        })?;

        let path = url.path().to_lowercase();
        // Checked first: an MPD URL can carry `manifest` or `playlist` in its
        // query or path, which the HLS heuristics below would claim.
        if path.ends_with(".mpd") {
            return Ok(ProtocolType::Dash);
        }

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
            url: crate::redact::redact_url(&url).to_string(),
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
            ProtocolSelection::Dash(_) => ProtocolType::Dash,
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
            ProtocolType::Dash => Ok(DownloaderSession::Dash(self.start_dash(request).await?)),
            ProtocolType::Auto => Err(DownloadError::ProtocolDetectionFailed {
                url: crate::redact::redact_url(&request.url).to_string(),
            }),
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
            let downloader = HlsDownloader::with_config(self.config.hls.clone())?;
            return downloader.start(request).await;
        }
        let config = self.config.hls.clone();
        let event_capacity = (config.scheduler_config.download_concurrency.max(1) * 64).max(256);
        self.start_with_sources(request, SegmentedProtocol::HLS, event_capacity, move || {
            HlsDownloader::with_config(config.clone())
        })
    }

    /// Starts a DASH session, failing over between `request.sources` when
    /// any are given.
    pub async fn start_dash(
        &self,
        mut request: DownloadRequest,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError> {
        self.apply_defaults(&mut request);
        if matches!(request.protocol, ProtocolSelection::Auto) {
            request.protocol = ProtocolSelection::Dash(request.options.dash.clone());
        }
        if request.sources.is_empty() {
            let downloader = DashDownloader::with_config(self.config.dash.clone())?;
            return downloader.start(request).await;
        }
        let config = self.config.dash.clone();
        config.validate()?;
        let event_capacity = (config.scheduler.download_concurrency.max(1) * 128).max(256);
        self.start_with_sources(
            request,
            SegmentedProtocol::DASH,
            event_capacity,
            move || DashDownloader::with_config(config.clone()),
        )
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

    /// A session that tries `request.sources` in turn, starting a fresh
    /// engine from `make_engine` per attempt.
    fn start_with_sources<E, F>(
        &self,
        request: DownloadRequest,
        protocol: SegmentedProtocol,
        event_capacity: usize,
        make_engine: F,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError>
    where
        E: MediaEngine<Item = HlsData>,
        F: Fn() -> Result<E, DownloadError> + Send + 'static,
    {
        let parent_token = request
            .cancel
            .clone()
            .unwrap_or_else(|| self.config.token.clone());
        let session_token = parent_token.child_token();
        let (events, event_stream) = EventSink::channel(event_capacity);
        let dropped_counter = events.dropped_counter();
        let (item_tx, item_rx) = mpsc::channel(32);
        let request_cache = request.cache.clone();
        let active_metrics = Arc::new(Mutex::new(None));
        let lifecycle_token = session_token.clone();
        let events_for_task = events.clone();
        let metrics_for_task = Arc::clone(&active_metrics);

        let lifecycle = tokio::spawn(async move {
            run_source_failover(
                protocol,
                make_engine,
                request,
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

/// A segmented protocol whose sessions fail over between sources.
#[derive(Clone, Copy)]
struct SegmentedProtocol {
    name: &'static str,
    /// The selection an attempt uses when the request left it on `Auto`.
    default_selection: fn() -> ProtocolSelection,
}

impl SegmentedProtocol {
    const HLS: Self = Self {
        name: "HLS",
        default_selection: || ProtocolSelection::Hls(HlsRequestOptions::default()),
    };
    const DASH: Self = Self {
        name: "DASH",
        default_selection: || ProtocolSelection::Dash(DashRequestOptions::default()),
    };
}

/// Runs attempts against the request's sources until one ends the stream,
/// the consumer goes away, or every source has failed. Items of successive
/// attempts are separated by a discontinuity marker once media was delivered.
#[allow(clippy::too_many_arguments)]
async fn run_source_failover<E, F>(
    protocol: SegmentedProtocol,
    make_engine: F,
    request: DownloadRequest,
    request_cache: Option<Arc<CacheManager>>,
    token: CancellationToken,
    events: EventSink,
    item_tx: mpsc::Sender<Result<HlsData, DownloadError>>,
    active_metrics: Arc<Mutex<Option<Arc<PerformanceMetrics>>>>,
) -> DownloadTerminal
where
    E: MediaEngine<Item = HlsData>,
    F: Fn() -> Result<E, DownloadError>,
{
    let name = protocol.name;
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
                    .unwrap_or_else(|| format!("all {name} sources failed"));
                let error = DownloadError::source_exhausted(reason.clone());
                if deliver(&item_tx, Err(error), &token).await == Delivery::Cancelled {
                    return DownloadTerminal::Cancelled;
                }
                return DownloadTerminal::PipelineError(Arc::from(reason));
            }
            Err(err) => {
                let reason = err.to_string();
                if deliver(&item_tx, Err(err), &token).await == Delivery::Cancelled {
                    return DownloadTerminal::Cancelled;
                }
                return DownloadTerminal::PipelineError(Arc::from(reason));
            }
        };
        attempted.insert(selected.original_url.clone());
        attempt = attempt.saturating_add(1);

        if pending_discontinuity {
            let marker = Ok(HlsData::end_marker_with_reason(
                hls::SplitReason::Discontinuity,
            ));
            match deliver(&item_tx, marker, &token).await {
                Delivery::Sent => {}
                Delivery::Closed => return DownloadTerminal::DownstreamClosed,
                Delivery::Cancelled => return DownloadTerminal::Cancelled,
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
            ProtocolSelection::Auto => (protocol.default_selection)(),
            other => other.clone(),
        };

        let downloader = match make_engine() {
            Ok(downloader) => downloader,
            Err(err) => {
                let reason = err.to_string();
                if deliver(&item_tx, Err(err), &token).await == Delivery::Cancelled {
                    return DownloadTerminal::Cancelled;
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
        let mut attempt_delivered = false;

        // Items that announce an fMP4 epoch (a structure-change marker and
        // an init) are held and delivered together with the media item that
        // follows, so a cancellation cannot leave a consumer with an
        // announced epoch and no media.
        let mut group: Vec<HlsData> = Vec::new();
        while let Some(item) = items.next().await {
            match item {
                Ok(item) => {
                    if !item.is_end_marker() {
                        if !attempt_delivered {
                            attempt_delivered = true;
                            manager.record_success(&selected.original_url, started_at.elapsed());
                            attempted.clear();
                            attempted.insert(selected.original_url.clone());
                        }
                        delivered_media = true;
                    }
                    let announces = matches!(
                        item,
                        HlsData::EndMarker(Some(hls::SplitReason::StreamStructureChange { .. }))
                            | HlsData::M4sData(hls::M4sData::InitSegment(_))
                    );
                    group.push(item);
                    if announces && group.len() < MAX_ANNOUNCEMENT {
                        continue;
                    }
                    let delivery =
                        deliver_group(&item_tx, std::mem::take(&mut group), &token).await;
                    if delivery != Delivery::Sent {
                        // The item stream and the attempt's engine task both
                        // hold event sinks; the forwarder only finishes once
                        // both are gone.
                        drop(items);
                        handle.cancel();
                        if let Some(Err(error)) = handle.join().await {
                            debug!(protocol = name, error = %crate::redact::redact_diagnostic(&error), "attempt failed after downstream closed");
                        }
                        if let Err(error) = event_task.await {
                            warn!(protocol = name, error = %crate::redact::redact_diagnostic(&error), "event forwarding task failed during cancellation");
                        }
                        return match delivery {
                            Delivery::Cancelled => DownloadTerminal::Cancelled,
                            _ => DownloadTerminal::DownstreamClosed,
                        };
                    }
                }
                Err(err) => {
                    failed = Some(err);
                    break;
                }
            }
        }
        if !group.is_empty() {
            debug!(
                protocol = name,
                items = group.len(),
                "attempt ended after announcing media it never delivered"
            );
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
            warn!(protocol = name, error = %crate::redact::redact_diagnostic(&error), "event forwarding task failed");
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

/// Longest run of announcing items held for the media that follows them: an
/// end marker and an init, with room to spare. A longer run is delivered as
/// it is rather than buffered without bound.
const MAX_ANNOUNCEMENT: usize = 4;

/// Delivers `group` whole, or nothing of it when `token` is cancelled first:
/// permits for every item are reserved before any is sent.
async fn deliver_group(
    tx: &mpsc::Sender<Result<HlsData, DownloadError>>,
    group: Vec<HlsData>,
    token: &CancellationToken,
) -> Delivery {
    if group.len() > tx.max_capacity() {
        for item in group {
            let delivery = deliver(tx, Ok(item), token).await;
            if delivery != Delivery::Sent {
                return delivery;
            }
        }
        return Delivery::Sent;
    }
    if tx.is_closed() {
        return Delivery::Closed;
    }
    tokio::select! {
        biased;
        permits = tx.reserve_many(group.len()) => match permits {
            Ok(permits) => {
                for (permit, item) in permits.zip(group) {
                    permit.send(Ok(item));
                }
                Delivery::Sent
            }
            Err(_) => Delivery::Closed,
        },
        _ = token.cancelled() => Delivery::Cancelled,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Delivery {
    Sent,
    Closed,
    Cancelled,
}

/// Sends `item` downstream unless `token` is cancelled first. A consumer
/// that stops reading without dropping the stream would otherwise keep a
/// cancelled session blocked here forever. A dropped stream (which also
/// cancels the session) reports `Closed`.
async fn deliver<T>(tx: &mpsc::Sender<T>, item: T, token: &CancellationToken) -> Delivery {
    if tx.is_closed() {
        return Delivery::Closed;
    }
    tokio::select! {
        biased;
        sent = tx.send(item) => match sent {
            Ok(()) => Delivery::Sent,
            Err(_) => Delivery::Closed,
        },
        _ = token.cancelled() => Delivery::Cancelled,
    }
}

fn wrap_flv_source_session(
    session: DownloadSession<flv::data::FlvData>,
    source: SelectedSource,
) -> Result<DownloadSession<flv::data::FlvData>, DownloadError> {
    // Drops in the inner session and in this re-emitting sink both count
    // toward the handle's total.
    let dropped_counter = Arc::clone(&session.handle.dropped_events);
    let (events, event_stream) =
        EventSink::channel_counting_into(256, Arc::clone(&dropped_counter));
    events.emit(DownloadEvent::SourceSelected {
        url: Arc::from(source.original_url.as_str()),
        priority: source.priority,
        attempt: 1,
    });
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

    /// An engine whose single attempt yields `items`, then stays open until
    /// cancelled.
    struct ScriptedEngine {
        items: Vec<HlsData>,
    }

    impl MediaEngine for ScriptedEngine {
        type Item = HlsData;

        async fn start(
            &self,
            request: DownloadRequest,
        ) -> Result<DownloadSession<HlsData>, DownloadError> {
            // Ends on cancellation, as a real engine's stream does.
            let cancel = request.cancel.unwrap_or_default();
            let stopped = cancel.clone().cancelled_owned();
            let items = futures::stream::iter(self.items.clone().into_iter().map(Ok))
                .chain(futures::stream::once(stopped).filter_map(|()| async { None }));
            Ok(DownloadSession {
                items: Box::pin(items),
                events: Box::pin(futures::stream::empty()),
                handle: DownloadHandle::new(cancel, None, Arc::new(AtomicU64::new(0)), None),
            })
        }
    }

    fn scripted_epoch_change() -> Vec<HlsData> {
        let segment = m3u8_rs::MediaSegment::default;
        let data = |tag: &'static [u8]| bytes::Bytes::from_static(tag);
        vec![
            HlsData::mp4_init(segment(), data(b"init0")),
            HlsData::mp4_segment(segment(), data(b"m0")),
            HlsData::mp4_segment(segment(), data(b"m0b")),
            HlsData::end_marker_with_reason(hls::SplitReason::StreamStructureChange {
                description: "next epoch".to_owned(),
            }),
            HlsData::mp4_init(segment(), data(b"init1")),
            HlsData::mp4_segment(segment(), data(b"m1")),
        ]
    }

    /// Runs the failover forwarder over the scripted attempt into a
    /// three-item consumer channel.
    fn forward_scripted() -> (
        mpsc::Receiver<Result<HlsData, DownloadError>>,
        CancellationToken,
        JoinHandle<DownloadTerminal>,
    ) {
        let url = "https://example.test/live.mpd";
        let request = DownloadRequest::from_url(url)
            .unwrap()
            .add_source(ContentSource::new(url, 0));
        let (item_tx, item_rx) = mpsc::channel(3);
        let (events, _events) = EventSink::channel(16);
        let token = CancellationToken::new();
        let task = tokio::spawn(run_source_failover(
            SegmentedProtocol::DASH,
            || {
                Ok(ScriptedEngine {
                    items: scripted_epoch_change(),
                })
            },
            request,
            None,
            token.clone(),
            events,
            item_tx,
            Arc::new(Mutex::new(None)),
        ));
        (item_rx, token, task)
    }

    fn payload(item: Option<Result<HlsData, DownloadError>>) -> Vec<u8> {
        match item.expect("an item").expect("no error") {
            HlsData::EndMarker(_) => b"marker".to_vec(),
            item => item.data().expect("payload").to_vec(),
        }
    }

    #[tokio::test]
    async fn failover_delivers_an_epoch_announcement_with_its_first_media() {
        let (mut items, token, task) = forward_scripted();
        for expected in [&b"init0"[..], b"m0", b"m0b", b"marker"] {
            let item = tokio::time::timeout(Duration::from_secs(5), items.recv()).await;
            assert_eq!(payload(item.expect("forwarded in time")), expected);
        }
        // The rest of the group is already there, cancellation or not.
        token.cancel();
        assert_eq!(payload(items.try_recv().ok()), b"init1");
        assert_eq!(payload(items.try_recv().ok()), b"m1");
        let terminal = tokio::time::timeout(Duration::from_secs(5), task).await;
        assert_eq!(
            terminal.expect("forwarder ends").unwrap(),
            DownloadTerminal::Cancelled
        );
    }

    #[tokio::test]
    async fn failover_cancelled_during_an_epoch_change_announces_nothing() {
        let (mut items, token, task) = forward_scripted();
        assert_eq!(payload(items.recv().await), b"init0");
        assert_eq!(payload(items.recv().await), b"m0");
        // The announcement group needs three free slots and has two.
        tokio::time::timeout(Duration::from_secs(5), async {
            while items.len() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the third media item is forwarded");
        token.cancel();
        let terminal = tokio::time::timeout(Duration::from_secs(5), task).await;
        assert_eq!(
            terminal.expect("forwarder ends").unwrap(),
            DownloadTerminal::Cancelled
        );
        assert_eq!(payload(items.recv().await), b"m0b");
        assert!(
            items.recv().await.is_none(),
            "no marker or init of the epoch"
        );
    }

    #[tokio::test]
    async fn wrapped_flv_session_reports_the_inner_sessions_dropped_events() {
        let (inner_sink, inner_events) = EventSink::channel(1);
        inner_sink.emit(DownloadEvent::Lagged { dropped: 0 });
        inner_sink.emit(DownloadEvent::Lagged { dropped: 0 }); // dropped: full
        let inner = DownloadSession {
            items: Box::pin(futures::stream::empty()),
            events: inner_events,
            handle: DownloadHandle::new(
                CancellationToken::new(),
                None,
                inner_sink.dropped_counter(),
                None,
            ),
        };
        let source = SelectedSource {
            url: Url::parse("https://e.com/stream.flv").unwrap(),
            original_url: "https://e.com/stream.flv".to_string(),
            priority: 0,
        };

        let wrapped = wrap_flv_source_session(inner, source).unwrap();

        assert_eq!(wrapped.handle.dropped_events(), 1);
    }

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
    fn mpd_paths_are_detected_as_dash_before_hls_heuristics() {
        let detect = |url: &str| MesioDownloader::detect_protocol(url).unwrap();
        assert_eq!(
            detect("https://e.com/live/Manifest.mpd"),
            ProtocolType::Dash
        );
        // `manifest`/`playlist` in the query or path must not make them HLS.
        assert_eq!(
            detect("https://e.com/live/stream.mpd?type=manifest&token=x"),
            ProtocolType::Dash
        );
        assert_eq!(
            detect("https://e.com/playlist/stream.mpd"),
            ProtocolType::Dash
        );
        assert_eq!(detect("https://e.com/live/index.m3u8"), ProtocolType::Hls);
        assert_eq!(
            detect("https://e.com/live/stream?manifest=1"),
            ProtocolType::Hls
        );
        assert_eq!(detect("https://e.com/live/stream.flv"), ProtocolType::Flv);
        assert!(MesioDownloader::detect_protocol("https://e.com/live/stream.mp4").is_err());
    }

    #[test]
    fn downloader_session_conversions_name_both_protocols() {
        let session = || DownloadSession::<hls::HlsData> {
            items: Box::pin(futures::stream::empty()),
            events: Box::pin(futures::stream::empty()),
            handle: DownloadHandle::new(
                CancellationToken::new(),
                None,
                Arc::new(AtomicU64::new(0)),
                None,
            ),
        };
        assert_eq!(
            DownloaderSession::Dash(session()).protocol(),
            ProtocolType::Dash
        );
        assert!(DownloaderSession::Dash(session()).into_dash().is_ok());
        let error = DownloaderSession::Dash(session())
            .into_hls()
            .err()
            .expect("a DASH session is not an HLS session");
        assert!(error.to_string().contains("expected HLS session, got DASH"));
        assert!(DownloaderSession::Hls(session()).into_dash().is_err());
    }

    #[test]
    fn handle_cancel_cancels_token() {
        let token = CancellationToken::new();
        let handle = DownloadHandle::new(token.clone(), None, Arc::new(AtomicU64::new(0)), None);
        handle.cancel();
        assert!(token.is_cancelled());
    }
}
