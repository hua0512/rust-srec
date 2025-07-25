use std::io;
use std::num::NonZeroU32;

use byteorder::{BigEndian, ReadBytesExt};
use bytes_util::{BitReader, BitWriter};

/// `TimingInfo` contains the fields that are set when `timing_info_present_flag == 1`.
///
/// This contains the following fields: `num_units_in_tick` and `time_scale`.
///
/// ISO/IEC-14496-10-2022 - E.2.1
///
/// Refer to the direct fields for more information.
#[derive(Debug, Clone, PartialEq)]
pub struct TimingInfo {
    /// The `num_units_in_tick` is the smallest unit used to measure time.
    ///
    /// It is used alongside `time_scale` to compute the `frame_rate` as follows:
    ///
    /// `frame_rate = time_scale / (2 * num_units_in_tick)`
    ///
    /// It must be greater than 0, therefore, it is a `NonZeroU32`.
    /// If it is 0, it will fail to parse.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1
    pub num_units_in_tick: NonZeroU32,

    /// The `time_scale` is the number of time units that pass in 1 second (hz).
    ///
    /// It is used alongside `num_units_in_tick` to compute the `frame_rate` as follows:
    ///
    /// `frame_rate = time_scale / (2 * num_units_in_tick)`
    ///
    /// It must be greater than 0, therefore, it is a `NonZeroU32`.
    /// If it is 0, it will fail to parse.
    ///
    /// ISO/IEC-14496-10-2022 - E.2.1
    pub time_scale: NonZeroU32,
}

impl TimingInfo {
    /// Parses the fields defined when the `timing_info_present_flag == 1` from a bitstream.
    /// Returns a `TimingInfo` struct.
    pub fn parse<T: io::Read>(reader: &mut BitReader<T>) -> io::Result<Self> {
        let num_units_in_tick =
            NonZeroU32::new(reader.read_u32::<BigEndian>()?).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "num_units_in_tick cannot be 0")
            })?;

        let time_scale = NonZeroU32::new(reader.read_u32::<BigEndian>()?)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "time_scale cannot be 0"))?;

        Ok(TimingInfo {
            num_units_in_tick,
            time_scale,
        })
    }

    /// Builds the TimingInfo struct into a byte stream.
    /// Returns a built byte stream.
    pub fn build<T: io::Write>(&self, writer: &mut BitWriter<T>) -> io::Result<()> {
        writer.write_bits(self.num_units_in_tick.get() as u64, 32)?;
        writer.write_bits(self.time_scale.get() as u64, 32)?;
        Ok(())
    }

    /// Returns the total bits of the TimingInfo struct. It is always 64 bits (8 bytes).
    pub fn bitsize(&self) -> u64 {
        64
    }

    /// Returns the total bytes of the TimingInfo struct. It is always 8 bytes (64 bits).
    pub fn bytesize(&self) -> u64 {
        8
    }

    /// Returns the frame rate of the TimingInfo struct.
    pub fn frame_rate(&self) -> f64 {
        self.time_scale.get() as f64 / (2.0 * self.num_units_in_tick.get() as f64)
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use bytes_util::{BitReader, BitWriter};

    use crate::sps::TimingInfo;

    #[test]
    fn test_build_size_timing_info() {
        // create bitstream for timing_info
        let mut data = Vec::new();
        let mut writer = BitWriter::new(&mut data);

        writer.write_bits(1234, 32).unwrap();
        writer.write_bits(321, 32).unwrap();
        writer.finish().unwrap();

        // parse bitstream
        let mut reader = BitReader::new_from_slice(&mut data);
        let timing_info = TimingInfo::parse(&mut reader).unwrap();

        // create a writer for the builder
        let mut buf = Vec::new();
        let mut writer2 = BitWriter::new(&mut buf);

        // build from the example result
        timing_info.build(&mut writer2).unwrap();
        writer2.finish().unwrap();

        assert_eq!(buf, data);

        // now we re-parse so we can compare the bit sizes.
        // create a reader for the parser
        let mut reader2 = BitReader::new_from_slice(buf);
        let rebuilt_timing_info = TimingInfo::parse(&mut reader2).unwrap();

        // now we can check the size:
        assert_eq!(rebuilt_timing_info.bitsize(), timing_info.bitsize());
        assert_eq!(rebuilt_timing_info.bytesize(), timing_info.bytesize());
    }
}
