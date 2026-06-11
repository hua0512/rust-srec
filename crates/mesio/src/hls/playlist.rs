// HLS Playlist Engine: Handles fetching, parsing, and managing HLS playlists.

use crate::cache::{CacheKey, CacheManager, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::events::HlsStreamEvent;
use crate::hls::scheduler::ScheduledSegmentJob;
use crate::hls::segment_lifecycle::{
    SegmentJobKind, SegmentJobOutcome, SegmentLifecycleConfig, SegmentLifecycleRegistry,
};
use crate::hls::twitch_processor::TwitchPlaylistProcessor;
use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, MediaPlaylist, MediaSegment, parse_playlist_res};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};
use url::Url;

#[async_trait]
pub trait PlaylistProvider: Send + Sync {
    async fn load_initial_playlist(&self, url: &str)
    -> Result<InitialPlaylist, HlsDownloaderError>;
    async fn select_media_playlist(
        &self,
        initial_playlist_with_base_url: &InitialPlaylist,
        policy: &HlsVariantSelectionPolicy,
    ) -> Result<MediaPlaylistDetails, HlsDownloaderError>;
    async fn monitor_media_playlist(
        &self,
        playlist_url: &str,
        initial_playlist: MediaPlaylist,
        base_url: String,
        channels: PlaylistEngineChannels,
        token: CancellationToken,
    ) -> Result<(), HlsDownloaderError>;
}

pub struct PlaylistEngineChannels {
    pub segment_request_tx: mpsc::Sender<ScheduledSegmentJob>,
    pub segment_outcome_rx: mpsc::UnboundedReceiver<SegmentJobOutcome>,
    pub client_event_tx: mpsc::Sender<Result<HlsStreamEvent, HlsDownloaderError>>,
}

#[derive(Debug, Clone)]
pub enum InitialPlaylist {
    Master(MasterPlaylist, String),
    Media(MediaPlaylist, String),
}

#[derive(Debug, Clone)]
pub struct MediaPlaylistDetails {
    pub playlist: MediaPlaylist,
    pub url: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum PlaylistUpdateEvent {
    PlaylistRefreshed {
        media_sequence_base: u64,
        target_duration: u64,
    },
    PlaylistEnded,
}

pub struct PlaylistEngine {
    clients: Arc<ClientPool>,
    cache_service: Option<Arc<CacheManager>>,
    config: Arc<HlsConfig>,
}

#[derive(Default)]
struct SegmentParseContext {
    last_non_empty_segment_uri: Option<String>,
    last_byterange_uri: Option<String>,
    last_byterange_end: Option<u64>,
}

struct SegmentProcessingContext<'a> {
    lifecycle: &'a mut SegmentLifecycleRegistry,
    last_map_uri: &'a mut Option<String>,
    parse: &'a mut SegmentParseContext,
    twitch_processor: &'a mut Option<TwitchPlaylistProcessor>,
}

/// Tracks segment arrival patterns to adaptively adjust playlist refresh intervals.
/// This helps reduce unnecessary network requests when segments arrive predictably,
/// while being more aggressive when segments are arriving faster than expected.
struct AdaptiveRefreshTracker {
    enabled: bool,
    min_interval: Duration,
    max_interval: Duration,
    /// Recent refresh results: true = got new segments, false = no new segments
    recent_results: std::collections::VecDeque<bool>,
    /// Number of consecutive refreshes with no new segments
    consecutive_empty: u32,
    /// New segments discovered on the most recent refresh.
    /// When this is >1, it usually indicates we're behind and should refresh more aggressively.
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

    /// Record the result of a playlist refresh
    fn record_refresh(&mut self, new_segments_count: usize) {
        self.last_new_segments_count = new_segments_count;
        let got_segments = new_segments_count > 0;

        // Track recent results (keep last 10)
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

    /// Get the recommended refresh interval based on recent patterns
    fn get_refresh_interval(&self, default_interval: Duration) -> Duration {
        if !self.enabled {
            return default_interval;
        }

        let mut interval = default_interval;

        // If we discovered multiple unseen segments, we're likely behind; poll aggressively
        // to catch up and reduce end-to-end latency.
        if self.last_new_segments_count >= 2 {
            interval = self.min_interval;
        } else if self.consecutive_empty >= 3 {
            // Exponential backoff after several empty refreshes.
            let backoff_factor = 1.5_f64.powi(self.consecutive_empty.min(5) as i32);
            interval = Duration::from_secs_f64(default_interval.as_secs_f64() * backoff_factor);
        } else {
            // If we're consistently getting segments, we can poll slightly faster.
            let recent_success_rate = self.recent_results.iter().filter(|&&got| got).count() as f64
                / self.recent_results.len().max(1) as f64;

            if recent_success_rate > 0.8 && self.recent_results.len() >= 5 {
                interval = Duration::from_secs_f64(default_interval.as_secs_f64() * 0.8);
            }
        }

        self.clamp_interval(interval)
    }
}

#[async_trait]
impl PlaylistProvider for PlaylistEngine {
    async fn load_initial_playlist(
        &self,
        url_str: &str,
    ) -> Result<InitialPlaylist, HlsDownloaderError> {
        let playlist_url = Url::parse(url_str).map_err(|e| HlsDownloaderError::Playlist {
            reason: format!("Invalid playlist URL {url_str}: {e}"),
        })?;
        let cache_key = CacheKey::new(CacheResourceType::Playlist, playlist_url.as_str(), None);

        if let Some(cache_service) = &self.cache_service
            && let Ok(Some((cached_data, _, _))) = cache_service.get(&cache_key).await
        {
            let playlist_content = std::str::from_utf8(cached_data.as_ref()).map_err(|e| {
                HlsDownloaderError::Playlist {
                    reason: format!("Failed to parse cached playlist from UTF-8: {e}"),
                }
            })?;
            let playlist_bytes_to_parse: Cow<[u8]> =
                if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                    let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                    Cow::Owned(preprocessed.into_bytes())
                } else {
                    Cow::Borrowed(cached_data.as_ref())
                };
            let base_url_obj =
                playlist_url
                    .join(".")
                    .map_err(|e| HlsDownloaderError::Playlist {
                        reason: format!("Failed to determine base URL: {e}"),
                    })?;
            let base_url = base_url_obj.to_string();
            debug!(
                "Derived base URL from playlist: {} -> {}",
                playlist_url, base_url
            );
            return match parse_playlist_res(&playlist_bytes_to_parse) {
                Ok(m3u8_rs::Playlist::MasterPlaylist(pl)) => {
                    Ok(InitialPlaylist::Master(pl, base_url))
                }
                Ok(m3u8_rs::Playlist::MediaPlaylist(pl)) => {
                    Ok(InitialPlaylist::Media(pl, base_url))
                }
                Err(e) => Err(HlsDownloaderError::Playlist {
                    reason: format!("Failed to parse cached playlist: {e}"),
                }),
            };
        }

        let client = self.clients.client_for_url(&playlist_url);
        let response = client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(|e| HlsDownloaderError::Network { source: e })?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch playlist {playlist_url}: HTTP {}",
                    response.status()
                ),
            });
        }
        let playlist_bytes = response
            .bytes()
            .await
            .map_err(|e| HlsDownloaderError::Network { source: e })?;

        if let Some(cache_service) = &self.cache_service {
            let metadata = CacheMetadata::new(playlist_bytes.len() as u64)
                .with_expiration(self.config.playlist_config.initial_playlist_fetch_timeout);

            cache_service
                .put(cache_key, playlist_bytes.clone(), metadata)
                .await?;
        }
        let playlist_content = std::str::from_utf8(playlist_bytes.as_ref()).map_err(|e| {
            HlsDownloaderError::Playlist {
                reason: format!("Playlist content is not valid UTF-8: {e}"),
            }
        })?;
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                Cow::Owned(preprocessed.into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes.as_ref())
            };
        let base_url_obj = playlist_url
            .join(".")
            .map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Failed to determine base URL: {e}"),
            })?;
        let base_url = base_url_obj.to_string();
        debug!(
            "Derived base URL from playlist: {} -> {}",
            playlist_url, base_url
        );
        match parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MasterPlaylist(pl)) => Ok(InitialPlaylist::Master(pl, base_url)),
            Ok(m3u8_rs::Playlist::MediaPlaylist(pl)) => Ok(InitialPlaylist::Media(pl, base_url)),
            Err(e) => Err(HlsDownloaderError::Playlist {
                reason: format!("Failed to parse fetched playlist: {e}"),
            }),
        }
    }

    async fn select_media_playlist(
        &self,
        initial_playlist_with_base_url: &InitialPlaylist,
        policy: &HlsVariantSelectionPolicy,
    ) -> Result<MediaPlaylistDetails, HlsDownloaderError> {
        let (master_playlist_ref, master_base_url_str) = match initial_playlist_with_base_url {
            InitialPlaylist::Master(pl, base) => (pl, base),
            InitialPlaylist::Media(_, _) => {
                return Err(HlsDownloaderError::Playlist {
                    reason:
                        "select_media_playlist called with a MediaPlaylist, expected MasterPlaylist"
                            .to_string(),
                });
            }
        };
        if master_playlist_ref.variants.is_empty() {
            return Err(HlsDownloaderError::Playlist {
                reason: "Master playlist has no variants".to_string(),
            });
        }
        let selected_variant = match policy {
            HlsVariantSelectionPolicy::HighestBitrate => master_playlist_ref
                .variants
                .iter()
                .max_by_key(|v| v.bandwidth)
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: "No variants for HighestBitrate".to_string(),
                })?,
            HlsVariantSelectionPolicy::LowestBitrate => master_playlist_ref
                .variants
                .iter()
                .min_by_key(|v| v.bandwidth)
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: "No variants for LowestBitrate".to_string(),
                })?,
            HlsVariantSelectionPolicy::ClosestToBitrate(target_bw) => master_playlist_ref
                .variants
                .iter()
                .min_by_key(|v| (*target_bw as i64 - v.bandwidth as i64).abs())
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: format!("No variants for ClosestToBitrate: {target_bw}"),
                })?,
            HlsVariantSelectionPolicy::AudioOnly => master_playlist_ref
                .variants
                .iter()
                .find(|v| {
                    v.audio.is_some()
                        && v.video.is_none()
                        && v.codecs.as_ref().is_some_and(|c| c.contains("mp4a"))
                })
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: "No AudioOnly variant".to_string(),
                })?,
            HlsVariantSelectionPolicy::VideoOnly => master_playlist_ref
                .variants
                .iter()
                .find(|v| v.video.is_some() && v.audio.is_none())
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: "No VideoOnly variant".to_string(),
                })?,
            HlsVariantSelectionPolicy::MatchingResolution { width, height } => master_playlist_ref
                .variants
                .iter()
                .find(|v| {
                    v.resolution
                        .is_some_and(|r| r.width == (*width as u64) && r.height == (*height as u64))
                })
                .ok_or_else(|| HlsDownloaderError::Playlist {
                    reason: format!("No variant for resolution {width}x{height}"),
                })?,
            HlsVariantSelectionPolicy::Custom(name) => {
                warn!("Custom policy '{name}' selected; falling back to first variant.");
                master_playlist_ref.variants.first().ok_or_else(|| {
                    HlsDownloaderError::Playlist {
                        reason: "No variants for Custom policy".to_string(),
                    }
                })?
            }
        };
        let master_playlist_url =
            Url::parse(master_base_url_str).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Invalid master base URL {master_base_url_str}: {e}"),
            })?;
        let media_playlist_url = master_playlist_url
            .join(&selected_variant.uri)
            .map_err(|e| HlsDownloaderError::Playlist {
                reason: format!(
                    "Could not join master URL with variant URI {}: {e}",
                    selected_variant.uri
                ),
            })?;

        debug!("Selected media playlist URL: {media_playlist_url}");
        let client = self.clients.client_for_url(&media_playlist_url);
        let response = client
            .get(media_playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(|e| HlsDownloaderError::Network { source: e })?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch media playlist {media_playlist_url}: HTTP {}",
                    response.status()
                ),
            });
        }
        let playlist_bytes = response
            .bytes()
            .await
            .map_err(|e| HlsDownloaderError::Network { source: e })?;
        let playlist_content = std::str::from_utf8(playlist_bytes.as_ref()).map_err(|e| {
            HlsDownloaderError::Playlist {
                reason: format!("Media playlist not UTF-8: {e}"),
            }
        })?;
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(media_playlist_url.as_str()) {
                let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                Cow::Owned(preprocessed.into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes.as_ref())
            };
        let base_url_obj =
            media_playlist_url
                .join(".")
                .map_err(|e| HlsDownloaderError::Playlist {
                    reason: format!("Bad base URL for media playlist: {e}"),
                })?;
        let media_base_url = base_url_obj.to_string();
        debug!(
            "Derived base URL from media playlist: {} -> {}",
            media_playlist_url, media_base_url
        );
        match parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MediaPlaylist(pl)) => Ok(MediaPlaylistDetails {
                playlist: pl,
                url: media_playlist_url.to_string(),
                base_url: media_base_url,
            }),
            Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => Err(HlsDownloaderError::Playlist {
                reason: "Expected Media Playlist, got Master".to_string(),
            }),
            Err(e) => Err(HlsDownloaderError::Playlist {
                reason: format!("Failed to parse media playlist: {e}"),
            }),
        }
    }

    async fn monitor_media_playlist(
        &self,
        playlist_url_str: &str,
        mut current_playlist: MediaPlaylist,
        base_url: String,
        channels: PlaylistEngineChannels,
        token: CancellationToken,
    ) -> Result<(), HlsDownloaderError> {
        let PlaylistEngineChannels {
            segment_request_tx,
            mut segment_outcome_rx,
            client_event_tx,
        } = channels;

        let playlist_url =
            Url::parse(playlist_url_str).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Invalid playlist URL for monitoring {playlist_url_str}: {e}"),
            })?;

        let mut last_map_uri: Option<String> = None;
        let mut segment_parse_context = SegmentParseContext::default();
        let mut retries = 0;
        let mut last_playlist_bytes: Option<bytes::Bytes> = None;

        let mut twitch_processor = if base_url.contains("ttvnw.net") {
            Some(TwitchPlaylistProcessor::new())
        } else {
            None
        };

        let mut segment_lifecycle = SegmentLifecycleRegistry::new(SegmentLifecycleConfig {
            max_entries: self.config.playlist_config.segment_lifecycle_max_entries,
            retry_delay: self.config.fetcher_config.segment_retry_delay_base,
            max_reschedules: self.config.fetcher_config.max_segment_retries,
        });

        // Adaptive refresh tracking
        let mut adaptive_tracker = AdaptiveRefreshTracker::new(
            self.config.playlist_config.adaptive_refresh_enabled,
            self.config.playlist_config.adaptive_refresh_min_interval,
            self.config.playlist_config.adaptive_refresh_max_interval,
        );

        loop {
            Self::drain_segment_outcomes(&mut segment_outcome_rx, &mut segment_lifecycle);
            match self
                .fetch_and_parse_playlist(&playlist_url, &last_playlist_bytes, &token)
                .await
            {
                Ok(Some((new_playlist, new_playlist_bytes))) => {
                    retries = 0;
                    let jobs = self
                        .process_segments(
                            &new_playlist,
                            &base_url,
                            SegmentProcessingContext {
                                lifecycle: &mut segment_lifecycle,
                                last_map_uri: &mut last_map_uri,
                                parse: &mut segment_parse_context,
                                twitch_processor: &mut twitch_processor,
                            },
                            playlist_url.query(),
                        )
                        .await?;

                    // Update adaptive tracker with segment arrival info
                    let new_segments_count = jobs.len();
                    adaptive_tracker.record_refresh(new_segments_count);

                    self.send_jobs(jobs, &segment_request_tx, playlist_url_str)
                        .await?;

                    current_playlist = new_playlist;
                    last_playlist_bytes = Some(new_playlist_bytes);
                    segment_lifecycle.prune_before_msn(current_playlist.media_sequence);

                    if current_playlist.end_list {
                        info!("Playlist monitoring finished (ENDLIST): {playlist_url}.");
                        // Notify the output side that this exit is authoritative
                        // (the upstream playlist explicitly ended) so the
                        // wrapper can promote the terminal event to
                        // `EngineEndSignal::HlsEndlist`. Send-failure is
                        // non-fatal: if the receiver is already gone the
                        // stream is being torn down anyway.
                        if let Err(e) = client_event_tx
                            .send(Ok(HlsStreamEvent::EndlistEncountered))
                            .await
                        {
                            debug!("Failed to send EndlistEncountered event: {e}");
                        }
                        return Ok(());
                    }
                }
                Ok(None) => {
                    // Playlist unchanged, just wait for next refresh
                    retries = 0;
                    adaptive_tracker.record_refresh(0); // No new segments
                    if segment_lifecycle.has_due_retry(Instant::now()) {
                        let jobs = self
                            .process_segments(
                                &current_playlist,
                                &base_url,
                                SegmentProcessingContext {
                                    lifecycle: &mut segment_lifecycle,
                                    last_map_uri: &mut last_map_uri,
                                    parse: &mut segment_parse_context,
                                    twitch_processor: &mut twitch_processor,
                                },
                                playlist_url.query(),
                            )
                            .await?;
                        adaptive_tracker.record_refresh(jobs.len());
                        self.send_jobs(jobs, &segment_request_tx, playlist_url_str)
                            .await?;
                    }
                }
                Err(e) => {
                    error!("Error refreshing playlist {playlist_url}: {e}");
                    retries += 1;
                    if retries > self.config.playlist_config.live_max_refresh_retries {
                        return Err(e);
                    }
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => {
                            info!("Playlist monitoring cancelled during retry backoff: {}.", playlist_url_str);
                            return Ok(());
                        }
                        _ = tokio::time::sleep(
                            self.config.playlist_config.live_refresh_retry_delay * retries,
                        ) => {}
                    }
                }
            }

            // Calculate refresh delay - use adaptive if enabled, otherwise use target_duration/2
            let base_refresh_interval =
                Duration::from_secs_f64(current_playlist.target_duration as f64 * 0.5)
                    .max(self.config.playlist_config.live_refresh_interval);
            let refresh_delay = segment_lifecycle
                .time_until_next_retry(Instant::now())
                .map(|retry_delay| {
                    retry_delay.min(adaptive_tracker.get_refresh_interval(base_refresh_interval))
                })
                .unwrap_or_else(|| adaptive_tracker.get_refresh_interval(base_refresh_interval));

            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    info!("Playlist monitoring cancelled: {}.", playlist_url_str);
                    return Ok(());
                }
                _ = tokio::time::sleep(refresh_delay) => {
                    // Time to refresh
                }
            }
        }
    }
}

impl PlaylistEngine {
    pub fn new(
        clients: Arc<ClientPool>,
        cache_service: Option<Arc<CacheManager>>,
        config: Arc<HlsConfig>,
    ) -> Self {
        Self {
            clients,
            cache_service,
            config,
        }
    }

    fn drain_segment_outcomes(
        segment_outcome_rx: &mut mpsc::UnboundedReceiver<SegmentJobOutcome>,
        segment_lifecycle: &mut SegmentLifecycleRegistry,
    ) {
        while let Ok(outcome) = segment_outcome_rx.try_recv() {
            segment_lifecycle.apply_outcome(outcome, Instant::now());
        }
    }

    fn parse_playlist_level_map(playlist: &MediaPlaylist) -> Option<m3u8_rs::Map> {
        let ext = playlist
            .unknown_tags
            .iter()
            .rev()
            .find(|t| t.tag == "X-MAP")?;
        let rest = ext.rest.as_deref()?;

        let mut uri: Option<String> = None;
        let mut byte_range: Option<m3u8_rs::ByteRange> = None;

        // Split on commas, but keep quoted values intact.
        let mut parts: Vec<&str> = Vec::new();
        let mut in_quotes = false;
        let mut start = 0usize;
        for (idx, ch) in rest.char_indices() {
            match ch {
                '"' => in_quotes = !in_quotes,
                ',' if !in_quotes => {
                    parts.push(rest[start..idx].trim());
                    start = idx + 1;
                }
                _ => {}
            }
        }
        if start < rest.len() {
            parts.push(rest[start..].trim());
        }

        for part in parts.into_iter().filter(|p| !p.is_empty()) {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            let key = k.trim();
            let mut val = v.trim();
            if let Some(stripped) = val.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                val = stripped;
            }

            if key.eq_ignore_ascii_case("URI") {
                uri = Some(val.to_string());
            } else if key.eq_ignore_ascii_case("BYTERANGE") {
                let (len_str, offset_str) = val.split_once('@').unwrap_or((val, ""));
                if let Ok(length) = len_str.trim().parse::<u64>() {
                    let offset = if offset_str.trim().is_empty() {
                        None
                    } else {
                        offset_str.trim().parse::<u64>().ok()
                    };
                    byte_range = Some(m3u8_rs::ByteRange { length, offset });
                }
            }
        }

        let uri = uri?;
        Some(m3u8_rs::Map {
            uri,
            byte_range,
            other_attributes: HashMap::new(),
        })
    }

    /// Transforms Twitch-specific tags into m3u8-rs compatible ones.
    ///
    /// - Keeps `#EXT-X-DATERANGE` tags so we can detect stitched ads (Streamlink logic)
    /// - Transforms `#EXT-X-TWITCH-PREFETCH` tags into standard segments
    fn preprocess_twitch_playlist(&self, playlist_content: &str) -> String {
        let mut out = String::with_capacity(playlist_content.len());
        for line in playlist_content.lines() {
            if let Some(prefetch_uri) = line.strip_prefix("#EXT-X-TWITCH-PREFETCH:") {
                debug!("Transformed prefetch tag to segment: {}", prefetch_uri);
                // The duration is not provided. Use a placeholder and let the Twitch
                // processor handle ad detection / time extrapolation.
                out.push_str("#EXTINF:2.002,PREFETCH_SEGMENT\n");
                out.push_str(prefetch_uri);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Fetches and parses a refreshed media playlist.
    async fn fetch_and_parse_playlist(
        &self,
        playlist_url: &Url,
        last_playlist_bytes: &Option<bytes::Bytes>,
        token: &CancellationToken,
    ) -> Result<Option<(MediaPlaylist, bytes::Bytes)>, HlsDownloaderError> {
        if token.is_cancelled() {
            return Err(HlsDownloaderError::Cancelled);
        }

        let client = self.clients.client_for_url(playlist_url);
        let response = client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params);

        let response = tokio::select! {
            _ = token.cancelled() => {
                return Err(HlsDownloaderError::Cancelled);
            }
            response = response.send() => response,
        }
        .map_err(|e| HlsDownloaderError::Network { source: e })?;

        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch playlist {playlist_url}: HTTP {}",
                    response.status()
                ),
            });
        }

        let playlist_bytes = tokio::select! {
            _ = token.cancelled() => {
                return Err(HlsDownloaderError::Cancelled);
            }
            bytes = response.bytes() => bytes,
        }
        .map_err(|e| HlsDownloaderError::Network { source: e })?;

        // Fast path: check if we have a previous playlist and if lengths differ
        if let Some(last_bytes) = last_playlist_bytes.as_ref()
            && last_bytes.len() == playlist_bytes.len()
        {
            // Same length, do full byte comparison
            if last_bytes == &playlist_bytes {
                // debug!(
                //     "Playlist content for {} has not changed. Skipping parsing.",
                //     playlist_url
                // );
                return Ok(None);
            }
        }

        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                let playlist_content = String::from_utf8_lossy(&playlist_bytes);
                let preprocessed = self.preprocess_twitch_playlist(&playlist_content);
                Cow::Owned(preprocessed.into_bytes())
            } else {
                Cow::Borrowed(&playlist_bytes)
            };

        match parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MediaPlaylist(new_mp)) => Ok(Some((new_mp, playlist_bytes))),
            Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => Err(HlsDownloaderError::Playlist {
                reason: format!("Expected Media Playlist, got Master for {playlist_url}"),
            }),
            Err(e) => Err(HlsDownloaderError::Playlist {
                reason: format!("Failed to parse refreshed playlist {playlist_url}: {e}"),
            }),
        }
    }

    /// Processes the segments of a new playlist to identify new ones and create jobs.
    async fn process_segments(
        &self,
        new_playlist: &MediaPlaylist,
        base_url: &str,
        processing_context: SegmentProcessingContext<'_>,
        parent_query: Option<&str>,
    ) -> Result<Vec<ScheduledSegmentJob>, HlsDownloaderError> {
        let SegmentProcessingContext {
            lifecycle: segment_lifecycle,
            last_map_uri,
            parse: parse_context,
            twitch_processor,
        } = processing_context;

        let mut jobs_to_send = Vec::new();
        let base_url_parsed = Url::parse(base_url).ok();
        let base_url_arc: Arc<str> = Arc::from(base_url);
        let playlist_level_map = Self::parse_playlist_level_map(new_playlist);

        // Helper to merge query params from parent if missing in child
        let parent_params: Vec<(String, String)> = parent_query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect()
            })
            .unwrap_or_default();

        let merge_params = |uri_str: &str| -> String {
            if parent_params.is_empty() {
                return uri_str.to_string();
            }

            if let Ok(mut url) = Url::parse(uri_str) {
                let original = url.to_string();
                for (k, v) in &parent_params {
                    if url
                        .query_pairs()
                        .any(|(existing_k, _)| existing_k == k.as_str())
                    {
                        continue;
                    }
                    url.query_pairs_mut().append_pair(k, v);
                }
                let merged = url.to_string();
                if original != merged {
                    trace!("Merged query params: {} -> {}", original, merged);
                }
                return merged;
            }
            uri_str.to_string()
        };

        let resolve_uri = |relative_uri: &str| -> Result<String, url::ParseError> {
            let resolved = if let Some(base) = base_url_parsed.as_ref() {
                base.join(relative_uri).map(|u| u.to_string())
            } else {
                Url::parse(base_url)
                    .and_then(|b| b.join(relative_uri))
                    .map(|u| u.to_string())
            };
            if let Ok(ref url) = resolved {
                trace!("Resolved URI: {} + {} -> {}", base_url, relative_uri, url);
            }
            resolved
        };

        macro_rules! handle_segment {
            ($idx:expr, $segment:expr, $is_ad:expr, $discontinuity:expr) => {{
                let idx: usize = $idx;
                let segment: &MediaSegment = $segment;
                let is_ad: bool = $is_ad;
                let discontinuity: bool = $discontinuity;
                let msn = new_playlist.media_sequence + idx as u64;

                let resolved_key = segment.key.as_ref().map(|key| {
                    let mut key = key.clone();
                    if let Some(uri) = key.uri.as_deref() {
                        let absolute_key_uri =
                            if uri.starts_with("http://") || uri.starts_with("https://") {
                                uri.to_string()
                            } else {
                                resolve_uri(uri).unwrap_or_else(|_| uri.to_string())
                            };

                        key.uri = Some(merge_params(&absolute_key_uri));
                    }
                    key
                });

                // m3u8-rs only attaches EXT-X-MAP to `MediaSegment.map` when it appears in the
                // segment-scoped tag region. If it appears before the first segment, it lands in
                // `MediaPlaylist.unknown_tags` as an `ExtTag` ("X-MAP").
                if let Some(map_info) = segment.map.as_ref().or(playlist_level_map.as_ref()) {
                    let absolute_map_uri = resolve_uri(&map_info.uri).unwrap_or_else(|_| {
                        error!(
                            "Failed to resolve map URI '{}' with base '{}'",
                            map_info.uri, base_url
                        );
                        map_info.uri.clone()
                    });

                    let final_map_uri = merge_params(&absolute_map_uri);

                    if last_map_uri.as_ref() != Some(&final_map_uri) {
                        debug!("New init segment detected: {}", final_map_uri);
                        *last_map_uri = Some(final_map_uri.clone());
                    }

                    let identity = Arc::<str>::from(format!("init:{final_map_uri}"));
                    let init_media_segment = MediaSegment {
                        uri: final_map_uri.clone(),
                        duration: 0.0,
                        byte_range: map_info.byte_range.clone(),
                        discontinuity,
                        key: resolved_key.clone(),
                        map: None,
                        ..Default::default()
                    };
                    if segment_lifecycle.should_schedule(identity.as_ref(), Instant::now()) {
                        let msn = new_playlist.media_sequence + idx as u64;
                        segment_lifecycle.mark_scheduled(
                            Arc::clone(&identity),
                            msn,
                            SegmentJobKind::Init,
                        );
                        let init_job = ScheduledSegmentJob {
                            identity,
                            base_url: Arc::clone(&base_url_arc),
                            media_sequence_number: msn,
                            media_segment: Arc::new(init_media_segment),
                            kind: SegmentJobKind::Init,
                            is_init_segment: true,
                            is_prefetch: false,
                            parsed_url: Url::parse(&final_map_uri).ok().map(Arc::new),
                        };
                        jobs_to_send.push(init_job);
                    }
                }

                let effective_segment_uri = if segment.uri.trim().is_empty() {
                    if segment.byte_range.is_some() {
                        parse_context
                            .last_non_empty_segment_uri
                            .as_deref()
                            .unwrap_or("")
                    } else {
                        ""
                    }
                } else {
                    parse_context.last_non_empty_segment_uri = Some(segment.uri.clone());
                    segment.uri.as_str()
                };

                if effective_segment_uri.trim().is_empty() {
                    warn!(
                        msn = msn,
                        "Skipping segment with empty URI (may be an incomplete segment entry)",
                    );
                } else {
                    let mut should_skip = false;
                    let mut effective_byte_range: Option<m3u8_rs::ByteRange> = None;

                    if let Some(byte_range) = segment.byte_range.as_ref() {
                        let inferred_offset = byte_range.offset.or_else(|| {
                            if parse_context.last_byterange_uri.as_deref()
                                == Some(effective_segment_uri)
                            {
                                parse_context.last_byterange_end
                            } else {
                                None
                            }
                        });

                        if let Some(offset) = inferred_offset {
                            effective_byte_range = Some(m3u8_rs::ByteRange {
                                length: byte_range.length,
                                offset: Some(offset),
                            });
                            parse_context.last_byterange_uri =
                                Some(effective_segment_uri.to_string());
                            parse_context.last_byterange_end =
                                Some(offset.saturating_add(byte_range.length));
                        } else {
                            warn!(
                                msn = msn,
                                uri = %effective_segment_uri,
                                "Skipping segment with BYTERANGE missing offset and no prior range to infer from"
                            );
                            parse_context.last_byterange_uri = None;
                            parse_context.last_byterange_end = None;
                            should_skip = true;
                        }
                    } else {
                        parse_context.last_byterange_uri = None;
                        parse_context.last_byterange_end = None;
                    }

                    if !should_skip {
                        let absolute_segment_uri =
                            resolve_uri(effective_segment_uri).unwrap_or_else(|_| {
                                error!(
                                    "Failed to resolve segment URI '{}' with base '{}'",
                                    effective_segment_uri, base_url
                                );
                                effective_segment_uri.to_string()
                            });

                        let final_segment_uri = merge_params(&absolute_segment_uri);

                        let segment_identity = if let Some(br) = effective_byte_range.as_ref() {
                            let offset = br
                                .offset
                                .map(|o| o.to_string())
                                .unwrap_or_else(|| "none".to_string());
                            format!("{final_segment_uri}|br={}@{offset}", br.length)
                        } else {
                            final_segment_uri.clone()
                        };

                        if segment_lifecycle.should_schedule(&segment_identity, Instant::now()) {
                            if is_ad {
                                debug!("Skipping Twitch ad segment: {}", segment.uri);
                            } else {
                                let job_kind = if segment.title.as_deref() == Some("PREFETCH_SEGMENT") {
                                    SegmentJobKind::Prefetch
                                } else {
                                    SegmentJobKind::Media
                                };
                                let identity = Arc::<str>::from(segment_identity);
                                let mut segment_for_job = segment.clone();
                                segment_for_job.key = resolved_key.clone();
                                segment_for_job.uri = final_segment_uri.clone();
                                segment_for_job.byte_range = effective_byte_range.clone();
                                segment_for_job.discontinuity = discontinuity;
                                segment_lifecycle.mark_scheduled(
                                    Arc::clone(&identity),
                                    msn,
                                    job_kind,
                                );
                                trace!("New segment detected: {}", final_segment_uri);
                                let job = ScheduledSegmentJob {
                                    identity,
                                    base_url: Arc::clone(&base_url_arc),
                                    media_sequence_number: msn,
                                    media_segment: Arc::new(segment_for_job),
                                    kind: job_kind,
                                    is_init_segment: false,
                                    is_prefetch: job_kind == SegmentJobKind::Prefetch,
                                    parsed_url: Url::parse(&final_segment_uri).ok().map(Arc::new),
                                };
                                jobs_to_send.push(job);
                            }
                        } else {
                            trace!("Segment {} already seen, skipping.", final_segment_uri);
                        }
                    }
                }

                Ok::<(), HlsDownloaderError>(())
            }};
        }

        if let Some(processor) = twitch_processor {
            let processed_segments = processor.process_playlist(new_playlist);
            for (idx, processed_segment) in processed_segments.into_iter().enumerate() {
                handle_segment!(
                    idx,
                    processed_segment.segment,
                    processed_segment.is_ad,
                    processed_segment.discontinuity
                )?;
            }
        } else {
            for (idx, segment) in new_playlist.segments.iter().enumerate() {
                handle_segment!(idx, segment, false, segment.discontinuity)?;
            }
        }
        Ok(jobs_to_send)
    }

    /// Sends the created jobs to the segment scheduler.
    async fn send_jobs(
        &self,
        jobs: Vec<ScheduledSegmentJob>,
        segment_request_tx: &mpsc::Sender<ScheduledSegmentJob>,
        playlist_url_str: &str,
    ) -> Result<(), HlsDownloaderError> {
        if jobs.is_empty() {
            return Ok(());
        }
        for job in jobs {
            trace!("Sending segment job: {:?}", job.media_segment.uri);
            if segment_request_tx.send(job).await.is_err() {
                error!(
                    "SegmentScheduler request channel closed for {}.",
                    playlist_url_str
                );
                return Err(HlsDownloaderError::Internal {
                    reason: "SegmentScheduler request channel closed".to_string(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::config::HlsConfig;
    use std::collections::VecDeque;
    use tokio_util::sync::CancellationToken;

    fn test_engine() -> PlaylistEngine {
        let config = Arc::new(HlsConfig::default());
        let clients =
            Arc::new(crate::downloader::create_client_pool(&config.base).expect("client pool"));
        PlaylistEngine::new(clients, None, config)
    }

    fn parse_media_playlist(input: &str) -> MediaPlaylist {
        match parse_playlist_res(input.as_bytes()).expect("playlist should parse") {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            m3u8_rs::Playlist::MasterPlaylist(_) => panic!("expected media playlist"),
        }
    }

    fn test_lifecycle() -> SegmentLifecycleRegistry {
        SegmentLifecycleRegistry::new(SegmentLifecycleConfig {
            max_entries: 100,
            retry_delay: Duration::ZERO,
            max_reschedules: 3,
        })
    }

    fn test_parse_context() -> SegmentParseContext {
        SegmentParseContext::default()
    }

    #[tokio::test]
    async fn process_segments_skips_empty_uri_segment() {
        let engine = test_engine();
        let playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n\n",
        );
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;
        let jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("process_segments should succeed");
        assert!(jobs.is_empty());
    }

    #[tokio::test]
    async fn process_segments_infers_byterange_offset_and_reuses_previous_uri() {
        let engine = test_engine();
        let mut playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n",
        );
        playlist.segments.push(MediaSegment {
            uri: String::new(),
            duration: 2.0,
            byte_range: Some(m3u8_rs::ByteRange {
                length: 5,
                offset: None,
            }),
            ..Default::default()
        });
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;
        let jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("process_segments should succeed");

        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[0].media_segment.uri,
            "https://example.com/path/file.ts"
        );
        assert_eq!(
            jobs[0].media_segment.byte_range,
            Some(m3u8_rs::ByteRange {
                length: 10,
                offset: Some(0),
            })
        );
        assert_eq!(
            jobs[1].media_segment.uri,
            "https://example.com/path/file.ts"
        );
        assert_eq!(
            jobs[1].media_segment.byte_range,
            Some(m3u8_rs::ByteRange {
                length: 5,
                offset: Some(10),
            })
        );
    }

    #[tokio::test]
    async fn process_segments_preserves_byterange_context_across_refreshes() {
        let engine = test_engine();
        let first_playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n",
        );
        let mut second_playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:2\n",
        );
        second_playlist.segments.push(MediaSegment {
            uri: String::new(),
            duration: 2.0,
            byte_range: Some(m3u8_rs::ByteRange {
                length: 5,
                offset: None,
            }),
            ..Default::default()
        });

        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;

        let first_jobs = engine
            .process_segments(
                &first_playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("first process_segments should succeed");
        assert_eq!(first_jobs.len(), 1);

        let second_jobs = engine
            .process_segments(
                &second_playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("second process_segments should succeed");

        assert_eq!(second_jobs.len(), 1);
        assert_eq!(
            second_jobs[0].media_segment.uri,
            "https://example.com/path/file.ts"
        );
        assert_eq!(
            second_jobs[0].media_segment.byte_range,
            Some(m3u8_rs::ByteRange {
                length: 5,
                offset: Some(10),
            })
        );
    }

    #[tokio::test]
    async fn process_segments_does_not_duplicate_in_flight_segment() {
        let engine = test_engine();
        let playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\nseg10.ts\n",
        );
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;

        let first_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("first process_segments should succeed");
        assert_eq!(first_jobs.len(), 1);

        let second_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("second process_segments should succeed");
        assert!(second_jobs.is_empty());
    }

    #[tokio::test]
    async fn process_segments_reschedules_retryable_failure() {
        let engine = test_engine();
        let playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\nseg10.ts\n",
        );
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;

        let first_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("first process_segments should succeed");
        assert_eq!(first_jobs.len(), 1);

        segment_lifecycle.apply_outcome(
            SegmentJobOutcome {
                identity: Arc::clone(&first_jobs[0].identity),
                media_sequence_number: first_jobs[0].media_sequence_number,
                kind: first_jobs[0].kind,
                result: crate::hls::segment_lifecycle::SegmentJobResult::Failed {
                    retryable: true,
                    reason: "404".to_string(),
                },
            },
            Instant::now(),
        );

        let retry_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("retry process_segments should succeed");
        assert_eq!(retry_jobs.len(), 1);
        assert_eq!(retry_jobs[0].identity, first_jobs[0].identity);
    }

    #[tokio::test]
    async fn process_segments_does_not_reschedule_terminal_failure() {
        let engine = test_engine();
        let playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\nseg10.ts\n",
        );
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;

        let first_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("first process_segments should succeed");
        assert_eq!(first_jobs.len(), 1);

        segment_lifecycle.apply_outcome(
            SegmentJobOutcome {
                identity: Arc::clone(&first_jobs[0].identity),
                media_sequence_number: first_jobs[0].media_sequence_number,
                kind: first_jobs[0].kind,
                result: crate::hls::segment_lifecycle::SegmentJobResult::Failed {
                    retryable: false,
                    reason: "403".to_string(),
                },
            },
            Instant::now(),
        );

        let retry_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("retry process_segments should succeed");
        assert!(retry_jobs.is_empty());
    }

    #[tokio::test]
    async fn process_segments_reschedules_retryable_init_segment_failure() {
        let engine = test_engine();
        let playlist = parse_media_playlist(
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nseg10.m4s\n",
        );
        let mut segment_lifecycle = test_lifecycle();
        let mut last_map_uri = None;
        let mut parse_context = test_parse_context();
        let mut twitch_processor = None;

        let first_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("first process_segments should succeed");
        let init_job = first_jobs
            .iter()
            .find(|job| job.is_init_segment)
            .expect("init job should be scheduled");

        segment_lifecycle.apply_outcome(
            SegmentJobOutcome {
                identity: Arc::clone(&init_job.identity),
                media_sequence_number: init_job.media_sequence_number,
                kind: init_job.kind,
                result: crate::hls::segment_lifecycle::SegmentJobResult::Failed {
                    retryable: true,
                    reason: "404".to_string(),
                },
            },
            Instant::now(),
        );

        let retry_jobs = engine
            .process_segments(
                &playlist,
                "https://example.com/path/",
                SegmentProcessingContext {
                    lifecycle: &mut segment_lifecycle,
                    last_map_uri: &mut last_map_uri,
                    parse: &mut parse_context,
                    twitch_processor: &mut twitch_processor,
                },
                None,
            )
            .await
            .expect("retry process_segments should succeed");
        assert!(retry_jobs.iter().any(|job| job.is_init_segment));
    }

    #[test]
    fn preprocess_twitch_playlist_keeps_daterange_and_transforms_prefetch() {
        let engine = test_engine();

        let input = "#EXTM3U\n\
#EXT-X-DATERANGE:ID=\"stitched-ad-1\",CLASS=\"twitch-stitched-ad\",START-DATE=\"2026-01-01T00:00:02Z\",DURATION=4.0\n\
#EXT-X-TWITCH-PREFETCH:https://example.com/prefetch.ts\n";

        let out = engine.preprocess_twitch_playlist(input);

        assert!(out.contains("#EXT-X-DATERANGE:ID=\"stitched-ad-1\""));
        assert!(!out.contains("#EXT-X-TWITCH-PREFETCH:"));
        assert!(out.contains("PREFETCH_SEGMENT"));
        assert!(out.contains("https://example.com/prefetch.ts"));
    }

    #[test]
    fn adaptive_refresh_backoff_respects_min_interval() {
        let mut tracker = AdaptiveRefreshTracker {
            enabled: true,
            min_interval: Duration::from_millis(500),
            max_interval: Duration::from_secs(3),
            recent_results: VecDeque::new(),
            consecutive_empty: 3,
            last_new_segments_count: 0,
        };

        // Simulate tiny default interval (e.g., user configured very small live_refresh_interval).
        let interval = tracker.get_refresh_interval(Duration::from_millis(100));
        assert!(interval >= Duration::from_millis(500));

        // Ensure we still clamp to max.
        tracker.consecutive_empty = 10;
        let interval = tracker.get_refresh_interval(Duration::from_secs(10));
        assert!(interval <= Duration::from_secs(3));
    }

    #[test]
    fn adaptive_refresh_success_path_respects_max_interval() {
        let mut tracker =
            AdaptiveRefreshTracker::new(true, Duration::from_millis(500), Duration::from_secs(3));

        for _ in 0..10 {
            tracker.record_refresh(1);
        }

        // Even if the default interval is large, adaptive refresh should still clamp to max.
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

    #[tokio::test]
    async fn fetch_and_parse_playlist_returns_cancelled_when_token_cancelled() {
        let engine = test_engine();
        let url = Url::parse("https://example.com/playlist.m3u8").expect("valid url");
        let token = CancellationToken::new();
        token.cancel();

        let res = engine.fetch_and_parse_playlist(&url, &None, &token).await;

        assert!(matches!(res, Err(HlsDownloaderError::Cancelled)));
    }
}
