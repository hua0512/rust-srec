use bytes::Bytes;

use crate::{header::FlvHeader, tag::FlvTag};

/// Parsed video codec configuration snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoCodecInfo {
    /// Codec identifier (e.g. "AVC", "HEVC", "AV1"), or the raw VideoCodecId/FourCC as string.
    pub codec: String,
    /// Profile (e.g. 100 for AVC High, general_profile_idc for HEVC, seq_profile for AV1).
    pub profile: Option<u8>,
    /// Level (e.g. 40 for AVC Level 4.0, general_level_idc for HEVC, seq_level_idx_0 for AV1).
    pub level: Option<u8>,
    /// Resolution width if parseable from the sequence header SPS.
    pub width: Option<u32>,
    /// Resolution height if parseable from the sequence header SPS.
    pub height: Option<u32>,
    /// CRC32 signature of the codec configuration portion.
    pub signature: u32,
}

/// Parsed audio codec configuration snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioCodecInfo {
    /// Codec identifier (e.g. "AAC", "MP3"), or the raw SoundFormat as string.
    pub codec: String,
    /// Sample rate in Hz (best-effort, from ADTS header or AudioSpecificConfig).
    pub sample_rate: Option<u32>,
    /// Number of channels (best-effort).
    pub channels: Option<u8>,
    /// CRC32 signature of the codec configuration portion.
    pub signature: u32,
}

/// Reason why a stream split occurred.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitReason {
    /// Video codec configuration changed.
    VideoCodecChange {
        /// Previous configuration (before the change).
        from: VideoCodecInfo,
        /// New configuration (after the change).
        to: VideoCodecInfo,
    },
    /// Audio codec configuration changed.
    AudioCodecChange {
        /// Previous configuration (before the change).
        from: AudioCodecInfo,
        /// New configuration (after the change).
        to: AudioCodecInfo,
    },
    /// File size limit reached.
    SizeLimit,
    /// Duration limit reached.
    DurationLimit,
    /// A new FLV header arrived from upstream (stream restart/reconnect).
    HeaderReceived,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FlvData {
    Header(FlvHeader),
    Tag(FlvTag),
    /// Explicit split marker emitted before a re-injected header.
    Split(SplitReason),
    EndOfSequence(Bytes),
}

impl FlvData {
    pub fn size(&self) -> usize {
        match self {
            FlvData::Header(_) => 9 + 4,
            FlvData::Tag(tag) => tag.size() + 4,
            FlvData::Split(_) => 0,
            FlvData::EndOfSequence(data) => data.len() + 4,
        }
    }

    pub fn is_header(&self) -> bool {
        matches!(self, FlvData::Header(_))
    }

    pub fn is_tag(&self) -> bool {
        matches!(self, FlvData::Tag(_))
    }

    pub fn is_split(&self) -> bool {
        matches!(self, FlvData::Split(_))
    }

    pub fn is_end_of_sequence(&self) -> bool {
        matches!(self, FlvData::EndOfSequence(_))
    }

    // Helper for easier comparison in tests, ignoring data potentially
    pub fn description(&self) -> String {
        match self {
            FlvData::Header(_) => "Header".to_string(),
            FlvData::Tag(tag) => format!("{:?}@{}", tag.tag_type, tag.timestamp_ms),
            FlvData::Split(reason) => format!("Split({reason:?})"),
            FlvData::EndOfSequence(_) => "EndOfSequence".to_string(),
        }
    }
}
