use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, Playlist};
use reqwest::Client;
use url::Url;

use super::error::ExtractorError;
use crate::media::{MediaFormat, stream_info::StreamInfo};

#[async_trait]
pub trait HlsExtractor {
    async fn extract_hls_stream(
        &self,
        client: &Client,
        headers: Option<reqwest::header::HeaderMap>,
        m3u8_url: &str,
        extras: Option<serde_json::Value>,
    ) -> Result<Vec<StreamInfo>, ExtractorError> {
        let base_url =
            Url::parse(m3u8_url).map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

        let response = client
            .get(m3u8_url)
            .headers(headers.unwrap_or_default())
            .send()
            .await?
            .bytes()
            .await?;
        let playlist = m3u8_rs::parse_playlist_res(&response)
            .map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

        let streams = match playlist {
            Playlist::MasterPlaylist(pl) => process_master_playlist(pl, &base_url, extras),
            Playlist::MediaPlaylist(_) => vec![StreamInfo {
                url: m3u8_url.to_string(),
                format: MediaFormat::Hls,
                quality: "source".to_string(),
                bitrate: 0,
                priority: 0,
                extras,
                codec: "".to_string(),
                fps: 0.0,
                is_headers_needed: false,
            }],
        };

        Ok(streams)
    }
}

fn process_master_playlist(
    playlist: MasterPlaylist,
    base_url: &Url,
    extras: Option<serde_json::Value>,
) -> Vec<StreamInfo> {
    playlist
        .variants
        .into_iter()
        .map(|variant| {
            let stream_url = base_url.join(&variant.uri).unwrap();
            let bitrate = variant.bandwidth / 1000;
            StreamInfo {
                url: stream_url.to_string(),
                format: MediaFormat::Hls,
                quality: variant
                    .resolution
                    .map(|r| format!("{}x{}", r.width, r.height))
                    .unwrap_or_default(),
                bitrate,
                priority: 0,
                extras: extras.clone(),
                codec: variant.codecs.unwrap_or_default(),
                fps: variant.frame_rate.unwrap_or(0.0),
                is_headers_needed: false,
            }
        })
        .collect()
}
