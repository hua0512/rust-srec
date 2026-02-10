use std::io;

use byteorder::ReadBytesExt;
use bytes::Bytes;
use bytes_util::{BitReader, BitWriter, BytesCursorExt};

/// AV1 Video Descriptor
///
/// <https://aomediacodec.github.io/av1-mpeg2-ts/#av1-video-descriptor>
#[derive(Debug, Clone, PartialEq)]
pub struct AV1VideoDescriptor {
    /// This value shall be set to `0x80`.
    ///
    /// 8 bits
    pub tag: u8,
    /// This value shall be set to 4.
    ///
    /// 8 bits
    pub length: u8,
    /// AV1 Codec Configuration Record
    pub codec_configuration_record: AV1CodecConfigurationRecord,
}

impl AV1VideoDescriptor {
    /// Demuxes the AV1 Video Descriptor from the given reader.
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        let tag = reader.read_u8()?;
        if tag != 0x80 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid AV1 video descriptor tag",
            ));
        }

        let length = reader.read_u8()?;
        if length != 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid AV1 video descriptor length",
            ));
        }

        Ok(AV1VideoDescriptor {
            tag,
            length,
            codec_configuration_record: AV1CodecConfigurationRecord::demux_mpeg2_ts(reader)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
/// AV1 Codec Configuration Record
///
/// <https://aomediacodec.github.io/av1-isobmff/#av1codecconfigurationbox-syntax>
pub struct AV1CodecConfigurationRecord {
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf).
    ///
    /// 3 bits
    pub seq_profile: u8,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf).
    ///
    /// 5 bits
    pub seq_level_idx_0: u8,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 1 bit
    pub seq_tier_0: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf).
    ///
    /// 1 bit
    pub high_bitdepth: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 1 bit
    pub twelve_bit: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 1 bit
    pub monochrome: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 1 bit
    pub chroma_subsampling_x: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 1 bit
    pub chroma_subsampling_y: bool,
    /// This field shall be coded according to the semantics defined in [AV1](https://aomediacodec.github.io/av1-spec/av1-spec.pdf), when present.
    /// If they are not present, they will be coded using the value inferred by the semantics.
    ///
    /// 2 bits
    pub chroma_sample_position: u8,
    /// The value of this syntax element indicates the presence or absence of high dynamic range (HDR) and/or
    /// wide color gamut (WCG) video components in the associated PID according to the table below.
    ///
    /// | HDR/WCG IDC | Description   |
    /// |-------------|---------------|
    /// | 0           | SDR           |
    /// | 1           | WCG only      |
    /// | 2           | HDR and WCG   |
    /// | 3           | No indication |
    ///
    /// 2 bits
    ///
    /// This is only signaled in the MPEG-2 TS AV1 video descriptor variant.
    ///
    /// For ISOBMFF `av1C`, these bits are reserved and this field is set to `0` by parser/writer.
    ///
    /// MPEG-2 TS reference: <https://aomediacodec.github.io/av1-mpeg2-ts/#av1-video-descriptor>
    pub hdr_wcg_idc: u8,
    /// Ignored for [MPEG-2 TS](https://www.iso.org/standard/83239.html) use,
    /// included only to aid conversion to/from ISOBMFF.
    ///
    /// 4 bits
    pub initial_presentation_delay_minus_one: Option<u8>,
    /// Zero or more OBUs. Refer to the linked specification for details.
    ///
    /// 8 bits
    pub config_obu: Bytes,
}

impl AV1CodecConfigurationRecord {
    /// Returns the `config_obu` payload as a zero-copy `Bytes` slice.
    ///
    /// The AV1 codec configuration record (`av1C` payload) has a fixed-size
    /// 4-byte header. Any remaining bytes are the `configOBUs` field.
    ///
    /// This helper validates the marker bit and version and then returns
    /// `data[4..]` as a `Bytes` slice without running the bit-level parser.
    pub fn config_obu_bytes(data: &Bytes) -> io::Result<Bytes> {
        if data.len() < 4 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "AV1 codec configuration record is too short",
            ));
        }

        let b0 = data[0];
        if b0 & 0x80 == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "marker is not set",
            ));
        }

        let version = b0 & 0x7f;
        if version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "version is not 1",
            ));
        }

        Ok(data.slice(4..))
    }

    /// Demuxes the ISOBMFF AV1 Codec Configuration Record (`av1C`) from the given reader.
    ///
    /// In this variant, 3 bits are reserved after `chroma_sample_position` and must be ignored.
    /// `hdr_wcg_idc` is set to `0`.
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        Self::demux_inner(reader, false)
    }

    /// Demuxes the MPEG-2 TS AV1 video-descriptor record from the given reader.
    ///
    /// In this variant, 2 of the reserved bits are interpreted as `hdr_wcg_idc`.
    pub fn demux_mpeg2_ts(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        Self::demux_inner(reader, true)
    }

    fn demux_inner(reader: &mut io::Cursor<Bytes>, mpeg2_ts_variant: bool) -> io::Result<Self> {
        let mut bit_reader = BitReader::new(reader);

        let marker = bit_reader.read_bit()?;
        if !marker {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "marker is not set",
            ));
        }

        let version = bit_reader.read_bits(7)? as u8;
        if version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "version is not 1",
            ));
        }

        let seq_profile = bit_reader.read_bits(3)? as u8;
        let seq_level_idx_0 = bit_reader.read_bits(5)? as u8;

        let seq_tier_0 = bit_reader.read_bit()?;
        let high_bitdepth = bit_reader.read_bit()?;
        let twelve_bit = bit_reader.read_bit()?;
        let monochrome = bit_reader.read_bit()?;
        let chroma_subsampling_x = bit_reader.read_bit()?;
        let chroma_subsampling_y = bit_reader.read_bit()?;
        let chroma_sample_position = bit_reader.read_bits(2)? as u8;

        let hdr_wcg_idc = if mpeg2_ts_variant {
            let idc = bit_reader.read_bits(2)? as u8;
            bit_reader.seek_bits(1)?; // reserved 1 bit
            idc
        } else {
            bit_reader.seek_bits(3)?; // reserved 3 bits
            0
        };

        let initial_presentation_delay_minus_one = if bit_reader.read_bit()? {
            Some(bit_reader.read_bits(4)? as u8)
        } else {
            bit_reader.seek_bits(4)?; // reserved 4 bits
            None
        };

        if !bit_reader.is_aligned() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Bit reader is not aligned",
            ));
        }

        let reader = bit_reader.into_inner();

        Ok(AV1CodecConfigurationRecord {
            seq_profile,
            seq_level_idx_0,
            seq_tier_0,
            high_bitdepth,
            twelve_bit,
            monochrome,
            chroma_subsampling_x,
            chroma_subsampling_y,
            chroma_sample_position,
            hdr_wcg_idc,
            initial_presentation_delay_minus_one,
            config_obu: reader.extract_remaining(),
        })
    }

    /// Returns the size of the AV1 Codec Configuration Record.
    pub fn size(&self) -> u64 {
        1 // marker, version
        + 1 // seq_profile, seq_level_idx_0
        + 1 // seq_tier_0, high_bitdepth, twelve_bit, monochrome, chroma_subsampling_x, chroma_subsampling_y, chroma_sample_position
        + 1 // reserved, initial_presentation_delay_present, initial_presentation_delay_minus_one/reserved
        + self.config_obu.len() as u64
    }

    /// Muxes the ISOBMFF AV1 Codec Configuration Record (`av1C`) to the given writer.
    ///
    /// Writes 3 reserved bits as zero after `chroma_sample_position`.
    pub fn mux<T: io::Write>(&self, writer: &mut T) -> io::Result<()> {
        self.mux_inner(writer, false)
    }

    /// Muxes the MPEG-2 TS AV1 video-descriptor record to the given writer.
    ///
    /// Writes `hdr_wcg_idc` (2 bits) followed by one reserved zero bit.
    pub fn mux_mpeg2_ts<T: io::Write>(&self, writer: &mut T) -> io::Result<()> {
        self.mux_inner(writer, true)
    }

    fn mux_inner<T: io::Write>(&self, writer: &mut T, mpeg2_ts_variant: bool) -> io::Result<()> {
        let mut bit_writer = BitWriter::new(writer);

        bit_writer.write_bit(true)?; // marker
        bit_writer.write_bits(1, 7)?; // version

        bit_writer.write_bits(self.seq_profile as u64, 3)?;
        bit_writer.write_bits(self.seq_level_idx_0 as u64, 5)?;

        bit_writer.write_bit(self.seq_tier_0)?;
        bit_writer.write_bit(self.high_bitdepth)?;
        bit_writer.write_bit(self.twelve_bit)?;
        bit_writer.write_bit(self.monochrome)?;
        bit_writer.write_bit(self.chroma_subsampling_x)?;
        bit_writer.write_bit(self.chroma_subsampling_y)?;
        bit_writer.write_bits(self.chroma_sample_position as u64, 2)?;

        if mpeg2_ts_variant {
            bit_writer.write_bits((self.hdr_wcg_idc & 0b11) as u64, 2)?;
            bit_writer.write_bits(0, 1)?; // reserved 1 bit
        } else {
            bit_writer.write_bits(0, 3)?; // reserved 3 bits
        }

        if let Some(initial_presentation_delay_minus_one) =
            self.initial_presentation_delay_minus_one
        {
            bit_writer.write_bit(true)?;
            bit_writer.write_bits(initial_presentation_delay_minus_one as u64, 4)?;
        } else {
            bit_writer.write_bit(false)?;
            bit_writer.write_bits(0, 4)?; // reserved 4 bits
        }

        bit_writer.finish()?.write_all(&self.config_obu)?;

        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {

    use super::*;

    #[test]
    fn test_config_demux() {
        let data = b"\x81\r\x0c\0\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@".to_vec();

        let config = AV1CodecConfigurationRecord::demux(&mut io::Cursor::new(data.into())).unwrap();

        insta::assert_debug_snapshot!(config, @r#"
        AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 13,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0,
            initial_presentation_delay_minus_one: None,
            config_obu: b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@",
        }
        "#);
    }

    #[test]
    fn test_config_obu_bytes_happy_path() {
        let data = Bytes::from_static(
            b"\x81\r\x0c\0\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@",
        );
        let obu = AV1CodecConfigurationRecord::config_obu_bytes(&data).unwrap();
        assert_eq!(obu.as_ref(), &data.as_ref()[4..]);
    }

    #[test]
    fn test_config_obu_bytes_invalid_marker() {
        let data = Bytes::from_static(b"\x01\x00\x00\x00");
        let err = AV1CodecConfigurationRecord::config_obu_bytes(&data).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "marker is not set");
    }

    #[test]
    fn test_config_obu_bytes_invalid_version() {
        // marker set, version=2
        let data = Bytes::from_static(b"\x82\x00\x00\x00");
        let err = AV1CodecConfigurationRecord::config_obu_bytes(&data).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "version is not 1");
    }

    #[test]
    fn test_config_obu_bytes_too_short() {
        let data = Bytes::from_static(b"\x81\x00\x00");
        let err = AV1CodecConfigurationRecord::config_obu_bytes(&data).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn test_marker_is_not_set() {
        let data = vec![0b00000000];

        let err =
            AV1CodecConfigurationRecord::demux(&mut io::Cursor::new(data.into())).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "marker is not set");
    }

    #[test]
    fn test_version_is_not_1() {
        let data = vec![0b10000000];

        let err =
            AV1CodecConfigurationRecord::demux(&mut io::Cursor::new(data.into())).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "version is not 1");
    }

    #[test]
    fn test_config_demux_with_initial_presentation_delay() {
        let data = b"\x81\r\x0c\x3f\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@".to_vec();

        let config = AV1CodecConfigurationRecord::demux(&mut io::Cursor::new(data.into())).unwrap();

        insta::assert_debug_snapshot!(config, @r#"
        AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 13,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: true,
            chroma_subsampling_y: true,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0,
            initial_presentation_delay_minus_one: Some(
                15,
            ),
            config_obu: b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@",
        }
        "#);
    }

    #[test]
    fn test_config_mux() {
        let config = AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 0,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: false,
            chroma_subsampling_y: false,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0,
            initial_presentation_delay_minus_one: None,
            config_obu: Bytes::from_static(b"HELLO FROM THE OBU"),
        };

        let mut buf = Vec::new();
        config.mux(&mut buf).unwrap();

        insta::assert_snapshot!(format!("{:?}", Bytes::from(buf)), @r#"b"\x81\0\0\0HELLO FROM THE OBU""#);
    }

    #[test]
    fn test_config_mux_with_delay() {
        let config = AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 0,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: false,
            chroma_subsampling_y: false,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0,
            initial_presentation_delay_minus_one: Some(0),
            config_obu: Bytes::from_static(b"HELLO FROM THE OBU"),
        };

        let mut buf = Vec::new();
        config.mux(&mut buf).unwrap();

        insta::assert_snapshot!(format!("{:?}", Bytes::from(buf)), @r#"b"\x81\0\0\x10HELLO FROM THE OBU""#);
    }

    #[test]
    fn test_config_demux_isobmff_ignores_hdr_wcg_bits() {
        // In ISOBMFF av1C, the three bits after chroma_sample_position are reserved.
        // Even if set, they should not be interpreted as hdr_wcg_idc.
        let data = b"\x81\x00\x00\xc0".to_vec();
        let config = AV1CodecConfigurationRecord::demux(&mut io::Cursor::new(data.into())).unwrap();
        assert_eq!(config.hdr_wcg_idc, 0);
    }

    #[test]
    fn test_config_demux_mpeg2_ts_reads_hdr_wcg_bits() {
        // In MPEG-2 TS descriptor variant, top two bits carry hdr_wcg_idc.
        let data = b"\x81\x00\x00\xc0".to_vec();
        let config =
            AV1CodecConfigurationRecord::demux_mpeg2_ts(&mut io::Cursor::new(data.into())).unwrap();
        assert_eq!(config.hdr_wcg_idc, 0b11);
    }

    #[test]
    fn test_config_mux_isobmff_zeros_reserved_bits() {
        let config = AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 0,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: false,
            chroma_subsampling_y: false,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0b11,
            initial_presentation_delay_minus_one: None,
            config_obu: Bytes::new(),
        };

        let mut buf = Vec::new();
        config.mux(&mut buf).unwrap();
        assert_eq!(buf, b"\x81\x00\x00\x00");
    }

    #[test]
    fn test_config_mux_mpeg2_ts_writes_hdr_wcg_bits() {
        let config = AV1CodecConfigurationRecord {
            seq_profile: 0,
            seq_level_idx_0: 0,
            seq_tier_0: false,
            high_bitdepth: false,
            twelve_bit: false,
            monochrome: false,
            chroma_subsampling_x: false,
            chroma_subsampling_y: false,
            chroma_sample_position: 0,
            hdr_wcg_idc: 0b11,
            initial_presentation_delay_minus_one: None,
            config_obu: Bytes::new(),
        };

        let mut buf = Vec::new();
        config.mux_mpeg2_ts(&mut buf).unwrap();
        assert_eq!(buf, b"\x81\x00\x00\xc0");
    }

    #[test]
    fn test_video_descriptor_demux() {
        let data = b"\x80\x04\x81\r\x0c\x3f\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@"
            .to_vec();

        let config = AV1VideoDescriptor::demux(&mut io::Cursor::new(data.into())).unwrap();

        insta::assert_debug_snapshot!(config, @r#"
        AV1VideoDescriptor {
            tag: 128,
            length: 4,
            codec_configuration_record: AV1CodecConfigurationRecord {
                seq_profile: 0,
                seq_level_idx_0: 13,
                seq_tier_0: false,
                high_bitdepth: false,
                twelve_bit: false,
                monochrome: false,
                chroma_subsampling_x: true,
                chroma_subsampling_y: true,
                chroma_sample_position: 0,
                hdr_wcg_idc: 0,
                initial_presentation_delay_minus_one: Some(
                    15,
                ),
                config_obu: b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@",
            },
        }
        "#);
    }

    #[test]
    fn test_video_descriptor_demux_invalid_tag() {
        let data = b"\x81".to_vec();

        let err = AV1VideoDescriptor::demux(&mut io::Cursor::new(data.into())).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "Invalid AV1 video descriptor tag");
    }

    #[test]
    fn test_video_descriptor_demux_invalid_length() {
        let data = b"\x80\x05ju".to_vec();

        let err = AV1VideoDescriptor::demux(&mut io::Cursor::new(data.into())).unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "Invalid AV1 video descriptor length");
    }
}
