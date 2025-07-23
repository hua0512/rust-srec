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
use std::fmt;
use tracing::{debug, info};

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
}

impl HlsAnalyzer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        self.stats.reset();
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

                let segment_size = init_segment.data.len() as u64;
                self.stats.mp4_init_segments_size += segment_size;
                self.stats.total_size += segment_size;

                // Update last segment info
                self.stats.last_segment_type = Some(SegmentType::M4sInit);
                self.stats.last_segment_size = segment_size;
                self.stats.last_segment_duration = 0.0; // Init segments don't have duration
            }
            HlsData::M4sData(M4sData::Segment(media_segment)) => {
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
}
