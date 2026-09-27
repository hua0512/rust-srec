//! Conservative random-access validation for independently playable fMP4 cuts.

use std::collections::HashMap;

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

    fn atom(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut result = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        result.extend_from_slice(name);
        result.extend_from_slice(body);
        result
    }

    fn init(video: bool, default_flags: u32) -> Bytes {
        let mut tkhd = vec![0; 16];
        tkhd[12..16].copy_from_slice(&1u32.to_be_bytes());
        let mut hdlr = vec![0; 12];
        hdlr[8..12].copy_from_slice(if video { b"vide" } else { b"soun" });
        let mut trak = atom(b"tkhd", &tkhd);
        trak.extend(atom(b"mdia", &atom(b"hdlr", &hdlr)));
        let mut trex = vec![0; 24];
        trex[4..8].copy_from_slice(&1u32.to_be_bytes());
        trex[20..24].copy_from_slice(&default_flags.to_be_bytes());
        let mut moov = atom(b"trak", &trak);
        moov.extend(atom(b"mvex", &atom(b"trex", &trex)));
        Bytes::from(atom(b"moov", &moov))
    }

    fn media(first_flags: Option<u32>) -> Bytes {
        let tfhd = [0x00, 0x02, 0, 0, 0, 0, 0, 1];
        let mut trun = if first_flags.is_some() { 4u32 } else { 0 }
            .to_be_bytes()
            .to_vec();
        trun.extend_from_slice(&1u32.to_be_bytes());
        if let Some(flags) = first_flags {
            trun.extend_from_slice(&flags.to_be_bytes());
        }
        let mut traf = atom(b"tfhd", &tfhd);
        traf.extend(atom(b"trun", &trun));
        let mut result = atom(b"moof", &atom(b"traf", &traf));
        result.extend(atom(b"mdat", &[1, 2, 3, 4]));
        Bytes::from(result)
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

fn child(data: &Bytes, parent: BoxView, name: [u8; 4]) -> Option<BoxView> {
    find_first_box(data, parent.body_start, parent.end, name)
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
            let tkhd = child(data, item, *b"tkhd")?;
            let body = &data[tkhd.body_start..tkhd.end];
            let id = word(
                body,
                match *body.first()? {
                    0 => 12,
                    1 => 20,
                    _ => return None,
                },
            )?;
            let mdia = child(data, item, *b"mdia")?;
            let hdlr = child(data, mdia, *b"hdlr")?;
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
        let mvex = child(data, moov, *b"mvex")?;
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

    fn check(&self, data: &Bytes) -> Option<bool> {
        let moof = find_first_box(data, 0, data.len(), *b"moof")?;
        let mdat = find_first_box(data, moof.end, data.len(), *b"mdat")?;
        if mdat.body_start == mdat.end {
            return Some(false);
        }
        let mut seen = std::collections::HashSet::new();
        let mut offset = moof.body_start;
        while offset < moof.end {
            let traf = box_at(data, offset, moof.end)?;
            offset = traf.end;
            if traf.fourcc != *b"traf" {
                continue;
            }
            let tfhd = child(data, traf, *b"tfhd")?;
            let body = &data[tfhd.body_start..tfhd.end];
            let flags = word(body, 0)? & 0x00ff_ffff;
            // Explicit absolute data offsets do not remain valid in a new file.
            if flags & 1 != 0 || flags & 0x010000 != 0 {
                return Some(false);
            }
            let id = word(body, 4)?;
            let track = self.tracks.get(&id)?;
            if !seen.insert(id) {
                return Some(false);
            }
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
            let trun = child(data, traf, *b"trun")?;
            let body = &data[trun.body_start..trun.end];
            let flags = word(body, 0)? & 0x00ff_ffff;
            if word(body, 4)? == 0 {
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
                Some(word(body, field)?)
            } else if flags & 0x000400 != 0 {
                if flags & 0x000100 != 0 {
                    field += 4;
                }
                if flags & 0x000200 != 0 {
                    field += 4;
                }
                Some(word(body, field)?)
            } else {
                default_flags
            };
            if track.video {
                let flags = sample_flags?;
                if flags & 0x0001_0000 != 0 || (flags >> 24) & 3 != 2 {
                    return Some(false);
                }
            }
        }
        Some(!seen.is_empty() && seen.len() == self.tracks.len())
    }
}
