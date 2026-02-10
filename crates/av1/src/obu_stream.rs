//! Low-overhead OBU bitstream parsing and writing.
//!
//! This module handles the low-overhead bitstream format defined in
//! the AV1 specification (Section 5.2), where OBUs are concatenated
//! with `obu_has_size_field=1`.
//!
//! Per the spec, `obu_has_size_field` MUST be 1 for all OBUs in the
//! pure low-overhead format. However, some container formats (ISO BMFF/MP4,
//! Matroska/WebM) allow the **last** OBU in a sample/block to have
//! `obu_has_size_field=0`, since the container frame boundary implies
//! the remaining size. This module enforces the strict requirement
//! (`obu_has_size_field=1` for all OBUs).

use std::io;

use bytes::Bytes;
use bytes_util::BytesCursorExt;

use crate::error::{Av1Error, Result};
use crate::obu::utils::leb128_size;
use crate::obu::{ObuExtensionHeader, ObuHeader, ObuType};

/// A single OBU with its header and payload data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obu {
    /// Parsed OBU header.
    pub header: ObuHeader,
    /// Raw OBU payload (not including the header or size field).
    pub data: Bytes,
}

/// Iterator over OBUs in a low-overhead bitstream.
///
/// Each call to `next()` parses one OBU header and extracts its payload.
/// Requires `obu_has_size_field=1` for all OBUs.
pub struct ObuIterator<'a> {
    reader: &'a mut io::Cursor<Bytes>,
}

impl<'a> ObuIterator<'a> {
    /// Creates a new iterator over OBUs in a low-overhead bitstream.
    pub fn new(reader: &'a mut io::Cursor<Bytes>) -> Self {
        Self { reader }
    }
}

impl Iterator for ObuIterator<'_> {
    type Item = Result<Obu>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.reader.get_ref().len() as u64 - self.reader.position();
        if remaining == 0 {
            return None;
        }

        Some(parse_obu(self.reader))
    }
}

/// Parses a single OBU from a `Cursor<Bytes>`, using zero-copy for the payload.
fn parse_obu(reader: &mut io::Cursor<Bytes>) -> Result<Obu> {
    let header = ObuHeader::parse(reader)?;

    let size = header.size.ok_or_else(|| {
        Av1Error::InvalidObu("obu_has_size_field must be 1 in low-overhead bitstream".into())
    })?;

    let data = reader.extract_bytes(size as usize).map_err(|_| Av1Error::UnexpectedEof {
        expected: size as usize,
        actual: (reader.get_ref().len() as u64 - reader.position()) as usize,
    })?;

    Ok(Obu { header, data })
}

/// Writes a single OBU in low-overhead bitstream format.
///
/// Constructs the OBU header with `obu_has_size_field=1` and writes the
/// header followed by the payload data.
///
/// Returns the total number of bytes written (header + payload).
pub fn write_obu<W: io::Write>(
    writer: &mut W,
    obu_type: ObuType,
    extension_header: Option<ObuExtensionHeader>,
    payload: &[u8],
) -> Result<usize> {
    let header = ObuHeader {
        obu_type,
        size: Some(payload.len() as u64),
        extension_header,
    };

    let header_bytes = header.mux(writer)?;
    writer.write_all(payload)?;

    Ok(header_bytes + payload.len())
}

/// Computes the total encoded size of an OBU in low-overhead bitstream format.
///
/// This includes the header bytes, the LEB128 size field, and the payload.
pub fn obu_encoded_size(extension_header: Option<&ObuExtensionHeader>, payload_len: usize) -> usize {
    let base = if extension_header.is_some() { 2 } else { 1 };
    let size_field = leb128_size(payload_len as u64);
    base + size_field + payload_len
}

/// Writes a raw [`Obu`] to the given writer in low-overhead bitstream format.
///
/// The OBU header is re-serialized with `obu_has_size_field=1` and the
/// size set to the length of `obu.data`.
///
/// Returns the total number of bytes written.
pub fn write_raw_obu<W: io::Write>(writer: &mut W, obu: &Obu) -> Result<usize> {
    write_obu(writer, obu.header.obu_type, obu.header.extension_header, &obu.data)
}

#[cfg(test)]
#[cfg_attr(all(coverage_nightly, test), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_obu() {
        // Sequence header OBU: type=1, has_size=1, size=15
        let data = b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@";
        let mut cursor = io::Cursor::new(Bytes::from_static(data));

        let mut iter = ObuIterator::new(&mut cursor);
        let obu = iter.next().unwrap().unwrap();
        assert_eq!(obu.header.obu_type, ObuType::SequenceHeader);
        assert_eq!(obu.header.size, Some(15));
        assert_eq!(obu.data.len(), 15);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_parse_multiple_obus() {
        // Build two OBUs: temporal delimiter (empty) + sequence header
        let mut data = Vec::new();
        write_obu(&mut data, ObuType::TemporalDelimiter, None, &[]).unwrap();
        write_obu(&mut data, ObuType::SequenceHeader, None, &[0xAA, 0xBB]).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(data));
        let mut iter = ObuIterator::new(&mut cursor);

        let obu1 = iter.next().unwrap().unwrap();
        assert_eq!(obu1.header.obu_type, ObuType::TemporalDelimiter);
        assert_eq!(obu1.data.len(), 0);

        let obu2 = iter.next().unwrap().unwrap();
        assert_eq!(obu2.header.obu_type, ObuType::SequenceHeader);
        assert_eq!(obu2.data.as_ref(), &[0xAA, 0xBB]);

        assert!(iter.next().is_none());
    }

    #[test]
    fn test_empty_stream() {
        let mut cursor = io::Cursor::new(Bytes::new());
        let mut iter = ObuIterator::new(&mut cursor);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_write_obu_round_trip() {
        let payload = b"test payload data";
        let mut buf = Vec::new();
        let written = write_obu(
            &mut buf,
            ObuType::Metadata,
            Some(ObuExtensionHeader {
                temporal_id: 2,
                spatial_id: 1,
            }),
            payload,
        )
        .unwrap();

        assert_eq!(written, buf.len());
        assert_eq!(
            written,
            obu_encoded_size(
                Some(&ObuExtensionHeader {
                    temporal_id: 2,
                    spatial_id: 1,
                }),
                payload.len(),
            ),
        );

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = ObuIterator::new(&mut cursor);
        let obu = iter.next().unwrap().unwrap();
        assert_eq!(obu.header.obu_type, ObuType::Metadata);
        assert_eq!(obu.header.extension_header.unwrap().temporal_id, 2);
        assert_eq!(obu.header.extension_header.unwrap().spatial_id, 1);
        assert_eq!(obu.data.as_ref(), payload);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_write_raw_obu_round_trip() {
        let original = Obu {
            header: ObuHeader {
                obu_type: ObuType::Frame,
                size: Some(5),
                extension_header: None,
            },
            data: Bytes::from_static(b"hello"),
        };

        let mut buf = Vec::new();
        write_raw_obu(&mut buf, &original).unwrap();

        let mut cursor = io::Cursor::new(Bytes::from(buf));
        let mut iter = ObuIterator::new(&mut cursor);
        let parsed = iter.next().unwrap().unwrap();
        assert_eq!(parsed.header.obu_type, original.header.obu_type);
        assert_eq!(parsed.data, original.data);
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_obu_without_size_field_errors() {
        // OBU header byte: type=1, extension=0, has_size=0, reserved=0
        // 0b0_0001_0_0_0 = 0x08
        let data = [0x08, 0xFF];
        let mut cursor = io::Cursor::new(Bytes::from(data.to_vec()));
        let mut iter = ObuIterator::new(&mut cursor);
        let err = iter.next().unwrap().unwrap_err();
        assert!(matches!(err, Av1Error::InvalidObu(_)));
    }
}
