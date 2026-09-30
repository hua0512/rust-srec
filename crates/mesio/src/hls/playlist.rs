// HLS Playlist loading: initial playlist fetch, parsing, and variant
// selection. Live refresh lives in `engine::watcher::PlaylistWatcher`.

use crate::cache::{CacheKey, CacheManager, CacheMetadata, CacheResourceType};
use crate::downloader::ClientPool;
use crate::hls::HlsDownloaderError;
use crate::hls::config::{HlsConfig, HlsVariantSelectionPolicy};
use crate::hls::twitch_processor::{TwitchPlaylistProcessor, preprocess_twitch_playlist};
use crate::redact::Redacted;
use crate::session::{DownloadEvent, EventSink, ResourceId};
use m3u8_rs::{MasterPlaylist, MediaPlaylist, VariantStream, parse_playlist_res};
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
    events: Option<EventSink>,
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
            events: None,
        }
    }

    pub fn with_events(mut self, events: Option<EventSink>) -> Self {
        self.events = events;
        self
    }

    pub async fn load_initial_playlist(
        &self,
        url_str: &str,
    ) -> Result<InitialPlaylist, HlsDownloaderError> {
        let playlist_url = Url::parse(url_str).map_err(|e| HlsDownloaderError::Playlist {
            reason: format!(
                "Invalid playlist URL {}: {e}",
                crate::redact::redact_url_str(url_str)
            ),
        })?;
        let cache_key = CacheKey::new(CacheResourceType::Playlist, playlist_url.as_str(), None);

        if let Some(cache_service) = &self.cache_service
            && let Ok(Some((cached_data, _, _))) = cache_service.get(&cache_key).await
        {
            emit_event(
                &self.events,
                DownloadEvent::ResourceFinished {
                    resource: ResourceId::HlsPlaylist {
                        url: Arc::from(playlist_url.as_str()),
                    },
                    bytes: cached_data.len() as u64,
                    from_cache: true,
                },
            );
            return Self::parse_initial(&playlist_url, &playlist_url, &cached_data);
        }

        let client = self.clients.client_for_url(&playlist_url);
        let resource = ResourceId::HlsPlaylist {
            url: Arc::from(playlist_url.as_str()),
        };
        emit_event(
            &self.events,
            DownloadEvent::ResourceStarted {
                resource: resource.clone(),
                display_url: Arc::from(Redacted(&playlist_url).to_string()),
                content_length: None,
            },
        );
        let response = client
            .get(playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(HlsDownloaderError::from)?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch playlist {}: HTTP {}",
                    Redacted(&playlist_url),
                    response.status()
                ),
            });
        }
        // Relative URIs resolve against the document actually served, which
        // differs from the requested URL after a redirect.
        let document_url = response.url().clone();
        let playlist_bytes = response.bytes().await.map_err(HlsDownloaderError::from)?;
        emit_event(
            &self.events,
            DownloadEvent::ResourceFinished {
                resource,
                bytes: playlist_bytes.len() as u64,
                from_cache: false,
            },
        );

        // A cache hit resolves against the requested URL, so only cache a
        // playlist whose relative URIs resolve the same way from there.
        if let Some(cache_service) = &self.cache_service
            && document_base_url(&document_url).ok() == document_base_url(&playlist_url).ok()
        {
            let metadata = CacheMetadata::new(playlist_bytes.len() as u64)
                .with_expiration(self.config.playlist_config.initial_playlist_fetch_timeout);
            // Caching is an optimisation; a failed write must not abort the download.
            if let Err(error) = cache_service
                .put(cache_key, playlist_bytes.clone(), metadata)
                .await
            {
                warn!(%error, "failed to cache initial playlist");
            }
        }

        Self::parse_initial(&playlist_url, &document_url, &playlist_bytes)
    }

    /// `document_url` is the URL the playlist was served from (after
    /// redirects); its relative URIs resolve against it.
    fn parse_initial(
        playlist_url: &Url,
        document_url: &Url,
        playlist_bytes: &[u8],
    ) -> Result<InitialPlaylist, HlsDownloaderError> {
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(playlist_url.as_str())
                || TwitchPlaylistProcessor::is_twitch_playlist(document_url.as_str())
            {
                let playlist_content = String::from_utf8_lossy(playlist_bytes);
                Cow::Owned(preprocess_twitch_playlist(&playlist_content).into_bytes())
            } else {
                Cow::Borrowed(playlist_bytes)
            };
        let base_url =
            document_base_url(document_url).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Failed to determine base URL: {e}"),
            })?;
        debug!(
            "Derived base URL from playlist: {} -> {}",
            Redacted(document_url),
            base_url
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
        let selected_variant = select_variant(master_playlist_ref, policy)?;
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

        debug!(
            "Selected media playlist URL: {}",
            Redacted(&media_playlist_url)
        );
        let client = self.clients.client_for_url(&media_playlist_url);
        let resource = ResourceId::HlsPlaylist {
            url: Arc::from(media_playlist_url.as_str()),
        };
        emit_event(
            &self.events,
            DownloadEvent::ResourceStarted {
                resource: resource.clone(),
                display_url: Arc::from(Redacted(&media_playlist_url).to_string()),
                content_length: None,
            },
        );
        let response = client
            .get(media_playlist_url.clone())
            .timeout(self.config.playlist_config.initial_playlist_fetch_timeout)
            .query(&self.config.base.params)
            .send()
            .await
            .map_err(HlsDownloaderError::from)?;
        if !response.status().is_success() {
            return Err(HlsDownloaderError::Playlist {
                reason: format!(
                    "Failed to fetch media playlist {}: HTTP {}",
                    Redacted(&media_playlist_url),
                    response.status()
                ),
            });
        }
        let document_url = response.url().clone();
        let playlist_bytes = response.bytes().await.map_err(HlsDownloaderError::from)?;
        emit_event(
            &self.events,
            DownloadEvent::ResourceFinished {
                resource,
                bytes: playlist_bytes.len() as u64,
                from_cache: false,
            },
        );
        let playlist_bytes_to_parse: Cow<[u8]> =
            if TwitchPlaylistProcessor::is_twitch_playlist(media_playlist_url.as_str())
                || TwitchPlaylistProcessor::is_twitch_playlist(document_url.as_str())
            {
                let playlist_content = String::from_utf8_lossy(&playlist_bytes);
                Cow::Owned(preprocess_twitch_playlist(&playlist_content).into_bytes())
            } else {
                Cow::Borrowed(&playlist_bytes)
            };
        let media_base_url =
            document_base_url(&document_url).map_err(|e| HlsDownloaderError::Playlist {
                reason: format!("Bad base URL for media playlist: {e}"),
            })?;
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

/// Choose the variant to record. I-frame-only entries
/// (`EXT-X-I-FRAME-STREAM-INF`) are trick-play playlists of keyframes, never a
/// recording target, so no policy considers them.
fn select_variant<'a>(
    master: &'a MasterPlaylist,
    policy: &HlsVariantSelectionPolicy,
) -> Result<&'a VariantStream, HlsDownloaderError> {
    let no_variant = |reason: String| HlsDownloaderError::Playlist { reason };
    let mut variants = master.variants.iter().filter(|v| !v.is_i_frame).peekable();
    if variants.peek().is_none() {
        return Err(no_variant("Master playlist has no variants".to_string()));
    }
    match policy {
        HlsVariantSelectionPolicy::HighestBitrate => variants
            .max_by_key(|v| v.bandwidth)
            .ok_or_else(|| no_variant("No variants for HighestBitrate".to_string())),
        HlsVariantSelectionPolicy::LowestBitrate => variants
            .min_by_key(|v| v.bandwidth)
            .ok_or_else(|| no_variant("No variants for LowestBitrate".to_string())),
        HlsVariantSelectionPolicy::ClosestToBitrate(target_bw) => variants
            .min_by_key(|v| v.bandwidth.abs_diff(*target_bw))
            .ok_or_else(|| no_variant(format!("No variants for ClosestToBitrate: {target_bw}"))),
        HlsVariantSelectionPolicy::AudioOnly => variants
            .find(|v| variant_media(v) == Some(VariantMedia::AudioOnly))
            .ok_or_else(|| no_variant("No AudioOnly variant".to_string())),
        HlsVariantSelectionPolicy::VideoOnly => variants
            .find(|v| variant_media(v) == Some(VariantMedia::VideoOnly))
            .ok_or_else(|| no_variant("No VideoOnly variant".to_string())),
        HlsVariantSelectionPolicy::MatchingResolution { width, height } => variants
            .find(|v| {
                v.resolution
                    .is_some_and(|r| r.width == u64::from(*width) && r.height == u64::from(*height))
            })
            .ok_or_else(|| no_variant(format!("No variant for resolution {width}x{height}"))),
        HlsVariantSelectionPolicy::Custom(name) => {
            warn!("Custom policy '{name}' selected; falling back to first variant.");
            variants
                .next()
                .ok_or_else(|| no_variant("No variants for Custom policy".to_string()))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum VariantMedia {
    AudioOnly,
    VideoOnly,
    AudioAndVideo,
}

/// What a variant carries, from its CODECS attribute. RFC 8216 §4.3.4.2
/// requires CODECS to list every format in the variant and its renditions, so
/// the `AUDIO`/`VIDEO` group attributes (which only name alternate renditions)
/// cannot tell an audio-only variant from a muxed one. Twitch's audio-only
/// variant is also recognized by its `VIDEO="audio_only"` group when CODECS is
/// absent.
///
/// A codec this does not recognize could be audio or video, so it rules out
/// both "only" classifications rather than being ignored.
fn variant_media(variant: &VariantStream) -> Option<VariantMedia> {
    let Some(codecs) = variant.codecs.as_deref() else {
        return (variant.video.as_deref() == Some("audio_only")).then_some(VariantMedia::AudioOnly);
    };
    let (mut audio, mut video) = (false, false);
    for codec in codecs.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        let family = codec
            .split('.')
            .next()
            .unwrap_or(codec)
            .to_ascii_lowercase();
        match codec_kind(&family) {
            Some(CodecKind::Audio) => audio = true,
            Some(CodecKind::Video) => video = true,
            Some(CodecKind::Other) => {}
            None => return None,
        }
    }
    match (audio, video) {
        (true, false) => Some(VariantMedia::AudioOnly),
        (false, true) => Some(VariantMedia::VideoOnly),
        (true, true) => Some(VariantMedia::AudioAndVideo),
        (false, false) => None,
    }
}

enum CodecKind {
    Audio,
    Video,
    /// Subtitles and captions: neither audio nor video.
    Other,
}

/// Classify an RFC 6381 codec family (the part before the first `.`).
fn codec_kind(family: &str) -> Option<CodecKind> {
    match family {
        "mp4a" | "ac-3" | "ec-3" | "ac-4" | "opus" | "flac" | "alac" | "mhm1" | "mha1" | "dtsc"
        | "dtse" | "dtsh" | "dtsl" | "dtsx" => Some(CodecKind::Audio),
        "avc1" | "avc3" | "hvc1" | "hev1" | "av01" | "vp08" | "vp09" | "vp8" | "vp9" | "vvc1"
        | "vvi1" | "mp4v" | "dva1" | "dvav" | "dvh1" | "dvhe" | "dav1" => Some(CodecKind::Video),
        "wvtt" | "stpp" | "tx3g" | "c608" | "c708" => Some(CodecKind::Other),
        _ => None,
    }
}

/// Base URL that relative URIs in the playlist served from `document_url`
/// resolve against (RFC 8216 §4.1: the playlist's own URI, after redirects).
pub(crate) fn document_base_url(document_url: &Url) -> Result<String, url::ParseError> {
    document_url.join(".").map(String::from)
}

fn emit_event(events: &Option<EventSink>, event: DownloadEvent) {
    if let Some(events) = events {
        events.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master(body: &str) -> MasterPlaylist {
        match parse_playlist_res(format!("#EXTM3U\n{body}").as_bytes()) {
            Ok(m3u8_rs::Playlist::MasterPlaylist(pl)) => pl,
            other => panic!("expected master playlist, got {other:?}"),
        }
    }

    fn selected_uri(master: &MasterPlaylist, policy: HlsVariantSelectionPolicy) -> String {
        select_variant(master, &policy)
            .expect("a variant is selected")
            .uri
            .clone()
    }

    #[test]
    fn bitrate_policies_ignore_i_frame_only_playlists() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=3000000,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
             high.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=800000,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
             low.m3u8\n\
             #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=90000,CODECS=\"avc1.64001f\",URI=\"iframe-low.m3u8\"\n\
             #EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=9000000,CODECS=\"avc1.64001f\",URI=\"iframe-high.m3u8\"\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::LowestBitrate),
            "low.m3u8"
        );
        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::HighestBitrate),
            "high.m3u8"
        );
        assert_eq!(
            selected_uri(
                &master,
                HlsVariantSelectionPolicy::ClosestToBitrate(100_000)
            ),
            "low.m3u8"
        );
    }

    #[test]
    fn master_with_only_i_frame_playlists_has_no_recordable_variant() {
        let master = master(
            "#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=90000,CODECS=\"avc1.64001f\",URI=\"iframe.m3u8\"\n",
        );

        assert!(select_variant(&master, &HlsVariantSelectionPolicy::HighestBitrate).is_err());
    }

    #[test]
    fn audio_only_picks_the_variant_without_video_codecs() {
        // The muxed variant names an alternate-audio group; that does not make
        // it audio-only.
        let master = master(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",URI=\"audio.m3u8\"\n\
             #EXT-X-STREAM-INF:BANDWIDTH=3000000,CODECS=\"avc1.64001f,mp4a.40.2\",AUDIO=\"aud\"\n\
             muxed.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=160000,CODECS=\"mp4a.40.2\"\n\
             audio-only.m3u8\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::AudioOnly),
            "audio-only.m3u8"
        );
    }

    #[test]
    fn twitch_style_variants_classify_audio_only_and_muxed() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=6000000,CODECS=\"avc1.64002A,mp4a.40.2\",VIDEO=\"chunked\"\n\
             chunked.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=160000,VIDEO=\"audio_only\"\n\
             audio_only.m3u8\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::AudioOnly),
            "audio_only.m3u8"
        );
        // The muxed "chunked" variant carries audio, so it is not video-only.
        assert!(select_variant(&master, &HlsVariantSelectionPolicy::VideoOnly).is_err());
    }

    #[test]
    fn dolby_vision_muxed_variant_is_not_audio_only() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=5000000,CODECS=\"dvav.09.01,mp4a.40.2\"\n\
             dolby.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=160000,CODECS=\"mp4a.40.2\"\n\
             audio-only.m3u8\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::AudioOnly),
            "audio-only.m3u8"
        );
    }

    #[test]
    fn unknown_codecs_rule_out_only_classifications() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=3000000,CODECS=\"xyz1.1,mp4a.40.2\"\n\
             unknown-plus-audio.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2000000,CODECS=\"avc1.64001f,xyz1.1\"\n\
             unknown-plus-video.m3u8\n",
        );

        assert!(select_variant(&master, &HlsVariantSelectionPolicy::AudioOnly).is_err());
        assert!(select_variant(&master, &HlsVariantSelectionPolicy::VideoOnly).is_err());
    }

    #[test]
    fn subtitle_codecs_do_not_prevent_audio_only() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=170000,CODECS=\"mp4a.40.2,wvtt\"\n\
             audio-with-subs.m3u8\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::AudioOnly),
            "audio-with-subs.m3u8"
        );
    }

    #[test]
    fn video_only_picks_the_variant_without_audio_codecs() {
        let master = master(
            "#EXT-X-STREAM-INF:BANDWIDTH=3000000,CODECS=\"avc1.64001f,mp4a.40.2\"\n\
             muxed.m3u8\n\
             #EXT-X-STREAM-INF:BANDWIDTH=2800000,CODECS=\"hvc1.1.6.L93.B0\"\n\
             video-only.m3u8\n",
        );

        assert_eq!(
            selected_uri(&master, HlsVariantSelectionPolicy::VideoOnly),
            "video-only.m3u8"
        );
    }
}
