//! Conservative random-access validation for independently playable fMP4 cuts.

use std::collections::{HashMap, HashSet};

use bytes::Bytes;

use crate::box_utils::{BoxView, box_at, find_first_box};

fn word(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::make_box;

    fn init(video: bool, default_flags: u32) -> Bytes {
        let mut tkhd = vec![0; 16];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        let mut hdlr = vec![0; 12];
        hdlr[8..12].copy_from_slice(if video { b"vide" } else { b"soun" });
        let mut trak = make_box(b"tkhd", &tkhd);
        trak.extend(make_box(b"mdia", &make_box(b"hdlr", &hdlr)));
        let mut trex = vec![0; 24];
        trex[4..8].copy_from_slice(&1u32.to_be_bytes());
        trex[20..24].copy_from_slice(&default_flags.to_be_bytes());
        let mut moov = make_box(b"trak", &trak);
        moov.extend(make_box(b"mvex", &make_box(b"trex", &trex)));
        Bytes::from(make_box(b"moov", &moov))
    }

    fn media(first_flags: Option<u32>) -> Bytes {
        let mut trun = if first_flags.is_some() { 4u32 } else { 0 }
            .to_be_bytes()
            .to_vec();
        trun.extend_from_slice(&1u32.to_be_bytes());
        if let Some(flags) = first_flags {
            trun.extend_from_slice(&flags.to_be_bytes());
        }
        fragment(trun)
    }

    /// A run of `samples` whose first sample flags come from `first_flags`
    /// (trun flag 0x04) and whose later samples use the track defaults.
    fn run_with_first_flags(first_flags: u32, samples: u32) -> Bytes {
        let mut trun = 4u32.to_be_bytes().to_vec();
        trun.extend_from_slice(&samples.to_be_bytes());
        trun.extend_from_slice(&first_flags.to_be_bytes());
        fragment(trun)
    }

    /// A run with per-sample durations and flags (trun flags 0x100 | 0x400).
    fn run_with_sample_flags(flags: &[u32]) -> Bytes {
        let mut trun = 0x0500u32.to_be_bytes().to_vec();
        trun.extend_from_slice(&(flags.len() as u32).to_be_bytes());
        for flags in flags {
            trun.extend_from_slice(&40u32.to_be_bytes());
            trun.extend_from_slice(&flags.to_be_bytes());
        }
        fragment(trun)
    }

    fn fragment(trun: Vec<u8>) -> Bytes {
        fragment_for_track(1, trun)
    }

    fn fragment_for_track(track_id: u32, trun: Vec<u8>) -> Bytes {
        let mut tfhd = vec![0x00, 0x02, 0, 0];
        tfhd.extend_from_slice(&track_id.to_be_bytes());
        let mut traf = make_box(b"tfhd", &tfhd);
        traf.extend(make_box(b"trun", &trun));
        let mut result = make_box(b"moof", &make_box(b"traf", &traf));
        result.extend(make_box(b"mdat", &[1, 2, 3, 4]));
        Bytes::from(result)
    }

    /// Track 1 is video with non-sync default flags, track 2 is audio.
    fn video_and_audio_init() -> Bytes {
        let trak = |id: u32, handler: &[u8; 4]| {
            let mut tkhd = vec![0; 16];
            tkhd[12..16].copy_from_slice(&id.to_be_bytes());
            let mut hdlr = vec![0; 12];
            hdlr[8..12].copy_from_slice(handler);
            let mut trak = make_box(b"tkhd", &tkhd);
            trak.extend(make_box(b"mdia", &make_box(b"hdlr", &hdlr)));
            make_box(b"trak", &trak)
        };
        let trex = |id: u32, flags: u32| {
            let mut trex = vec![0; 24];
            trex[4..8].copy_from_slice(&id.to_be_bytes());
            trex[20..24].copy_from_slice(&flags.to_be_bytes());
            make_box(b"trex", &trex)
        };
        let mvex = make_box(b"mvex", &[trex(1, 0x0101_0000), trex(2, 0)].concat());
        Bytes::from(make_box(
            b"moov",
            &[trak(1, b"vide"), trak(2, b"soun"), mvex].concat(),
        ))
    }

    fn first_flags_run(first_flags: u32) -> Vec<u8> {
        let mut trun = 4u32.to_be_bytes().to_vec();
        trun.extend_from_slice(&2u32.to_be_bytes());
        trun.extend_from_slice(&first_flags.to_be_bytes());
        trun
    }

    /// Merged DASH output carries one fragment per track; a cut point needs
    /// every track's first fragment in the item, not every track in the
    /// first `moof`.
    #[test]
    fn tracks_in_separate_fragments_are_all_considered() {
        let check = IndependentFragmentCheck::from_init(&video_and_audio_init()).unwrap();
        let video_sync = fragment_for_track(1, first_flags_run(0x0200_0000));
        let video_inter = fragment_for_track(1, first_flags_run(0x0101_0000));
        let audio = fragment_for_track(2, first_flags_run(0));
        let item = |parts: &[&Bytes]| {
            Bytes::from(
                parts
                    .iter()
                    .flat_map(|part| part.to_vec())
                    .collect::<Vec<_>>(),
            )
        };

        assert!(check.is_independent(&item(&[&video_sync, &audio])));
        assert!(check.is_independent(&item(&[&audio, &video_sync])));
        // Only the first fragment of a track decides.
        assert!(check.is_independent(&item(&[&video_sync, &video_inter, &audio])));
        assert!(!check.is_independent(&item(&[&video_inter, &video_sync, &audio])));
        // Every track must be present.
        assert!(!check.is_independent(&video_sync));
        assert!(!check.is_independent(&item(&[&audio, &audio])));

        // A moof cannot borrow the media data of the fragment after it.
        let without_mdat = |fragment: &Bytes| {
            let moof_len = u32::from_be_bytes(fragment[..4].try_into().unwrap()) as usize;
            fragment.slice(..moof_len)
        };
        assert!(!check.is_independent(&item(&[&without_mdat(&video_sync), &audio])));
    }

    #[test]
    fn manual_cut_requires_a_sync_sample_not_merely_a_moof() {
        let check = IndependentFragmentCheck::from_init(&init(true, 0x0101_0000)).unwrap();
        assert!(!check.is_independent(&media(None)));
        assert!(!check.is_independent(&media(Some(0))));
        assert!(!check.is_independent(&media(Some(0x0201_0000))));
        assert!(check.is_independent(&media(Some(0x0200_0000))));
        assert!(
            IndependentFragmentCheck::from_init(&init(true, 0x0200_0000))
                .unwrap()
                .is_independent(&media(None))
        );
        assert!(
            IndependentFragmentCheck::from_init(&init(false, 0))
                .unwrap()
                .is_independent(&media(None))
        );
    }

    #[test]
    fn sync_samples_with_unknown_dependency_need_non_sync_signalling_in_the_run() {
        let signalled = IndependentFragmentCheck::from_init(&init(true, 0x0101_0000)).unwrap();
        let unsignalled = IndependentFragmentCheck::from_init(&init(true, 0)).unwrap();
        // First-sample flags with later samples marked non-sync by default.
        assert!(signalled.is_independent(&run_with_first_flags(0, 2)));
        assert!(!signalled.is_independent(&run_with_first_flags(0, 1)));
        // All flags zero: nothing distinguishes a keyframe from an inter frame.
        assert!(!unsignalled.is_independent(&run_with_first_flags(0, 2)));
        assert!(!unsignalled.is_independent(&media(None)));
        // Per-sample flags.
        assert!(unsignalled.is_independent(&run_with_sample_flags(&[0, 0x0101_0000])));
        assert!(!unsignalled.is_independent(&run_with_sample_flags(&[0, 0])));
        assert!(!unsignalled.is_independent(&run_with_sample_flags(&[0x0101_0000, 0])));
        // An explicit dependency or a non-sync first sample is never a cut point.
        assert!(!signalled.is_independent(&run_with_first_flags(0x0100_0000, 2)));
        assert!(!signalled.is_independent(&run_with_first_flags(0x0001_0000, 2)));
        // Any later non-sync sample in the run is sufficient evidence.
        let complete = run_with_sample_flags(&[0, 0, 0x0101_0000]);
        assert!(unsignalled.is_independent(&complete));
    }

    #[test]
    fn manual_cut_rejects_truncated_boxes_and_missing_initialization() {
        let init = init(true, 0x0200_0000);
        for end in 0..init.len() {
            assert!(IndependentFragmentCheck::from_init(&init.slice(..end)).is_none());
        }
        let check = IndependentFragmentCheck::from_init(&init).unwrap();
        let fragment = media(None);
        for end in 0..fragment.len() {
            assert!(!check.is_independent(&fragment.slice(..end)));
        }
    }

    #[test]
    fn standalone_cmaf_fragment_checks_walk_past_leading_boxes_and_reject_non_idr_parts() {
        let prefix = [
            make_box(b"styp", b"msdh\0\0\0\0msdhmsix"),
            make_box(b"sidx", &[0; 24]),
            make_box(b"prft", &[0; 16]),
            make_box(b"emsg", &[0; 12]),
        ]
        .concat();
        let with_prefix = |fragment: &Bytes| {
            let mut buf = prefix.clone();
            buf.extend_from_slice(fragment);
            Bytes::from(buf)
        };

        let idr = with_prefix(&fragment_for_track(1, first_flags_run(0x0200_0000)));
        assert!(has_movie_fragment(&idr));
        assert!(fragment_has_sync_sample(&idr));

        // Non-IDR LL-HLS partial segments start with `moof` (so they are
        // fragment starts) but carry no sync sample.
        let non_idr_explicit = with_prefix(&fragment_for_track(1, first_flags_run(0x0101_0000)));
        assert!(has_movie_fragment(&non_idr_explicit));
        assert!(!fragment_has_sync_sample(&non_idr_explicit));

        let non_idr_inherited = with_prefix(&media(None));
        assert!(has_movie_fragment(&non_idr_inherited));
        assert!(!fragment_has_sync_sample(&non_idr_inherited));

        // In a muxed partial fragment, sync audio samples must not mask an
        // inter-only video track.
        let audio_sync = {
            let mut tfhd = vec![0x00, 0x02, 0x00, 0x20];
            tfhd.extend_from_slice(&2u32.to_be_bytes());
            tfhd.extend_from_slice(&0x0200_0000u32.to_be_bytes());
            let mut trun = 0u32.to_be_bytes().to_vec();
            trun.extend_from_slice(&2u32.to_be_bytes());
            let mut traf = make_box(b"tfhd", &tfhd);
            traf.extend(make_box(b"trun", &trun));
            traf
        };
        let video_inter_traf = {
            let mut tfhd = vec![0x00, 0x02, 0x00, 0x20];
            tfhd.extend_from_slice(&1u32.to_be_bytes());
            tfhd.extend_from_slice(&0x0101_0000u32.to_be_bytes());
            let mut trun = 0u32.to_be_bytes().to_vec();
            trun.extend_from_slice(&2u32.to_be_bytes());
            let mut traf = make_box(b"tfhd", &tfhd);
            traf.extend(make_box(b"trun", &trun));
            traf
        };
        let mut muxed_non_idr = prefix;
        muxed_non_idr.extend(make_box(b"moof", &[video_inter_traf, audio_sync].concat()));
        muxed_non_idr.extend(make_box(b"mdat", &[1, 2, 3, 4]));
        let muxed_non_idr = Bytes::from(muxed_non_idr);
        assert!(has_movie_fragment(&muxed_non_idr));
        assert!(!fragment_has_sync_sample(&muxed_non_idr));

        let mdat_only = Bytes::from(make_box(b"mdat", &[1, 2, 3, 4]));
        assert!(!has_movie_fragment(&mdat_only));
        assert!(!fragment_has_sync_sample(&mdat_only));
    }
}

/// `sample_is_non_sync_sample` in ISO/IEC 14496-12 sample flags.
const NON_SYNC: u32 = 0x0001_0000;

/// Returns whether `data` contains a well-formed top-level `moof` box,
/// walking past leading segment boxes such as `styp`, `sidx`, `prft`, or
/// `emsg` (ISO/IEC 14496-12 §4.2, §8.8.4).
pub fn has_movie_fragment(data: &Bytes) -> bool {
    let mut offset = 0;
    while offset < data.len() {
        let Some(item) = box_at(data, offset, data.len()) else {
            return false;
        };
        if item.fourcc == *b"moof" {
            return true;
        }
        offset = item.end;
    }
    false
}

/// Returns whether an fMP4 media segment or LL-HLS partial segment contains a
/// sync sample, inspecting `moof`/`traf`/`tfhd`/`trun` sample flags without
/// requiring the initialization segment (ISO/IEC 14496-12 §8.8.3.1, §8.8.7,
/// §8.8.8).
///
/// Leading boxes (`styp`, `sidx`, `prft`, `emsg`, etc.) are skipped via
/// top-level box walking. For muxed multi-track fragments, any track carrying
/// explicitly non-sync/dependent samples must also carry at least one sync
/// sample in the segment, preventing sync audio tracks from masking non-IDR
/// video partial fragments.
///
/// Note: when a muxer omits both `tfhd` default sample flags (`0x000020`) and
/// `trun` sample flags (`0x000004` / `0x000400`) and relies exclusively on
/// `moov/mvex/trex` defaults, standalone `moof` inspection returns `false`;
/// use [`IndependentFragmentCheck`] when the initialization segment is known.
pub fn fragment_has_sync_sample(data: &Bytes) -> bool {
    check_fragment_sync_samples(data).unwrap_or(false)
}

#[derive(Default)]
struct TrackSyncSummary {
    has_samples: bool,
    has_sync: bool,
}

fn check_fragment_sync_samples(data: &Bytes) -> Option<bool> {
    let mut tracks: HashMap<u32, TrackSyncSummary> = HashMap::new();
    let mut offset = 0;
    while offset < data.len() {
        let item = box_at(data, offset, data.len())?;
        offset = item.end;
        if item.fourcc != *b"moof" {
            continue;
        }

        let mut next = item.end;
        let mdat = loop {
            let following = box_at(data, next, data.len())?;
            match &following.fourcc {
                b"mdat" => break following,
                b"moof" => return Some(false),
                _ => next = following.end,
            }
        };
        if mdat.body_start == mdat.end {
            return Some(false);
        }

        let mut traf_offset = item.body_start;
        while traf_offset < item.end {
            let traf = box_at(data, traf_offset, item.end)?;
            traf_offset = traf.end;
            if traf.fourcc != *b"traf" {
                continue;
            }
            inspect_traf_sync_samples(data, &traf, &mut tracks)?;
        }
    }

    let any_sync = tracks.values().any(|t| t.has_sync);
    let all_sampled_sync = tracks
        .values()
        .filter(|t| t.has_samples)
        .all(|t| t.has_sync);
    Some(any_sync && all_sampled_sync)
}

fn inspect_traf_sync_samples(
    data: &Bytes,
    traf: &BoxView,
    tracks: &mut HashMap<u32, TrackSyncSummary>,
) -> Option<()> {
    let tfhd = find_first_box(data, traf.body_start, traf.end, *b"tfhd")?;
    let tfhd_body = &data[tfhd.body_start..tfhd.end];
    let version_and_flags = word(tfhd_body, 0)?;
    if (version_and_flags >> 24) != 0 {
        return None;
    }
    let tfhd_flags = version_and_flags & 0x00ff_ffff;
    let track_id = word(tfhd_body, 4)?;
    if tfhd_flags & 0x010000 != 0 {
        return Some(());
    }

    let mut field = 8;
    if tfhd_flags & 0x000001 != 0 {
        word(tfhd_body, field)?;
        word(tfhd_body, field + 4)?;
        field += 8;
    }
    for flag in [0x000002, 0x000008, 0x000010] {
        if tfhd_flags & flag != 0 {
            word(tfhd_body, field)?;
            field += 4;
        }
    }
    let default_flags = if tfhd_flags & 0x000020 != 0 {
        Some(word(tfhd_body, field)?)
    } else {
        None
    };

    let summary = tracks.entry(track_id).or_default();
    let mut child_offset = traf.body_start;
    while child_offset < traf.end {
        let child = box_at(data, child_offset, traf.end)?;
        child_offset = child.end;
        if child.fourcc != *b"trun" {
            continue;
        }
        inspect_trun_sync_samples(&data[child.body_start..child.end], default_flags, summary)?;
    }
    Some(())
}

fn inspect_trun_sync_samples(
    body: &[u8],
    default_flags: Option<u32>,
    summary: &mut TrackSyncSummary,
) -> Option<()> {
    let version_and_flags = word(body, 0)?;
    if (version_and_flags >> 24) > 1 {
        return None;
    }
    let flags = version_and_flags & 0x00ff_ffff;
    let sample_count = word(body, 4)?;
    if sample_count == 0 {
        return Some(());
    }
    summary.has_samples = true;

    let mut field = 8;
    if flags & 0x000001 != 0 {
        word(body, field)?;
        field += 4;
    }
    let first_sample_flags = if flags & 0x000004 != 0 {
        if flags & 0x000400 != 0 {
            return None;
        }
        let first = word(body, field)?;
        field += 4;
        Some(first)
    } else {
        None
    };

    let rows = field;
    let has_per_sample_flags = flags & 0x000400 != 0;
    let row_stride = [0x000100, 0x000200, 0x000400, 0x000800]
        .into_iter()
        .filter(|flag| flags & flag != 0)
        .count()
        * 4;
    let flags_in_row = [0x000100, 0x000200]
        .into_iter()
        .filter(|flag| flags & flag != 0)
        .count()
        * 4;

    if row_stride > 0 {
        let total_rows_bytes = usize::try_from(sample_count)
            .ok()?
            .checked_mul(row_stride)?;
        if rows.checked_add(total_rows_bytes)? > body.len() {
            return None;
        }
    }

    let sample_flags_at = |index: usize| -> Option<Option<u32>> {
        if index == 0
            && let Some(first) = first_sample_flags
        {
            return Some(Some(first));
        }
        if has_per_sample_flags {
            let offset = rows
                .checked_add(index.checked_mul(row_stride)?)?
                .checked_add(flags_in_row)?;
            return Some(Some(word(body, offset)?));
        }
        Some(default_flags)
    };

    // Determine whether this run signals non-sync samples anywhere, allowing a
    // sync sample with `sample_depends_on == 0` (unknown) to be distinguished
    // from a muxer that leaves all sample flags at zero.
    let count = usize::try_from(sample_count).ok()?;
    let run_signals_non_sync = if has_per_sample_flags {
        let mut found = false;
        for index in 0..count {
            if let Some(sample_flags) = sample_flags_at(index)?
                && sample_flags & NON_SYNC != 0
            {
                found = true;
                break;
            }
        }
        found
    } else {
        (count > 1 && default_flags.is_some_and(|f| f & NON_SYNC != 0))
            || first_sample_flags.is_some_and(|f| f & NON_SYNC != 0)
            || (count == 1
                && first_sample_flags.is_none()
                && default_flags.is_some_and(|f| f & NON_SYNC != 0))
    };

    if has_per_sample_flags {
        for index in 0..count {
            if let Some(sample_flags) = sample_flags_at(index)? {
                record_sample_flags(sample_flags, run_signals_non_sync, summary);
            }
        }
    } else {
        if let Some(sample_flags) = sample_flags_at(0)? {
            record_sample_flags(sample_flags, run_signals_non_sync, summary);
        }
        if count > 1
            && let Some(rest_flags) = default_flags
        {
            record_sample_flags(rest_flags, run_signals_non_sync, summary);
        }
    }

    Some(())
}

fn record_sample_flags(flags: u32, run_signals_non_sync: bool, summary: &mut TrackSyncSummary) {
    let depends_on = (flags >> 24) & 3;
    if flags & NON_SYNC != 0 || depends_on == 1 {
        return;
    }
    if depends_on == 2 || (depends_on == 0 && run_signals_non_sync) {
        summary.has_sync = true;
    }
}

#[derive(Debug, Clone)]
struct Track {
    video: bool,
    default_flags: Option<u32>,
}

/// Parsed once per initialization segment. Unknown/malformed track layouts
/// fail closed; a `moof` box alone is never evidence of a random-access sample.
#[derive(Debug, Clone, Default)]
pub struct IndependentFragmentCheck {
    tracks: HashMap<u32, Track>,
}

impl IndependentFragmentCheck {
    pub fn from_init(data: &Bytes) -> Option<Self> {
        let moov = find_first_box(data, 0, data.len(), *b"moov")?;
        let mut tracks = HashMap::new();
        let mut offset = moov.body_start;
        while offset < moov.end {
            let item = box_at(data, offset, moov.end)?;
            offset = item.end;
            if item.fourcc != *b"trak" {
                continue;
            }
            let tkhd = find_first_box(data, item.body_start, item.end, *b"tkhd")?;
            let body = &data[tkhd.body_start..tkhd.end];
            let id = word(
                body,
                match *body.first()? {
                    0 => 12,
                    1 => 20,
                    _ => return None,
                },
            )?;
            let mdia = find_first_box(data, item.body_start, item.end, *b"mdia")?;
            let hdlr = find_first_box(data, mdia.body_start, mdia.end, *b"hdlr")?;
            let handler = data.get(hdlr.body_start..hdlr.end)?.get(8..12)?;
            let video = match handler {
                b"vide" => true,
                b"soun" => false,
                _ => return None,
            };
            if tracks
                .insert(
                    id,
                    Track {
                        video,
                        default_flags: None,
                    },
                )
                .is_some()
            {
                return None;
            }
        }
        let mvex = find_first_box(data, moov.body_start, moov.end, *b"mvex")?;
        let mut offset = mvex.body_start;
        while offset < mvex.end {
            let item = box_at(data, offset, mvex.end)?;
            offset = item.end;
            if item.fourcc == *b"trex" {
                let body = &data[item.body_start..item.end];
                let track = tracks.get_mut(&word(body, 4)?)?;
                track.default_flags = Some(word(body, 20)?);
            }
        }
        (!tracks.is_empty()).then_some(Self { tracks })
    }

    pub fn is_independent(&self, data: &Bytes) -> bool {
        self.check(data).unwrap_or(false)
    }

    /// Whether `data` (one or more `moof`/`mdat` pairs) can start a new
    /// file: every track of the initialization segment must appear, and the
    /// first fragment of each track in `data` must start with an
    /// independently decodable sample. Later fragments of a track that was
    /// already seen do not affect the result.
    fn check(&self, data: &Bytes) -> Option<bool> {
        let mut seen = HashSet::new();
        let mut offset = 0;
        while offset < data.len() && seen.len() < self.tracks.len() {
            let moof = box_at(data, offset, data.len())?;
            offset = moof.end;
            if moof.fourcc != *b"moof" {
                continue;
            }
            // The fragment's media data is the next `mdat`, which must come
            // before any other `moof`; otherwise this fragment has none.
            let mut next = moof.end;
            let mdat = loop {
                let item = box_at(data, next, data.len())?;
                match &item.fourcc {
                    b"mdat" => break item,
                    b"moof" => return Some(false),
                    _ => next = item.end,
                }
            };
            if mdat.body_start == mdat.end {
                return Some(false);
            }
            let mut in_fragment = HashSet::new();
            let mut traf_offset = moof.body_start;
            while traf_offset < moof.end {
                let traf = box_at(data, traf_offset, moof.end)?;
                traf_offset = traf.end;
                if traf.fourcc != *b"traf" {
                    continue;
                }
                let tfhd = find_first_box(data, traf.body_start, traf.end, *b"tfhd")?;
                let id = word(&data[tfhd.body_start..tfhd.end], 4)?;
                if !in_fragment.insert(id) {
                    return Some(false);
                }
                if seen.contains(&id) {
                    continue;
                }
                if !self.starts_independently(data, &traf, id)? {
                    return Some(false);
                }
            }
            seen.extend(in_fragment);
        }
        Some(!seen.is_empty() && seen.len() == self.tracks.len())
    }

    /// Whether the first sample of track fragment `traf` (of track `id`) is
    /// a valid cut point. Non-video tracks only need a well-formed fragment.
    fn starts_independently(&self, data: &Bytes, traf: &BoxView, id: u32) -> Option<bool> {
        let tfhd = find_first_box(data, traf.body_start, traf.end, *b"tfhd")?;
        let body = &data[tfhd.body_start..tfhd.end];
        let tfhd_version_and_flags = word(body, 0)?;
        if (tfhd_version_and_flags >> 24) != 0 {
            return None;
        }
        let flags = tfhd_version_and_flags & 0x00ff_ffff;
        // Explicit absolute data offsets do not remain valid in a new file.
        if flags & 1 != 0 || flags & 0x010000 != 0 {
            return Some(false);
        }
        let track = self.tracks.get(&id)?;
        let mut field = 8;
        for flag in [0x000002, 0x000008, 0x000010] {
            if flags & flag != 0 {
                word(body, field)?;
                field += 4;
            }
        }
        let default_flags = if flags & 0x000020 != 0 {
            Some(word(body, field)?)
        } else {
            track.default_flags
        };
        let trun = find_first_box(data, traf.body_start, traf.end, *b"trun")?;
        let body = &data[trun.body_start..trun.end];
        let trun_version_and_flags = word(body, 0)?;
        if (trun_version_and_flags >> 24) > 1 {
            return None;
        }
        let flags = trun_version_and_flags & 0x00ff_ffff;
        let sample_count = word(body, 4)?;
        if sample_count == 0 {
            return Some(false);
        }
        let mut field = 8;
        if flags & 1 != 0 {
            word(body, field)?;
            field += 4;
        }
        let sample_flags = if flags & 0x000004 != 0 {
            if flags & 0x000400 != 0 {
                return Some(false);
            }
            let first = word(body, field)?;
            field += 4;
            Some(first)
        } else if flags & 0x000400 != 0 {
            let mut offset = field;
            if flags & 0x000100 != 0 {
                offset += 4;
            }
            if flags & 0x000200 != 0 {
                offset += 4;
            }
            Some(word(body, offset)?)
        } else {
            default_flags
        };
        // Per-sample rows start after the optional first-sample flags.
        let rows = field;
        let row_stride = [0x000100, 0x000200, 0x000400, 0x000800]
            .into_iter()
            .filter(|flag| flags & flag != 0)
            .count()
            * 4;
        if row_stride > 0 {
            let total_rows_bytes = usize::try_from(sample_count)
                .ok()?
                .checked_mul(row_stride)?;
            if rows.checked_add(total_rows_bytes)? > body.len() {
                return None;
            }
        }
        // Whether the flags of a sample after the first mark it non-sync:
        // proof that this muxer signals sync samples rather than leaving
        // every sample's flags at zero.
        let later_non_sync = || -> Option<bool> {
            if sample_count < 2 {
                return Some(false);
            }
            if flags & 0x000400 == 0 {
                return Some(default_flags.is_some_and(|flags| flags & NON_SYNC != 0));
            }
            let before_flags = [0x000100, 0x000200]
                .into_iter()
                .filter(|flag| flags & flag != 0)
                .count()
                * 4;
            for sample in 1..sample_count as usize {
                let offset = rows
                    .checked_add(sample.checked_mul(row_stride)?)?
                    .checked_add(before_flags)?;
                if word(body, offset)? & NON_SYNC != 0 {
                    return Some(true);
                }
            }
            Some(false)
        };
        if track.video {
            let flags = sample_flags?;
            if flags & NON_SYNC != 0 {
                return Some(false);
            }
            // ISO/IEC 14496-12 sample_depends_on: 2 = independently
            // decodable, 1 = depends on other samples, 0 = unknown. A sync
            // sample with an unknown dependency is still a sync sample, but
            // is trusted only when the same run marks later samples
            // non-sync; a muxer that zeroes all flags would otherwise make
            // every inter frame look like a cut point.
            match (flags >> 24) & 3 {
                2 => {}
                0 if later_non_sync()? => {}
                _ => return Some(false),
            }
        }
        Some(true)
    }
}
