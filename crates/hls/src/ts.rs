use bytes::Bytes;
use m3u8_rs::MediaSegment;
use std::cell::{Cell, RefCell};
use tracing::warn;
use ts::descriptor::{
    TAG_ISO_639_LANGUAGE, TAG_REGISTRATION, parse_iso639_language, parse_registration_descriptor,
};
use ts::{PatRef, PesHeader, PmtRef, SpliceInfoSection, StreamType, TsPacketRef, TsParser};

use crate::profile::SegmentType;

/// Transport Stream segment data
#[derive(Debug, Clone)]
pub struct TsSegmentData {
    pub segment: MediaSegment,
    pub data: Bytes,
    /// Whether to validate CRC-32/MPEG-2 on PAT/PMT sections
    pub validate_crc: bool,
    /// Continuity counter handling mode
    pub continuity_mode: ts::ContinuityMode,
}

impl TsSegmentData {
    const PID_SPACE: usize = 8192;

    /// Enable or disable CRC-32/MPEG-2 validation on PAT/PMT sections.
    pub fn with_crc_validation(mut self, enable: bool) -> Self {
        self.validate_crc = enable;
        self
    }

    /// Enable or disable continuity counter checking.
    pub fn with_continuity_check(mut self, enable: bool) -> Self {
        self.continuity_mode = if enable {
            ts::ContinuityMode::Warn
        } else {
            ts::ContinuityMode::Disabled
        };
        self
    }

    /// Enable or disable strict continuity handling (fail on discontinuity).
    pub fn with_strict_continuity(mut self, enable: bool) -> Self {
        if enable {
            self.continuity_mode = ts::ContinuityMode::Strict;
        } else if self.continuity_mode == ts::ContinuityMode::Strict {
            self.continuity_mode = ts::ContinuityMode::Warn;
        }
        self
    }

    /// Set continuity counter handling mode.
    pub fn with_continuity_mode(mut self, mode: ts::ContinuityMode) -> Self {
        self.continuity_mode = mode;
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
    pub fn media_segment(&self) -> Option<&MediaSegment> {
        Some(&self.segment)
    }

    fn make_parser(&self) -> TsParser {
        let mut parser = TsParser::new();
        if self.validate_crc {
            parser = parser.with_crc_validation(true);
        }
        parser = parser.with_continuity_mode(self.continuity_mode);
        parser
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
        let mut parser = self.make_parser();
        let transport_stream_id = Cell::new(0u16);
        let program_count = Cell::new(0usize);
        let found_pat = Cell::new(false);
        let completed = Cell::new(false);
        let programs = RefCell::new(Vec::<ProgramInfo>::new());
        let scte35_events = RefCell::new(Vec::<SpliceInfoSection>::new());

        let parse_result = parser.parse_packets_with_scte35(
            self.data.clone(),
            |pat: PatRef| {
                if completed.get() {
                    return Ok(());
                }

                transport_stream_id.set(pat.transport_stream_id);
                program_count.set(pat.program_count());
                found_pat.set(true);

                if program_count.get() == 0 {
                    completed.set(true);
                }

                if program_count.get() > 0 && programs.borrow().len() >= program_count.get() {
                    completed.set(true);
                }

                Ok(())
            },
            |pmt: PmtRef| {
                if completed.get() {
                    return Ok(());
                }

                let mut program_info = ProgramInfo {
                    program_number: pmt.program_number,
                    pcr_pid: pmt.pcr_pid,
                    video_streams: Vec::new(),
                    audio_streams: Vec::new(),
                    other_streams: Vec::new(),
                    scte35_pids: Vec::new(),
                };
                Self::fill_program_streams(&mut program_info, pmt);
                programs.borrow_mut().push(program_info);

                if found_pat.get() && programs.borrow().len() >= program_count.get() {
                    completed.set(true);
                }

                Ok(())
            },
            None::<fn(&TsPacketRef) -> ts::Result<()>>,
            |scte35_ref| {
                scte35_events.borrow_mut().push(scte35_ref.inner.clone());
                Ok(())
            },
        );

        parse_result?;

        let stream_info = TsStreamInfo {
            transport_stream_id: transport_stream_id.get(),
            program_count: program_count.get(),
            programs: programs.into_inner(),
            scte35_events: scte35_events.into_inner(),
            first_pcr: None,
            last_pcr: None,
        };

        self.report_continuity_warnings(&parser);
        Ok(stream_info)
    }

    /// Parse TS segments returning lightweight stream information
    pub fn parse_psi_tables(&self) -> Result<TsStreamInfo, ts::TsError> {
        self.parse_stream_info_only()
    }

    /// Parse TS segments returning both stream info and raw packets
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
        let is_pcr_pid = RefCell::new([false; Self::PID_SPACE]);
        let is_stream_pid = RefCell::new([false; Self::PID_SPACE]);
        let mut first_pts_by_pid = [None; Self::PID_SPACE];

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

                let pcr_idx = program_info.pcr_pid as usize;
                if pcr_idx < Self::PID_SPACE {
                    is_pcr_pid.borrow_mut()[pcr_idx] = true;
                }

                for stream in program_info
                    .video_streams
                    .iter()
                    .chain(program_info.audio_streams.iter())
                    .chain(program_info.other_streams.iter())
                {
                    let idx = stream.pid as usize;
                    if idx < Self::PID_SPACE {
                        is_stream_pid.borrow_mut()[idx] = true;
                    }
                }

                programs.push(program_info);
                Ok(())
            },
            Some(|packet: &TsPacketRef| {
                let pid_idx = packet.pid as usize;

                if pid_idx < Self::PID_SPACE
                    && is_pcr_pid.borrow()[pid_idx]
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
                    && pid_idx < Self::PID_SPACE
                    && is_stream_pid.borrow()[pid_idx]
                    && first_pts_by_pid[pid_idx].is_none()
                    && let Some(payload) = packet.payload()
                    && let Ok(pes) = PesHeader::parse(&payload)
                    && let Some(pts) = pes.pts
                {
                    first_pts_by_pid[pid_idx] = Some(pts);
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
                let idx = stream.pid as usize;
                stream.first_pts = if idx < Self::PID_SPACE {
                    first_pts_by_pid[idx]
                } else {
                    None
                };
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
        let mut parser = self.make_parser();
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

        self.report_continuity_warnings(&parser);

        match result {
            Ok(()) => found_psi.get(),
            Err(_) => found_psi.get(),
        }
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
        let segment = TsSegmentData {
            segment: make_media_segment(),
            data: Bytes::new(),
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Disabled,
        };
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
        let segment = TsSegmentData {
            segment: make_media_segment(),
            data: Bytes::new(),
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Disabled,
        };
        assert!(!segment.has_psi_tables());
    }

    #[test]
    fn test_has_psi_tables_non_ts() {
        let segment = TsSegmentData {
            segment: make_media_segment(),
            data: Bytes::from_static(b"this is not ts data"),
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Disabled,
        };
        assert!(!segment.has_psi_tables());
    }

    #[test]
    fn test_get_video_streams_empty() {
        let segment = TsSegmentData {
            segment: make_media_segment(),
            data: Bytes::new(),
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Disabled,
        };
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
}
