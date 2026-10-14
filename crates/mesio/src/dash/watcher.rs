//! The MPD watcher task: the only place the MPD, the Segment Indexes it
//! refers to and clock sources are fetched.
//!
//! Publishes [`MpdSnapshot`] values through a coalescing `watch` channel
//! shared by the per-track planners. Besides re-fetching at the MPD's
//! update period, the watcher republishes the current MPD on a timer:
//! segments of a `@duration` template become available with wall-clock time
//! alone, without any MPD change, and planners only run when a snapshot is
//! published.
//!
//! A refresh (MPD fetch plus clock synchronization) runs alongside the
//! republish timer, so a slow origin or dead time server never stops
//! planning: the last usable snapshot keeps being republished meanwhile.
//!
//! A static MPD (from the start, or after a dynamic MPD turned static) is
//! not fetched again; it keeps being republished so the planners can page
//! through it, and they end the stream once everything is planned. Refresh
//! failures beyond the retry budget, or rate limiting beyond its bound (see
//! `crate::rate_limit`), are published as a failure cause on the snapshot,
//! as with HLS.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use reqwest::header::DATE;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use crate::dash::config::DashConfig;
use crate::dash::manifest::{
    ClockOffset, MAX_MPD_BYTES, Manifest, ManifestError, PresentationType, UtcTiming,
    parse_http_date,
};
use crate::dash::segment_index::{self, HttpRangeReader, OnIndexFailure};
use crate::downloader::ClientPool;
use crate::error::{DownloadError, SharedError};
use crate::redact::Redacted;
use crate::segment_engine::control::TerminalCause;
use crate::segment_engine::descriptor::MediaTrack;
use crate::session::{DownloadEvent, EventSink, ResourceId};

/// How often the offset to a `UTCTiming` source is re-measured. Clocks drift
/// slowly; querying a time server on every MPD refresh would only add load.
const UTC_TIMING_RESYNC: Duration = Duration::from_secs(10 * 60);

/// Bounds of the republish timer. The lower bound keeps a manifest with tiny
/// segments from spinning the planners; the upper one keeps long segments
/// from being discovered late.
const MIN_REPUBLISH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_REPUBLISH_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_REPUBLISH_INTERVAL: Duration = Duration::from_secs(1);

/// Republish interval of a static MPD: planners page through it as output
/// is consumed.
const STATIC_REPUBLISH_INTERVAL: Duration = Duration::from_millis(100);

/// Total time one clock synchronization may spend on `UTCTiming` sources
/// before falling back to the MPD's `Date` header.
const CLOCK_SYNC_BUDGET: Duration = Duration::from_secs(3);

/// Largest `UTCTiming` response body read; a timestamp is a few dozen bytes.
const MAX_TIMING_BODY_BYTES: usize = 1024;

/// Cap on any single wait, so delays taken from the MPD or the
/// configuration cannot overflow a deadline.
const MAX_WAIT: Duration = Duration::from_secs(24 * 60 * 60);

fn deadline(delay: Duration) -> Instant {
    Instant::now() + delay.min(MAX_WAIT)
}

/// One published view of the presentation. Cheap to clone.
#[derive(Debug, Clone)]
pub struct MpdSnapshot {
    /// Publication counter, incremented by refreshes and republishes alike.
    pub generation: u64,
    /// Incremented only when a fetched MPD differs from the previous one.
    pub manifest_generation: u64,
    pub manifest: Arc<Manifest>,
    /// The URL the MPD was served from, after redirects.
    pub document_url: Arc<Url>,
    /// Query parameters of the requested MPD URL, inherited by segment URLs
    /// that do not carry them.
    pub parent_params: Arc<[(String, String)]>,
    /// Local time of the fetch.
    pub fetched_at: DateTime<Utc>,
    /// Server clock minus local clock.
    pub clock_offset: ClockOffset,
    pub terminal: Option<TerminalCause>,
}

/// The result of one successful MPD fetch.
pub(crate) struct FetchedMpd {
    pub manifest: Arc<Manifest>,
    pub document_url: Arc<Url>,
    pub body: Bytes,
    pub fetched_at: DateTime<Utc>,
    pub clock_offset: ClockOffset,
    /// Whether `clock_offset` came from a `UTCTiming` source.
    pub utc_synced: bool,
}

/// Fetches MPDs and measures the server clock offset.
pub(crate) struct MpdFetcher {
    clients: Arc<ClientPool>,
    config: Arc<DashConfig>,
    events: Option<EventSink>,
    cancel: CancellationToken,
    /// The tracks the session records, set once they are chosen. Until
    /// then no Segment Index is read (see [`Self::read_segment_indexes`]).
    tracks: OnceLock<Vec<MediaTrack>>,
}

impl MpdFetcher {
    pub(crate) fn new(
        clients: Arc<ClientPool>,
        config: Arc<DashConfig>,
        events: Option<EventSink>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            clients,
            config,
            events,
            cancel,
            tracks: OnceLock::new(),
        }
    }

    /// How long refreshes ride out rate limiting before the stream fails.
    pub(crate) fn max_throttled(&self) -> Duration {
        self.clients.rate_limiter().policy().max_throttled
    }

    /// What is left of the allowance for rate limiting that began at
    /// `since`.
    pub(crate) fn allowance_left(&self, since: Instant) -> Option<Duration> {
        self.clients.rate_limiter().policy().allowance_left(since)
    }

    /// Sets the tracks the session records; later fetches read the Segment
    /// Indexes those tracks select.
    pub(crate) fn record_tracks(&self, tracks: Vec<MediaTrack>) {
        if self.tracks.set(tracks).is_err() {
            warn!("DASH tracks were already set; keeping the first selection");
        }
    }

    /// Reads and applies the Segment Index of each `SegmentBase`
    /// Representation the recorded tracks select, which planners need to
    /// address its segments. Does nothing before [`Self::record_tracks`].
    /// `url` is the MPD URL, whose query parameters segment URLs inherit.
    pub(crate) async fn read_segment_indexes(
        &self,
        manifest: &mut Manifest,
        url: &Url,
        on_failure: OnIndexFailure,
        throttle_deadline: Instant,
    ) -> Result<(), DownloadError> {
        let Some(tracks) = self.tracks.get() else {
            return Ok(());
        };
        let parent_params = crate::segment_engine::descriptor::query_params(url.query());
        let reader = HttpRangeReader {
            clients: &self.clients,
            config: &self.config,
            parent_params: &parent_params,
            cancel: &self.cancel,
            throttle_deadline,
        };
        segment_index::read_selected(
            manifest,
            &self.config.selection,
            tracks,
            &reader,
            on_failure,
        )
        .await
    }

    /// Fetch and resolve the MPD at `url`. `previous` lets an unchanged body
    /// skip parsing and keeps a recent `UTCTiming` offset;
    /// `utc_timing_due` forces a new `UTCTiming` measurement. Requests wait
    /// out a cool-down after rate limiting until `throttle_deadline`.
    pub(crate) async fn fetch(
        &self,
        url: &Url,
        previous: Option<&FetchedMpd>,
        utc_timing_due: bool,
        throttle_deadline: Instant,
    ) -> Result<FetchedMpd, DownloadError> {
        let resource = ResourceId::DashManifest {
            url: Arc::from(url.as_str()),
        };
        self.emit(DownloadEvent::ResourceStarted {
            resource: resource.clone(),
            display_url: Arc::from(Redacted(url).to_string()),
            content_length: None,
        });
        let client = self.clients.client_for_url(url);
        let request = client
            .get(url.clone())
            .timeout(self.config.manifest.fetch_timeout)
            .query(&self.config.base.params);
        let admission = self
            .clients
            .rate_limiter()
            .admit(url, &self.cancel, throttle_deadline, "MPD fetch")
            .await?;
        let sent = self.config.clock.now();
        let response = tokio::select! {
            _ = self.cancel.cancelled() => return Err(DownloadError::Cancelled),
            response = request.send() => response,
        }
        .map_err(DownloadError::from)?;
        let status = response.status();
        if let Some(retry_in) = admission.observe(response.url(), status, response.headers()) {
            return Err(DownloadError::rate_limited(
                status,
                response.url().as_str(),
                "MPD fetch",
                retry_in,
            ));
        }
        drop(admission);
        if !status.is_success() {
            return Err(DownloadError::http_status(
                status,
                url.as_str(),
                "MPD fetch",
            ));
        }
        let date = response
            .headers()
            .get(DATE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_http_date(value).ok());
        let document_url = Arc::new(response.url().clone());
        let body = tokio::select! {
            _ = self.cancel.cancelled() => return Err(DownloadError::Cancelled),
            body = read_capped(response, MAX_MPD_BYTES) => body,
        }
        .map_err(|error| match error {
            CappedRead::TooLarge(size) => ManifestError::TooLarge {
                size,
                limit: MAX_MPD_BYTES,
            }
            .into(),
            CappedRead::Network(error) => error,
        })?;
        let received = self.config.clock.now();
        self.emit(DownloadEvent::ResourceFinished {
            resource,
            bytes: body.len() as u64,
            from_cache: false,
        });

        let manifest = match previous {
            Some(previous) if previous.body == body && previous.document_url == document_url => {
                Arc::clone(&previous.manifest)
            }
            _ => {
                let parse_body = body.clone();
                let parse_url = Arc::clone(&document_url);
                // Parsing a large MPD is CPU work measured in milliseconds;
                // keep it off the async workers.
                let mut parsed =
                    tokio::task::spawn_blocking(move || Manifest::parse(&parse_body, &parse_url))
                        .await
                        .map_err(|e| DownloadError::Internal {
                            reason: format!("MPD parse task failed: {e}"),
                        })??;
                // Recording is under way: a planner keeps its Representation
                // while it stays usable, so it may not need a failed index.
                self.read_segment_indexes(
                    &mut parsed,
                    url,
                    OnIndexFailure::Skip,
                    throttle_deadline,
                )
                .await?;
                Arc::new(parsed)
            }
        };

        let (clock_offset, utc_synced) = self
            .measure_clock(&manifest, date, sent, received, previous, utc_timing_due)
            .await;
        Ok(FetchedMpd {
            manifest,
            document_url,
            body,
            fetched_at: received,
            clock_offset,
            utc_synced,
        })
    }

    /// The server clock offset, from the first `UTCTiming` source that
    /// answers, else the MPD response's `Date` header, else none.
    async fn measure_clock(
        &self,
        manifest: &Manifest,
        date: Option<DateTime<Utc>>,
        sent: DateTime<Utc>,
        received: DateTime<Utc>,
        previous: Option<&FetchedMpd>,
        utc_timing_due: bool,
    ) -> (ClockOffset, bool) {
        if !self.config.manifest.sync_clock {
            return (ClockOffset::ZERO, false);
        }
        if !manifest.utc_timings.is_empty() {
            if !utc_timing_due && let Some(previous) = previous.filter(|p| p.utc_synced) {
                return (previous.clock_offset, true);
            }
            let synced = tokio::time::timeout(CLOCK_SYNC_BUDGET, async {
                for timing in &manifest.utc_timings {
                    if let Some(offset) = self.query_timing(timing, sent, received).await {
                        return Some(offset);
                    }
                }
                None
            })
            .await;
            if let Ok(Some(offset)) = synced {
                debug!(
                    offset_ms = offset.as_time_delta().num_milliseconds(),
                    "clock synchronized from UTCTiming"
                );
                return (offset, true);
            }
            warn!("no UTCTiming source answered in time; falling back to the MPD Date header");
        }
        match date {
            Some(date) => (ClockOffset::from_exchange(date, sent, received), false),
            None => (ClockOffset::ZERO, false),
        }
    }

    async fn query_timing(
        &self,
        timing: &UtcTiming,
        mpd_sent: DateTime<Utc>,
        mpd_received: DateTime<Utc>,
    ) -> Option<ClockOffset> {
        let urls = match timing {
            UtcTiming::Direct { time } => {
                return Some(ClockOffset::from_exchange(*time, mpd_sent, mpd_received));
            }
            UtcTiming::HttpIso { urls }
            | UtcTiming::HttpXsDate { urls }
            | UtcTiming::HttpHead { urls } => urls,
        };
        for url in urls {
            let client = self.clients.client_for_url(url);
            let head = matches!(timing, UtcTiming::HttpHead { .. });
            let request = if head {
                client.head(url.clone())
            } else {
                client.get(url.clone())
            }
            .timeout(self.config.manifest.fetch_timeout.min(CLOCK_SYNC_BUDGET));
            let sent = self.config.clock.now();
            let response = tokio::select! {
                _ = self.cancel.cancelled() => return None,
                response = request.send() => response,
            };
            let response = match response {
                Ok(response) if response.status().is_success() => response,
                Ok(response) => {
                    debug!(status = %response.status(), url = %Redacted(url), "UTCTiming source failed");
                    continue;
                }
                Err(error) => {
                    debug!(error = %crate::redact::redact_reqwest(error), "UTCTiming source failed");
                    continue;
                }
            };
            let server_time = if head {
                response
                    .headers()
                    .get(DATE)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| parse_http_date(value).ok())
            } else {
                let body = tokio::select! {
                    _ = self.cancel.cancelled() => return None,
                    body = read_capped(response, MAX_TIMING_BODY_BYTES) => body.ok(),
                };
                body.and_then(|body| UtcTiming::parse_body(&body).ok())
            };
            let received = self.config.clock.now();
            if let Some(server_time) = server_time {
                return Some(ClockOffset::from_exchange(server_time, sent, received));
            }
            debug!(url = %Redacted(url), "UTCTiming response carried no usable time");
        }
        None
    }

    fn emit(&self, event: DownloadEvent) {
        if let Some(events) = &self.events {
            events.emit(event);
        }
    }
}

enum CappedRead {
    TooLarge(usize),
    Network(DownloadError),
}

/// Reads a response body, failing once it exceeds `limit` bytes instead of
/// buffering an unbounded one.
async fn read_capped(response: reqwest::Response, limit: usize) -> Result<Bytes, CappedRead> {
    if let Some(length) = response.content_length()
        && length > limit as u64
    {
        return Err(CappedRead::TooLarge(
            usize::try_from(length).unwrap_or(usize::MAX),
        ));
    }
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CappedRead::Network(DownloadError::from(error)))?;
        let size = body.len().saturating_add(chunk.len());
        if size > limit {
            return Err(CappedRead::TooLarge(size));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body.freeze())
}

/// Builds the snapshot for a fetched MPD.
pub(crate) fn snapshot(
    generation: u64,
    manifest_generation: u64,
    fetched: &FetchedMpd,
    parent_params: &Arc<[(String, String)]>,
) -> MpdSnapshot {
    MpdSnapshot {
        generation,
        manifest_generation,
        manifest: Arc::clone(&fetched.manifest),
        document_url: Arc::clone(&fetched.document_url),
        parent_params: Arc::clone(parent_params),
        fetched_at: fetched.fetched_at,
        clock_offset: fetched.clock_offset,
        terminal: None,
    }
}

/// A refresh's result, whether it measured `UTCTiming`, and when it began.
type RefreshOutput = (Result<FetchedMpd, DownloadError>, bool, Instant);

type Refresh = Pin<Box<dyn Future<Output = RefreshOutput> + Send>>;

/// The in-flight refresh, or never when there is none.
async fn refresh_done(refresh: &mut Option<Refresh>) -> RefreshOutput {
    match refresh {
        Some(refresh) => refresh.await,
        None => std::future::pending().await,
    }
}

pub(crate) struct MpdWatcher {
    fetcher: Arc<MpdFetcher>,
    config: Arc<DashConfig>,
    /// The URL refreshes are requested from (the MPD URL, or its `Location`).
    url: Url,
    cancel: CancellationToken,
}

impl MpdWatcher {
    pub(crate) fn new(
        fetcher: Arc<MpdFetcher>,
        config: Arc<DashConfig>,
        url: Url,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            fetcher,
            config,
            url,
            cancel,
        }
    }

    /// Spawn the watcher. `initial` (fetched during setup) becomes
    /// generation 0.
    pub(crate) fn spawn(
        self,
        initial: FetchedMpd,
    ) -> (watch::Receiver<MpdSnapshot>, tokio::task::JoinHandle<()>) {
        let parent_params: Arc<[(String, String)]> =
            crate::segment_engine::descriptor::query_params(self.url.query()).into();
        let (tx, rx) = watch::channel(snapshot(0, 0, &initial, &parent_params));
        let handle = tokio::spawn(self.run(tx, Arc::new(initial), parent_params));
        (rx, handle)
    }

    async fn run(
        mut self,
        tx: watch::Sender<MpdSnapshot>,
        initial: Arc<FetchedMpd>,
        parent_params: Arc<[(String, String)]>,
    ) {
        let mut current = initial;
        let mut generation: u64 = 0;
        let mut manifest_generation: u64 = 0;
        let mut retries: u32 = 0;
        // When the refreshes started being rate limited, while they are.
        let mut throttled_since: Option<Instant> = None;
        let mut last_utc_sync = Instant::now();
        if let Some(location) = &current.manifest.location {
            self.url = location.clone();
        }
        let mut next_fetch = self.refresh_interval(&current.manifest).map(deadline);
        let mut next_republish = deadline(republish_interval(&current, &self.config));
        let mut refresh: Option<Refresh> = None;

        loop {
            if refresh.is_none() && next_fetch.is_some_and(|at| at <= Instant::now()) {
                next_fetch = None;
                let fetcher = Arc::clone(&self.fetcher);
                let url = self.url.clone();
                let previous = Arc::clone(&current);
                let utc_due = last_utc_sync.elapsed() >= UTC_TIMING_RESYNC;
                let started = Instant::now();
                // Waiting for a cooling server counts towards the bound on
                // rate limiting like refused refreshes do.
                let throttle_deadline =
                    throttled_since.unwrap_or(started) + self.fetcher.max_throttled();
                refresh = Some(Box::pin(async move {
                    let result = fetcher
                        .fetch(&url, Some(&previous), utc_due, throttle_deadline)
                        .await;
                    (result, utc_due, started)
                }));
            }
            let wake = next_fetch.map_or(next_republish, |fetch| fetch.min(next_republish));
            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    debug!("MPD watcher cancelled");
                    return;
                }
                _ = tx.closed() => {
                    debug!("MPD snapshot receivers dropped; watcher exiting");
                    return;
                }
                (result, utc_due, started) = refresh_done(&mut refresh) => {
                    refresh = None;
                    match result {
                        Ok(fetched) => {
                            retries = 0;
                            throttled_since = None;
                            if fetched.utc_synced && utc_due {
                                last_utc_sync = Instant::now();
                            }
                            if !Arc::ptr_eq(&fetched.manifest, &current.manifest) {
                                manifest_generation += 1;
                            }
                            if let Some(location) = &fetched.manifest.location {
                                self.url = location.clone();
                            }
                            current = Arc::new(fetched);
                            generation += 1;
                            let published =
                                snapshot(generation, manifest_generation, &current, &parent_params);
                            if tx.send(published).is_err() {
                                return;
                            }
                            if current.manifest.presentation_type == PresentationType::Static {
                                info!(url = %Redacted(&self.url), "MPD became static; no further refreshes");
                            }
                            next_fetch = self.refresh_interval(&current.manifest).map(deadline);
                            next_republish = deadline(republish_interval(&current, &self.config));
                        }
                        Err(DownloadError::Cancelled) => return,
                        Err(error) => {
                            // Rate limiting does not spend the retry budget:
                            // the current MPD keeps being republished, so
                            // planning goes on, and the stream fails only
                            // once the throttling has lasted its bound.
                            let delay = if let DownloadError::RateLimited { retry_in, .. } = &error {
                                let since = *throttled_since.get_or_insert(started);
                                warn!(
                                    url = %Redacted(&self.url),
                                    throttled_secs = since.elapsed().as_secs(),
                                    error = %crate::redact::redact_diagnostic(&error),
                                    "MPD refresh rate limited"
                                );
                                self.fetcher
                                    .allowance_left(since)
                                    .map(|left| (*retry_in).min(left))
                            } else {
                                retries += 1;
                                warn!(
                                    url = %Redacted(&self.url),
                                    retries,
                                    error = %crate::redact::redact_diagnostic(&error),
                                    "MPD refresh failed"
                                );
                                (retries <= self.config.manifest.max_refresh_retries).then(|| {
                                    self.config.manifest.retry_delay.saturating_mul(retries)
                                })
                            };
                            let Some(delay) = delay else {
                                let cause = TerminalCause::Failed(SharedError::new(error));
                                tx.send_modify(|snapshot| {
                                    snapshot.generation = generation + 1;
                                    snapshot.terminal = Some(cause);
                                });
                                return;
                            };
                            next_fetch = Some(deadline(delay));
                        }
                    }
                    continue;
                }
                _ = tokio::time::sleep_until(wake) => {}
            }

            if next_republish <= Instant::now() {
                generation += 1;
                let published = snapshot(generation, manifest_generation, &current, &parent_params);
                if tx.send(published).is_err() {
                    return;
                }
                next_republish = deadline(republish_interval(&current, &self.config));
            }
        }
    }

    /// `max(minimumUpdatePeriod, min_refresh_interval)`; `None` when a
    /// dynamic MPD declares no update period, meaning it does not change
    /// (ISO/IEC 23009-1 §5.4.1).
    /// A static MPD is not fetched again.
    fn refresh_interval(&self, manifest: &Manifest) -> Option<Duration> {
        if manifest.presentation_type == PresentationType::Static {
            return None;
        }
        manifest
            .minimum_update_period
            .map(|period| period.max(self.config.manifest.min_refresh_interval))
    }
}

/// About half the shortest segment duration currently available in any
/// Representation, so a newly available segment is planned within half a
/// segment of its availability.
fn republish_interval(fetched: &FetchedMpd, config: &DashConfig) -> Duration {
    if fetched.manifest.presentation_type == PresentationType::Static {
        return STATIC_REPUBLISH_INTERVAL;
    }
    let now = fetched.clock_offset.apply(config.clock.now());
    let shortest = fetched
        .manifest
        .periods
        .iter()
        .flat_map(|period| &period.adaptation_sets)
        .flat_map(|set| &set.representations)
        .filter_map(|representation| {
            let window = representation.segments(now).ok()?;
            let segment = window.last()?.ok()?;
            (segment.timescale > 0)
                .then(|| Duration::try_from_secs_f64(segment.duration_secs()).ok())
                .flatten()
        })
        .filter(|duration| !duration.is_zero())
        .min();
    shortest
        .map(|duration| duration / 2)
        .unwrap_or(DEFAULT_REPUBLISH_INTERVAL)
        .clamp(MIN_REPUBLISH_INTERVAL, MAX_REPUBLISH_INTERVAL)
}
