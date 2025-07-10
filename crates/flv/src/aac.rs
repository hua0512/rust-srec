// This

use std::io;

use bytes::Bytes;
use std::fmt;

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
        match self {
            AacPacket::SequenceHeader(data) => {
                // Check if the first byte is 0xFF and the second byte is 0xF1
                if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xF1 {
                    // Check the channel configuration in the 4th byte
                    let channel_config = (data[3] >> 3) & 0x0F;
                    channel_config == 2 // Stereo
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    pub(crate) fn sample_rate(&self) -> f32 {
        match self {
            AacPacket::SequenceHeader(data) => {
                // Check if the first byte is 0xFF and the second byte is 0xF1
                if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xF1 {
                    // Check the sample rate index in the 2nd byte
                    let sample_rate_index = (data[2] >> 2) & 0x03;
                    match sample_rate_index {
                        0 => 96000.0,
                        1 => 88200.0,
                        2 => 64000.0,
                        3 => 48000.0,
                        _ => 44100.0, // Default to 44100 Hz
                    }
                } else {
                    44100.0 // Default to 44100 Hz
                }
            }
            _ => 44100.0, // Default to 44100 Hz
        }
    }

    pub(crate) fn sample_size(&self) -> u32 {
        match self {
            AacPacket::SequenceHeader(data) => {
                // Check if the first byte is 0xFF and the second byte is 0xF1
                if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xF1 {
                    // Check the sample size in the 3rd byte
                    let sample_size = (data[2] >> 4) & 0x0F;
                    sample_size as u32
                } else {
                    16 // Default to 16 bits
                }
            }
            _ => 16, // Default to 16 bits
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

    #[test]
    fn test_new() {
        // Test AAC Sequence Header packet
        let seq_header_data = Bytes::from(vec![0, 1, 2, 3]);
        let seq_header_packet =
            AacPacket::new(AacPacketType::SequenceHeader, seq_header_data.clone());
        assert_eq!(
            seq_header_packet,
            AacPacket::SequenceHeader(seq_header_data)
        );

        // Test AAC Raw packet
        let raw_data = Bytes::from(vec![4, 5, 6, 7]);
        let raw_packet = AacPacket::new(AacPacketType::Raw, raw_data.clone());
        assert_eq!(raw_packet, AacPacket::Raw(raw_data));

        // Test that the AacPacket::new method properly handles the different packet types
        assert!(matches!(
            AacPacket::new(AacPacketType::SequenceHeader, Bytes::new()),
            AacPacket::SequenceHeader(_)
        ));
        assert!(matches!(
            AacPacket::new(AacPacketType::Raw, Bytes::new()),
            AacPacket::Raw(_)
        ));
    }

    #[test]
    fn test_aac_packet_type() {
        assert_eq!(
            format!("{:?}", AacPacketType::SequenceHeader),
            "SequenceHeader"
        );
        assert_eq!(format!("{:?}", AacPacketType::Raw), "Raw");
        let packet_type_2 = AacPacketType::new(0x2).unwrap_or(AacPacketType::Raw);
        let packet_type_3 = AacPacketType::new(0x3).unwrap_or(AacPacketType::Raw);
        assert_eq!(format!("{packet_type_2:?}"), "Raw");
        assert_eq!(format!("{packet_type_3:?}"), "Raw");

        assert_eq!(AacPacketType::new(0x01).unwrap(), AacPacketType::Raw);
        assert_eq!(
            AacPacketType::new(0x00).unwrap(),
            AacPacketType::SequenceHeader
        );
    }
}
