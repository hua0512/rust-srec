use std::fmt;

use byteorder::{BigEndian, ReadBytesExt};
use bytes::{Buf, Bytes};
use bytes_util::BytesCursorExt;
use tracing::error;

use crate::audio::SoundFormat;
use crate::resolution::Resolution;
use crate::video::{EnhancedPacketType, VideoCodecId, VideoFrameType, VideoPacketType};

use super::audio::AudioData;
use super::script::ScriptData;
use super::video::VideoData;

/// An FLV Tag
///
/// Tags have different types and thus different data structures. To accommodate
/// this the [`FlvTagData`] enum is used.
///
/// Defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - FLV
///   tags)
/// - video_file_format_spec_v10_1.pdf (Annex E.4.1 - FLV Tag)
///
/// The v10.1 spec adds some additional fields to the tag to accomodate
/// encryption. We dont support this because it is not needed for our use case.
/// (and I suspect it is not used anywhere anymore.)
///
/// However if the Tag is encrypted the tag_type will be a larger number (one we
/// dont support), and therefore the [`FlvTagData::Unknown`] variant will be
/// used.
#[derive(Debug, Clone, PartialEq)]
pub struct FlvTagOwned {
    /// A timestamp in milliseconds
    pub timestamp_ms: u32,
    /// A stream id
    pub stream_id: u32,
    pub data: FlvTagData,
}

/// An FLV Tag with an Bytes buffer
#[derive(Debug, Clone, PartialEq)]
pub struct FlvTag {
    /// A timestamp in milliseconds
    pub timestamp_ms: u32,
    /// A stream id
    pub stream_id: u32,
    /// The type of the tag
    pub tag_type: FlvTagType,
    /// Copy free buffer
    pub data: Bytes,
}

pub trait FlvUtil<T> {
    fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<T>;

    fn is_script_tag(&self) -> bool;

    fn is_audio_tag(&self) -> bool;

    fn is_video_tag(&self) -> bool;

    fn is_key_frame(&self) -> bool;

    fn is_video_sequence_header(&self) -> bool;

    fn is_audio_sequence_header(&self) -> bool;
}

impl FlvUtil<FlvTagOwned> for FlvTagOwned {
    /// Demux a FLV tag from the given reader.
    ///
    /// The reader will be advanced to the end of the tag.
    ///
    /// The reader needs to be a [`std::io::Cursor`] with a [`Bytes`] buffer because we
    /// take advantage of zero-copy reading.
    fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<Self> {
        let tag_type = FlvTagType::from(reader.read_u8()?);

        let data_size = reader.read_u24::<BigEndian>()?;

        // check if we have the correct amount of bytes to read
        if reader.remaining() < data_size as usize {
            // set the position back to the start of the tag
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "Not enough bytes to read for tag type {}. Expected {} bytes, got {} bytes",
                    tag_type,
                    data_size,
                    reader.remaining()
                ),
            ));
        }

        // The timestamp bit is weird. Its 24bits but then there is an extended 8 bit
        // number to create a 32bit number.
        let timestamp_ms = reader.read_u24::<BigEndian>()? | ((reader.read_u8()? as u32) << 24);

        // The stream id according to the spec is ALWAYS 0. (likely not true)
        let stream_id = reader.read_u24::<BigEndian>()?;

        // We then extract the data from the reader. (advancing the cursor to the end of
        // the tag)
        let data = reader.extract_bytes(data_size as usize)?;

        // Finally we demux the data.
        let data = FlvTagData::demux(tag_type, &mut std::io::Cursor::new(data))?;

        Ok(FlvTagOwned {
            timestamp_ms,
            stream_id,
            data,
        })
    }

    fn is_script_tag(&self) -> bool {
        matches!(self.data, FlvTagData::ScriptData(_))
    }

    fn is_audio_tag(&self) -> bool {
        matches!(self.data, FlvTagData::Audio(_))
    }

    fn is_video_tag(&self) -> bool {
        matches!(self.data, FlvTagData::Video(_))
    }

    fn is_key_frame(&self) -> bool {
        match self.data {
            FlvTagData::Video(ref video_data) => video_data.frame_type == VideoFrameType::KeyFrame,
            _ => false,
        }
    }

    fn is_video_sequence_header(&self) -> bool {
        match self.data {
            FlvTagData::Video(ref video_data) => video_data.body.is_sequence_header(),
            _ => false,
        }
    }

    fn is_audio_sequence_header(&self) -> bool {
        match self.data {
            FlvTagData::Audio(ref audio_data) => audio_data.body.is_sequence_header(),
            _ => false,
        }
    }
}

impl FlvUtil<FlvTag> for FlvTag {
    fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<FlvTag> {
        {
            let tag_type = FlvTagType::from(reader.read_u8()?);

            let data_size = reader.read_u24::<BigEndian>()?;

            // check if we have the correct amount of bytes to read
            if reader.remaining() < data_size as usize {
                // set the position back to the start of the tag
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    format!(
                        "Not enough bytes to read for tag type {}. Expected {} bytes, got {} bytes",
                        tag_type,
                        data_size,
                        reader.remaining()
                    ),
                ));
            }

            // The timestamp bit is weird. Its 24bits but then there is an extended 8 bit
            // number to create a 32bit number.
            let timestamp_ms = reader.read_u24::<BigEndian>()? | ((reader.read_u8()? as u32) << 24);

            // The stream id according to the spec is ALWAYS 0. (likely not true)
            let stream_id = reader.read_u24::<BigEndian>()?;

            // We then extract the data from the reader. (advancing the cursor to the end of
            // the tag)
            let data = reader.extract_bytes(data_size as usize)?;

            Ok(FlvTag {
                timestamp_ms,
                stream_id,
                tag_type,
                // Bytes is a copy free buffer
                data,
            })
        }
    }

    fn is_script_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::ScriptData)
    }

    fn is_audio_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::Audio)
    }

    fn is_video_tag(&self) -> bool {
        matches!(self.tag_type, FlvTagType::Video)
    }

    fn is_key_frame(&self) -> bool {
        match self.tag_type {
            FlvTagType::Video => {
                if self.data.is_empty() {
                    return false;
                }

                let bytes = self.data.as_ref();
                let first_byte = bytes[0];

                // Check if this is an enhanced type
                // let enhanced = (first_byte & 0b1000_0000) != 0;

                // For both legacy and enhanced, frame type is in bits 4-7
                let frame_type = (first_byte >> 4) & 0x07;

                // VideoFrameType::KeyFrame = 1
                frame_type == VideoFrameType::KeyFrame as u8
            }
            _ => false,
        }
    }

    fn is_video_sequence_header(&self) -> bool {
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
    fn is_audio_sequence_header(&self) -> bool {
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

impl fmt::Display for FlvTagOwned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "FlvTag [Time: {}ms, Stream: {}] {}",
            self.timestamp_ms, self.stream_id, self.data
        )
    }
}

impl FlvTag {
    pub fn demux(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<FlvTag> {
        let tag_type = FlvTagType::from(reader.read_u8()?);

        let data_size = reader.read_u24::<BigEndian>()?;

        // check if we have the correct amount of bytes to read
        if reader.remaining() < data_size as usize {
            // set the position back to the start of the tag
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "Not enough bytes to read for tag type {}. Expected {} bytes, got {} bytes",
                    tag_type,
                    data_size,
                    reader.remaining()
                ),
            ));
        }

        // The timestamp bit is weird. Its 24bits but then there is an extended 8 bit
        // number to create a 32bit number.
        let timestamp_ms = reader.read_u24::<BigEndian>()? | ((reader.read_u8()? as u32) << 24);

        // The stream id according to the spec is ALWAYS 0. (likely not true)
        let stream_id = reader.read_u24::<BigEndian>()?;

        // We then extract the data from the reader. (advancing the cursor to the end of
        // the tag)
        let data = reader.extract_bytes(data_size as usize)?;

        Ok(FlvTag {
            timestamp_ms,
            stream_id,
            tag_type,
            data,
        })
    }

    pub fn size(&self) -> usize {
        self.data.len() + 11
    }

    /// Get the video resolution from the tag
    pub fn get_video_resolution(&self) -> Option<Resolution> {
        // Only video tags have a resolution
        if self.tag_type != FlvTagType::Video {
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
                error!("Error parsing demuxing data: {}", e);
                None
            }
        }
    }

    pub fn get_video_codec_id(&self) -> Option<VideoCodecId> {
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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// FLV Tag Data
///
/// This is a container for the actual media data.
/// This enum contains the data for the different types of tags.
///
/// Defined by:
/// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - FLV tags)
/// - video_file_format_spec_v10_1.pdf (Annex E.4.1 - FLV Tag)
#[derive(Debug, Clone, PartialEq)]
pub enum FlvTagData {
    /// AudioData when the FlvTagType is Audio(8)
    /// Defined by:
    /// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Audio tags)
    /// - video_file_format_spec_v10_1.pdf (Annex E.4.2.1 - AUDIODATA)
    Audio(AudioData),
    /// VideoData when the FlvTagType is Video(9)
    /// Defined by:
    /// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Video tags)
    /// - video_file_format_spec_v10_1.pdf (Annex E.4.3.1 - VIDEODATA)
    Video(VideoData),
    /// ScriptData when the FlvTagType is ScriptData(18)
    /// Defined by:
    /// - video_file_format_spec_v10.pdf (Chapter 1 - The FLV File Format - Data tags)
    /// - video_file_format_spec_v10_1.pdf (Annex E.4.4.1 - SCRIPTDATA)
    ScriptData(ScriptData),
    /// Any tag type that we dont know how to parse, with the corresponding data
    /// being the raw bytes of the tag
    Unknown { tag_type: FlvTagType, data: Bytes },
}

impl FlvTagData {
    /// Demux a FLV tag data from the given reader.
    ///
    /// The reader will be enirely consumed.
    ///
    /// The reader needs to be a [`std::io::Cursor`] with a [`Bytes`] buffer because we
    /// take advantage of zero-copy reading.
    pub fn demux(
        tag_type: FlvTagType,
        reader: &mut std::io::Cursor<Bytes>,
    ) -> std::io::Result<Self> {
        match tag_type {
            FlvTagType::Audio => Ok(FlvTagData::Audio(AudioData::demux(reader, None)?)),
            FlvTagType::Video => Ok(FlvTagData::Video(VideoData::demux(reader)?)),
            FlvTagType::ScriptData => Ok(FlvTagData::ScriptData(ScriptData::demux(reader)?)),
            _ => Ok(FlvTagData::Unknown {
                tag_type,
                data: reader.extract_remaining(),
            }),
        }
    }
}

impl fmt::Display for FlvTagData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlvTagData::Audio(audio) => write!(f, "Audio: {audio}"),
            FlvTagData::Video(video) => write!(f, "Video: {video}"),
            FlvTagData::ScriptData(script) => write!(f, "Script: {script}"),
            FlvTagData::Unknown { tag_type, data } => {
                write!(f, "Unknown (type: {tag_type:?}, {data_len} bytes)", data_len = data.len())
            }
        }
    }
}
