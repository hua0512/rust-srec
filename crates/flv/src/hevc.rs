use std::io;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;
use bytes_util::BytesCursorExt;
use h265::{HEVCDecoderConfigurationRecord, NaluType};

use crate::resolution::Resolution;

#[repr(u8)]
#[derive(Debug, Clone, PartialEq)]
pub enum HevcPacketType {
    /// HEVC Sequence Header
    SeqHdr = 0,
    /// HEVC NALU
    Nalu = 1,
    /// HEVC End of Sequence
    EndOfSequence = 2,

    Unknown(u8),
}

impl TryFrom<u8> for HevcPacketType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::SeqHdr),
            1 => Ok(Self::Nalu),
            2 => Ok(Self::EndOfSequence),
            _ => Ok(Self::Unknown(value)),
        }
    }
}

/// HEVC Packet
#[derive(Debug, Clone, PartialEq)]
pub enum HevcPacket {
    /// HEVC Sequence Start
    SequenceStart(HEVCDecoderConfigurationRecord),
    /// HEVC NALU
    Nalu {
        composition_time: Option<i32>,
        data: Bytes,
    },

    /// HEVC End of Sequence
    /// End of Sequence
    EndOfSequence,
    /// HEVC Unknown (we don't know how to parse it)
    /// Unknown
    Unknown {
        hevc_packet_type: HevcPacketType,
        composition_time: Option<i32>,
        data: Bytes,
    },
}

impl HevcPacket {
    /// Demux HEVC packet
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        let hevc_packet_type = HevcPacketType::try_from(reader.read_u8()?)?;
        // In legacy hevc, this composition is always parsed regardless of the packet type
        let composition_time = Some(reader.read_i24::<BigEndian>()?);
        match hevc_packet_type {
            HevcPacketType::SeqHdr => Ok(Self::SequenceStart(
                HEVCDecoderConfigurationRecord::demux(reader)?,
            )),
            HevcPacketType::Nalu => Ok(Self::Nalu {
                composition_time,
                data: reader.extract_remaining(),
            }),
            HevcPacketType::EndOfSequence => Ok(Self::EndOfSequence),
            _ => Ok(Self::Unknown {
                hevc_packet_type,
                composition_time,
                data: reader.extract_remaining(),
            }),
        }
    }

    pub fn get_video_resolution(&self) -> Option<Resolution> {
        match self {
            HevcPacket::SequenceStart(config) => {
                // Find first SPS NAL unit
                config
                    .arrays
                    .iter()
                    .find(|array| array.nal_unit_type == NaluType::Sps)
                    .and_then(|sps_array| sps_array.nalus.first())
                    .and_then(|sps| {
                        if sps.len() < 4 {
                            return None;
                        }
                        h265::Sps::parse(sps.clone()).ok().map(|sps| Resolution {
                            width: sps.width as f32,
                            height: sps.height as f32,
                        })
                    })
            }
            _ => None,
        }
    }
}

impl std::fmt::Display for HevcPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HevcPacket::SequenceStart(config) => write!(
                f,
                "SequenceStart [Profile: {}, Level: {}, Tier: {}]",
                config.general_profile_idc, config.general_level_idc, config.general_tier_flag
            ),
            HevcPacket::Nalu {
                composition_time,
                data,
            } => {
                if let Some(time) = composition_time {
                    write!(f, "NALU [CTS: {}ms] ({} bytes)", time, data.len())
                } else {
                    write!(f, "NALU ({} bytes)", data.len())
                }
            }
            HevcPacket::EndOfSequence => write!(f, "EndOfSequence"),
            HevcPacket::Unknown {
                hevc_packet_type,
                composition_time,
                data,
            } => {
                if let Some(time) = composition_time {
                    write!(
                        f,
                        "Unknown [Type: {:?}, CTS: {}ms] ({} bytes)",
                        hevc_packet_type,
                        time,
                        data.len()
                    )
                } else {
                    write!(
                        f,
                        "Unknown [Type: {:?}] ({} bytes)",
                        hevc_packet_type,
                        data.len()
                    )
                }
            }
        }
    }
}
