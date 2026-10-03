//! Ephemeral, principal-bound media and authentication. Only opaque IDs leave it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use platforms_parser::media::MediaInfo;
use serde::Serialize;
use tokio::time::Instant;

use crate::credentials::{CredentialBinding, CredentialOwner, CredentialSnapshot};

const MAX_CONTEXTS: usize = 1024;
const MAX_RESOURCES: usize = 2048;
pub(crate) const MAX_PLAYBACK_STREAMS: usize = 128;
const IDLE_EXPIRY: Duration = Duration::from_secs(15 * 60);
const ABSOLUTE_EXPIRY: Duration = Duration::from_secs(12 * 60 * 60);
pub(crate) type PlaybackVariables = std::collections::BTreeMap<String, String>;
pub(crate) const MAX_PLAYBACK_VARIABLES: usize = 64;
pub(crate) const MAX_PLAYBACK_VARIABLE_BYTES: usize = 65_536;

pub(crate) fn validate_playback_variables(
    variables: &PlaybackVariables,
) -> Result<(), PlaybackError> {
    if variables.len() > MAX_PLAYBACK_VARIABLES
        || variables
            .iter()
            .map(|(name, value)| name.len().saturating_add(value.len()))
            .sum::<usize>()
            > MAX_PLAYBACK_VARIABLE_BYTES
    {
        return Err(PlaybackError::ResourceLimit);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("Playback context expired; parse the source again")]
    Expired,
    #[error("Playback context belongs to another principal")]
    Forbidden,
    #[error("Playback credentials or policy changed; renew playback")]
    RenewalRequired,
    #[error("Unknown or invalid playback resource")]
    InvalidResource,
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
    stream_ids: Vec<String>,
    auth_origins: Vec<HashSet<String>>,
}

impl PlaybackData {
    pub(crate) fn permits_authentication(&self, target: &url::Url, stream_index: usize) -> bool {
        self.auth_origins
            .get(stream_index)
            .is_some_and(|origins| origins.contains(&target.origin().ascii_serialization()))
    }
}

/// Safe display data; original URLs, extras and headers never enter this DTO.
#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct ManagedPlayback {
    pub handle: String,
    pub binding: CredentialBinding,
    pub title: String,
    pub artist: String,
    pub is_live: bool,
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

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybackStream {
    pub id: String,
    pub quality: String,
    pub stream_format: String,
    pub media_format: String,
    pub codec: String,
    pub bitrate: u64,
    pub fps: f64,
    pub is_audio_only: bool,
}

#[derive(Clone, Serialize, utoipa::ToSchema)]
pub struct PlaybackResolved {
    pub handle: String,
    pub stream_id: String,
    pub resource_id: String,
}

impl std::fmt::Debug for PlaybackResolved {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PlaybackResolved")
            .field("handle", &"[redacted]")
            .finish()
    }
}

struct Entry {
    principal: String,
    created: Instant,
    used: Instant,
    data: Arc<PlaybackData>,
    resources: HashMap<String, StoredResource>,
    resource_ids: HashMap<String, String>,
    /// Orders resource use within this context for least-recently-used eviction.
    resource_clock: u64,
}

struct StoredResource {
    resource: PlaybackResource,
    key: String,
    used: u64,
}

#[derive(Clone)]
pub(crate) struct PlaybackResource {
    pub target: url::Url,
    pub stream_index: usize,
    pub variables: Arc<PlaybackVariables>,
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

fn opaque_id() -> String {
    hex::encode(rand::random::<[u8; 32]>())
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
        let source_origin = url::Url::parse(&source.url)
            .ok()
            .map(|url| url.origin().ascii_serialization());
        let mut auth_origins = Vec::with_capacity(media.streams.len());
        for stream in &media.streams {
            let url = checked_url(&stream.url)?;
            let mut origins = HashSet::new();
            origins.insert(url.origin().ascii_serialization());
            origins.extend(source_origin.iter().cloned());
            auth_origins.push(origins);
        }
        let stream_ids = media
            .streams
            .iter()
            .map(|_| opaque_id())
            .collect::<Vec<_>>();
        let streams = media
            .streams
            .iter()
            .zip(&stream_ids)
            .map(|(stream, id)| PlaybackStream {
                id: id.clone(),
                quality: stream.quality.clone(),
                stream_format: stream.stream_format.to_string(),
                media_format: stream.media_format.to_string(),
                codec: stream.codec.clone(),
                bitrate: stream.bitrate,
                fps: stream.fps,
                is_audio_only: stream.is_audio_only,
            })
            .collect();
        let handle = opaque_id();
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
                    stream_ids,
                    auth_origins,
                }),
                resources: HashMap::new(),
                resource_ids: HashMap::new(),
                resource_clock: 0,
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

    fn entry<'a>(
        &self,
        entries: &'a mut HashMap<String, Entry>,
        handle: &str,
        principal: &str,
    ) -> Result<&'a mut Entry, PlaybackError> {
        let now = Instant::now();
        self.prune(entries, now);
        let entry = entries.get_mut(handle).ok_or(PlaybackError::Expired)?;
        if entry.principal != principal {
            return Err(PlaybackError::Forbidden);
        }
        entry.used = now;
        Ok(entry)
    }

    pub(crate) fn get(
        &self,
        handle: &str,
        principal: &str,
    ) -> Result<Arc<PlaybackData>, PlaybackError> {
        Ok(self
            .entry(&mut self.entries.lock(), handle, principal)?
            .data
            .clone())
    }

    #[cfg(test)]
    pub(crate) fn register(
        &self,
        handle: &str,
        principal: &str,
        target: url::Url,
    ) -> Result<String, PlaybackError> {
        self.register_resource(handle, principal, target, 0)
    }

    pub(crate) fn register_resource(
        &self,
        handle: &str,
        principal: &str,
        target: url::Url,
        stream_index: usize,
    ) -> Result<String, PlaybackError> {
        self.register_resource_with_variables(
            handle,
            principal,
            target,
            stream_index,
            Arc::new(PlaybackVariables::new()),
        )
    }

    pub(crate) fn register_resource_with_variables(
        &self,
        handle: &str,
        principal: &str,
        target: url::Url,
        stream_index: usize,
        variables: Arc<PlaybackVariables>,
    ) -> Result<String, PlaybackError> {
        use sha2::{Digest, Sha256};
        checked_url(target.as_str())?;
        validate_playback_variables(&variables)?;
        let mut entries = self.entries.lock();
        let entry = self.entry(&mut entries, handle, principal)?;
        if stream_index >= entry.data.media.streams.len() {
            return Err(PlaybackError::InvalidResource);
        }
        let mut hash = Sha256::new();
        for (name, value) in variables.iter() {
            hash.update((name.len() as u64).to_be_bytes());
            hash.update(name.as_bytes());
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        let key = format!(
            "{stream_index}:{}:{}",
            target.as_str(),
            hex::encode(hash.finalize())
        );
        entry.resource_clock += 1;
        let used = entry.resource_clock;
        if let Some(id) = entry.resource_ids.get(&key) {
            if let Some(stored) = entry.resources.get_mut(id) {
                stored.used = used;
            }
            return Ok(id.clone());
        }
        // A live playlist registers new segment URIs for as long as it plays.
        // Evict the least recently used registration instead of ending playback;
        // playlists the player keeps polling stay recent.
        if entry.resources.len() >= MAX_RESOURCES
            && let Some(oldest) = entry
                .resources
                .iter()
                .min_by_key(|(_, stored)| stored.used)
                .map(|(id, _)| id.clone())
            && let Some(evicted) = entry.resources.remove(&oldest)
        {
            entry.resource_ids.remove(&evicted.key);
        }
        let id = opaque_id();
        entry.resource_ids.insert(key.clone(), id.clone());
        entry.resources.insert(
            id.clone(),
            StoredResource {
                resource: PlaybackResource {
                    target,
                    stream_index,
                    variables,
                },
                key,
                used,
            },
        );
        Ok(id)
    }

    pub(crate) fn resource(
        &self,
        handle: &str,
        principal: &str,
        id: &str,
    ) -> Result<PlaybackResource, PlaybackError> {
        let mut entries = self.entries.lock();
        let entry = self.entry(&mut entries, handle, principal)?;
        entry.resource_clock += 1;
        let stored = entry
            .resources
            .get_mut(id)
            .ok_or(PlaybackError::InvalidResource)?;
        stored.used = entry.resource_clock;
        Ok(stored.resource.clone())
    }

    pub(crate) fn resolve(
        &self,
        handle: &str,
        principal: &str,
        stream_id: &str,
    ) -> Result<PlaybackResolved, PlaybackError> {
        let data = self.get(handle, principal)?;
        let index = data
            .stream_ids
            .iter()
            .position(|id| id == stream_id)
            .ok_or(PlaybackError::InvalidResource)?;
        let target = checked_url(&data.media.streams[index].url)?;
        let resource_id = self.register_resource(handle, principal, target, index)?;
        Ok(PlaybackResolved {
            handle: handle.to_owned(),
            stream_id: stream_id.to_owned(),
            resource_id,
        })
    }
}

fn checked_url(value: &str) -> Result<url::Url, PlaybackError> {
    if value.len() > 16_384 {
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
    fn public_results_never_serialize_material_and_resource_ids_are_context_bound() {
        let service = PlaybackContextService::default();
        let (source, snapshot, media) =
            test_bundle("https://cdn.test/video.m3u8?signature=secret-signature");
        let response = service.insert("alice", source, snapshot, media).unwrap();
        assert_eq!(response.handle.len(), 64);
        let public = serde_json::to_string(&response).unwrap();
        for secret in [
            "secret-cookie",
            "secret-refresh",
            "secret-access",
            "secret-signature",
            "cdn.test",
        ] {
            assert!(!public.contains(secret));
        }
        assert!(!format!("{response:?}").contains(&response.handle));
        assert!(matches!(
            service.get(&response.handle, "bob"),
            Err(PlaybackError::Forbidden)
        ));
        let resolved = service
            .resolve(&response.handle, "alice", &response.streams[0].id)
            .unwrap();
        assert!(
            !serde_json::to_string(&resolved)
                .unwrap()
                .contains("secret-signature")
        );
        assert_eq!(
            service
                .resolve(&response.handle, "alice", &response.streams[0].id)
                .unwrap()
                .resource_id,
            resolved.resource_id
        );
        assert!(
            service
                .resource(&response.handle, "bob", &resolved.resource_id)
                .is_err()
        );
        assert!(
            service
                .resource(&response.handle, "alice", "foreign-resource")
                .is_err()
        );
        let data = service.get(&response.handle, "alice").unwrap();
        assert!(data.permits_authentication(&url::Url::parse("https://cdn.test/key").unwrap(), 0));
        assert!(
            !data.permits_authentication(&url::Url::parse("https://other.test/key").unwrap(), 0)
        );
        assert!(
            matches!(
                PlaybackContextService::default().get(&response.handle, "alice"),
                Err(PlaybackError::Expired)
            ),
            "restart does not restore media"
        );
    }

    #[test]
    fn identical_urls_keep_variant_specific_resource_identity() {
        let service = PlaybackContextService::default();
        let (source, snapshot, mut media) = test_bundle("https://cdn.test/same.m3u8");
        let mut alternate = media.streams[0].clone();
        media.streams[0].extras = Some(serde_json::json!({"headers":{"Cookie":"cdn=A"}}));
        alternate.extras = Some(serde_json::json!({"headers":{"Cookie":"cdn=B"}}));
        media.streams.push(alternate);
        let context = service.insert("alice", source, snapshot, media).unwrap();
        let first = service
            .resolve(&context.handle, "alice", &context.streams[0].id)
            .unwrap();
        let second = service
            .resolve(&context.handle, "alice", &context.streams[1].id)
            .unwrap();
        assert_ne!(first.resource_id, second.resource_id);
        assert_eq!(
            service
                .resource(&context.handle, "alice", &first.resource_id)
                .unwrap()
                .stream_index,
            0
        );
        assert_eq!(
            service
                .resource(&context.handle, "alice", &second.resource_id)
                .unwrap()
                .stream_index,
            1
        );
        let data = service.get(&context.handle, "alice").unwrap();
        assert_eq!(data.snapshot.material.cookies, "session=secret-cookie");
        for (index, expected) in [(0, "cdn=A"), (1, "cdn=B")] {
            let cookie = data.media.streams[index].extras.as_ref().unwrap()["headers"]["Cookie"]
                .as_str()
                .unwrap();
            let outgoing =
                crate::credentials::merge_cookie_updates(&data.snapshot.material.cookies, [cookie]);
            assert!(outgoing.contains(expected));
            assert!(outgoing.contains("session=secret-cookie"));
        }
    }

    #[test]
    fn expiry_eviction_and_resource_bounds_are_explicit() {
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
        let ids = (0..MAX_RESOURCES)
            .map(|index| {
                service
                    .register(
                        &next.handle,
                        "alice",
                        url::Url::parse(&format!("https://cdn.test/{index}")).unwrap(),
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        // A playlist the player keeps fetching stays registered; the stale
        // segment registered after it is evicted to make room.
        service.resource(&next.handle, "alice", &ids[0]).unwrap();
        service
            .register(
                &next.handle,
                "alice",
                url::Url::parse("https://cdn.test/overflow").unwrap(),
            )
            .unwrap();
        assert!(service.resource(&next.handle, "alice", &ids[0]).is_ok());
        assert!(matches!(
            service.resource(&next.handle, "alice", &ids[1]),
            Err(PlaybackError::InvalidResource)
        ));
        assert_ne!(
            service
                .register(
                    &next.handle,
                    "alice",
                    url::Url::parse("https://cdn.test/1").unwrap(),
                )
                .unwrap(),
            ids[1],
            "an evicted URL receives a fresh opaque ID"
        );
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
