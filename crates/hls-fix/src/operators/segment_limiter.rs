use std::sync::Arc;
use std::time::Duration;

use hls::{HlsData, M4sData, SplitReason};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::debug;

use crate::output_state::OutputState;

/// Limits recordings at HLS media item boundaries, preserving initialization.
pub struct SegmentLimiterOperator {
    fragment_check: Option<hls::mp4::IndependentFragmentCheck>,
    output_state: OutputState,
    max_duration: Option<Duration>,
    max_size: Option<u64>,
    current_duration: Duration,
}

impl SegmentLimiterOperator {
    pub fn new(max_duration: Option<Duration>, max_size: Option<u64>) -> Self {
        Self {
            fragment_check: None,
            output_state: OutputState::default(),
            max_duration,
            max_size,
            current_duration: Duration::ZERO,
        }
    }

    fn safe_duration(secs: f32) -> Duration {
        if !secs.is_finite() || secs <= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f32(secs)
        }
    }

    fn check_limit_reached(&self, item: &HlsData, segment_duration: f32) -> Option<SplitReason> {
        if !self.output_state.has_media() {
            return None;
        }
        if self.output_state.would_exceed(item, self.max_size) {
            return Some(SplitReason::SizeLimit);
        }
        if let Some(max_duration) = self.max_duration
            && !max_duration.is_zero()
            && self.current_duration + Self::safe_duration(segment_duration) > max_duration
        {
            return Some(SplitReason::DurationLimit);
        }
        None
    }

    fn reset_counters(&mut self) {
        self.current_duration = Duration::ZERO;
        self.output_state.reset_file();
    }

    /// Claims a waiting manual cut at this segment when it starts
    /// independently. A dependent segment is recorded as the reason the
    /// request is still waiting, so an expiry can explain itself.
    fn manual_boundary(
        context: &StreamerContext,
        media_written: bool,
        independent: impl FnOnce() -> bool,
    ) -> Option<u64> {
        let control = context.manual_split.as_ref()?;
        if !media_written || !control.is_pending() {
            return None;
        }
        if independent() {
            control.begin()
        } else {
            control.defer(pipeline_common::ManualSplitExpiryReason::NoIndependentSegment);
            None
        }
    }

    fn process_media(
        &mut self,
        item: HlsData,
        duration: f32,
        manual: Option<u64>,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if let Some(reason) = manual
            .map(|request_id| SplitReason::Manual { request_id })
            .or_else(|| self.check_limit_reached(&item, duration))
        {
            debug!(?reason, "Splitting HLS recording");
            output(HlsData::end_marker_with_reason(reason))?;
            self.reset_counters();
        }
        if item.is_mp4_media()
            && let Some(init) = self.output_state.pending_init().cloned()
        {
            let init = HlsData::M4sData(M4sData::InitSegment(init));
            self.output_state.record(&init);
            output(init)?;
        }
        self.output_state.record(&item);
        self.current_duration += Self::safe_duration(duration);
        output(item)
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
        match &input {
            HlsData::TsData(segment) => {
                let manual = Self::manual_boundary(context, self.output_state.has_media(), || {
                    segment
                        .analysis(hls::StreamProfileOptions {
                            include_resolution: false,
                        })
                        .is_ok_and(|analysis| analysis.independent_start)
                });
                let duration = segment.segment.duration;
                self.process_media(input, duration, manual, output)
            }
            HlsData::M4sData(M4sData::Segment(segment)) => {
                let manual = Self::manual_boundary(context, self.output_state.has_media(), || {
                    self.fragment_check
                        .as_ref()
                        .is_some_and(|check| check.is_independent(&segment.data))
                });
                let duration = segment.segment.duration;
                self.process_media(input, duration, manual, output)
            }
            HlsData::M4sData(M4sData::InitSegment(init)) => {
                if !self.output_state.is_repeated_init(init) {
                    self.fragment_check = hls::mp4::IndependentFragmentCheck::from_init(&init.data);
                }
                self.output_state.record(&input);
                output(input)
            }
            HlsData::EndMarker(_) => {
                output(input)?;
                self.reset_counters();
                Ok(())
            }
        }
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

    use crate::test_support::{INIT, MEDIA0, MEDIA1, OTHER_INIT, OTHER_MEDIA, init, media};

    #[test]
    fn size_limit_includes_initialization_on_every_output_sequence() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator =
            SegmentLimiterOperator::new(None, Some((INIT.len() + MEDIA0.len()) as u64));
        let mut out = Vec::new();
        for item in [
            init(INIT),
            media(MEDIA0),
            media(MEDIA1),
            media(MEDIA1),
            media(MEDIA0),
        ] {
            operator
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(out.len(), 9);
        assert!(matches!(
            out[2],
            HlsData::EndMarker(Some(SplitReason::SizeLimit))
        ));
        assert!(matches!(
            out[6],
            HlsData::EndMarker(Some(SplitReason::SizeLimit))
        ));
        assert_eq!(
            out.iter()
                .filter_map(HlsData::data)
                .map(Bytes::as_ref)
                .collect::<Vec<_>>(),
            [INIT, MEDIA0, INIT, MEDIA1, MEDIA1, INIT, MEDIA0]
        );
    }

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

        // A request that sees only dependent fragments explains its expiry.
        control.complete(control.snapshot().request_id);
        control.request(Duration::ZERO).unwrap();
        limiter
            .process(&context, media(0x0101_0000, 4), &mut |_| Ok(()))
            .unwrap();
        control.expire();
        assert_eq!(
            control.snapshot().expiry_reason,
            Some(pipeline_common::ManualSplitExpiryReason::NoIndependentSegment)
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
    fn reemits_latest_init_after_a_configuration_change_and_size_split() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator =
            SegmentLimiterOperator::new(None, Some((OTHER_INIT.len() + OTHER_MEDIA.len()) as u64));
        let mut out = Vec::new();
        for item in [
            init(INIT),
            media(MEDIA0),
            HlsData::end_marker_with_reason(SplitReason::StreamStructureChange {
                description: "init segment changed".into(),
            }),
            init(OTHER_INIT),
            media(OTHER_MEDIA),
            media(OTHER_MEDIA),
        ] {
            operator
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(out.len(), 8);
        assert!(matches!(
            out[2],
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. }))
        ));
        assert!(matches!(
            out[5],
            HlsData::EndMarker(Some(SplitReason::SizeLimit))
        ));
        assert_eq!(
            out.iter()
                .filter_map(HlsData::data)
                .map(Bytes::as_ref)
                .collect::<Vec<_>>(),
            [
                INIT,
                MEDIA0,
                OTHER_INIT,
                OTHER_MEDIA,
                OTHER_INIT,
                OTHER_MEDIA
            ]
        );
    }
}
