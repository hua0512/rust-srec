use bytes::Bytes;
use m3u8_rs::MediaSegment;

#[derive(Debug, Clone)]
pub struct TsSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

pub enum M4sData {
    InitSegment(M4sInitSegmentData),
    Segment(M4sSegmentData),
}

#[derive(Debug, Clone)]
pub struct M4sInitSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct M4sSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
}

pub enum HlsData {
    TsData(TsSegmentData),
    M4sData(M4sData),
    EndPlaylist(),
}
