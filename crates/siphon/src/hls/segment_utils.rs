use bytes::Bytes;
use hls::segment::{HlsData, M4sData, M4sInitSegmentData, M4sSegmentData, TsSegmentData};
use m3u8_rs::MediaSegment;
use url::Url;

/// Determine if a segment URL represents an M4S segment
pub fn is_m4s_segment(url: &Url) -> bool {
    let path = url.path().to_lowercase();
    let query = url.query().unwrap_or("").to_lowercase();

    // debug!("is_m4s_segment: {}", path);
    path.ends_with(".m4s")
        || path.ends_with(".mp4")
        || path.ends_with(".cmfv")
        || query.contains("format=mp4")
        || query.contains("fmt=mp4")
}

/// Create the appropriate HlsData type based on segment URL and content
pub fn create_hls_data(segment: MediaSegment, data: Bytes, url: &Url, is_init: bool) -> HlsData {
    if is_m4s_segment(url) {
        if is_init {
            HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData { segment, data }))
        } else {
            HlsData::M4sData(M4sData::Segment(M4sSegmentData { segment, data }))
        }
    } else {
        // Default to TS segment
        HlsData::TsData(TsSegmentData { segment, data })
    }
}
