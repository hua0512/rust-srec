// HLS Playlist Engine: Handles fetching, parsing, and managing HLS playlists.

use crate::CacheManager;
use crate::cache::{CacheKey, CacheMetadata, CacheResourceType};
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::scheduler::ScheduledSegmentJob;
use async_trait::async_trait;
use lru::LruCache;
use m3u8_rs::{MasterPlaylist, MediaPlaylist, parse_playlist_res};
use reqwest::Client;
use std::num::NonZeroUsize;
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
                let playlist_content = String::from_utf8(cached_data.0.to_vec()).map_err(|e| {
                    HlsDownloaderError::PlaylistError(format!(
                        "Failed to parse cached playlist from UTF-8: {e}"
                    ))
                })?;
                let base_url_obj = playlist_url.join(".").map_err(|e| {
                    HlsDownloaderError::PlaylistError(format!(
                        "Failed to determine base URL: {e}"
                    ))
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
        let playlist_content = String::from_utf8(playlist_bytes.to_vec()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Playlist content is not valid UTF-8: {e}"))
        })?;
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
        let playlist_content = String::from_utf8(playlist_bytes.to_vec()).map_err(|e| {
            HlsDownloaderError::PlaylistError(format!("Media playlist not UTF-8: {e}"))
        })?;
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

        /// The LRU cache capacity for seen segments.
        const SEEN_SEGMENTS_LRU_CAPACITY: usize = 20;
        let mut seen_segment_uris: LruCache<String, ()> =
            LruCache::new(NonZeroUsize::new(SEEN_SEGMENTS_LRU_CAPACITY).unwrap());

        let mut last_map_uri: Option<String> = None;

        let mut retries = 0;

        loop {
            let response_result = self
                .http_client
                .get(playlist_url.clone())
                .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
                .send()
                .await;

            match response_result {
                Ok(response) => {
                    if !response.status().is_success() {
                        error!(
                            "HTTP error refreshing playlist {playlist_url}: {}",
                            response.status()
                        );
                        retries += 1;
                        if retries > self.config.playlist_config.live_max_refresh_retries {
                            return Err(HlsDownloaderError::PlaylistError(format!(
                                "Max retries for live playlist {playlist_url}: {}",
                                response.status()
                            )));
                        }
                        tokio::time::sleep(
                            self.config.playlist_config.live_refresh_retry_delay * retries,
                        )
                        .await;
                        continue;
                    }
                    retries = 0;

                    let playlist_bytes = match response.bytes().await {
                        Ok(b) => b,
                        Err(e) => {
                            error!(
                                "Error reading refreshed playlist bytes for {playlist_url}: {e}"
                            );
                            continue;
                        }
                    };

                    match parse_playlist_res(playlist_bytes.as_ref()) {
                        Ok(m3u8_rs::Playlist::MediaPlaylist(new_mp)) => {
                            let mut jobs_to_send = Vec::new();

                            for (idx, segment) in new_mp.segments.iter().enumerate() {
                                if let Some(map_info) = &segment.map {
                                    let absolute_map_uri = if map_info.uri.starts_with("http://")
                                        || map_info.uri.starts_with("https://")
                                    {
                                        map_info.uri.clone()
                                    } else {
                                        match Url::parse(&base_url) {
                                            Ok(b_url) => b_url.join(&map_info.uri).map(|u| u.to_string()).unwrap_or_else(|e| {
                                                error!("Error joining base_url '{}' with map URI '{}': {}", base_url, map_info.uri, e);
                                                map_info.uri.clone()
                                            }),
                                            Err(e) => {
                                                error!("Invalid base_url '{}' for resolving map URI '{}': {}", base_url, map_info.uri, e);
                                                map_info.uri.clone()
                                            }
                                        }
                                    };

                                    if last_map_uri.as_ref() != Some(&absolute_map_uri) {
                                        debug!("New init segment detected: {}", absolute_map_uri);
                                        let init_job = ScheduledSegmentJob {
                                            segment_uri: absolute_map_uri.clone(),
                                            base_url: base_url.clone(),
                                            media_sequence_number: new_mp.media_sequence
                                                + idx as u64,
                                            duration: 0.0,
                                            key: segment.key.clone(),
                                            byte_range: map_info.byte_range.clone(),
                                            discontinuity: segment.discontinuity,
                                            media_segment: segment.clone(),
                                            is_init_segment: true,
                                        };
                                        if segment_request_tx.send(init_job).await.is_err() {
                                            error!(
                                                "SegmentScheduler request channel closed while sending init job for {}.",
                                                playlist_url_str
                                            );
                                            return Err(HlsDownloaderError::InternalError(
                                                "SegmentScheduler request channel closed"
                                                    .to_string(),
                                            ));
                                        }
                                        last_map_uri = Some(absolute_map_uri);
                                    }
                                }

                                let absolute_segment_uri = if segment.uri.starts_with("http://")
                                    || segment.uri.starts_with("https://")
                                {
                                    segment.uri.clone()
                                } else {
                                    match Url::parse(&base_url) {
                                        Ok(b_url) => b_url.join(&segment.uri).map(|u| u.to_string()).unwrap_or_else(|e| {
                                            error!("Error joining base_url '{}' with segment URI '{}': {}", base_url, segment.uri, e);
                                            segment.uri.clone()
                                        }),
                                        Err(e) => {
                                             error!("Invalid base_url '{}' for resolving segment URI '{}': {}", base_url, segment.uri, e);
                                             segment.uri.clone()
                                        }
                                    }
                                };

                                if seen_segment_uris.get(&absolute_segment_uri).is_none() {
                                    seen_segment_uris.put(absolute_segment_uri.clone(), ());
                                    debug!("New segment detected: {}", absolute_segment_uri);
                                    let job = ScheduledSegmentJob {
                                        segment_uri: absolute_segment_uri,
                                        base_url: base_url.clone(),
                                        media_sequence_number: new_mp.media_sequence + idx as u64,
                                        duration: segment.duration,
                                        key: segment.key.clone(),
                                        byte_range: segment.byte_range.clone(),
                                        discontinuity: segment.discontinuity,
                                        media_segment: segment.clone(),
                                        is_init_segment: false,
                                    };
                                    jobs_to_send.push(job);
                                } else {
                                    trace!(
                                        "Segment {} already seen, skipping.",
                                        absolute_segment_uri
                                    );
                                    // Skip this segment
                                    continue;
                                }
                            }

                            if !jobs_to_send.is_empty() {
                                for job in jobs_to_send {
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
                            }

                            current_playlist = new_mp;
                            if current_playlist.end_list {
                                info!("ENDLIST for {playlist_url}. Stopping monitoring.");
                                return Ok(());
                            }
                        }
                        Ok(m3u8_rs::Playlist::MasterPlaylist(_)) => {
                            return Err(HlsDownloaderError::PlaylistError(format!(
                                "Expected Media Playlist, got Master for {playlist_url_str}"
                            )));
                        }
                        Err(e) => {
                            error!("Failed to parse refreshed playlist {playlist_url}: {e}");
                        }
                    }
                }
                Err(e) => {
                    error!("Network error refreshing playlist {playlist_url}: {e}");
                    retries += 1;
                    if retries > self.config.playlist_config.live_max_refresh_retries {
                        return Err(HlsDownloaderError::NetworkError {
                            source: Arc::new(e),
                        });
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
}
