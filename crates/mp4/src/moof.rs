//! Codec for movie fragment boxes (ISO/IEC 14496-12 §8.8.4–§8.8.8, §8.8.12).
//!
//! Only the boxes the fragment merger edits are modelled: `mfhd`, `traf`,
//! `tfhd`, `tfdt` and `trun`. A `trun` keeps its per-sample table as raw
//! bytes, so encoding a decoded value reproduces every field exactly; the
//! only differences from the input are that box headers are written in their
//! compact 32-bit form and padding boxes are dropped. Other `traf` children
//! are kept verbatim when they hold no byte offsets into the file, and
//! rejected otherwise: relocating the media data would silently invalidate
//! those offsets.

use bytes::Bytes;

use crate::box_utils::{BodyReader, children, write_box, write_full_box_header};
use crate::merge::MergeError;

/// `tfhd` flags (§8.8.7.1).
mod tfhd_flags {
    pub(super) const BASE_DATA_OFFSET: u32 = 0x00_0001;
    pub(super) const SAMPLE_DESCRIPTION_INDEX: u32 = 0x00_0002;
    pub(super) const DEFAULT_SAMPLE_DURATION: u32 = 0x00_0008;
    pub(super) const DEFAULT_SAMPLE_SIZE: u32 = 0x00_0010;
    pub(super) const DEFAULT_SAMPLE_FLAGS: u32 = 0x00_0020;
    pub(super) const DURATION_IS_EMPTY: u32 = 0x01_0000;
    pub(super) const DEFAULT_BASE_IS_MOOF: u32 = 0x02_0000;
    pub(super) const KNOWN: u32 = BASE_DATA_OFFSET
        | SAMPLE_DESCRIPTION_INDEX
        | DEFAULT_SAMPLE_DURATION
        | DEFAULT_SAMPLE_SIZE
        | DEFAULT_SAMPLE_FLAGS
        | DURATION_IS_EMPTY
        | DEFAULT_BASE_IS_MOOF;
}

/// `trun` flags (§8.8.8.1).
mod trun_flags {
    pub(super) const DATA_OFFSET: u32 = 0x00_0001;
    pub(super) const FIRST_SAMPLE_FLAGS: u32 = 0x00_0004;
    pub(super) const SAMPLE_DURATION: u32 = 0x00_0100;
    pub(super) const SAMPLE_SIZE: u32 = 0x00_0200;
    pub(super) const SAMPLE_FLAGS: u32 = 0x00_0400;
    pub(super) const SAMPLE_COMPOSITION_TIME_OFFSET: u32 = 0x00_0800;
    /// Flags that add one 32-bit field to every sample row.
    pub(super) const PER_SAMPLE: u32 =
        SAMPLE_DURATION | SAMPLE_SIZE | SAMPLE_FLAGS | SAMPLE_COMPOSITION_TIME_OFFSET;
    pub(super) const KNOWN: u32 = DATA_OFFSET | FIRST_SAMPLE_FLAGS | PER_SAMPLE;
}

/// Movie fragment box (§8.8.4) with its fragment header (§8.8.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Moof {
    pub(crate) sequence_number: u32,
    pub(crate) trafs: Vec<Traf>,
}

/// Track fragment box (§8.8.6). `tfhd` is always the first child; the order
/// of the remaining children is preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Traf {
    pub(crate) tfhd: Tfhd,
    pub(crate) children: Vec<TrafChild>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrafChild {
    Tfdt(Tfdt),
    Trun(Trun),
    /// A complete box (header included) that holds no file offsets.
    Opaque(Bytes),
}

/// Track fragment header box (§8.8.7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Tfhd {
    pub(crate) track_id: u32,
    pub(crate) base_data_offset: Option<u64>,
    pub(crate) sample_description_index: Option<u32>,
    pub(crate) default_sample_duration: Option<u32>,
    pub(crate) default_sample_size: Option<u32>,
    pub(crate) default_sample_flags: Option<u32>,
    pub(crate) duration_is_empty: bool,
    pub(crate) default_base_is_moof: bool,
}

/// Track fragment decode time box (§8.8.12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tfdt {
    version: u8,
    base_media_decode_time: u64,
}

/// Track fragment run box (§8.8.8). The sample table stays encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Trun {
    version: u8,
    sample_count: u32,
    pub(crate) data_offset: Option<i32>,
    first_sample_flags: Option<u32>,
    /// The subset of `trun_flags::PER_SAMPLE` present in each row.
    row_fields: u32,
    rows: Bytes,
}

/// Summed sample durations and sizes of one run, with `tfhd`/`trex`
/// defaults applied where the run stores no per-sample value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct RunTotals {
    pub(crate) duration: u64,
    pub(crate) size: u64,
}

impl Moof {
    /// Decode a complete `moof` box (header included).
    pub(crate) fn decode(data: &Bytes) -> Result<Self, MergeError> {
        let mut boxes = children(data, 0, data.len(), b"moof");
        let moof = match boxes.next() {
            Some(result) => result?,
            None => return Err(MergeError::malformed(b"moof", "empty input")),
        };
        if moof.fourcc != *b"moof" || moof.end != data.len() {
            return Err(MergeError::malformed(
                b"moof",
                "input is not exactly one moof box",
            ));
        }

        let mut sequence_number = None;
        let mut trafs = Vec::new();
        for child in children(data, moof.body_start, moof.end, b"moof") {
            let child = child?;
            let body = &data[child.body_start..child.end];
            match &child.fourcc {
                b"mfhd" => {
                    if sequence_number.is_some() || !trafs.is_empty() {
                        return Err(MergeError::malformed(
                            b"moof",
                            "mfhd must appear once, before any traf",
                        ));
                    }
                    sequence_number = Some(decode_mfhd(body)?);
                }
                b"traf" => {
                    if sequence_number.is_none() {
                        return Err(MergeError::malformed(
                            b"moof",
                            "mfhd must appear once, before any traf",
                        ));
                    }
                    trafs.push(Traf::decode(data, child.body_start, child.end)?);
                }
                b"pssh" => {
                    return Err(MergeError::Protected {
                        fourcc: b"pssh".into(),
                    });
                }
                b"free" | b"skip" => {}
                other => {
                    return Err(MergeError::unsupported(
                        other,
                        "box is not supported inside moof",
                    ));
                }
            }
        }

        Ok(Self {
            sequence_number: sequence_number
                .ok_or_else(|| MergeError::missing(b"mfhd", b"moof"))?,
            trafs,
        })
    }

    /// Append the encoded box to `out`. Returns, for every `trun` in order,
    /// the position in `out` of its `data_offset` field when present, so the
    /// caller can patch offsets once the box length is known.
    pub(crate) fn encode(&self, out: &mut Vec<u8>) -> Result<Vec<Option<usize>>, MergeError> {
        let mut data_offset_positions = Vec::new();
        write_box(out, b"moof", |out| {
            write_box(out, b"mfhd", |out| {
                write_full_box_header(out, 0, 0);
                out.extend_from_slice(&self.sequence_number.to_be_bytes());
                Ok(())
            })?;
            for traf in &self.trafs {
                traf.encode(out, &mut data_offset_positions)?;
            }
            Ok(())
        })?;
        Ok(data_offset_positions)
    }
}

fn decode_mfhd(body: &[u8]) -> Result<u32, MergeError> {
    let mut reader = BodyReader::new(body, *b"mfhd");
    if reader.full_box_header()? != (0, 0) {
        return Err(MergeError::unsupported(
            b"mfhd",
            "only version 0 with zero flags is defined",
        ));
    }
    let sequence_number = reader.u32()?;
    reader.finish()?;
    Ok(sequence_number)
}

impl Traf {
    fn decode(data: &Bytes, start: usize, end: usize) -> Result<Self, MergeError> {
        let mut tfhd = None;
        let mut has_tfdt = false;
        let mut traf_children = Vec::new();
        for child in children(data, start, end, b"traf") {
            let child = child?;
            let body = &data[child.body_start..child.end];
            if tfhd.is_none() && child.fourcc != *b"tfhd" {
                return Err(MergeError::malformed(
                    b"traf",
                    "tfhd must be the first child",
                ));
            }
            match &child.fourcc {
                b"tfhd" => {
                    if tfhd.is_some() {
                        return Err(MergeError::malformed(b"traf", "more than one tfhd"));
                    }
                    tfhd = Some(Tfhd::decode(body)?);
                }
                b"tfdt" => {
                    if has_tfdt {
                        return Err(MergeError::malformed(b"traf", "more than one tfdt"));
                    }
                    has_tfdt = true;
                    traf_children.push(TrafChild::Tfdt(Tfdt::decode(body)?));
                }
                b"trun" => traf_children.push(TrafChild::Trun(Trun::decode(
                    data.slice(child.body_start..child.end),
                )?)),
                // Sample groups and sub-sample information index samples, not
                // bytes, so they stay valid when the media data moves.
                b"sbgp" | b"sgpd" | b"subs" => {
                    check_opaque_child(&child.fourcc, body)?;
                    traf_children.push(TrafChild::Opaque(data.slice(child.start..child.end)));
                }
                b"senc" => {
                    return Err(MergeError::Protected {
                        fourcc: b"senc".into(),
                    });
                }
                b"saiz" | b"saio" => {
                    return Err(MergeError::unsupported(
                        &child.fourcc,
                        "auxiliary information offsets would be invalidated",
                    ));
                }
                b"free" | b"skip" => {}
                other => {
                    return Err(MergeError::unsupported(
                        other,
                        "box is not supported inside traf",
                    ));
                }
            }
        }
        Ok(Self {
            tfhd: tfhd.ok_or_else(|| MergeError::missing(b"tfhd", b"traf"))?,
            children: traf_children,
        })
    }

    fn encode(
        &self,
        out: &mut Vec<u8>,
        data_offset_positions: &mut Vec<Option<usize>>,
    ) -> Result<(), MergeError> {
        write_box(out, b"traf", |out| {
            self.tfhd.encode(out)?;
            for child in &self.children {
                match child {
                    TrafChild::Tfdt(tfdt) => tfdt.encode(out)?,
                    TrafChild::Trun(trun) => data_offset_positions.push(trun.encode(out)?),
                    TrafChild::Opaque(bytes) => out.extend_from_slice(bytes),
                }
            }
            Ok(())
        })
    }

    #[cfg(test)]
    pub(crate) fn tfdt(&self) -> Option<&Tfdt> {
        self.children.iter().find_map(|child| match child {
            TrafChild::Tfdt(tfdt) => Some(tfdt),
            _ => None,
        })
    }

    pub(crate) fn tfdt_mut(&mut self) -> Option<&mut Tfdt> {
        self.children.iter_mut().find_map(|child| match child {
            TrafChild::Tfdt(tfdt) => Some(tfdt),
            _ => None,
        })
    }

    pub(crate) fn truns(&self) -> impl Iterator<Item = &Trun> {
        self.children.iter().filter_map(|child| match child {
            TrafChild::Trun(trun) => Some(trun),
            _ => None,
        })
    }

    pub(crate) fn truns_mut(&mut self) -> impl Iterator<Item = &mut Trun> {
        self.children.iter_mut().filter_map(|child| match child {
            TrafChild::Trun(trun) => Some(trun),
            _ => None,
        })
    }
}

/// Minimal syntax check for `traf` children copied verbatim: a defined
/// version and the first field after the FullBox header (`grouping_type` for
/// `sbgp`/`sgpd` §8.9.2–§8.9.3, `entry_count` for `subs` §8.7.7). Their
/// contents index samples, so they need no rewriting.
fn check_opaque_child(fourcc: &[u8; 4], body: &[u8]) -> Result<(), MergeError> {
    let mut reader = BodyReader::new(body, *fourcc);
    let (version, _flags) = reader.full_box_header()?;
    let max_version = if fourcc == b"sgpd" { 2 } else { 1 };
    if version > max_version {
        return Err(MergeError::unsupported(fourcc, "undefined box version"));
    }
    reader.u32()?;
    Ok(())
}

impl Tfhd {
    fn decode(body: &[u8]) -> Result<Self, MergeError> {
        use tfhd_flags::*;

        let mut reader = BodyReader::new(body, *b"tfhd");
        let (version, flags) = reader.full_box_header()?;
        if version != 0 {
            return Err(MergeError::unsupported(
                b"tfhd",
                "only version 0 is defined",
            ));
        }
        // Undefined flags may add fields this decoder cannot see; reading
        // past them would misparse everything that follows.
        if flags & !KNOWN != 0 {
            return Err(MergeError::unsupported(b"tfhd", "undefined flags are set"));
        }
        let track_id = reader.u32()?;
        let present = |flag| flags & flag != 0;
        let tfhd = Self {
            track_id,
            base_data_offset: present(BASE_DATA_OFFSET)
                .then(|| reader.u64())
                .transpose()?,
            sample_description_index: present(SAMPLE_DESCRIPTION_INDEX)
                .then(|| reader.u32())
                .transpose()?,
            default_sample_duration: present(DEFAULT_SAMPLE_DURATION)
                .then(|| reader.u32())
                .transpose()?,
            default_sample_size: present(DEFAULT_SAMPLE_SIZE)
                .then(|| reader.u32())
                .transpose()?,
            default_sample_flags: present(DEFAULT_SAMPLE_FLAGS)
                .then(|| reader.u32())
                .transpose()?,
            duration_is_empty: present(DURATION_IS_EMPTY),
            default_base_is_moof: present(DEFAULT_BASE_IS_MOOF),
        };
        reader.finish()?;
        Ok(tfhd)
    }

    fn flags(&self) -> u32 {
        use tfhd_flags::*;

        [
            (self.base_data_offset.is_some(), BASE_DATA_OFFSET),
            (
                self.sample_description_index.is_some(),
                SAMPLE_DESCRIPTION_INDEX,
            ),
            (
                self.default_sample_duration.is_some(),
                DEFAULT_SAMPLE_DURATION,
            ),
            (self.default_sample_size.is_some(), DEFAULT_SAMPLE_SIZE),
            (self.default_sample_flags.is_some(), DEFAULT_SAMPLE_FLAGS),
            (self.duration_is_empty, DURATION_IS_EMPTY),
            (self.default_base_is_moof, DEFAULT_BASE_IS_MOOF),
        ]
        .into_iter()
        .filter(|(present, _)| *present)
        .fold(0, |flags, (_, flag)| flags | flag)
    }

    fn encode(&self, out: &mut Vec<u8>) -> Result<(), MergeError> {
        write_box(out, b"tfhd", |out| {
            write_full_box_header(out, 0, self.flags());
            out.extend_from_slice(&self.track_id.to_be_bytes());
            if let Some(offset) = self.base_data_offset {
                out.extend_from_slice(&offset.to_be_bytes());
            }
            for value in [
                self.sample_description_index,
                self.default_sample_duration,
                self.default_sample_size,
                self.default_sample_flags,
            ]
            .into_iter()
            .flatten()
            {
                out.extend_from_slice(&value.to_be_bytes());
            }
            Ok(())
        })
    }
}

impl Tfdt {
    fn decode(body: &[u8]) -> Result<Self, MergeError> {
        let mut reader = BodyReader::new(body, *b"tfdt");
        let (version, flags) = reader.full_box_header()?;
        if flags != 0 {
            return Err(MergeError::unsupported(b"tfdt", "undefined flags are set"));
        }
        let base_media_decode_time = match version {
            0 => u64::from(reader.u32()?),
            1 => reader.u64()?,
            _ => {
                return Err(MergeError::unsupported(
                    b"tfdt",
                    "only versions 0 and 1 are defined",
                ));
            }
        };
        reader.finish()?;
        Ok(Self {
            version,
            base_media_decode_time,
        })
    }

    pub(crate) fn base_media_decode_time(&self) -> u64 {
        self.base_media_decode_time
    }

    /// Set the decode time, switching to the 64-bit version 1 layout when the
    /// value no longer fits version 0.
    pub(crate) fn set_base_media_decode_time(&mut self, time: u64) {
        if time > u64::from(u32::MAX) {
            self.version = 1;
        }
        self.base_media_decode_time = time;
    }

    fn encode(&self, out: &mut Vec<u8>) -> Result<(), MergeError> {
        write_box(out, b"tfdt", |out| {
            write_full_box_header(out, self.version, 0);
            match u32::try_from(self.base_media_decode_time) {
                Ok(time) if self.version == 0 => out.extend_from_slice(&time.to_be_bytes()),
                _ => out.extend_from_slice(&self.base_media_decode_time.to_be_bytes()),
            }
            Ok(())
        })
    }
}

impl Trun {
    fn decode(body: Bytes) -> Result<Self, MergeError> {
        use trun_flags::*;

        let mut reader = BodyReader::new(&body, *b"trun");
        let (version, flags) = reader.full_box_header()?;
        if version > 1 {
            return Err(MergeError::unsupported(
                b"trun",
                "only versions 0 and 1 are defined",
            ));
        }
        if flags & !KNOWN != 0 {
            return Err(MergeError::unsupported(b"trun", "undefined flags are set"));
        }
        // The first-sample flags override the default for sample 0 only;
        // combined with per-sample flags it is unclear which applies.
        if flags & FIRST_SAMPLE_FLAGS != 0 && flags & SAMPLE_FLAGS != 0 {
            return Err(MergeError::unsupported(
                b"trun",
                "first-sample flags and per-sample flags are both present",
            ));
        }
        let sample_count = reader.u32()?;
        let data_offset = (flags & DATA_OFFSET != 0)
            .then(|| reader.i32())
            .transpose()?;
        let first_sample_flags = (flags & FIRST_SAMPLE_FLAGS != 0)
            .then(|| reader.u32())
            .transpose()?;
        let row_fields = flags & PER_SAMPLE;
        let rows_len = usize::try_from(sample_count)
            .ok()
            .and_then(|count| count.checked_mul(row_len(row_fields)))
            .ok_or_else(|| MergeError::malformed(b"trun", "sample table length overflows"))?;
        if rows_len != reader.remaining() {
            return Err(MergeError::malformed(
                b"trun",
                "sample table length does not match sample_count",
            ));
        }
        let rows_start = reader.position();
        Ok(Self {
            version,
            sample_count,
            data_offset,
            first_sample_flags,
            row_fields,
            rows: body.slice(rows_start..),
        })
    }

    /// Append the encoded box; returns the position of the `data_offset`
    /// field in `out` when present.
    fn encode(&self, out: &mut Vec<u8>) -> Result<Option<usize>, MergeError> {
        use trun_flags::*;

        let mut flags = self.row_fields;
        if self.data_offset.is_some() {
            flags |= DATA_OFFSET;
        }
        if self.first_sample_flags.is_some() {
            flags |= FIRST_SAMPLE_FLAGS;
        }
        let mut position = None;
        write_box(out, b"trun", |out| {
            write_full_box_header(out, self.version, flags);
            out.extend_from_slice(&self.sample_count.to_be_bytes());
            if let Some(offset) = self.data_offset {
                position = Some(out.len());
                out.extend_from_slice(&offset.to_be_bytes());
            }
            if let Some(sample_flags) = self.first_sample_flags {
                out.extend_from_slice(&sample_flags.to_be_bytes());
            }
            out.extend_from_slice(&self.rows);
            Ok(())
        })?;
        Ok(position)
    }

    pub(crate) fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// Total duration and size of the run. Values missing from the rows use
    /// the given defaults.
    ///
    /// Runs without per-sample fields are summed by multiplication: such a
    /// run may declare billions of samples in a few bytes. Neither sum can
    /// overflow, since each is at most `u32::MAX * u32::MAX`.
    pub(crate) fn totals(&self, default_duration: u32, default_size: u32) -> RunTotals {
        use trun_flags::*;

        let count = u64::from(self.sample_count);
        let has_duration = self.row_fields & SAMPLE_DURATION != 0;
        let has_size = self.row_fields & SAMPLE_SIZE != 0;
        let mut totals = RunTotals {
            duration: if has_duration {
                0
            } else {
                count * u64::from(default_duration)
            },
            size: if has_size {
                0
            } else {
                count * u64::from(default_size)
            },
        };
        if has_duration || has_size {
            // Decoding guaranteed `rows.len() == sample_count * row_len`.
            let size_at = if has_duration { 4 } else { 0 };
            let word = |row: &[u8], at: usize| {
                u64::from(u32::from_be_bytes([
                    row[at],
                    row[at + 1],
                    row[at + 2],
                    row[at + 3],
                ]))
            };
            for row in self.rows.chunks_exact(row_len(self.row_fields)) {
                if has_duration {
                    totals.duration += word(row, 0);
                }
                if has_size {
                    totals.size += word(row, size_at);
                }
            }
        }
        totals
    }
}

/// Bytes per `trun` sample row: one 32-bit field per per-sample flag.
fn row_len(row_fields: u32) -> usize {
    4 * (row_fields & trun_flags::PER_SAMPLE).count_ones() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_support::{make_box, make_full_box};

    fn tfhd_body(flags: u32) -> Vec<u8> {
        use tfhd_flags::*;

        let mut payload = 7u32.to_be_bytes().to_vec();
        if flags & BASE_DATA_OFFSET != 0 {
            payload.extend_from_slice(&0x0102_0304_0506_0708u64.to_be_bytes());
        }
        for (flag, value) in [
            (SAMPLE_DESCRIPTION_INDEX, 1u32),
            (DEFAULT_SAMPLE_DURATION, 1024),
            (DEFAULT_SAMPLE_SIZE, 300),
            (DEFAULT_SAMPLE_FLAGS, 0x0101_0000),
        ] {
            if flags & flag != 0 {
                payload.extend_from_slice(&value.to_be_bytes());
            }
        }
        make_full_box(b"tfhd", 0, flags, &payload)
    }

    fn trun_body(version: u8, flags: u32, samples: u32) -> Vec<u8> {
        use trun_flags::*;

        let mut payload = samples.to_be_bytes().to_vec();
        if flags & DATA_OFFSET != 0 {
            payload.extend_from_slice(&(-8i32).to_be_bytes());
        }
        if flags & FIRST_SAMPLE_FLAGS != 0 {
            payload.extend_from_slice(&0x0200_0000u32.to_be_bytes());
        }
        for sample in 0..samples {
            for (field, flag) in [
                SAMPLE_DURATION,
                SAMPLE_SIZE,
                SAMPLE_FLAGS,
                SAMPLE_COMPOSITION_TIME_OFFSET,
            ]
            .into_iter()
            .enumerate()
            {
                if flags & flag != 0 {
                    payload.extend_from_slice(&(sample * 16 + field as u32 + 1).to_be_bytes());
                }
            }
        }
        make_full_box(b"trun", version, flags, &payload)
    }

    /// Every subset of the bits in `mask`, including 0 and `mask` itself.
    fn subsets(mask: u32) -> impl Iterator<Item = u32> {
        let mut next = Some(0u32);
        std::iter::from_fn(move || {
            let current = next?;
            next = (current != mask).then(|| ((current | !mask).wrapping_add(1)) & mask);
            Some(current)
        })
    }

    fn moof(sequence: u32, traf_children: &[Vec<u8>]) -> Bytes {
        let mfhd = make_full_box(b"mfhd", 0, 0, &sequence.to_be_bytes());
        let traf = make_box(b"traf", &traf_children.concat());
        Bytes::from(make_box(b"moof", &[mfhd, traf].concat()))
    }

    fn roundtrip(input: &Bytes) -> Moof {
        let decoded = Moof::decode(input).unwrap();
        let mut encoded = Vec::new();
        decoded.encode(&mut encoded).unwrap();
        assert_eq!(encoded, input.as_ref(), "encode(decode(x)) must equal x");
        decoded
    }

    /// Every combination of defined `tfhd` flags round-trips byte for byte.
    #[test]
    fn every_tfhd_layout_roundtrips() {
        assert_eq!(subsets(tfhd_flags::KNOWN).count(), 1 << 7);
        for flags in subsets(tfhd_flags::KNOWN) {
            let input = moof(1, &[tfhd_body(flags)]);
            let decoded = roundtrip(&input);
            assert_eq!(decoded.trafs[0].tfhd.track_id, 7);
        }
    }

    /// Every `trun` version and flag combination round-trips byte for byte,
    /// for zero, one and several samples, and sums the stored values.
    #[test]
    fn every_trun_layout_roundtrips() {
        use trun_flags::*;

        assert_eq!(subsets(KNOWN).count(), 1 << 6);
        for version in 0..=1 {
            for flags in subsets(KNOWN) {
                if flags & FIRST_SAMPLE_FLAGS != 0 && flags & SAMPLE_FLAGS != 0 {
                    let input = moof(1, &[tfhd_body(0), trun_body(version, flags, 1)]);
                    assert!(matches!(
                        Moof::decode(&input),
                        Err(MergeError::Unsupported { .. })
                    ));
                    continue;
                }
                for samples in [0u32, 1, 3] {
                    let input = moof(1, &[tfhd_body(0), trun_body(version, flags, samples)]);
                    let decoded = roundtrip(&input);
                    let trun = decoded.trafs[0].truns().next().unwrap();
                    assert_eq!(trun.sample_count(), samples);
                    // Row values are `sample * 16 + field + 1`, where `field`
                    // is 0 for duration and 1 for size.
                    let stored = |field: u32| {
                        (0..samples)
                            .map(|sample| u64::from(sample * 16 + field + 1))
                            .sum::<u64>()
                    };
                    let expected = RunTotals {
                        duration: if flags & SAMPLE_DURATION != 0 {
                            stored(0)
                        } else {
                            u64::from(samples) * 20
                        },
                        size: if flags & SAMPLE_SIZE != 0 {
                            stored(1)
                        } else {
                            u64::from(samples) * 30
                        },
                    };
                    assert_eq!(
                        trun.totals(20, 30),
                        expected,
                        "version {version} flags {flags:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn opaque_children_keep_their_order_and_bytes() {
        let sgpd = make_full_box(
            b"sgpd",
            1,
            0,
            &[b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 0xff, 0xfe],
        );
        let sbgp = make_full_box(
            b"sbgp",
            0,
            0,
            &[b'r', b'o', b'l', b'l', 0, 0, 0, 1, 0, 0, 0, 5, 0, 0, 0, 1],
        );
        let tfdt = make_full_box(b"tfdt", 1, 0, &90_000u64.to_be_bytes());
        let input = moof(
            9,
            &[
                tfhd_body(0x02_0000),
                tfdt,
                trun_body(0, 0x201, 2),
                sgpd,
                sbgp,
                trun_body(1, 0x300, 1),
            ],
        );
        let decoded = roundtrip(&input);
        assert_eq!(decoded.sequence_number, 9);
        assert_eq!(decoded.trafs[0].truns().count(), 2);
        assert_eq!(
            decoded.trafs[0].tfdt().unwrap().base_media_decode_time(),
            90_000
        );
    }

    #[test]
    fn tfdt_upgrades_to_version_1_only_when_needed() {
        let input = moof(
            1,
            &[
                tfhd_body(0),
                make_full_box(b"tfdt", 0, 0, &5u32.to_be_bytes()),
            ],
        );
        let mut decoded = roundtrip(&input);
        let tfdt = decoded.trafs[0].tfdt_mut().unwrap();
        tfdt.set_base_media_decode_time(u64::from(u32::MAX));
        assert_eq!(tfdt.version, 0);
        tfdt.set_base_media_decode_time(u64::from(u32::MAX) + 1);
        assert_eq!(tfdt.version, 1);

        let mut encoded = Vec::new();
        decoded.encode(&mut encoded).unwrap();
        let again = Moof::decode(&Bytes::from(encoded)).unwrap();
        assert_eq!(
            again.trafs[0].tfdt().unwrap().base_media_decode_time(),
            u64::from(u32::MAX) + 1
        );
    }

    #[test]
    fn encode_reports_data_offset_positions() {
        let input = moof(
            1,
            &[tfhd_body(0), trun_body(0, 0x201, 1), trun_body(0, 0x200, 1)],
        );
        let decoded = Moof::decode(&input).unwrap();
        let mut out = vec![0xaa; 3];
        let positions = decoded.encode(&mut out).unwrap();
        assert_eq!(positions.len(), 2);
        let position = positions[0].unwrap();
        assert_eq!(out[position..position + 4], (-8i32).to_be_bytes());
        assert_eq!(positions[1], None);
    }

    #[test]
    fn rejects_layouts_it_cannot_rewrite_safely() {
        let cases: Vec<(Vec<Vec<u8>>, MergeError)> = vec![
            (
                vec![tfhd_body(0), make_full_box(b"senc", 0, 0, &[0; 4])],
                MergeError::Protected {
                    fourcc: b"senc".into(),
                },
            ),
            (
                vec![
                    tfhd_body(0),
                    make_full_box(b"saio", 0, 0, &[0, 0, 0, 1, 0, 0, 0, 0]),
                ],
                MergeError::unsupported(
                    b"saio",
                    "auxiliary information offsets would be invalidated",
                ),
            ),
            (
                vec![tfhd_body(0), make_box(b"uuid", &[0; 16])],
                MergeError::unsupported(b"uuid", "box is not supported inside traf"),
            ),
            (
                // A `uuid` box without room for its 16-byte extended type.
                vec![tfhd_body(0), make_box(b"uuid", &[0; 8])],
                MergeError::malformed(b"traf", "child box is truncated or larger than its parent"),
            ),
            (
                vec![tfhd_body(0), make_full_box(b"sbgp", 0, 0, &[])],
                MergeError::malformed(b"sbgp", "box body is truncated"),
            ),
            (
                vec![tfhd_body(0), make_full_box(b"sgpd", 3, 0, b"roll")],
                MergeError::unsupported(b"sgpd", "undefined box version"),
            ),
            (
                vec![make_full_box(b"tfdt", 0, 0, &[0; 4]), tfhd_body(0)],
                MergeError::malformed(b"traf", "tfhd must be the first child"),
            ),
            (
                vec![make_full_box(b"tfhd", 0, 0x40, &7u32.to_be_bytes())],
                MergeError::unsupported(b"tfhd", "undefined flags are set"),
            ),
            (
                vec![
                    tfhd_body(0),
                    make_full_box(b"trun", 2, 0, &0u32.to_be_bytes()),
                ],
                MergeError::unsupported(b"trun", "only versions 0 and 1 are defined"),
            ),
            (
                vec![
                    tfhd_body(0),
                    make_full_box(b"trun", 0, 0x200, &2u32.to_be_bytes()),
                ],
                MergeError::malformed(b"trun", "sample table length does not match sample_count"),
            ),
            (
                vec![tfhd_body(0), make_full_box(b"tfdt", 1, 0, &[0; 4])],
                MergeError::malformed(b"tfdt", "box body is truncated"),
            ),
        ];
        for (traf_children, expected) in cases {
            assert_eq!(Moof::decode(&moof(1, &traf_children)), Err(expected));
        }

        let pssh = Bytes::from(make_box(
            b"moof",
            &[
                make_full_box(b"mfhd", 0, 0, &[0; 4]),
                make_full_box(b"pssh", 0, 0, &[0; 20]),
            ]
            .concat(),
        ));
        assert_eq!(
            Moof::decode(&pssh),
            Err(MergeError::Protected {
                fourcc: b"pssh".into()
            })
        );
    }

    /// A huge `sample_count` with no per-sample fields allocates nothing and
    /// still decodes; with per-sample fields it is rejected before any
    /// allocation because the table cannot fit in the box.
    #[test]
    fn sample_count_is_bounded_by_the_box() {
        let empty_rows = moof(1, &[tfhd_body(0), trun_body(0, 0, 0)]);
        let mut patched = empty_rows.to_vec();
        let count_at = patched.len() - 4;
        patched[count_at..].copy_from_slice(&u32::MAX.to_be_bytes());
        let decoded = Moof::decode(&Bytes::from(patched)).unwrap();
        let trun = decoded.trafs[0].truns().next().unwrap();
        assert_eq!(trun.sample_count(), u32::MAX);
        assert_eq!(
            trun.totals(u32::MAX, 1),
            RunTotals {
                duration: u64::from(u32::MAX) * u64::from(u32::MAX),
                size: u64::from(u32::MAX),
            }
        );

        let input = moof(
            1,
            &[
                tfhd_body(0),
                make_full_box(b"trun", 0, 0x200, &u32::MAX.to_be_bytes()),
            ],
        );
        assert!(matches!(
            Moof::decode(&input),
            Err(MergeError::Malformed { .. })
        ));
    }

    /// Truncating a valid box at every offset yields an error, never a panic.
    #[test]
    fn truncation_at_every_offset_is_an_error() {
        let input = moof(
            3,
            &[
                tfhd_body(tfhd_flags::KNOWN),
                make_full_box(b"tfdt", 1, 0, &1u64.to_be_bytes()),
                trun_body(1, trun_flags::KNOWN & !trun_flags::FIRST_SAMPLE_FLAGS, 4),
            ],
        );
        roundtrip(&input);
        for end in 0..input.len() {
            assert!(
                Moof::decode(&input.slice(..end)).is_err(),
                "prefix of {end} bytes"
            );
        }
    }

    /// Corrupting each byte of a valid box never panics.
    #[test]
    fn byte_corruption_never_panics() {
        let input = moof(
            3,
            &[
                tfhd_body(tfhd_flags::DEFAULT_SAMPLE_SIZE | tfhd_flags::DEFAULT_BASE_IS_MOOF),
                make_full_box(b"tfdt", 0, 0, &1u32.to_be_bytes()),
                trun_body(0, 0x0b01, 2),
            ],
        );
        for index in 0..input.len() {
            for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut corrupted = input.to_vec();
                corrupted[index] = value;
                if let Ok(decoded) = Moof::decode(&Bytes::from(corrupted)) {
                    let mut out = Vec::new();
                    decoded.encode(&mut out).unwrap();
                    for trun in decoded.trafs.iter().flat_map(Traf::truns) {
                        trun.totals(1, 1);
                    }
                }
            }
        }
    }
}
