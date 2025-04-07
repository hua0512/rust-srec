use flv::{
    audio::{AudioTagUtils, SoundFormat, SoundRate, SoundSize, SoundType},
    header::FlvHeader,
    resolution::Resolution,
    tag::{FlvTag, FlvTagData, FlvUtil},
    video::VideoCodecId,
};

use std::{fmt, io::Cursor};
use tracing::{debug, error};

use crate::utils::{FLV_HEADER_SIZE, FLV_PREVIOUS_TAG_SIZE, FLV_TAG_HEADER_SIZE};

// Stats structure to hold all the metrics
#[derive(Debug, Clone)]
pub struct FlvStats {
    pub file_size: u64,
    pub duration: u32,
    pub has_video: bool,
    pub has_audio: bool,
    pub video_codec: Option<VideoCodecId>,
    pub audio_codec: Option<SoundFormat>,

    pub tag_count: u32,
    pub audio_tag_count: u32,
    pub video_tag_count: u32,
    pub script_tag_count: u32,

    pub tags_size: u64,
    pub audio_tags_size: u64,
    pub video_tags_size: u64,

    pub audio_data_size: u64,
    pub video_data_size: u64,

    pub audio_data_rate: f32,
    pub audio_stereo: bool,
    pub audio_sample_rate: f32,
    pub audio_sample_size: u32,

    pub video_frame_rate: f32,
    pub video_data_rate: f32,

    pub last_timestamp: u32,
    pub last_audio_timestamp: u32,
    pub last_video_timestamp: u32,

    pub first_keyframe_timestamp: Option<u32>,

    pub resolution: Option<Resolution>,
    pub last_keyframe_timestamp: u32,
    pub last_keyframe_position: u64,
    pub keyframes: Vec<(f64, u64)>,
}

impl FlvStats {
    pub fn new() -> Self {
        Self {
            file_size: 0,
            duration: 0,
            has_video: false,
            has_audio: false,
            video_codec: None,
            audio_codec: None,
            tag_count: 0,
            audio_tag_count: 0,
            video_tag_count: 0,
            script_tag_count: 0,
            tags_size: 0,
            audio_tags_size: 0,
            video_tags_size: 0,
            audio_data_size: 0,
            video_data_size: 0,
            last_timestamp: 0,
            last_audio_timestamp: 0,
            last_video_timestamp: 0,
            resolution: None,
            last_keyframe_timestamp: 0,
            last_keyframe_position: 0,
            keyframes: Vec::new(),
            audio_stereo: true,
            audio_sample_rate: 0.0,
            audio_sample_size: 0,
            video_data_rate: 0.0,
            video_frame_rate: 0.0,
            first_keyframe_timestamp: None,
            audio_data_rate: 0.0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn calculate_frame_rate(&self) -> f32 {
        if self.last_timestamp <= 0 {
            return 0.0;
        }
        let duration_in_seconds =
            self.last_video_timestamp - self.first_keyframe_timestamp.unwrap_or(0).min(0);
        (self.video_tag_count as f32) * 1000.0 / duration_in_seconds as f32
    }

    pub fn calculate_video_bitrate(&self) -> f32 {
        if self.last_timestamp <= 0 {
            return 0.0;
        }
        (self.video_data_size as f32) * 8.0 / self.last_timestamp as f32
    }

    pub fn calculate_audio_bitrate(&self) -> f32 {
        if self.last_timestamp <= 0 {
            return 0.0;
        }
        (self.audio_data_size as f32) * 8.0 / self.last_timestamp as f32
    }
}

impl fmt::Display for FlvStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "FLV Statistics:")?;
        writeln!(f, "  File size: {} bytes", self.file_size)?;
        writeln!(f, "  Duration: {}s", self.duration)?;

        writeln!(f, "  Media:")?;
        writeln!(f, "    Has video: {}", self.has_video)?;
        if self.has_video {
            writeln!(
                f,
                "    Video codec: {:?}",
                self.video_codec.unwrap_or(VideoCodecId::Avc)
            )?;
            if let Some(resolution) = &self.resolution {
                writeln!(
                    f,
                    "    Resolution: {}x{}",
                    resolution.width, resolution.height
                )?;
            }
            writeln!(f, "    Frame rate: {:.2} fps", self.video_frame_rate)?;
            writeln!(f, "    Video data rate: {:.2} kbps", self.video_data_rate)?;
        }

        writeln!(f, "    Has audio: {}", self.has_audio)?;
        if self.has_audio {
            writeln!(
                f,
                "    Audio codec: {:?}",
                self.audio_codec.unwrap_or(SoundFormat::Aac)
            )?;
            writeln!(f, "    Sample rate: {:.0} Hz", self.audio_sample_rate)?;
            writeln!(f, "    Sample size: {} bits", self.audio_sample_size)?;
            writeln!(f, "    Stereo: {}", self.audio_stereo)?;
            writeln!(f, "    Audio data rate: {:.2} kbps", self.audio_data_rate)?;
        }

        writeln!(f, "  Tags:")?;
        writeln!(f, "    Total tags: {}", self.tag_count)?;
        writeln!(f, "    Audio tags: {}", self.audio_tag_count)?;
        writeln!(f, "    Video tags: {}", self.video_tag_count)?;
        writeln!(f, "    Script tags: {}", self.script_tag_count)?;

        writeln!(f, "  Sizes:")?;
        writeln!(f, "    Tags size: {} bytes", self.tags_size)?;
        writeln!(f, "    Audio tags size: {} bytes", self.audio_tags_size)?;
        writeln!(f, "    Video tags size: {} bytes", self.video_tags_size)?;
        writeln!(f, "    Audio data size: {} bytes", self.audio_data_size)?;
        writeln!(f, "    Video data size: {} bytes", self.video_data_size)?;

        writeln!(f, "  Timestamps:")?;
        writeln!(f, "    Last timestamp: {}ms", self.last_timestamp)?;
        writeln!(
            f,
            "    Last audio timestamp: {}ms",
            self.last_audio_timestamp
        )?;
        writeln!(
            f,
            "    Last video timestamp: {}ms",
            self.last_video_timestamp
        )?;

        // Compress keyframes information
        let keyframe_count = self.keyframes.len();
        if keyframe_count > 0 {
            writeln!(f, "  Keyframes: {}", keyframe_count)?;

            // Show first keyframe
            if keyframe_count > 0 {
                let first = &self.keyframes[0];
                writeln!(f, "    First: {:.2}s @ position {}", first.0, first.1)?;
            }

            // Show last keyframe
            if keyframe_count > 1 {
                let last = &self.keyframes[keyframe_count - 1];
                writeln!(f, "    Last: {:.2}s @ position {}", last.0, last.1)?;
            }

            // Show distribution info if there are many keyframes
            if keyframe_count > 5 {
                // Calculate average keyframe interval
                let mut intervals = Vec::with_capacity(keyframe_count - 1);
                for i in 1..keyframe_count {
                    intervals.push(self.keyframes[i].0 - self.keyframes[i - 1].0);
                }

                // Calculate statistics
                let avg_interval = intervals.iter().sum::<f64>() / intervals.len() as f64;

                // Find min and max intervals
                let mut min_interval = f64::MAX;
                let mut max_interval = f64::MIN;

                for interval in &intervals {
                    min_interval = min_interval.min(*interval);
                    max_interval = max_interval.max(*interval);
                }

                writeln!(
                    f,
                    "    Keyframe intervals: {:.2}s avg, {:.2}s min, {:.2}s max",
                    avg_interval, min_interval, max_interval
                )?;
            } else if keyframe_count > 2 {
                // For a small number of keyframes, show them all
                writeln!(f, "    All keyframes (time in seconds @ position):")?;
                for (i, keyframe) in self.keyframes.iter().enumerate() {
                    write!(f, "      {}: {:.2}s @ {}", i, keyframe.0, keyframe.1)?;
                    if i < keyframe_count - 1 {
                        writeln!(f)?;
                    }
                }
            }
        } else {
            writeln!(f, "  No keyframes found")?;
        }

        Ok(())
    }
}

pub struct FlvAnalyzer {
    pub stats: FlvStats,

    pub header_analyzed: bool,
    pub has_video_sequence_header: bool,
    pub has_audio_sequence_header: bool,
}

impl FlvAnalyzer {
    pub fn new() -> Self {
        Self {
            stats: FlvStats::new(),
            header_analyzed: false,
            has_video_sequence_header: false,
            has_audio_sequence_header: false,
        }
    }

    pub fn reset(&mut self) {
        self.stats.reset();
        self.header_analyzed = false;
        self.has_video_sequence_header = false;
        self.has_audio_sequence_header = false;
    }

    pub fn analyze_header(&mut self, header: &FlvHeader) -> Result<(), String> {
        debug!("Analyzing FLV header: {:?}", header);
        if self.header_analyzed {
            return Err("Header already analyzed".to_string());
        }
        let version = header.version;
        if version != 1 {
            return Err(format!("Unsupported FLV version: {}", version));
        }

        // delay those stats to be set when a sequence header is found
        // self.stats.has_audio = header.has_audio;
        // self.stats.has_video = header.has_video;
        self.stats.file_size = (FLV_HEADER_SIZE + FLV_PREVIOUS_TAG_SIZE) as u64; // 9 bytes for header + 4 bytes for previous tag size
        self.header_analyzed = true;

        Ok(())
    }

    fn analyze_audio_tag(&mut self, tag: &FlvTag) {
        if tag.is_audio_sequence_header() {
            self.stats.has_audio = true;
            self.has_audio_sequence_header = true;

            if self.stats.audio_codec.is_none() {
                let audio_tag_utils = AudioTagUtils::new(tag.data.clone());
                debug!(
                    "Audio properties detected: codec={:?}, rate={:?}, size={:?}, type={:?}",
                    audio_tag_utils.sound_format(),
                    audio_tag_utils.sound_rate(),
                    audio_tag_utils.sound_size(),
                    audio_tag_utils.sound_type()
                );
                let sound_format = audio_tag_utils.sound_format().unwrap_or(SoundFormat::Aac);
                let sample_rate = audio_tag_utils
                    .sound_rate()
                    .map(|s| match s {
                        SoundRate::Hz5512 => 5512.0,
                        SoundRate::Hz11025 => 11025.0,
                        SoundRate::Hz22050 => 22050.0,
                        SoundRate::Hz44100 => 44100.0,
                        SoundRate::Hz48000 => 48000.0,
                    })
                    .unwrap_or(44100.0); // Default to 44100 Hz if not found
                let sample_size = audio_tag_utils
                    .sound_size()
                    .map(|s| match s {
                        SoundSize::Bits8 => 8,
                        SoundSize::Bits16 => 16,
                        SoundSize::Bits24 => 24,
                    })
                    .unwrap_or(16); // Default to 16 bits if not found

                let sound_type = audio_tag_utils.sound_type().unwrap_or(SoundType::Stereo);

                let stereo = sound_type == SoundType::Stereo;

                self.stats.audio_sample_rate = sample_rate;
                self.stats.audio_sample_size = sample_size;
                self.stats.audio_stereo = stereo;
                self.stats.audio_codec = Some(sound_format);
            }
        }

        let data_size = tag.data.len() as u64;
        self.stats.audio_tag_count += 1;
        self.stats.audio_tags_size += data_size + FLV_TAG_HEADER_SIZE as u64; // 11 bytes for header
        self.stats.audio_data_size += data_size;
        self.stats.last_audio_timestamp = tag.timestamp_ms;
    }

    fn analyze_video_tag(&mut self, tag: &FlvTag) {
        let timestamp = tag.timestamp_ms;
        if tag.is_video_sequence_header() {
            if self.stats.resolution.is_none() {
                if let Some(resolution) = tag.get_video_resolution() {
                    self.stats.resolution = Some(resolution);
                } else {
                    error!("Failed to get video resolution");
                }
            }

            if self.stats.video_codec.is_none() {
                // parse the codec id
                if let Some(codec_id) = tag.get_video_codec_id() {
                    self.stats.video_codec = Some(codec_id);
                } else {
                    error!("Failed to get video codec id");
                }
            }

            self.stats.has_video = true;
            self.has_video_sequence_header = true;
        } else if tag.is_key_frame() {
            let position = self.stats.file_size;
            self.stats
                .keyframes
                .push((timestamp as f64 / 1000.0, position));
            self.stats.last_keyframe_timestamp = timestamp;
            self.stats.last_keyframe_position = position;
            // set the first keyframe timestamp
            if self.stats.first_keyframe_timestamp.is_none() {
                self.stats.first_keyframe_timestamp = Some(timestamp);
            }
        }

        let data_size = tag.data.len() as u64;
        self.stats.video_tag_count += 1;
        self.stats.video_tags_size +=
            data_size as u64 + FLV_TAG_HEADER_SIZE as u64 + FLV_PREVIOUS_TAG_SIZE as u64; // 11 bytes for header
        self.stats.video_data_size += data_size as u64;
        self.stats.last_video_timestamp = timestamp;
    }

    pub fn analyze_tag(&mut self, tag: &FlvTag) -> Result<(), String> {
        if tag.is_audio_tag() {
            self.analyze_audio_tag(tag);
        } else if tag.is_video_tag() {
            self.analyze_video_tag(tag);
        } else if tag.is_script_tag() {
            self.stats.script_tag_count += 1;
        } else {
            return Err(format!("Unknown tag type: {}", tag.tag_type));
        }

        let data_size = tag.data.len() as u64;

        self.stats.tag_count += 1;
        self.stats.tags_size += data_size as u64;
        self.stats.file_size +=
            data_size as u64 + FLV_TAG_HEADER_SIZE as u64 + FLV_PREVIOUS_TAG_SIZE as u64; // 11 bytes for header

        self.stats.last_timestamp = tag.timestamp_ms;

        Ok(())
    }

    pub fn build_stats(&mut self) -> Result<FlvStats, String> {
        if !self.header_analyzed {
            return Err("Header not analyzed".to_string());
        }

        if self.stats.has_video {
            self.stats.video_data_rate = self.stats.calculate_video_bitrate();
            self.stats.video_frame_rate = self.stats.calculate_frame_rate();
        }

        if self.stats.has_audio {
            self.stats.audio_data_rate = self.stats.calculate_audio_bitrate();
        }

        self.stats.duration = self.stats.last_timestamp / 1000;

        Ok(self.stats.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flv::header::FlvHeader;

    #[test]
    fn test_analyze_header() {
        let mut analyzer = FlvAnalyzer::new();
        let header = FlvHeader::new(true, true);
        assert!(analyzer.analyze_header(&header).is_ok());
        assert_eq!(analyzer.stats.file_size, 13); // 9 bytes for header + 4 bytes for previous tag size
    }
}
