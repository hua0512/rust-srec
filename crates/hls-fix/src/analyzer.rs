//! # HLS Analyzer Module
//!
//! This module provides functionality for analyzing HLS (HTTP Live Streaming) segments
//! and collecting statistics about the content.
//!
//! ## Key Features:
//!
//! - Analyzes different segment types (TS, fMP4 init, fMP4 media)
//! - Tracks content metadata (codecs, bitrates, resolutions)
//! - Collects statistics on segments (counts, durations, sizes)
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!

use hls::{HlsData, M4sData, SegmentType};
use mp4::fragment::{
    Av1ValidationOptions, extract_av1_track_ids_from_init,
    validate_av1_media_segment_with_track_ids_and_options,
};
use std::fmt;
use tracing::{debug, info};

/// AV1 fMP4 sample validation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Av1SampleValidationMode {
    /// Disable AV1 sample-level validation.
    Off,
    /// Reject AV1 ISOBMFF "SHOULD NOT" OBU types.
    #[default]
    StrictShouldNot,
    /// Strict mode: also reject reserved OBU types.
    StrictAll,
}

// Stats structure to hold all the metrics
#[derive(Debug, Clone)]
pub struct HlsStats {
    // General stats
    pub total_size: u64,
    pub total_duration: f32,
    pub has_ts_segments: bool,
    pub has_mp4_segments: bool,

    // Segment counts
    pub ts_segment_count: u32,
    pub mp4_init_segment_count: u32,
    pub mp4_media_segment_count: u32,
    pub total_segment_count: u32,

    // Sizes by segment type
    pub ts_segments_size: u64,
    pub mp4_init_segments_size: u64,
    pub mp4_media_segments_size: u64,

    // Duration tracking
    pub ts_segments_duration: f32,
    pub mp4_segments_duration: f32,

    // Last segment info
    pub last_segment_type: Option<SegmentType>,
    pub last_segment_size: u64,
    pub last_segment_duration: f32,
}

impl Default for HlsStats {
    fn default() -> Self {
        Self {
            total_size: 0,
            total_duration: 0.0,
            has_ts_segments: false,
            has_mp4_segments: false,
            ts_segment_count: 0,
            mp4_init_segment_count: 0,
            mp4_media_segment_count: 0,
            total_segment_count: 0,
            ts_segments_size: 0,
            mp4_init_segments_size: 0,
            mp4_media_segments_size: 0,
            ts_segments_duration: 0.0,
            mp4_segments_duration: 0.0,
            last_segment_type: None,
            last_segment_size: 0,
            last_segment_duration: 0.0,
        }
    }
}

impl HlsStats {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Calculate overall average bitrate in kbps
    pub fn calculate_overall_bitrate(&self) -> f32 {
        if self.total_duration <= 0.0 {
            return 0.0;
        }

        // Convert bytes to bits and duration to seconds
        let bits = (self.total_size * 8) as f32;
        let kbits = bits / 1000.0;

        // Return kbps
        kbits / self.total_duration
    }

    /// Calculate TS segments bitrate in kbps
    pub fn calculate_ts_bitrate(&self) -> f32 {
        if self.ts_segments_duration <= 0.0 {
            return 0.0;
        }

        // Convert bytes to bits and duration to seconds
        let bits = (self.ts_segments_size * 8) as f32;
        let kbits = bits / 1000.0;

        // Return kbps
        kbits / self.ts_segments_duration
    }

    /// Calculate MP4 segments bitrate in kbps (excluding init segments)
    pub fn calculate_mp4_bitrate(&self) -> f32 {
        if self.mp4_segments_duration <= 0.0 {
            return 0.0;
        }

        // Convert bytes to bits and duration to seconds
        let bits = (self.mp4_media_segments_size * 8) as f32;
        let kbits = bits / 1000.0;

        // Return kbps
        kbits / self.mp4_segments_duration
    }
}

impl fmt::Display for HlsStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "HLS Stream Statistics:")?;
        writeln!(f, "  Total size: {} bytes", self.total_size)?;
        writeln!(f, "  Total duration: {:.2}s", self.total_duration)?;
        writeln!(
            f,
            "  Overall bitrate: {:.2} kbps",
            self.calculate_overall_bitrate()
        )?;

        writeln!(f, "  Segments:")?;
        writeln!(f, "    Total segments: {}", self.total_segment_count)?;

        if self.has_ts_segments {
            writeln!(f, "    TS segments: {}", self.ts_segment_count)?;
            writeln!(f, "    TS segments size: {} bytes", self.ts_segments_size)?;
            writeln!(
                f,
                "    TS segments duration: {:.2}s",
                self.ts_segments_duration
            )?;
            writeln!(f, "    TS bitrate: {:.2} kbps", self.calculate_ts_bitrate())?;
        }

        if self.has_mp4_segments {
            writeln!(f, "    MP4 segments: {}", self.mp4_media_segment_count)?;
            writeln!(f, "    MP4 init segments: {}", self.mp4_init_segment_count)?;
            writeln!(
                f,
                "    MP4 segments size: {} bytes",
                self.mp4_media_segments_size
            )?;
            writeln!(
                f,
                "    MP4 init segments size: {} bytes",
                self.mp4_init_segments_size
            )?;
            writeln!(
                f,
                "    MP4 segments duration: {:.2}s",
                self.mp4_segments_duration
            )?;
            writeln!(
                f,
                "    MP4 bitrate: {:.2} kbps",
                self.calculate_mp4_bitrate()
            )?;
        }

        // Last segment info
        if let Some(segment_type) = &self.last_segment_type {
            writeln!(f, "  Last segment:")?;
            writeln!(f, "    Type: {segment_type:?}")?;
            writeln!(f, "    Size: {} bytes", self.last_segment_size)?;
            if self.last_segment_duration > 0.0 {
                writeln!(f, "    Duration: {:.2}s", self.last_segment_duration)?;
            }
        }

        Ok(())
    }
}

/// HLS analyzer for collecting segment statistics
#[derive(Default)]
pub struct HlsAnalyzer {
    pub stats: HlsStats,
    last_mp4_av1_track_ids: Option<Vec<u32>>,
    av1_validation_mode: Av1SampleValidationMode,
}

impl HlsAnalyzer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_av1_validation_mode(mode: Av1SampleValidationMode) -> Self {
        Self {
            av1_validation_mode: mode,
            ..Self::default()
        }
    }

    pub fn reset(&mut self) {
        self.stats.reset();
        self.last_mp4_av1_track_ids = None;
    }

    /// Analyze a segment and update statistics
    pub fn analyze_segment(&mut self, segment: &HlsData) -> Result<(), String> {
        match segment {
            HlsData::TsData(ts_data) => {
                self.stats.has_ts_segments = true;
                self.stats.ts_segment_count += 1;

                let segment_size = ts_data.data.len() as u64;
                self.stats.ts_segments_size += segment_size;
                self.stats.total_size += segment_size;

                let duration = ts_data.segment.duration;
                self.stats.ts_segments_duration += duration;
                self.stats.total_duration += duration;

                // Update last segment info
                self.stats.last_segment_type = Some(SegmentType::Ts);
                self.stats.last_segment_size = segment_size;
                self.stats.last_segment_duration = duration;
            }
            HlsData::M4sData(M4sData::InitSegment(init_segment)) => {
                self.stats.has_mp4_segments = true;
                self.stats.mp4_init_segment_count += 1;

                let mut track_ids = extract_av1_track_ids_from_init(&init_segment.data);
                track_ids.sort_unstable();
                track_ids.dedup();
                self.last_mp4_av1_track_ids = Some(track_ids);

                let segment_size = init_segment.data.len() as u64;
                self.stats.mp4_init_segments_size += segment_size;
                self.stats.total_size += segment_size;

                // Update last segment info
                self.stats.last_segment_type = Some(SegmentType::M4sInit);
                self.stats.last_segment_size = segment_size;
                self.stats.last_segment_duration = 0.0; // Init segments don't have duration
            }
            HlsData::M4sData(M4sData::Segment(media_segment)) => {
                if self.av1_validation_mode != Av1SampleValidationMode::Off
                    && let Some(track_ids) = &self.last_mp4_av1_track_ids
                {
                    let options = match self.av1_validation_mode {
                        Av1SampleValidationMode::Off => Av1ValidationOptions {
                            enforce_should_not_obus: false,
                            enforce_reserved_obus: false,
                        },
                        Av1SampleValidationMode::StrictShouldNot => Av1ValidationOptions {
                            enforce_should_not_obus: true,
                            enforce_reserved_obus: false,
                        },
                        Av1SampleValidationMode::StrictAll => Av1ValidationOptions {
                            enforce_should_not_obus: true,
                            enforce_reserved_obus: true,
                        },
                    };

                    let validation = validate_av1_media_segment_with_track_ids_and_options(
                        &media_segment.data,
                        track_ids,
                        options,
                    )
                    .map_err(|e| format!("AV1 fMP4 media-segment validation failed: {e}"))?;

                    if validation.checked_samples > 0 {
                        debug!(
                            checked_tracks = validation.checked_tracks,
                            checked_samples = validation.checked_samples,
                            "Validated AV1 fMP4 media samples"
                        );
                    }
                }

                self.stats.has_mp4_segments = true;
                self.stats.mp4_media_segment_count += 1;

                let segment_size = media_segment.data.len() as u64;
                self.stats.mp4_media_segments_size += segment_size;
                self.stats.total_size += segment_size;

                let duration = media_segment.segment.duration;
                self.stats.mp4_segments_duration += duration;
                self.stats.total_duration += duration;

                // Update last segment info
                self.stats.last_segment_type = Some(SegmentType::M4sMedia);
                self.stats.last_segment_size = segment_size;
                self.stats.last_segment_duration = duration;
            }
            HlsData::EndMarker => {
                debug!("End marker received, no analysis needed");
            }
        }

        self.stats.total_segment_count = self.stats.ts_segment_count
            + self.stats.mp4_init_segment_count
            + self.stats.mp4_media_segment_count;

        Ok(())
    }

    /// Build final stats after analyzing all segments
    pub fn build_stats(&mut self) -> Result<HlsStats, String> {
        info!(
            "HLS analysis complete: {} segments, {:.2}s total duration",
            self.stats.total_segment_count, self.stats.total_duration
        );

        Ok(self.stats.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use mp4::test_support::{make_init_with_video_sample_entry, make_media_segment_for_track};

    fn create_test_mp4_init_segment_with_av1(track_id: u32) -> HlsData {
        HlsData::M4sData(M4sData::InitSegment(hls::M4sInitSegmentData {
            segment: MediaSegment {
                uri: "init-av1.mp4".to_string(),
                ..MediaSegment::empty()
            },
            data: make_init_with_video_sample_entry(track_id, *b"av01"),
        }))
    }

    fn create_test_mp4_media_segment_for_track(
        track_id: u32,
        sample: &[u8],
        duration: f32,
    ) -> HlsData {
        HlsData::M4sData(M4sData::Segment(hls::M4sSegmentData {
            segment: MediaSegment {
                uri: "segment-av1.m4s".to_string(),
                duration,
                ..MediaSegment::empty()
            },
            data: make_media_segment_for_track(track_id, sample),
        }))
    }

    fn create_test_ts_segment(duration: f32) -> HlsData {
        let mut data = vec![0u8; 188 * 10]; // 10 TS packets
        data[0] = 0x47; // TS sync byte
        data[188] = 0x47; // Next packet sync byte

        HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment {
                uri: "segment.ts".to_string(),
                duration,
                ..MediaSegment::empty()
            },
            data: Bytes::from(data),
            validate_crc: false,
            continuity_mode: ts::ContinuityMode::Disabled,
        })
    }

    fn create_test_mp4_init_segment() -> HlsData {
        let mut data = vec![0u8; 128];

        // Add fake 'ftyp' box
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x20; // size: 32 bytes
        data[4] = b'f';
        data[5] = b't';
        data[6] = b'y';
        data[7] = b'p';

        // Add fake 'moov' box
        data[32] = 0x00;
        data[33] = 0x00;
        data[34] = 0x00;
        data[35] = 0x60; // size: 96 bytes
        data[36] = b'm';
        data[37] = b'o';
        data[38] = b'o';
        data[39] = b'v';

        HlsData::M4sData(M4sData::InitSegment(hls::M4sInitSegmentData {
            segment: MediaSegment {
                uri: "init.mp4".to_string(),
                ..MediaSegment::empty()
            },
            data: Bytes::from(data),
        }))
    }

    fn create_test_mp4_media_segment(duration: f32) -> HlsData {
        let mut data = vec![0u8; 128];

        // Add fake 'moof' box
        data[0] = 0x00;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x40; // size: 64 bytes
        data[4] = b'm';
        data[5] = b'o';
        data[6] = b'o';
        data[7] = b'f';

        // Add fake 'mdat' box
        data[64] = 0x00;
        data[65] = 0x00;
        data[66] = 0x00;
        data[67] = 0x40; // size: 64 bytes
        data[68] = b'm';
        data[69] = b'd';
        data[70] = b'a';
        data[71] = b't';

        HlsData::M4sData(M4sData::Segment(hls::M4sSegmentData {
            segment: MediaSegment {
                uri: "segment.m4s".to_string(),
                duration,
                ..MediaSegment::empty()
            },
            data: Bytes::from(data),
        }))
    }

    #[test]
    fn test_analyze_ts_segment() {
        let mut analyzer = HlsAnalyzer::new();
        let segment = create_test_ts_segment(2.0);

        let result = analyzer.analyze_segment(&segment);
        assert!(result.is_ok());

        let stats = analyzer.stats.clone();
        assert_eq!(stats.ts_segment_count, 1);
        assert_eq!(stats.total_segment_count, 1);
        assert_eq!(stats.total_duration, 2.0);
        assert!(stats.has_ts_segments);
        assert!(!stats.has_mp4_segments);
    }

    #[test]
    fn test_analyze_mp4_segments() {
        let mut analyzer = HlsAnalyzer::new();

        // First analyze init segment
        let init_segment = create_test_mp4_init_segment();
        let result = analyzer.analyze_segment(&init_segment);
        assert!(result.is_ok());

        // Then analyze media segment
        let media_segment = create_test_mp4_media_segment(4.0);
        let result = analyzer.analyze_segment(&media_segment);
        assert!(result.is_ok());

        let stats = analyzer.stats.clone();
        assert_eq!(stats.mp4_init_segment_count, 1);
        assert_eq!(stats.mp4_media_segment_count, 1);
        assert_eq!(stats.total_segment_count, 2);
        assert_eq!(stats.total_duration, 4.0); // Init segments don't have duration
        assert!(!stats.has_ts_segments);
        assert!(stats.has_mp4_segments);

        // Check that video codec was detected
    }

    #[test]
    fn test_build_stats() {
        let mut analyzer = HlsAnalyzer::new();

        // Add TS segment
        analyzer
            .analyze_segment(&create_test_ts_segment(2.0))
            .unwrap();

        // Add MP4 segments
        analyzer
            .analyze_segment(&create_test_mp4_init_segment())
            .unwrap();
        analyzer
            .analyze_segment(&create_test_mp4_media_segment(3.0))
            .unwrap();

        let result = analyzer.build_stats();
        assert!(result.is_ok());

        let stats = result.unwrap();
        assert_eq!(stats.total_segment_count, 3);
        assert_eq!(stats.total_duration, 5.0);
        assert!(stats.has_ts_segments);
        assert!(stats.has_mp4_segments);
    }

    #[test]
    fn test_analyze_mp4_av1_media_segment_validation_ok() {
        let mut analyzer = HlsAnalyzer::new();

        let init = create_test_mp4_init_segment_with_av1(1);
        analyzer.analyze_segment(&init).unwrap();

        // OBU_FRAME with size 1 and payload 0xAA
        let sample = [0x32, 0x01, 0xAA];
        let media = create_test_mp4_media_segment_for_track(1, &sample, 1.0);
        analyzer.analyze_segment(&media).unwrap();

        assert_eq!(analyzer.stats.mp4_init_segment_count, 1);
        assert_eq!(analyzer.stats.mp4_media_segment_count, 1);
    }

    #[test]
    fn test_analyze_mp4_av1_media_segment_validation_rejects_disallowed_obu() {
        let mut analyzer = HlsAnalyzer::new();

        let init = create_test_mp4_init_segment_with_av1(1);
        analyzer.analyze_segment(&init).unwrap();

        // OBU_TEMPORAL_DELIMITER with size 0 (disallowed under strict ISOBMFF validation)
        let sample = [0x12, 0x00];
        let media = create_test_mp4_media_segment_for_track(1, &sample, 1.0);
        let err = analyzer.analyze_segment(&media).unwrap_err();

        assert!(err.contains("OBU_TEMPORAL_DELIMITER"));
    }

    #[test]
    fn test_analyze_mp4_av1_validation_mode_off_skips_checks() {
        let mut analyzer = HlsAnalyzer::with_av1_validation_mode(Av1SampleValidationMode::Off);

        let init = create_test_mp4_init_segment_with_av1(1);
        analyzer.analyze_segment(&init).unwrap();

        // Disallowed under strict modes, but should pass when validation is off.
        let sample = [0x12, 0x00];
        let media = create_test_mp4_media_segment_for_track(1, &sample, 1.0);
        analyzer.analyze_segment(&media).unwrap();
    }

    #[test]
    fn test_analyze_mp4_av1_validation_mode_strict_all_rejects_reserved_obu() {
        let mut analyzer =
            HlsAnalyzer::with_av1_validation_mode(Av1SampleValidationMode::StrictAll);

        let init = create_test_mp4_init_segment_with_av1(1);
        analyzer.analyze_segment(&init).unwrap();

        // Header: forbidden=0, type=9 (reserved), extension=0, size=1, reserved=0 => 0x4A
        let sample = [0x4A, 0x01, 0x00];
        let media = create_test_mp4_media_segment_for_track(1, &sample, 1.0);
        let err = analyzer.analyze_segment(&media).unwrap_err();

        assert!(err.contains("Reserved OBU type"));
    }
}
