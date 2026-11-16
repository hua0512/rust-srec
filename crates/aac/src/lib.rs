//! A crate for decoding AAC audio headers.
//!
//! ## License
//!
//! This project is licensed under the [MIT](https://github.com/hua0512/rust-srec/blob/main/LICENSE-MIT) or
//! [Apache-2.0](https://github.com/hua0512/rust-srec/blob/main/LICENSE-APACHE) license. You can choose between one of
//! them if you use this work.
//!
//! `SPDX-License-Identifier: MIT OR Apache-2.0`
#![cfg_attr(all(coverage_nightly, test), feature(coverage_attribute))]
#![deny(missing_docs)]
#![deny(unsafe_code)]

use std::io;

use bytes_util::BitReader;

/// A Partial Audio Specific Config
/// ISO/IEC 14496-3:2019(E) - 1.6
///
/// This struct does not represent the full AudioSpecificConfig, it only
/// represents the top few fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct PartialAudioSpecificConfig {
    /// Audio Object Type
    pub audio_object_type: AudioObjectType,
    /// Sampling Frequency
    pub sampling_frequency: u32,
    /// Channel Configuration
    pub channel_configuration: u8,
}

/// SBR Audio Object Type
/// ISO/IEC 14496-3:2019(E) - 1.5.1.2.6
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum AudioObjectType {
    /// AAC main
    AacMain,
    /// AAC LC
    AacLowComplexity,
    /// Any other object type
    Unknown(u16),
}

impl AudioObjectType {
    /// Converts an AudioObjectType to a u16
    pub const fn as_u16(&self) -> u16 {
        match self {
            AudioObjectType::AacMain => 1,
            AudioObjectType::AacLowComplexity => 2,
            AudioObjectType::Unknown(value) => *value,
        }
    }

    /// Converts a u16 to an AudioObjectType
    pub const fn from_u16(value: u16) -> Self {
        match value {
            1 => AudioObjectType::AacMain,
            2 => AudioObjectType::AacLowComplexity,
            _ => AudioObjectType::Unknown(value),
        }
    }
}

impl From<u16> for AudioObjectType {
    fn from(value: u16) -> Self {
        Self::from_u16(value)
    }
}

impl From<AudioObjectType> for u16 {
    fn from(value: AudioObjectType) -> Self {
        value.as_u16()
    }
}

/// Sampling Frequency Index
///
/// The purpose of the FrequencyIndex is to encode commonly used frequencies in
/// 4 bits to save space. These are the set of commonly used frequencies defined
/// in the specification.
///
/// ISO/IEC 14496-3:2019(E) - 1.6.2.4 (Table 1.22)
#[derive(Debug, Clone, PartialEq, Copy, Eq, PartialOrd, Ord)]
#[repr(u8)]
#[must_use]
pub enum SampleFrequencyIndex {
    /// 96000 Hz
    Freq96000 = 0x0,
    /// 88200 Hz
    Freq88200 = 0x1,
    /// 64000 Hz
    Freq64000 = 0x2,
    /// 48000 Hz
    Freq48000 = 0x3,
    /// 44100 Hz
    Freq44100 = 0x4,
    /// 32000 Hz
    Freq32000 = 0x5,
    /// 24000 Hz
    Freq24000 = 0x6,
    /// 22050 Hz
    Freq22050 = 0x7,
    /// 16000 Hz
    Freq16000 = 0x8,
    /// 12000 Hz
    Freq12000 = 0x9,
    /// 11025 Hz
    Freq11025 = 0xA,
    /// 8000 Hz
    Freq8000 = 0xB,
    /// 7350 Hz
    Freq7350 = 0xC,
    /// Reserved
    FreqReserved = 0xD,
    /// Reserved
    FreqReserved2 = 0xE,
    /// Escape (Meaning the frequency is not in the table, and we need to read
    /// an additional 24 bits to get the frequency)
    FreqEscape = 0xF,
}

impl SampleFrequencyIndex {
    /// Convert a `u8` to a `SampleFrequencyIndex`, returning `None` for invalid values.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x0 => Some(Self::Freq96000),
            0x1 => Some(Self::Freq88200),
            0x2 => Some(Self::Freq64000),
            0x3 => Some(Self::Freq48000),
            0x4 => Some(Self::Freq44100),
            0x5 => Some(Self::Freq32000),
            0x6 => Some(Self::Freq24000),
            0x7 => Some(Self::Freq22050),
            0x8 => Some(Self::Freq16000),
            0x9 => Some(Self::Freq12000),
            0xA => Some(Self::Freq11025),
            0xB => Some(Self::Freq8000),
            0xC => Some(Self::Freq7350),
            0xD => Some(Self::FreqReserved),
            0xE => Some(Self::FreqReserved2),
            0xF => Some(Self::FreqEscape),
            _ => None,
        }
    }

    /// Convert the SampleFrequencyIndex to the actual frequency in Hz
    pub const fn to_freq(&self) -> Option<u32> {
        match self {
            SampleFrequencyIndex::Freq96000 => Some(96000),
            SampleFrequencyIndex::Freq88200 => Some(88200),
            SampleFrequencyIndex::Freq64000 => Some(64000),
            SampleFrequencyIndex::Freq48000 => Some(48000),
            SampleFrequencyIndex::Freq44100 => Some(44100),
            SampleFrequencyIndex::Freq32000 => Some(32000),
            SampleFrequencyIndex::Freq24000 => Some(24000),
            SampleFrequencyIndex::Freq22050 => Some(22050),
            SampleFrequencyIndex::Freq16000 => Some(16000),
            SampleFrequencyIndex::Freq12000 => Some(12000),
            SampleFrequencyIndex::Freq11025 => Some(11025),
            SampleFrequencyIndex::Freq8000 => Some(8000),
            SampleFrequencyIndex::Freq7350 => Some(7350),
            SampleFrequencyIndex::FreqReserved => None,
            SampleFrequencyIndex::FreqReserved2 => None,
            SampleFrequencyIndex::FreqEscape => None,
        }
    }
}

impl PartialAudioSpecificConfig {
    /// Parse the Audio Specific Config from given bytes
    /// The implementation is based on ISO/IEC 14496-3:2019(E) - 1.6.2.1 (Table
    /// 1.19) This does not parse the entire AAC Data, it only parses the
    /// top few fields.
    /// - Audio Object Type
    /// - Sampling Frequency
    /// - Channel Configuration
    pub fn parse(data: &[u8]) -> io::Result<Self> {
        let mut bitreader = BitReader::new_from_slice(data);

        // GetAudioObjectType() # ISO/IEC 14496-3:2019(E) - 1.6.2.1 (Table 1.20)
        let mut audio_object_type = bitreader.read_bits(5)? as u16;
        if audio_object_type == 31 {
            audio_object_type = 32 + bitreader.read_bits(6)? as u16;
        }

        // The table calls for us to read a 4-bit value. If the value is type FreqEscape
        // (0xF), we need to read 24 bits to get the sampling frequency.
        let sampling_frequency_index = SampleFrequencyIndex::from_u8(bitreader.read_bits(4)? as u8)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid sampling frequency index",
                )
            })?;

        let sampling_frequency = match sampling_frequency_index {
            // Uses the extended sampling frequency to represent the freq as a non-common value
            SampleFrequencyIndex::FreqEscape => bitreader.read_bits(24)? as u32,
            _ => sampling_frequency_index.to_freq().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid sampling frequency index",
                )
            })?,
        };

        // 4 Bits to get the channel configuration
        let channel_configuration = bitreader.read_bits(4)? as u8;

        Ok(Self {
            audio_object_type: audio_object_type.into(),
            sampling_frequency,
            channel_configuration,
        })
    }
}

/// The header of an ADTS frame.
///
/// ISO/IEC 14496-3:2019(E) - 1.A.3.2 (`adts_fixed_header` and
/// `adts_variable_header`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct AdtsHeader {
    /// Audio Object Type: the header's profile plus one.
    pub audio_object_type: AudioObjectType,
    /// Sampling Frequency Index; never an escape or reserved value.
    pub sampling_frequency_index: SampleFrequencyIndex,
    /// Channel Configuration; never 0, which would need a program config
    /// element from the payload.
    pub channel_configuration: u8,
    /// Bytes in the header: 7, or 9 with a CRC.
    pub header_length: usize,
    /// Bytes in the frame, including the header.
    pub frame_length: usize,
    /// Raw data blocks in the frame, each of 1024 samples.
    pub raw_data_blocks: u8,
}

impl AdtsHeader {
    /// Parses the header at the start of an ADTS frame.
    pub fn parse(data: &[u8]) -> io::Result<Self> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
        let mut bitreader = BitReader::new_from_slice(data);

        if bitreader.read_bits(12)? != 0xfff {
            return Err(invalid("ADTS frame without a sync word"));
        }
        // ID, then layer, which is always 0.
        bitreader.read_bits(1)?;
        if bitreader.read_bits(2)? != 0 {
            return Err(invalid("ADTS layer is not 0"));
        }
        let protection_absent = bitreader.read_bit()?;
        let profile = bitreader.read_bits(2)? as u16;
        let sampling_frequency_index = SampleFrequencyIndex::from_u8(bitreader.read_bits(4)? as u8)
            .filter(|index| index.to_freq().is_some())
            .ok_or_else(|| invalid("ADTS sampling frequency index is reserved"))?;
        // Private bit.
        bitreader.read_bits(1)?;
        let channel_configuration = bitreader.read_bits(3)? as u8;
        if channel_configuration == 0 {
            return Err(invalid("ADTS channel configuration 0 is not supported"));
        }
        // Original/copy, home, and the two copyright identification bits.
        bitreader.read_bits(4)?;
        let frame_length = bitreader.read_bits(13)? as usize;
        // Buffer fullness.
        bitreader.read_bits(11)?;
        let raw_data_blocks = bitreader.read_bits(2)? as u8 + 1;

        let header_length = if protection_absent { 7 } else { 9 };
        if frame_length < header_length {
            return Err(invalid("ADTS frame is shorter than its header"));
        }
        Ok(Self {
            audio_object_type: AudioObjectType::from_u16(profile + 1),
            sampling_frequency_index,
            channel_configuration,
            header_length,
            frame_length,
            raw_data_blocks,
        })
    }

    /// The sampling frequency in Hz.
    pub fn sampling_frequency(&self) -> u32 {
        self.sampling_frequency_index.to_freq().unwrap_or_default()
    }

    /// The AudioSpecificConfig that describes this stream, as an MP4 or FLV
    /// sequence header carries it.
    ///
    /// ISO/IEC 14496-3:2019(E) - 1.6.2.1 (Table 1.19), with a
    /// `GASpecificConfig` of 1024-sample frames, no core coder and no
    /// extension.
    pub fn audio_specific_config(&self) -> [u8; 2] {
        let config = (self.audio_object_type.as_u16() << 11)
            | (u16::from(self.sampling_frequency_index as u8) << 7)
            | (u16::from(self.channel_configuration) << 3);
        config.to_be_bytes()
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_aac_config_parse() {
        let data = [
            0x12, 0x10, 0x56, 0xe5, 0x00, 0x2d, 0x96, 0x01, 0x80, 0x80, 0x05, 0x00, 0x00, 0x00,
            0x00,
        ];

        let config = PartialAudioSpecificConfig::parse(&data).unwrap();
        assert_eq!(config.audio_object_type, AudioObjectType::AacLowComplexity);
        assert_eq!(config.sampling_frequency, 44100);
        assert_eq!(config.channel_configuration, 2);
    }

    #[test]
    fn adts_headers_give_frame_layout_and_audio_specific_config() {
        // A live AAC-LC 22.05 kHz mono frame; FFmpeg's 44.1 kHz stereo
        // encoder output with the AudioSpecificConfig FFmpeg writes for it;
        // the same header with a CRC and two raw data blocks.
        let cases = [
            (*b"\xff\xf1\x5c\x40\x45\xa0\xd0", (7, 557, 1), [0x13, 0x88]),
            (*b"\xff\xf1\x50\x80\x2a\xff\xfc", (7, 343, 1), [0x12, 0x10]),
            (*b"\xff\xf0\x50\x80\x2a\xff\xfd", (9, 343, 2), [0x12, 0x10]),
        ];
        for (header, layout, config) in cases {
            let adts = AdtsHeader::parse(&header).unwrap();
            assert_eq!(
                (adts.header_length, adts.frame_length, adts.raw_data_blocks),
                layout
            );
            assert_eq!(adts.audio_specific_config(), config);
        }

        let mono = AdtsHeader::parse(b"\xff\xf1\x5c\x40\x45\xa0\xd0").unwrap();
        assert_eq!(
            PartialAudioSpecificConfig::parse(&mono.audio_specific_config()).unwrap(),
            PartialAudioSpecificConfig {
                audio_object_type: AudioObjectType::AacLowComplexity,
                sampling_frequency: 22050,
                channel_configuration: 1,
            }
        );
    }

    #[test]
    fn test_idx_to_freq() {
        let cases = [
            (SampleFrequencyIndex::FreqEscape, None),
            (SampleFrequencyIndex::FreqReserved2, None),
            (SampleFrequencyIndex::FreqReserved, None),
            (SampleFrequencyIndex::Freq7350, Some(7350)),
            (SampleFrequencyIndex::Freq8000, Some(8000)),
            (SampleFrequencyIndex::Freq11025, Some(11025)),
            (SampleFrequencyIndex::Freq12000, Some(12000)),
            (SampleFrequencyIndex::Freq16000, Some(16000)),
            (SampleFrequencyIndex::Freq22050, Some(22050)),
            (SampleFrequencyIndex::Freq24000, Some(24000)),
            (SampleFrequencyIndex::Freq32000, Some(32000)),
            (SampleFrequencyIndex::Freq44100, Some(44100)),
            (SampleFrequencyIndex::Freq48000, Some(48000)),
            (SampleFrequencyIndex::Freq64000, Some(64000)),
            (SampleFrequencyIndex::Freq88200, Some(88200)),
            (SampleFrequencyIndex::Freq96000, Some(96000)),
        ];

        for (idx, freq) in cases {
            assert_eq!(freq, idx.to_freq(), "Expected frequency for {idx:?}");
        }
    }
}
