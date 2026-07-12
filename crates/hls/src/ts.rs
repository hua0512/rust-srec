use bytes::Bytes;
use m3u8_rs::MediaSegment;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tracing::warn;
use ts::descriptor::{
    TAG_ISO_639_LANGUAGE, TAG_REGISTRATION, parse_iso639_language, parse_registration_descriptor,
};
use ts::{PatRef, PesHeader, PmtRef, SpliceInfoSection, StreamType, TsPacketRef, TsParser};

use crate::profile::{SegmentType, StreamProfile, StreamProfileOptions};
use crate::resolution::{Resolution, StreamingResolutionDetector};

#[derive(Debug, Default)]
struct TsAnalysisCache {
    base: OnceLock<Result<Arc<TsAnalysis>, ts::TsError>>,
    with_resolution: OnceLock<Result<Arc<TsAnalysis>, ts::TsError>>,
    #[cfg(test)]
    parse_count: AtomicUsize,
}

#[derive(Debug)]
pub struct TsAnalysis {
    pub stream_info: TsStreamInfo,
    pub has_psi: bool,
    pub has_random_access: bool,
    pub resolution: Option<Resolution>,
}

impl TsAnalysis {
    pub fn stream_profile(&self) -> StreamProfile {
        let mut has_video = false;
        let mut has_audio = false;
        let mut has_h264 = false;
        let mut has_h265 = false;
        let mut has_aac = false;
        let mut has_ac3 = false;
        let mut video_count = 0usize;
        let mut audio_count = 0usize;

        for program in &self.stream_info.programs {
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

        let mut summary_parts = Vec::new();
        if video_count > 0 {
            summary_parts.push(format!("{video_count} video stream(s)"));
        }
        if audio_count > 0 {
            summary_parts.push(format!("{audio_count} audio stream(s)"));
        }

        StreamProfile {
            has_video,
            has_audio,
            has_h264,
            has_h265,
            has_av1: false,
            has_aac,
            has_ac3,
            resolution: self.resolution,
            summary: if summary_parts.is_empty() {
                "No recognized streams".to_string()
            } else {
                summary_parts.join(", ")
            },
        }
    }
}

struct TsAnalysisBuilder {
    transport_stream_id: u16,
    program_count: usize,
    programs: Vec<ProgramInfo>,
    scte35_events: Vec<SpliceInfoSection>,
    pcr_pids: HashSet<u16>,
    stream_pids: HashSet<u16>,
    first_pts_by_pid: HashMap<u16, u64>,
    first_pcr: Option<f64>,
    last_pcr: Option<f64>,
    has_psi: bool,
    has_random_access: bool,
    resolution_detector: Option<StreamingResolutionDetector>,
}

impl TsAnalysisBuilder {
    fn new(include_resolution: bool) -> Self {
        Self {
            transport_stream_id: 0,
            program_count: 0,
            programs: Vec::new(),
            scte35_events: Vec::new(),
            pcr_pids: HashSet::new(),
            stream_pids: HashSet::new(),
            first_pts_by_pid: HashMap::new(),
            first_pcr: None,
            last_pcr: None,
            has_psi: false,
            has_random_access: false,
            resolution_detector: include_resolution.then(StreamingResolutionDetector::new),
        }
    }

    fn finish(mut self) -> TsAnalysis {
        for program in &mut self.programs {
            for stream in program
                .video_streams
                .iter_mut()
                .chain(program.audio_streams.iter_mut())
                .chain(program.other_streams.iter_mut())
            {
                stream.first_pts = self.first_pts_by_pid.get(&stream.pid).copied();
            }
        }

        let resolution = self
            .resolution_detector
            .take()
            .and_then(StreamingResolutionDetector::finish);

        TsAnalysis {
            stream_info: TsStreamInfo {
                transport_stream_id: self.transport_stream_id,
                program_count: self.program_count,
                programs: self.programs,
                scte35_events: self.scte35_events,
                first_pcr: self.first_pcr,
                last_pcr: self.last_pcr,
            },
            has_psi: self.has_psi,
            has_random_access: self.has_random_access,
            resolution,
        }
    }
}

/// Transport Stream segment data
#[derive(Debug, Clone)]
pub struct TsSegmentData {
    pub segment: MediaSegment,
    data: Bytes,
    /// Whether to validate CRC-32/MPEG-2 on PAT/PMT sections
    validate_crc: bool,
    /// Continuity counter handling mode
    continuity_mode: ts::ContinuityMode,
    analysis_cache: Arc<TsAnalysisCache>,
}

impl TsSegmentData {
    pub fn new(segment: MediaSegment, data: Bytes) -> Self {
        Self {
            segment,
            data,
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Warn,
            analysis_cache: Arc::default(),
        }
    }

    fn invalidate_analysis(&mut self) {
        self.analysis_cache = Arc::default();
    }

    pub fn analysis(&self, options: StreamProfileOptions) -> Result<Arc<TsAnalysis>, ts::TsError> {
        if options.include_resolution {
            return self
                .analysis_cache
                .with_resolution
                .get_or_init(|| self.compute_analysis(true).map(Arc::new))
                .clone();
        }

        if let Some(analysis) = self.analysis_cache.with_resolution.get() {
            return analysis.clone();
        }

        self.analysis_cache
            .base
            .get_or_init(|| self.compute_analysis(false).map(Arc::new))
            .clone()
    }

    #[cfg(test)]
    fn parser_run_count(&self) -> usize {
        self.analysis_cache.parse_count.load(Ordering::Relaxed)
    }

    /// Enable or disable CRC-32/MPEG-2 validation on PAT/PMT sections.
    pub fn with_crc_validation(mut self, enable: bool) -> Self {
        self.validate_crc = enable;
        self.invalidate_analysis();
        self
    }

    /// Enable or disable continuity counter checking.
    pub fn with_continuity_check(mut self, enable: bool) -> Self {
        self.continuity_mode = if enable {
            ts::ContinuityMode::Warn
        } else {
            ts::ContinuityMode::Disabled
        };
        self.invalidate_analysis();
        self
    }

    /// Enable or disable strict continuity handling (fail on discontinuity).
    pub fn with_strict_continuity(mut self, enable: bool) -> Self {
        if enable {
            self.continuity_mode = ts::ContinuityMode::Strict;
        } else if self.continuity_mode == ts::ContinuityMode::Strict {
            self.continuity_mode = ts::ContinuityMode::Warn;
        }
        self.invalidate_analysis();
        self
    }

    /// Set continuity counter handling mode.
    pub fn with_continuity_mode(mut self, mode: ts::ContinuityMode) -> Self {
        self.continuity_mode = mode;
        self.invalidate_analysis();
        self
    }

    #[inline]
    pub fn segment_type(&self) -> SegmentType {
        SegmentType::Ts
    }

    #[inline]
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    #[inline]
    pub fn data_mut(&mut self) -> &mut Bytes {
        self.invalidate_analysis();
        &mut self.data
    }

    #[inline]
    pub fn media_segment(&self) -> Option<&MediaSegment> {
        Some(&self.segment)
    }

    fn make_parser(&self) -> TsParser {
        #[cfg(test)]
        self.analysis_cache
            .parse_count
            .fetch_add(1, Ordering::Relaxed);

        let mut parser = TsParser::new();
        if self.validate_crc {
            parser = parser.with_crc_validation(true);
        }
        parser = parser.with_continuity_mode(self.continuity_mode);
        parser
    }

    fn compute_analysis(&self, include_resolution: bool) -> Result<TsAnalysis, ts::TsError> {
        let mut parser = self.make_parser();
        let builder = RefCell::new(TsAnalysisBuilder::new(include_resolution));

        parser.parse_packets_with_scte35(
            self.data.clone(),
            |pat: PatRef| {
                let mut builder = builder.borrow_mut();
                builder.has_psi = true;
                builder.transport_stream_id = pat.transport_stream_id;
                builder.program_count = pat.program_count();
                Ok(())
            },
            |pmt: PmtRef| {
                let mut program_info = ProgramInfo {
                    program_number: pmt.program_number,
                    pcr_pid: pmt.pcr_pid,
                    video_streams: Vec::new(),
                    audio_streams: Vec::new(),
                    other_streams: Vec::new(),
                    scte35_pids: Vec::new(),
                };
                Self::fill_program_streams(&mut program_info, pmt);

                let mut builder = builder.borrow_mut();
                builder.has_psi = true;
                builder.pcr_pids.insert(program_info.pcr_pid);
                for stream in program_info
                    .video_streams
                    .iter()
                    .chain(program_info.audio_streams.iter())
                    .chain(program_info.other_streams.iter())
                {
                    builder.stream_pids.insert(stream.pid);
                }
                if let Some(detector) = &mut builder.resolution_detector {
                    for stream in &program_info.video_streams {
                        detector.add_video_stream(stream.pid, stream.stream_type);
                    }
                }
                builder.programs.push(program_info);
                Ok(())
            },
            Some(|packet: &TsPacketRef| {
                let mut builder = builder.borrow_mut();
                builder.has_random_access |= packet.has_random_access_indicator();
                if let Some(detector) = &mut builder.resolution_detector {
                    detector.push_packet(packet);
                }

                if builder.pcr_pids.contains(&packet.pid)
                    && let Some(adaptation_field) = packet.parse_adaptation_field()
                    && let Some(pcr) = adaptation_field.pcr()
                {
                    let seconds = pcr.as_seconds();
                    if builder.first_pcr.is_none() {
                        builder.first_pcr = Some(seconds);
                    }
                    builder.last_pcr = Some(seconds);
                }

                if packet.payload_unit_start_indicator
                    && builder.stream_pids.contains(&packet.pid)
                    && !builder.first_pts_by_pid.contains_key(&packet.pid)
                    && let Some(payload) = packet.payload()
                    && let Ok(pes) = PesHeader::parse(&payload)
                    && let Some(pts) = pes.pts
                {
                    builder.first_pts_by_pid.insert(packet.pid, pts);
                }

                Ok(())
            }),
            |scte35_ref| {
                builder
                    .borrow_mut()
                    .scte35_events
                    .push(scte35_ref.inner.clone());
                Ok(())
            },
        )?;

        self.report_continuity_warnings(&parser);
        Ok(builder.into_inner().finish())
    }

    fn report_continuity_warnings(&self, parser: &TsParser) {
        if self.continuity_mode == ts::ContinuityMode::Warn {
            let issues = parser.continuity_issue_count();
            if issues > 0 {
                warn!(
                    issues,
                    duplicate = parser.continuity_duplicate_count(),
                    discontinuity = parser.continuity_discontinuity_count(),
                    segment_uri = %self.segment.uri,
                    "TS continuity issues detected"
                );
            }
        }
    }

    fn fill_program_streams(program_info: &mut ProgramInfo, pmt: PmtRef) {
        for stream in pmt.streams().flatten() {
            let mut language = None;
            let mut is_scte35 = false;

            for desc in stream.descriptors() {
                if desc.tag == TAG_ISO_639_LANGUAGE {
                    let entries = parse_iso639_language(&desc.data);
                    if let Some(entry) = entries.first() {
                        language = Some(String::from_utf8_lossy(&entry.language_code).into_owned());
                    }
                }

                if desc.tag == TAG_REGISTRATION
                    && let Some(id) = parse_registration_descriptor(&desc.data)
                    && &id == b"CUEI"
                {
                    is_scte35 = true;
                }
            }

            if is_scte35 {
                program_info.scte35_pids.push(stream.elementary_pid);
            }

            let stream_entry = StreamEntry {
                pid: stream.elementary_pid,
                stream_type: stream.stream_type,
                language,
                first_pts: None,
            };

            if stream.stream_type.is_video() {
                program_info.video_streams.push(stream_entry);
            } else if stream.stream_type.is_audio() {
                program_info.audio_streams.push(stream_entry);
            } else {
                program_info.other_streams.push(stream_entry);
            }
        }
    }

    /// Parse TS segments returning stream information only (without packet capture).
    pub fn parse_stream_info_only(&self) -> Result<TsStreamInfo, ts::TsError> {
        self.analysis(StreamProfileOptions {
            include_resolution: false,
        })
        .map(|analysis| analysis.stream_info.clone())
    }

    /// Parse TS segments returning lightweight stream information
    pub fn parse_psi_tables(&self) -> Result<TsStreamInfo, ts::TsError> {
        self.parse_stream_info_only()
    }

    /// Parse TS segments returning both stream info and raw packets
    #[deprecated(note = "use TsSegmentData::analysis to avoid packet materialization")]
    pub fn parse_stream_and_packets(
        &self,
    ) -> Result<(TsStreamInfo, Vec<TsPacketRef>), ts::TsError> {
        let mut parser = self.make_parser();
        let mut stream_info = TsStreamInfo::default();
        let mut transport_stream_id = 0u16;
        let mut program_count = 0usize;
        let mut programs: Vec<ProgramInfo> = Vec::new();
        let mut scte35_events: Vec<SpliceInfoSection> = Vec::new();
        let mut packets = Vec::new();
        let pcr_pids = RefCell::new(HashSet::new());
        let stream_pids = RefCell::new(HashSet::new());
        let mut first_pts_by_pid = HashMap::new();

        parser.parse_packets_with_scte35(
            self.data.clone(),
            |pat: PatRef| {
                transport_stream_id = pat.transport_stream_id;
                program_count = pat.program_count();
                Ok(())
            },
            |pmt: PmtRef| {
                let mut program_info = ProgramInfo {
                    program_number: pmt.program_number,
                    pcr_pid: pmt.pcr_pid,
                    video_streams: Vec::new(),
                    audio_streams: Vec::new(),
                    other_streams: Vec::new(),
                    scte35_pids: Vec::new(),
                };
                Self::fill_program_streams(&mut program_info, pmt);

                pcr_pids.borrow_mut().insert(program_info.pcr_pid);

                for stream in program_info
                    .video_streams
                    .iter()
                    .chain(program_info.audio_streams.iter())
                    .chain(program_info.other_streams.iter())
                {
                    stream_pids.borrow_mut().insert(stream.pid);
                }

                programs.push(program_info);
                Ok(())
            },
            Some(|packet: &TsPacketRef| {
                if pcr_pids.borrow().contains(&packet.pid)
                    && let Some(af) = packet.parse_adaptation_field()
                    && let Some(pcr) = af.pcr()
                {
                    let seconds = pcr.as_seconds();
                    if stream_info.first_pcr.is_none() {
                        stream_info.first_pcr = Some(seconds);
                    }
                    stream_info.last_pcr = Some(seconds);
                }

                if packet.payload_unit_start_indicator
                    && stream_pids.borrow().contains(&packet.pid)
                    && !first_pts_by_pid.contains_key(&packet.pid)
                    && let Some(payload) = packet.payload()
                    && let Ok(pes) = PesHeader::parse(&payload)
                    && let Some(pts) = pes.pts
                {
                    first_pts_by_pid.insert(packet.pid, pts);
                }

                packets.push(packet.clone());
                Ok(())
            }),
            |scte35_ref| {
                scte35_events.push(scte35_ref.inner.clone());
                Ok(())
            },
        )?;

        stream_info.transport_stream_id = transport_stream_id;
        stream_info.program_count = program_count;
        stream_info.programs = programs;
        stream_info.scte35_events = scte35_events;

        self.report_continuity_warnings(&parser);

        // Fill in first_pts on stream entries
        for program in &mut stream_info.programs {
            for stream in program
                .video_streams
                .iter_mut()
                .chain(program.audio_streams.iter_mut())
                .chain(program.other_streams.iter_mut())
            {
                stream.first_pts = first_pts_by_pid.get(&stream.pid).copied();
            }
        }

        Ok((stream_info, packets))
    }

    /// Get video streams from this TS segment
    pub fn get_video_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables()?;
        let mut video_streams = Vec::new();

        for program in stream_info.programs {
            for stream in program.video_streams {
                video_streams.push((stream.pid, stream.stream_type));
            }
        }

        Ok(video_streams)
    }

    /// Get audio streams from this TS segment
    pub fn get_audio_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables()?;
        let mut audio_streams = Vec::new();

        for program in stream_info.programs {
            for stream in program.audio_streams {
                audio_streams.push((stream.pid, stream.stream_type));
            }
        }

        Ok(audio_streams)
    }

    /// Get all elementary streams from this TS segment
    pub fn get_all_streams(&self) -> Result<Vec<(u16, StreamType)>, ts::TsError> {
        let stream_info = self.parse_psi_tables()?;
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

    /// Check if this TS segment contains specific stream types
    pub fn contains_stream_type(&self, stream_type: StreamType) -> bool {
        match self.get_all_streams() {
            Ok(streams) => streams.iter().any(|(_, st)| *st == stream_type),
            Err(_) => false,
        }
    }

    /// Get stream summary
    pub fn get_stream_summary(&self) -> Option<String> {
        match self.parse_psi_tables() {
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

    /// Check if this segment contains PAT/PMT tables
    pub fn has_psi_tables(&self) -> bool {
        self.analysis(StreamProfileOptions {
            include_resolution: false,
        })
        .is_ok_and(|analysis| analysis.has_psi)
    }
}

/// Lightweight stream information extracted with zero-copy parsing
#[derive(Debug, Clone, Default)]
pub struct TsStreamInfo {
    pub transport_stream_id: u16,
    pub program_count: usize,
    pub programs: Vec<ProgramInfo>,
    /// SCTE-35 splice events found in this segment
    pub scte35_events: Vec<SpliceInfoSection>,
    /// First PCR value in seconds, if found
    pub first_pcr: Option<f64>,
    /// Last PCR value in seconds, if found
    pub last_pcr: Option<f64>,
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
    /// PIDs carrying SCTE-35 splice information (detected via "CUEI" registration descriptor)
    pub scte35_pids: Vec<u16>,
}

/// Lightweight stream entry with optional descriptor-derived metadata
#[derive(Debug, Clone)]
pub struct StreamEntry {
    pub pid: u16,
    pub stream_type: StreamType,
    /// ISO 639 language code (e.g., "eng", "fra"), extracted from descriptors
    pub language: Option<String>,
    /// First PTS value on this PID (90kHz ticks), if found
    pub first_pts: Option<u64>,
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
    fn test_parse_psi_tables_empty_data() {
        let segment = TsSegmentData::new(make_media_segment(), Bytes::new())
            .with_continuity_mode(ts::ContinuityMode::Disabled);
        // Empty data should return an error or empty result
        let result = segment.parse_psi_tables();
        // Empty bytes should parse without error but produce no programs
        // An error is also acceptable for empty data.
        if let Ok(info) = result {
            assert!(info.programs.is_empty());
        }
    }

    #[test]
    fn test_has_psi_tables_empty() {
        let segment = TsSegmentData::new(make_media_segment(), Bytes::new())
            .with_continuity_mode(ts::ContinuityMode::Disabled);
        assert!(!segment.has_psi_tables());
    }

    #[test]
    fn test_has_psi_tables_non_ts() {
        let segment = TsSegmentData::new(
            make_media_segment(),
            Bytes::from_static(b"this is not ts data"),
        )
        .with_continuity_mode(ts::ContinuityMode::Disabled);
        assert!(!segment.has_psi_tables());
    }

    #[test]
    fn test_get_video_streams_empty() {
        let segment = TsSegmentData::new(make_media_segment(), Bytes::new())
            .with_continuity_mode(ts::ContinuityMode::Disabled);
        // An error is also acceptable for empty data.
        if let Ok(streams) = segment.get_video_streams() {
            assert!(streams.is_empty());
        }
    }

    #[test]
    fn test_ts_stream_info_first_video_stream_empty() {
        let info = TsStreamInfo::default();
        assert!(info.first_video_stream().is_none());
    }

    #[test]
    fn test_ts_stream_info_first_video_stream() {
        let info = TsStreamInfo {
            transport_stream_id: 1,
            program_count: 1,
            programs: vec![ProgramInfo {
                program_number: 1,
                pcr_pid: 256,
                video_streams: vec![StreamEntry {
                    pid: 256,
                    stream_type: StreamType::H264,
                    language: None,
                    first_pts: None,
                }],
                audio_streams: vec![],
                other_streams: vec![],
                scte35_pids: vec![],
            }],
            ..Default::default()
        };
        let (pid, st) = info.first_video_stream().unwrap();
        assert_eq!(pid, 256);
        assert_eq!(st, StreamType::H264);
    }

    #[test]
    fn analysis_is_shared_across_segment_clones() {
        let segment =
            TsSegmentData::new(make_media_segment(), Bytes::new()).with_continuity_check(false);
        let cloned = segment.clone();

        let first = segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();
        let second = cloned
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(segment.parser_run_count(), 1);
    }

    #[test]
    fn public_queries_reuse_cached_analysis() {
        let segment =
            TsSegmentData::new(make_media_segment(), Bytes::new()).with_continuity_check(false);

        segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();
        assert!(!segment.has_psi_tables());
        assert!(
            segment
                .parse_stream_info_only()
                .unwrap()
                .programs
                .is_empty()
        );

        assert_eq!(segment.parser_run_count(), 1);
    }

    #[test]
    fn resolution_enabled_analysis_is_not_satisfied_by_base_cache() {
        let segment =
            TsSegmentData::new(make_media_segment(), Bytes::new()).with_continuity_check(false);

        segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();
        segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: true,
            })
            .unwrap();

        assert_eq!(segment.parser_run_count(), 2);
    }

    #[test]
    fn resolution_enabled_analysis_serves_later_base_queries() {
        let segment =
            TsSegmentData::new(make_media_segment(), Bytes::new()).with_continuity_check(false);

        segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: true,
            })
            .unwrap();
        assert!(!segment.has_psi_tables());
        assert!(
            segment
                .parse_stream_info_only()
                .unwrap()
                .programs
                .is_empty()
        );

        assert_eq!(segment.parser_run_count(), 1);
    }

    #[test]
    fn mutating_segment_data_invalidates_cached_analysis() {
        let mut segment =
            TsSegmentData::new(make_media_segment(), Bytes::new()).with_continuity_check(false);
        let first = segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();

        *segment.data_mut() = Bytes::new();
        let second = segment
            .analysis(crate::StreamProfileOptions {
                include_resolution: false,
            })
            .unwrap();

        assert!(!Arc::ptr_eq(&first, &second));
    }
}
