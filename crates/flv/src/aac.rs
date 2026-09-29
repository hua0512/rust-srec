use std::{fmt, io};

use ::aac::PartialAudioSpecificConfig;
use bytes::Bytes;

#[derive(Debug, Clone, PartialEq)]
pub enum AacPacketType {
    /// AAC Sequence Header
    SequenceHeader = 0x00,
    /// AAC Raw
    Raw = 0x01,
}

impl TryFrom<u8> for AacPacketType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(AacPacketType::SequenceHeader),
            0x01 => Ok(AacPacketType::Raw),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid AAC packet type: {value}"),
            )),
        }
    }
}

impl AacPacketType {
    /// Create a new AacPacketType from the given value
    pub fn new(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(AacPacketType::SequenceHeader),
            0x01 => Some(AacPacketType::Raw),
            _ => None,
        }
    }
}

/// AAC Packet
/// This is a container for aac data.
/// This enum contains the data for the different types of aac packets.
/// Defined in the FLV specification. Chapter 1 - AACAUDIODATA
#[derive(Debug, Clone, PartialEq)]
pub enum AacPacket {
    /// AAC Sequence Header
    SequenceHeader(Bytes),
    /// AAC Raw
    Raw(Bytes),
    /// Data we don't know how to parse
    Unknown {
        aac_packet_type: AacPacketType,
        data: Bytes,
    },
}

impl AacPacket {
    /// Create a new AAC packet from the given data and packet type
    pub fn new(aac_packet_type: AacPacketType, data: Bytes) -> Self {
        match aac_packet_type {
            AacPacketType::Raw => AacPacket::Raw(data),
            AacPacketType::SequenceHeader => AacPacket::SequenceHeader(data),
        }
    }

    pub fn is_sequence_header(&self) -> bool {
        matches!(self, AacPacket::SequenceHeader(_))
    }

    pub(crate) fn is_stereo(&self) -> bool {
        self.config()
            .is_some_and(|config| config.channel_configuration == 2)
    }

    pub(crate) fn sample_rate(&self) -> f32 {
        self.config()
            .map_or(0.0, |config| config.sampling_frequency as f32)
    }

    pub(crate) fn sample_size(&self) -> u32 {
        // FLV's AAC sample-size convention is 16 bits; AudioSpecificConfig
        // does not contain a PCM sample-size field (FLV Annex E.4.2.1).
        16
    }

    fn config(&self) -> Option<PartialAudioSpecificConfig> {
        // FLV carries AudioSpecificConfig, not an ADTS frame header.
        match self {
            AacPacket::SequenceHeader(data) => PartialAudioSpecificConfig::parse(data).ok(),
            _ => None,
        }
    }
}

impl fmt::Display for AacPacket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AacPacket::SequenceHeader(data) => {
                write!(f, "AAC Sequence Header [{} bytes]", data.len())
            }
            AacPacket::Raw(data) => {
                write!(f, "AAC Raw Data [{} bytes]", data.len())
            }
            AacPacket::Unknown {
                aac_packet_type,
                data,
            } => {
                write!(
                    f,
                    "Unknown AAC Packet [Type: {}, {} bytes]",
                    aac_packet_type,
                    data.len()
                )
            }
        }
    }
}

impl fmt::Display for AacPacketType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AacPacketType::SequenceHeader => write!(f, "Sequence Header"),
            AacPacketType::Raw => write!(f, "Raw"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;

    fn decode_sequence_header(config: &[u8]) -> crate::audio::AudioData {
        let mut payload = vec![0xaf, 0];
        payload.extend_from_slice(config);
        crate::FlvTag::new(0, 0, crate::FlvTagType::Audio, false, Bytes::from(payload))
            .decode_audio()
            .unwrap()
    }

    #[test]
    fn audio_specific_config_controls_rate_and_channels() {
        for (data, rate, stereo) in [
            (&[0x12, 0x10][..], 44100.0, true),
            (&[0x11, 0x90][..], 48000.0, true),
            (&[0x11, 0x88][..], 48000.0, false),
            // AAC LC, explicit 24-bit sampling frequency of 48000 Hz, stereo.
            (&[0x17, 0x80, 0x5d, 0xc0, 0x10][..], 48000.0, true),
        ] {
            let audio = decode_sequence_header(data);
            assert!(audio.body.is_sequence_header());
            assert_eq!(audio.body.sample_rate(), rate, "config {data:02x?}");
            assert_eq!(audio.body.is_stereo(), stereo, "config {data:02x?}");
            assert_eq!(audio.body.sample_size(), 16);
        }
    }

    #[test]
    fn truncated_config_accessors_do_not_panic_or_invent_a_rate() {
        for data in [&[0xff, 0xf1][..], &[][..], &[0x12][..], &[0x17, 0x80][..]] {
            let audio = decode_sequence_header(data);
            assert_eq!(audio.body.sample_size(), 16, "config {data:02x?}");
            assert!(!audio.body.is_stereo(), "config {data:02x?}");
            assert_eq!(audio.body.sample_rate(), 0.0, "config {data:02x?}");
        }
    }

    #[test]
    fn test_aac_packet_type() {
        // Only 0x00 and 0x01 are defined by the FLV spec; anything else must
        // be rejected rather than silently mapped onto a valid variant.
        assert_eq!(
            AacPacketType::new(0x00).unwrap(),
            AacPacketType::SequenceHeader
        );
        assert_eq!(AacPacketType::new(0x01).unwrap(), AacPacketType::Raw);
        assert_eq!(AacPacketType::new(0x02), None);
        assert_eq!(AacPacketType::new(0x03), None);
    }
}
