use bytes::Bytes;
use hls::{HlsData, M4sData, M4sInitSegmentData, SegmentType};
use pipeline_common::{PipelineError, Processor};
use std::time::Duration;

/// HLS processor: Limits HLS segments based on size or duration
pub struct SegmentLimiterOperator {
    max_duration: Option<Duration>,
    max_size: Option<u64>,
    current_duration: Duration,
    current_size: u64,
    // Store the first initialization segment we encounter
    init_segment: Option<M4sInitSegmentData>,
    // Track if we've output an init segment recently
    init_segment_sent: bool,
}

impl SegmentLimiterOperator {
    pub fn new(max_duration: Option<Duration>, max_size: Option<u64>) -> Self {
        Self {
            max_duration,
            max_size,
            current_duration: Duration::from_secs(0),
            current_size: 0,
            init_segment: None,
            init_segment_sent: false,
        }
    }

    /// Helper function to check if any limit is reached
    fn is_limit_reached(&self, segment_data: &Bytes, segment_duration: f32) -> bool {
        // If no limits are set, no limit can be reached
        if self.max_duration.is_none() && self.max_size.is_none() {
            return false;
        }

        // Check size limit
        if let Some(max_size) = self.max_size {
            let segment_size = segment_data.len() as u64;
            if self.current_size + segment_size > max_size {
                return true;
            }
        }

        // Check duration limit
        if let Some(max_duration) = self.max_duration {
            let segment_duration_ms = (segment_duration * 1000.0) as u64;
            let segment_duration = Duration::from_millis(segment_duration_ms);
            if self.current_duration + segment_duration > max_duration {
                return true;
            }
        }

        false
    }

    /// Reset tracking counters
    fn reset_counters(&mut self) {
        self.current_duration = Duration::from_secs(0);
        self.current_size = 0;
        self.init_segment_sent = false;
    }

    /// Add segment to current tracking
    fn track_segment(&mut self, segment_data: &Bytes, segment_duration: f32) {
        self.current_size += segment_data.len() as u64;
        self.current_duration += Duration::from_millis((segment_duration * 1000.0) as u64);
    }
}

impl Processor<HlsData> for SegmentLimiterOperator {
    fn process(
        &mut self,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Using new methods from the HlsData struct to simplify our code
        match input.segment_type() {
            SegmentType::Ts => {
                if let HlsData::TsData(ts_data) = input {
                    // Check if limit is reached
                    if self.is_limit_reached(&ts_data.data, ts_data.segment.duration) {
                        // Send the last tag and reset
                        output(HlsData::TsData(ts_data.clone()))?;
                        self.reset_counters();
                    } else {
                        // No limit reached, include the segment and track it
                        let data_clone = ts_data.data.clone();
                        output(HlsData::TsData(ts_data.clone()))?;
                        self.track_segment(&data_clone, ts_data.segment.duration);
                    }
                }
            }
            SegmentType::M4sInit => {
                if let HlsData::M4sData(M4sData::InitSegment(init_segment)) = input {
                    // Store the init segment for later use if it's the first one we've seen
                    if self.init_segment.is_none() {
                        self.init_segment = Some(init_segment.clone());
                    }

                    // Always output the init segment when we encounter it directly
                    output(HlsData::M4sData(M4sData::InitSegment(init_segment)))?;
                    self.init_segment_sent = true;
                }
            }
            SegmentType::M4sMedia => {
                if let HlsData::M4sData(M4sData::Segment(segment)) = input {
                    // Check if limit is reached
                    if self.is_limit_reached(&segment.data, segment.segment.duration) {
                        // Send the init segment if needed and the last segment, then reset
                        if !self.init_segment_sent {
                            if let Some(init_segment) = &self.init_segment {
                                output(HlsData::M4sData(M4sData::InitSegment(
                                    init_segment.clone(),
                                )))?;
                            }
                        }
                        output(HlsData::M4sData(M4sData::Segment(segment.clone())))?;
                        self.reset_counters();
                    } else {
                        // No limit reached, send init if needed, include the segment and track it
                        if !self.init_segment_sent {
                            if let Some(init_segment) = &self.init_segment {
                                output(HlsData::M4sData(M4sData::InitSegment(
                                    init_segment.clone(),
                                )))?;
                                self.init_segment_sent = true;
                            }
                        }

                        let data_clone = segment.data.clone();
                        output(HlsData::M4sData(M4sData::Segment(segment.clone())))?;
                        self.track_segment(&data_clone, segment.segment.duration);
                    }
                }
            }
            SegmentType::EndMarker => {
                // Always include EndPlaylist markers
                output(HlsData::end_marker())?;
                self.reset_counters();
            }
        }

        Ok(())
    }

    fn finish(
        &mut self,
        _output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SegmentLimiter"
    }
}
