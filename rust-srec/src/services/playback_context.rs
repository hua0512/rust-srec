//! Ephemeral, principal-bound playback authentication.
//!
//! A context keeps the credential snapshot and provider headers of one managed
//! extraction server-side behind an opaque handle. Media URLs are not secret:
//! the browser receives them and sends them back with the handle, and the proxy
//! attaches the context's headers only to origins the extraction produced.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use platforms_parser::media::MediaInfo;
use serde::Serialize;
use tokio::time::Instant;

use crate::credentials::{CredentialBinding, CredentialOwner, CredentialSnapshot};

const MAX_CONTEXTS: usize = 1024;
pub(crate) const MAX_PLAYBACK_STREAMS: usize = 128;
const IDLE_EXPIRY: Duration = Duration::from_secs(15 * 60);
const ABSOLUTE_EXPIRY: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_URL_BYTES: usize = 16_384;

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("Playback context expired; parse the source again")]
    Expired,
    #[error("Playback context belongs to another principal")]
    Forbidden,
    #[error("Playback credentials or policy changed; renew playback")]
    RenewalRequired,
    #[error("Unknown or invalid playback stream or URL")]
    InvalidResource,
    #[error("Playback authentication is not allowed for this host")]
    HostNotAllowed,
    #[error("Playback resource limit reached; renew playback")]
    ResourceLimit,
}

#[derive(Clone)]
pub(crate) struct PlaybackSource {
    pub url: String,
    pub owner: CredentialOwner,
    pub explicit_profile: Option<String>,
    pub configured_generation: Option<String>,
}

/// This data deliberately implements neither Debug nor Serialize.
pub(crate) struct PlaybackData {
    pub source: PlaybackSource,
    pub snapshot: CredentialSnapshot,
    pub media: MediaInfo,
}

impl PlaybackData {
    /// Whether stream `stream_index`'s credentials may be sent to `target`.
    ///
    /// Only the origin of that stream's extracted URL and the origin of the
    /// source page qualify. Playlists, segments and redirects on any other
    /// origin are fetched without the context's headers, so neither a client
    /// nor an upstream manifest can direct the account's cookies elsewhere.
    pub(crate) fn permits_authentication(&self, target: &url::Url, stream_index: usize) -> bool {
        let Some(stream) = self.media.streams.get(stream_index) else {
            return false;
        };
        let origin = target.origin();
        [stream.url.as_str(), self.source.url.as_str()]
            .into_iter()
            .filter_map(|candidate| url::Url::parse(candidate).ok())
            .any(|candidate| candidate.origin() == origin)
    }
}

/// Safe display data. Cookies, provider headers and extractor extras never
/// enter this DTO; media URLs do because the player needs them.
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct ManagedPlayback {
    pub handle: String,
    pub binding: CredentialBinding,
    pub title: String,
    pub artist: String,
    pub is_live: bool,
    /// Proxy requests name a stream by its index in this list.
    pub streams: Vec<PlaybackStream>,
}

impl std::fmt::Debug for ManagedPlayback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedPlayback")
            .field("handle", &"[redacted]")
            .field("binding", &self.binding)
            .finish()
    }
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybackStream {
    pub url: String,
    pub quality: String,
    pub stream_format: String,
    pub media_format: String,
    pub codec: String,
    pub bitrate: u64,
    pub fps: f64,
    pub is_audio_only: bool,
}

struct Entry {
    principal: String,
    created: Instant,
    used: Instant,
    data: Arc<PlaybackData>,
}

pub struct PlaybackContextService {
    entries: Mutex<HashMap<String, Entry>>,
    capacity: usize,
    idle: Duration,
    absolute: Duration,
}

impl Default for PlaybackContextService {
    fn default() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            capacity: MAX_CONTEXTS,
            idle: IDLE_EXPIRY,
            absolute: ABSOLUTE_EXPIRY,
        }
    }
}

impl PlaybackContextService {
    pub(crate) fn insert(
        &self,
        principal: &str,
        source: PlaybackSource,
        snapshot: CredentialSnapshot,
        media: MediaInfo,
    ) -> Result<ManagedPlayback, PlaybackError> {
        if media.streams.len() > MAX_PLAYBACK_STREAMS {
            return Err(PlaybackError::ResourceLimit);
        }
        for stream in &media.streams {
            checked_url(&stream.url)?;
        }
        let streams = media
            .streams
            .iter()
            .map(|stream| PlaybackStream {
                url: stream.url.clone(),
                quality: stream.quality.clone(),
                stream_format: stream.stream_format.to_string(),
                media_format: stream.media_format.to_string(),
                codec: stream.codec.clone(),
                bitrate: stream.bitrate,
                fps: stream.fps,
                is_audio_only: stream.is_audio_only,
            })
            .collect();
        let handle = hex::encode(rand::random::<[u8; 32]>());
        let response = ManagedPlayback {
            handle: handle.clone(),
            binding: snapshot.binding.clone(),
            title: media.title.clone(),
            artist: media.artist.clone(),
            is_live: media.is_live,
            streams,
        };
        let now = Instant::now();
        let mut entries = self.entries.lock();
        self.prune(&mut entries, now);
        if entries.len() >= self.capacity
            && let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, value)| value.used)
                .map(|(key, _)| key.clone())
        {
            entries.remove(&oldest);
        }
        entries.insert(
            handle,
            Entry {
                principal: principal.to_owned(),
                created: now,
                used: now,
                data: Arc::new(PlaybackData {
                    source,
                    snapshot,
                    media,
                }),
            },
        );
        Ok(response)
    }

    fn prune(&self, entries: &mut HashMap<String, Entry>, now: Instant) {
        entries.retain(|_, entry| {
            now.duration_since(entry.created) < self.absolute
                && now.duration_since(entry.used) < self.idle
        });
    }

    /// Looks up a live context and renews its idle expiry.
    pub(crate) fn get(
        &self,
        handle: &str,
        principal: &str,
    ) -> Result<Arc<PlaybackData>, PlaybackError> {
        let now = Instant::now();
        let mut entries = self.entries.lock();
        self.prune(&mut entries, now);
        let entry = entries.get_mut(handle).ok_or(PlaybackError::Expired)?;
        if entry.principal != principal {
            return Err(PlaybackError::Forbidden);
        }
        entry.used = now;
        Ok(entry.data.clone())
    }
}

/// Parses a media URL a context may fetch: HTTP(S), bounded, no userinfo.
pub(crate) fn checked_url(value: &str) -> Result<url::Url, PlaybackError> {
    if value.len() > MAX_URL_BYTES {
        return Err(PlaybackError::ResourceLimit);
    }
    let url = url::Url::parse(value).map_err(|_| PlaybackError::InvalidResource)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(PlaybackError::InvalidResource);
    }
    Ok(url)
}

#[cfg(test)]
pub(crate) fn test_bundle(target: &str) -> (PlaybackSource, CredentialSnapshot, MediaInfo) {
    use crate::credentials::{
        CredentialIdentity, CredentialMaterial, CredentialSelection, ResolvedCredentialPolicy,
    };
    use platforms_parser::media::{MediaFormat, StreamFormat, StreamInfo};
    let owner = CredentialOwner::Platform {
        platform_id: "test-platform".into(),
    };
    let policy = ResolvedCredentialPolicy::new(
        "test-platform".into(),
        owner.clone(),
        CredentialSelection::Fixed {
            credential_id: "profile-a".into(),
        },
    )
    .unwrap();
    let source = PlaybackSource {
        url: "https://platform.test/room".into(),
        owner,
        explicit_profile: None,
        configured_generation: Some(policy.generation.clone()),
    };
    let snapshot = CredentialSnapshot {
        binding: CredentialBinding {
            identity: CredentialIdentity::Profile {
                profile_id: "profile-a".into(),
            },
            revision: 1,
            epoch: 0,
            policy,
        },
        material: CredentialMaterial {
            cookies: "session=secret-cookie".into(),
            refresh_token: Some("secret-refresh".into()),
            access_token: None,
            reauth_config: None,
        },
        route: crate::proxies::ResolvedRoute::default(),
    };
    let mut media = MediaInfo::builder("https://platform.test/room", "Title", "Artist")
        .is_live(true)
        .streams(vec![
            StreamInfo::builder(target, StreamFormat::Hls, MediaFormat::Ts).build(),
        ])
        .build();
    media.headers = Some(
        [
            ("Cookie".into(), "session=secret-cookie".into()),
            ("Authorization".into(), "Bearer secret-access".into()),
        ]
        .into_iter()
        .collect(),
    );
    (source, snapshot, media)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_results_never_serialize_material_and_handles_are_principal_bound() {
        let service = PlaybackContextService::default();
        let (source, snapshot, media) =
            test_bundle("https://cdn.test/video.m3u8?signature=signed-url");
        let response = service.insert("alice", source, snapshot, media).unwrap();
        assert_eq!(response.handle.len(), 64);
        let public = serde_json::to_string(&response).unwrap();
        for secret in ["secret-cookie", "secret-refresh", "secret-access", "Cookie"] {
            assert!(!public.contains(secret), "{secret} leaked");
        }
        assert!(public.contains("https://cdn.test/video.m3u8?signature=signed-url"));
        assert!(!format!("{response:?}").contains(&response.handle));
        assert!(matches!(
            service.get(&response.handle, "bob"),
            Err(PlaybackError::Forbidden)
        ));
        assert!(
            matches!(
                PlaybackContextService::default().get(&response.handle, "alice"),
                Err(PlaybackError::Expired)
            ),
            "restart does not restore media"
        );
    }

    #[test]
    fn authentication_is_limited_to_the_stream_and_source_origins() {
        let service = PlaybackContextService::default();
        let (source, snapshot, mut media) = test_bundle("https://cdn.test/a.m3u8");
        let mut other = media.streams[0].clone();
        other.url = "https://other-cdn.test/b.m3u8".into();
        media.streams.push(other);
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let data = service.get(&context.handle, "alice").unwrap();
        let permits =
            |url: &str, index| data.permits_authentication(&url::Url::parse(url).unwrap(), index);
        assert!(permits("https://cdn.test/segment.ts?sig=x", 0));
        assert!(permits("https://platform.test/api/key", 0));
        assert!(permits("https://other-cdn.test/segment.ts", 1));
        assert!(!permits("https://other-cdn.test/segment.ts", 0));
        assert!(
            !permits("http://cdn.test/segment.ts", 0),
            "scheme is part of the origin"
        );
        assert!(!permits("https://cdn.test:8443/segment.ts", 0));
        assert!(!permits("https://attacker.test/collect", 0));
        assert!(!permits("https://cdn.test.attacker.test/", 0));
        assert!(
            !permits("https://cdn.test/segment.ts", 2),
            "unknown stream index"
        );
    }

    #[test]
    fn invalid_stream_urls_are_rejected_at_parse() {
        let service = PlaybackContextService::default();
        for url in [
            "ftp://cdn.test/a",
            "https://user:pass@cdn.test/a",
            "not a url",
        ] {
            let (source, snapshot, media) = test_bundle(url);
            assert!(service.insert("alice", source, snapshot, media).is_err());
        }
    }

    #[test]
    fn expiry_and_eviction_are_explicit() {
        let mut service = PlaybackContextService {
            capacity: 1,
            ..PlaybackContextService::default()
        };
        let (source, snapshot, media) = test_bundle("https://cdn.test/a");
        let first = service.insert("alice", source, snapshot, media).unwrap();
        let (source, snapshot, media) = test_bundle("https://cdn.test/b");
        let next = service.insert("alice", source, snapshot, media).unwrap();
        assert!(matches!(
            service.get(&first.handle, "alice"),
            Err(PlaybackError::Expired)
        ));
        assert!(service.get(&next.handle, "alice").is_ok());
        service.absolute = Duration::ZERO;
        assert!(matches!(
            service.get(&next.handle, "alice"),
            Err(PlaybackError::Expired)
        ));
        service.absolute = ABSOLUTE_EXPIRY;
        service.idle = Duration::ZERO;
        let (source, snapshot, media) = test_bundle("https://cdn.test/c");
        let expired = service.insert("alice", source, snapshot, media).unwrap();
        assert!(matches!(
            service.get(&expired.handle, "alice"),
            Err(PlaybackError::Expired)
        ));
    }
}
