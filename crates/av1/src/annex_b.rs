//! Annex B length-delimited bitstream format parsing and writing.
//!
//! This module handles the Annex B format defined in the AV1 specification,
//! where temporal units and frame units are prefixed with LEB128 length fields.
//!
//! Per the spec, OBUs within Annex B may have `obu_has_size_field` set to
//! either 0 or 1. When set to 1, the `obu_size` field must be consistent
//! with the `obu_length` from the Annex B framing. The writer always produces
//! OBUs with `obu_has_size_field=0` for compactness.
//!
//! **Note**: The spec requires a temporal delimiter OBU as the first OBU in the
//! first frame unit of each temporal unit. This parser does not enforce that
//! constraint to remain tolerant of non-conformant streams; callers should
//! validate this if strict conformance is needed.
//!
//! Structure:
//! ```text
//! temporal_unit_size  (LEB128)
//!   frame_unit_size   (LEB128)
//!     obu_length      (LEB128)
//!     obu_header      (1 or 2 bytes)
//!     obu_data[]
//!   ...more frame units...
//! ...more temporal units...
//! ```

use std::io;

use bytes::Bytes;
use bytes_util::BytesCursorExt;

use crate::error::{Av1Error, Result};
use crate::obu::utils::{leb128_size, write_leb128};
use crate::obu::ObuHeader;
use crate::obu_stream::Obu;

/// A temporal unit parsed from an Annex B bitstream.
///
/// A temporal unit represents all data associated with a specific
/// presentation time and contains one or more frame units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemporalUnit {
    /// Frame units contained in this temporal unit.
    pub frame_units: Vec<FrameUnit>,
}

/// A frame unit within a temporal unit.
///
/// A frame unit contains one or more OBUs that together represent
/// a single frame's data (frame header + tile groups, or a single
/// frame OBU).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameUnit {
    /// OBUs contained in this frame unit.
    pub obus: Vec<Obu>,
}

/// Iterator over temporal units in an Annex B bitstream.
pub struct AnnexBIterator<'a> {
    reader: &'a mut io::Cursor<Bytes>,
}

impl<'a> AnnexBIterator<'a> {
    /// Creates a new Annex B iterator.
    pub fn new(reader: &'a mut io::Cursor<Bytes>) -> Self {
        Self { reader }
    }
}

impl Iterator for AnnexBIterator<'_> {
    type Item = Result<TemporalUnit>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.reader.get_ref().len() as u64 - self.reader.position();
        if remaining == 0 {
            return None;
        }

        Some(parse_temporal_unit(self.reader))
    }
}

/// Parses a single temporal unit from an Annex B bitstream.
fn parse_temporal_unit(reader: &mut io::Cursor<Bytes>) -> Result<TemporalUnit> {
    let tu_size = read_leb128_from_cursor(reader)?;
    let tu_start = reader.position();

    let mut frame_units = Vec::new();
    while reader.position() - tu_start < tu_size {
        frame_units.push(parse_frame_unit(reader)?);
    }

    let consumed = reader.position() - tu_start;
    if consumed != tu_size {
        return Err(Av1Error::TemporalUnitSizeMismatch {
            declared: tu_size,
            consumed,
        });
    }

    Ok(TemporalUnit { frame_units })
}

/// Parses a single frame unit from an Annex B bitstream.
fn parse_frame_unit(reader: &mut io::Cursor<Bytes>) -> Result<FrameUnit> {
    let fu_size = read_leb128_from_cursor(reader)?;
    let fu_start = reader.position();

    let mut obus = Vec::new();
    while reader.position() - fu_start < fu_size {
        let obu_length = read_leb128_from_cursor(reader)?;
        let obu_start = reader.position();

        // In Annex B, OBUs may have obu_has_size_field=0 (common) or 1 (also valid).
        // The obu_length field includes the header bytes + payload bytes.
        let header = ObuHeader::parse(reader)?;
        let header_bytes_consumed = reader.position() - obu_start;
        let payload_size = obu_length - header_bytes_consumed;

        // If obu_has_size_field=1, the header includes an obu_size field.
        // Verify it is consistent with the obu_length from the Annex B framing.
        if let Some(obu_size) = header.size
            && obu_size != payload_size
        {
            return Err(Av1Error::FrameUnitSizeMismatch {
                declared: obu_size,
                consumed: payload_size,
            });
        }

        let data = reader.extract_bytes(payload_size as usize).map_err(|_| {
            Av1Error::UnexpectedEof {
                expected: payload_size as usize,
                actual: (reader.get_ref().len() as u64 - reader.position()) as usize,
            }
        })?;

        obus.push(Obu { header, data });
    }

    let consumed = reader.position() - fu_start;
    if consumed != fu_size {
        return Err(Av1Error::FrameUnitSizeMismatch {
            declared: fu_size,
            consumed,
        });
    }

    Ok(FrameUnit { obus })
}

/// Reads a LEB128 value directly from a `Cursor<Bytes>`.
///
/// Per the AV1 spec, conforming bitstreams produce values `<= (1 << 32) - 1`.
/// Returns [`Av1Error::Leb128Overflow`] if the decoded value exceeds that limit.
fn read_leb128_from_cursor(reader: &mut io::Cursor<Bytes>) -> Result<u64> {
    use io::Read;

    let mut result = 0u64;
    for i in 0..8 {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)?;
        result |= ((byte[0] & 0x7f) as u64) << (i * 7);
        if byte[0] & 0x80 == 0 {
            if result > u32::MAX as u64 {
                return Err(Av1Error::Leb128Overflow);
            }
            return Ok(result);
        }
    }
    if result > u32::MAX as u64 {
        return Err(Av1Error::Leb128Overflow);
    }
    Ok(result)
}

/// Writes a temporal unit in Annex B format.
///
/// Returns the total number of bytes written.
pub fn write_temporal_unit<W: io::Write>(
    writer: &mut W,
    temporal_unit: &TemporalUnit,
) -> Result<usize> {
    let tu_payload_size = temporal_unit
        .frame_units
        .iter()
        .map(compute_frame_unit_size)
        .sum::<u64>();

    let mut total = write_leb128(writer, tu_payload_size)?;

    for fu in &temporal_unit.frame_units {
        total += write_frame_unit(writer, fu)?;
    }

    Ok(total)
}

/// Writes a single frame unit in Annex B format.
///
/// Returns the total number of bytes written.
pub fn write_frame_unit<W: io::Write>(writer: &mut W, frame_unit: &FrameUnit) -> Result<usize> {
    let fu_payload_size = compute_frame_unit_payload_size(frame_unit);

    let mut total = write_leb128(writer, fu_payload_size)?;

    for obu in &frame_unit.obus {
        total += write_annex_b_obu(writer, obu)?;
    }

    Ok(total)
}

/// Writes a single OBU in Annex B format (with obu_length prefix, obu_has_size_field=0).
fn write_annex_b_obu<W: io::Write>(writer: &mut W, obu: &Obu) -> Result<usize> {
    // Build header with obu_has_size_field=0
    let header_no_size = ObuHeader {
        obu_type: obu.header.obu_type,
        size: None, // obu_has_size_field=0
        extension_header: obu.header.extension_header,
    };

    let header_size = header_no_size.header_size();
    let obu_length = header_size as u64 + obu.data.len() as u64;

    let mut total = write_leb128(writer, obu_length)?;
    total += header_no_size.mux(writer)?;
    writer.write_all(&obu.data)?;
    total += obu.data.len();

    Ok(total)
}

/// Computes the total encoded size of a frame unit (including its LEB128 length prefix).
fn compute_frame_unit_size(frame_unit: &FrameUnit) -> u64 {
    let payload = compute_frame_unit_payload_size(frame_unit);
    leb128_size(payload) as u64 + payload
}

/// Computes the payload size of a frame unit (excluding its LEB128 length prefix).
fn compute_frame_unit_payload_size(frame_unit: &FrameUnit) -> u64 {
    frame_unit
        .obus
        .iter()
        .map(|obu| {
            let header_no_size = ObuHeader {
                obu_type: obu.header.obu_type,
                size: None,
                extension_header: obu.header.extension_header,
            };
            let header_size = header_no_size.header_size() as u64;
            let obu_length = header_size + obu.data.len() as u64;
            leb128_size(obu_length) as u64 + obu_length
        })
        .sum()
}

#[cfg(test)]
#[cfg_attr(all(coverage_nightly, test), coverage(off))]
mod tests {
    use super::*;
    use crate::obu::{ObuExtensionHeader, ObuType};

    fn make_obu(obu_type: ObuType, data: &[u8]) -> Obu {
        Obu {
            header: ObuHeader {
                obu_type,
                size: Some(data.len() as u64),
                extension_header: None,
            },
            data: Bytes::from(data.to_vec()),
        }
    }

    #[test]
    fn test_annex_b_single_tu_single_fu_single_obu() {
        let obu = make_obu(ObuType::SequenceHeader, b"seqhdr");
        let tu = TemporalUnit {
            frame_units: vec![FrameUnit {
                obus: vec![obu.clone()],
            }],
        };

        let mut buf = Vec::new();
        let written = write_temporal_unit(&mut buf, &tu).unwrap();
        assert_eq!(written, buf.len());

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);
        let parsed_tu = iter.next().unwrap().unwrap();
        assert!(iter.next().is_none());

        assert_eq!(parsed_tu.frame_units.len(), 1);
        assert_eq!(parsed_tu.frame_units[0].obus.len(), 1);
        assert_eq!(
            parsed_tu.frame_units[0].obus[0].header.obu_type,
            ObuType::SequenceHeader,
        );
        assert_eq!(parsed_tu.frame_units[0].obus[0].data.as_ref(), b"seqhdr");
    }

    #[test]
    fn test_annex_b_multiple_frame_units() {
        let tu = TemporalUnit {
            frame_units: vec![
                FrameUnit {
                    obus: vec![make_obu(ObuType::FrameHeader, b"fh")],
                },
                FrameUnit {
                    obus: vec![make_obu(ObuType::TileGroup, b"tiles")],
                },
            ],
        };

        let mut buf = Vec::new();
        write_temporal_unit(&mut buf, &tu).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);
        let parsed = iter.next().unwrap().unwrap();
        assert!(iter.next().is_none());

        assert_eq!(parsed.frame_units.len(), 2);
        assert_eq!(
            parsed.frame_units[0].obus[0].header.obu_type,
            ObuType::FrameHeader,
        );
        assert_eq!(parsed.frame_units[0].obus[0].data.as_ref(), b"fh");
        assert_eq!(
            parsed.frame_units[1].obus[0].header.obu_type,
            ObuType::TileGroup,
        );
        assert_eq!(parsed.frame_units[1].obus[0].data.as_ref(), b"tiles");
    }

    #[test]
    fn test_annex_b_multiple_temporal_units() {
        let tu1 = TemporalUnit {
            frame_units: vec![FrameUnit {
                obus: vec![make_obu(ObuType::Frame, b"frame1")],
            }],
        };
        let tu2 = TemporalUnit {
            frame_units: vec![FrameUnit {
                obus: vec![make_obu(ObuType::Frame, b"frame2")],
            }],
        };

        let mut buf = Vec::new();
        write_temporal_unit(&mut buf, &tu1).unwrap();
        write_temporal_unit(&mut buf, &tu2).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);

        let parsed1 = iter.next().unwrap().unwrap();
        assert_eq!(parsed1.frame_units[0].obus[0].data.as_ref(), b"frame1");

        let parsed2 = iter.next().unwrap().unwrap();
        assert_eq!(parsed2.frame_units[0].obus[0].data.as_ref(), b"frame2");

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_annex_b_empty_stream() {
        let mut cursor = io::Cursor::new(Bytes::new());
        let mut iter = AnnexBIterator::new(&mut cursor);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_annex_b_obu_with_extension() {
        let obu = Obu {
            header: ObuHeader {
                obu_type: ObuType::Metadata,
                size: Some(3),
                extension_header: Some(ObuExtensionHeader {
                    temporal_id: 2,
                    spatial_id: 1,
                }),
            },
            data: Bytes::from_static(b"ext"),
        };

        let tu = TemporalUnit {
            frame_units: vec![FrameUnit {
                obus: vec![obu],
            }],
        };

        let mut buf = Vec::new();
        write_temporal_unit(&mut buf, &tu).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);
        let parsed = iter.next().unwrap().unwrap();

        let parsed_obu = &parsed.frame_units[0].obus[0];
        assert_eq!(parsed_obu.header.obu_type, ObuType::Metadata);
        assert_eq!(parsed_obu.header.extension_header.unwrap().temporal_id, 2);
        assert_eq!(parsed_obu.header.extension_header.unwrap().spatial_id, 1);
        assert_eq!(parsed_obu.data.as_ref(), b"ext");
    }

    #[test]
    fn test_annex_b_multiple_obus_per_frame_unit() {
        let fu = FrameUnit {
            obus: vec![
                make_obu(ObuType::FrameHeader, b"header"),
                make_obu(ObuType::TileGroup, b"tiles_data"),
            ],
        };
        let tu = TemporalUnit {
            frame_units: vec![fu],
        };

        let mut buf = Vec::new();
        write_temporal_unit(&mut buf, &tu).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);
        let parsed = iter.next().unwrap().unwrap();

        assert_eq!(parsed.frame_units[0].obus.len(), 2);
        assert_eq!(
            parsed.frame_units[0].obus[0].header.obu_type,
            ObuType::FrameHeader,
        );
        assert_eq!(parsed.frame_units[0].obus[0].data.as_ref(), b"header");
        assert_eq!(
            parsed.frame_units[0].obus[1].header.obu_type,
            ObuType::TileGroup,
        );
        assert_eq!(parsed.frame_units[0].obus[1].data.as_ref(), b"tiles_data");
    }

    #[test]
    fn test_annex_b_obu_with_size_field_set() {
        // Build an Annex B stream where the inner OBU has obu_has_size_field=1.
        // This is valid per spec; the obu_size must be consistent with obu_length.
        let payload = b"data";

        // Manually construct:
        //   temporal_unit_size (LEB128)
        //     frame_unit_size (LEB128)
        //       obu_length (LEB128)
        //       obu_header (with obu_has_size_field=1, includes LEB128 obu_size)
        //       payload

        // OBU header: type=SequenceHeader(1), ext=0, has_size=1, reserved=0
        // Byte: 0b0_0001_0_1_0 = 0x0A
        let obu_header_byte = 0x0Au8;
        let obu_size_leb128 = [payload.len() as u8]; // 4 < 128, fits in 1 LEB128 byte
        let obu_length = 1 + obu_size_leb128.len() + payload.len(); // header + size + payload

        let mut frame_unit_payload = Vec::new();
        crate::obu::utils::write_leb128(&mut frame_unit_payload, obu_length as u64).unwrap();
        frame_unit_payload.push(obu_header_byte);
        frame_unit_payload.extend_from_slice(&obu_size_leb128);
        frame_unit_payload.extend_from_slice(payload);

        let mut temporal_unit_payload = Vec::new();
        crate::obu::utils::write_leb128(
            &mut temporal_unit_payload,
            frame_unit_payload.len() as u64,
        )
        .unwrap();
        temporal_unit_payload.extend_from_slice(&frame_unit_payload);

        let mut buf = Vec::new();
        crate::obu::utils::write_leb128(&mut buf, temporal_unit_payload.len() as u64).unwrap();
        buf.extend_from_slice(&temporal_unit_payload);

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = AnnexBIterator::new(&mut cursor);
        let parsed = iter.next().unwrap().unwrap();

        assert_eq!(parsed.frame_units.len(), 1);
        assert_eq!(parsed.frame_units[0].obus.len(), 1);
        let obu = &parsed.frame_units[0].obus[0];
        assert_eq!(obu.header.obu_type, ObuType::SequenceHeader);
        assert_eq!(obu.header.size, Some(4)); // obu_size field present
        assert_eq!(obu.data.as_ref(), payload);
    }
}
