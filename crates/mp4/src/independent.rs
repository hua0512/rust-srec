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
}

/// `sample_is_non_sync_sample` in ISO/IEC 14496-12 sample flags.
const NON_SYNC: u32 = 0x0001_0000;

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
        let flags = word(body, 0)? & 0x00ff_ffff;
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
        let flags = word(body, 0)? & 0x00ff_ffff;
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
            let row = [0x000100, 0x000200, 0x000400, 0x000800]
                .into_iter()
                .filter(|flag| flags & flag != 0)
                .count()
                * 4;
            let before_flags = [0x000100, 0x000200]
                .into_iter()
                .filter(|flag| flags & flag != 0)
                .count()
                * 4;
            for sample in 1..sample_count as usize {
                let offset = rows
                    .checked_add(sample.checked_mul(row)?)?
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
