use bytes::Bytes;
use hls::{HlsData, M4sData, M4sInitSegmentData, SegmentType};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

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

    fn safe_duration(secs: f32) -> Duration {
        if !secs.is_finite() || secs <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f32(secs)
        }
    }

    /// Helper function to check if any limit is reached
    fn is_limit_reached(&self, segment_data: &Bytes, segment_duration: f32) -> bool {
        // If no limits are set, no limit can be reached
        if self.max_duration.is_none() && self.max_size.is_none() {
            return false;
        }

        // Check size limit
        if let Some(max_size) = self.max_size
            && max_size > 0
        {
            let segment_size = segment_data.len() as u64;
            if self.current_size + segment_size > max_size {
                debug!(
                    "Size limit reached: {} > {}",
                    self.current_size + segment_size,
                    max_size
                );
                return true;
            }
        }

        // Check duration limit
        if let Some(max_duration) = self.max_duration
            && !max_duration.is_zero()
        {
            let segment_duration = Self::safe_duration(segment_duration);
            if self.current_duration + segment_duration > max_duration {
                debug!(
                    "Duration limit reached: {:?} > {:?}",
                    self.current_duration + segment_duration,
                    max_duration
                );
                return true;
            }
        }

        false
    }

    /// Reset tracking counters
    fn reset_counters(&mut self) {
        debug!("Resetting counters");
        self.current_duration = Duration::from_secs(0);
        self.current_size = 0;
        self.init_segment_sent = false;
    }

    /// Add segment to current tracking
    fn track_segment(&mut self, segment_data: &Bytes, segment_duration: f32) {
        self.current_size += segment_data.len() as u64;
        self.current_duration += Self::safe_duration(segment_duration);
    }
}

impl Processor<HlsData> for SegmentLimiterOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        match input.segment_type() {
            SegmentType::Ts => {
                if let HlsData::TsData(ts_data) = input {
                    // Check if the current segment would exceed the limit. If so, start a new sequence.
                    if self.is_limit_reached(&ts_data.data, ts_data.segment.duration) {
                        output(HlsData::end_marker())?;
                        self.reset_counters();
                    }

                    self.track_segment(&ts_data.data, ts_data.segment.duration);

                    // Unconditionally output the current segment and track its metrics.
                    output(HlsData::TsData(ts_data))?;
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
                    // Check if the current segment would exceed the limit. If so, start a new sequence.
                    if self.is_limit_reached(&segment.data, segment.segment.duration) {
                        output(HlsData::end_marker())?;
                        self.reset_counters();
                    }

                    // Ensure each new sequence starts with an init segment.
                    if !self.init_segment_sent
                        && let Some(init_segment) = &self.init_segment
                    {
                        output(HlsData::M4sData(M4sData::InitSegment(init_segment.clone())))?;
                        self.init_segment_sent = true;
                    }

                    self.track_segment(&segment.data, segment.segment.duration);

                    // Unconditionally output the current media segment and track its metrics.
                    output(HlsData::M4sData(M4sData::Segment(segment)))?;
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
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SegmentLimiter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::StreamerContext;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn splits_on_fractional_duration_limit() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = SegmentLimiterOperator::new(Some(Duration::from_secs_f32(1.0)), None);

        let mut out = Vec::new();
        let mut output = |item: HlsData| -> Result<(), PipelineError> {
            out.push(item);
            Ok(())
        };

        let seg1 = HlsData::ts(
            MediaSegment {
                duration: 0.6,
                ..MediaSegment::empty()
            },
            Bytes::from_static(b"aaaaaaaaaa"),
        );
        let seg2 = HlsData::ts(
            MediaSegment {
                duration: 0.6,
                ..MediaSegment::empty()
            },
            Bytes::from_static(b"bbbbbbbbbb"),
        );

        operator.process(&context, seg1, &mut output).unwrap();
        operator.process(&context, seg2, &mut output).unwrap();

        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], HlsData::TsData(_)));
        assert!(matches!(out[1], HlsData::EndMarker));
        assert!(matches!(out[2], HlsData::TsData(_)));
    }

    #[test]
    fn does_not_panic_on_non_finite_durations() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = SegmentLimiterOperator::new(Some(Duration::from_secs(1)), None);

        let mut out = Vec::new();
        let mut output = |item: HlsData| -> Result<(), PipelineError> {
            out.push(item);
            Ok(())
        };

        let seg = HlsData::ts(
            MediaSegment {
                duration: f32::NAN,
                ..MediaSegment::empty()
            },
            Bytes::from_static(b"aaaaaaaaaa"),
        );

        operator.process(&context, seg, &mut output).unwrap();
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], HlsData::TsData(_)));
    }
}
