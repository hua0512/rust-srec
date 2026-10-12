//! Segment Index box (`sidx`, ISO/IEC 14496-12 §8.16.3).
//!
//! A `sidx` documents how a media resource divides into subsegments: each
//! reference gives the byte size and duration of one subsegment, or points
//! to another `sidx` that indexes it further (hierarchical and daisy-chained
//! indexes). Byte positions are relative to the *anchor point*, the first
//! byte after the `sidx` box, so a reader needs the box's offset in its
//! file to turn references into byte ranges; this module only decodes the
//! box and finds it among top-level boxes.

use crate::fourcc::FourCc;

const SIDX: [u8; 4] = *b"sidx";

/// Bytes of one reference entry.
const REFERENCE_BYTES: usize = 12;

/// A decoded Segment Index box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentIndex {
    /// Stream ID (track ID) of the reference stream.
    pub reference_id: u32,
    /// Ticks per second of `earliest_presentation_time` and the subsegment
    /// durations; never zero.
    pub timescale: u32,
    /// Earliest presentation time of any content in the reference stream in
    /// the first subsegment, in `timescale` units.
    pub earliest_presentation_time: u64,
    /// Bytes from the anchor point to the first referenced item.
    pub first_offset: u64,
    pub references: Vec<SegmentReference>,
}

/// One entry of a [`SegmentIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentReference {
    pub kind: ReferenceKind,
    /// Bytes from the first byte of the referenced item to the first byte of
    /// the next one (or, for the last entry, to the end of the indexed
    /// material).
    pub referenced_size: u32,
    /// Duration in `timescale` units: of the subsegment, or of everything the
    /// referenced `sidx` indexes.
    pub subsegment_duration: u32,
    pub starts_with_sap: bool,
    pub sap_type: u8,
    pub sap_delta_time: u32,
}

/// What a [`SegmentReference`] points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// Media: a subsegment of `moof`/`mdat` pairs.
    Media,
    /// Another `sidx` box, which indexes the referenced bytes further.
    Index,
}

/// Where a `sidx` box sits in a buffer of top-level boxes (see [`locate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoxExtent {
    /// Offset of the box header in the buffer.
    pub start: u64,
    /// Offset one past the last byte of the box; may lie beyond the buffer
    /// when only the beginning of the box was read.
    pub end: u64,
}

impl BoxExtent {
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Why a Segment Index box could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SidxError {
    #[error("malformed `{fourcc}` box: {reason}")]
    Malformed {
        fourcc: FourCc,
        reason: &'static str,
    },
    /// The specification defines versions 0 and 1 only; the field layout of
    /// any other is unknown.
    #[error("unsupported `sidx` version {0}")]
    UnsupportedVersion(u8),
}

fn malformed(fourcc: [u8; 4], reason: &'static str) -> SidxError {
    SidxError::Malformed {
        fourcc: FourCc(fourcc),
        reason,
    }
}

/// A box header read from the start of `data`: `(size, fourcc, header
/// size)`. `None` when `data` is too short to hold it. A size of 0 ("to the
/// end of the file", §4.2) is returned as 0 for the caller to judge.
fn header(data: &[u8]) -> Option<(u64, [u8; 4], u64)> {
    let size = u32::from_be_bytes(data.get(..4)?.try_into().ok()?);
    let fourcc: [u8; 4] = data.get(4..8)?.try_into().ok()?;
    if size == 1 {
        let large = u64::from_be_bytes(data.get(8..16)?.try_into().ok()?);
        return Some((large, fourcc, 16));
    }
    Some((u64::from(size), fourcc, 8))
}

/// Finds the first `sidx` box among the top-level boxes at the start of
/// `data`, skipping any boxes before it (an `@indexRange` may begin with
/// `styp` or `free`).
///
/// Returns `None` when `data` ends before a `sidx` header is seen, so more
/// bytes might still hold one; the returned extent may end beyond `data`
/// when the box itself is only partly present.
pub fn locate(data: &[u8]) -> Result<Option<BoxExtent>, SidxError> {
    let mut offset: u64 = 0;
    loop {
        let Some(rest) = usize::try_from(offset).ok().and_then(|at| data.get(at..)) else {
            return Ok(None);
        };
        let Some((size, fourcc, header_size)) = header(rest) else {
            return Ok(None);
        };
        if size == 0 {
            // Valid only for the last box of a file, which an index range
            // cannot usefully end with.
            return Err(malformed(fourcc, "box extends to the end of the file"));
        }
        if size < header_size {
            return Err(malformed(fourcc, "box size is smaller than its header"));
        }
        let end = offset
            .checked_add(size)
            .ok_or_else(|| malformed(fourcc, "box size overflows"))?;
        if fourcc == SIDX {
            return Ok(Some(BoxExtent { start: offset, end }));
        }
        offset = end;
    }
}

impl SegmentIndex {
    /// Decodes one complete `sidx` box, header included.
    ///
    /// Bytes after the last reference are ignored. The reserved field is
    /// not checked.
    pub fn parse(data: &[u8]) -> Result<Self, SidxError> {
        let truncated = || malformed(SIDX, "box is truncated");
        let (size, fourcc, header_size) = header(data).ok_or_else(truncated)?;
        if fourcc != SIDX {
            return Err(malformed(fourcc, "not a Segment Index box"));
        }
        if size != data.len() as u64 {
            return Err(malformed(SIDX, "box size does not match the data"));
        }
        let mut reader = Reader {
            data,
            // A 16-byte header is only produced for sizes that need it, so
            // `header_size` fits in usize.
            position: header_size as usize,
        };
        let version = reader.u8().ok_or_else(truncated)?;
        reader.skip(3).ok_or_else(truncated)?; // flags
        if version > 1 {
            return Err(SidxError::UnsupportedVersion(version));
        }
        let reference_id = reader.u32().ok_or_else(truncated)?;
        let timescale = reader.u32().ok_or_else(truncated)?;
        if timescale == 0 {
            return Err(malformed(SIDX, "timescale is zero"));
        }
        let (earliest_presentation_time, first_offset) = if version == 0 {
            (
                u64::from(reader.u32().ok_or_else(truncated)?),
                u64::from(reader.u32().ok_or_else(truncated)?),
            )
        } else {
            (
                reader.u64().ok_or_else(truncated)?,
                reader.u64().ok_or_else(truncated)?,
            )
        };
        reader.skip(2).ok_or_else(truncated)?; // reserved
        let count = usize::from(reader.u16().ok_or_else(truncated)?);
        if reader.remaining() < count * REFERENCE_BYTES {
            return Err(malformed(SIDX, "references extend past the box"));
        }
        let mut references = Vec::with_capacity(count);
        for _ in 0..count {
            let (Some(size), Some(duration), Some(sap)) =
                (reader.u32(), reader.u32(), reader.u32())
            else {
                return Err(truncated());
            };
            references.push(SegmentReference {
                kind: if size >> 31 == 0 {
                    ReferenceKind::Media
                } else {
                    ReferenceKind::Index
                },
                referenced_size: size & 0x7fff_ffff,
                subsegment_duration: duration,
                starts_with_sap: sap >> 31 == 1,
                sap_type: ((sap >> 28) & 0x7) as u8,
                sap_delta_time: sap & 0x0fff_ffff,
            });
        }
        Ok(Self {
            reference_id,
            timescale,
            earliest_presentation_time,
            first_offset,
            references,
        })
    }
}

/// Big-endian reads from a byte slice; `None` past the end.
struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.position.checked_add(N)?;
        let bytes = self.data.get(self.position..end)?.try_into().ok()?;
        self.position = end;
        Some(bytes)
    }

    fn skip(&mut self, count: usize) -> Option<()> {
        let end = self.position.checked_add(count)?;
        if end > self.data.len() {
            return None;
        }
        self.position = end;
        Some(())
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|[byte]| byte)
    }

    fn u16(&mut self) -> Option<u16> {
        self.take().map(u16::from_be_bytes)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_be_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `sidx` box with the given header form and references
    /// `(type, size, duration, sap word)`.
    fn sidx(version: u8, large_header: bool, references: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        let mut body = vec![version, 0, 0, 0];
        body.extend_from_slice(&7u32.to_be_bytes()); // reference_ID
        body.extend_from_slice(&90_000u32.to_be_bytes());
        if version == 0 {
            body.extend_from_slice(&1_000u32.to_be_bytes());
            body.extend_from_slice(&16u32.to_be_bytes());
        } else {
            body.extend_from_slice(&(u64::from(u32::MAX) + 5).to_be_bytes());
            body.extend_from_slice(&(u64::from(u32::MAX) + 9).to_be_bytes());
        }
        body.extend_from_slice(&[0, 0]);
        body.extend_from_slice(&(references.len() as u16).to_be_bytes());
        for &(kind, size, duration, sap) in references {
            body.extend_from_slice(&((kind << 31) | size).to_be_bytes());
            body.extend_from_slice(&duration.to_be_bytes());
            body.extend_from_slice(&sap.to_be_bytes());
        }
        let mut data = Vec::new();
        if large_header {
            data.extend_from_slice(&1u32.to_be_bytes());
            data.extend_from_slice(b"sidx");
            data.extend_from_slice(&(16 + body.len() as u64).to_be_bytes());
        } else {
            data.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
            data.extend_from_slice(b"sidx");
        }
        data.extend_from_slice(&body);
        data
    }

    fn plain_box(fourcc: &[u8; 4], body: usize) -> Vec<u8> {
        let mut data = ((8 + body) as u32).to_be_bytes().to_vec();
        data.extend_from_slice(fourcc);
        data.resize(8 + body, 0);
        data
    }

    #[test]
    fn decodes_version_0() {
        let data = sidx(
            0,
            false,
            &[
                (0, 4_000, 180_000, 0x9000_0000),
                (1, 0x7fff_ffff, 3, 0x2fff_ffff),
            ],
        );
        let index = SegmentIndex::parse(&data).unwrap();
        assert_eq!(index.reference_id, 7);
        assert_eq!(index.timescale, 90_000);
        assert_eq!(index.earliest_presentation_time, 1_000);
        assert_eq!(index.first_offset, 16);
        assert_eq!(
            index.references,
            [
                SegmentReference {
                    kind: ReferenceKind::Media,
                    referenced_size: 4_000,
                    subsegment_duration: 180_000,
                    starts_with_sap: true,
                    sap_type: 1,
                    sap_delta_time: 0,
                },
                SegmentReference {
                    kind: ReferenceKind::Index,
                    referenced_size: 0x7fff_ffff,
                    subsegment_duration: 3,
                    starts_with_sap: false,
                    sap_type: 2,
                    sap_delta_time: 0x0fff_ffff,
                },
            ]
        );
    }

    #[test]
    fn decodes_version_1_with_a_large_size_header() {
        let data = sidx(1, true, &[(0, 10, 20, 0)]);
        let index = SegmentIndex::parse(&data).unwrap();
        assert_eq!(index.earliest_presentation_time, u64::from(u32::MAX) + 5);
        assert_eq!(index.first_offset, u64::from(u32::MAX) + 9);
        assert_eq!(index.references.len(), 1);
    }

    #[test]
    fn rejects_unknown_versions_and_bad_fields() {
        let mut data = sidx(0, false, &[]);
        data[8] = 2;
        assert_eq!(
            SegmentIndex::parse(&data),
            Err(SidxError::UnsupportedVersion(2))
        );

        let mut data = sidx(0, false, &[]);
        data[16..20].copy_from_slice(&[0; 4]);
        assert!(matches!(
            SegmentIndex::parse(&data),
            Err(SidxError::Malformed {
                reason: "timescale is zero",
                ..
            })
        ));

        let data = plain_box(b"moof", 40);
        assert!(matches!(
            SegmentIndex::parse(&data),
            Err(SidxError::Malformed {
                reason: "not a Segment Index box",
                ..
            })
        ));
    }

    #[test]
    fn a_reference_count_beyond_the_box_is_malformed() {
        let mut data = sidx(0, false, &[(0, 1, 1, 0)]);
        // Claim two references while carrying one.
        let count_at = data.len() - REFERENCE_BYTES - 2;
        data[count_at..count_at + 2].copy_from_slice(&2u16.to_be_bytes());
        assert!(matches!(
            SegmentIndex::parse(&data),
            Err(SidxError::Malformed {
                reason: "references extend past the box",
                ..
            })
        ));
    }

    #[test]
    fn trailing_bytes_after_the_references_are_ignored() {
        let mut data = sidx(0, false, &[(0, 1, 1, 0)]);
        data.extend_from_slice(&[0; 4]);
        let size = data.len() as u32;
        data[..4].copy_from_slice(&size.to_be_bytes());
        assert_eq!(SegmentIndex::parse(&data).unwrap().references.len(), 1);
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        for data in [
            sidx(0, false, &[(0, 1, 2, 3), (1, 4, 5, 6)]),
            sidx(1, true, &[(0, 1, 2, 3)]),
        ] {
            for len in 0..data.len() {
                assert!(SegmentIndex::parse(&data[..len]).is_err(), "length {len}");
                // `locate` sees a partial box, or not yet its header.
                if let Some(extent) = locate(&data[..len]).unwrap() {
                    assert_eq!(extent.end, data.len() as u64);
                }
            }
        }
    }

    #[test]
    fn locate_skips_leading_boxes() {
        let mut data = plain_box(b"ftyp", 12);
        data.extend(plain_box(b"moov", 100));
        let start = data.len() as u64;
        let index = sidx(0, false, &[(0, 1, 1, 0)]);
        data.extend(&index);
        data.extend(plain_box(b"moof", 8));
        assert_eq!(
            locate(&data).unwrap(),
            Some(BoxExtent {
                start,
                end: start + index.len() as u64
            })
        );
        let extent = locate(&data).unwrap().unwrap();
        let body = &data[extent.start as usize..extent.end as usize];
        assert!(SegmentIndex::parse(body).is_ok());
    }

    #[test]
    fn locate_reports_a_partly_read_box_and_nothing_found() {
        let index = sidx(0, false, &[(0, 1, 1, 0); 4]);
        assert_eq!(
            locate(&index[..12]).unwrap(),
            Some(BoxExtent {
                start: 0,
                end: index.len() as u64
            })
        );
        // A box running past the data hides whatever follows it.
        assert_eq!(locate(&plain_box(b"moov", 100)[..50]).unwrap(), None);
        assert_eq!(locate(&plain_box(b"free", 4)).unwrap(), None);
        assert_eq!(locate(&[]).unwrap(), None);
    }

    #[test]
    fn decodes_an_ffmpeg_segment_index() {
        // `styp`, then a version 1 `sidx` whose one reference spans the rest.
        let data = include_bytes!("../tests/fixtures/dash/audio-3.m4s");
        let extent = locate(data).unwrap().unwrap();
        assert_eq!((extent.start, extent.end), (24, 76));
        let index = SegmentIndex::parse(&data[24..76]).unwrap();
        assert_eq!(index.timescale, 48_000);
        assert_eq!(index.earliest_presentation_time, 95_232);
        let [reference] = index.references[..] else {
            panic!("one reference expected");
        };
        assert_eq!(reference.kind, ReferenceKind::Media);
        assert_eq!(reference.subsegment_duration, 768);
        assert_eq!(
            extent.end + index.first_offset + u64::from(reference.referenced_size),
            data.len() as u64
        );
    }

    #[test]
    fn locate_rejects_impossible_sizes() {
        let mut data = plain_box(b"free", 4);
        data[..4].copy_from_slice(&0u32.to_be_bytes());
        assert!(locate(&data).is_err());
        let mut data = plain_box(b"free", 4);
        data[..4].copy_from_slice(&4u32.to_be_bytes());
        assert!(locate(&data).is_err());
        let mut data = plain_box(b"styp", 4);
        data.extend_from_slice(&1u32.to_be_bytes());
        data.extend_from_slice(b"free");
        data.extend_from_slice(&u64::MAX.to_be_bytes());
        data.extend(plain_box(b"sidx", 4));
        // Skipping a box whose end overflows cannot reach the next one.
        assert!(locate(&data).is_err());
    }
}
