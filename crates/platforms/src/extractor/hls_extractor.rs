use std::future::Future;

use m3u8_rs::{MasterPlaylist, Playlist, QuotedOrUnquoted, VariantStream};
use reqwest::Client;
use serde::Serialize;
use url::Url;

use super::error::ExtractorError;
use crate::media::{MediaFormat, StreamFormat, stream_info::StreamInfo};

pub trait HlsExtractor: Sync {
    /// Extract HLS streams without query parameters
    fn extract_hls_stream(
        &self,
        client: &Client,
        headers: Option<reqwest::header::HeaderMap>,
        m3u8_url: &str,
        quality_name: Option<&str>,
        extras: Option<serde_json::Value>,
    ) -> impl Future<Output = Result<Vec<StreamInfo>, ExtractorError>> + Send {
        self.extract_hls_stream_with_params::<()>(
            client,
            headers,
            None,
            m3u8_url,
            quality_name,
            extras,
        )
    }

    /// Extract HLS streams with query parameters
    fn extract_hls_stream_with_params<Q>(
        &self,
        client: &Client,
        headers: Option<reqwest::header::HeaderMap>,
        params: Option<&Q>,
        m3u8_url: &str,
        quality_name: Option<&str>,
        extras: Option<serde_json::Value>,
    ) -> impl Future<Output = Result<Vec<StreamInfo>, ExtractorError>> + Send
    where
        Q: Serialize + Send + Sync + ?Sized,
    {
        async move {
            let base_url = Url::parse(m3u8_url)
                .map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

            let mut request = client.get(m3u8_url).headers(headers.unwrap_or_default());

            if let Some(params) = params {
                request = request.query(params);
            }

            let response = ExtractorError::check_response(request.send().await?)?
                .bytes()
                .await?;
            let playlist = m3u8_rs::parse_playlist_res(&response)
                .map_err(|e| ExtractorError::HlsPlaylistError(e.to_string()))?;

            let streams = match playlist {
                Playlist::MasterPlaylist(pl) => process_master_playlist(pl, &base_url, extras),
                Playlist::MediaPlaylist(pl) => {
                    let media_format = if pl
                        .segments
                        .iter()
                        .any(|s| s.uri.contains("fmp4") || s.uri.contains(".m4s"))
                    {
                        MediaFormat::Fmp4
                    } else if pl.segments.iter().any(|s| s.uri.contains(".mp4")) {
                        MediaFormat::Mp4
                    } else {
                        MediaFormat::Ts
                    };

                    vec![
                        StreamInfo::builder(m3u8_url.to_string(), StreamFormat::Hls, media_format)
                            .quality(quality_name.unwrap_or("Source"))
                            .extras_opt(extras)
                            .build(),
                    ]
                }
            };

            Ok(streams)
        }
    }
}

/// The variant name an Amazon IVS playlist (Twitch usher v2) carries in place
/// of an `EXT-X-MEDIA` video group. The source rendition maps to `chunked`,
/// the group id those playlists used for it, so it keeps the name "Source".
fn ivs_variant_name(variant: &VariantStream) -> Option<String> {
    let attributes = variant.other_attributes.as_ref()?;
    let attribute = |name: &str| match attributes.get(name)? {
        QuotedOrUnquoted::Quoted(value) | QuotedOrUnquoted::Unquoted(value) => Some(value.as_str()),
    };
    if attribute("IVS-VARIANT-SOURCE").is_some_and(|source| source.eq_ignore_ascii_case("source")) {
        return Some("chunked".to_owned());
    }
    attribute("STABLE-VARIANT-ID")
        .or_else(|| attribute("IVS-NAME"))
        .map(str::to_owned)
}

fn process_master_playlist(
    playlist: MasterPlaylist,
    base_url: &Url,
    extras: Option<serde_json::Value>,
) -> Vec<StreamInfo> {
    playlist
        .variants
        .into_iter()
        .filter_map(|variant| {
            // Some playlists contain invalid/odd URIs; skip those rather than panicking.
            let stream_url = base_url.join(&variant.uri).ok()?;
            let bitrate = variant.bandwidth / 1000;

            let video = variant
                .video
                .clone()
                .or_else(|| ivs_variant_name(&variant))
                .unwrap_or_default();
            let video = if video == "chunked" || video.is_empty() {
                "Source".to_string()
            } else {
                video
            };
            let quality = variant
                .resolution
                .map(|r| format!("{} - {}x{}", video, r.width, r.height))
                .unwrap_or(video);

            Some(
                StreamInfo::builder(stream_url.to_string(), StreamFormat::Hls, MediaFormat::Ts)
                    .quality(quality)
                    .bitrate(bitrate)
                    .extras_opt(extras.as_ref().cloned())
                    .codec(variant.codecs.unwrap_or_default())
                    .fps(variant.frame_rate.unwrap_or(0.0))
                    .build(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qualities(playlist: &str) -> Vec<String> {
        let Ok(Playlist::MasterPlaylist(playlist)) =
            m3u8_rs::parse_playlist_res(playlist.as_bytes())
        else {
            panic!("not a master playlist");
        };
        let base_url = Url::parse("https://usher.example/master.m3u8").unwrap();
        process_master_playlist(playlist, &base_url, None)
            .into_iter()
            .map(|stream| stream.quality)
            .collect()
    }

    /// Twitch usher v2 drops the `EXT-X-MEDIA` groups the v1 playlist names
    /// its variants by; both must yield the same quality names.
    #[test]
    fn twitch_usher_v2_variants_keep_the_v1_quality_names() {
        let v1 = r#"#EXTM3U
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="chunked",NAME="1080p60 (source)",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=9008208,RESOLUTION=1920x1080,CODECS="avc1.64002A,mp4a.40.2",VIDEO="chunked",FRAME-RATE=60.000
https://example/1080p60.m3u8
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="480p30",NAME="480p",AUTOSELECT=YES,DEFAULT=YES
#EXT-X-STREAM-INF:BANDWIDTH=1427999,RESOLUTION=852x480,CODECS="avc1.4D401F,mp4a.40.2",VIDEO="480p30",FRAME-RATE=30.000
https://example/480p30.m3u8
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID="audio_only",NAME="audio_only",AUTOSELECT=NO,DEFAULT=NO
#EXT-X-STREAM-INF:BANDWIDTH=160000,CODECS="mp4a.40.2",VIDEO="audio_only"
https://example/audio_only.m3u8
"#;
        let v2 = r#"#EXTM3U
#EXT-X-SESSION-DATA:DATA-ID="NODE",VALUE="example"
#EXT-X-STREAM-INF:BANDWIDTH=9008208,RESOLUTION=1920x1080,CODECS="avc1.64002A,mp4a.40.2",FRAME-RATE=60.000,STABLE-VARIANT-ID="1080p60",IVS-NAME="1080p60",IVS-VARIANT-SOURCE="source"
https://example/1080p60.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1427999,RESOLUTION=852x480,CODECS="avc1.4D401F,mp4a.40.2",FRAME-RATE=30.000,STABLE-VARIANT-ID="480p30",IVS-NAME="480p",IVS-VARIANT-SOURCE="transcode"
https://example/480p30.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=160000,CODECS="mp4a.40.2",STABLE-VARIANT-ID="audio_only",IVS-NAME="audio_only",IVS-VARIANT-SOURCE="transcode"
https://example/audio_only.m3u8
"#;

        let expected = ["Source - 1920x1080", "480p30 - 852x480", "audio_only"];
        assert_eq!(qualities(v1), expected);
        assert_eq!(qualities(v2), expected);
    }
}
