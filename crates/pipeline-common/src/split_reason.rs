//! Split reason types shared across FLV and HLS pipelines.
//!
//! These types describe why a stream was split into a new segment file.

use std::fmt;

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
    /// Video resolution changed.
    ResolutionChange { from: (u32, u32), to: (u32, u32) },
    /// Stream structural parameter changed (program info, transport stream ID, init segment CRC, etc.).
    StreamStructureChange { description: String },
    /// HLS playlist discontinuity tag encountered.
    Discontinuity,
}

impl fmt::Display for SplitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VideoCodecChange { from, to } => {
                write!(f, "video codec change: {} -> {}", from.codec, to.codec)?;
                if let (Some(w), Some(h)) = (from.width, from.height) {
                    write!(f, " (from {w}x{h}")?;
                    if let (Some(w2), Some(h2)) = (to.width, to.height) {
                        write!(f, " to {w2}x{h2}")?;
                    }
                    write!(f, ")")?;
                } else if let (Some(w2), Some(h2)) = (to.width, to.height) {
                    write!(f, " (to {w2}x{h2})")?;
                }
                Ok(())
            }
            Self::AudioCodecChange { from, to } => {
                write!(f, "audio codec change: {} -> {}", from.codec, to.codec)
            }
            Self::SizeLimit => write!(f, "size limit"),
            Self::DurationLimit => write!(f, "duration limit"),
            Self::HeaderReceived => write!(f, "header received"),
            Self::ResolutionChange { from, to } => {
                write!(
                    f,
                    "resolution change: {}x{} -> {}x{}",
                    from.0, from.1, to.0, to.1
                )
            }
            Self::StreamStructureChange { description } => {
                write!(f, "stream structure change: {description}")
            }
            Self::Discontinuity => write!(f, "discontinuity"),
        }
    }
}
