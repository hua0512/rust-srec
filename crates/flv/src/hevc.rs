use std::io;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;
use bytes_util::BytesCursorExt;
use h265::{HEVCDecoderConfigurationRecord, NALUnitType};

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
                    .find(|array| array.nal_unit_type == NALUnitType::SpsNut)
                    .and_then(|sps_array| sps_array.nalus.first())
                    .and_then(|sps| {
                        if sps.len() < 4 {
                            return None;
                        }
                        h265::SpsNALUnit::parse(std::io::Cursor::new(sps.clone()))
                            .ok()
                            .map(|sps_nalu| Resolution {
                                width: sps_nalu.rbsp.cropped_width() as f32,
                                height: sps_nalu.rbsp.cropped_height() as f32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_video_resolution_uses_cropped_sps_dimensions() {
        // Real OBS 1080p HEVC SPS coded as 1920x1088 with conformance bottom crop = 4 (SubHeightC = 2 -> 1080).
        let sps_nalu = Bytes::from_static(
            b"\x42\x01\x01\x01\x40\x00\x00\x03\x00\x90\x00\x00\x03\x00\x00\x03\x00\x78\xa0\x03\xc0\x80\x11\x07\xcb\x96\xb4\xa4\x25\x92\xe3\x01\x6a\x02\x02\x02\x08\x00\x00\x03\x00\x08\x00\x00\x03\x00\xf3\x00\x2e\xf2\x88\x00\x02\x62\x5a\x00\x00\x13\x12\xd0\x20",
        );
        let packet = HevcPacket::SequenceStart(HEVCDecoderConfigurationRecord {
            general_profile_space: 0,
            general_tier_flag: false,
            general_profile_idc: 1,
            general_profile_compatibility_flags: h265::ProfileCompatibilityFlags::empty(),
            general_constraint_indicator_flags: 0,
            general_level_idc: 120,
            min_spatial_segmentation_idc: 0,
            parallelism_type: h265::ParallelismType::MixedOrUnknown,
            chroma_format_idc: 1,
            bit_depth_luma_minus8: 0,
            bit_depth_chroma_minus8: 0,
            avg_frame_rate: 0,
            constant_frame_rate: h265::ConstantFrameRate::Unknown,
            num_temporal_layers: h265::NumTemporalLayers::NotScalable,
            temporal_id_nested: true,
            length_size_minus_one: 3,
            arrays: vec![h265::NaluArray {
                array_completeness: true,
                nal_unit_type: NALUnitType::SpsNut,
                nalus: vec![sps_nalu],
            }],
        });

        assert_eq!(
            packet.get_video_resolution(),
            Some(Resolution {
                width: 1920.0,
                height: 1080.0,
            })
        );
    }
}
