// HLS Playlist Engine: Handles fetching, parsing, and managing HLS playlists.

use crate::cache::{CacheKey, CacheManager, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::scheduler::ScheduledSegmentJob;
use crate::hls::twitch_processor::TwitchPlaylistProcessor;
use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, MediaPlaylist, MediaSegment, parse_playlist_res};
use moka::future::Cache;
use moka::policy::EvictionPolicy;
use reqwest::Client;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace};
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
        segment_request_tx: mpsc::Sender<ScheduledSegmentJob>,
        token: CancellationToken,
    ) -> Result<(), HlsDownloaderError>;
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
pub enum PlaylistUpdateEvent {
    PlaylistRefreshed {
        media_sequence_base: u64,
        target_duration: u64,
    },
    PlaylistEnded,
}

pub struct PlaylistEngine {
    http_client: Client,
    cache_service: Option<Arc<CacheManager>>,
    config: Arc<HlsConfig>,
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
    /// Last time we got new segments
    last_segment_time: Option<std::time::Instant>,
}

impl AdaptiveRefreshTracker {
    fn new(enabled: bool, min_interval: Duration, max_interval: Duration) -> Self {
        Self {
            enabled,
            min_interval,
            max_interval,
            recent_results: std::collections::VecDeque::with_capacity(10),
            consecutive_empty: 0,
            last_segment_time: None,
        }
    }

    /// Record the result of a playlist refresh
    fn record_refresh(&mut self, new_segments_count: usize) {
        let got_segments = new_segments_count > 0;

        // Track recent results (keep last 10)
        if self.recent_results.len() >= 10 {
            self.recent_results.pop_front();
        }
        self.recent_results.push_back(got_segments);

        if got_segments {
            self.consecutive_empty = 0;
            self.last_segment_time = Some(std::time::Instant::now());
        } else {
            self.consecutive_empty += 1;
        }
    }

    /// Get the recommended refresh interval based on recent patterns
    fn get_refresh_interval(&self, default_interval: Duration) -> Duration {
        if !self.enabled {
            return default_interval;
        }

        // If we've had multiple consecutive empty refreshes, back off
        if self.consecutive_empty >= 3 {
            // Exponential backoff, capped at max_interval
            let backoff_factor = 1.5_f64.powi(self.consecutive_empty.min(5) as i32);
            let backed_off =
                Duration::from_secs_f64(default_interval.as_secs_f64() * backoff_factor);
            return backed_off.min(self.max_interval);
        }

        // If we're consistently getting segments, we can be more aggressive
        let recent_success_rate = self.recent_results.iter().filter(|&&got| got).count() as f64
            / self.recent_results.len().max(1) as f64;

        if recent_success_rate > 0.8 && self.recent_results.len() >= 5 {
            // High success rate - can poll slightly faster
            let faster = Duration::from_secs_f64(default_interval.as_secs_f64() * 0.8);
            return faster.max(self.min_interval);
        }

        // Default behavior
        default_interval
            .max(self.min_interval)
            .min(self.max_interval)
    }
}

#[async_trait]
impl PlaylistProvider for PlaylistEngine {
    async fn load_initial_playlist(
        &self,
        url_str: &str,
    ) -> Result<InitialPlaylist, HlsDownloaderError> {
        let playlist_url = Url::parse(url_str).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Invalid playlist URL {url_str}: {e}"))
        })?;
        let cache_key = CacheKey::new(CacheResourceType::Playlist, playlist_url.as_str(), None);

        if let Some(cache_service) = &self.cache_service
            && let Ok(Some((cached_data, _, _))) = cache_service.get(&cache_key).await
        {
            let playlist_content = std::str::from_utf8(cached_data.as_ref()).map_err(|e| {
                HlsDownloaderError::PlaylistError(format!(
                    "Failed to parse cached playlist from UTF-8: {e}"
                ))
            })?;
            let playlist_bytes_to_parse: Cow<[u8]> =
                if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                    let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                    Cow::Owned(preprocessed.into_bytes())
                } else {
                    Cow::Borrowed(cached_data.as_ref())
                };
            let base_url_obj = playlist_url.join(".").map_err(|e| {
                HlsDownloaderError::PlaylistError(format!("Failed to determine base URL: {e}"))
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
                Err(e) => Err(HlsDownloaderError::PlaylistError(format!(
                    "Failed to parse cached playlist: {e}"
                ))),
            };
        }

        let response = self
            .http_client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(|e| HlsDownloaderError::NetworkError {
                source: Arc::new(e),
            })?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::PlaylistError(format!(
                "Failed to fetch playlist {playlist_url}: HTTP {}",
                response.status()
            )));
        }
        let playlist_bytes =
            response
                .bytes()
                .await
                .map_err(|e| HlsDownloaderError::NetworkError {
                    source: Arc::new(e),
                })?;

        if let Some(cache_service) = &self.cache_service {
            let metadata = CacheMetadata::new(playlist_bytes.len() as u64)
                .with_expiration(self.config.playlist_config.initial_playlist_fetch_timeout);

            cache_service
                .put(cache_key, playlist_bytes.clone(), metadata)
                .await?;
        }
        let playlist_content = std::str::from_utf8(playlist_bytes.as_ref()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Playlist content is not valid UTF-8: {e}"))
        })?;
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                Cow::Owned(preprocessed.into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes.as_ref())
            };
        let base_url_obj = playlist_url.join(".").map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Failed to determine base URL: {e}"))
        })?;
        let base_url = base_url_obj.to_string();
        debug!(
            "Derived base URL from playlist: {} -> {}",
            playlist_url, base_url
        );
        match parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MasterPlaylist(pl)) => Ok(InitialPlaylist::Master(pl, base_url)),
            Ok(m3u8_rs::Playlist::MediaPlaylist(pl)) => Ok(InitialPlaylist::Media(pl, base_url)),
            Err(e) => Err(HlsDownloaderError::PlaylistError(format!(
                "Failed to parse fetched playlist: {e}"
            ))),
        }
    }

    async fn select_media_playlist(
        &self,
        initial_playlist_with_base_url: &InitialPlaylist,
        policy: &HlsVariantSelectionPolicy,
    ) -> Result<MediaPlaylistDetails, HlsDownloaderError> {
        let (master_playlist_ref, master_base_url_str) =
            match initial_playlist_with_base_url {
                InitialPlaylist::Master(pl, base) => (pl, base),
                InitialPlaylist::Media(_, _) => return Err(HlsDownloaderError::PlaylistError(
                    "select_media_playlist called with a MediaPlaylist, expected MasterPlaylist"
                        .to_string(),
                )),
            };
        if master_playlist_ref.variants.is_empty() {
            return Err(HlsDownloaderError::PlaylistError(
                "Master playlist has no variants".to_string(),
            ));
        }
        let selected_variant = match policy {
            HlsVariantSelectionPolicy::HighestBitrate => master_playlist_ref
                .variants
                .iter()
                .max_by_key(|v| v.bandwidth)
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError("No variants for HighestBitrate".to_string())
                })?,
            HlsVariantSelectionPolicy::LowestBitrate => master_playlist_ref
                .variants
                .iter()
                .min_by_key(|v| v.bandwidth)
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError("No variants for LowestBitrate".to_string())
                })?,
            HlsVariantSelectionPolicy::ClosestToBitrate(target_bw) => master_playlist_ref
                .variants
                .iter()
                .min_by_key(|v| (*target_bw as i64 - v.bandwidth as i64).abs())
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError(format!(
                        "No variants for ClosestToBitrate: {target_bw}"
                    ))
                })?,
            HlsVariantSelectionPolicy::AudioOnly => master_playlist_ref
                .variants
                .iter()
                .find(|v| {
                    v.audio.is_some()
                        && v.video.is_none()
                        && v.codecs.as_ref().is_some_and(|c| c.contains("mp4a"))
                })
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError("No AudioOnly variant".to_string())
                })?,
            HlsVariantSelectionPolicy::VideoOnly => master_playlist_ref
                .variants
                .iter()
                .find(|v| v.video.is_some() && v.audio.is_none())
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError("No VideoOnly variant".to_string())
                })?,
            HlsVariantSelectionPolicy::MatchingResolution { width, height } => master_playlist_ref
                .variants
                .iter()
                .find(|v| {
                    v.resolution
                        .is_some_and(|r| r.width == (*width as u64) && r.height == (*height as u64))
                })
                .ok_or_else(|| {
                    HlsDownloaderError::PlaylistError(format!(
                        "No variant for resolution {width}x{height}"
                    ))
                })?,
            HlsVariantSelectionPolicy::Custom(name) => {
                error!("Warning: Custom policy '{name}' selecting first variant.");
                master_playlist_ref.variants.first().ok_or_else(|| {
                    HlsDownloaderError::PlaylistError("No variants for Custom policy".to_string())
                })?
            }
        };
        let master_playlist_url = Url::parse(master_base_url_str).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!(
                "Invalid master base URL {master_base_url_str}: {e}"
            ))
        })?;
        let media_playlist_url = master_playlist_url
            .join(&selected_variant.uri)
            .map_err(|e| {
                HlsDownloaderError::PlaylistError(format!(
                    "Could not join master URL with variant URI {}: {e}",
                    selected_variant.uri
                ))
            })?;

        debug!("Selected media playlist URL: {media_playlist_url}");
        let response = self
            .http_client
            .get(media_playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(|e| HlsDownloaderError::NetworkError {
                source: Arc::new(e),
            })?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::PlaylistError(format!(
                "Failed to fetch media playlist {media_playlist_url}: HTTP {}",
                response.status()
            )));
        }
        let playlist_bytes =
            response
                .bytes()
                .await
                .map_err(|e| HlsDownloaderError::NetworkError {
                    source: Arc::new(e),
                })?;
        let playlist_content = std::str::from_utf8(playlist_bytes.as_ref()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Media playlist not UTF-8: {e}"))
        })?;
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(media_playlist_url.as_str()) {
                let preprocessed = self.preprocess_twitch_playlist(playlist_content);
                Cow::Owned(preprocessed.into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes.as_ref())
            };
        let base_url_obj = media_playlist_url.join(".").map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Bad base URL for media playlist: {e}"))
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
            Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => Err(HlsDownloaderError::PlaylistError(
                "Expected Media Playlist, got Master".to_string(),
            )),
            Err(e) => Err(HlsDownloaderError::PlaylistError(format!(
                "Failed to parse media playlist: {e}",
            ))),
        }
    }

    async fn monitor_media_playlist(
        &self,
        playlist_url_str: &str,
        mut current_playlist: MediaPlaylist,
        base_url: String,
        segment_request_tx: mpsc::Sender<ScheduledSegmentJob>,
        token: CancellationToken,
    ) -> Result<(), HlsDownloaderError> {
        let playlist_url = Url::parse(playlist_url_str).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!(
                "Invalid playlist URL for monitoring {playlist_url_str}: {e}"
            ))
        })?;

        let mut last_map_uri: Option<String> = None;
        let mut retries = 0;
        let mut last_playlist_bytes: Option<bytes::Bytes> = None;

        let mut twitch_processor = if base_url.contains("ttvnw.net") {
            Some(TwitchPlaylistProcessor::new())
        } else {
            None
        };

        const SEEN_SEGMENTS_LRU_CAPACITY: usize = 100;
        let seen_segment_uris: Cache<String, ()> = Cache::builder()
            .max_capacity(SEEN_SEGMENTS_LRU_CAPACITY as u64)
            .eviction_policy(EvictionPolicy::lru())
            .build();

        // Adaptive refresh tracking
        let mut adaptive_tracker = AdaptiveRefreshTracker::new(
            self.config.playlist_config.adaptive_refresh_enabled,
            self.config.playlist_config.adaptive_refresh_min_interval,
            self.config.playlist_config.adaptive_refresh_max_interval,
        );

        loop {
            match self
                .fetch_and_parse_playlist(&playlist_url, &last_playlist_bytes)
                .await
            {
                Ok(Some((new_playlist, new_playlist_bytes))) => {
                    retries = 0;
                    let jobs = self
                        .process_segments(
                            &new_playlist,
                            &base_url,
                            &seen_segment_uris,
                            &mut last_map_uri,
                            &mut twitch_processor,
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

                    if current_playlist.end_list {
                        info!("ENDLIST for {playlist_url}. Stopping monitoring.");
                        return Ok(());
                    }
                }
                Ok(None) => {
                    // Playlist unchanged or parse error, just wait for next refresh
                    retries = 0;
                    adaptive_tracker.record_refresh(0); // No new segments
                }
                Err(e) => {
                    error!("Error refreshing playlist {playlist_url}: {e}");
                    retries += 1;
                    if retries > self.config.playlist_config.live_max_refresh_retries {
                        return Err(e);
                    }
                    tokio::time::sleep(
                        self.config.playlist_config.live_refresh_retry_delay * retries,
                    )
                    .await;
                }
            }

            // Calculate refresh delay - use adaptive if enabled, otherwise use target_duration/2
            let refresh_delay = adaptive_tracker.get_refresh_interval(
                Duration::from_secs(current_playlist.target_duration / 2)
                    .max(self.config.playlist_config.live_refresh_interval),
            );

            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    info!("Cancellation token received during monitoring for {}.", playlist_url_str);
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
        http_client: Client,
        cache_service: Option<Arc<CacheManager>>,
        config: Arc<HlsConfig>,
    ) -> Self {
        Self {
            http_client,
            cache_service,
            config,
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

    /// Removes Twitch ad-related EXT-X-DATERANGE tags from the playlist and transforms
    /// EXT-X-TWITCH-PREFETCH tags into standard segments.
    fn preprocess_twitch_playlist(&self, playlist_content: &str) -> String {
        let mut out = String::with_capacity(playlist_content.len());
        for line in playlist_content.lines() {
            if line.starts_with("#EXT-X-DATERANGE")
                && (line.contains("twitch-stitched-ad") || line.contains("stitched-ad-"))
            {
                // skip ad tag
            } else if let Some(prefetch_uri) = line.strip_prefix("#EXT-X-TWITCH-PREFETCH:") {
                debug!("Transformed prefetch tag to segment: {}", prefetch_uri);
                // The duration is not provided, so we use a common value.
                // The title is used as a heuristic to identify the segment as an ad later.
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
    ) -> Result<Option<(MediaPlaylist, bytes::Bytes)>, HlsDownloaderError> {
        let response = self
            .http_client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(|e| HlsDownloaderError::NetworkError {
                source: Arc::new(e),
            })?;

        if !response.status().is_success() {
            return Err(HlsDownloaderError::PlaylistError(format!(
                "Failed to fetch playlist {playlist_url}: HTTP {}",
                response.status()
            )));
        }

        let playlist_bytes =
            response
                .bytes()
                .await
                .map_err(|e| HlsDownloaderError::NetworkError {
                    source: Arc::new(e),
                })?;

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
            Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => Err(HlsDownloaderError::PlaylistError(
                format!("Expected Media Playlist, got Master for {playlist_url}"),
            )),
            Err(e) => {
                error!("Failed to parse refreshed playlist {playlist_url}: {e}");
                Ok(None)
            }
        }
    }

    /// Processes the segments of a new playlist to identify new ones and create jobs.
    #[allow(clippy::too_many_arguments)]
    async fn process_segments(
        &self,
        new_playlist: &MediaPlaylist,
        base_url: &str,
        seen_segment_uris: &Cache<String, ()>,
        last_map_uri: &mut Option<String>,
        twitch_processor: &mut Option<TwitchPlaylistProcessor>,
        parent_query: Option<&str>,
    ) -> Result<Vec<ScheduledSegmentJob>, HlsDownloaderError> {
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
            ($idx:expr, $segment:expr, $is_ad:expr) => {{
                let idx: usize = $idx;
                let segment: &MediaSegment = $segment;
                let is_ad: bool = $is_ad;

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
                        let init_media_segment = MediaSegment {
                            uri: final_map_uri.clone(),
                            duration: 0.0,
                            byte_range: map_info.byte_range.clone(),
                            discontinuity: segment.discontinuity,
                            key: resolved_key.clone(),
                            map: None,
                            ..Default::default()
                        };
                        let init_job = ScheduledSegmentJob {
                            base_url: Arc::clone(&base_url_arc),
                            media_sequence_number: new_playlist.media_sequence + idx as u64,
                            media_segment: Arc::new(init_media_segment),
                            is_init_segment: true,
                            is_prefetch: false,
                        };
                        jobs_to_send.push(init_job);
                        *last_map_uri = Some(final_map_uri);
                    }
                }

                let absolute_segment_uri = resolve_uri(&segment.uri).unwrap_or_else(|_| {
                    error!(
                        "Failed to resolve segment URI '{}' with base '{}'",
                        segment.uri, base_url
                    );
                    segment.uri.clone()
                });

                let final_segment_uri = merge_params(&absolute_segment_uri);

                if !seen_segment_uris.contains_key(&final_segment_uri) {
                    if is_ad {
                        debug!("Skipping Twitch ad segment: {}", segment.uri);
                    } else {
                        let mut segment_for_job = segment.clone();
                        segment_for_job.key = resolved_key.clone();
                        segment_for_job.uri = final_segment_uri.clone();
                        seen_segment_uris
                            .insert(final_segment_uri.clone(), ())
                            .await;
                        trace!("New segment detected: {}", final_segment_uri);
                        let job = ScheduledSegmentJob {
                            base_url: Arc::clone(&base_url_arc),
                            media_sequence_number: new_playlist.media_sequence + idx as u64,
                            media_segment: Arc::new(segment_for_job),
                            is_init_segment: false,
                            is_prefetch: false,
                        };
                        jobs_to_send.push(job);
                    }
                } else {
                    trace!("Segment {} already seen, skipping.", final_segment_uri);
                }

                Ok::<(), HlsDownloaderError>(())
            }};
        }

        if let Some(processor) = twitch_processor {
            let processed_segments = processor.process_playlist(new_playlist);
            for (idx, processed_segment) in processed_segments.into_iter().enumerate() {
                handle_segment!(idx, processed_segment.segment, processed_segment.is_ad)?;
            }
        } else {
            for (idx, segment) in new_playlist.segments.iter().enumerate() {
                handle_segment!(idx, segment, false)?;
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
                return Err(HlsDownloaderError::InternalError(
                    "SegmentScheduler request channel closed".to_string(),
                ));
            }
        }
        Ok(())
    }
}
