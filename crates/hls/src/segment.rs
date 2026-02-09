use bytes::Bytes;
use m3u8_rs::MediaSegment;
use ts::StreamType;

use crate::mp4::{M4sData, M4sInitSegmentData, M4sSegmentData};
use crate::profile::{SegmentType, StreamProfile};
use crate::resolution::ResolutionDetector;
use crate::ts::{TsSegmentData, TsStreamInfo};

/// Main HLS data type representing various segment types
#[derive(Debug, Clone)]
pub enum HlsData {
    TsData(TsSegmentData),
    M4sData(M4sData),
    EndMarker,
}

impl HlsData {
    /// Create a new TS segment
    #[inline]
    pub fn ts(segment: MediaSegment, data: Bytes) -> Self {
        HlsData::TsData(TsSegmentData {
            segment,
            data,
            validate_crc: false,
            check_continuity: false,
        })
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
    /// For TS: checks for Adaptation Field random access indicator across all packets
    /// For MP4: checks for moof box at the beginning for media segments
    #[inline]
    pub fn has_keyframe(&self) -> bool {
        match self {
            HlsData::TsData(ts) => {
                let bytes = ts.data.as_ref();
                // Iterate 188-byte-aligned packets looking for any with RAI set
                let mut offset = 0;
                while offset + 188 <= bytes.len() {
                    if let Ok(packet) = ts::TsPacketRef::parse(ts.data.slice(offset..offset + 188))
                        && packet.has_random_access_indicator()
                    {
                        return true;
                    }
                    offset += 188;
                }
                false
            }
            HlsData::M4sData(M4sData::Segment(seg)) => {
                let bytes = seg.data.as_ref();
                if bytes.len() >= 8 {
                    return &bytes[4..8] == b"moof";
                }
                false
            }
            _ => false,
        }
    }

    /// Check if this segment is a discontinuity
    pub fn is_discontinuity(&self) -> bool {
        match self {
            HlsData::TsData(ts) => ts.segment.discontinuity,
            HlsData::M4sData(m4s) => m4s.media_segment().unwrap().discontinuity,
            HlsData::EndMarker => false,
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
            ts.has_psi_tables()
        } else {
            false
        }
    }

    /// Check if TS segment has PSI tables (only for TS data)
    pub fn ts_has_psi_tables(&self) -> bool {
        self.is_pmt_or_pat()
    }

    /// Parse TS PSI tables (only for TS data)
    pub fn parse_ts_psi_tables(&self) -> Option<Result<TsStreamInfo, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.parse_psi_tables()),
            _ => None,
        }
    }

    /// Get video streams from TS segments (only for TS data)
    pub fn get_ts_video_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_video_streams()),
            _ => None,
        }
    }

    /// Get audio streams from TS segments (only for TS data)
    pub fn get_ts_audio_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_audio_streams()),
            _ => None,
        }
    }

    /// Get all elementary streams from TS segments (only for TS data)
    pub fn get_ts_all_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_all_streams()),
            _ => None,
        }
    }

    /// Check if TS segment contains specific stream type (only for TS data)
    pub fn ts_contains_stream_type(&self, stream_type: StreamType) -> bool {
        match self {
            HlsData::TsData(ts) => ts.contains_stream_type(stream_type),
            _ => false,
        }
    }

    /// Get a summary of streams (works for TS segments)
    pub fn get_stream_summary(&self) -> Option<String> {
        match self {
            HlsData::TsData(ts) => ts.get_stream_summary(),
            _ => None,
        }
    }

    /// Quick check if this TS segment contains video streams
    pub fn has_video_streams(&self) -> bool {
        match self.get_ts_video_streams() {
            Some(Ok(streams)) => !streams.is_empty(),
            _ => false,
        }
    }

    /// Quick check if this TS segment contains audio streams
    pub fn has_audio_streams(&self) -> bool {
        match self.get_ts_audio_streams() {
            Some(Ok(streams)) => !streams.is_empty(),
            _ => false,
        }
    }

    /// Quick check if this TS segment contains H.264 video
    pub fn has_h264_video(&self) -> bool {
        self.ts_contains_stream_type(StreamType::H264)
    }

    /// Quick check if this TS segment contains H.265 video
    pub fn has_h265_video(&self) -> bool {
        self.ts_contains_stream_type(StreamType::H265)
    }

    /// Quick check if this TS segment contains AAC audio
    pub fn has_aac_audio(&self) -> bool {
        self.ts_contains_stream_type(StreamType::AdtsAac)
            || self.ts_contains_stream_type(StreamType::LatmAac)
    }

    /// Quick check if this TS segment contains AC-3 audio
    pub fn has_ac3_audio(&self) -> bool {
        self.ts_contains_stream_type(StreamType::Ac3)
            || self.ts_contains_stream_type(StreamType::EAc3)
    }

    /// Get a compact stream profile for this segment
    pub fn get_stream_profile(&self) -> Option<StreamProfile> {
        let ts_data = match self {
            HlsData::TsData(ts_data) => ts_data,
            _ => return None,
        };

        let (stream_info, packets) = match ts_data.parse_stream_and_packets() {
            Ok(data) => data,
            Err(_) => return None,
        };

        let mut has_video = false;
        let mut has_audio = false;
        let mut has_h264 = false;
        let mut has_h265 = false;
        let mut has_aac = false;
        let mut has_ac3 = false;
        let mut video_count = 0;
        let mut audio_count = 0;

        for program in &stream_info.programs {
            if !program.video_streams.is_empty() {
                has_video = true;
                video_count += program.video_streams.len();
                for stream in &program.video_streams {
                    match stream.stream_type {
                        StreamType::H264 => has_h264 = true,
                        StreamType::H265 => has_h265 = true,
                        _ => {}
                    }
                }
            }
            if !program.audio_streams.is_empty() {
                has_audio = true;
                audio_count += program.audio_streams.len();
                for stream in &program.audio_streams {
                    match stream.stream_type {
                        StreamType::AdtsAac | StreamType::LatmAac => has_aac = true,
                        StreamType::Ac3 | StreamType::EAc3 => has_ac3 = true,
                        _ => {}
                    }
                }
            }
        }

        let resolution = if has_video {
            let video_streams: Vec<_> = stream_info
                .programs
                .iter()
                .flat_map(|p| &p.video_streams)
                .map(|s| (s.pid, s.stream_type))
                .collect();
            ResolutionDetector::extract_from_ts_packets(packets.iter(), &video_streams)
        } else {
            None
        };

        let mut summary_parts = Vec::new();
        if video_count > 0 {
            summary_parts.push(format!("{video_count} video stream(s)"));
        }
        if audio_count > 0 {
            summary_parts.push(format!("{audio_count} audio stream(s)"));
        }

        let summary = if summary_parts.is_empty() {
            "No recognized streams".to_string()
        } else {
            summary_parts.join(", ")
        };

        Some(StreamProfile {
            has_video,
            has_audio,
            has_h264,
            has_h265,
            has_aac,
            has_ac3,
            resolution,
            summary,
        })
    }
}

impl AsRef<[u8]> for HlsData {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        match self {
            HlsData::TsData(ts) => ts.data.as_ref(),
            HlsData::M4sData(m4s) => m4s.data().as_ref(),
            HlsData::EndMarker => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_media_segment() -> MediaSegment {
        MediaSegment {
            uri: "test.ts".to_string(),
            duration: 6.0,
            ..Default::default()
        }
    }

    #[test]
    fn test_hlsdata_constructors() {
        let ts = HlsData::ts(make_media_segment(), Bytes::from_static(b"ts_data"));
        assert_eq!(ts.segment_type(), SegmentType::Ts);

        let mp4_init = HlsData::mp4_init(make_media_segment(), Bytes::from_static(b"moov"));
        assert_eq!(mp4_init.segment_type(), SegmentType::M4sInit);

        let mp4_seg = HlsData::mp4_segment(make_media_segment(), Bytes::from_static(b"moof"));
        assert_eq!(mp4_seg.segment_type(), SegmentType::M4sMedia);

        let end = HlsData::end_marker();
        assert_eq!(end.segment_type(), SegmentType::EndMarker);
    }

    #[test]
    fn test_hlsdata_size() {
        let ts = HlsData::ts(make_media_segment(), Bytes::from_static(b"hello"));
        assert_eq!(ts.size(), 5);

        let end = HlsData::end_marker();
        assert_eq!(end.size(), 0);
    }

    #[test]
    fn test_hlsdata_is_checks() {
        let ts = HlsData::ts(make_media_segment(), Bytes::new());
        assert!(ts.is_ts());
        assert!(!ts.is_mp4());
        assert!(!ts.is_end_marker());

        let mp4_init = HlsData::mp4_init(make_media_segment(), Bytes::new());
        assert!(mp4_init.is_mp4());
        assert!(mp4_init.is_mp4_init());
        assert!(!mp4_init.is_mp4_media());
        assert!(mp4_init.is_init_segment());

        let mp4_seg = HlsData::mp4_segment(make_media_segment(), Bytes::new());
        assert!(mp4_seg.is_mp4());
        assert!(!mp4_seg.is_mp4_init());
        assert!(mp4_seg.is_mp4_media());
        assert!(!mp4_seg.is_init_segment());

        let end = HlsData::end_marker();
        assert!(end.is_end_marker());
        assert!(!end.is_ts());
        assert!(!end.is_mp4());
    }

    #[test]
    fn test_hlsdata_data_access() {
        let data = Bytes::from_static(b"test");
        let ts = HlsData::ts(make_media_segment(), data.clone());
        assert_eq!(ts.data().unwrap(), &data);
        assert!(ts.media_segment().is_some());

        let end = HlsData::end_marker();
        assert!(end.data().is_none());
        assert!(end.media_segment().is_none());
    }

    #[test]
    fn test_hlsdata_as_ref() {
        let ts = HlsData::ts(make_media_segment(), Bytes::from_static(b"payload"));
        assert_eq!(ts.as_ref(), b"payload");

        let end = HlsData::end_marker();
        assert_eq!(end.as_ref(), b"");
    }

    #[test]
    fn test_hlsdata_is_discontinuity() {
        let mut seg = make_media_segment();
        seg.discontinuity = true;
        let ts = HlsData::ts(seg, Bytes::new());
        assert!(ts.is_discontinuity());

        let ts_no_disc = HlsData::ts(make_media_segment(), Bytes::new());
        assert!(!ts_no_disc.is_discontinuity());

        let end = HlsData::end_marker();
        assert!(!end.is_discontinuity());
    }
}
