use m3u8_rs::{self, Playlist as M3u8Playlist};

/// Error type for playlist parsing
#[derive(Debug, thiserror::Error)]
pub enum PlaylistError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("Format error: {0}")]
    Format(String),
}

/// Parse an HLS playlist from a string
pub fn parse_playlist(content: &str) -> Result<M3u8Playlist, PlaylistError> {
    let bytes = content.as_bytes();
    let (_, playlist) =
        m3u8_rs::parse_playlist(bytes).map_err(|e| PlaylistError::Parse(e.to_string()))?;

    Ok(playlist)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_master_playlist() {
        let content = r#"
#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=1280000,AVERAGE-BANDWIDTH=1000000,CODECS="avc1.640029,mp4a.40.2",RESOLUTION=1280x720,FRAME-RATE=29.97
http://example.com/video_720p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2560000,AVERAGE-BANDWIDTH=2000000,CODECS="avc1.640029,mp4a.40.2",RESOLUTION=1920x1080,FRAME-RATE=29.97
http://example.com/video_1080p.m3u8
        "#;

        let playlist = parse_playlist(content.trim()).unwrap();

        match playlist {
            M3u8Playlist::MasterPlaylist(master) => {
                assert_eq!(master.variants.len(), 2);
                assert_eq!(master.variants[0].uri, "http://example.com/video_720p.m3u8");
                assert_eq!(master.variants[0].bandwidth, 1280000);
                assert_eq!(
                    master.variants[1].uri,
                    "http://example.com/video_1080p.m3u8"
                );
                assert_eq!(master.variants[1].bandwidth, 2560000);
            }
            M3u8Playlist::MediaPlaylist(_) => panic!("Expected master playlist"),
        }
    }

    #[test]
    fn test_parse_media_playlist() {
        let content = r#"
#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:8
#EXT-X-MEDIA-SEQUENCE:2680

#EXTINF:7.975,
segment_2680.ts
#EXTINF:7.941,
segment_2681.ts
#EXTINF:7.975,
segment_2682.ts
        "#;

        let playlist = parse_playlist(content.trim()).unwrap();

        match playlist {
            M3u8Playlist::MediaPlaylist(media) => {
                assert_eq!(media.version, Some(3));
                assert_eq!(media.target_duration, 8);
                assert_eq!(media.media_sequence, 2680);
                assert_eq!(media.segments.len(), 3);
                assert_eq!(media.segments[0].uri, "segment_2680.ts");
                assert_eq!(media.segments[0].duration, 7.975);
            }
            M3u8Playlist::MasterPlaylist(_) => panic!("Expected media playlist"),
        }
    }
}
