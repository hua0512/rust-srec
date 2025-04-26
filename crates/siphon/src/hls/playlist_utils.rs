use m3u8_rs::{MasterPlaylist, MediaPlaylist, Playlist};
use reqwest::{Client, Url};
use tracing::{debug, info};

use crate::{DownloadError, hls::playlist::parse_playlist};

/// Helper function to create a common IO error
fn io_error(message: impl Into<String>) -> DownloadError {
    DownloadError::IoError(std::io::Error::new(
        std::io::ErrorKind::Other,
        message.into(),
    ))
}

/// Helper function to resolve a segment URI against a base URL
pub(crate) fn resolve_url(uri: &str, base_url: &Url) -> Result<Url, DownloadError> {
    if uri.starts_with("http://") || uri.starts_with("https://") {
        uri.parse::<Url>()
            .map_err(|_| DownloadError::UrlError(uri.to_string()))
    } else {
        base_url
            .join(uri)
            .map_err(|_| DownloadError::UrlError(uri.to_string()))
    }
}

/// Download and parse an HLS playlist from a URL
pub(crate) async fn get_playlist_url(client: &Client, url: Url) -> Result<Playlist, DownloadError> {
    info!(url = %url, "Downloading HLS playlist");

    // Start the request
    let response = client.get(url.clone()).send().await?;

    // Check response status
    if !response.status().is_success() {
        return Err(DownloadError::StatusCode(response.status()));
    }

    // Get the playlist content
    let content = response.text().await?;

    // Parse the playlist
    let playlist = parse_playlist(&content)
        .map_err(|e| io_error(format!("Failed to parse HLS playlist: {}", e)))?;

    Ok(playlist)
}

/// Handle live playlist refresh
pub(crate) async fn refresh_live_playlist(
    client: &Client,
    playlist_url: &Url,
) -> Result<MediaPlaylist, DownloadError> {
    debug!(url = %playlist_url, "Refreshing HLS playlist");
    let response = client.get(playlist_url.clone()).send().await?;

    if !response.status().is_success() {
        return Err(DownloadError::StatusCode(response.status()));
    }

    let content = response.text().await?;
    let playlist = parse_playlist(&content)
        .map_err(|e| io_error(format!("Failed to parse HLS playlist: {}", e)))?;

    match playlist {
        Playlist::MediaPlaylist(media) => Ok(media),
        _ => Err(io_error("Expected media playlist but got master playlist")),
    }
}

/// Process a master playlist and return the best quality stream's media playlist and its URL
pub(crate) async fn get_best_quality_playlist(
    client: &Client,
    master: &MasterPlaylist,
    master_playlist_url: &Url, // URL the master playlist was fetched from
) -> Result<(MediaPlaylist, Url), DownloadError> {
    // Return playlist and its URL
    // Base URL for resolving relative variant URIs
    // Use url.join(".") for robust base URL derivation
    let base_url = master_playlist_url
        .join(".")
        .map_err(|_| DownloadError::UrlError("Failed to derive base URL".to_string()))?;

    // Find the variant with highest bandwidth
    let best_variant = master
        .variants
        .iter()
        .max_by_key(|v| v.bandwidth)
        .ok_or_else(|| io_error("Master playlist contains no variants"))?;

    debug!(
        bandwidth = best_variant.bandwidth,
        uri = %best_variant.uri,
        "Selected best quality stream from master playlist"
    );

    // Resolve the URL for this variant using the master playlist's base URL
    let variant_url = resolve_url(&best_variant.uri, &base_url)?;

    // Download the media playlist using its specific URL
    let playlist = get_playlist_url(client, variant_url.clone()).await?; // Clone variant_url here

    match playlist {
        Playlist::MediaPlaylist(media) => Ok((media, variant_url)), // Return the media playlist and its URL
        Playlist::MasterPlaylist(_) => Err(io_error(
            "Expected media playlist, but got another master playlist",
        )),
    }
}
