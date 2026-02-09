use bytes::Bytes;
use m3u8_rs::MediaSegment;

use crate::profile::SegmentType;

/// MP4 segment types (init or media)
#[derive(Debug, Clone)]
pub enum M4sData {
    InitSegment(M4sInitSegmentData),
    Segment(M4sSegmentData),
}

impl M4sData {
    #[inline]
    pub fn segment_type(&self) -> SegmentType {
        match self {
            M4sData::InitSegment(_) => SegmentType::M4sInit,
            M4sData::Segment(_) => SegmentType::M4sMedia,
        }
    }

    #[inline]
    pub fn data(&self) -> &Bytes {
        match self {
            M4sData::InitSegment(init) => &init.data,
            M4sData::Segment(seg) => &seg.data,
        }
    }

    #[inline]
    pub fn media_segment(&self) -> Option<&MediaSegment> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_media_segment() -> MediaSegment {
        MediaSegment {
            uri: "test.m4s".to_string(),
            duration: 6.0,
            ..Default::default()
        }
    }

    #[test]
    fn test_m4s_init_segment_type() {
        let init = M4sData::InitSegment(M4sInitSegmentData {
            segment: make_media_segment(),
            data: Bytes::from_static(b"moov"),
        });
        assert_eq!(init.segment_type(), SegmentType::M4sInit);
    }

    #[test]
    fn test_m4s_media_segment_type() {
        let media = M4sData::Segment(M4sSegmentData {
            segment: make_media_segment(),
            data: Bytes::from_static(b"moof"),
        });
        assert_eq!(media.segment_type(), SegmentType::M4sMedia);
    }

    #[test]
    fn test_m4s_data_access() {
        let data = Bytes::from_static(b"test_data");
        let init = M4sData::InitSegment(M4sInitSegmentData {
            segment: make_media_segment(),
            data: data.clone(),
        });
        assert_eq!(init.data(), &data);
        assert!(init.media_segment().is_some());
    }
}
