use bytes::Bytes;
use m3u8_rs::MediaSegment;

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

/// Common trait for segment data
pub trait SegmentData {
    /// Get the segment type
    fn segment_type(&self) -> SegmentType;

    /// Get the raw data bytes
    fn data(&self) -> &Bytes;

    /// Get the media segment information if available
    fn media_segment(&self) -> Option<&MediaSegment>;
}

/// Transport Stream segment data
#[derive(Debug, Clone)]
pub struct TsSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

impl SegmentData for TsSegmentData {
    #[inline]
    fn segment_type(&self) -> SegmentType {
        SegmentType::Ts
    }

    #[inline]
    fn data(&self) -> &Bytes {
        &self.data
    }

    #[inline]
    fn media_segment(&self) -> Option<&MediaSegment> {
        Some(&self.segment)
    }
}

/// MP4 segment types (init or media)
#[derive(Debug, Clone)]
pub enum M4sData {
    InitSegment(M4sInitSegmentData),
    Segment(M4sSegmentData),
}

impl SegmentData for M4sData {
    #[inline]
    fn segment_type(&self) -> SegmentType {
        match self {
            M4sData::InitSegment(_) => SegmentType::M4sInit,
            M4sData::Segment(_) => SegmentType::M4sMedia,
        }
    }

    #[inline]
    fn data(&self) -> &Bytes {
        match self {
            M4sData::InitSegment(init) => &init.data,
            M4sData::Segment(seg) => &seg.data,
        }
    }

    #[inline]
    fn media_segment(&self) -> Option<&MediaSegment> {
        match self {
            M4sData::InitSegment(init) => Some(&init.segment),
            M4sData::Segment(seg) => Some(&seg.segment),
        }
    }
}

/// MP4 initialization segment data
#[derive(Debug, Clone)]
pub struct M4sInitSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

/// MP4 media segment data
#[derive(Debug, Clone)]
pub struct M4sSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

/// Main HLS data type representing various segment types
#[derive(Debug, Clone)] // Added Clone
pub enum HlsData {
    TsData(TsSegmentData),
    M4sData(M4sData),
    EndMarker,
}

impl HlsData {
    /// Create a new TS segment
    #[inline]
    pub fn ts(segment: MediaSegment, data: Bytes) -> Self {
        HlsData::TsData(TsSegmentData { segment, data })
    }

    /// Create a new MP4 initialization segment
    #[inline]
    pub fn mp4_init(segment: MediaSegment, data: Bytes) -> Self {
        HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData { segment, data }))
    }

    /// Create a new MP4 media segment
    #[inline]
    pub fn mp4_segment(segment: MediaSegment, data: Bytes) -> Self {
        HlsData::M4sData(M4sData::Segment(M4sSegmentData { segment, data }))
    }

    /// Create an end of playlist marker
    #[inline]
    pub fn end_marker() -> Self {
        HlsData::EndMarker
    }

    /// Get the segment type
    #[inline]
    pub fn segment_type(&self) -> SegmentType {
        match self {
            HlsData::TsData(_) => SegmentType::Ts,
            HlsData::M4sData(m4s) => m4s.segment_type(),
            HlsData::EndMarker => SegmentType::EndMarker,
        }
    }

    /// Get the segment data if available
    #[inline]
    pub fn data(&self) -> Option<&Bytes> {
        match self {
            HlsData::TsData(ts) => Some(&ts.data),
            HlsData::M4sData(m4s) => Some(m4s.data()),
            HlsData::EndMarker => None,
        }
    }

    /// Get the segment data as mutable reference if available
    #[inline]
    pub fn data_mut(&mut self) -> Option<&mut Bytes> {
        match self {
            HlsData::TsData(ts) => Some(&mut ts.data),
            HlsData::M4sData(M4sData::InitSegment(init)) => Some(&mut init.data),
            HlsData::M4sData(M4sData::Segment(seg)) => Some(&mut seg.data),
            HlsData::EndMarker => None,
        }
    }

    /// Get the media segment information if available
    #[inline]
    pub fn media_segment(&self) -> Option<&MediaSegment> {
        match self {
            HlsData::TsData(ts) => Some(&ts.segment),
            HlsData::M4sData(m4s) => m4s.media_segment(),
            HlsData::EndMarker => None,
        }
    }

    /// Check if this is a TS segment
    #[inline]
    pub fn is_ts(&self) -> bool {
        matches!(self, HlsData::TsData(_))
    }

    /// Check if this is an MP4 segment (either init or media)
    #[inline]
    pub fn is_mp4(&self) -> bool {
        matches!(self, HlsData::M4sData(_))
    }

    /// Check if this is an MP4 initialization segment
    #[inline]
    pub fn is_mp4_init(&self) -> bool {
        matches!(self, HlsData::M4sData(M4sData::InitSegment(_)))
    }

    /// Check if this is an MP4 media segment
    #[inline]
    pub fn is_mp4_media(&self) -> bool {
        matches!(self, HlsData::M4sData(M4sData::Segment(_)))
    }

    /// Check if this is an end of playlist marker
    #[inline]
    pub fn is_end_marker(&self) -> bool {
        matches!(self, HlsData::EndMarker)
    }

    /// Get the size of the segment data in bytes, or 0 for end markers
    #[inline]
    pub fn size(&self) -> usize {
        match self {
            HlsData::TsData(ts) => ts.data.len(),
            HlsData::M4sData(m4s) => m4s.data().len(),
            HlsData::EndMarker => 0,
        }
    }
    /// Check if this segment contains a keyframe
    /// For TS: checks for Adaptation Field random access indicator
    /// For MP4: checks for moof box at the beginning for media segments
    #[inline]
    pub fn has_keyframe(&self) -> bool {
        match self {
            // For TS data, check for IDR frame with random access indicator
            HlsData::TsData(ts) => {
                let bytes = ts.data.as_ref();
                if bytes.len() < 6 {
                    return false;
                }

                // Check for adaptation field with random access indicator
                if bytes[0] == 0x47 && (bytes[3] & 0x20) != 0 {
                    // Check if adaptation field exists
                    let adaptation_len = bytes[4] as usize;
                    if adaptation_len > 0 && bytes.len() > 5 {
                        // Random access indicator is bit 6 (0x40)
                        return (bytes[5] & 0x40) != 0;
                    }
                }
                false
            }
            // For M4S, check for moof box which typically starts a fragment with keyframe
            HlsData::M4sData(M4sData::Segment(seg)) => {
                let bytes = seg.data.as_ref();
                if bytes.len() >= 8 {
                    return &bytes[4..8] == b"moof";
                }
                false
            }
            // Init segments don't have keyframes
            _ => false,
        }
    }

    /// Check if this segment indicates the start of a new segment
    /// For TS: typically a keyframe with PAT/PMT tables following
    /// For MP4: an init segment or a media segment starting with moof box
    #[inline]
    pub fn is_segment_start(&self) -> bool {
        match self {
            HlsData::TsData(_) => self.has_keyframe(),
            HlsData::M4sData(M4sData::InitSegment(_)) => true,
            HlsData::M4sData(M4sData::Segment(seg)) => {
                let bytes = seg.data.as_ref();
                if bytes.len() >= 8 {
                    return &bytes[4..8] == b"moof";
                }
                false
            }
            HlsData::EndMarker => false,
        }
    }

    /// Check if this is an initialization segment
    #[inline]
    pub fn is_init_segment(&self) -> bool {
        matches!(self, HlsData::M4sData(M4sData::InitSegment(_)))
    }

    /// Check if this segment contains a PAT or PMT table (TS only)
    #[inline]
    pub fn is_pmt_or_pat(&self) -> bool {
        if let HlsData::TsData(ts) = self {
            let bytes = ts.data.as_ref();
            if bytes.len() < 4 {
                return false;
            }

            // Check if this is a TS packet with sync byte
            if bytes[0] != 0x47 {
                return false;
            }

            // Extract PID (Program ID)
            let pid = ((bytes[1] as u16 & 0x1F) << 8) | bytes[2] as u16;

            // PAT has PID 0x0000, PMT typically has PID 0x0020-0x1FFE
            return pid == 0 || (pid >= 0x0020 && pid <= 0x1FFE);
        }
        false
    }

    /// Get the tag type (same as segment type)
    #[inline]
    pub fn tag_type(&self) -> Option<SegmentType> {
        Some(self.segment_type())
    }
}

// Implementation to allow using HlsData with AsRef<[u8]>
impl AsRef<[u8]> for HlsData {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match self {
            HlsData::TsData(ts) => ts.data.as_ref(),
            HlsData::M4sData(m4s) => m4s.data().as_ref(),
            HlsData::EndMarker => &[], // Empty slice for end marker
        }
    }
}

// Add additional segment formats for the future
#[derive(Debug, Clone)]
pub struct WebVttSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}
