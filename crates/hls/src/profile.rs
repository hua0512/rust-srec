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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_type_display() {
        assert_eq!(SegmentType::Ts.to_string(), "ts");
        assert_eq!(SegmentType::M4sInit.to_string(), "m4s");
        assert_eq!(SegmentType::M4sMedia.to_string(), "m4s");
        assert_eq!(SegmentType::EndMarker.to_string(), "end_marker");
    }

    #[test]
    fn test_stream_profile_is_complete() {
        let profile = StreamProfile {
            has_video: true,
            has_audio: true,
            has_h264: true,
            has_h265: false,
            has_av1: false,
            has_aac: true,
            has_ac3: false,
            resolution: None,
            summary: String::new(),
        };
        assert!(profile.is_complete());

        let video_only = StreamProfile {
            has_video: true,
            has_audio: false,
            has_h264: true,
            has_h265: false,
            has_av1: false,
            has_aac: false,
            has_ac3: false,
            resolution: None,
            summary: String::new(),
        };
        assert!(!video_only.is_complete());
    }

    #[test]
    fn test_stream_profile_codec_description() {
        let profile = StreamProfile {
            has_video: true,
            has_audio: true,
            has_h264: true,
            has_h265: false,
            has_av1: false,
            has_aac: true,
            has_ac3: false,
            resolution: None,
            summary: String::new(),
        };
        assert_eq!(profile.codec_description(), "Video: H.264/AVC, Audio: AAC");

        let hevc_ac3 = StreamProfile {
            has_video: true,
            has_audio: true,
            has_h264: false,
            has_h265: true,
            has_av1: false,
            has_aac: false,
            has_ac3: true,
            resolution: None,
            summary: String::new(),
        };
        assert_eq!(
            hevc_ac3.codec_description(),
            "Video: H.265/HEVC, Audio: AC-3"
        );

        let unknown = StreamProfile {
            has_video: false,
            has_audio: false,
            has_h264: false,
            has_h265: false,
            has_av1: false,
            has_aac: false,
            has_ac3: false,
            resolution: None,
            summary: String::new(),
        };
        assert_eq!(
            unknown.codec_description(),
            "Video: Unknown, Audio: Unknown"
        );
    }

    #[test]
    fn test_stream_profile_primary_codecs() {
        // H.265 takes priority over H.264
        let both = StreamProfile {
            has_video: true,
            has_audio: true,
            has_h264: true,
            has_h265: true,
            has_av1: false,
            has_aac: true,
            has_ac3: true,
            resolution: None,
            summary: String::new(),
        };
        assert_eq!(both.primary_video_codec(), Some("H.265/HEVC"));
        assert_eq!(both.primary_audio_codec(), Some("AAC"));
    }

    #[test]
    fn test_stream_profile_av1_priority() {
        let profile = StreamProfile {
            has_video: true,
            has_audio: true,
            has_h264: true,
            has_h265: true,
            has_av1: true,
            has_aac: true,
            has_ac3: false,
            resolution: None,
            summary: String::new(),
        };
        assert_eq!(profile.primary_video_codec(), Some("AV1"));
        assert_eq!(profile.codec_description(), "Video: AV1, Audio: AAC");
    }
}
