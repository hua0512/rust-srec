use std::fmt::Display;

use bytes::Bytes;
use m3u8_rs::MediaSegment;
use ts::{OwnedTsParser, Pat, PatRef, Pmt, PmtRef, StreamType, TsPacketRef, TsParser};

use crate::resolution::{self, ResolutionDetector};

pub type PsiParseResult = Result<(Option<Pat>, Vec<Pmt>), ts::TsError>;

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

impl TsSegmentData {
    /// Parse PAT and PMT tables from this TS segment
    pub fn parse_psi_tables(&self) -> PsiParseResult {
        let mut parser = OwnedTsParser::new();
        parser.parse_packets(self.data.as_ref())?;

        let pat = parser.pat().cloned();
        let pmts = parser.pmts().values().cloned().collect();

        Ok((pat, pmts))
    }

    /// Parse TS segments with zero-copy approach for minimal memory usage
    /// Returns stream information without copying descriptor data
    pub fn parse_psi_tables_zero_copy(&self) -> Result<TsStreamInfo, ts::TsError> {
        let (stream_info, _) = self.parse_stream_and_packets_zero_copy()?;
        Ok(stream_info)
    }

    /// Parse TS segments with zero-copy approach, returning both stream info and raw packets
    pub fn parse_stream_and_packets_zero_copy(
        &self,
    ) -> Result<(TsStreamInfo, Vec<TsPacketRef>), ts::TsError> {
        let mut parser = TsParser::new();
        let mut stream_info = TsStreamInfo::default();
        let mut packets = Vec::new();

        parser.parse_packets(
            self.data.clone(),
            |pat: PatRef| {
                stream_info.transport_stream_id = pat.transport_stream_id;
                stream_info.program_count = pat.program_count();
                Ok(())
            },
            |pmt: PmtRef| {
                let mut program_info = ProgramInfo {
                    program_number: pmt.program_number,
                    pcr_pid: pmt.pcr_pid,
                    video_streams: Vec::new(),
                    audio_streams: Vec::new(),
                    other_streams: Vec::new(),
                };

                for stream in pmt.streams().flatten() {
                    let stream_entry = StreamEntry {
                        pid: stream.elementary_pid,
                        stream_type: stream.stream_type,
                    };

                    if stream.stream_type.is_video() {
                        program_info.video_streams.push(stream_entry);
                    } else if stream.stream_type.is_audio() {
                        program_info.audio_streams.push(stream_entry);
                    } else {
                        program_info.other_streams.push(stream_entry);
                    }
                }

                stream_info.programs.push(program_info);
                Ok(())
            },
            Some(|packet: &TsPacketRef| {
                packets.push(packet.clone());
                Ok(())
            }),
        )?;

        Ok((stream_info, packets))
    }

    /// Get video streams from this TS segment using zero-copy parsing
    pub fn get_video_streams_zero_copy(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables_zero_copy()?;
        let mut video_streams = Vec::new();

        for program in stream_info.programs {
            for stream in program.video_streams {
                video_streams.push((stream.pid, stream.stream_type));
            }
        }

        Ok(video_streams)
    }

    /// Get audio streams from this TS segment using zero-copy parsing
    pub fn get_audio_streams_zero_copy(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables_zero_copy()?;
        let mut audio_streams = Vec::new();

        for program in stream_info.programs {
            for stream in program.audio_streams {
                audio_streams.push((stream.pid, stream.stream_type));
            }
        }

        Ok(audio_streams)
    }

    /// Get all elementary streams from this TS segment using zero-copy parsing
    pub fn get_all_streams_zero_copy(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables_zero_copy()?;
        let mut all_streams = Vec::new();

        for program in stream_info.programs {
            for stream in program
                .video_streams
                .into_iter()
                .chain(program.audio_streams)
                .chain(program.other_streams)
            {
                all_streams.push((stream.pid, stream.stream_type));
            }
        }

        Ok(all_streams)
    }

    /// Check if this TS segment contains specific stream types using zero-copy parsing
    pub fn contains_stream_type_zero_copy(&self, stream_type: StreamType) -> bool {
        match self.get_all_streams_zero_copy() {
            Ok(streams) => streams.iter().any(|(_, st)| *st == stream_type),
            Err(_) => false,
        }
    }

    /// Get stream summary using zero-copy parsing
    pub fn get_stream_summary_zero_copy(&self) -> Option<String> {
        match self.parse_psi_tables_zero_copy() {
            Ok(stream_info) => {
                let mut video_count = 0;
                let mut audio_count = 0;

                for program in &stream_info.programs {
                    video_count += program.video_streams.len();
                    audio_count += program.audio_streams.len();
                }

                let mut summary = Vec::new();
                if video_count > 0 {
                    summary.push(format!("{video_count} video stream(s)"));
                }
                if audio_count > 0 {
                    summary.push(format!("{audio_count} audio stream(s)"));
                }

                if summary.is_empty() {
                    Some("No recognized streams".to_string())
                } else {
                    Some(summary.join(", "))
                }
            }
            Err(_) => Some("Failed to parse streams".to_string()),
        }
    }

    /// Get video streams from this TS segment
    pub fn get_video_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let (_, pmts) = self.parse_psi_tables()?;
        let mut video_streams = Vec::new();

        for pmt in pmts {
            for stream in pmt.video_streams() {
                video_streams.push((stream.elementary_pid, stream.stream_type));
            }
        }

        Ok(video_streams)
    }

    /// Get audio streams from this TS segment
    pub fn get_audio_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let (_, pmts) = self.parse_psi_tables()?;
        let mut audio_streams = Vec::new();

        for pmt in pmts {
            for stream in pmt.audio_streams() {
                audio_streams.push((stream.elementary_pid, stream.stream_type));
            }
        }

        Ok(audio_streams)
    }

    /// Get all elementary streams from this TS segment
    pub fn get_all_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let (_, pmts) = self.parse_psi_tables()?;
        let mut all_streams = Vec::new();

        for pmt in pmts {
            for stream in &pmt.streams {
                all_streams.push((stream.elementary_pid, stream.stream_type));
            }
        }

        Ok(all_streams)
    }

    /// Check if this TS segment contains specific stream types
    pub fn contains_stream_type(&self, stream_type: StreamType) -> bool {
        match self.get_all_streams() {
            Ok(streams) => streams.iter().any(|(_, st)| *st == stream_type),
            Err(_) => false,
        }
    }

    /// Get program numbers from PAT
    pub fn get_program_numbers(&self) -> Result<Vec<u16>, ts::TsError> {
        let (pat, _) = self.parse_psi_tables()?;
        Ok(pat.map(|p| p.program_numbers()).unwrap_or_default())
    }

    /// Check if this segment contains PAT/PMT tables using zero-copy parser
    /// This is the preferred method for performance and memory efficiency
    pub fn has_psi_tables(&self) -> bool {
        use std::cell::Cell;
        use ts::TsParser;

        let mut parser = TsParser::new();
        let found_psi = Cell::new(false);

        let result = parser.parse_packets(
            self.data.clone(),
            |_pat| {
                found_psi.set(true);
                Ok(())
            },
            |_pmt| {
                found_psi.set(true);
                Ok(())
            },
            None::<fn(&ts::TsPacketRef) -> ts::Result<()>>,
        );

        result.is_ok() && found_psi.get()
    }

    /// Check if this segment contains PAT/PMT tables using traditional parser (for compatibility)
    pub fn has_psi_tables_traditional(&self) -> bool {
        match self.parse_psi_tables() {
            Ok((pat, pmts)) => pat.is_some() || !pmts.is_empty(),
            Err(_) => false,
        }
    }
}

/// Lightweight stream information extracted with zero-copy parsing
#[derive(Debug, Clone, Default)]
pub struct TsStreamInfo {
    pub transport_stream_id: u16,
    pub program_count: usize,
    pub programs: Vec<ProgramInfo>,
}

impl TsStreamInfo {
    /// Get the first video stream found, if any
    pub fn first_video_stream(&self) -> Option<(u16, StreamType)> {
        for program in &self.programs {
            if let Some(stream) = program.video_streams.first() {
                return Some((stream.pid, stream.stream_type));
            }
        }
        None
    }
}

/// Information about a program
#[derive(Debug, Clone)]
pub struct ProgramInfo {
    pub program_number: u16,
    pub pcr_pid: u16,
    pub video_streams: Vec<StreamEntry>,
    pub audio_streams: Vec<StreamEntry>,
    pub other_streams: Vec<StreamEntry>,
}

/// Lightweight stream entry
#[derive(Debug, Clone, Copy)]
pub struct StreamEntry {
    pub pid: u16,
    pub stream_type: StreamType,
}

/// Compact stream profile for quick segment analysis
#[derive(Debug, Clone)]
pub struct StreamProfile {
    pub has_video: bool,
    pub has_audio: bool,
    pub has_h264: bool,
    pub has_h265: bool,
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
        if self.has_h265 {
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

    /// Check if this segment is a discontinuity
    /// For TS: checks for discontinuity flag in the segment
    /// For MP4: checks for discontinuity flag in the segment
    /// For EndMarker: returns false
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
    /// Uses efficient zero-copy parsing for reliable detection
    #[inline]
    pub fn is_pmt_or_pat(&self) -> bool {
        if let HlsData::TsData(ts) = self {
            // Use zero-copy parser for efficient and reliable PAT/PMT detection
            let mut parser = TsParser::new();
            let found_psi = std::cell::Cell::new(false);

            // Parse packets and check for PAT/PMT using proper TS parsing
            let result = parser.parse_packets(
                ts.data.clone(),
                |_pat| {
                    found_psi.set(true);
                    Ok(())
                },
                |_pmt| {
                    found_psi.set(true);
                    Ok(())
                },
                None::<fn(&ts::TsPacketRef) -> ts::Result<()>>,
            );

            // Return true if we successfully found PAT or PMT tables
            result.is_ok() && found_psi.get()
        } else {
            false
        }
    }

    /// Get the tag type (same as segment type)
    #[inline]
    pub fn tag_type(&self) -> Option<SegmentType> {
        Some(self.segment_type())
    }

    /// Parse PAT and PMT tables from TS segments (only for TS data)
    pub fn parse_ts_psi_tables(&self) -> Option<PsiParseResult> {
        match self {
            HlsData::TsData(ts) => Some(ts.parse_psi_tables()),
            _ => None,
        }
    }

    /// Parse TS segments with zero-copy approach for minimal memory usage (only for TS data)
    pub fn parse_ts_psi_tables_zero_copy(&self) -> Option<Result<TsStreamInfo, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.parse_psi_tables_zero_copy()),
            _ => None,
        }
    }

    /// Get video streams from TS segments using zero-copy parsing (only for TS data)
    /// This is the preferred method for performance and memory efficiency
    pub fn get_ts_video_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        self.get_ts_video_streams_zero_copy()
    }

    /// Get video streams from TS segments using traditional parsing (for compatibility)
    pub fn get_ts_video_streams_traditional(
        &self,
    ) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_video_streams()),
            _ => None,
        }
    }

    /// Get video streams from TS segments using zero-copy parsing (only for TS data)
    pub fn get_ts_video_streams_zero_copy(
        &self,
    ) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_video_streams_zero_copy()),
            _ => None,
        }
    }

    /// Get audio streams from TS segments using zero-copy parsing (only for TS data)
    /// This is the preferred method for performance and memory efficiency
    pub fn get_ts_audio_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_audio_streams_zero_copy()),
            _ => None,
        }
    }

    /// Get audio streams from TS segments using traditional parsing (for compatibility)
    pub fn get_ts_audio_streams_traditional(
        &self,
    ) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_audio_streams()),
            _ => None,
        }
    }

    /// Get all elementary streams from TS segments using zero-copy parsing (only for TS data)
    /// This is the preferred method for performance and memory efficiency
    pub fn get_ts_all_streams(&self) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_all_streams_zero_copy()),
            _ => None,
        }
    }

    /// Get all elementary streams from TS segments using traditional parsing (for compatibility)
    pub fn get_ts_all_streams_traditional(
        &self,
    ) -> Option<Result<Vec<(u16, StreamType)>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_all_streams()),
            _ => None,
        }
    }

    /// Check if TS segment contains specific stream type using zero-copy parsing (only for TS data)
    /// This is the preferred method for performance and memory efficiency
    pub fn ts_contains_stream_type(&self, stream_type: StreamType) -> bool {
        self.ts_contains_stream_type_zero_copy(stream_type)
    }

    /// Check if TS segment contains specific stream type using traditional parsing (for compatibility)
    pub fn ts_contains_stream_type_traditional(&self, stream_type: StreamType) -> bool {
        match self {
            HlsData::TsData(ts) => ts.contains_stream_type(stream_type),
            _ => false,
        }
    }

    /// Get program numbers from TS segments (only for TS data)
    pub fn get_ts_program_numbers(&self) -> Option<Result<Vec<u16>, ts::TsError>> {
        match self {
            HlsData::TsData(ts) => Some(ts.get_program_numbers()),
            _ => None,
        }
    }

    /// Check if TS segment has PSI tables using zero-copy parser (only for TS data)
    pub fn ts_has_psi_tables(&self) -> bool {
        match self {
            HlsData::TsData(_) => self.is_pmt_or_pat(), // Use optimized zero-copy detection
            _ => false,
        }
    }

    /// Get a summary of streams in this HLS data using zero-copy parsing (works for TS segments)
    /// This is the preferred method for performance and memory efficiency
    pub fn get_stream_summary(&self) -> Option<String> {
        self.get_stream_summary_zero_copy()
    }

    /// Get a summary of streams using traditional parsing (for compatibility)
    pub fn get_stream_summary_traditional(&self) -> Option<String> {
        match self {
            HlsData::TsData(ts) => match ts.get_all_streams() {
                Ok(streams) => {
                    let mut summary = Vec::new();
                    let video_count = streams.iter().filter(|(_, st)| st.is_video()).count();
                    let audio_count = streams.iter().filter(|(_, st)| st.is_audio()).count();

                    if video_count > 0 {
                        summary.push(format!("{video_count} video stream(s)"));
                    }
                    if audio_count > 0 {
                        summary.push(format!("{audio_count} audio stream(s)"));
                    }

                    if summary.is_empty() {
                        Some("No recognized streams".to_string())
                    } else {
                        Some(summary.join(", "))
                    }
                }
                Err(_) => Some("Failed to parse streams".to_string()),
            },
            _ => None,
        }
    }

    /// Get a summary of streams using zero-copy parsing (works for TS segments)
    pub fn get_stream_summary_zero_copy(&self) -> Option<String> {
        match self {
            HlsData::TsData(ts) => ts.get_stream_summary_zero_copy(),
            _ => None,
        }
    }

    /// Check if TS segment contains specific stream type using zero-copy parsing (only for TS data)
    pub fn ts_contains_stream_type_zero_copy(&self, stream_type: StreamType) -> bool {
        match self {
            HlsData::TsData(ts) => ts.contains_stream_type_zero_copy(stream_type),
            _ => false,
        }
    }

    /// Quick check if this TS segment contains video streams (zero-copy)
    pub fn has_video_streams(&self) -> bool {
        match self.get_ts_video_streams() {
            Some(Ok(streams)) => !streams.is_empty(),
            _ => false,
        }
    }

    /// Quick check if this TS segment contains audio streams (zero-copy)
    pub fn has_audio_streams(&self) -> bool {
        match self.get_ts_audio_streams() {
            Some(Ok(streams)) => !streams.is_empty(),
            _ => false,
        }
    }

    /// Quick check if this TS segment contains H.264 video (zero-copy)
    pub fn has_h264_video(&self) -> bool {
        self.ts_contains_stream_type(StreamType::H264)
    }

    /// Quick check if this TS segment contains H.265 video (zero-copy)
    pub fn has_h265_video(&self) -> bool {
        self.ts_contains_stream_type(StreamType::H265)
    }

    /// Quick check if this TS segment contains AAC audio (zero-copy)
    pub fn has_aac_audio(&self) -> bool {
        self.ts_contains_stream_type(StreamType::AdtsAac)
            || self.ts_contains_stream_type(StreamType::LatmAac)
    }

    /// Quick check if this TS segment contains AC-3 audio (zero-copy)
    pub fn has_ac3_audio(&self) -> bool {
        self.ts_contains_stream_type(StreamType::Ac3)
            || self.ts_contains_stream_type(StreamType::EAc3)
    }

    /// Get a compact stream profile for this segment (zero-copy)
    pub fn get_stream_profile(&self) -> Option<StreamProfile> {
        let ts_data = match self {
            HlsData::TsData(ts_data) => ts_data,
            _ => return None,
        };

        let (stream_info, packets) = match ts_data.parse_stream_and_packets_zero_copy() {
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
