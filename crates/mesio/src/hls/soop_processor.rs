use m3u8_rs::MediaSegment;
use url::Url;

/// SOOP CDN hosts live under `*.sooplive.co.kr` / `*.sooplive.com` (legacy
/// `*.afreecatv.com`). Host-based so a non-SOOP URL that merely mentions the
/// brand in its path or query is not misclassified.
#[inline]
pub(crate) fn is_soop_playlist(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .host_str()
                .map(|host| host.contains("sooplive") || host.contains("afreeca"))
        })
        .unwrap_or(false)
}

/// SOOP live playlists interleave placeholder entries (URI contains
/// "preloading") with real segments. They carry no broadcast content and must
/// be skipped, but only at the planner's MSN-attribution stage — removing
/// them from `MediaPlaylist.segments` would shift the positional
/// `msn = window_start + idx` derivation in `engine::planner::plan`.
#[inline]
pub(crate) fn is_preloading_segment(segment: &MediaSegment) -> bool {
    segment.uri.contains("preloading")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_soop_playlist_hosts() {
        assert!(is_soop_playlist("https://live.sooplive.com/playlist.m3u8"));
        assert!(is_soop_playlist(
            "https://pc-web.stream.sooplive.co.kr/live.m3u8"
        ));
        assert!(is_soop_playlist("https://cdn.afreecatv.com/playlist.m3u8"));
        assert!(!is_soop_playlist("https://example.com/playlist.m3u8"));
        assert!(!is_soop_playlist(
            "https://example.com/sooplive/playlist.m3u8?ref=afreeca"
        ));
    }

    #[test]
    fn detects_preloading_segments() {
        let mut segment = MediaSegment::empty();
        segment.uri = "preloading-segment.ts".to_string();
        assert!(is_preloading_segment(&segment));

        segment.uri = "segment-1.ts".to_string();
        assert!(!is_preloading_segment(&segment));
    }
}
