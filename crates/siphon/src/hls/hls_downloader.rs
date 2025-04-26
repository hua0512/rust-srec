use m3u8_rs::{MasterPlaylist, MediaPlaylist, Playlist as M3u8Playlist};
use reqwest::{Client, Url};
use tracing::info;

use crate::{
    DownloadError, DownloaderConfig,
    downloader::create_client,
    hls::{
        playlist_utils::{get_best_quality_playlist, get_playlist_url as get_playlist_url_util},
        stream_handler::{SegmentStream, download_live_playlist, download_vod_playlist},
    },
};

/// HLS Downloader for streaming HLS content from URLs
pub struct HlsDownloader {
    client: Client,
    config: DownloaderConfig, // Keep config even if unused for now, might be used later
}

impl HlsDownloader {
    /// Create a new HlsDownloader with default configuration
    pub fn new() -> Result<Self, DownloadError> {
        Self::with_config(DownloaderConfig::default())
    }

    /// Create a new HlsDownloader with custom configuration
    pub fn with_config(config: DownloaderConfig) -> Result<Self, DownloadError> {
        let client = create_client(&config)?;
        Ok(Self { client, config })
    }

    /// Download and parse an HLS playlist from a URL string
    pub async fn get_playlist(&self, url_str: &str) -> Result<M3u8Playlist, DownloadError> {
        let url = url_str
            .parse::<Url>()
            .map_err(|_| DownloadError::UrlError(url_str.to_string()))?;
        self.get_playlist_url(url).await
    }

    /// Download and parse an HLS playlist from a URL
    pub async fn get_playlist_url(&self, url: Url) -> Result<M3u8Playlist, DownloadError> {
        get_playlist_url_util(&self.client, url).await
    }

    /// Download media segments from a media playlist
    pub async fn download_media_playlist(
        &self,
        playlist: &MediaPlaylist,
        playlist_url: &Url, // Use the actual playlist URL for refreshes
    ) -> Result<SegmentStream, DownloadError> {
        // Base URL for resolving relative segment URIs
        let base_url = playlist_url
            .join(".")
            .map_err(|_| DownloadError::UrlError("Failed to derive base URL".to_string()))?;

        info!(
            segments = playlist.segments.len(),
            duration = playlist.target_duration,
            is_live = !playlist.end_list,
            playlist_url = %playlist_url,
            base_url = %base_url,
            "Starting HLS media playlist download"
        );

        // Call the appropriate stream handler function, passing the config
        if playlist.end_list {
            // VOD content
            download_vod_playlist(&self.client, playlist, &base_url, &self.config).await // Pass config
        } else {
            // Live stream
            download_live_playlist(
                &self.client,
                playlist,
                playlist_url,
                &base_url,
                &self.config,
            )
            .await // Pass config
        }
    }

    /// Process a master playlist and return the best quality stream's media playlist and its URL
    pub async fn get_best_quality_playlist(
        &self,
        master: &MasterPlaylist,
        master_playlist_url: &Url, // URL the master playlist was fetched from
    ) -> Result<(MediaPlaylist, Url), DownloadError> {
        get_best_quality_playlist(&self.client, master, master_playlist_url).await
    }
}
