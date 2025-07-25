use std::io;

use bytes_util::{BitReader, BitWriter};
use expgolomb::{BitReaderExpGolombExt, BitWriterExpGolombExt, size_of_exp_golomb};

/// `FrameCropInfo` contains the frame cropping info.
///
/// This includes `frame_crop_left_offset`, `frame_crop_right_offset`, `frame_crop_top_offset`,
/// and `frame_crop_bottom_offset`.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameCropInfo {
    /// The `frame_crop_left_offset` is the the left crop offset which is used to compute the width:
    ///
    /// `width = ((pic_width_in_mbs_minus1 + 1) * 16) - frame_crop_right_offset * 2 - frame_crop_left_offset * 2`
    ///
    /// This is a variable number of bits as it is encoded by an exp golomb (unsigned).
    /// ISO/IEC-14496-10-2022 - 7.4.2.1.1
    ///
    /// For more information:
    ///
    /// <https://en.wikipedia.org/wiki/Exponential-Golomb_coding>
    pub frame_crop_left_offset: u64,

    /// The `frame_crop_right_offset` is the the right crop offset which is used to compute the width:
    ///
    /// `width = ((pic_width_in_mbs_minus1 + 1) * 16) - frame_crop_right_offset * 2 - frame_crop_left_offset * 2`
    ///
    /// This is a variable number of bits as it is encoded by an exp golomb (unsigned).
    /// ISO/IEC-14496-10-2022 - 7.4.2.1.1
    ///
    /// For more information:
    ///
    /// <https://en.wikipedia.org/wiki/Exponential-Golomb_coding>
    pub frame_crop_right_offset: u64,

    /// The `frame_crop_top_offset` is the the top crop offset which is used to compute the height:
    ///
    /// `height = ((2 - frame_mbs_only_flag as u64) * (pic_height_in_map_units_minus1 + 1) * 16)
    /// - frame_crop_bottom_offset * 2 - frame_crop_top_offset * 2`
    ///
    /// This is a variable number of bits as it is encoded by an exp golomb (unsigned).
    /// ISO/IEC-14496-10-2022 - 7.4.2.1.1
    ///
    /// For more information:
    ///
    /// <https://en.wikipedia.org/wiki/Exponential-Golomb_coding>
    pub frame_crop_top_offset: u64,

    /// The `frame_crop_bottom_offset` is the the bottom crop offset which is used to compute the height:
    ///
    /// `height = ((2 - frame_mbs_only_flag as u64) * (pic_height_in_map_units_minus1 + 1) * 16)
    /// - frame_crop_bottom_offset * 2 - frame_crop_top_offset * 2`
    ///
    /// This is a variable number of bits as it is encoded by an exp golomb (unsigned).
    /// ISO/IEC-14496-10-2022 - 7.4.2.1.1
    ///
    /// For more information:
    ///
    /// <https://en.wikipedia.org/wiki/Exponential-Golomb_coding>
    pub frame_crop_bottom_offset: u64,
}

impl FrameCropInfo {
    /// Parses the fields defined when the `frame_cropping_flag == 1` from a bitstream.
    /// Returns a `FrameCropInfo` struct.
    pub fn parse<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<Self> {
        let frame_crop_left_offset = reader.read_exp_golomb()?;
        let frame_crop_right_offset = reader.read_exp_golomb()?;
        let frame_crop_top_offset = reader.read_exp_golomb()?;
        let frame_crop_bottom_offset = reader.read_exp_golomb()?;

        Ok(FrameCropInfo {
            frame_crop_left_offset,
            frame_crop_right_offset,
            frame_crop_top_offset,
            frame_crop_bottom_offset,
        })
    }

    /// Builds the FrameCropInfo struct into a byte stream.
    /// Returns a built byte stream.
    pub fn build<T: io::Write>(&self, writer: &mut BitWriter<T>) -> io::Result<()> {
        writer.write_exp_golomb(self.frame_crop_left_offset)?;
        writer.write_exp_golomb(self.frame_crop_right_offset)?;
        writer.write_exp_golomb(self.frame_crop_top_offset)?;
        writer.write_exp_golomb(self.frame_crop_bottom_offset)?;
        Ok(())
    }

    /// Returns the total bits of the FrameCropInfo struct.
    ///
    /// Note that this isn't the bytesize since aligning it may cause some values to be different.
    pub fn bitsize(&self) -> u64 {
        size_of_exp_golomb(self.frame_crop_left_offset)
            + size_of_exp_golomb(self.frame_crop_right_offset)
            + size_of_exp_golomb(self.frame_crop_top_offset)
            + size_of_exp_golomb(self.frame_crop_bottom_offset)
    }

    /// Returns the total bytes of the FrameCropInfo struct.
    ///
    /// Note that this calls [`FrameCropInfo::bitsize()`] and calculates the number of bytes
    /// including any necessary padding such that the bitstream is byte aligned.
    pub fn bytesize(&self) -> u64 {
        self.bitsize().div_ceil(8)
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use bytes_util::{BitReader, BitWriter};
    use expgolomb::BitWriterExpGolombExt;

    use crate::sps::FrameCropInfo;

    #[test]
    fn test_build_size_frame_crop() {
        // create bitstream for frame_crop
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_exp_golomb(1).unwrap();
        writer.write_exp_golomb(10).unwrap();
        writer.write_exp_golomb(7).unwrap();
        writer.write_exp_golomb(38).unwrap();

        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let frame_crop_info = FrameCropInfo::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        frame_crop_info.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);

        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_frame_crop_info = FrameCropInfo::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(rebuilt_frame_crop_info.bitsize(), frame_crop_info.bitsize());
        assert_eq!(
            rebuilt_frame_crop_info.bytesize(),
            frame_crop_info.bytesize()
        );
    }
}
