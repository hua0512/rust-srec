// HLS Playlist loading: initial playlist fetch, parsing, and variant
// selection. Live refresh lives in `engine::watcher::PlaylistWatcher`.

use crate::cache::{CacheKey, CacheManager, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::twitch_processor::{TwitchPlaylistProcessor, preprocess_twitch_playlist};
use m3u8_rs::{MasterPlaylist, MediaPlaylist, parse_playlist_res};
use std::borrow::Cow;
use std::sync::Arc;
use tracing::{debug, warn};
use url::Url;

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

pub struct PlaylistEngine {
    clients: Arc<ClientPool>,
    cache_service: Option<Arc<CacheManager>>,
    config: Arc<HlsConfig>,
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

    pub async fn load_initial_playlist(
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
            return Self::parse_initial(&playlist_url, &cached_data);
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

        Self::parse_initial(&playlist_url, &playlist_bytes)
    }

    fn parse_initial(
        playlist_url: &Url,
        playlist_bytes: &[u8],
    ) -> Result<InitialPlaylist, HlsDownloaderError> {
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str()) {
                let playlist_content = String::from_utf8_lossy(playlist_bytes);
                Cow::Owned(preprocess_twitch_playlist(&playlist_content).into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes)
            };
        let base_url = playlist_url
            .join(".")
            .map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Failed to determine base URL: {e}"),
            })?
            .to_string();
        debug!(
            "Derived base URL from playlist: {} -> {}",
            playlist_url, base_url
        );
        match parse_playlist_res(&playlist_bytes_to_parse) {
            Ok(m3u8_rs::Playlist::MasterPlaylist(pl)) => Ok(InitialPlaylist::Master(pl, base_url)),
            Ok(m3u8_rs::Playlist::MediaPlaylist(pl)) => Ok(InitialPlaylist::Media(pl, base_url)),
            Err(e) => Err(HlsDownloaderError::Playlist {
                reason: format!("Failed to parse playlist: {e}"),
            }),
        }
    }

    pub async fn select_media_playlist(
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
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(media_playlist_url.as_str()) {
                let playlist_content = String::from_utf8_lossy(&playlist_bytes);
                Cow::Owned(preprocess_twitch_playlist(&playlist_content).into_bytes())
            } else {
                Cow::Borrowed(&playlist_bytes)
            };
        let media_base_url = media_playlist_url
            .join(".")
            .map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Bad base URL for media playlist: {e}"),
            })?
            .to_string();
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
}
