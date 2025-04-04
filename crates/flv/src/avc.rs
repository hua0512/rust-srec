use std::io;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;
use bytes_util::BytesCursorExt;
use h264::AVCDecoderConfigurationRecord;

use crate::resolution::Resolution;

/// AVC Packet
#[derive(Debug, Clone, PartialEq)]
pub enum AvcPacket {
    /// AVC NALU
    Nalu { composition_time: u32, data: Bytes },
    /// AVC Sequence Header
    SequenceHeader(AVCDecoderConfigurationRecord),
    /// AVC End of Sequence
    EndOfSequence,
    /// AVC Unknown (we don't know how to parse it)
    Unknown {
        avc_packet_type: AvcPacketType,
        composition_time: u32,
        data: Bytes,
    },
}

impl AvcPacket {
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        let avc_packet_type = AvcPacketType::try_from(reader.read_u8()?)?;
        let composition_time = reader.read_u24::<BigEndian>()?;

        match avc_packet_type {
            AvcPacketType::SeqHdr => Ok(Self::SequenceHeader(
                AVCDecoderConfigurationRecord::parse(reader)?,
            )),
            AvcPacketType::Nalu => Ok(Self::Nalu {
                composition_time,
                data: reader.extract_remaining(),
            }),
            AvcPacketType::EndOfSequence => Ok(Self::EndOfSequence),
            _ => Ok(Self::Unknown {
                avc_packet_type,
                composition_time,
                data: reader.extract_remaining(),
            }),
        }
    }

    pub fn get_video_resolution(&self) -> Option<Resolution> {
        match self {
            AvcPacket::SequenceHeader(config) => {
                if (config.sps.is_empty()) || config.pps.is_empty() {
                    return None;
                }
                // Parse the first SPS to get the resolution
                let sps = &config.sps[0];

                match h264::Sps::parse_with_emulation_prevention(std::io::Cursor::new(&sps)) {
                    Ok(sps) => {
                        let width = sps.width();
                        let height = sps.height();
                        Some(Resolution {
                            width: width as f32,
                            height: height as f32,
                        })
                    }
                    Err(_) => None,
                }
            }
            // We are not able to parse other packets to get the resolution
            _ => None,
        }
    }
}

impl std::fmt::Display for AvcPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AvcPacket::SequenceHeader(config) => write!(
                f,
                "SequenceHeader [Profile: {}, Level: {}, Length: {}]",
                config.profile_indication, config.profile_compatibility, config.level_indication
            ),
            AvcPacket::Nalu {
                composition_time,
                data,
            } => {
                write!(
                    f,
                    "NALU [CTS: {}ms] ({} bytes)",
                    composition_time,
                    data.len()
                )
            }
            AvcPacket::EndOfSequence => write!(f, "EndOfSequence"),
            AvcPacket::Unknown {
                avc_packet_type,
                composition_time,
                data,
            } => write!(
                f,
                "Unknown [Type: {:?}, CTS: {}ms] ({} bytes)",
                avc_packet_type,
                composition_time,
                data.len()
            ),
        }
    }
}

/// FLV AVC Packet Type
/// Defined in the FLV specification. Chapter 1 - AVCVIDEODATA
/// The AVC packet type is used to determine if the video data is a sequence
/// header or a NALU.
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AvcPacketType {
    SeqHdr = 0,
    Nalu = 1,
    EndOfSequence = 2,
    Unknown = 255,
}

impl TryFrom<u8> for AvcPacketType {
    type Error = io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::SeqHdr),
            1 => Ok(Self::Nalu),
            2 => Ok(Self::EndOfSequence),
            _ => Ok(Self::Unknown),
        }
    }
}
