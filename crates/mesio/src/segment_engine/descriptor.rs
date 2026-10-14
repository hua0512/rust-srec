//! Normalized segment descriptors — the input to lifecycle scheduling.
//!
//! The manifest planner owns all manifest-specific normalization; later stages
//! never inspect protocol metadata to decide identity or fetch policy. What
//! they do need from it (duration, container, resource identity) goes through
//! the accessors on [`SegmentMeta`] and [`SegmentDescriptor`].

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use url::Url;

use crate::session::ResourceId;

use super::container::ContainerKind;
use super::identity::SegmentKey;

/// Where a descriptor came from. Carries prefetch-ness for scheduling priority
/// only; deliberately absent from `SegmentKey` so a prefetch URI and its later
/// media incarnation resolve to one key (see `SegmentKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentSource {
    Playlist,
    PlaylistPrefetch,
}

/// Normalized encryption metadata, created by the planner so the payload
/// processor never reinterprets raw playlist key tags.
#[derive(Debug, Clone)]
pub struct EncryptionDescriptor {
    pub method: EncryptionMethod,
    /// Normalized cache identity for the key (auth params stripped per the
    /// source's `IdentityPolicy`). Stable across refreshes; the key cache and
    /// its single-flight coalescing key on this, never on `key_fetch_url`.
    pub key_identity_uri: Arc<str>,
    /// Full URL actually fetched, retaining rotating auth params. Refreshed on
    /// re-discovery exactly like `SegmentDescriptor::parsed_url`.
    pub key_fetch_url: Arc<Url>,
    pub iv: EffectiveIv,
    pub key_format: KeyFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionMethod {
    Aes128Cbc,
    /// Any method the processor cannot decrypt yet (SAMPLE-AES, AES-256, ...).
    /// Carries the raw method token for diagnostics; always maps to a terminal
    /// segment failure (`FailureClass::UnsupportedCrypto`).
    Unsupported(Arc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectiveIv {
    Explicit([u8; 16]),
    /// AES-128 key tag omitted the IV: derive it from the segment MSN at
    /// decrypt time (big-endian in the low 8 bytes).
    MediaSequenceDerived(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFormat {
    Identity,
    Unsupported(Arc<str>),
}

/// Normalized input to lifecycle scheduling. `key.uri` is the dedup identity;
/// `parsed_url` is what actually gets fetched (it keeps the rotating auth
/// params the CDN requires).
#[derive(Debug, Clone)]
pub struct SegmentDescriptor {
    pub key: SegmentKey,
    /// For media: the media sequence number. For init: the MSN of the first
    /// segment the init map covers — ordering metadata for the assembler,
    /// never part of identity.
    pub msn: u64,
    pub source: SegmentSource,
    pub parsed_url: Arc<Url>,
    pub discontinuity: bool,
    pub encryption: Option<EncryptionDescriptor>,
    /// For media segments governed by an `EXT-X-MAP`: the key of that init
    /// resource. The assembler gates emission of this media on that init
    /// having arrived (or terminally failed), so a rotated init cannot lose
    /// the race against the first media segment it covers.
    pub init_key: Option<SegmentKey>,
    /// Protocol metadata carried for output construction. Identity and
    /// scheduling use the typed fields above.
    pub meta: SegmentMeta,
}

/// Protocol-specific segment metadata. Each variant answers the accessors the
/// engine core needs, so the core never matches on a protocol itself.
#[derive(Debug, Clone)]
pub enum SegmentMeta {
    /// The parser-native HLS segment, used to build `HlsData` on output.
    Hls(Arc<m3u8_rs::MediaSegment>),
    /// A DASH segment of one selected track.
    Dash(Arc<DashSegmentMeta>),
}

impl SegmentMeta {
    /// The protocol name errors carry.
    pub fn protocol(&self) -> &'static str {
        match self {
            Self::Hls(_) => "hls",
            Self::Dash(_) => "dash",
        }
    }

    /// Nominal media duration in seconds (`0.0` for init segments).
    pub fn duration_secs(&self) -> f32 {
        match self {
            Self::Hls(segment) => segment.duration,
            Self::Dash(meta) => meta.duration_secs() as f32,
        }
    }

    /// The container the manifest declares, or `None` when it must be
    /// detected from the downloaded body. HLS playlists do not declare it
    /// reliably (`EXT-X-MAP` is allowed for MPEG-TS too), so HLS always
    /// detects. DASH declares it through the Representation's MIME type, and
    /// only ISO BMFF Representations are ever planned.
    pub fn container(&self) -> Option<ContainerKind> {
        match self {
            Self::Hls(_) => None,
            Self::Dash(_) => Some(ContainerKind::Fmp4),
        }
    }
}

/// A media track of a stream delivered as separate tracks (DASH).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaTrack {
    Video,
    Audio,
}

impl MediaTrack {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Video => "video",
            Self::Audio => "audio",
        }
    }
}

impl std::fmt::Display for MediaTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Manifest facts about a DASH segment, captured when it was planned.
///
/// Media timing here comes from the MPD and is only used for scheduling and
/// buffer accounting. Output timing is read from the downloaded fragments,
/// because MPD timelines may differ from the media's decode times (encoder
/// priming, segments that are not frame-aligned to the template duration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashSegmentMeta {
    pub track: MediaTrack,
    /// `Period@id`, when the MPD gives one.
    pub period_id: Option<Arc<str>>,
    /// Period start on the MPD timeline.
    pub period_start: Duration,
    pub representation_id: Arc<str>,
    /// `$Number$` of the segment; for an init, of the first media segment
    /// planned with it.
    pub number: u64,
    /// `$Time$` in `timescale` units; for an init, of the first media
    /// segment planned with it.
    pub time: u64,
    /// MPD duration in `timescale` units; 0 for an init.
    pub duration: u64,
    /// MPD timescale of the Representation's segment information.
    pub timescale: u64,
    /// `@presentationTimeOffset` in `timescale` units.
    pub presentation_time_offset: u64,
    /// `MPD@availabilityStartTime`, the wall-clock anchor of a dynamic
    /// presentation's timeline. For media, the one the segment's epoch
    /// opened with, and `None` once an MPD update contradicted that
    /// epoch's timing.
    pub availability_start_time: Option<DateTime<Utc>>,
}

impl DashSegmentMeta {
    /// MPD duration in seconds; 0 for an init or an invalid timescale.
    pub fn duration_secs(&self) -> f64 {
        if self.timescale == 0 {
            return 0.0;
        }
        self.duration as f64 / self.timescale as f64
    }
}

/// Adds each `parent` query parameter whose key `url` does not already carry.
///
/// Manifest-relative segment URLs lose the manifest URL's query string when
/// resolved, yet token-authenticated CDNs often expect the same parameters on
/// every request of a stream. Keys already present on the segment URL win.
pub(crate) fn inherit_query_params(url: &mut Url, parent: &[(String, String)]) {
    for (key, value) in parent {
        if url.query_pairs().any(|(existing, _)| existing == *key) {
            continue;
        }
        url.query_pairs_mut().append_pair(key, value);
    }
}

/// Query parameters of a manifest URL, for [`inherit_query_params`].
pub(crate) fn query_params(query: Option<&str>) -> Vec<(String, String)> {
    query
        .map(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect()
        })
        .unwrap_or_default()
}

impl SegmentDescriptor {
    /// Resource identity for session events (progress, retries) about this
    /// segment.
    pub fn resource_id(&self) -> ResourceId {
        match &self.meta {
            SegmentMeta::Hls(_) => ResourceId::HlsSegment {
                key: self.key.clone(),
            },
            SegmentMeta::Dash(meta) => ResourceId::DashSegment {
                track: meta.track,
                key: self.key.clone(),
            },
        }
    }

    /// Admission-time size estimate. A BYTERANGE length is the one size the
    /// playlist states exactly; everything else falls back to the caller's
    /// running estimate (`next_ready_jobs` passes the configured/EMA value).
    pub fn size_estimate(&self, fallback: u64) -> u64 {
        match self.key.byte_range {
            Some(range) => range.length,
            None => fallback,
        }
    }
}
