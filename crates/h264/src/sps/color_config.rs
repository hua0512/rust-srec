use std::io;

use byteorder::ReadBytesExt;
use bytes_util::{BitReader, BitWriter};

use crate::VideoFormat;

/// The color config for SPS. ISO/IEC-14496-10-2022 - E.2.1
#[derive(Debug, Clone, PartialEq)]
pub struct ColorConfig {
    /// The `video_format` is comprised of 3 bits stored as a u8.
    ///
    /// Refer to the `VideoFormat` nutype enum for more info.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1 Table E-2
    pub video_format: VideoFormat,

    /// The `video_full_range_flag` is a single bit indicating the black level and range of
    /// luma and chroma signals.
    ///
    /// This field is passed into the `ColorConfig`.
    /// ISO/IEC-14496-10-2022 - E.2.1
    pub video_full_range_flag: bool,

    /// The `colour_primaries` byte as a u8. If `color_description_present_flag` is not set,
    /// the value defaults to 2. ISO/IEC-14496-10-2022 - E.2.1 Table E-3
    pub color_primaries: u8,

    /// The `transfer_characteristics` byte as a u8. If `color_description_present_flag` is not set,
    /// the value defaults to 2. ISO/IEC-14496-10-2022 - E.2.1 Table E-4
    pub transfer_characteristics: u8,

    /// The `matrix_coefficients` byte as a u8. If `color_description_present_flag` is not set,
    /// the value defaults to 2. ISO/IEC-14496-10-2022 - E.2.1 Table E-5
    pub matrix_coefficients: u8,
}

impl ColorConfig {
    /// Parses the fields defined when the `video_signal_type_present_flag == 1` from a bitstream.
    /// Returns a `ColorConfig` struct.
    pub fn parse<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<Self> {
        let video_format = reader.read_bits(3)? as u8;
        let video_full_range_flag = reader.read_bit()?;

        let color_primaries;
        let transfer_characteristics;
        let matrix_coefficients;

        let color_description_present_flag = reader.read_bit()?;
        if color_description_present_flag {
            color_primaries = reader.read_u8()?;
            transfer_characteristics = reader.read_u8()?;
            matrix_coefficients = reader.read_u8()?;
        } else {
            color_primaries = 2; // UNSPECIFIED
            transfer_characteristics = 2; // UNSPECIFIED
            matrix_coefficients = 2; // UNSPECIFIED
        }

        Ok(ColorConfig {
            video_format: VideoFormat::try_from(video_format)?, // defalut value is 5 E.2.1 Table E-2
            video_full_range_flag,
            color_primaries,
            transfer_characteristics,
            matrix_coefficients,
        })
    }

    /// Builds the ColorConfig struct into a byte stream.
    /// Returns a built byte stream.
    pub fn build<T: io::Write>(&self, writer: &mut BitWriter<T>) -> io::Result<()> {
        writer.write_bits(self.video_format as u64, 3)?;
        writer.write_bit(self.video_full_range_flag)?;

        match (
            self.color_primaries,
            self.transfer_characteristics,
            self.matrix_coefficients,
        ) {
            (2, 2, 2) => {
                writer.write_bit(false)?;
            }
            (color_priamries, transfer_characteristics, matrix_coefficients) => {
                writer.write_bit(true)?;
                writer.write_bits(color_priamries as u64, 8)?;
                writer.write_bits(transfer_characteristics as u64, 8)?;
                writer.write_bits(matrix_coefficients as u64, 8)?;
            }
        }
        Ok(())
    }

    /// Returns the total bits of the ColorConfig struct.
    ///
    /// Note that this isn't the bytesize since aligning it may cause some values to be different.
    pub fn bitsize(&self) -> u64 {
        3 + // video_format
        1 + // video_full_range_flag
        1 + // color_description_present_flag
        match (self.color_primaries, self.transfer_characteristics, self.matrix_coefficients) {
            (2, 2, 2) => 0,
            _ => 24
        }
    }

    /// Returns the total bytes of the ColorConfig struct.
    ///
    /// Note that this calls [`ColorConfig::bitsize()`] and calculates the number of bytes
    /// including any necessary padding such that the bitstream is byte aligned.
    pub fn bytesize(&self) -> u64 {
        self.bitsize().div_ceil(8)
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use bytes_util::{BitReader, BitWriter};

    use crate::sps::ColorConfig;

    #[test]
    fn test_build_size_color_config() {
        // create bitstream for color_config
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_bits(4, 3).unwrap();
        writer.write_bit(true).unwrap();

        // color_desc_present_flag
        writer.write_bit(true).unwrap();
        writer.write_bits(2, 8).unwrap();
        writer.write_bits(6, 8).unwrap();
        writer.write_bits(1, 8).unwrap();
        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let color_config = ColorConfig::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        color_config.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);
        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_color_config = ColorConfig::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(rebuilt_color_config.bitsize(), color_config.bitsize());
        assert_eq!(rebuilt_color_config.bytesize(), color_config.bytesize());
    }

    #[test]
    fn test_build_size_color_config_no_desc() {
        // create bitstream for color_config
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_bits(4, 3).unwrap();
        writer.write_bit(true).unwrap();

        // color_desc_present_flag
        writer.write_bit(false).unwrap();
        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let color_config = ColorConfig::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        color_config.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);

        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_color_config = ColorConfig::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(rebuilt_color_config.bitsize(), color_config.bitsize());
        assert_eq!(rebuilt_color_config.bytesize(), color_config.bytesize());
    }
}
