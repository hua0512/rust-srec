use std::fmt;
use std::io::Read;

use bytes::{Buf, Bytes};
use bytes_util::BytesCursorExt;
use tracing::{debug, trace};

use crate::audio::SoundFormat;
use crate::resolution::Resolution;
use crate::video::{EnhancedPacketType, VideoCodecId, VideoFrameType, VideoPacketType};
use crate::{framing, framing::ParsedTagHeader};

use super::audio::AudioData;
use super::script::ScriptData;
use super::video::VideoData;

/// An FLV Tag with a `Bytes` payload buffer.
#[derive(Debug, Clone, PartialEq)]
pub struct FlvTag {
    /// A timestamp in milliseconds
    pub timestamp_ms: u32,
    /// A stream id
    pub stream_id: u32,
    /// The type of the tag
    pub tag_type: FlvTagType,
    /// Whether the tag payload is filtered/encrypted (Filter bit set in tag header).
    ///
    /// When this is true, the payload is not a plain legacy AUDIODATA/VIDEODATA/SCRIPTDATA payload.
    pub is_filtered: bool,
    /// Copy free buffer
    pub data: Bytes,
}

fn demux_tag_header(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<ParsedTagHeader> {
    let mut header_bytes = [0u8; framing::TAG_HEADER_SIZE];
    reader.read_exact(&mut header_bytes)?;
    framing::parse_tag_header_bytes(header_bytes)
}

impl FlvTag {
    pub fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<FlvTag> {
        let header = demux_tag_header(reader)?;
        if reader.remaining() < header.data_size as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "Not enough bytes to read for tag type {}. Expected {} bytes, got {} bytes",
                    header.tag_type,
                    header.data_size,
                    reader.remaining()
                ),
            ));
        }

        let data = reader.extract_bytes(header.data_size as usize)?;

        Ok(FlvTag {
            timestamp_ms: header.timestamp_ms,
            stream_id: header.stream_id,
            tag_type: header.tag_type,
            is_filtered: header.is_filtered,
            data,
        })
    }

    pub fn is_script_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::ScriptData)
    }

    pub fn is_audio_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::Audio)
    }

    pub fn is_video_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::Video)
    }

    pub fn is_key_frame(&self) -> bool {
        if self.is_filtered {
            return false;
        }
        match self.tag_type {
            FlvTagType::Video => {
                if self.data.is_empty() {
                    return false;
                }

                let bytes = self.data.as_ref();
                let first_byte = bytes[0];

                // Check if this is an enhanced type
                let enhanced = (first_byte & 0b1000_0000) != 0;

                if enhanced {
                    // For enhanced video, frame type is in bits 0-3
                    let frame_type = first_byte & 0x0F;
                    // VideoFrameType::KeyFrame = 1
                    frame_type == VideoFrameType::KeyFrame as u8
                } else {
                    // For legacy video, frame type is in bits 4-7
                    let frame_type = (first_byte >> 4) & 0x0F;
                    // VideoFrameType::KeyFrame = 1
                    frame_type == VideoFrameType::KeyFrame as u8
                }
            }
            _ => false,
        }
    }

    pub fn is_video_sequence_header(&self) -> bool {
        if self.is_filtered {
            return false;
        }
        match self.tag_type {
            FlvTagType::Video => {
                let bytes = self.data.as_ref();
                // peek the first byte
                let enhanced = (bytes[0] & 0b1000_0000) != 0;
                // for legacy formats, we detect the sequence header by checking the packet type
                if !enhanced {
                    let video_packet_type = bytes.get(1).unwrap_or(&0) & 0x0F;
                    video_packet_type == 0x0
                } else {
                    let video_packet_type = bytes.first().unwrap_or(&0) & 0x0F;
                    let video_packet_type = VideoPacketType::new(video_packet_type, enhanced);
                    match video_packet_type {
                        VideoPacketType::Enhanced(packet) => {
                            packet == EnhancedPacketType::SEQUENCE_START
                        }
                        _ => false,
                    }
                }
            }
            _ => false,
        }
    }

    /// Determines if the audio tag is a sequence header.
    ///
    /// For audio tags, the sequence header is indicated by the second byte being 0
    /// in AAC format audio packets. This function checks the following:
    /// - If the tag type is `Audio`.
    /// - If the sound format is AAC (10).
    /// - If the AAC packet type (at offset 1) is 0, which indicates a sequence header.
    pub fn is_audio_sequence_header(&self) -> bool {
        if self.is_filtered {
            return false;
        }
        match self.tag_type {
            FlvTagType::Audio => {
                let bytes = self.data.as_ref();
                if bytes.len() < 2 {
                    return false;
                }

                let sound_format = (bytes[0] >> 4) & 0xF;

                if sound_format == SoundFormat::Aac as u8 {
                    return bytes[1] == 0;
                }
                false
            }
            _ => false,
        }
    }
}

impl FlvTag {
    pub fn size(&self) -> usize {
        self.data.len() + 11
    }

    pub fn decode_audio(&self) -> std::io::Result<AudioData> {
        if self.is_filtered {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cannot decode filtered/encrypted FLV audio tag payload",
            ));
        }

        if self.tag_type != FlvTagType::Audio {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tag is not an audio tag",
            ));
        }

        let mut cursor = std::io::Cursor::new(self.data.clone());
        AudioData::demux(&mut cursor, None)
    }

    pub fn decode_video(&self) -> std::io::Result<VideoData> {
        if self.is_filtered {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cannot decode filtered/encrypted FLV video tag payload",
            ));
        }

        if self.tag_type != FlvTagType::Video {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tag is not a video tag",
            ));
        }

        let mut cursor = std::io::Cursor::new(self.data.clone());
        VideoData::demux(&mut cursor)
    }

    pub fn decode_script(&self) -> std::io::Result<ScriptData> {
        if self.is_filtered {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cannot decode filtered/encrypted FLV script tag payload",
            ));
        }

        if self.tag_type != FlvTagType::ScriptData {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "tag is not a script tag",
            ));
        }

        let mut cursor = std::io::Cursor::new(self.data.clone());
        ScriptData::demux(&mut cursor)
    }

    /// Get the video resolution from the tag
    pub fn get_video_resolution(&self) -> Option<Resolution> {
        if self.is_filtered {
            return None;
        }
        // Only video tags have a resolution
        if self.tag_type != FlvTagType::Video {
            return None;
        }

        // Best-effort parsing: avoid noisy demux errors for truncated/placeholder tags.
        // (The AVC/HEVC header alone is already >= 5 bytes; anything shorter can't be parsed.)
        if self.data.len() < 5 {
            trace!(
                len = self.data.len(),
                "Video tag too small for resolution parsing"
            );
            return None;
        }

        let data = self.data.clone();
        let mut reader = std::io::Cursor::new(data);
        // parse to owned version
        match VideoData::demux(&mut reader) {
            Ok(video_data) => {
                let body = video_data.body;
                body.get_video_resolution().and_then(|res| {
                    if res.width > 0.0 && res.height > 0.0 {
                        Some(res)
                    } else {
                        None
                    }
                })
            }
            Err(e) => {
                debug!(
                    len = self.data.len(),
                    error = %e,
                    "Failed to demux video tag while extracting resolution"
                );
                None
            }
        }
    }

    pub fn get_video_codec_id(&self) -> Option<VideoCodecId> {
        if self.is_filtered {
            return None;
        }
        // Only video tags have a codec id
        if self.tag_type != FlvTagType::Video {
            return None;
        }

        let data = self.data.clone();
        let mut reader = std::io::Cursor::new(data);
        // peek the first byte
        let first_byte = reader.get_u8();
        // check if this is an enhanced type
        let enhanced = (first_byte & 0b1000_0000) != 0;
        // for legacy formats, we detect the codec id by checking the packet type
        if !enhanced {
            let video_packet_type = first_byte & 0x0F;
            VideoCodecId::try_from(video_packet_type).ok()
        } else {
            // unable to parse the codec id for enhanced formats
            None
        }
    }

    pub fn get_audio_codec_id(&self) -> Option<SoundFormat> {
        if self.is_filtered {
            return None;
        }
        // Only audio tags have a codec id
        if self.tag_type != FlvTagType::Audio {
            return None;
        }

        let data = self.data.clone();
        let mut reader = std::io::Cursor::new(data);
        // peek the first byte
        let first_byte = reader.get_u8();
        // check if this is an enhanced type
        let sound_format = (first_byte >> 4) & 0xF;
        SoundFormat::try_from(sound_format).ok()
    }

    /// Check if the tag is a key frame NALU
    pub fn is_key_frame_nalu(&self) -> bool {
        if self.is_filtered {
            return false;
        }
        // Only applicable for video tags
        if self.tag_type != FlvTagType::Video {
            return false;
        }

        // Make sure we have enough data
        if self.data.len() < 2 {
            return false;
        }

        let bytes = self.data.as_ref();

        // check if its keyframe
        let frame_type = (bytes[0] >> 4) & 0x07;
        if frame_type != VideoFrameType::KeyFrame as u8 {
            return false;
        }

        // Check if this is an enhanced type
        let enhanced = (bytes[0] & 0b1000_0000) != 0;

        // For non-enhanced types, the codec type is in the lower 4 bits of the first byte
        if !enhanced {
            let codec_id = bytes[0] & 0x0F;

            // Check for AVC/H.264 (codec ID 7) or HEVC (codec ID 12)
            if codec_id == VideoCodecId::Avc as u8 || codec_id == VideoCodecId::LegacyHevc as u8 {
                // The packet type is in the second byte:
                // 0 = sequence header, 1 = NALU, 2 = end of sequence

                // Check if this is a NALU packet (type 1)
                return bytes[1] == 1;
            }
        }

        false
    }
}

/// FLV Tag Type
///
/// This is the type of the tag.
///
/// Defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - FLV tags)
/// - video_file_format_spec_v10_1.pdf (Annex E.4.1 - FLV Tag)
///
/// The 3 types that are supported are:
/// - Audio(8)
/// - Video(9)
/// - ScriptData(18)
///
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlvTagType {
    Audio = 8,
    Video = 9,
    ScriptData = 18,
    Unknown(u8),
}

impl From<u8> for FlvTagType {
    fn from(value: u8) -> Self {
        match value {
            8 => FlvTagType::Audio,
            9 => FlvTagType::Video,
            18 => FlvTagType::ScriptData,
            _ => FlvTagType::Unknown(value),
        }
    }
}

impl From<FlvTagType> for u8 {
    fn from(value: FlvTagType) -> Self {
        match value {
            FlvTagType::Audio => 8,
            FlvTagType::Video => 9,
            FlvTagType::ScriptData => 18,
            FlvTagType::Unknown(val) => val,
        }
    }
}

impl fmt::Display for FlvTagType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlvTagType::Audio => write!(f, "Audio"),
            FlvTagType::Video => write!(f, "Video"),
            FlvTagType::ScriptData => write!(f, "Script"),
            FlvTagType::Unknown(value) => write!(f, "Unknown({value})"),
        }
    }
}
