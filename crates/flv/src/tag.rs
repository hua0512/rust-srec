use std::fmt;
use std::io::Read;

use bytes::{Buf, Bytes};
use bytes_util::BytesCursorExt;
use tracing::{debug, trace};

use crate::audio::{AudioFourCC, AudioPacketType, SoundFormat};
use crate::resolution::Resolution;
use crate::video::{EnhancedPacketType, VideoCodec, VideoCodecId, VideoFourCC, VideoFrameType};
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
    tag_type: FlvTagType,
    /// Whether the tag payload is filtered/encrypted (Filter bit set in tag header).
    ///
    /// When this is true, the payload is not a plain legacy AUDIODATA/VIDEODATA/SCRIPTDATA payload.
    is_filtered: bool,
    /// Copy free buffer
    data: Bytes,
    class: TagClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecKind {
    SorensonH263,
    ScreenVideo,
    On2Vp6,
    On2Vp6Alpha,
    Avc,
    Hevc,
    Vp8,
    Vp9,
    Av1,
    Pcm,
    AdPcm,
    Mp3,
    PcmLe,
    Nellymoser16khzMono,
    Nellymoser8khzMono,
    Nellymoser,
    G711ALaw,
    G711MuLaw,
    Aac,
    Speex,
    Mp38k,
    DeviceSpecific,
    Ac3,
    EAc3,
    Opus,
    Flac,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TagClass {
    pub keyframe: bool,
    pub keyframe_media: bool,
    pub sequence_header: bool,
    pub end_of_sequence: bool,
    pub enhanced: bool,
    pub codec: Option<CodecKind>,
}

/// E-RTMP tag header fields with ModEx records skipped and the Multitrack
/// wrapper descended into. Audio and video share the record layout and differ
/// only in the packet-type discriminants ([`EnhancedPacketType::MULTITRACK`]/
/// [`EnhancedPacketType::MOD_EX`] for video, [`AudioPacketType::Multitrack`]/
/// [`AudioPacketType::ModEx`] for audio).
struct ExPacketHeader {
    /// The effective packet type nibble: taken from the first byte, replaced
    /// by the value carried after any ModEx records, then by the per-track
    /// packet type when the tag is a Multitrack wrapper.
    packet_type: u8,
    /// The four FourCC bytes when present; `TagClass::keyframe_media` uses
    /// their presence as its payload-present check.
    four_cc_bytes: Option<[u8; 4]>,
}

/// Parses the E-RTMP header of an enhanced audio or video payload (first byte
/// has the ExHeader marker).
///
/// ModEx packets are `[size][data]` records whose trailing byte carries the
/// ModEx data type in the high nibble and the next packet type in the low
/// nibble. Multitrack packets carry the `AvMultitrackType` nibble and the
/// per-track packet type in the following byte; in every multitrack layout
/// the next four bytes are a FourCC (shared for OneTrack/ManyTracks, the
/// first track's for ManyTracksManyCodecs). Returns `None` when the bytes
/// needed to resolve the packet type are missing, or on a nested Multitrack
/// wrapper, which E-RTMP forbids.
fn parse_ex_packet_header(data: &[u8], multitrack: u8, mod_ex: u8) -> Option<ExPacketHeader> {
    let first_byte = *data.first()?;
    let mut packet_type = first_byte & 0x0F;
    let mut pos = 1_usize;

    while packet_type == mod_ex {
        // modExDataSize is UI8 + 1, escaping to UI16 + 1 when the UI8 is 255.
        let mut size = *data.get(pos)? as usize + 1;
        pos += 1;
        if size == 256 {
            let bytes = <[u8; 2]>::try_from(data.get(pos..pos + 2)?).ok()?;
            size = u16::from_be_bytes(bytes) as usize + 1;
            pos += 2;
        }
        pos = pos.checked_add(size)?;
        packet_type = *data.get(pos)? & 0x0F;
        pos += 1;
    }

    if packet_type == multitrack {
        // High nibble is AvMultitrackType; the FourCC read below lands on the
        // same offset for all of its values, so the nibble needs no decoding.
        packet_type = *data.get(pos)? & 0x0F;
        pos += 1;
        if packet_type == multitrack {
            return None;
        }
    }

    Some(ExPacketHeader {
        packet_type,
        four_cc_bytes: data
            .get(pos..pos + 4)
            .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok()),
    })
}

fn parse_enhanced_video_header(data: &[u8]) -> Option<ExPacketHeader> {
    parse_ex_packet_header(
        data,
        EnhancedPacketType::MULTITRACK.0,
        EnhancedPacketType::MOD_EX.0,
    )
}

fn video_codec_from_payload(data: &[u8]) -> Option<VideoCodec> {
    let first_byte = *data.first()?;
    let frame_type = (first_byte >> 4) & 0x07;
    if frame_type == VideoFrameType::VideoInfoFrame as u8 {
        return None;
    }

    if first_byte & 0x80 != 0 {
        return parse_enhanced_video_header(data)?
            .four_cc_bytes
            .and_then(|bytes| VideoFourCC::try_from(bytes).ok())
            .map(VideoCodec::Enhanced);
    }

    VideoCodecId::try_from(first_byte & 0x0F)
        .ok()
        .filter(|codec_id| *codec_id != VideoCodecId::ExHeader)
        .map(VideoCodec::Legacy)
}

fn codec_kind_from_video_codec(codec: VideoCodec) -> Option<CodecKind> {
    Some(match codec {
        VideoCodec::Legacy(VideoCodecId::SorensonH263) => CodecKind::SorensonH263,
        VideoCodec::Legacy(VideoCodecId::ScreenVideo) => CodecKind::ScreenVideo,
        VideoCodec::Legacy(VideoCodecId::On2VP6) => CodecKind::On2Vp6,
        VideoCodec::Legacy(VideoCodecId::On2VP6Alpha) => CodecKind::On2Vp6Alpha,
        VideoCodec::Legacy(VideoCodecId::Avc) | VideoCodec::Enhanced(VideoFourCC::Avc1) => {
            CodecKind::Avc
        }
        VideoCodec::Legacy(VideoCodecId::LegacyHevc) | VideoCodec::Enhanced(VideoFourCC::Hvc1) => {
            CodecKind::Hevc
        }
        VideoCodec::Enhanced(VideoFourCC::Vp08) => CodecKind::Vp8,
        VideoCodec::Enhanced(VideoFourCC::Vp09) => CodecKind::Vp9,
        VideoCodec::Enhanced(VideoFourCC::Av01) => CodecKind::Av1,
        VideoCodec::Legacy(VideoCodecId::ExHeader) => return None,
    })
}

impl TagClass {
    fn from_payload(tag_type: FlvTagType, is_filtered: bool, data: &[u8]) -> Self {
        if is_filtered {
            return Self::default();
        }

        match tag_type {
            FlvTagType::Video => Self::from_video_payload(data),
            FlvTagType::Audio => Self::from_audio_payload(data),
            _ => Self::default(),
        }
    }

    fn from_video_payload(data: &[u8]) -> Self {
        let Some(&first_byte) = data.first() else {
            return Self::default();
        };

        let enhanced = first_byte & 0x80 != 0;
        let frame_type = (first_byte >> 4) & 0x07;
        let keyframe = frame_type == VideoFrameType::KeyFrame as u8;

        if frame_type == VideoFrameType::VideoInfoFrame as u8 {
            return Self {
                enhanced,
                ..Self::default()
            };
        }

        if enhanced {
            // A truncated or nested-multitrack header cannot be classified
            // beyond the frame type; report no packet flags rather than guess.
            let Some(header) = parse_enhanced_video_header(data) else {
                return Self {
                    keyframe,
                    enhanced: true,
                    ..Self::default()
                };
            };
            let packet_type = EnhancedPacketType::from(header.packet_type);
            let codec = header
                .four_cc_bytes
                .and_then(|bytes| VideoFourCC::try_from(bytes).ok())
                .map(VideoCodec::Enhanced)
                .and_then(codec_kind_from_video_codec);
            let coded_frames = packet_type == EnhancedPacketType::CODED_FRAMES
                || packet_type == EnhancedPacketType::CODED_FRAMES_X;

            return Self {
                keyframe,
                keyframe_media: keyframe && coded_frames && header.four_cc_bytes.is_some(),
                sequence_header: packet_type == EnhancedPacketType::SEQUENCE_START,
                end_of_sequence: packet_type == EnhancedPacketType::SEQUENCE_END,
                enhanced: true,
                codec,
            };
        }

        let video_codec = video_codec_from_payload(data);
        let codec_id = match video_codec {
            Some(VideoCodec::Legacy(codec_id)) => Some(codec_id),
            _ => None,
        };
        let codec = video_codec.and_then(codec_kind_from_video_codec);
        let packet_type = data.get(1).copied();
        let packetized = matches!(codec_id, Some(VideoCodecId::Avc | VideoCodecId::LegacyHevc));

        Self {
            keyframe,
            keyframe_media: keyframe
                && if packetized {
                    packet_type == Some(1)
                } else {
                    codec.is_some() && data.len() >= 2
                },
            sequence_header: packetized && packet_type == Some(0),
            end_of_sequence: packetized && packet_type == Some(2),
            enhanced: false,
            codec,
        }
    }

    fn from_audio_payload(data: &[u8]) -> Self {
        let Some(&first_byte) = data.first() else {
            return Self::default();
        };
        let Ok(sound_format) = SoundFormat::try_from(first_byte >> 4) else {
            return Self::default();
        };

        if sound_format == SoundFormat::ExHeader {
            // A truncated or nested-multitrack header cannot be classified
            // beyond the ExHeader marker; report no packet flags rather than
            // guess.
            let Some(header) = parse_ex_packet_header(
                data,
                AudioPacketType::Multitrack as u8,
                AudioPacketType::ModEx as u8,
            ) else {
                return Self {
                    enhanced: true,
                    ..Self::default()
                };
            };
            let codec = header
                .four_cc_bytes
                .and_then(|bytes| AudioFourCC::from_u32(u32::from_be_bytes(bytes)).ok())
                .map(|codec| match codec {
                    AudioFourCC::Ac3 => CodecKind::Ac3,
                    AudioFourCC::Eac3 => CodecKind::EAc3,
                    AudioFourCC::Opus => CodecKind::Opus,
                    AudioFourCC::Mp3 => CodecKind::Mp3,
                    AudioFourCC::Flac => CodecKind::Flac,
                    AudioFourCC::Aac => CodecKind::Aac,
                });

            return Self {
                sequence_header: header.packet_type == AudioPacketType::SequenceStart as u8,
                end_of_sequence: header.packet_type == AudioPacketType::SequenceEnd as u8,
                enhanced: true,
                codec,
                ..Self::default()
            };
        }

        let codec = Some(match sound_format {
            SoundFormat::Pcm => CodecKind::Pcm,
            SoundFormat::AdPcm => CodecKind::AdPcm,
            SoundFormat::Mp3 => CodecKind::Mp3,
            SoundFormat::PcmLe => CodecKind::PcmLe,
            SoundFormat::Nellymoser16khzMono => CodecKind::Nellymoser16khzMono,
            SoundFormat::Nellymoser8khzMono => CodecKind::Nellymoser8khzMono,
            SoundFormat::Nellymoser => CodecKind::Nellymoser,
            SoundFormat::G711ALaw => CodecKind::G711ALaw,
            SoundFormat::G711MuLaw => CodecKind::G711MuLaw,
            SoundFormat::Aac => CodecKind::Aac,
            SoundFormat::Speex => CodecKind::Speex,
            SoundFormat::Mp38k => CodecKind::Mp38k,
            SoundFormat::DeviceSpecific => CodecKind::DeviceSpecific,
            SoundFormat::ExHeader => return Self::default(),
        });

        Self {
            sequence_header: sound_format == SoundFormat::Aac && data.get(1) == Some(&0),
            codec,
            ..Self::default()
        }
    }
}

fn demux_tag_header(reader: &mut std::io::Cursor<Bytes>) -> std::io::Result<ParsedTagHeader> {
    let mut header_bytes = [0u8; framing::TAG_HEADER_SIZE];
    reader.read_exact(&mut header_bytes)?;
    framing::parse_tag_header_bytes(header_bytes)
}

impl FlvTag {
    pub fn new(
        timestamp_ms: u32,
        stream_id: u32,
        tag_type: FlvTagType,
        is_filtered: bool,
        data: Bytes,
    ) -> Self {
        let class = TagClass::from_payload(tag_type, is_filtered, &data);
        Self {
            timestamp_ms,
            stream_id,
            tag_type,
            is_filtered,
            data,
            class,
        }
    }

    pub fn classification(&self) -> TagClass {
        self.class
    }

    pub fn tag_type(&self) -> FlvTagType {
        self.tag_type
    }

    pub fn set_tag_type(&mut self, tag_type: FlvTagType) {
        self.tag_type = tag_type;
        self.refresh_classification();
    }

    pub fn is_filtered(&self) -> bool {
        self.is_filtered
    }

    pub fn set_filtered(&mut self, is_filtered: bool) {
        self.is_filtered = is_filtered;
        self.refresh_classification();
    }

    pub fn data(&self) -> &Bytes {
        &self.data
    }

    pub fn into_data(self) -> Bytes {
        self.data
    }

    pub fn set_data(&mut self, data: Bytes) {
        self.data = data;
        self.refresh_classification();
    }

    fn refresh_classification(&mut self) {
        self.class = TagClass::from_payload(self.tag_type, self.is_filtered, &self.data);
    }

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

        Ok(Self::new(
            header.timestamp_ms,
            header.stream_id,
            header.tag_type,
            header.is_filtered,
            data,
        ))
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
        self.tag_type == FlvTagType::Video && self.class.keyframe
    }

    pub fn is_video_sequence_header(&self) -> bool {
        self.tag_type == FlvTagType::Video && self.class.sequence_header
    }

    /// Determines if the audio tag is a sequence header.
    ///
    /// For audio tags, the sequence header is indicated by the second byte being 0
    /// in AAC format audio packets. This function checks the following:
    /// - If the tag type is `Audio`.
    /// - If the sound format is AAC (10).
    /// - If the AAC packet type (at offset 1) is 0, which indicates a sequence header.
    pub fn is_audio_sequence_header(&self) -> bool {
        self.tag_type == FlvTagType::Audio && self.class.sequence_header
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
                body.get_video_resolution()
                    .filter(|res| res.width > 0.0 && res.height > 0.0)
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
        match self.get_video_codec()? {
            VideoCodec::Legacy(codec_id) => Some(codec_id),
            VideoCodec::Enhanced(_) => None,
        }
    }

    /// Returns the codec identifier carried by this video tag.
    ///
    /// Legacy tags return [`VideoCodec::Legacy`], while enhanced tags return
    /// [`VideoCodec::Enhanced`] with their FourCC value.
    pub fn get_video_codec(&self) -> Option<VideoCodec> {
        if self.tag_type != FlvTagType::Video || self.is_filtered {
            return None;
        }

        video_codec_from_payload(&self.data)
    }

    pub fn get_audio_codec_id(&self) -> Option<SoundFormat> {
        if self.tag_type != FlvTagType::Audio {
            return None;
        }

        if self.class.enhanced {
            return Some(SoundFormat::ExHeader);
        }

        match self.class.codec? {
            CodecKind::Pcm => Some(SoundFormat::Pcm),
            CodecKind::AdPcm => Some(SoundFormat::AdPcm),
            CodecKind::Mp3 => Some(SoundFormat::Mp3),
            CodecKind::PcmLe => Some(SoundFormat::PcmLe),
            CodecKind::Nellymoser16khzMono => Some(SoundFormat::Nellymoser16khzMono),
            CodecKind::Nellymoser8khzMono => Some(SoundFormat::Nellymoser8khzMono),
            CodecKind::Nellymoser => Some(SoundFormat::Nellymoser),
            CodecKind::G711ALaw => Some(SoundFormat::G711ALaw),
            CodecKind::G711MuLaw => Some(SoundFormat::G711MuLaw),
            CodecKind::Aac => Some(SoundFormat::Aac),
            CodecKind::Speex => Some(SoundFormat::Speex),
            CodecKind::Mp38k => Some(SoundFormat::Mp38k),
            CodecKind::DeviceSpecific => Some(SoundFormat::DeviceSpecific),
            _ => None,
        }
    }

    /// Check if the tag is a key frame NALU
    pub fn is_key_frame_nalu(&self) -> bool {
        self.tag_type == FlvTagType::Video && self.class.keyframe_media
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

#[cfg(test)]
mod tests {
    use super::*;

    fn video_tag(data: &[u8]) -> FlvTag {
        FlvTag::new(0, 0, FlvTagType::Video, false, Bytes::copy_from_slice(data))
    }

    fn audio_tag(data: &[u8]) -> FlvTag {
        FlvTag::new(0, 0, FlvTagType::Audio, false, Bytes::copy_from_slice(data))
    }

    // Enhanced byte 0 layout: [IsExHeader:1][FrameType:3][PacketType:4].
    // 0x9_ = ExHeader + KeyFrame, 0xA_ = ExHeader + InterFrame.

    #[test]
    fn enhanced_keyframe_coded_frames_is_keyframe_nalu() {
        // KeyFrame + CodedFrames (hvc1)
        let tag = video_tag(&[0x91, b'h', b'v', b'c', b'1', 0, 0, 0]);
        assert!(tag.is_key_frame());
        assert!(tag.is_key_frame_nalu());
        assert!(!tag.is_video_sequence_header());

        // KeyFrame + CodedFramesX (av01)
        let tag = video_tag(&[0x93, b'a', b'v', b'0', b'1', 0, 0, 0]);
        assert!(tag.is_key_frame_nalu());
    }

    #[test]
    fn classifies_enhanced_hevc_coded_frame_at_construction() {
        let tag = FlvTag::new(
            42,
            0,
            FlvTagType::Video,
            false,
            Bytes::from_static(&[0x91, b'h', b'v', b'c', b'1', 0, 0, 0]),
        );

        assert_eq!(
            tag.classification(),
            TagClass {
                keyframe: true,
                keyframe_media: true,
                sequence_header: false,
                end_of_sequence: false,
                enhanced: true,
                codec: Some(CodecKind::Hevc),
            }
        );
    }

    #[test]
    fn classifies_legacy_avc_sequence_header() {
        let tag = video_tag(&[0x17, 0x00, 0, 0, 0]);
        let class = tag.classification();

        assert!(class.keyframe);
        assert!(!class.keyframe_media);
        assert!(class.sequence_header);
        assert!(!class.end_of_sequence);
        assert!(!class.enhanced);
        assert_eq!(class.codec, Some(CodecKind::Avc));
    }

    #[test]
    fn classifies_enhanced_av1_sequence_end() {
        let tag = video_tag(&[0x92, b'a', b'v', b'0', b'1']);
        let class = tag.classification();

        assert!(class.keyframe);
        assert!(!class.keyframe_media);
        assert!(!class.sequence_header);
        assert!(class.end_of_sequence);
        assert!(class.enhanced);
        assert_eq!(class.codec, Some(CodecKind::Av1));
        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
    }

    #[test]
    fn returns_legacy_video_codec() {
        let tag = video_tag(&[0x17, 0, 0, 0, 0]);

        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Legacy(VideoCodecId::Avc))
        );
    }

    #[test]
    fn classifies_enhanced_aac_sequence_header() {
        let tag = audio_tag(&[0x90, b'm', b'p', b'4', b'a']);
        let class = tag.classification();

        assert!(class.sequence_header);
        assert!(class.enhanced);
        assert_eq!(class.codec, Some(CodecKind::Aac));
    }

    // Audio ExHeader byte 0 layout: [SoundFormat:4][AudioPacketType:4].
    // 0x95 = ExHeader + Multitrack, 0x97 = ExHeader + ModEx; the multitrack
    // byte carries [AvMultitrackType:4][AudioPacketType:4].

    #[test]
    fn audio_multitrack_one_track_sequence_start_classifies_codec() {
        // Shared FourCC, track id, then an AudioSpecificConfig.
        let tag = audio_tag(&[0x95, 0x00, b'm', b'p', b'4', b'a', 0x00, 0x12, 0x10]);
        let class = tag.classification();

        assert!(class.sequence_header);
        assert!(class.enhanced);
        assert_eq!(class.codec, Some(CodecKind::Aac));
        assert!(tag.is_audio_sequence_header());
    }

    #[test]
    fn audio_multitrack_many_tracks_sequence_end_classifies_codec() {
        // Shared FourCC, then per track: track id + UI24 track size.
        let tag = audio_tag(&[0x95, 0x12, b'm', b'p', b'4', b'a', 0x00, 0x00, 0x00, 0x00]);
        let class = tag.classification();

        assert!(class.end_of_sequence);
        assert!(!class.sequence_header);
        assert_eq!(class.codec, Some(CodecKind::Aac));
    }

    #[test]
    fn audio_mod_ex_record_resolves_real_packet_type() {
        // Record = size byte 0x02 (3 bytes of data), the data, then
        // TimestampOffsetNano + CodedFrames.
        let tag = audio_tag(&[
            0x97, 0x02, 0x00, 0x00, 0x01, 0x01, b'O', b'p', b'u', b's', 0xAA,
        ]);
        let class = tag.classification();

        assert!(!class.sequence_header);
        assert!(!class.end_of_sequence);
        assert!(class.enhanced);
        assert_eq!(class.codec, Some(CodecKind::Opus));
    }

    #[test]
    fn truncated_audio_multitrack_and_mod_ex_headers_classify_nothing() {
        for payload in [&[0x95][..], &[0x97][..], &[0x97, 0x02, 0x00][..]] {
            let tag = audio_tag(payload);
            let class = tag.classification();

            assert!(class.enhanced);
            assert!(!class.sequence_header);
            assert!(!class.end_of_sequence);
            assert_eq!(class.codec, None);
        }
    }

    #[test]
    fn nested_audio_multitrack_is_rejected() {
        // An audio Multitrack per-track packet type of Multitrack is forbidden
        // by E-RTMP; refuse to classify instead of misreading the FourCC.
        let tag = audio_tag(&[0x95, 0x25, b'm', b'p', b'4', b'a', 0x00]);
        let class = tag.classification();

        assert!(class.enhanced);
        assert!(!class.sequence_header);
        assert_eq!(class.codec, None);
    }

    #[test]
    fn filtered_payload_has_no_classification() {
        let tag = FlvTag::new(
            0,
            0,
            FlvTagType::Video,
            true,
            Bytes::from_static(&[0x91, b'h', b'v', b'c', b'1']),
        );

        assert_eq!(tag.classification(), TagClass::default());
        assert_eq!(tag.get_video_codec(), None);
    }

    #[test]
    fn classification_stays_current_when_payload_fields_change() {
        let mut tag = video_tag(&[0x27, 0x01, 0, 0, 0]);
        assert!(!tag.is_key_frame_nalu());

        tag.set_data(Bytes::from_static(&[0x17, 0x01, 0, 0, 0]));
        assert!(tag.is_key_frame_nalu());

        tag.set_filtered(true);
        assert_eq!(tag.classification(), TagClass::default());

        tag.set_tag_type(FlvTagType::Audio);
        tag.set_filtered(false);
        tag.set_data(Bytes::from_static(&[0xAF, 0x00]));
        assert!(tag.is_audio_sequence_header());
        assert_eq!(tag.tag_type(), FlvTagType::Audio);
        assert!(!tag.is_filtered());
        assert_eq!(tag.data(), &Bytes::from_static(&[0xAF, 0x00]));
    }

    #[test]
    fn enhanced_interframe_is_not_keyframe() {
        // InterFrame + CodedFrames must not be treated as a keyframe:
        // frame type lives in bits 4-6, not in the packet-type nibble.
        let tag = video_tag(&[0xA1, b'h', b'v', b'c', b'1', 0, 0, 0]);
        assert!(!tag.is_key_frame());
        assert!(!tag.is_key_frame_nalu());
    }

    #[test]
    fn enhanced_sequence_start_is_header_not_keyframe_nalu() {
        // KeyFrame + SequenceStart carries codec config, not frame data.
        let tag = video_tag(&[0x90, b'h', b'v', b'c', b'1', 0, 0, 0]);
        assert!(tag.is_video_sequence_header());
        assert!(!tag.is_key_frame_nalu());
    }

    #[test]
    fn enhanced_command_does_not_expose_a_codec_or_packet_classification() {
        let tag = video_tag(&[0xD0, 0x00]);

        assert!(tag.classification().enhanced);
        assert!(!tag.is_video_sequence_header());
        assert_eq!(tag.classification().codec, None);
        assert_eq!(tag.get_video_codec(), None);
    }

    // Multitrack byte layout: [AvMultitrackType:4][VideoPacketType:4].
    // 0x00 = OneTrack + SequenceStart, 0x13 = ManyTracks + CodedFramesX,
    // 0x21 = ManyTracksManyCodecs + CodedFrames.

    #[test]
    fn multitrack_one_track_sequence_start_classifies_codec() {
        // 0x96 = ExHeader + KeyFrame + Multitrack; then the shared FourCC,
        // track id, and an av1C configuration record.
        let tag = video_tag(&[
            0x96, 0x00, b'a', b'v', b'0', b'1', 0x00, 0x81, 0x0D, 0x0C, 0x00,
        ]);
        let class = tag.classification();

        assert!(class.enhanced);
        assert!(class.sequence_header);
        assert!(!class.end_of_sequence);
        assert!(!class.keyframe_media);
        assert_eq!(class.codec, Some(CodecKind::Av1));
        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
    }

    #[test]
    fn multitrack_many_tracks_coded_frames_is_keyframe_media() {
        // Shared FourCC, then per track: track id + UI24 track size + payload.
        let tag = video_tag(&[
            0x96, 0x13, b'h', b'v', b'c', b'1', 0x00, 0x00, 0x00, 0x02, 0xAA, 0xBB,
        ]);
        let class = tag.classification();

        assert!(class.keyframe);
        assert!(class.keyframe_media);
        assert!(!class.sequence_header);
        assert_eq!(class.codec, Some(CodecKind::Hevc));
    }

    #[test]
    fn multitrack_many_codecs_uses_first_track_four_cc() {
        // ManyTracksManyCodecs puts each track's FourCC ahead of its track id.
        let tag = video_tag(&[
            0x96, 0x21, b'a', b'v', b'0', b'1', 0x01, 0x00, 0x00, 0x01, 0xCC,
        ]);

        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
        assert!(tag.classification().keyframe_media);
        assert_eq!(tag.classification().codec, Some(CodecKind::Av1));
    }

    #[test]
    fn mod_ex_record_resolves_real_packet_type() {
        // 0x97 = ExHeader + KeyFrame + ModEx; record = size byte 0x02 (3 bytes
        // of data), the data, then 0x01 = TimestampOffsetNano + CodedFrames.
        let tag = video_tag(&[
            0x97, 0x02, 0x00, 0x00, 0x01, 0x01, b'a', b'v', b'0', b'1', 0xDE, 0xAD,
        ]);
        let class = tag.classification();

        assert!(class.keyframe_media);
        assert!(!class.sequence_header);
        assert_eq!(class.codec, Some(CodecKind::Av1));
        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
    }

    #[test]
    fn mod_ex_ui16_size_escape_is_honored() {
        // Size byte 0xFF escapes to a UI16 size; 0x0000 + 1 = one data byte.
        let tag = video_tag(&[
            0x97, 0xFF, 0x00, 0x00, 0x42, 0x00, b'a', b'v', b'0', b'1', 0x81, 0x0D, 0x0C, 0x00,
        ]);

        assert!(tag.is_video_sequence_header());
        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Av01))
        );
    }

    #[test]
    fn mod_ex_record_chains_into_multitrack() {
        // ModEx record (one data byte) resolving to Multitrack, then
        // ManyTracks + CodedFramesX with the shared FourCC.
        let tag = video_tag(&[
            0x97, 0x00, 0x42, 0x06, 0x13, b'h', b'v', b'c', b'1', 0x00, 0x00, 0x00, 0x02, 0xAA,
            0xBB,
        ]);
        let class = tag.classification();

        assert!(class.keyframe_media);
        assert_eq!(class.codec, Some(CodecKind::Hevc));
        assert_eq!(
            tag.get_video_codec(),
            Some(VideoCodec::Enhanced(VideoFourCC::Hvc1))
        );
    }

    #[test]
    fn truncated_multitrack_and_mod_ex_headers_classify_nothing() {
        // Missing the multitrack byte / ModEx record: only the frame type and
        // the ExHeader bit are trustworthy.
        for payload in [&[0x96][..], &[0x97][..], &[0x97, 0x02, 0x00][..]] {
            let tag = video_tag(payload);
            let class = tag.classification();

            assert!(class.enhanced);
            assert!(class.keyframe);
            assert!(!class.sequence_header);
            assert!(!class.keyframe_media);
            assert_eq!(class.codec, None);
            assert_eq!(tag.get_video_codec(), None);
        }
    }

    #[test]
    fn nested_multitrack_is_rejected() {
        // A Multitrack per-track packet type of Multitrack is forbidden by
        // E-RTMP; refuse to classify instead of misreading the FourCC.
        let tag = video_tag(&[0x96, 0x16, b'a', b'v', b'0', b'1', 0x00]);
        let class = tag.classification();

        assert!(class.enhanced);
        assert!(!class.sequence_header);
        assert_eq!(class.codec, None);
        assert_eq!(tag.get_video_codec(), None);
    }

    #[test]
    fn legacy_avc_packet_types() {
        // 0x17 = KeyFrame + AVC; byte 1 is AVCPacketType.
        let seq_header = video_tag(&[0x17, 0x00, 0, 0, 0]);
        assert!(seq_header.is_video_sequence_header());
        assert!(!seq_header.is_key_frame_nalu());

        let keyframe_nalu = video_tag(&[0x17, 0x01, 0, 0, 0]);
        assert!(keyframe_nalu.is_key_frame_nalu());
        assert!(!keyframe_nalu.is_video_sequence_header());

        // 0x27 = InterFrame + AVC
        let inter_nalu = video_tag(&[0x27, 0x01, 0, 0, 0]);
        assert!(!inter_nalu.is_key_frame_nalu());
    }

    #[test]
    fn legacy_h263_keyframe_is_gop_boundary_not_sequence_header() {
        // 0x12 = KeyFrame + SorensonH263; byte 1 is frame data (here 0x00),
        // which must not be misread as an AVCPacketType sequence-header marker.
        let tag = video_tag(&[0x12, 0x00, 0x00, 0x84]);
        assert!(tag.is_key_frame_nalu());
        assert!(!tag.is_video_sequence_header());
    }

    #[test]
    fn empty_payload_predicates_do_not_panic() {
        let video = video_tag(&[]);
        assert!(!video.is_key_frame());
        assert!(!video.is_key_frame_nalu());
        assert!(!video.is_video_sequence_header());
        assert_eq!(video.get_video_codec_id(), None);
        assert_eq!(video.get_video_codec(), None);

        let audio = audio_tag(&[]);
        assert!(!audio.is_audio_sequence_header());
        assert_eq!(audio.get_audio_codec_id(), None);

        // One-byte legacy AVC tag: too short to carry an AVCPacketType.
        let short = video_tag(&[0x17]);
        assert!(!short.is_video_sequence_header());
        assert!(!short.is_key_frame_nalu());
    }
}
