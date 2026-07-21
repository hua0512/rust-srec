//! The manifest planner: snapshot -> normalized segment descriptors.
//!
//! `plan` is a deterministic function over `(snapshot, ctx)` — no I/O, no
//! clock reads. It owns *all* playlist-specific normalization (URI resolution,
//! query inheritance, identity, BYTERANGE offsets, encryption metadata, ad
//! filtering); later stages compare keys, never URLs, and never inspect raw
//! `MediaSegment` fields to decide identity.
//!
//! `PlannerContext` is owned by the reactor and threaded through every call,
//! because two pieces of normalization are inherently cross-snapshot: the
//! BYTERANGE inference chain (a snapshot's first BYTERANGE segment may
//! continue a chain started in the previous snapshot) and the decision
//! watermark used for window-slide gap detection.

use std::collections::HashSet;
use std::sync::Arc;

use tracing::{debug, trace, warn};
use url::Url;

use crate::hls::twitch_processor::{PREFETCH_SEGMENT_TITLE, TwitchPlaylistProcessor};

use super::super::soop_processor::is_preloading_segment;

use super::descriptor::{
    EffectiveIv, EncryptionDescriptor, EncryptionMethod, KeyFormat, SegmentDescriptor,
    SegmentSource,
};
use super::identity::{
    ByteRangeKey, IdentityPolicy, SegmentIdentityPolicy, SegmentKey, SegmentKind,
};
use super::watcher::PlaylistSnapshot;

/// Inclusive MSN range.
pub type MsnRange = (u64, u64);

#[derive(Debug, Default)]
pub struct Planned {
    /// Every plannable segment in the window — including already-known keys,
    /// which `SegmentStateStore::ingest` uses to refresh volatile fetch
    /// metadata (re-discovery is not a no-op).
    pub descriptors: Vec<SegmentDescriptor>,
    /// MSNs proven gone: the window slid past them before they were ever
    /// decided (including across coalesced snapshots). Forwarded as explicit
    /// `AssemblerInput::Skipped`, never a silent stall.
    pub missing: Vec<MsnRange>,
    /// MSNs that will never be planned: ads, empty URIs, uninferable
    /// BYTERANGEs, malformed URLs. Decided exactly once (watermark-gated).
    pub skipped: Vec<MsnRange>,
    /// A media-sequence reset was detected: the window regressed too far to
    /// be a stale edge response. Output continuity cannot be preserved across
    /// a reset (every re-based payload would sit below the assembler's emit
    /// cursor and be stale-rejected forever), so the reactor surfaces this as
    /// a pipeline error and the stream must restart as a new session.
    pub reset: bool,
}

#[derive(Debug)]
pub struct PlannerContext {
    policy: SegmentIdentityPolicy,
    /// First MSN for which no plan/skip decision has been made yet. The
    /// baseline for window-slide gap detection and the guard that makes skip
    /// decisions one-shot across refreshes.
    next_undecided_msn: Option<u64>,
    /// BYTERANGE inference anchor, keyed by the MSN it applies to. A segment
    /// at `next_msn` with `uri` and no explicit offset starts at
    /// `next_offset`. Keying on MSN (not just URI) keeps re-derivation
    /// deterministic when an overlapping window is re-scanned: each segment
    /// re-anchors the chain for *its own* successor, so a new tail segment
    /// infers from the true end of the segment before it rather than a stale
    /// anchor. It also survives refresh boundaries (the predecessor sliding
    /// out of the window) because the anchor is carried across plan() calls.
    byterange_chain: Option<ByteChain>,
    /// Last non-empty segment URI, for BYTERANGE entries that omit the URI.
    last_non_empty_segment_uri: Option<String>,
    /// Active `EXT-X-KEY` scope. m3u8-rs attaches a key tag only to the
    /// segment immediately following it, but per RFC 8216 §4.3.2.4 a key
    /// applies to every later segment until the next key tag — including
    /// across refresh boundaries.
    current_key: Option<m3u8_rs::Key>,
    /// Active `EXT-X-MAP` scope, propagated for the same reason as
    /// `current_key`: a map applies to every following segment until the next
    /// map tag (RFC 8216 §4.3.2.5).
    current_map: Option<m3u8_rs::Map>,
    /// Stateful Twitch ad detection (stitched-ad dateranges span snapshots).
    twitch: Option<TwitchPlaylistProcessor>,
    /// When true, SOOP `preloading` placeholder segments are skipped at plan
    /// time without removing them from the playlist (MSN indices stay stable).
    soop: bool,
}

impl PlannerContext {
    pub fn new(policy: SegmentIdentityPolicy, twitch: bool, soop: bool) -> Self {
        Self {
            policy,
            next_undecided_msn: None,
            byterange_chain: None,
            last_non_empty_segment_uri: None,
            current_key: None,
            current_map: None,
            twitch: twitch.then(TwitchPlaylistProcessor::new),
            soop,
        }
    }
}

/// One segment as the planner iterates it, after optional Twitch processing.
struct ScanSegment<'a> {
    segment: &'a m3u8_rs::MediaSegment,
    is_ad: bool,
    discontinuity: bool,
}

/// BYTERANGE inference anchor. `next_offset` is where a segment at `next_msn`
/// with matching `uri` and no explicit offset begins.
#[derive(Debug, Clone)]
struct ByteChain {
    uri: String,
    next_offset: u64,
    next_msn: u64,
}

pub fn plan(snapshot: &PlaylistSnapshot, ctx: &mut PlannerContext) -> Planned {
    let mut planned = Planned::default();
    let playlist = snapshot.playlist.as_ref();
    let window_start = playlist.media_sequence;
    let window_end = window_start.saturating_add(playlist.segments.len() as u64);

    // MSN-base gap: the window slid past undecided MSNs (possibly across
    // coalesced snapshot generations). Mark them missing — explicitly.
    if let Some(next_undecided) = ctx.next_undecided_msn {
        if window_start > next_undecided {
            debug!(
                from = next_undecided,
                to = window_start - 1,
                generation = snapshot.generation,
                "window slid past unplanned MSNs"
            );
            planned.missing.push((next_undecided, window_start - 1));
        } else if window_end < next_undecided {
            let window_len = (playlist.segments.len() as u64).max(1);
            let regression = next_undecided - window_end;
            if regression > window_len * 4 {
                // A genuine media-sequence reset (playlist restart). The
                // assembler's emit cursor cannot regress, so every re-based
                // payload would be stale-rejected and the stream would
                // silently download-and-discard forever. Surface a pipeline
                // error instead; the caller restarts a new session.
                warn!(
                    window_start,
                    window_end,
                    watermark = next_undecided,
                    "media-sequence reset detected; surfacing as pipeline error"
                );
                planned.reset = true;
            } else {
                // A stale window from a lagging CDN edge: nothing new can be
                // decided, and re-scanning would re-anchor the BYTERANGE
                // chain to old ranges and regress fetch URLs to older
                // generations. Plan nothing and wait for a fresh window.
                debug!(
                    window_start,
                    window_end,
                    watermark = next_undecided,
                    generation = snapshot.generation,
                    "stale playlist window; planning nothing"
                );
            }
            return planned;
        }
    }

    let base_url = Url::parse(snapshot.base_url.as_ref()).ok();
    let parent_params: Vec<(String, String)> = snapshot
        .parent_query
        .as_deref()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        })
        .unwrap_or_default();

    let scan: Vec<ScanSegment<'_>> = match ctx.twitch.as_mut() {
        Some(twitch) => twitch
            .process_playlist(playlist)
            .into_iter()
            .map(|p| ScanSegment {
                segment: p.segment,
                is_ad: p.is_ad,
                discontinuity: p.discontinuity,
            })
            .collect(),
        None => playlist
            .segments
            .iter()
            .map(|segment| ScanSegment {
                segment,
                is_ad: false,
                discontinuity: segment.discontinuity,
            })
            .collect(),
    };

    let playlist_level_map = parse_playlist_level_map(playlist);
    if playlist_level_map.is_some() {
        ctx.current_map = playlist_level_map;
    }
    // One init descriptor per distinct init *key* per plan() call (URI plus
    // byte range — a same-URI different-range map rotation is a distinct
    // resource); the store dedups across calls.
    let mut emitted_init_keys: HashSet<SegmentKey> = HashSet::new();
    let watermark = ctx.next_undecided_msn;
    let skip = |planned: &mut Planned, msn: u64| {
        // Coalesce contiguous skips produced within this scan.
        if let Some((_, to)) = planned.skipped.last_mut()
            && *to + 1 == msn
        {
            *to = msn;
        } else {
            planned.skipped.push((msn, msn));
        }
    };

    for (idx, scanned) in scan.iter().enumerate() {
        let msn = window_start + idx as u64;
        let deciding = watermark.is_none_or(|w| msn >= w);
        let segment = scanned.segment;

        // --- Encryption normalization (shared by init + media) ---
        // A key tag opens a scope covering every following segment until the
        // next tag; the parser only attaches it to the first one.
        if segment.key.is_some() {
            ctx.current_key = segment.key.clone();
        }
        let scoped_key = segment.key.as_ref().or(ctx.current_key.as_ref());
        let encryption = scoped_key
            .and_then(|key| normalize_encryption(key, msn, &base_url, &parent_params, &ctx.policy));
        let resolved_key = scoped_key.map(|key| {
            let mut key = key.clone();
            if let Some(uri) = key.uri.as_deref() {
                let absolute = resolve_uri(&base_url, uri).unwrap_or_else(|| uri.to_string());
                key.uri = Some(merge_params(&parent_params, &absolute));
            }
            key
        });

        // --- Init map (EXT-X-MAP) ---
        // A map tag opens a scope covering every following segment until the
        // next map tag (RFC 8216 §4.3.2.5); the parser attaches it only to
        // the segment it precedes, so the scope is propagated via current_map.
        // A playlist-level X-MAP re-seeds current_map at the start of each
        // snapshot; if a refresh omits it, the carried scope remains active.
        if segment.map.is_some() {
            ctx.current_map = segment.map.clone();
        }
        // The init key the media segments below this map depend on, so the
        // assembler can gate media emission on its arrival.
        let mut active_init_key: Option<SegmentKey> = None;
        if let Some(map_info) = ctx.current_map.clone()
            && let Some(absolute_map_uri) = resolve_uri(&base_url, &map_info.uri)
        {
            let final_map_uri = merge_params(&parent_params, &absolute_map_uri);
            if let Some(descriptor) = build_init_descriptor(
                &final_map_uri,
                &map_info,
                msn,
                scanned.discontinuity,
                init_map_encryption(encryption.clone()),
                resolved_key.clone(),
                &ctx.policy,
            ) {
                active_init_key = Some(descriptor.key.clone());
                // Emit one descriptor per distinct init key per call; the
                // store dedups across calls, so only the first occurrence in
                // this scan produces a descriptor.
                if emitted_init_keys.insert(descriptor.key.clone()) {
                    planned.descriptors.push(descriptor);
                }
            } else {
                warn!(msn, uri = %final_map_uri, "unparseable init map URI");
            }
        }

        // --- Effective URI (BYTERANGE entries may omit the URI) ---
        let effective_uri: Option<String> = if segment.uri.trim().is_empty() {
            if segment.byte_range.is_some() {
                ctx.last_non_empty_segment_uri.clone()
            } else {
                None
            }
        } else {
            ctx.last_non_empty_segment_uri = Some(segment.uri.clone());
            Some(segment.uri.clone())
        };
        let Some(effective_uri) = effective_uri.filter(|u| !u.trim().is_empty()) else {
            if deciding {
                warn!(msn, "skipping segment with empty URI");
                skip(&mut planned, msn);
            }
            continue;
        };

        // --- BYTERANGE offset resolution ---
        // The chain is keyed by MSN, so it advances deterministically through
        // every segment in the window — including already-decided ones below
        // the watermark — and a new tail segment infers from the true end of
        // its immediate predecessor, never a stale anchor. Re-emitting an
        // already-decided segment's descriptor is intentional: the store uses
        // it to refresh volatile fetch metadata (re-discovery is not a no-op).
        let mut byte_range_key: Option<ByteRangeKey> = None;
        match segment.byte_range.as_ref() {
            Some(range) => {
                let offset = match range.offset {
                    Some(offset) => Some(offset),
                    None => ctx
                        .byterange_chain
                        .as_ref()
                        .filter(|chain| chain.uri == effective_uri && chain.next_msn == msn)
                        .map(|chain| chain.next_offset),
                };
                match offset {
                    Some(offset) => {
                        byte_range_key = Some(ByteRangeKey {
                            length: range.length,
                            offset,
                        });
                        ctx.byterange_chain = Some(ByteChain {
                            uri: effective_uri.clone(),
                            next_offset: offset.saturating_add(range.length),
                            next_msn: msn + 1,
                        });
                    }
                    None => {
                        // Uninferable here: no explicit offset and no anchor
                        // matching this MSN. Only break the chain when it is
                        // not a *future* anchor (next_msn > msn). When an
                        // overlapping window is re-scanned and an already-
                        // decided offset-less segment leads while its explicit
                        // predecessor has slid out, the carried anchor points at
                        // a later MSN and the genuinely-new segment there still
                        // depends on it — clobbering it would skip that segment
                        // permanently. A stale anchor (next_msn <= msn) is
                        // genuinely broken and is cleared.
                        let chain_is_future_anchor = ctx
                            .byterange_chain
                            .as_ref()
                            .is_some_and(|chain| chain.next_msn > msn);
                        if !chain_is_future_anchor {
                            ctx.byterange_chain = None;
                        }
                        // Record the skip only while deciding so refresh does
                        // not re-emit it.
                        if deciding {
                            warn!(
                                msn,
                                uri = %effective_uri,
                                "skipping BYTERANGE segment with no explicit offset and no prior range to infer from"
                            );
                            skip(&mut planned, msn);
                        }
                        continue;
                    }
                }
            }
            None => {
                ctx.byterange_chain = None;
            }
        }

        // --- Ad filtering (planner policy; the watcher only preprocesses) ---
        if scanned.is_ad {
            if deciding {
                debug!(msn, uri = %segment.uri, "dropping ad segment");
                skip(&mut planned, msn);
            }
            continue;
        }

        // SOOP CDN playlists interleave placeholder segments whose URI contains
        // "preloading". They carry no broadcast content; skip download but keep
        // the playlist row so MSN = window_start + idx stays consistent.
        if ctx.soop && is_preloading_segment(segment) {
            if deciding {
                debug!(msn, uri = %segment.uri, "dropping SOOP preloading placeholder");
                skip(&mut planned, msn);
            }
            continue;
        }

        // --- Resolve, inherit query params, build identity ---
        let Some(absolute_uri) = resolve_uri(&base_url, &effective_uri) else {
            if deciding {
                warn!(msn, uri = %effective_uri, "skipping segment with unresolvable URI");
                skip(&mut planned, msn);
            }
            continue;
        };
        let final_uri = merge_params(&parent_params, &absolute_uri);
        let Ok(parsed_url) = Url::parse(&final_uri) else {
            // A malformed URL never produces a job (terminal by construction).
            if deciding {
                warn!(msn, uri = %final_uri, "skipping segment with malformed URL");
                skip(&mut planned, msn);
            }
            continue;
        };

        let source = if segment.title.as_deref() == Some(PREFETCH_SEGMENT_TITLE) {
            SegmentSource::PlaylistPrefetch
        } else {
            SegmentSource::Playlist
        };

        let mut media_segment = segment.clone();
        media_segment.uri = final_uri;
        media_segment.key = resolved_key;
        media_segment.byte_range = byte_range_key.map(|br| m3u8_rs::ByteRange {
            length: br.length,
            offset: Some(br.offset),
        });
        media_segment.discontinuity = scanned.discontinuity;

        trace!(msn, uri = %media_segment.uri, ?source, "planned segment");
        planned.descriptors.push(SegmentDescriptor {
            key: SegmentKey {
                kind: SegmentKind::Media,
                uri: ctx.policy.canonical_uri(&parsed_url),
                byte_range: byte_range_key,
            },
            msn,
            source,
            parsed_url: Arc::new(parsed_url),
            discontinuity: scanned.discontinuity,
            encryption,
            init_key: active_init_key,
            media_segment: Arc::new(media_segment),
        });
    }

    ctx.next_undecided_msn = Some(ctx.next_undecided_msn.unwrap_or(0).max(window_end));
    planned
}

fn build_init_descriptor(
    final_map_uri: &str,
    map_info: &m3u8_rs::Map,
    msn: u64,
    discontinuity: bool,
    encryption: Option<EncryptionDescriptor>,
    resolved_key: Option<m3u8_rs::Key>,
    policy: &SegmentIdentityPolicy,
) -> Option<SegmentDescriptor> {
    let parsed_url = Url::parse(final_map_uri).ok()?;
    // EXT-X-MAP BYTERANGE: a missing offset means "from the start of the
    // resource" (RFC 8216 §4.3.2.5), unlike media-segment BYTERANGE where it
    // means "continue the previous sub-range".
    let byte_range_key = map_info.byte_range.as_ref().map(|range| ByteRangeKey {
        length: range.length,
        offset: range.offset.unwrap_or(0),
    });
    let media_segment = m3u8_rs::MediaSegment {
        uri: final_map_uri.to_string(),
        duration: 0.0,
        byte_range: byte_range_key.map(|br| m3u8_rs::ByteRange {
            length: br.length,
            offset: Some(br.offset),
        }),
        discontinuity,
        key: resolved_key,
        map: None,
        ..Default::default()
    };
    Some(SegmentDescriptor {
        key: SegmentKey {
            kind: SegmentKind::Init,
            uri: policy.canonical_uri(&parsed_url),
            byte_range: byte_range_key,
        },
        // For init, msn is the MSN of the first segment the map covers —
        // assembler ordering metadata, never identity.
        msn,
        source: SegmentSource::Playlist,
        parsed_url: Arc::new(parsed_url),
        discontinuity,
        encryption,
        init_key: None,
        media_segment: Arc::new(media_segment),
    })
}

fn init_map_encryption(encryption: Option<EncryptionDescriptor>) -> Option<EncryptionDescriptor> {
    encryption.map(|mut enc| {
        if enc.method == EncryptionMethod::Aes128Cbc
            && matches!(enc.iv, EffectiveIv::MediaSequenceDerived(_))
        {
            // AES-128 encrypted init maps require an explicit IV. Treat a
            // missing IV as terminal unsupported crypto instead of deriving a
            // media-sequence IV that cannot decrypt the init section.
            enc.method =
                EncryptionMethod::Unsupported(Arc::from("AES-128 EXT-X-MAP without explicit IV"));
        }
        enc
    })
}

/// Normalize a playlist key tag into the typed encryption descriptor.
/// `KeyMethod::None` yields `None` (clear segment); anything the processor
/// cannot decrypt maps to `Unsupported`, which terminalizes the segment.
fn normalize_encryption(
    key: &m3u8_rs::Key,
    msn: u64,
    base_url: &Option<Url>,
    parent_params: &[(String, String)],
    policy: &SegmentIdentityPolicy,
) -> Option<EncryptionDescriptor> {
    let unsupported = |reason: String, fetch_url: Arc<Url>| EncryptionDescriptor {
        method: EncryptionMethod::Unsupported(Arc::from(reason)),
        key_identity_uri: Arc::from(""),
        key_fetch_url: fetch_url,
        iv: EffectiveIv::MediaSequenceDerived(msn),
        key_format: KeyFormat::Identity,
    };
    // A placeholder URL for unsupported descriptors that never fetch.
    let placeholder =
        || Arc::new(Url::parse("data:,unsupported").expect("static placeholder URL parses"));

    match &key.method {
        m3u8_rs::KeyMethod::None => None,
        m3u8_rs::KeyMethod::AES128 => {
            let Some(uri) = key.uri.as_deref().filter(|u| !u.trim().is_empty()) else {
                return Some(unsupported(
                    "AES-128 key tag without URI".to_string(),
                    placeholder(),
                ));
            };
            let absolute = resolve_uri(base_url, uri).unwrap_or_else(|| uri.to_string());
            let merged = merge_params(parent_params, &absolute);
            let Ok(fetch_url) = Url::parse(&merged) else {
                return Some(unsupported(
                    format!("unparseable key URI {merged}"),
                    placeholder(),
                ));
            };

            let iv = match key.iv.as_deref() {
                None => EffectiveIv::MediaSequenceDerived(msn),
                Some(iv_hex) => match parse_iv(iv_hex) {
                    Some(iv) => EffectiveIv::Explicit(iv),
                    None => {
                        return Some(unsupported(
                            format!("malformed AES-128 IV {iv_hex}"),
                            Arc::new(fetch_url),
                        ));
                    }
                },
            };

            let key_format = match key.keyformat.as_deref() {
                None | Some("identity") => KeyFormat::Identity,
                Some(other) => KeyFormat::Unsupported(Arc::from(other)),
            };

            let fetch_url = Arc::new(fetch_url);
            Some(EncryptionDescriptor {
                method: EncryptionMethod::Aes128Cbc,
                key_identity_uri: policy.canonical_uri(&fetch_url),
                key_fetch_url: fetch_url,
                iv,
                key_format,
            })
        }
        m3u8_rs::KeyMethod::SampleAES => {
            // SAMPLE-AES needs NAL/container-aware partial decryption, not a
            // cipher swap; it stays Unsupported until that path exists.
            Some(unsupported("SAMPLE-AES".to_string(), placeholder()))
        }
        m3u8_rs::KeyMethod::Other(name) => Some(unsupported(name.clone(), placeholder())),
    }
}

fn parse_iv(iv_hex: &str) -> Option<[u8; 16]> {
    let iv_str = iv_hex.trim_start_matches("0x").trim_start_matches("0X");
    let mut iv = [0u8; 16];
    hex::decode_to_slice(iv_str, &mut iv).ok()?;
    Some(iv)
}

fn resolve_uri(base_url: &Option<Url>, relative: &str) -> Option<String> {
    if relative.starts_with("http://") || relative.starts_with("https://") {
        return Some(relative.to_string());
    }
    base_url
        .as_ref()
        .and_then(|base| base.join(relative).ok())
        .map(|u| u.to_string())
}

/// Inherit query params from the media-playlist URL when the segment URL does
/// not already carry them.
fn merge_params(parent_params: &[(String, String)], uri: &str) -> String {
    if parent_params.is_empty() {
        return uri.to_string();
    }
    let Ok(mut url) = Url::parse(uri) else {
        return uri.to_string();
    };
    for (k, v) in parent_params {
        if url.query_pairs().any(|(existing, _)| existing == *k) {
            continue;
        }
        url.query_pairs_mut().append_pair(k, v);
    }
    url.to_string()
}

/// m3u8-rs only attaches EXT-X-MAP to `MediaSegment.map` when it appears in
/// the segment-scoped tag region; before the first segment it lands in
/// `MediaPlaylist.unknown_tags` as an `ExtTag` ("X-MAP").
fn parse_playlist_level_map(playlist: &m3u8_rs::MediaPlaylist) -> Option<m3u8_rs::Map> {
    let ext = playlist
        .unknown_tags
        .iter()
        .rev()
        .find(|t| t.tag == "X-MAP")?;
    let rest = ext.rest.as_deref()?;

    let mut uri: Option<String> = None;
    let mut byte_range: Option<m3u8_rs::ByteRange> = None;

    // Split on commas, but keep quoted values intact.
    let mut parts: Vec<&str> = Vec::new();
    let mut in_quotes = false;
    let mut start = 0usize;
    for (idx, ch) in rest.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(rest[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    if start < rest.len() {
        parts.push(rest[start..].trim());
    }

    for part in parts.into_iter().filter(|p| !p.is_empty()) {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        let key = k.trim();
        let mut val = v.trim();
        if let Some(stripped) = val.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            val = stripped;
        }

        if key.eq_ignore_ascii_case("URI") {
            uri = Some(val.to_string());
        } else if key.eq_ignore_ascii_case("BYTERANGE") {
            let (len_str, offset_str) = val.split_once('@').unwrap_or((val, ""));
            if let Ok(length) = len_str.trim().parse::<u64>() {
                let offset = if offset_str.trim().is_empty() {
                    None
                } else {
                    offset_str.trim().parse::<u64>().ok()
                };
                byte_range = Some(m3u8_rs::ByteRange { length, offset });
            }
        }
    }

    Some(m3u8_rs::Map {
        uri: uri?,
        byte_range,
        other_attributes: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hls::engine::identity::StripQueryIdentity;

    fn snapshot(generation: u64, input: &str) -> PlaylistSnapshot {
        snapshot_with_query(generation, input, None)
    }

    fn snapshot_with_query(
        generation: u64,
        input: &str,
        parent_query: Option<&str>,
    ) -> PlaylistSnapshot {
        let playlist = match m3u8_rs::parse_playlist_res(input.as_bytes()).expect("parses") {
            m3u8_rs::Playlist::MediaPlaylist(pl) => pl,
            m3u8_rs::Playlist::MasterPlaylist(_) => panic!("expected media playlist"),
        };
        PlaylistSnapshot {
            generation,
            playlist: Arc::new(playlist),
            base_url: Arc::from("https://example.com/path/"),
            parent_query: parent_query.map(Arc::from),
            terminal: None,
        }
    }

    fn ctx() -> PlannerContext {
        PlannerContext::new(SegmentIdentityPolicy::default(), false, false)
    }

    #[test]
    fn plans_media_segments_with_resolved_urls() {
        let snap = snapshot(
            0,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\nseg10.ts\n#EXTINF:2.0,\nseg11.ts\n",
        );
        let mut ctx = ctx();
        let planned = plan(&snap, &mut ctx);
        assert_eq!(planned.descriptors.len(), 2);
        assert_eq!(planned.descriptors[0].msn, 10);
        assert_eq!(
            planned.descriptors[0].parsed_url.as_str(),
            "https://example.com/path/seg10.ts"
        );
        assert!(planned.missing.is_empty());
        assert!(planned.skipped.is_empty());
    }

    #[test]
    fn detects_window_slide_as_missing_range() {
        let mut c = ctx();
        let snap1 = snapshot(
            0,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\nseg10.ts\n",
        );
        plan(&snap1, &mut c);
        // Coalesced refreshes: window jumped from 11 to 15.
        let snap2 = snapshot(
            3,
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:15\n#EXTINF:2.0,\nseg15.ts\n",
        );
        let planned = plan(&snap2, &mut c);
        assert_eq!(planned.missing, vec![(11, 14)]);
        assert_eq!(planned.descriptors.len(), 1);
        assert_eq!(planned.descriptors[0].msn, 15);
    }

    #[test]
    fn byterange_offsets_resolve_and_chain_within_snapshot() {
        let snap = snapshot(
            0,
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n",
        );
        let mut c = ctx();
        let planned = plan(&snap, &mut c);
        assert_eq!(planned.descriptors.len(), 2);
        assert_eq!(
            planned.descriptors[0].key.byte_range,
            Some(ByteRangeKey {
                length: 10,
                offset: 0
            })
        );
        assert_eq!(
            planned.descriptors[1].key.byte_range,
            Some(ByteRangeKey {
                length: 5,
                offset: 10
            })
        );
        // Distinct ranges at one URI are distinct keys.
        assert_ne!(planned.descriptors[0].key, planned.descriptors[1].key);
    }

    #[test]
    fn byterange_chain_survives_refresh_boundary() {
        let mut c = ctx();
        let snap1 = snapshot(
            0,
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n",
        );
        plan(&snap1, &mut c);
        // The next snapshot's first segment continues the chain with no
        // explicit offset.
        let snap2 = snapshot(
            1,
            "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:2\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n",
        );
        let planned = plan(&snap2, &mut c);
        assert_eq!(planned.descriptors.len(), 1);
        assert_eq!(
            planned.descriptors[0].key.byte_range,
            Some(ByteRangeKey {
                length: 5,
                offset: 10
            })
        );
    }

    #[test]
    fn refresh_reemits_stable_keys_including_inferred_byterange() {
        let mut c = ctx();
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n";
        let first = plan(&snapshot(0, body), &mut c);
        // Same window re-planned (refresh): BOTH segments re-emit with
        // identical keys. Re-emitting already-decided segments is required so
        // the store refreshes their fetch metadata; the MSN-keyed chain keeps
        // the inferred offset deterministic across the re-scan.
        let second = plan(&snapshot(1, body), &mut c);
        assert_eq!(second.descriptors.len(), 2);
        assert_eq!(second.descriptors[0].key, first.descriptors[0].key);
        assert_eq!(second.descriptors[1].key, first.descriptors[1].key);
        assert!(second.skipped.is_empty());
    }

    #[test]
    fn window_slide_preserves_byterange_anchor_for_new_leading_inferred_segment() {
        // Regression: when the window slides so an already-decided offset-less
        // segment leads (its explicit predecessor slid out), re-scanning it
        // must not clobber the carried anchor that the genuinely-new segment
        // after it depends on.
        let mut c = ctx();
        // gen0: msn1 explicit @0 (len 10), msn2 inferred (len 5 -> offset 10).
        plan(
            &snapshot(
                0,
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n",
            ),
            &mut c,
        );
        // gen1: window slid to msn2-3; msn2 leads (offset-less, already
        // decided), msn3 is new (offset-less, must infer 15).
        let g1 = plan(
            &snapshot(
                1,
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:2\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:7\nfile.ts\n",
            ),
            &mut c,
        );
        let tail = g1
            .descriptors
            .iter()
            .find(|d| d.msn == 3)
            .expect("new live-edge segment must be planned, not skipped");
        assert_eq!(
            tail.key.byte_range,
            Some(ByteRangeKey {
                length: 7,
                offset: 15,
            })
        );
        assert!(g1.skipped.is_empty(), "new segment must not be skipped");
    }

    #[test]
    fn new_tail_byterange_infers_from_true_predecessor_end_on_refresh() {
        // Regression guard: a window whose explicit @0 anchor stays in-window
        // while a new offset-less tail segment appears must infer from the
        // real end of the segment before it, not a stale anchor.
        let mut c = ctx();
        let body1 = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n";
        plan(&snapshot(0, body1), &mut c);
        let body2 = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:10@0\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:7\nfile.ts\n";
        let second = plan(&snapshot(1, body2), &mut c);
        let tail = second
            .descriptors
            .iter()
            .find(|d| d.msn == 3)
            .expect("new tail segment planned");
        assert_eq!(
            tail.key.byte_range,
            Some(ByteRangeKey {
                length: 7,
                offset: 15, // 0..10, 10..15, 15..22 — NOT the stale 10
            })
        );
    }

    #[test]
    fn uninferable_byterange_is_skipped_once() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:5\n#EXTINF:2.0,\n#EXT-X-BYTERANGE:5\nfile.ts\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        assert!(planned.descriptors.is_empty());
        assert_eq!(planned.skipped, vec![(5, 5)]);
        // Refresh: decision is not re-emitted.
        let planned = plan(&snapshot(1, body), &mut c);
        assert!(planned.skipped.is_empty());
    }

    #[test]
    fn rotated_auth_param_resolves_to_same_key_under_policy() {
        let policy = SegmentIdentityPolicy::StripQuery(StripQueryIdentity::new(["token"]));
        let mut c = PlannerContext::new(policy, false, false);
        let body_v1 = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nseg1.ts?token=aaa\n";
        let body_v2 = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nseg1.ts?token=bbb\n";
        let first = plan(&snapshot(0, body_v1), &mut c);
        let second = plan(&snapshot(1, body_v2), &mut c);
        assert_eq!(first.descriptors[0].key, second.descriptors[0].key);
        // The fetch URL is volatile and reflects the latest token.
        assert_ne!(
            first.descriptors[0].parsed_url,
            second.descriptors[0].parsed_url
        );
    }

    #[test]
    fn inherits_parent_query_params() {
        let body = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nseg1.ts\n";
        let mut c = ctx();
        let planned = plan(&snapshot_with_query(0, body, Some("auth=tok1")), &mut c);
        assert_eq!(
            planned.descriptors[0].parsed_url.as_str(),
            "https://example.com/path/seg1.ts?auth=tok1"
        );
    }

    #[test]
    fn emits_init_descriptor_with_distinct_kind() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:7\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nseg7.m4s\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        assert_eq!(planned.descriptors.len(), 2);
        let init = &planned.descriptors[0];
        assert_eq!(init.key.kind, SegmentKind::Init);
        assert_eq!(init.msn, 7, "init msn = first covered segment");
        let media = &planned.descriptors[1];
        assert_eq!(media.key.kind, SegmentKind::Media);
        // Same-MSN init and media never collide.
        assert_ne!(init.key, media.key);
    }

    #[test]
    fn playlist_level_map_rotation_reseeds_init_scope() {
        let mut c = ctx();
        let first = plan(
            &snapshot(
                0,
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"init1.mp4\"\n#EXTINF:2.0,\nseg0.m4s\n",
            ),
            &mut c,
        );
        let second = plan(
            &snapshot(
                1,
                "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-MAP:URI=\"init2.mp4\"\n#EXTINF:2.0,\nseg1.m4s\n",
            ),
            &mut c,
        );

        let first_media = first
            .descriptors
            .iter()
            .find(|d| d.key.kind == SegmentKind::Media)
            .expect("first media planned");
        let second_media = second
            .descriptors
            .iter()
            .find(|d| d.key.kind == SegmentKind::Media)
            .expect("second media planned");
        assert_eq!(
            first_media.init_key.as_ref().map(|key| key.uri.as_ref()),
            Some("https://example.com/path/init1.mp4")
        );
        assert_eq!(
            second_media.init_key.as_ref().map(|key| key.uri.as_ref()),
            Some("https://example.com/path/init2.mp4")
        );
    }

    #[test]
    fn prefetch_title_sets_source_but_not_identity() {
        let body = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.002,{PREFETCH_SEGMENT_TITLE}\nhttps://example.com/path/pre.ts\n"
        );
        let mut c = ctx();
        let planned = plan(&snapshot(0, &body), &mut c);
        assert_eq!(planned.descriptors.len(), 1);
        assert_eq!(
            planned.descriptors[0].source,
            SegmentSource::PlaylistPrefetch
        );

        // The same URI later as a normal media segment: identical key.
        let body2 = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nhttps://example.com/path/pre.ts\n";
        let mut c2 = ctx();
        let planned2 = plan(&snapshot(0, body2), &mut c2);
        assert_eq!(planned2.descriptors[0].source, SegmentSource::Playlist);
        assert_eq!(planned.descriptors[0].key, planned2.descriptors[0].key);
    }

    #[test]
    fn aes128_key_normalizes_with_msn_derived_iv() {
        let body = "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:42\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXTINF:2.0,\nseg42.ts\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        let enc = planned.descriptors[0]
            .encryption
            .as_ref()
            .expect("encrypted");
        assert_eq!(enc.method, EncryptionMethod::Aes128Cbc);
        assert_eq!(enc.iv, EffectiveIv::MediaSequenceDerived(42));
        assert_eq!(
            enc.key_fetch_url.as_str(),
            "https://example.com/path/key.bin"
        );
        assert_eq!(enc.key_format, KeyFormat::Identity);
    }

    #[test]
    fn aes128_init_map_without_explicit_iv_is_unsupported() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:42\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nseg42.m4s\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        let init = planned
            .descriptors
            .iter()
            .find(|d| d.key.kind == SegmentKind::Init)
            .expect("init descriptor planned");
        let init_enc = init.encryption.as_ref().expect("init marked encrypted");
        assert!(matches!(
            init_enc.method,
            EncryptionMethod::Unsupported(ref reason)
                if reason.as_ref() == "AES-128 EXT-X-MAP without explicit IV"
        ));

        let media = planned
            .descriptors
            .iter()
            .find(|d| d.key.kind == SegmentKind::Media)
            .expect("media descriptor planned");
        let media_enc = media.encryption.as_ref().expect("media marked encrypted");
        assert_eq!(media_enc.method, EncryptionMethod::Aes128Cbc);
        assert_eq!(media_enc.iv, EffectiveIv::MediaSequenceDerived(42));
    }

    #[test]
    fn aes128_init_map_with_explicit_iv_stays_decryptable() {
        let body = "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:42\n#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x0000000000000000000000000000002A\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:2.0,\nseg42.m4s\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        let init = planned
            .descriptors
            .iter()
            .find(|d| d.key.kind == SegmentKind::Init)
            .expect("init descriptor planned");
        let enc = init.encryption.as_ref().expect("init marked encrypted");
        assert_eq!(enc.method, EncryptionMethod::Aes128Cbc);
        assert_eq!(enc.iv, EffectiveIv::Explicit(42u128.to_be_bytes()));
    }

    #[test]
    fn sample_aes_maps_to_unsupported() {
        let body = "#EXTM3U\n#EXT-X-VERSION:5\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"key.bin\"\n#EXTINF:2.0,\nseg1.ts\n";
        let mut c = ctx();
        let planned = plan(&snapshot(0, body), &mut c);
        let enc = planned.descriptors[0].encryption.as_ref().expect("marked");
        assert!(matches!(enc.method, EncryptionMethod::Unsupported(_)));
    }

    #[test]
    fn msn_reset_surfaces_as_pipeline_reset() {
        let mut c = ctx();
        plan(
            &snapshot(
                0,
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1000\n#EXTINF:2.0,\nseg1000.ts\n",
            ),
            &mut c,
        );
        // A window regressing far below the watermark is a media-sequence
        // reset. Continuity cannot be preserved (re-based payloads would be
        // stale-rejected forever), so it is surfaced as a reset, not silently
        // re-planned.
        let planned = plan(
            &snapshot(
                1,
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:1\n#EXTINF:2.0,\nrestart1.ts\n",
            ),
            &mut c,
        );
        assert!(planned.reset, "reset must be flagged");
        assert!(planned.descriptors.is_empty());
        assert!(planned.missing.is_empty());
    }

    #[test]
    fn small_window_regression_plans_nothing_and_does_not_reset() {
        let mut c = ctx();
        // Establish watermark at 13 (segments 10,11,12).
        plan(
            &snapshot(
                0,
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:10\n#EXTINF:2.0,\ns10.ts\n#EXTINF:2.0,\ns11.ts\n#EXTINF:2.0,\ns12.ts\n",
            ),
            &mut c,
        );
        // A stale edge serves an older window (8,9). Regression (13-10=3) is
        // within 4x the window length: plan nothing, do not reset, do not
        // emit a bogus missing range.
        let planned = plan(
            &snapshot(
                1,
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:8\n#EXTINF:2.0,\ns8.ts\n#EXTINF:2.0,\ns9.ts\n",
            ),
            &mut c,
        );
        assert!(!planned.reset);
        assert!(planned.descriptors.is_empty());
        assert!(planned.missing.is_empty());

        // A fresh window resuming at the live edge must NOT report the
        // already-planned MSNs as missing.
        let planned = plan(
            &snapshot(
                2,
                "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:2\n#EXT-X-MEDIA-SEQUENCE:13\n#EXTINF:2.0,\ns13.ts\n",
            ),
            &mut c,
        );
        assert_eq!(planned.descriptors.len(), 1);
        assert_eq!(planned.descriptors[0].msn, 13);
        assert!(
            planned.missing.is_empty(),
            "stale regression must not manufacture a missing range over decided MSNs"
        );
    }
}
