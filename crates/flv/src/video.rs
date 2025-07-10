//! # FLV Video Module
//!
//! Implementation of FLV video tag data parsing following the E-RTMP v2 specification.
//!
//! This module handles parsing of video data from FLV (Flash Video) files, with support
//! for both legacy and enhanced video formats including modern codecs like HEVC, AV1, etc.
//!
//! ## Video Formats
//!
//! The module supports various video formats including:
//! - AVC/H.264 (legacy and enhanced)
//! - HEVC/H.265 (legacy and enhanced)
//! - AV1
//!
//!  Below codecs are not supported:
//! - VP8 and VP9
//! - Sorenson H.263
//! - Screen Video
//! - On2 VP6
//!
//! ## Enhanced Features (E-RTMP v2)
//!
//! - FourCC-based codec identification for modern formats
//! - Extended packet types for advanced features
//! - Multi-track video support
//! - Command frames for client-side seeking
//! - Metadata packets
//!
//! ## Specifications
//!
//! - [E-RTMP v2 specification](https://github.com/veovera/enhanced-rtmp/blob/main/docs/enhanced/enhanced-rtmp-v2.md#enhanced-video)
//! - [Flash Video File Format Specification v10](https://www.adobe.com/content/dam/acom/en/devnet/flv/video_file_format_spec_v10.pdf)
//! - [Flash Video File Format Specification v10.1](https://www.adobe.com/content/dam/acom/en/devnet/flv/video_file_format_spec_v10_1.pdf)
//!
//! ## Usage
//!
//! ```no_run
//! use flv::video::{VideoData, VideoCodecId, VideoFrameType};
//! use bytes::Bytes;
//! use std::io::Cursor;
//!
//! // Parse video data from an FLV tag body
//! let data = vec![/* FLV video tag data */];
//! let mut cursor = Cursor::new(Bytes::from(data));
//! let video = VideoData::demux(&mut cursor).unwrap();
//!
//! // Check frame type and codec
//! match video.frame_type {
//!     VideoFrameType::KeyFrame => println!("Found key frame"),
//!     VideoFrameType::InterFrame => println!("Found inter frame"),
//!     _ => println!("Found other frame type"),
//! }
//! ```
//!
//! ## Credits
//!
//! Based on specifications from Adobe and the E-RTMP project.
//!
//! Based on the work of [ScuffleCloud project](https://github.com/ScuffleCloud/scuffle/blob/main/crates/flv/src/video.rs)
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - ScuffleCloud project contributors
//! - hua0512

use std::io::{self, Read};

use byteorder::{BigEndian, ReadBytesExt};
use bytes::Bytes;

use av1::{AV1CodecConfigurationRecord, AV1VideoDescriptor};
use bytes_util::BytesCursorExt;
use h265::HEVCDecoderConfigurationRecord;
use tracing::debug;

use super::av1::Av1Packet;
use super::hevc::HevcPacket;
use crate::avc::AvcPacket;
use crate::resolution::Resolution;

/// Represents the type of video frame in an FLV video tag
#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum VideoFrameType {
    /// Key frame (for AVC, a seekable frame)
    KeyFrame = 1,

    /// Inter frame, for AVC, a non-key seekable frame
    InterFrame = 2,

    /// Disposable inter frame, H.263 only
    DisposableInterFrame = 3,

    /// Generated key frame, reserved for server use only
    GeneratedKeyFrame = 4,

    /// Video info/command frame
    ///
    /// If videoFrameType is not ignored and is set to VideoFrameType::VideoInfoFrame,
    /// the payload will not contain video data. Instead, (Ex)VideoTagHeader
    /// will be followed by a UI8, representing the following meanings:
    ///
    /// * 0 = Start of client-side seeking video frame sequence
    /// * 1 = End of client-side seeking video frame sequence
    ///
    /// frameType is ignored if videoPacketType is VideoPacketType::MetaData
    VideoInfoFrame = 5,
    // Reserved for future use
    // Reserved = 6,

    // Reserved for future use
    // Reserved2 = 7,
}

impl TryFrom<u8> for VideoFrameType {
    type Error = std::io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Some(Self::KeyFrame),
            2 => Some(Self::InterFrame),
            3 => Some(Self::DisposableInterFrame),
            4 => Some(Self::GeneratedKeyFrame),
            5 => Some(Self::VideoInfoFrame),
            // 6 => Some(Self::Reserved),
            // 7 => Some(Self::Reserved2),
            _ => None,
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid video frame type: {value}"),
            )
        })
    }
}

/// New in E-RTMP v2
#[repr(u8)]
#[derive(Debug, Clone, PartialEq)]
pub enum VideoCommand {
    StartSeek = 1,
    EndSeek = 2,
}

impl TryFrom<u8> for VideoCommand {
    type Error = std::io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Some(Self::StartSeek),
            2 => Some(Self::EndSeek),
            _ => None,
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid video command: {value}"),
            )
        })
    }
}

#[repr(u8)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Copy)]
pub enum VideoCodecId {
    /// Sorenson H.263
    SorensonH263 = 2,

    /// Screen video
    ScreenVideo = 3,

    /// On2 VP6
    On2VP6 = 4,

    /// On2 VP6 with alpha channel
    On2VP6Alpha = 5,

    /// AVC (H.264)
    Avc = 7,

    /// Reserved for future use
    ExHeader = 9,

    /// Legacy HEVC
    LegacyHevc = 12,
}

impl TryFrom<u8> for VideoCodecId {
    type Error = std::io::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            2 => Some(Self::SorensonH263),
            3 => Some(Self::ScreenVideo),
            4 => Some(Self::On2VP6),
            5 => Some(Self::On2VP6Alpha),
            7 => Some(Self::Avc),
            9 => Some(Self::ExHeader),
            12 => Some(Self::LegacyHevc),
            _ => None,
        }
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid video codec: {value}"),
            )
        })
    }
}

/// Represents a video codec using the FourCC (four character code) identification
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoFourCC {
    /// AVC (H.264) - "avc1"
    Avc1,
    /// HEVC (H.265) - "hvc1"
    Hvc1,
    /// VP8 - "vp08"
    Vp08,
    /// VP9 - "vp09"
    Vp09,
    /// AV1 - "av01"
    Av01,
}

impl VideoFourCC {
    /// Returns the 32-bit integer representation of the FourCC
    pub fn as_u32(&self) -> u32 {
        match self {
            Self::Avc1 => 0x61766331, // "avc1"
            Self::Hvc1 => 0x68766331, // "hvc1"
            Self::Vp08 => 0x76703038, // "vp08"
            Self::Vp09 => 0x76703039, // "vp09"
            Self::Av01 => 0x61763031, // "av01"
        }
    }

    /// Returns the FourCC as a 4-byte ASCII string
    pub fn as_bytes(&self) -> [u8; 4] {
        let value = self.as_u32();
        [
            ((value >> 24) & 0xFF) as u8,
            ((value >> 16) & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            (value & 0xFF) as u8,
        ]
    }

    /// Returns the FourCC as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Avc1 => "avc1",
            Self::Hvc1 => "hvc1",
            Self::Vp08 => "vp08",
            Self::Vp09 => "vp09",
            Self::Av01 => "av01",
        }
    }
}

impl TryFrom<u32> for VideoFourCC {
    type Error = std::io::Error;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0x61766331 => Ok(Self::Avc1),
            0x68766331 => Ok(Self::Hvc1),
            0x76703038 => Ok(Self::Vp08),
            0x76703039 => Ok(Self::Vp09),
            0x61763031 => Ok(Self::Av01),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unknown video FourCC: 0x{value:08x}"),
            )),
        }
    }
}

impl From<VideoFourCC> for u32 {
    fn from(four_cc: VideoFourCC) -> Self {
        four_cc.as_u32()
    }
}

impl std::fmt::Display for VideoFourCC {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<[u8; 4]> for VideoFourCC {
    fn from(bytes: [u8; 4]) -> Self {
        match bytes {
            [b'a', b'v', b'c', b'1'] => Self::Avc1,
            [b'h', b'v', b'c', b'1'] => Self::Hvc1,
            [b'v', b'p', b'0', b'8'] => Self::Vp08,
            [b'v', b'p', b'0', b'9'] => Self::Vp09,
            [b'a', b'v', b'0', b'1'] => Self::Av01,
            _ => Self::Av01, // Default case
        }
    }
}

/// FLV Tag Video Data Body
///
/// This is a container for video data.
/// This enum contains the data for the different types of video tags.
///
/// Defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Video
///   tags)
/// - video_file_format_spec_v10_1.pdf (Annex E.4.3.1 - VIDEODATA)
#[derive(Debug, Clone, PartialEq)]
pub enum VideoTagBody {
    /// AVC Video Packet (H.264)
    /// When [`VideoPacketType::CodecId`] is [`VideoCodecId::Avc`]
    Avc(AvcPacket),
    Hevc(HevcPacket),
    /// Enhanced Packet (AV1, H.265, etc.)
    /// When [`VideoPacketType::Enhanced`] is used
    Enhanced(EnhancedPacket),
    /// Command Frame (VideoInfo or Command)
    /// When [`VideoFrameType::VideoInfoOrCommandFrame`] is used
    Command(VideoCommand),
    /// Data we don't know how to parse
    Unknown {
        codec_id: VideoCodecId,
        data: Bytes,
    },
}

impl VideoTagBody {
    pub fn is_sequence_header(&self) -> bool {
        match self {
            VideoTagBody::Avc(avc_data) => {
                // Check if the data is a sequence header
                matches!(avc_data, AvcPacket::SequenceHeader(_))
            }
            VideoTagBody::Hevc(hevc_data) => {
                // Check if the data is a sequence header
                matches!(hevc_data, HevcPacket::SequenceStart(_))
            }
            _ => false,
        }
    }
}

/// A wrapper enum for the different types of video packets that can be used in
/// a FLV file.
///
/// Used to construct a [`VideoTagBody`].
///
/// See:
/// - [`VideoCodecId`]
/// - [`EnhancedPacketType`]
/// - [`VideoTagBody`]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VideoPacketType {
    /// Codec ID (legacy)
    CodecId(VideoCodecId),
    /// Enhanced (modern)
    Enhanced(EnhancedPacketType),
}

impl VideoPacketType {
    pub fn new(byte: u8, enhanced: bool) -> Self {
        if enhanced {
            Self::Enhanced(EnhancedPacketType::from(byte))
        } else {
            Self::CodecId(VideoCodecId::try_from(byte).unwrap())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Copy, Eq, PartialOrd, Ord, Hash)]
pub struct EnhancedPacketType(pub u8);

impl EnhancedPacketType {
    /// Sequence Start
    pub const SEQUENCE_START: Self = Self(0);
    /// Coded Frames
    pub const CODED_FRAMES: Self = Self(1);
    /// Sequence End
    pub const SEQUENCE_END: Self = Self(2);
    /// Coded Frames X
    pub const CODED_FRAMES_X: Self = Self(3);
    /// Metadata
    pub const METADATA: Self = Self(4);
    /// MPEG-2 Sequence Start
    pub const MPEG2_SEQUENCE_START: Self = Self(5);
}

impl From<u8> for EnhancedPacketType {
    fn from(value: u8) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for EnhancedPacketType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            0 => write!(f, "SequenceStart"),
            1 => write!(f, "CodedFrames"),
            2 => write!(f, "SequenceEnd"),
            3 => write!(f, "CodedFramesX"),
            4 => write!(f, "Metadata"),
            5 => write!(f, "Mpeg2SequenceStart"),
            _ => write!(f, "Unknown({})", self.0),
        }
    }
}

/// An Enhanced FLV Packet
///
/// This is a container for enhanced video packets.
/// The enchanced spec adds modern codecs to the FLV file format.
///
/// Defined by:
/// - enhanced_rtmp-v1.pdf (Defining Additional Video Codecs)
/// - enhanced_rtmp-v2.pdf (Enhanced Video)
#[derive(Debug, Clone, PartialEq)]
pub enum EnhancedPacket {
    /// Metadata
    Metadata {
        video_codec: VideoFourCC,
        data: Bytes,
    },
    /// Sequence End
    SequenceEnd { video_codec: VideoFourCC },
    /// Av1 Video Packet
    Av1(Av1Packet),
    /// Hevc (H.265) Video Packet
    Hevc(HevcPacket),
    /// We don't know how to parse it
    Unknown {
        packet_type: EnhancedPacketType,
        video_codec: VideoFourCC,
        data: Bytes,
    },
}

/// FLV Tag Video Header
/// This is a container for video data.
/// This enum contains the data for the different types of video tags.
/// Defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Video tags)
/// - video_file_format_spec_v10_1.pdf (Annex E.4.3.1 - VIDEODATA)
#[derive(Debug, Clone, PartialEq)]
pub struct VideoData {
    /// The frame type of the video data. (4 bits)
    pub frame_type: VideoFrameType,
    /// The body of the video data.
    pub body: VideoTagBody,
}

impl VideoData {
    /// Demux a video data from the given reader
    pub fn demux(reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        let byte = reader.read_u8()?;
        let enhanced = (byte & 0b1000_0000) != 0;
        let frame_type_byte = (byte >> 4) & 0b0111;
        let packet_type_byte = byte & 0b0000_1111;
        let frame_type: VideoFrameType = VideoFrameType::try_from(frame_type_byte)?;
        let body = if frame_type == VideoFrameType::VideoInfoFrame {
            let command_packet = VideoCommand::try_from(reader.read_u8()?)?;
            VideoTagBody::Command(command_packet)
        } else {
            VideoTagBody::demux(VideoPacketType::new(packet_type_byte, enhanced), reader)?
        };

        Ok(VideoData { frame_type, body })
    }
}

impl VideoTagBody {
    /// Demux a video packet from the given reader.
    /// The reader will consume all the data from the reader.
    pub fn demux(packet_type: VideoPacketType, reader: &mut io::Cursor<Bytes>) -> io::Result<Self> {
        match packet_type {
            VideoPacketType::CodecId(codec_id) => match codec_id {
                VideoCodecId::Avc => Ok(VideoTagBody::Avc(AvcPacket::demux(reader)?)),
                VideoCodecId::LegacyHevc => Ok(VideoTagBody::Hevc(HevcPacket::demux(reader)?)),
                _ => Ok(VideoTagBody::Unknown {
                    codec_id,
                    data: reader.extract_remaining(),
                }),
            },

            VideoPacketType::Enhanced(packet_type) => {
                let mut video_codec = [0; 4];
                reader.read_exact(&mut video_codec)?;
                let video_codec = VideoFourCC::from(video_codec);

                match packet_type {
                    EnhancedPacketType::SEQUENCE_END => {
                        return Ok(VideoTagBody::Enhanced(EnhancedPacket::SequenceEnd {
                            video_codec,
                        }));
                    }
                    EnhancedPacketType::METADATA => {
                        return Ok(VideoTagBody::Enhanced(EnhancedPacket::Metadata {
                            video_codec,
                            data: reader.extract_remaining(),
                        }));
                    }
                    _ => {}
                }

                debug!("Video codec: {:?}", video_codec);
                debug!("Packet type: {:?}", packet_type);

                match (video_codec, packet_type) {
                    (VideoFourCC::Avc1, EnhancedPacketType::SEQUENCE_START) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Av1(
                            Av1Packet::SequenceStart(AV1CodecConfigurationRecord::demux(reader)?),
                        )))
                    }
                    (VideoFourCC::Avc1, EnhancedPacketType::MPEG2_SEQUENCE_START) => Ok(
                        VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::SequenceStart(
                            AV1VideoDescriptor::demux(reader)?.codec_configuration_record,
                        ))),
                    ),
                    (VideoFourCC::Avc1, EnhancedPacketType::CODED_FRAMES) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::Raw(
                            reader.extract_remaining(),
                        ))))
                    }
                    (VideoFourCC::Hvc1, EnhancedPacketType::SEQUENCE_START) => Ok(
                        VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::SequenceStart(
                            HEVCDecoderConfigurationRecord::demux(reader)?,
                        ))),
                    ),
                    (VideoFourCC::Hvc1, EnhancedPacketType::CODED_FRAMES) => Ok(
                        VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                            composition_time: Some(reader.read_i24::<BigEndian>()?),
                            data: reader.extract_remaining(),
                        })),
                    ),
                    (VideoFourCC::Hvc1, EnhancedPacketType::CODED_FRAMES_X) => Ok(
                        VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                            composition_time: None,
                            data: reader.extract_remaining(),
                        })),
                    ),
                    (VideoFourCC::Av01, EnhancedPacketType::SEQUENCE_START) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Av1(
                            Av1Packet::SequenceStart(AV1CodecConfigurationRecord::demux(reader)?),
                        )))
                    }
                    (VideoFourCC::Av01, EnhancedPacketType::CODED_FRAMES) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::Raw(
                            reader.extract_remaining(),
                        ))))
                    }
                    (VideoFourCC::Av01, EnhancedPacketType::CODED_FRAMES_X) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::Raw(
                            reader.extract_remaining(),
                        ))))
                    }
                    (VideoFourCC::Av01, EnhancedPacketType::METADATA) => {
                        Ok(VideoTagBody::Enhanced(EnhancedPacket::Metadata {
                            video_codec,
                            data: reader.extract_remaining(),
                        }))
                    }
                    (VideoFourCC::Av01, EnhancedPacketType::MPEG2_SEQUENCE_START) => Ok(
                        VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::SequenceStart(
                            AV1VideoDescriptor::demux(reader)?.codec_configuration_record,
                        ))),
                    ),

                    _ => Ok(VideoTagBody::Enhanced(EnhancedPacket::Unknown {
                        packet_type,
                        video_codec,
                        data: reader.extract_remaining(),
                    })),
                }
            }
        }
    }

    pub fn get_video_resolution(&self) -> Option<Resolution> {
        match self {
            VideoTagBody::Avc(avc_data) => avc_data.get_video_resolution(),
            VideoTagBody::Hevc(hevc_data) => hevc_data.get_video_resolution(),
            _ => None,
        }
    }
}

impl std::fmt::Display for VideoData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VideoTag [{}] {}", self.frame_type, self.body)
    }
}

impl std::fmt::Display for VideoFrameType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyFrame => write!(f, "KeyFrame"),
            Self::InterFrame => write!(f, "InterFrame"),
            Self::DisposableInterFrame => write!(f, "DisposableInterFrame"),
            Self::GeneratedKeyFrame => write!(f, "GeneratedKeyFrame"),
            Self::VideoInfoFrame => write!(f, "VideoInfoFrame"),
        }
    }
}

impl std::fmt::Display for VideoTagBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoTagBody::Avc(packet) => write!(f, "AVC {packet}"),
            VideoTagBody::Hevc(packet) => write!(f, "HEVC {packet}"),
            VideoTagBody::Enhanced(packet) => write!(f, "{packet}"),
            VideoTagBody::Command(cmd) => write!(f, "Command: {cmd:?}"),
            VideoTagBody::Unknown { codec_id, data } => {
                write!(f, "Unknown Codec: {codec_id:?}, {data_len} bytes", data_len = data.len())
            }
        }
    }
}

impl std::fmt::Display for EnhancedPacket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnhancedPacket::Metadata { video_codec, data } => {
                write!(f, "Metadata [{video_codec}] ({}) bytes", data.len())
            }
            EnhancedPacket::SequenceEnd { video_codec } => {
                write!(f, "Sequence End [{video_codec}]")
            }
            EnhancedPacket::Av1(packet) => write!(f, "AV1 {packet}"),
            EnhancedPacket::Hevc(packet) => write!(f, "HEVC {packet}"),
            EnhancedPacket::Unknown {
                packet_type,
                video_codec,
                data,
            } => write!(
                f,
                "Unknown [{video_codec}] Type: {packet_type}, {data_len} bytes",
                packet_type = packet_type,
                data_len = data.len()
            ),
        }
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use super::*;
    use crate::avc::AvcPacketType;

    #[test]
    fn test_video_fourcc() {
        let cases = [
            (VideoFourCC::Av01, *b"av01", "Av01"),
            (VideoFourCC::Vp09, *b"vp09", "Vp09"),
            (VideoFourCC::Hvc1, *b"hvc1", "Hvc1"),
            (VideoFourCC::Avc1, *b"avc1", "Avc1"),
        ];

        for (expected, bytes, name) in cases {
            assert_eq!(VideoFourCC::from(bytes), expected);
            assert_eq!(format!("{:?}", VideoFourCC::from(bytes)), name);
        }
    }

    #[test]
    fn test_enhanced_packet_type() {
        let cases = [
            (EnhancedPacketType::SEQUENCE_START, 0, "SequenceStart"),
            (EnhancedPacketType::CODED_FRAMES, 1, "CodedFrames"),
            (EnhancedPacketType::SEQUENCE_END, 2, "SequenceEnd"),
            (EnhancedPacketType::CODED_FRAMES_X, 3, "CodedFramesX"),
            (EnhancedPacketType::METADATA, 4, "Metadata"),
            (
                EnhancedPacketType::MPEG2_SEQUENCE_START,
                5,
                "Mpeg2SequenceStart",
            ),
            (EnhancedPacketType(6), 6, "Unknown(6)"),
            (EnhancedPacketType(7), 7, "Unknown(7)"),
        ];

        for (expected, value, name) in cases {
            assert_eq!(EnhancedPacketType::from(value), expected);
            // Compare with Display format instead of Debug format
            assert_eq!(format!("{}", EnhancedPacketType::from(value)), name);
        }
    }

    #[test]
    fn test_frame_type() {
        let cases = [
            (VideoFrameType::KeyFrame, 1, "KeyFrame"),
            (VideoFrameType::InterFrame, 2, "InterFrame"),
            (
                VideoFrameType::DisposableInterFrame,
                3,
                "DisposableInterFrame",
            ),
            (VideoFrameType::GeneratedKeyFrame, 4, "GeneratedKeyFrame"),
            (VideoFrameType::VideoInfoFrame, 5, "VideoInfoFrame"),
        ];

        for (expected, value, name) in cases {
            assert_eq!(VideoFrameType::try_from(value).unwrap(), expected);
            // Compare with short name instead of full enum path
            assert_eq!(
                format!("{:?}", VideoFrameType::try_from(value).unwrap())
                    .split("::")
                    .last()
                    .unwrap(),
                name
            );
        }

        // Test error cases
        assert!(VideoFrameType::try_from(6).is_err());
        assert!(VideoFrameType::try_from(7).is_err());
    }

    #[test]
    fn test_video_codec_id() {
        let cases = [
            (VideoCodecId::SorensonH263, 2, "SorensonH263"),
            (VideoCodecId::ScreenVideo, 3, "ScreenVideo"),
            (VideoCodecId::On2VP6, 4, "On2VP6"),
            (VideoCodecId::On2VP6Alpha, 5, "On2VP6Alpha"),
            (VideoCodecId::Avc, 7, "Avc"),
            (VideoCodecId::ExHeader, 9, "ExHeader"),
            (VideoCodecId::LegacyHevc, 12, "LegacyHevc"),
        ];

        for (expected, value, name) in cases {
            assert_eq!(VideoCodecId::try_from(value).unwrap(), expected);
            // Compare with short name instead of full enum path
            assert_eq!(
                format!("{:?}", VideoCodecId::try_from(value).unwrap())
                    .split("::")
                    .last()
                    .unwrap(),
                name
            );
        }

        // Test error cases
        assert!(VideoCodecId::try_from(10).is_err());
        assert!(VideoCodecId::try_from(11).is_err());
        assert!(VideoCodecId::try_from(15).is_err());

        // Fix the ScreenVideo case which now appears twice
        assert!(VideoCodecId::try_from(6).is_err());
    }

    #[test]
    fn test_command_packet() {
        let cases = [
            (VideoCommand::StartSeek, 1, "StartSeek"),
            (VideoCommand::EndSeek, 2, "EndSeek"),
        ];

        for (expected, value, name) in cases {
            assert_eq!(VideoCommand::try_from(value).unwrap(), expected);
            // Compare with short name instead of full enum path
            assert_eq!(
                format!("{:?}", VideoCommand::try_from(value).unwrap())
                    .split("::")
                    .last()
                    .unwrap(),
                name
            );
        }

        // Test error cases
        assert!(VideoCommand::try_from(3).is_err());
        assert!(VideoCommand::try_from(4).is_err());
    }

    #[test]
    fn test_video_packet_type() {
        let cases = [
            (
                1,
                true,
                VideoPacketType::Enhanced(EnhancedPacketType::CODED_FRAMES),
            ),
            (7, false, VideoPacketType::CodecId(VideoCodecId::Avc)),
        ];

        for (value, enhanced, expected) in cases {
            assert_eq!(VideoPacketType::new(value, enhanced), expected);
        }
    }

    #[test]
    fn test_video_data_body_avc() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        ]));
        let packet_type = VideoPacketType::new(0x7, false);
        let body = VideoTagBody::demux(packet_type, &mut reader).unwrap();
        assert_eq!(
            body,
            VideoTagBody::Avc(AvcPacket::Nalu {
                // first byte is the avc packet type (in this case, 1 = nalu)
                composition_time: 0x020304,
                data: Bytes::from_static(&[0x05, 0x06, 0x07, 0x08]),
            })
        );

        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0x05, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
        ]));
        let packet_type = VideoPacketType::new(0x7, false);
        let body = VideoTagBody::demux(packet_type, &mut reader).unwrap();
        assert_eq!(
            body,
            VideoTagBody::Avc(AvcPacket::Unknown {
                avc_packet_type: AvcPacketType::try_from(5).unwrap(),
                composition_time: 0x020304,
                data: Bytes::from_static(&[0x05, 0x06, 0x07, 0x08]),
            })
        );
    }

    #[test]
    fn test_video_data_body_hevc() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            b'h', b'v', b'c', b'1', // video codec
            0x01, 0x02, 0x03, 0x04, // data
            0x05, 0x06, 0x07, 0x08, // data
        ]));
        let packet_type = VideoPacketType::new(0x3, true);
        let body = VideoTagBody::demux(packet_type, &mut reader).unwrap();
        assert_eq!(
            body,
            VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                composition_time: None,
                data: Bytes::from_static(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]),
            }))
        );

        // Fix this assertion by using the correct codec in the expected value
        assert_eq!(
            body,
            VideoTagBody::Enhanced(EnhancedPacket::Hevc(HevcPacket::Nalu {
                composition_time: None,
                data: Bytes::from_static(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]),
            }))
        );
    }

    #[test]
    fn test_video_data_command_packet() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0b01010000, // frame type (5)
            0x01,       // command packet
        ]));
        let body = VideoData::demux(&mut reader).unwrap();
        assert_eq!(
            body,
            VideoData {
                frame_type: VideoFrameType::VideoInfoFrame,
                body: VideoTagBody::Command(VideoCommand::StartSeek),
            }
        );
    }

    #[test]
    fn test_video_data_demux_h263() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0b00010010, // enhanced + keyframe
            0, 1, 2, 3, // data
        ]));
        let body = VideoData::demux(&mut reader).unwrap();
        assert_eq!(
            body,
            VideoData {
                frame_type: VideoFrameType::KeyFrame,
                body: VideoTagBody::Unknown {
                    codec_id: VideoCodecId::SorensonH263,
                    data: Bytes::from_static(&[0, 1, 2, 3]),
                },
            }
        );
    }

    #[test]
    fn test_av1_mpeg2_sequence_start() {
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            0b10010101, // enhanced + keyframe
            b'a', b'v', b'0', b'1', // video codec
            0x80, 0x4, 129, 13, 12, 0, 10, 15, 0, 0, 0, 106, 239, 191, 225, 188, 2, 25, 144, 16,
            16, 16, 64,
        ]));

        let body = VideoData::demux(&mut reader).unwrap();
        assert_eq!(
            body,
            VideoData {
                frame_type: VideoFrameType::KeyFrame,
                body: VideoTagBody::Enhanced(EnhancedPacket::Av1(Av1Packet::SequenceStart(
                    AV1CodecConfigurationRecord {
                        seq_profile: 0,
                        seq_level_idx_0: 13,
                        seq_tier_0: false,
                        high_bitdepth: false,
                        twelve_bit: false,
                        monochrome: false,
                        chroma_subsampling_x: true,
                        chroma_subsampling_y: true,
                        chroma_sample_position: 0,
                        hdr_wcg_idc: 0,
                        initial_presentation_delay_minus_one: None,
                        config_obu: Bytes::from_static(
                            b"\n\x0f\0\0\0j\xef\xbf\xe1\xbc\x02\x19\x90\x10\x10\x10@"
                        ),
                    }
                ))),
            }
        );
    }

    #[test]
    fn test_legacy_hevc_parsing() {
        // Test 1: Network Abstraction Layer Unit packet (type 1) - should have composition time
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            // HEVC packet type 1 (Network Abstraction Layer Unit)
            0x01, // Composition time offset (0x001234)
            0x00, 0x12, 0x34, // Some fake HEVC Network Abstraction Layer Unit data
            0x40, 0x01, 0x0c, 0x01, 0x77,
        ]));

        // Create packet type for legacy HEVC
        let packet_type = VideoPacketType::CodecId(VideoCodecId::LegacyHevc);

        // Parse the data
        let body = VideoTagBody::demux(packet_type, &mut reader).unwrap();

        // Verify the parsed data
        assert_eq!(
            body,
            VideoTagBody::Hevc(HevcPacket::Nalu {
                composition_time: Some(0x001234),
                data: Bytes::from_static(&[0x40, 0x01, 0x0c, 0x01, 0x77]),
            })
        );

        // Test 2: End of sequence packet (type 2)
        let mut reader = io::Cursor::new(Bytes::from_static(&[
            // HEVC packet type 2 (end of sequence)
            0x02, 0x00, 0x00, 0x00, // Need composition time even for end of sequence
        ]));

        // Create packet type for legacy HEVC
        let packet_type = VideoPacketType::CodecId(VideoCodecId::LegacyHevc);

        // Parse the data
        let body = VideoTagBody::demux(packet_type, &mut reader).unwrap();

        // Verify the parsed data
        assert_eq!(body, VideoTagBody::Hevc(HevcPacket::EndOfSequence));
    }
}
