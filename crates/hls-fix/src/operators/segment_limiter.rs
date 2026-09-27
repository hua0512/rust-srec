use bytes::Bytes;
use hls::{HlsData, M4sData, M4sInitSegmentData, SegmentType, SplitReason};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

/// HLS processor: Limits HLS segments based on size or duration
pub struct SegmentLimiterOperator {
    fragment_check: Option<hls::mp4::IndependentFragmentCheck>,
    media_written: bool,
    max_duration: Option<Duration>,
    max_size: Option<u64>,
    current_duration: Duration,
    current_size: u64,
    // Most recent initialization segment, re-emitted at the start of each new sequence.
    init_segment: Option<M4sInitSegmentData>,
    // Track if we've output an init segment recently
    init_segment_sent: bool,
}

impl SegmentLimiterOperator {
    pub fn new(max_duration: Option<Duration>, max_size: Option<u64>) -> Self {
        Self {
            fragment_check: None,
            media_written: false,
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

    /// Helper function to check if any limit is reached, returning the reason if so
    fn check_limit_reached(
        &self,
        segment_data: &Bytes,
        segment_duration: f32,
    ) -> Option<SplitReason> {
        // If no limits are set, no limit can be reached
        if self.max_duration.is_none() && self.max_size.is_none() {
            return None;
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
                return Some(SplitReason::SizeLimit);
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
                return Some(SplitReason::DurationLimit);
            }
        }

        None
    }

    /// Reset tracking counters
    fn reset_counters(&mut self) {
        debug!("Resetting counters");
        self.current_duration = Duration::from_secs(0);
        self.current_size = 0;
        self.init_segment_sent = false;
        self.media_written = false;
    }

    /// Add segment to current tracking
    fn track_segment(&mut self, segment_data: &Bytes, segment_duration: f32) {
        self.media_written = true;
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
                    let manual = if self.media_written
                        && context.manual_split.as_ref().is_some_and(|control| {
                            control.snapshot().status == pipeline_common::ManualSplitStatus::Pending
                        })
                        && ts_data
                            .analysis(hls::StreamProfileOptions {
                                include_resolution: false,
                            })
                            .is_ok_and(|analysis| analysis.independent_start)
                    {
                        context
                            .manual_split
                            .as_ref()
                            .and_then(|control| control.begin())
                    } else {
                        None
                    };
                    // Check if the current segment would exceed the limit. If so, start a new sequence.
                    if let Some(reason) = manual
                        .map(|request_id| SplitReason::Manual { request_id })
                        .or_else(|| {
                            self.check_limit_reached(ts_data.data(), ts_data.segment.duration)
                        })
                    {
                        output(HlsData::end_marker_with_reason(reason))?;
                        self.reset_counters();
                    }

                    self.track_segment(ts_data.data(), ts_data.segment.duration);

                    // Unconditionally output the current segment and track its metrics.
                    output(HlsData::TsData(ts_data))?;
                }
            }
            SegmentType::M4sInit => {
                if let HlsData::M4sData(M4sData::InitSegment(init_segment)) = input {
                    self.fragment_check =
                        hls::mp4::IndependentFragmentCheck::from_init(&init_segment.data);
                    // Track the most recent init segment so a later size/duration split re-emits
                    // the codec configuration the following M4sMedia is encoded against, not a
                    // stale one from before a SegmentSplitOperator init-CRC switch.
                    self.init_segment = Some(init_segment.clone());

                    // Always output the init segment when we encounter it directly
                    output(HlsData::M4sData(M4sData::InitSegment(init_segment)))?;
                    self.init_segment_sent = true;
                }
            }
            SegmentType::M4sMedia => {
                if let HlsData::M4sData(M4sData::Segment(segment)) = input {
                    let manual = if self.media_written
                        && context.manual_split.as_ref().is_some_and(|control| {
                            control.snapshot().status == pipeline_common::ManualSplitStatus::Pending
                        })
                        && self
                            .fragment_check
                            .as_ref()
                            .is_some_and(|check| check.is_independent(&segment.data))
                    {
                        context
                            .manual_split
                            .as_ref()
                            .and_then(|control| control.begin())
                    } else {
                        None
                    };
                    // Check if the current segment would exceed the limit. If so, start a new sequence.
                    if let Some(reason) = manual
                        .map(|request_id| SplitReason::Manual { request_id })
                        .or_else(|| {
                            self.check_limit_reached(&segment.data, segment.segment.duration)
                        })
                    {
                        output(HlsData::end_marker_with_reason(reason))?;
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
                // Forward upstream EndMarkers as-is (preserve their reason)
                output(input)?;
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
    fn manual_cut_waits_for_sync_sample_and_repeats_the_init_without_losing_segments() {
        use mp4::test_support::{make_box, make_full_box};
        let mut track_header = vec![0; 12];
        track_header[8..12].copy_from_slice(&1u32.to_be_bytes());
        let mut handler = vec![0; 8];
        handler[4..8].copy_from_slice(b"vide");
        let mut track = make_full_box(b"tkhd", 0, 0, &track_header);
        track.extend(make_box(b"mdia", &make_full_box(b"hdlr", 0, 0, &handler)));
        let mut defaults = vec![0; 20];
        defaults[..4].copy_from_slice(&1u32.to_be_bytes());
        let mut movie = make_box(b"trak", &track);
        movie.extend(make_box(b"mvex", &make_full_box(b"trex", 0, 0, &defaults)));
        let init = Bytes::from(make_box(b"moov", &movie));
        let media = |flags: u32, value| {
            let mut fragment = make_full_box(b"tfhd", 0, 0x020000, &1u32.to_be_bytes());
            let mut samples = 1u32.to_be_bytes().to_vec();
            samples.extend_from_slice(&flags.to_be_bytes());
            fragment.extend(make_full_box(b"trun", 0, 4, &samples));
            let mut data = make_box(b"moof", &make_box(b"traf", &fragment));
            data.extend(make_box(b"mdat", &[value]));
            HlsData::mp4_segment(
                MediaSegment {
                    duration: 1.0,
                    ..Default::default()
                },
                data.into(),
            )
        };
        let control = Arc::new(pipeline_common::ManualSplitControl::default());
        control.enable();
        let context = Arc::new(
            StreamerContext::new(CancellationToken::new()).with_manual_split(control.clone()),
        );
        let mut limiter = SegmentLimiterOperator::new(None, None);
        let mut out = Vec::new();
        for item in [
            HlsData::mp4_init(Default::default(), init.clone()),
            media(0x0200_0000, 1),
        ] {
            limiter
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }
        control.request(Duration::from_secs(30)).unwrap();
        limiter
            .process(&context, media(0x0101_0000, 2), &mut |item| {
                out.push(item);
                Ok(())
            })
            .unwrap();
        assert!(!out.iter().any(HlsData::is_end_marker));
        limiter
            .process(&context, media(0x0200_0000, 3), &mut |item| {
                out.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(out.len(), 6);
        assert!(matches!(
            out[3],
            HlsData::EndMarker(Some(SplitReason::Manual { .. }))
        ));
        assert_eq!(out[4].data(), Some(&init));
        assert_eq!(
            out.iter()
                .filter_map(|item| if item.is_mp4_media() {
                    item.data().and_then(|data| data.last()).copied()
                } else {
                    None
                })
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

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
        assert!(matches!(out[1], HlsData::EndMarker(_)));
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

    #[test]
    fn reemits_latest_init_after_split() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = SegmentLimiterOperator::new(None, Some(10));

        let mut out = Vec::new();
        let mut output = |item: HlsData| -> Result<(), PipelineError> {
            out.push(item);
            Ok(())
        };

        let media = |bytes: &'static [u8]| {
            HlsData::mp4_segment(
                MediaSegment {
                    duration: 1.0,
                    ..MediaSegment::empty()
                },
                Bytes::from_static(bytes),
            )
        };

        operator
            .process(
                &context,
                HlsData::mp4_init(MediaSegment::empty(), Bytes::from_static(b"AAAA")),
                &mut output,
            )
            .unwrap();
        operator
            .process(&context, media(b"11111111"), &mut output)
            .unwrap();
        // Codec change mid-sequence: a new init segment replaces the tracked one.
        operator
            .process(
                &context,
                HlsData::mp4_init(MediaSegment::empty(), Bytes::from_static(b"BBBB")),
                &mut output,
            )
            .unwrap();
        // 8 more bytes exceed max_size=10, forcing a split; the new sequence must
        // restart with the latest init segment, which this media is encoded against.
        operator
            .process(&context, media(b"22222222"), &mut output)
            .unwrap();

        assert_eq!(out.len(), 6);
        assert!(matches!(
            out[3],
            HlsData::EndMarker(Some(SplitReason::SizeLimit))
        ));
        match &out[4] {
            HlsData::M4sData(M4sData::InitSegment(init)) => {
                assert_eq!(init.data.as_ref(), b"BBBB");
            }
            other => panic!("expected re-emitted init segment, got {other:?}"),
        }
        assert!(matches!(out[5], HlsData::M4sData(M4sData::Segment(_))));
    }
}
