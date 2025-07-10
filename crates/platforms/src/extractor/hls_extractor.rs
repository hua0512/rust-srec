use async_trait::async_trait;
use m3u8_rs::{MasterPlaylist, Playlist};
use reqwest::Client;
use serde::Serialize;
use url::Url;

use super::error::ExtractorError;
use crate::media::{MediaFormat, StreamFormat, stream_info::StreamInfo};

#[async_trait]
pub trait HlsExtractor {
    async fn extract_hls_stream<Q>(
        &self,
        client: &Client,
        headers: Option<reqwest::header::HeaderMap>,
        params: Option<&Q>,
        m3u8_url: &str,
        quality_name: Option<&str>,
        extras: Option<serde_json::Value>,
    ) -> Result<Vec<StreamInfo>, ExtractorError>
    where
        Q: Serialize + Send + Sync + ?Sized,
    {
        let base_url =
            Url::parse(m3u8_url).map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

        let mut request = client.get(m3u8_url).headers(headers.unwrap_or_default());

        if let Some(params) = params {
            request = request.query(params);
        }

        let response = request.send().await?.bytes().await?;
        let playlist = m3u8_rs::parse_playlist_res(&response)
            .map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

        let streams = match playlist {
            Playlist::MasterPlaylist(pl) => process_master_playlist(pl, &base_url, extras),
            Playlist::MediaPlaylist(pl) => {
                let media_format = if pl
                    .segments
                    .iter()
                    .any(|s| s.uri.contains("fmp4") || s.uri.contains(".mp4"))
                {
                    MediaFormat::Mp4
                } else {
                    MediaFormat::Ts
                };

                vec![StreamInfo {
                    url: m3u8_url.to_string(),
                    stream_format: StreamFormat::Hls,
                    media_format,
                    quality: quality_name.unwrap_or("Source").to_string(),
                    bitrate: 0,
                    priority: 0,
                    extras,
                    codec: "".to_string(),
                    fps: 0.0,
                    is_headers_needed: false,
                }]
            }
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

            // debug!("variant: {:?}", variant);
            let video = variant.video.unwrap_or_default();
            let video = if video == "chunked" {
                "Source".to_string()
            } else {
                video
            };
            // debug!("video: {:?}", video);
            let quality = variant
                .resolution
                .map(|r| format!("{} - {}x{}", video, r.width, r.height))
                .unwrap_or(video);

            StreamInfo {
                url: stream_url.to_string(),
                stream_format: StreamFormat::Hls,
                // we do not know the media format here, so we use Ts as default
                media_format: MediaFormat::Ts,
                quality,
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
