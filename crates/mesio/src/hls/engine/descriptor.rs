//! Normalized segment descriptors — the input to lifecycle scheduling.
//!
//! The manifest planner owns all playlist-specific normalization; later stages
//! never inspect raw `MediaSegment` fields to decide identity or fetch policy.

use std::sync::Arc;
use url::Url;

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
    /// Parser-native segment carried for output compatibility (`HlsData`
    /// construction). Identity and scheduling use the typed fields above.
    pub media_segment: Arc<m3u8_rs::MediaSegment>,
}

impl SegmentDescriptor {
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
