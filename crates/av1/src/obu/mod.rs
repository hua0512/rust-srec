use std::io;

use bytes_util::{BitReader, BitWriter};
use utils::read_leb128;

pub mod seq;
pub mod utils;

/// OBU Header
/// AV1-Spec-2 - 5.3.2
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct ObuHeader {
    /// `obu_type`
    ///
    /// 4 bits
    pub obu_type: ObuType,
    /// `obu_size` if `obu_has_size_field` is 1
    ///
    /// leb128()
    pub size: Option<u64>,
    /// `obu_extension_header()` if `obu_extension_flag` is 1
    pub extension_header: Option<ObuExtensionHeader>,
}

/// Obu Header Extension
/// AV1-Spec-2 - 5.3.3
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub struct ObuExtensionHeader {
    /// `temporal_id`
    pub temporal_id: u8,
    /// `spatial_id`
    pub spatial_id: u8,
}

impl ObuHeader {
    /// Parses an OBU header from the given `cursor`.
    pub fn parse(cursor: &mut impl io::Read) -> io::Result<Self> {
        let mut bit_reader = BitReader::new(cursor);
        let forbidden_bit = bit_reader.read_bit()?;
        if forbidden_bit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "obu_forbidden_bit is not 0",
            ));
        }

        let obu_type = bit_reader.read_bits(4)?;
        let extension_flag = bit_reader.read_bit()?;
        let has_size_field = bit_reader.read_bit()?;

        bit_reader.read_bit()?; // reserved_1bit

        let extension_header = if extension_flag {
            let temporal_id = bit_reader.read_bits(3)?;
            let spatial_id = bit_reader.read_bits(2)?;
            bit_reader.read_bits(3)?; // reserved_3bits
            Some(ObuExtensionHeader {
                temporal_id: temporal_id as u8,
                spatial_id: spatial_id as u8,
            })
        } else {
            None
        };

        let size = if has_size_field {
            // obu_size
            Some(read_leb128(&mut bit_reader)?)
        } else {
            None
        };

        if !bit_reader.is_aligned() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bit reader is not aligned",
            ));
        }

        Ok(ObuHeader {
            obu_type: ObuType::from(obu_type as u8),
            size,
            extension_header,
        })
    }

    /// Writes this OBU header to the given writer.
    ///
    /// If `self.size` is `Some`, writes with `obu_has_size_field=1` and
    /// encodes the size as LEB128. If `None`, writes with `obu_has_size_field=0`.
    ///
    /// Returns the number of bytes written.
    pub fn mux<W: io::Write>(&self, writer: &mut W) -> io::Result<usize> {
        let mut bit_writer = BitWriter::new(writer);

        bit_writer.write_bit(false)?; // obu_forbidden_bit
        bit_writer.write_bits(u8::from(self.obu_type) as u64, 4)?;
        bit_writer.write_bit(self.extension_header.is_some())?;
        bit_writer.write_bit(self.size.is_some())?;
        bit_writer.write_bit(false)?; // obu_reserved_1bit

        if let Some(ext) = &self.extension_header {
            bit_writer.write_bits(ext.temporal_id as u64, 3)?;
            bit_writer.write_bits(ext.spatial_id as u64, 2)?;
            bit_writer.write_bits(0, 3)?; // extension_header_reserved_3bits
        }

        let mut bytes_written = if self.extension_header.is_some() { 2 } else { 1 };

        let writer = bit_writer.finish()?;

        if let Some(size) = self.size {
            bytes_written += utils::write_leb128(writer, size)?;
        }

        Ok(bytes_written)
    }

    /// Returns the encoded size of this OBU header in bytes.
    pub fn header_size(&self) -> usize {
        let base = if self.extension_header.is_some() { 2 } else { 1 };
        let size_field = self.size.map_or(0, utils::leb128_size);
        base + size_field
    }
}

/// OBU Type
/// AV1-Spec-2 - 6.2.2
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ObuType {
    /// `OBU_SEQUENCE_HEADER`
    SequenceHeader,
    /// `OBU_TEMPORAL_DELIMITER`
    TemporalDelimiter,
    /// `OBU_FRAME_HEADER`
    FrameHeader,
    /// `OBU_TILE_GROUP`
    TileGroup,
    /// `OBU_METADATA`
    Metadata,
    /// `OBU_FRAME`
    Frame,
    /// `OBU_REDUNDANT_FRAME_HEADER`
    RedundantFrameHeader,
    /// `OBU_TILE_LIST`
    TileList,
    /// `OBU_PADDING`
    Padding,
    /// Reserved
    Reserved(u8),
}

impl From<u8> for ObuType {
    fn from(value: u8) -> Self {
        match value {
            1 => ObuType::SequenceHeader,
            2 => ObuType::TemporalDelimiter,
            3 => ObuType::FrameHeader,
            4 => ObuType::TileGroup,
            5 => ObuType::Metadata,
            6 => ObuType::Frame,
            7 => ObuType::RedundantFrameHeader,
            8 => ObuType::TileList,
            15 => ObuType::Padding,
            _ => ObuType::Reserved(value),
        }
    }
}

impl From<ObuType> for u8 {
    fn from(value: ObuType) -> Self {
        match value {
            ObuType::SequenceHeader => 1,
            ObuType::TemporalDelimiter => 2,
            ObuType::FrameHeader => 3,
            ObuType::TileGroup => 4,
            ObuType::Metadata => 5,
            ObuType::Frame => 6,
            ObuType::RedundantFrameHeader => 7,
            ObuType::TileList => 8,
            ObuType::Padding => 15,
            ObuType::Reserved(value) => value,
        }
    }
}

#[cfg(test)]
#[cfg_attr(all(coverage_nightly, test), coverage(off))]
mod tests {
    use bytes::Buf;

    use super::*;

    #[test]
    fn test_obu_header_parse() {
        let mut cursor =
            std::io::Cursor::new(b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@");
        let header = ObuHeader::parse(&mut cursor).unwrap();
        insta::assert_debug_snapshot!(header, @r"
        ObuHeader {
            obu_type: SequenceHeader,
            size: Some(
                15,
            ),
            extension_header: None,
        }
        ");

        assert_eq!(cursor.position(), 2);
        assert_eq!(cursor.remaining(), 15);
    }

    #[test]
    fn test_obu_header_parse_no_size_field() {
        let mut cursor = std::io::Cursor::new(b"\x00");
        let header = ObuHeader::parse(&mut cursor).unwrap();
        insta::assert_debug_snapshot!(header, @r"
        ObuHeader {
            obu_type: Reserved(
                0,
            ),
            size: None,
            extension_header: None,
        }
        ");

        assert_eq!(cursor.position(), 1);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn test_obu_header_parse_extension_header() {
        let mut cursor = std::io::Cursor::new([0b00000100, 0b11010000]);
        let header = ObuHeader::parse(&mut cursor).unwrap();
        insta::assert_debug_snapshot!(header, @r"
        ObuHeader {
            obu_type: Reserved(
                0,
            ),
            size: None,
            extension_header: Some(
                ObuExtensionHeader {
                    temporal_id: 6,
                    spatial_id: 2,
                },
            ),
        }
        ");

        assert_eq!(cursor.position(), 2);
        assert_eq!(cursor.remaining(), 0);
    }

    #[test]
    fn test_obu_header_forbidden_bit_set() {
        let err = ObuHeader::parse(&mut std::io::Cursor::new(
            b"\xff\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@",
        ))
        .unwrap_err();
        insta::assert_debug_snapshot!(err, @r#"
        Custom {
            kind: InvalidData,
            error: "obu_forbidden_bit is not 0",
        }
        "#);
    }

    #[test]
    fn test_obu_to_from_u8() {
        let case = [
            (ObuType::SequenceHeader, 1),
            (ObuType::TemporalDelimiter, 2),
            (ObuType::FrameHeader, 3),
            (ObuType::TileGroup, 4),
            (ObuType::Metadata, 5),
            (ObuType::Frame, 6),
            (ObuType::RedundantFrameHeader, 7),
            (ObuType::TileList, 8),
            (ObuType::Padding, 15),
            (ObuType::Reserved(0), 0),
            (ObuType::Reserved(100), 100),
        ];

        for (obu_type, value) in case {
            assert_eq!(u8::from(obu_type), value);
            assert_eq!(ObuType::from(value), obu_type);
        }
    }

    #[test]
    fn test_obu_header_mux_round_trip() {
        // Test with size field, no extension
        let data = b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@";
        let mut cursor = std::io::Cursor::new(data.as_slice());
        let header = ObuHeader::parse(&mut cursor).unwrap();

        let mut buf = Vec::new();
        let written = header.mux(&mut buf).unwrap();
        assert_eq!(written, header.header_size());
        assert_eq!(&buf, &data[..written]);

        // Parse back and verify
        let mut cursor2 = std::io::Cursor::new(buf.as_slice());
        let header2 = ObuHeader::parse(&mut cursor2).unwrap();
        assert_eq!(header, header2);
    }

    #[test]
    fn test_obu_header_mux_no_size_field() {
        let header = ObuHeader {
            obu_type: ObuType::TemporalDelimiter,
            size: None,
            extension_header: None,
        };

        let mut buf = Vec::new();
        let written = header.mux(&mut buf).unwrap();
        assert_eq!(written, 1);
        assert_eq!(header.header_size(), 1);

        let mut cursor = std::io::Cursor::new(buf.as_slice());
        let parsed = ObuHeader::parse(&mut cursor).unwrap();
        assert_eq!(parsed, header);
    }

    #[test]
    fn test_obu_header_mux_with_extension() {
        let header = ObuHeader {
            obu_type: ObuType::Frame,
            size: Some(100),
            extension_header: Some(ObuExtensionHeader {
                temporal_id: 3,
                spatial_id: 1,
            }),
        };

        let mut buf = Vec::new();
        let written = header.mux(&mut buf).unwrap();
        assert_eq!(written, header.header_size());

        let mut cursor = std::io::Cursor::new(buf.as_slice());
        let parsed = ObuHeader::parse(&mut cursor).unwrap();
        assert_eq!(parsed, header);
    }
}
