//! The playlist watcher task (the only place playlists are fetched).
//!
//! Publishes `PlaylistSnapshot` values through a coalescing, single-slot,
//! latest-wins `tokio::sync::watch` channel. Coalescing can drop intermediate
//! generations; that is safe only because the planner detects MSN-base gaps
//! explicitly (see `plan`) — a live window is a sliding window, not a
//! superset.
//!
//! The watcher decides nothing about segments: no eligibility, no in-flight
//! tracking, no per-segment retry, and no consumer-facing events. The terminal
//! cause is carried *on the snapshot* (`TerminalCause`), never inferred from a
//! sender drop, so a watcher crash can never masquerade as a clean ENDLIST.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use m3u8_rs::MediaPlaylist;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};
use url::Url;

use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::HlsConfig;
use crate::hls::twitch_processor::{TwitchPlaylistProcessor, preprocess_twitch_playlist};
use crate::session::{DownloadEvent, EventSink, ResourceId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalCause {
    /// `EXT-X-ENDLIST` observed: authoritative end. The snapshot carrying this
    /// still contains the final window and must be planned before draining.
    Endlist,
    /// The watcher could not keep refreshing (fetch/parse failure after
    /// retries). A pipeline error, never a clean end.
    Failed(Arc<str>),
}

/// One observed playlist generation. Cheap to clone — the reactor clones the
/// borrowed value on every read, so the parsed playlist sits behind an `Arc`.
#[derive(Debug, Clone)]
pub struct PlaylistSnapshot {
    /// Monotonic refresh generation, for tracing and staleness checks.
    pub generation: u64,
    pub playlist: Arc<MediaPlaylist>,
    pub base_url: Arc<str>,
    /// Query string of the media-playlist URL; segment URLs missing these
    /// params inherit them (query-param inheritance, see the planner).
    pub parent_query: Option<Arc<str>>,
    pub terminal: Option<TerminalCause>,
}

pub struct PlaylistWatcher {
    clients: Arc<ClientPool>,
    config: Arc<HlsConfig>,
    playlist_url: Url,
    base_url: Arc<str>,
    cancel: CancellationToken,
    events: Option<EventSink>,
}

impl PlaylistWatcher {
    pub fn new(
        clients: Arc<ClientPool>,
        config: Arc<HlsConfig>,
        playlist_url: Url,
        base_url: Arc<str>,
        cancel: CancellationToken,
    ) -> Self {
        Self::new_with_events(clients, config, playlist_url, base_url, cancel, None)
    }

    pub fn new_with_events(
        clients: Arc<ClientPool>,
        config: Arc<HlsConfig>,
        playlist_url: Url,
        base_url: Arc<str>,
        cancel: CancellationToken,
        events: Option<EventSink>,
    ) -> Self {
        Self {
            clients,
            config,
            playlist_url,
            base_url,
            cancel,
            events,
        }
    }

    pub fn with_events(mut self, events: Option<EventSink>) -> Self {
        self.events = events;
        self
    }

    /// Spawn the watcher task. The initial playlist (already fetched during
    /// setup) becomes generation 0 and is retained as the channel's first
    /// value; a VOD/ENDLIST initial playlist terminates immediately.
    pub fn spawn(
        self,
        initial_playlist: MediaPlaylist,
    ) -> (
        watch::Receiver<PlaylistSnapshot>,
        tokio::task::JoinHandle<()>,
    ) {
        let parent_query: Option<Arc<str>> = self.playlist_url.query().map(Arc::from);
        let initial_terminal = initial_playlist.end_list.then_some(TerminalCause::Endlist);
        let initial = PlaylistSnapshot {
            generation: 0,
            playlist: Arc::new(initial_playlist),
            base_url: Arc::clone(&self.base_url),
            parent_query: parent_query.clone(),
            terminal: initial_terminal.clone(),
        };
        let (tx, rx) = watch::channel(initial);

        let handle = tokio::spawn(async move {
            if initial_terminal.is_some() {
                // VOD: the retained generation-0 snapshot already carries
                // Endlist; nothing to refresh.
                return;
            }
            self.run(tx, parent_query).await;
        });
        (rx, handle)
    }

    async fn run(self, tx: watch::Sender<PlaylistSnapshot>, parent_query: Option<Arc<str>>) {
        let mut generation: u64 = 0;
        let mut retries: u32 = 0;
        let mut last_playlist_bytes: Option<bytes::Bytes> = None;
        let mut current_target_duration = tx.borrow().playlist.target_duration as f64;
        // Tracks the end of the highest window seen, to feed the adaptive
        // refresh tracker with "how many segments were new this refresh".
        let mut last_window_end: Option<u64> = {
            let snapshot = tx.borrow();
            Some(snapshot.playlist.media_sequence + snapshot.playlist.segments.len() as u64)
        };

        let mut tracker = AdaptiveRefreshTracker::new(
            self.config.playlist_config.adaptive_refresh_enabled,
            self.config.playlist_config.adaptive_refresh_min_interval,
            self.config.playlist_config.adaptive_refresh_max_interval,
        );

        loop {
            let base_refresh_interval = Duration::from_secs_f64(current_target_duration * 0.5)
                .max(self.config.playlist_config.live_refresh_interval);
            let refresh_delay = tracker.get_refresh_interval(base_refresh_interval);

            tokio::select! {
                biased;
                _ = self.cancel.cancelled() => {
                    info!("Playlist watcher cancelled: {}", self.playlist_url);
                    return;
                }
                _ = tx.closed() => {
                    debug!("Playlist snapshot receiver dropped; watcher exiting");
                    return;
                }
                _ = tokio::time::sleep(refresh_delay) => {}
            }

            match self.fetch_and_parse(&last_playlist_bytes).await {
                Ok(Some((playlist, raw_bytes))) => {
                    retries = 0;
                    generation += 1;
                    current_target_duration = playlist.target_duration as f64;

                    let window_end = playlist.media_sequence + playlist.segments.len() as u64;
                    let new_segments = last_window_end
                        .map(|prev| window_end.saturating_sub(prev) as usize)
                        .unwrap_or(playlist.segments.len());
                    last_window_end = Some(window_end);
                    tracker.record_refresh(new_segments);

                    let terminal = playlist.end_list.then_some(TerminalCause::Endlist);
                    let ended = terminal.is_some();
                    let snapshot = PlaylistSnapshot {
                        generation,
                        playlist: Arc::new(playlist),
                        base_url: Arc::clone(&self.base_url),
                        parent_query: parent_query.clone(),
                        terminal,
                    };
                    last_playlist_bytes = Some(raw_bytes);
                    if tx.send(snapshot).is_err() {
                        debug!("Playlist snapshot receiver dropped; watcher exiting");
                        return;
                    }
                    if ended {
                        info!("Playlist watcher finished (ENDLIST): {}", self.playlist_url);
                        // The Endlist snapshot is retained as the latest value;
                        // dropping the sender after it is unambiguous.
                        return;
                    }
                }
                Ok(None) => {
                    // Unchanged playlist: nothing to publish (URLs unchanged
                    // means there is no fetch metadata to refresh either).
                    retries = 0;
                    tracker.record_refresh(0);
                }
                Err(e) => {
                    if matches!(e, HlsDownloaderError::Cancelled) {
                        return;
                    }
                    error!("Error refreshing playlist {}: {e}", self.playlist_url);
                    retries += 1;
                    if retries > self.config.playlist_config.live_max_refresh_retries {
                        // Publish the explicit failure cause before dropping
                        // the sender, so the reactor can distinguish this from
                        // a clean end.
                        tx.send_modify(|snapshot| {
                            snapshot.terminal =
                                Some(TerminalCause::Failed(Arc::from(e.to_string())));
                        });
                        return;
                    }
                    tokio::select! {
                        biased;
                        _ = self.cancel.cancelled() => return,
                        _ = tokio::time::sleep(
                            self.config.playlist_config.live_refresh_retry_delay * retries,
                        ) => {}
                    }
                }
            }
        }
    }

    /// Fetch and parse one refresh. `Ok(None)` means byte-identical to the
    /// previous fetch (parse skipped).
    async fn fetch_and_parse(
        &self,
        last_playlist_bytes: &Option<bytes::Bytes>,
    ) -> Result<Option<(MediaPlaylist, bytes::Bytes)>, HlsDownloaderError> {
        if self.cancel.is_cancelled() {
            return Err(HlsDownloaderError::Cancelled);
        }

        let client = self.clients.client_for_url(&self.playlist_url);
        let resource = ResourceId::HlsPlaylist {
            url: Arc::from(self.playlist_url.as_str()),
        };
        emit_event(
            &self.events,
            DownloadEvent::ResourceStarted {
                resource: resource.clone(),
                display_url: Arc::from(self.playlist_url.as_str()),
                content_length: None,
            },
        );
        let request = client
            .get(self.playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params);

        let response = tokio::select! {
            _ = self.cancel.cancelled() => return Err(HlsDownloaderError::Cancelled),
            response = request.send() => response,
        }
        .map_err(|e| HlsDownloaderError::Network { source: e })?;

        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch playlist {}: HTTP {}",
                    self.playlist_url,
                    response.status()
                ),
            });
        }

        let playlist_bytes = tokio::select! {
            _ = self.cancel.cancelled() => return Err(HlsDownloaderError::Cancelled),
            bytes = response.bytes() => bytes,
        }
        .map_err(|e| HlsDownloaderError::Network { source: e })?;
        emit_event(
            &self.events,
            DownloadEvent::ResourceFinished {
                resource,
                bytes: playlist_bytes.len() as u64,
                from_cache: false,
            },
        );

        if let Some(last_bytes) = last_playlist_bytes.as_ref()
            && last_bytes == &playlist_bytes
        {
            return Ok(None);
        }

        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(self.playlist_url.as_str()) {
                let playlist_content = String::from_utf8_lossy(&playlist_bytes);
                Cow::Owned(preprocess_twitch_playlist(&playlist_content).into_bytes())
            } else {
                Cow::Borrowed(&playlist_bytes)
            };

        match m3u8_rs::parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MediaPlaylist(new_mp)) => Ok(Some((new_mp, playlist_bytes))),
            Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Expected media playlist, got master for {}",
                    self.playlist_url
                ),
            }),
            Err(e) => Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to parse refreshed playlist {}: {e}",
                    self.playlist_url
                ),
            }),
        }
    }
}

fn emit_event(events: &Option<EventSink>, event: DownloadEvent) {
    if let Some(events) = events {
        events.emit(event);
    }
}

/// Tracks segment arrival patterns to adaptively adjust playlist refresh
/// intervals: aggressive when behind the live edge, backed off when refreshes
/// keep coming back empty.
struct AdaptiveRefreshTracker {
    enabled: bool,
    min_interval: Duration,
    max_interval: Duration,
    /// Recent refresh results: true = got new segments.
    recent_results: std::collections::VecDeque<bool>,
    consecutive_empty: u32,
    /// New segments discovered on the most recent refresh; >1 usually means
    /// we are behind and should poll harder.
    last_new_segments_count: usize,
}

impl AdaptiveRefreshTracker {
    fn new(enabled: bool, min_interval: Duration, max_interval: Duration) -> Self {
        Self {
            enabled,
            min_interval,
            max_interval,
            recent_results: std::collections::VecDeque::with_capacity(10),
            consecutive_empty: 0,
            last_new_segments_count: 0,
        }
    }

    fn record_refresh(&mut self, new_segments_count: usize) {
        self.last_new_segments_count = new_segments_count;
        let got_segments = new_segments_count > 0;

        if self.recent_results.len() >= 10 {
            self.recent_results.pop_front();
        }
        self.recent_results.push_back(got_segments);

        if got_segments {
            self.consecutive_empty = 0;
        } else {
            self.consecutive_empty += 1;
        }
    }

    fn clamp_interval(&self, interval: Duration) -> Duration {
        interval.max(self.min_interval).min(self.max_interval)
    }

    fn get_refresh_interval(&self, default_interval: Duration) -> Duration {
        if !self.enabled {
            return default_interval;
        }

        let mut interval = default_interval;

        if self.last_new_segments_count >= 2 {
            // Multiple unseen segments: we are behind, catch up.
            interval = self.min_interval;
        } else if self.consecutive_empty >= 3 {
            // Exponential backoff after several empty refreshes.
            let backoff_factor = 1.5_f64.powi(self.consecutive_empty.min(5) as i32);
            interval = Duration::from_secs_f64(default_interval.as_secs_f64() * backoff_factor);
        } else {
            let recent_success_rate = self.recent_results.iter().filter(|&&got| got).count() as f64
                / self.recent_results.len().max(1) as f64;
            if recent_success_rate > 0.8 && self.recent_results.len() >= 5 {
                interval = Duration::from_secs_f64(default_interval.as_secs_f64() * 0.8);
            }
        }

        self.clamp_interval(interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{StatusCode, Uri};
    use axum::response::Response;

    #[test]
    fn adaptive_refresh_backoff_respects_min_interval() {
        let mut tracker =
            AdaptiveRefreshTracker::new(true, Duration::from_millis(500), Duration::from_secs(3));
        for _ in 0..3 {
            tracker.record_refresh(0);
        }
        let interval = tracker.get_refresh_interval(Duration::from_millis(100));
        assert!(interval >= Duration::from_millis(500));

        for _ in 0..7 {
            tracker.record_refresh(0);
        }
        let interval = tracker.get_refresh_interval(Duration::from_secs(10));
        assert!(interval <= Duration::from_secs(3));
    }

    #[test]
    fn adaptive_refresh_catches_up_when_behind() {
        let mut tracker =
            AdaptiveRefreshTracker::new(true, Duration::from_millis(500), Duration::from_secs(3));
        tracker.record_refresh(3);
        let interval = tracker.get_refresh_interval(Duration::from_secs(1));
        assert_eq!(interval, Duration::from_millis(500));
    }

    fn media_playlist(input: &str) -> MediaPlaylist {
        match m3u8_rs::parse_playlist_res(input.as_bytes()).expect("playlist parses") {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            m3u8_rs::Playlist::MasterPlaylist(_) => panic!("expected media playlist"),
        }
    }

    async fn frozen_playlist_handler(State(body): State<Arc<str>>, uri: Uri) -> Response {
        if uri.path() != "/live.m3u8" {
            return Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .expect("response builds");
        }

        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(body.to_string()))
            .expect("response builds")
    }

    async fn serve_frozen_playlist(body: &'static str) -> Url {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock origin");
        let addr = listener.local_addr().expect("local addr");
        let app = Router::new()
            .fallback(frozen_playlist_handler)
            .with_state(Arc::from(body));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Url::parse(&format!("http://{addr}/live.m3u8")).expect("playlist URL")
    }

    #[tokio::test]
    async fn vod_initial_playlist_publishes_endlist_terminal_and_closes() {
        let config = Arc::new(HlsConfig::default());
        let clients =
            Arc::new(crate::downloader::create_client_pool(&config.base).expect("client pool"));
        let watcher = PlaylistWatcher::new(
            clients,
            config,
            Url::parse("https://example.com/v.m3u8").unwrap(),
            Arc::from("https://example.com/"),
            CancellationToken::new(),
        );
        let playlist = media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:2.0,\nseg0.ts\n#EXT-X-ENDLIST\n",
        );
        let (mut rx, handle) = watcher.spawn(playlist);

        let snapshot = rx.borrow_and_update().clone();
        assert_eq!(snapshot.generation, 0);
        assert_eq!(snapshot.terminal, Some(TerminalCause::Endlist));

        handle.await.unwrap();
        // Sender dropped after the terminal value: changed() errors, and the
        // retained value still carries the cause.
        assert!(rx.changed().await.is_err());
        assert_eq!(rx.borrow().terminal, Some(TerminalCause::Endlist));
    }

    #[tokio::test]
    async fn dropped_receiver_stops_watcher_on_unchanged_refresh_path() {
        let body = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:0\n#EXT-X-MEDIA-SEQUENCE:0\n#EXTINF:0.5,\nseg0.ts\n";
        let playlist_url = serve_frozen_playlist(body).await;
        let mut config = HlsConfig::default();
        config.playlist_config.live_refresh_interval = Duration::from_millis(5);
        config.playlist_config.adaptive_refresh_enabled = false;
        config.playlist_config.initial_playlist_fetch_timeout = Duration::from_secs(1);
        let config = Arc::new(config);
        let clients =
            Arc::new(crate::downloader::create_client_pool(&config.base).expect("client pool"));
        let watcher = PlaylistWatcher::new(
            clients,
            config,
            playlist_url,
            Arc::from("http://127.0.0.1/"),
            CancellationToken::new(),
        );
        let initial = media_playlist(body);
        let (mut rx, handle) = watcher.spawn(initial);

        tokio::time::timeout(Duration::from_secs(1), rx.changed())
            .await
            .expect("first refresh")
            .expect("watcher stays open");
        assert_eq!(rx.borrow().generation, 1);

        drop(rx);

        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .expect("watcher exits after receiver drop")
            .expect("watcher task joins");
    }
}
