// HLS Playlist Engine: Handles fetching, parsing, and managing HLS playlists.

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::scheduler::ScheduledSegmentJob;
use crate::hls::twitch_processor::{ProcessedSegment, TwitchPlaylistProcessor};
use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, MediaPlaylist, parse_playlist_res};
use moka::future::Cache;
use moka::policy::EvictionPolicy;
use reqwest::Client;
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
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
        mut shutdown_rx: broadcast::Receiver<()>,
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

        if let Some(cache_service) = &self.cache_service {
            if let Some(cached_data) = cache_service.get(&cache_key).await? {
                let mut playlist_content =
                    String::from_utf8(cached_data.0.to_vec()).map_err(|e| {
                        HlsDownloaderError::PlaylistError(format!(
                            "Failed to parse cached playlist from UTF-8: {e}"
                        ))
                    })?;
                if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                    playlist_content = self.preprocess_twitch_playlist(&playlist_content);
                }
                let base_url_obj = playlist_url.join(".").map_err(|e| {
                    HlsDownloaderError::PlaylistError(format!("Failed to determine base URL: {e}"))
                })?;
                let base_url = base_url_obj.to_string();
                return match parse_playlist_res(playlist_content.as_bytes()) {
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
        }
        let response = self
            .http_client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
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
        let mut playlist_content = String::from_utf8(playlist_bytes.to_vec()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Playlist content is not valid UTF-8: {e}"))
        })?;
        if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
            playlist_content = self.preprocess_twitch_playlist(&playlist_content);
        }
        let base_url_obj = playlist_url.join(".").map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Failed to determine base URL: {e}"))
        })?;
        let base_url = base_url_obj.to_string();
        match parse_playlist_res(playlist_content.as_bytes()) {
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
        let mut playlist_content = String::from_utf8(playlist_bytes.to_vec()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Media playlist not UTF-8: {e}"))
        })?;
        if TwitchPlaylistProcessor::is_twitch_playlist(media_playlist_url.as_str()) {
            playlist_content = self.preprocess_twitch_playlist(&playlist_content);
        }
        let base_url_obj = media_playlist_url.join(".").map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Bad base URL for media playlist: {e}"))
        })?;
        let media_base_url = base_url_obj.to_string();
        match parse_playlist_res(playlist_content.as_bytes()) {
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
        mut shutdown_rx: broadcast::Receiver<()>,
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

        const SEEN_SEGMENTS_LRU_CAPACITY: usize = 30;
        let seen_segment_uris: Cache<String, ()> = Cache::builder()
            .max_capacity(SEEN_SEGMENTS_LRU_CAPACITY as u64)
            .eviction_policy(EvictionPolicy::lru())
            .build();

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
                        )
                        .await?;

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

            let refresh_delay = Duration::from_secs(current_playlist.target_duration / 2)
                .max(self.config.playlist_config.live_refresh_interval);

            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received during monitoring for {}.", playlist_url_str);
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
        if let Some(last_bytes) = last_playlist_bytes.as_ref() {
            if last_bytes.len() == playlist_bytes.len() {
                // Same length, do full byte comparison
                if last_bytes == &playlist_bytes {
                    debug!(
                        "Playlist content for {} has not changed. Skipping parsing.",
                        playlist_url
                    );
                    return Ok(None);
                }
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
    ) -> Result<Vec<ScheduledSegmentJob>, HlsDownloaderError> {
        let mut jobs_to_send = Vec::new();
        let processed_segments = if let Some(processor) = twitch_processor {
            processor.process_playlist(new_playlist)
        } else {
            new_playlist
                .segments
                .iter()
                .map(|s| ProcessedSegment {
                    segment: s.clone(),
                    is_ad: false,
                })
                .collect()
        };

        for (idx, processed_segment) in processed_segments.into_iter().enumerate() {
            let segment = processed_segment.segment;
            if let Some(map_info) = &segment.map {
                let absolute_map_uri = Url::parse(base_url)
                    .and_then(|b| b.join(&map_info.uri))
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| {
                        error!(
                            "Failed to resolve map URI '{}' with base '{}'",
                            map_info.uri, base_url
                        );
                        map_info.uri.clone()
                    });

                if last_map_uri.as_ref() != Some(&absolute_map_uri) {
                    debug!("New init segment detected: {}", absolute_map_uri);
                    let init_job = ScheduledSegmentJob {
                        segment_uri: absolute_map_uri.clone(),
                        base_url: base_url.to_string(),
                        media_sequence_number: new_playlist.media_sequence + idx as u64,
                        duration: 0.0,
                        key: segment.key.clone(),
                        byte_range: map_info.byte_range.clone(),
                        discontinuity: segment.discontinuity,
                        media_segment: segment.clone(),
                        is_init_segment: true,
                    };
                    jobs_to_send.push(init_job);
                    *last_map_uri = Some(absolute_map_uri);
                }
            }

            let absolute_segment_uri = Url::parse(base_url)
                .and_then(|b| b.join(&segment.uri))
                .map(|u| u.to_string())
                .unwrap_or_else(|_| {
                    error!(
                        "Failed to resolve segment URI '{}' with base '{}'",
                        segment.uri, base_url
                    );
                    segment.uri.clone()
                });

            if !seen_segment_uris.contains_key(&absolute_segment_uri) {
                if processed_segment.is_ad {
                    debug!("Skipping Twitch ad segment: {}", segment.uri);
                    continue;
                }

                seen_segment_uris
                    .insert(absolute_segment_uri.clone(), ())
                    .await;
                debug!("New segment detected: {}", absolute_segment_uri);
                let job = ScheduledSegmentJob {
                    segment_uri: absolute_segment_uri,
                    base_url: base_url.to_string(),
                    media_sequence_number: new_playlist.media_sequence + idx as u64,
                    duration: segment.duration,
                    key: segment.key.clone(),
                    byte_range: segment.byte_range.clone(),
                    discontinuity: segment.discontinuity,
                    media_segment: segment.clone(),
                    is_init_segment: false,
                };
                jobs_to_send.push(job);
            } else {
                trace!("Segment {} already seen, skipping.", absolute_segment_uri);
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
            debug!("Sending segment job: {:?}", job.segment_uri);
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
