use std::io;

use byteorder::ReadBytesExt;
use bytes_util::{BitReader, BitWriter};

use crate::AspectRatioIdc;

/// `SarDimensions` contains the fields that are set when `aspect_ratio_info_present_flag == 1`,
/// and `aspect_ratio_idc == 255`.
///
/// This contains the following fields: `sar_width` and `sar_height`.
#[derive(Debug, Clone, PartialEq)]
pub struct SarDimensions {
    /// The `aspect_ratio_idc` is the sample aspect ratio of the luma samples as a u8.
    ///
    /// This is a full byte, and defaults to 0.
    ///
    /// Refer to the `AspectRatioIdc` nutype enum for more info.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1 Table E-1
    pub aspect_ratio_idc: AspectRatioIdc,

    /// The `sar_width` is the horizontal size of the aspect ratio as a u16.
    ///
    /// This is a full 2 bytes.
    ///
    /// The value is supposed to be "relatively prime or equal to 0". If set to 0,
    /// the sample aspect ratio is considered to be unspecified by ISO/IEC-14496-10-2022.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1
    pub sar_width: u16,

    /// The `sar_height` is the vertical size of the aspect ratio as a u16.
    ///
    /// This is a full 2 bytes.
    ///
    /// The value is supposed to be "relatively prime or equal to 0". If set to 0,
    /// the sample aspect ratio is considered to be unspecified by ISO/IEC-14496-10-2022.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1
    pub sar_height: u16,
}

impl SarDimensions {
    /// Parses the fields defined when the `aspect_ratio_info_present_flag == 1` from a bitstream.
    /// Returns a `SarDimensions` struct.
    pub fn parse<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<Self> {
        let mut sar_width = 0; // defaults to 0, E.2.1
        let mut sar_height = 0; // defaults to 0, E.2.1

        let aspect_ratio_idc = AspectRatioIdc::from_u8(reader.read_u8()?);
        if aspect_ratio_idc == AspectRatioIdc::ExtendedSar {
            sar_width = reader.read_bits(16)? as u16;
            sar_height = reader.read_bits(16)? as u16;
        }

        Ok(SarDimensions {
            aspect_ratio_idc,
            sar_width,
            sar_height,
        })
    }

    /// Builds the SarDimensions struct into a byte stream.
    /// Returns a built byte stream.
    pub fn build<T: io::Write>(&self, writer: &mut BitWriter<T>) -> io::Result<()> {
        writer.write_bits(self.aspect_ratio_idc as u64, 8)?;

        if self.aspect_ratio_idc == AspectRatioIdc::ExtendedSar {
            writer.write_bits(self.sar_width as u64, 16)?;
            writer.write_bits(self.sar_height as u64, 16)?;
        }
        Ok(())
    }

    /// Returns the total bits of the SarDimensions struct.
    pub fn bitsize(&self) -> u64 {
        8 + u64::from(self.aspect_ratio_idc == AspectRatioIdc::ExtendedSar) * 32
    }

    /// Returns the total bytes of the SarDimensions struct.
    ///
    /// Note that this calls [`SarDimensions::bitsize()`] and calculates the number of bytes.
    pub fn bytesize(&self) -> u64 {
        self.bitsize().div_ceil(8)
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use bytes_util::{BitReader, BitWriter};

    use crate::sps::SarDimensions;

    #[test]
    fn test_build_size_sar_idc_not_255() {
        // create bitstream for sample_aspect_ratio
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_bits(1, 8).unwrap();
        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let sample_aspect_ratio = SarDimensions::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        sample_aspect_ratio.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);

        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_sample_aspect_ratio = SarDimensions::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(
            rebuilt_sample_aspect_ratio.bitsize(),
            sample_aspect_ratio.bitsize()
        );
        assert_eq!(
            rebuilt_sample_aspect_ratio.bytesize(),
            sample_aspect_ratio.bytesize()
        );
    }

    #[test]
    fn test_build_size_sar_idc_255() {
        // create bitstream for sample_aspect_ratio
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_bits(255, 8).unwrap();
        writer.write_bits(11, 16).unwrap();
        writer.write_bits(32, 16).unwrap();
        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let sample_aspect_ratio = SarDimensions::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        sample_aspect_ratio.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);

        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_sample_aspect_ratio = SarDimensions::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(
            rebuilt_sample_aspect_ratio.bitsize(),
            sample_aspect_ratio.bitsize()
        );
        assert_eq!(
            rebuilt_sample_aspect_ratio.bytesize(),
            sample_aspect_ratio.bytesize()
        );
    }
}
