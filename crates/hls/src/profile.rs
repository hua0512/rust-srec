use std::fmt::Display;

use crate::resolution;

/// The type of segment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    /// Transport Stream segment
    Ts,
    /// MP4 initialization segment
    M4sInit,
    /// MP4 media segment
    M4sMedia,
    /// End of playlist marker
    EndMarker,
}

impl Display for SegmentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SegmentType::Ts => write!(f, "ts"),
            SegmentType::M4sInit => write!(f, "m4s"),
            SegmentType::M4sMedia => write!(f, "m4s"),
            SegmentType::EndMarker => write!(f, "end_marker"),
        }
    }
}

/// Compact stream profile for quick segment analysis
#[derive(Debug, Clone)]
pub struct StreamProfile {
    pub has_video: bool,
    pub has_audio: bool,
    pub has_h264: bool,
    pub has_h265: bool,
    pub has_av1: bool,
    pub has_aac: bool,
    pub has_ac3: bool,
    pub resolution: Option<resolution::Resolution>,
    pub summary: String,
}

/// Options controlling how stream profiling is performed.
///
/// Some profile fields (like resolution) may require parsing codec
/// configuration data and can be skipped for performance.
#[derive(Debug, Clone, Copy)]
pub struct StreamProfileOptions {
    /// If `true`, attempt to parse and populate [`StreamProfile::resolution`].
    pub include_resolution: bool,
}

impl Default for StreamProfileOptions {
    fn default() -> Self {
        Self {
            include_resolution: true,
        }
    }
}

impl StreamProfile {
    /// Check if this profile indicates a complete multimedia stream
    pub fn is_complete(&self) -> bool {
        self.has_video && self.has_audio
    }

    /// Get primary video codec
    pub fn primary_video_codec(&self) -> Option<&'static str> {
        if self.has_av1 {
            Some("AV1")
        } else if self.has_h265 {
            Some("H.265/HEVC")
        } else if self.has_h264 {
            Some("H.264/AVC")
        } else {
            None
        }
    }

    /// Get primary audio codec
    pub fn primary_audio_codec(&self) -> Option<&'static str> {
        if self.has_aac {
            Some("AAC")
        } else if self.has_ac3 {
            Some("AC-3")
        } else {
            None
        }
    }

    /// Get a brief codec description
    pub fn codec_description(&self) -> String {
        let video = self.primary_video_codec().unwrap_or("Unknown");
        let audio = self.primary_audio_codec().unwrap_or("Unknown");
        format!("Video: {video}, Audio: {audio}")
    }
}
