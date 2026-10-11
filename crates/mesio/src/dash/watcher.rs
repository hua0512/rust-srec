//! The MPD watcher task: the only place the MPD and clock sources are
//! fetched.
//!
//! Publishes [`MpdSnapshot`] values through a coalescing `watch` channel
//! shared by the per-track planners. Besides re-fetching at the MPD's
//! update period, the watcher republishes the current MPD on a timer:
//! segments of a `@duration` template become available with wall-clock time
//! alone, without any MPD change, and planners only run when a snapshot is
//! published.
//!
//! As with HLS, the terminal cause travels on the snapshot. A static MPD
//! (from the start, or after a dynamic MPD turned static) is published with
//! `TerminalCause::End` and nothing is fetched afterwards; refresh failures
//! beyond the retry budget are published as a failure cause.

use std::sync::Arc;
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
use crate::downloader::ClientPool;
use crate::error::DownloadError;
use crate::redact::Redacted;
use crate::segment_engine::control::TerminalCause;
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
        }
    }

    /// Fetch and resolve the MPD at `url`. `previous` lets an unchanged body
    /// skip parsing and keeps a recent `UTCTiming` offset;
    /// `utc_timing_due` forces a new `UTCTiming` measurement.
    pub(crate) async fn fetch(
        &self,
        url: &Url,
        previous: Option<&FetchedMpd>,
        utc_timing_due: bool,
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
        let sent = self.config.clock.now();
        let response = tokio::select! {
            _ = self.cancel.cancelled() => return Err(DownloadError::Cancelled),
            response = request.send() => response,
        }
        .map_err(DownloadError::from)?;
        let status = response.status();
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
            body = read_capped(response) => body,
        }?;
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
                let parsed =
                    tokio::task::spawn_blocking(move || Manifest::parse(&parse_body, &parse_url))
                        .await
                        .map_err(|e| DownloadError::Internal {
                            reason: format!("MPD parse task failed: {e}"),
                        })?;
                Arc::new(parsed?)
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
            for timing in &manifest.utc_timings {
                if let Some(offset) = self.query_timing(timing, sent, received).await {
                    debug!(
                        offset_ms = offset.as_time_delta().num_milliseconds(),
                        "clock synchronized from UTCTiming"
                    );
                    return (offset, true);
                }
            }
            warn!("no UTCTiming source answered; falling back to the MPD Date header");
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
            .timeout(self.config.manifest.fetch_timeout);
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
                    body = response.bytes() => body.ok(),
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

/// Reads a response body, failing once it exceeds [`MAX_MPD_BYTES`]
/// instead of buffering an unbounded document.
async fn read_capped(response: reqwest::Response) -> Result<Bytes, DownloadError> {
    let too_large = |size: usize| -> DownloadError {
        ManifestError::TooLarge {
            size,
            limit: MAX_MPD_BYTES,
        }
        .into()
    };
    if let Some(length) = response.content_length()
        && length > MAX_MPD_BYTES as u64
    {
        return Err(too_large(usize::try_from(length).unwrap_or(usize::MAX)));
    }
    let mut body = BytesMut::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(DownloadError::from)?;
        let size = body.len().saturating_add(chunk.len());
        if size > MAX_MPD_BYTES {
            return Err(too_large(size));
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
    let terminal = (fetched.manifest.presentation_type == PresentationType::Static)
        .then_some(TerminalCause::End);
    MpdSnapshot {
        generation,
        manifest_generation,
        manifest: Arc::clone(&fetched.manifest),
        document_url: Arc::clone(&fetched.document_url),
        parent_params: Arc::clone(parent_params),
        fetched_at: fetched.fetched_at,
        clock_offset: fetched.clock_offset,
        terminal,
    }
}

pub(crate) struct MpdWatcher {
    fetcher: MpdFetcher,
    config: Arc<DashConfig>,
    /// The URL refreshes are requested from (the MPD URL, or its `Location`).
    url: Url,
    cancel: CancellationToken,
}

impl MpdWatcher {
    pub(crate) fn new(
        fetcher: MpdFetcher,
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

    /// Spawn the watcher. `initial` (fetched during setup) becomes generation
    /// 0; a static presentation publishes it as final and fetches nothing
    /// more.
    pub(crate) fn spawn(
        self,
        initial: FetchedMpd,
    ) -> (watch::Receiver<MpdSnapshot>, tokio::task::JoinHandle<()>) {
        let parent_params: Arc<[(String, String)]> =
            crate::segment_engine::descriptor::query_params(self.url.query()).into();
        let first = snapshot(0, 0, &initial, &parent_params);
        let ended = first.terminal.is_some();
        let (tx, rx) = watch::channel(first);
        let handle = tokio::spawn(async move {
            if ended {
                return;
            }
            self.run(tx, initial, parent_params).await;
        });
        (rx, handle)
    }

    async fn run(
        mut self,
        tx: watch::Sender<MpdSnapshot>,
        initial: FetchedMpd,
        parent_params: Arc<[(String, String)]>,
    ) {
        let mut current = initial;
        let mut generation: u64 = 0;
        let mut manifest_generation: u64 = 0;
        let mut retries: u32 = 0;
        let mut last_utc_sync = Instant::now();
        if let Some(location) = &current.manifest.location {
            self.url = location.clone();
        }
        let mut next_fetch = self.refresh_interval(&current.manifest).map(deadline);
        let mut next_republish = deadline(republish_interval(&current, &self.config));

        loop {
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
                _ = tokio::time::sleep_until(wake) => {}
            }
            let now = Instant::now();

            if next_fetch.is_some_and(|at| at <= now) {
                let utc_due = last_utc_sync.elapsed() >= UTC_TIMING_RESYNC;
                match self.fetcher.fetch(&self.url, Some(&current), utc_due).await {
                    Ok(fetched) => {
                        retries = 0;
                        if fetched.utc_synced && utc_due {
                            last_utc_sync = Instant::now();
                        }
                        let changed = !Arc::ptr_eq(&fetched.manifest, &current.manifest);
                        if changed {
                            manifest_generation += 1;
                        }
                        if let Some(location) = &fetched.manifest.location {
                            self.url = location.clone();
                        }
                        current = fetched;
                        generation += 1;
                        let published =
                            snapshot(generation, manifest_generation, &current, &parent_params);
                        let ended = published.terminal.is_some();
                        if tx.send(published).is_err() {
                            return;
                        }
                        if ended {
                            info!(url = %Redacted(&self.url), "MPD became static; watcher finished");
                            return;
                        }
                        next_fetch = self.refresh_interval(&current.manifest).map(deadline);
                        next_republish = deadline(republish_interval(&current, &self.config));
                        continue;
                    }
                    Err(DownloadError::Cancelled) => return,
                    Err(error) => {
                        retries += 1;
                        warn!(
                            url = %Redacted(&self.url),
                            retries,
                            error = %crate::redact::redact_diagnostic(&error),
                            "MPD refresh failed"
                        );
                        if retries > self.config.manifest.max_refresh_retries {
                            let cause = match error {
                                DownloadError::HttpStatus {
                                    status,
                                    url,
                                    operation,
                                } => TerminalCause::HttpFailure {
                                    status,
                                    url: Arc::from(url),
                                    operation,
                                },
                                error => TerminalCause::Failed(Arc::from(error.to_string())),
                            };
                            tx.send_modify(|snapshot| {
                                snapshot.generation = generation + 1;
                                snapshot.terminal = Some(cause);
                            });
                            return;
                        }
                        next_fetch = Some(deadline(
                            self.config.manifest.retry_delay.saturating_mul(retries),
                        ));
                    }
                }
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
    fn refresh_interval(&self, manifest: &Manifest) -> Option<Duration> {
        manifest
            .minimum_update_period
            .map(|period| period.max(self.config.manifest.min_refresh_interval))
    }
}

/// About half the shortest segment duration currently available in any
/// Representation, so a newly available segment is planned within half a
/// segment of its availability.
fn republish_interval(fetched: &FetchedMpd, config: &DashConfig) -> Duration {
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
