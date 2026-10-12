//! Combining single-track initialization segments into one `moov`.

use bytes::Bytes;

use super::MergeError;
use crate::FourCc;
use crate::box_utils::{BodyReader, BoxView, children, top_level_boxes, write_box};

/// `ftyp` of the merged file (ISO/IEC 14496-12 §4.3). `iso5` is the brand
/// that permits `default-base-is-moof`, which every rewritten fragment uses;
/// `iso6` covers `tfdt`. No CMAF or DASH brand is claimed: the merged
/// multi-track file is not a single-track CMAF/DASH segment.
const MAJOR_BRAND: &[u8; 4] = b"iso5";
const COMPATIBLE_BRANDS: [&[u8; 4]; 4] = [b"isom", b"iso5", b"iso6", b"mp41"];

/// Sample entry types of protected content: `enc*` from ISO/IEC 14496-12
/// §8.12 and ISO/IEC 23001-7, plus Apple FairPlay's `drms`/`drmi`.
const PROTECTED_SAMPLE_ENTRIES: [&[u8; 4]; 8] = [
    b"encv", b"enca", b"enct", b"encs", b"encm", b"encf", b"drms", b"drmi",
];

/// Bytes of an `mvhd` body after its version-dependent time fields
/// (§8.2.2: rate, volume, reserved, matrix, pre_defined, next_track_ID).
const MVHD_TAIL_LEN: usize = 80;

/// One multi-track initialization segment built from single-track sources.
#[derive(Debug, Clone)]
pub struct MergedInit {
    data: Bytes,
    tracks: Vec<MergedTrack>,
}

/// A track of a [`MergedInit`], in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedTrack {
    track_id: u32,
    source_track_id: u32,
    handler: FourCc,
    timescale: u32,
    pub(crate) default_sample_duration: u32,
    pub(crate) default_sample_size: u32,
    /// Number of sample entries in `stsd`; valid sample description
    /// indices are `1..=sample_entry_count`.
    pub(crate) sample_entry_count: u32,
}

impl MergedInit {
    /// The `ftyp` and `moov` boxes to write at the start of the output.
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// Merged tracks; index `i` comes from the `i`-th source passed to
    /// [`merge_init`].
    pub fn tracks(&self) -> &[MergedTrack] {
        &self.tracks
    }
}

impl MergedTrack {
    /// Track ID in the merged file (source index + 1).
    pub fn track_id(&self) -> u32 {
        self.track_id
    }

    /// Track ID used by the source segments.
    pub fn source_track_id(&self) -> u32 {
        self.source_track_id
    }

    /// Media handler type from `hdlr`, such as `vide` or `soun`.
    pub fn handler(&self) -> FourCc {
        self.handler
    }

    /// Media timescale from `mdhd`; fragment decode times use this unit.
    pub fn timescale(&self) -> u32 {
        self.timescale
    }
}

/// Merge single-track initialization segments into one initialization
/// segment, assigning track ID `i + 1` to the `i`-th source.
///
/// The first source supplies the movie header and so the movie timescale;
/// edit lists of other sources are rescaled to it. Source `trak` boxes are
/// otherwise copied unchanged, so codec configuration and any boxes this
/// module does not interpret are preserved.
pub fn merge_init(sources: &[Bytes]) -> Result<MergedInit, MergeError> {
    let parsed = sources
        .iter()
        .map(parse_source)
        .collect::<Result<Vec<_>, _>>()?;
    let Some(movie) = parsed.first().map(|source| &source.mvhd) else {
        return Err(MergeError::NoSources);
    };
    let next_track_id = u32::try_from(parsed.len())
        .ok()
        .and_then(|count| count.checked_add(1))
        .ok_or(MergeError::Overflow("track count"))?;

    let mut out = Vec::new();
    write_box(&mut out, b"ftyp", |out| {
        out.extend_from_slice(MAJOR_BRAND);
        out.extend_from_slice(&0u32.to_be_bytes());
        for brand in COMPATIBLE_BRANDS {
            out.extend_from_slice(brand);
        }
        Ok(())
    })?;
    write_box(&mut out, b"moov", |out| {
        write_box(out, b"mvhd", |out| {
            out.extend_from_slice(&movie.merged_body(next_track_id));
            Ok(())
        })?;
        for (track_id, source) in (1..).zip(&parsed) {
            out.extend_from_slice(&source.trak.merged_box(
                track_id,
                source.mvhd.timescale,
                movie.timescale,
            )?);
        }
        write_box(out, b"mvex", |out| {
            for (track_id, source) in (1..).zip(&parsed) {
                write_box(out, b"trex", |out| {
                    out.extend_from_slice(&source.trex.merged_body(track_id));
                    Ok(())
                })?;
            }
            Ok(())
        })
    })?;

    let tracks = (1..)
        .zip(&parsed)
        .map(|(track_id, source)| MergedTrack {
            track_id,
            source_track_id: source.trak.track_id,
            handler: FourCc(source.trak.handler),
            timescale: source.trak.timescale,
            default_sample_duration: source.trex.default_sample_duration,
            default_sample_size: source.trex.default_sample_size,
            sample_entry_count: source.trak.sample_entry_count,
        })
        .collect();
    Ok(MergedInit {
        data: Bytes::from(out),
        tracks,
    })
}

/// Checks an initialization segment that is used as is, without merging
/// (for example one source that already carries several tracks), and
/// returns its track count.
///
/// Every track must be well-formed and unprotected, under the same rules
/// [`merge_init`] applies to a source track: a `tkhd`, an `mdia` whose
/// sample entries are clear and reference a `url ` data entry, and a `trex`
/// in `mvex`. Track references are allowed, since nothing is renumbered.
pub fn validate_init(data: &Bytes) -> Result<usize, MergeError> {
    let mut moov = None;
    for item in top_level_boxes(data) {
        let item = item?;
        match &item.fourcc {
            b"moov" => set_once(&mut moov, item)?,
            b"moof" | b"mdat" => {
                return Err(MergeError::unsupported(
                    &item.fourcc,
                    "initialization segment contains media data",
                ));
            }
            _ => {}
        }
    }
    let moov = moov.ok_or_else(|| MergeError::missing(b"moov", b"file"))?;
    let (mut mvhd, mut mvex) = (None, None);
    // (track ID, sample entries) of every track.
    let mut tracks: Vec<(u32, u32)> = Vec::new();
    for child in children(data, moov.body_start, moov.end, b"moov") {
        let child = child?;
        match &child.fourcc {
            b"mvhd" => set_once(&mut mvhd, child)?,
            b"mvex" => set_once(&mut mvex, child)?,
            b"pssh" => {
                return Err(MergeError::Protected {
                    fourcc: b"pssh".into(),
                });
            }
            b"trak" => {
                let (mut tkhd, mut mdia) = (None, None);
                for item in children(data, child.body_start, child.end, b"trak") {
                    let item = item?;
                    match &item.fourcc {
                        b"tkhd" => set_once(&mut tkhd, item)?,
                        b"mdia" => set_once(&mut mdia, item)?,
                        _ => {}
                    }
                }
                let tkhd = tkhd.ok_or_else(|| MergeError::missing(b"tkhd", b"trak"))?;
                let (_, track_id) = parse_tkhd(&data[tkhd.body_start..tkhd.end])?;
                let mdia = mdia.ok_or_else(|| MergeError::missing(b"mdia", b"trak"))?;
                let media = parse_mdia(data, &mdia)?;
                if tracks.iter().any(|&(id, _)| id == track_id) {
                    return Err(MergeError::malformed(b"tkhd", "track ID is not unique"));
                }
                tracks.push((track_id, media.sample_entry_count));
            }
            _ => {}
        }
    }
    let mvhd = mvhd.ok_or_else(|| MergeError::missing(b"mvhd", b"moov"))?;
    Mvhd::parse(data.slice(mvhd.body_start..mvhd.end))?;
    if tracks.is_empty() {
        return Err(MergeError::missing(b"trak", b"moov"));
    }
    // Exactly one `trex` per track (§8.8.3), whose default sample
    // description refers to one of that track's sample entries, as
    // `merge_init` requires of a single-track source.
    let mvex = mvex.ok_or_else(|| MergeError::missing(b"mvex", b"moov"))?;
    let mut with_trex: Vec<u32> = Vec::new();
    for item in children(data, mvex.body_start, mvex.end, b"mvex") {
        let item = item?;
        if item.fourcc != *b"trex" {
            continue;
        }
        let trex = Trex::parse(&data[item.body_start..item.end])?;
        let Some(&(_, entries)) = tracks.iter().find(|&&(id, _)| id == trex.track_id) else {
            return Err(MergeError::UnknownTrack {
                track_id: trex.track_id,
            });
        };
        if with_trex.contains(&trex.track_id) {
            return Err(MergeError::malformed(
                b"mvex",
                "more than one trex for the track",
            ));
        }
        if !(1..=entries).contains(&trex.sample_description_index) {
            return Err(MergeError::malformed(
                b"trex",
                "default sample description index refers to no sample entry",
            ));
        }
        with_trex.push(trex.track_id);
    }
    if tracks.iter().any(|(id, _)| !with_trex.contains(id)) {
        return Err(MergeError::missing(b"trex", b"mvex"));
    }
    Ok(tracks.len())
}

struct SourceInit {
    mvhd: Mvhd,
    trak: SourceTrak,
    trex: Trex,
}

/// Record `child` as the only box of its type in its parent.
fn set_once(slot: &mut Option<BoxView>, child: BoxView) -> Result<(), MergeError> {
    if slot.replace(child).is_some() {
        return Err(MergeError::malformed(
            &child.fourcc,
            "box appears more than once in its parent",
        ));
    }
    Ok(())
}

fn parse_source(data: &Bytes) -> Result<SourceInit, MergeError> {
    let mut moov = None;
    for item in top_level_boxes(data) {
        let item = item?;
        match &item.fourcc {
            b"moov" => set_once(&mut moov, item)?,
            b"moof" | b"mdat" => {
                return Err(MergeError::unsupported(
                    &item.fourcc,
                    "initialization segment contains media data",
                ));
            }
            // `ftyp` is rebuilt; other top-level boxes are not carried over.
            _ => {}
        }
    }
    let moov = moov.ok_or_else(|| MergeError::missing(b"moov", b"file"))?;

    let mut mvhd = None;
    let mut mvex = None;
    let mut traks = Vec::new();
    for child in children(data, moov.body_start, moov.end, b"moov") {
        let child = child?;
        match &child.fourcc {
            b"mvhd" => set_once(&mut mvhd, child)?,
            b"mvex" => set_once(&mut mvex, child)?,
            b"trak" => traks.push(child),
            b"pssh" => {
                return Err(MergeError::Protected {
                    fourcc: b"pssh".into(),
                });
            }
            // `udta`, `meta`, `iods` and other movie-level boxes describe the
            // source file, not the merged one, and are not carried over.
            _ => {}
        }
    }

    let mvhd = mvhd.ok_or_else(|| MergeError::missing(b"mvhd", b"moov"))?;
    let mvhd = Mvhd::parse(data.slice(mvhd.body_start..mvhd.end))?;
    let trak = match traks.as_slice() {
        [trak] => SourceTrak::parse(data, trak)?,
        [] => return Err(MergeError::missing(b"trak", b"moov")),
        traks => return Err(MergeError::MultipleTracks { count: traks.len() }),
    };

    // Fragments need `trex` defaults for their track (§8.8.3).
    let mvex = mvex.ok_or_else(|| MergeError::missing(b"mvex", b"moov"))?;
    let mut trex = None;
    for item in children(data, mvex.body_start, mvex.end, b"mvex") {
        let item = item?;
        if item.fourcc != *b"trex" {
            continue;
        }
        let parsed = Trex::parse(&data[item.body_start..item.end])?;
        if parsed.track_id == trak.track_id && trex.replace(parsed).is_some() {
            return Err(MergeError::malformed(
                b"mvex",
                "more than one trex for the track",
            ));
        }
    }
    let trex = trex.ok_or_else(|| MergeError::missing(b"trex", b"mvex"))?;
    if !(1..=trak.sample_entry_count).contains(&trex.sample_description_index) {
        return Err(MergeError::malformed(
            b"trex",
            "default sample description index refers to no sample entry",
        ));
    }
    Ok(SourceInit { mvhd, trak, trex })
}

/// Movie header box (§8.2.2).
struct Mvhd {
    body: Bytes,
    version: u8,
    timescale: u32,
}

impl Mvhd {
    fn parse(body: Bytes) -> Result<Self, MergeError> {
        let mut reader = BodyReader::new(&body, *b"mvhd");
        let (version, _flags) = reader.full_box_header()?;
        let timescale = match version {
            0 => {
                reader.take(8)?;
                let timescale = reader.u32()?;
                reader.u32()?;
                timescale
            }
            1 => {
                reader.take(16)?;
                let timescale = reader.u32()?;
                reader.u64()?;
                timescale
            }
            _ => {
                return Err(MergeError::unsupported(
                    b"mvhd",
                    "only versions 0 and 1 are defined",
                ));
            }
        };
        reader.take(MVHD_TAIL_LEN)?;
        reader.finish()?;
        if timescale == 0 {
            return Err(MergeError::malformed(b"mvhd", "timescale is zero"));
        }
        Ok(Self {
            body,
            version,
            timescale,
        })
    }

    /// Body for the merged file: the duration is cleared because the merged
    /// `moov` holds no samples (every sample is in a fragment, §8.8.1), and
    /// `next_track_ID` follows the merged track IDs.
    fn merged_body(&self, next_track_id: u32) -> Vec<u8> {
        let mut body = self.body.to_vec();
        let duration = if self.version == 0 { 16..20 } else { 24..32 };
        body[duration].fill(0);
        let next = body.len() - 4;
        body[next..].copy_from_slice(&next_track_id.to_be_bytes());
        body
    }
}

/// Track extends box (§8.8.3): per-track sample defaults for fragments.
struct Trex {
    body: [u8; 24],
    track_id: u32,
    sample_description_index: u32,
    default_sample_duration: u32,
    default_sample_size: u32,
}

impl Trex {
    fn parse(body: &[u8]) -> Result<Self, MergeError> {
        let mut reader = BodyReader::new(body, *b"trex");
        if reader.full_box_header()? != (0, 0) {
            return Err(MergeError::unsupported(
                b"trex",
                "only version 0 with zero flags is defined",
            ));
        }
        let track_id = reader.u32()?;
        let sample_description_index = reader.u32()?;
        let default_sample_duration = reader.u32()?;
        let default_sample_size = reader.u32()?;
        let _sample_flags = reader.u32()?;
        reader.finish()?;
        let mut copy = [0; 24];
        copy.copy_from_slice(body);
        Ok(Self {
            body: copy,
            track_id,
            sample_description_index,
            default_sample_duration,
            default_sample_size,
        })
    }

    fn merged_body(&self, track_id: u32) -> [u8; 24] {
        let mut body = self.body;
        body[4..8].copy_from_slice(&track_id.to_be_bytes());
        body
    }
}

/// The source `trak` and the positions of the fields the merge rewrites,
/// relative to the start of the `trak` box.
struct SourceTrak {
    bytes: Bytes,
    track_id: u32,
    track_id_at: usize,
    /// `tkhd.duration`: position and width (4 or 8 bytes).
    duration_at: (usize, usize),
    alternate_group_at: usize,
    timescale: u32,
    handler: [u8; 4],
    sample_entry_count: u32,
    edit_list: Option<EditList>,
}

/// Position of the `elst` entries (§8.6.6) inside the `trak` box.
struct EditList {
    entries_at: usize,
    count: usize,
    /// 4 for version 0, 8 for version 1.
    duration_width: usize,
}

impl SourceTrak {
    fn parse(data: &Bytes, trak: &BoxView) -> Result<Self, MergeError> {
        let (mut tkhd, mut edts, mut mdia) = (None, None, None);
        for child in children(data, trak.body_start, trak.end, b"trak") {
            let child = child?;
            match &child.fourcc {
                b"tkhd" => set_once(&mut tkhd, child)?,
                b"edts" => set_once(&mut edts, child)?,
                b"mdia" => set_once(&mut mdia, child)?,
                b"tref" => {
                    return Err(MergeError::unsupported(
                        b"tref",
                        "track references point outside a single-track source",
                    ));
                }
                // Everything else is kept verbatim in the merged `trak`.
                _ => {}
            }
        }
        let relative = |position: usize| position - trak.start;

        let tkhd = tkhd.ok_or_else(|| MergeError::missing(b"tkhd", b"trak"))?;
        let (version, track_id) = parse_tkhd(&data[tkhd.body_start..tkhd.end])?;
        // Field offsets within the tkhd body, FullBox header included (§8.3.2).
        let (id_at, duration_at, group_at) = if version == 0 {
            (12, (20, 4), 34)
        } else {
            (20, (28, 8), 46)
        };
        let tkhd_body = relative(tkhd.body_start);

        let edit_list = match edts {
            Some(edts) => {
                let mut elst = None;
                for item in children(data, edts.body_start, edts.end, b"edts") {
                    let item = item?;
                    if item.fourcc == *b"elst" {
                        set_once(&mut elst, item)?;
                    }
                }
                elst.map(|elst| {
                    let (count, duration_width) = parse_elst(&data[elst.body_start..elst.end])?;
                    Ok::<_, MergeError>(EditList {
                        entries_at: relative(elst.body_start) + 8,
                        count,
                        duration_width,
                    })
                })
                .transpose()?
            }
            None => None,
        };

        let mdia = mdia.ok_or_else(|| MergeError::missing(b"mdia", b"trak"))?;
        let media = parse_mdia(data, &mdia)?;
        Ok(Self {
            bytes: data.slice(trak.start..trak.end),
            track_id,
            track_id_at: tkhd_body + id_at,
            duration_at: (tkhd_body + duration_at.0, duration_at.1),
            alternate_group_at: tkhd_body + group_at,
            timescale: media.timescale,
            handler: media.handler,
            sample_entry_count: media.sample_entry_count,
            edit_list,
        })
    }

    /// The `trak` box for the merged file:
    /// - the merged track ID;
    /// - a cleared duration (see [`Mvhd::merged_body`]);
    /// - no alternate group: sources are independent tracks, and a group
    ///   number shared by two sources would make players pick only one of
    ///   them (§8.3.2.3);
    /// - edit-list durations converted from the source movie timescale to the
    ///   merged one (§8.6.6: `segment_duration` is in movie timescale units;
    ///   `media_time` is in media units and is kept).
    fn merged_box(
        &self,
        track_id: u32,
        source_timescale: u32,
        movie_timescale: u32,
    ) -> Result<Vec<u8>, MergeError> {
        let mut bytes = self.bytes.to_vec();
        bytes[self.track_id_at..self.track_id_at + 4].copy_from_slice(&track_id.to_be_bytes());
        let (duration_at, width) = self.duration_at;
        bytes[duration_at..duration_at + width].fill(0);
        bytes[self.alternate_group_at..self.alternate_group_at + 2].fill(0);

        if source_timescale != movie_timescale
            && let Some(edits) = &self.edit_list
        {
            let width = edits.duration_width;
            let entry_len = 2 * width + 4;
            for entry in 0..edits.count {
                let at = edits.entries_at + entry * entry_len;
                let field = &mut bytes[at..at + width];
                let duration = field
                    .iter()
                    .fold(0u128, |value, &byte| (value << 8) | u128::from(byte));
                let rescaled = (duration * u128::from(movie_timescale)
                    + u128::from(source_timescale / 2))
                    / u128::from(source_timescale);
                if rescaled >> (8 * width) != 0 {
                    return Err(MergeError::Overflow("edit list segment duration"));
                }
                field.copy_from_slice(&rescaled.to_be_bytes()[16 - width..]);
            }
        }
        Ok(bytes)
    }
}

/// Returns `(version, track_ID)` of a track header box (§8.3.2).
fn parse_tkhd(body: &[u8]) -> Result<(u8, u32), MergeError> {
    let mut reader = BodyReader::new(body, *b"tkhd");
    let (version, _flags) = reader.full_box_header()?;
    // Fields after track_ID: reserved, duration, then 60 bytes of reserved,
    // layer, alternate_group, volume, reserved, matrix, width and height.
    let (times, duration) = match version {
        0 => (8, 4),
        1 => (16, 8),
        _ => {
            return Err(MergeError::unsupported(
                b"tkhd",
                "only versions 0 and 1 are defined",
            ));
        }
    };
    reader.take(times)?;
    let track_id = reader.u32()?;
    reader.take(4 + duration + 60)?;
    reader.finish()?;
    if track_id == 0 {
        return Err(MergeError::malformed(b"tkhd", "track_ID is zero"));
    }
    Ok((version, track_id))
}

/// Returns `(entry_count, segment_duration width)` of an edit list (§8.6.6).
fn parse_elst(body: &[u8]) -> Result<(usize, usize), MergeError> {
    let mut reader = BodyReader::new(body, *b"elst");
    let (version, _flags) = reader.full_box_header()?;
    let width = match version {
        0 => 4,
        1 => 8,
        _ => {
            return Err(MergeError::unsupported(
                b"elst",
                "only versions 0 and 1 are defined",
            ));
        }
    };
    let count = usize::try_from(reader.u32()?)
        .map_err(|_| MergeError::malformed(b"elst", "entry count overflows"))?;
    let entries_len = count
        .checked_mul(2 * width + 4)
        .ok_or_else(|| MergeError::malformed(b"elst", "entry count overflows"))?;
    reader.take(entries_len)?;
    reader.finish()?;
    Ok((count, width))
}

struct Media {
    timescale: u32,
    handler: [u8; 4],
    sample_entry_count: u32,
}

/// Media box (§8.4): timescale, handler, and a validated sample table.
fn parse_mdia(data: &Bytes, mdia: &BoxView) -> Result<Media, MergeError> {
    let (mut mdhd, mut hdlr, mut minf) = (None, None, None);
    for child in children(data, mdia.body_start, mdia.end, b"mdia") {
        let child = child?;
        match &child.fourcc {
            b"mdhd" => set_once(&mut mdhd, child)?,
            b"hdlr" => set_once(&mut hdlr, child)?,
            b"minf" => set_once(&mut minf, child)?,
            _ => {}
        }
    }
    let mdhd = mdhd.ok_or_else(|| MergeError::missing(b"mdhd", b"mdia"))?;
    let hdlr = hdlr.ok_or_else(|| MergeError::missing(b"hdlr", b"mdia"))?;
    let minf = minf.ok_or_else(|| MergeError::missing(b"minf", b"mdia"))?;

    let (mut dinf, mut stbl) = (None, None);
    for child in children(data, minf.body_start, minf.end, b"minf") {
        let child = child?;
        match &child.fourcc {
            b"dinf" => set_once(&mut dinf, child)?,
            b"stbl" => set_once(&mut stbl, child)?,
            _ => {}
        }
    }
    let dinf = dinf.ok_or_else(|| MergeError::missing(b"dinf", b"minf"))?;
    let stbl = stbl.ok_or_else(|| MergeError::missing(b"stbl", b"minf"))?;
    let data_references = parse_dinf(data, &dinf)?;
    Ok(Media {
        timescale: parse_mdhd(&data[mdhd.body_start..mdhd.end])?,
        handler: parse_hdlr(&data[hdlr.body_start..hdlr.end])?,
        sample_entry_count: parse_stbl(data, &stbl, data_references)?,
    })
}

fn parse_mdhd(body: &[u8]) -> Result<u32, MergeError> {
    let mut reader = BodyReader::new(body, *b"mdhd");
    let (version, _flags) = reader.full_box_header()?;
    let (times, duration) = match version {
        0 => (8, 4),
        1 => (16, 8),
        _ => {
            return Err(MergeError::unsupported(
                b"mdhd",
                "only versions 0 and 1 are defined",
            ));
        }
    };
    reader.take(times)?;
    let timescale = reader.u32()?;
    // Duration, then language and pre_defined.
    reader.take(duration + 4)?;
    reader.finish()?;
    if timescale == 0 {
        return Err(MergeError::malformed(b"mdhd", "timescale is zero"));
    }
    Ok(timescale)
}

/// Handler reference box (§8.4.3): pre_defined, handler_type, three
/// reserved words, then a name.
fn parse_hdlr(body: &[u8]) -> Result<[u8; 4], MergeError> {
    let mut reader = BodyReader::new(body, *b"hdlr");
    if reader.full_box_header()?.0 != 0 {
        return Err(MergeError::unsupported(
            b"hdlr",
            "only version 0 is defined",
        ));
    }
    reader.u32()?;
    let mut handler = [0; 4];
    handler.copy_from_slice(reader.take(4)?);
    reader.take(12)?;
    Ok(handler)
}

/// Data information box (§8.7.1–§8.7.2). Returns the number of data
/// references; every one must say "media data is in this file", since the
/// merged recording is standalone.
fn parse_dinf(data: &Bytes, dinf: &BoxView) -> Result<u32, MergeError> {
    let mut dref = None;
    for child in children(data, dinf.body_start, dinf.end, b"dinf") {
        let child = child?;
        if child.fourcc == *b"dref" {
            set_once(&mut dref, child)?;
        }
    }
    let dref = dref.ok_or_else(|| MergeError::missing(b"dref", b"dinf"))?;
    let mut reader = BodyReader::new(&data[dref.body_start..dref.end], *b"dref");
    if reader.full_box_header()?.0 != 0 {
        return Err(MergeError::unsupported(
            b"dref",
            "only version 0 is defined",
        ));
    }
    let declared = reader.u32()?;
    let mut found = 0u32;
    for entry in children(data, dref.body_start + 8, dref.end, b"dref") {
        let entry = entry?;
        if found == declared {
            return Err(MergeError::malformed(
                b"dref",
                "more entries than entry_count",
            ));
        }
        found += 1;
        let (_version, flags) =
            BodyReader::new(&data[entry.body_start..entry.end], entry.fourcc).full_box_header()?;
        // Flag 1: the media data is in the same file as the movie box.
        if entry.fourcc != *b"url " || flags & 1 == 0 {
            return Err(MergeError::unsupported(
                &entry.fourcc,
                "media data in another file is not supported",
            ));
        }
    }
    if declared == 0 || found != declared {
        return Err(MergeError::malformed(
            b"dref",
            "entry_count does not match the data references",
        ));
    }
    Ok(declared)
}

/// Sample table box (§8.5.1). The tables must be empty: every sample of a
/// fragmented track is in a movie fragment, and samples described here would
/// point at chunk offsets that do not exist in the merged file. Returns the
/// number of sample entries.
fn parse_stbl(data: &Bytes, stbl: &BoxView, data_references: u32) -> Result<u32, MergeError> {
    let mut stsd = None;
    for child in children(data, stbl.body_start, stbl.end, b"stbl") {
        let child = child?;
        let body = &data[child.body_start..child.end];
        // Position of the sample or entry count within each table's body.
        let count_at = match &child.fourcc {
            b"stsd" => {
                set_once(&mut stsd, child)?;
                continue;
            }
            b"stts" | b"stsc" | b"stco" | b"co64" => 4,
            b"stsz" | b"stz2" => 8,
            _ => continue,
        };
        let mut reader = BodyReader::new(body, child.fourcc);
        reader.take(count_at)?;
        if reader.u32()? != 0 {
            return Err(MergeError::unsupported(
                &child.fourcc,
                "initialization segment describes samples outside fragments",
            ));
        }
    }
    let stsd = stsd.ok_or_else(|| MergeError::missing(b"stsd", b"stbl"))?;
    check_sample_entries(data, &stsd, data_references)
}

/// Sample description box (§8.5.2): at least one entry, none protected, each
/// referring to an existing data reference. Returns the entry count.
fn check_sample_entries(
    data: &Bytes,
    stsd: &BoxView,
    data_references: u32,
) -> Result<u32, MergeError> {
    let mut reader = BodyReader::new(&data[stsd.body_start..stsd.end], *b"stsd");
    // Version 1 only changes which audio sample entry versions may follow.
    if reader.full_box_header()?.0 > 1 {
        return Err(MergeError::unsupported(
            b"stsd",
            "only versions 0 and 1 are defined",
        ));
    }
    let declared = reader.u32()?;
    let mut found = 0u32;
    for entry in children(data, stsd.body_start + 8, stsd.end, b"stsd") {
        let entry = entry?;
        if found == declared {
            return Err(MergeError::malformed(
                b"stsd",
                "more entries than entry_count",
            ));
        }
        found += 1;
        if PROTECTED_SAMPLE_ENTRIES.contains(&&entry.fourcc) {
            return Err(MergeError::Protected {
                fourcc: FourCc(entry.fourcc),
            });
        }
        // SampleEntry (§8.5.2.2): six reserved bytes, data_reference_index.
        let mut fields = BodyReader::new(&data[entry.body_start..entry.end], entry.fourcc);
        fields.take(6)?;
        let index = fields.take(2)?;
        let index = u32::from(u16::from_be_bytes([index[0], index[1]]));
        if !(1..=data_references).contains(&index) {
            return Err(MergeError::malformed(
                &entry.fourcc,
                "data reference index refers to no data reference",
            ));
        }
    }
    if declared == 0 || found != declared {
        return Err(MergeError::malformed(
            b"stsd",
            "entry_count does not match the sample entries",
        ));
    }
    Ok(declared)
}
