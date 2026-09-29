use std::sync::Arc;

use hls::{
    HlsData, M4sData, M4sInitSegmentData, Resolution, StreamProfileOptions, TsAnalysis,
    TsSegmentData,
};
use pipeline_common::{PipelineError, Processor, SplitReason, StreamerContext, crc32};
use tracing::{debug, info, warn};

/// An operator that splits HLS segments when meaningful stream parameters change.
///
/// The SegmentSplitOperator performs deep inspection of stream metadata and splits
/// the output when it detects changes that would cause playback issues if concatenated.
///
/// # Split Triggers (will cause a new output file)
///
/// - **MP4 init segment changes**: Different CRC indicates codec/resolution changes
/// - **Video resolution changes**: Detected via SPS parsing or stream profile
/// - **Program number changes**: Indicates a different broadcast program
/// - **Transport Stream ID changes**: Indicates a different stream source
/// - **Elementary stream codec type changes**: e.g., H.264 → H.265 at PMT level
///
/// Missing PSI is tolerated without replacing the last known program layout.
/// Playlist discontinuities are handled upstream: RFC 8216 4.3.2.3 requires
/// them for changes in track count, type, identifiers, or timestamp sequence.
/// A PMT describes the program, not just the packets present in one segment.
pub struct SegmentSplitOperator {
    context: Arc<StreamerContext>,
    last_init_segment_crc: Option<u32>,
    last_ts_analysis: Option<Arc<TsAnalysis>>,
    last_resolution: Option<Resolution>,
    last_init_segment: Option<M4sInitSegmentData>,
    /// Best-effort budget for TS resolution probing until we establish a baseline.
    ///
    /// Some streams only carry SPS intermittently; once we have a baseline
    /// resolution, we only probe when we can compare against it.
    resolution_probe_remaining: u8,
}

impl SegmentSplitOperator {
    /// Creates a new SegmentSplitOperator with the given context.
    ///
    /// # Arguments
    ///
    /// * `context` - The shared StreamerContext containing configuration and state
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self {
            context,
            last_init_segment_crc: None,
            last_ts_analysis: None,
            last_resolution: None,
            last_init_segment: None,
            resolution_probe_remaining: 50,
        }
    }

    // Calculate CRC32 for byte content (zlib CRC-32 for data fingerprinting, not MPEG-2 CRC-32)
    fn calculate_crc(data: &[u8]) -> u32 {
        crc32::crc32(data)
    }

    // Handle MP4 init segment - returns Some(reason) if a split is needed
    fn handle_init_segment(&mut self, data: &M4sInitSegmentData) -> Option<SplitReason> {
        let crc = Self::calculate_crc(&data.data);
        let mut split_reason = None;

        if let Some(previous_crc) = self.last_init_segment_crc {
            if previous_crc != crc {
                info!(
                    "{} Detected different init segment, splitting the stream",
                    self.context.name
                );
                split_reason = Some(SplitReason::StreamStructureChange {
                    description: "init segment changed".to_string(),
                });
            }
        } else {
            // First init segment encountered
            info!("{} First init segment encountered", self.context.name);
        }

        // Always update to the latest init segment, since this is the only place we see them.
        self.last_init_segment = Some(data.clone());
        self.last_init_segment_crc = Some(crc);

        split_reason
    }

    // Handle TS segment
    // Returns Some(reason) if a split is needed
    fn handle_ts_segment(&mut self, input: &TsSegmentData) -> Option<SplitReason> {
        let include_resolution =
            self.last_resolution.is_some() || self.resolution_probe_remaining > 0;
        let analysis = match input.analysis(StreamProfileOptions { include_resolution }) {
            Ok(analysis) => analysis,
            Err(error) => {
                warn!(stream = %self.context.name, %error, "Failed to analyze TS segment");
                return None;
            }
        };

        if !analysis.has_psi {
            debug!(
                "{} TS segment has no PSI tables, skipping analysis",
                self.context.name
            );
            return None;
        }

        // has_psi is set by a PAT alone; an empty programs list means the PMT was absent,
        // cut across the segment boundary, or failed CRC. Comparing an empty layout against
        // the previous layout would emit a spurious StreamStructureChange, so treat it like the
        // no-PSI case and retain the last complete analysis.
        if analysis.stream_info.programs.is_empty() {
            debug!(
                "{} TS segment has PAT but no parsed PMT, skipping structural comparison",
                self.context.name
            );
            return None;
        }

        let current_stream_info = &analysis.stream_info;
        let mut resolution = analysis.resolution;
        if self.last_resolution.is_some() && !analysis.has_random_access {
            resolution = None;
        }
        if current_stream_info
            .programs
            .iter()
            .any(|p| !p.video_streams.is_empty())
            && include_resolution
            && resolution.is_none()
            && self.last_resolution.is_none()
            && self.resolution_probe_remaining > 0
        {
            self.resolution_probe_remaining = self.resolution_probe_remaining.saturating_sub(1);
        }

        let mut split_reason: Option<SplitReason> = None;

        // Compare with previous stream information
        if let Some(previous_analysis) = &self.last_ts_analysis {
            let previous_info = &previous_analysis.stream_info;
            // Check for program changes
            if previous_info.program_count != current_stream_info.program_count {
                info!(
                    "{} Program count changed: {} -> {}",
                    self.context.name,
                    previous_info.program_count,
                    current_stream_info.program_count
                );
                split_reason = Some(SplitReason::StreamStructureChange {
                    description: format!(
                        "program count changed: {} -> {}",
                        previous_info.program_count, current_stream_info.program_count
                    ),
                });
            }

            // Check for transport stream ID changes
            if previous_info.transport_stream_id != current_stream_info.transport_stream_id {
                info!(
                    "{} Transport Stream ID changed: {} -> {}",
                    self.context.name,
                    previous_info.transport_stream_id,
                    current_stream_info.transport_stream_id
                );
                split_reason = Some(SplitReason::StreamStructureChange {
                    description: format!(
                        "transport stream ID changed: {} -> {}",
                        previous_info.transport_stream_id, current_stream_info.transport_stream_id
                    ),
                });
            }

            // Compare stream layouts within programs
            if split_reason.is_none()
                && previous_info.programs.len() != current_stream_info.programs.len()
            {
                info!(
                    "{} Number of programs changed: {} -> {}",
                    self.context.name,
                    previous_info.programs.len(),
                    current_stream_info.programs.len()
                );
                split_reason = Some(SplitReason::StreamStructureChange {
                    description: format!(
                        "number of programs changed: {} -> {}",
                        previous_info.programs.len(),
                        current_stream_info.programs.len()
                    ),
                });
            }

            // Check individual program changes
            if split_reason.is_none() {
                for (prev_prog, curr_prog) in previous_info
                    .programs
                    .iter()
                    .zip(current_stream_info.programs.iter())
                {
                    if prev_prog.program_number != curr_prog.program_number {
                        info!(
                            "{} Program number changed: {} -> {}",
                            self.context.name, prev_prog.program_number, curr_prog.program_number
                        );
                        split_reason = Some(SplitReason::StreamStructureChange {
                            description: format!(
                                "program number changed: {} -> {}",
                                prev_prog.program_number, curr_prog.program_number
                            ),
                        });
                        break;
                    }

                    // Check for codec changes in video streams
                    for (prev_stream, curr_stream) in prev_prog
                        .video_streams
                        .iter()
                        .zip(curr_prog.video_streams.iter())
                    {
                        if prev_stream.stream_type != curr_stream.stream_type {
                            info!(
                                "{} Video codec changed for program {}: {:?} -> {:?}",
                                self.context.name,
                                curr_prog.program_number,
                                prev_stream.stream_type,
                                curr_stream.stream_type
                            );
                            split_reason = Some(SplitReason::VideoCodecChange {
                                from: pipeline_common::VideoCodecInfo {
                                    codec: format!("{:?}", prev_stream.stream_type),
                                    profile: None,
                                    level: None,
                                    width: None,
                                    height: None,
                                    signature: 0,
                                },
                                to: pipeline_common::VideoCodecInfo {
                                    codec: format!("{:?}", curr_stream.stream_type),
                                    profile: None,
                                    level: None,
                                    width: None,
                                    height: None,
                                    signature: 0,
                                },
                            });
                            break;
                        }
                    }

                    // Check for codec changes in audio streams
                    if split_reason.is_none() {
                        for (prev_stream, curr_stream) in prev_prog
                            .audio_streams
                            .iter()
                            .zip(curr_prog.audio_streams.iter())
                        {
                            if prev_stream.stream_type != curr_stream.stream_type {
                                info!(
                                    "{} Audio codec changed for program {}: {:?} -> {:?}",
                                    self.context.name,
                                    curr_prog.program_number,
                                    prev_stream.stream_type,
                                    curr_stream.stream_type
                                );
                                split_reason = Some(SplitReason::AudioCodecChange {
                                    from: pipeline_common::AudioCodecInfo {
                                        codec: format!("{:?}", prev_stream.stream_type),
                                        sample_rate: None,
                                        channels: None,
                                        signature: 0,
                                    },
                                    to: pipeline_common::AudioCodecInfo {
                                        codec: format!("{:?}", curr_stream.stream_type),
                                        sample_rate: None,
                                        channels: None,
                                        signature: 0,
                                    },
                                });
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Update the baseline even if a codec/program change already selected a
        // split. Otherwise the following unchanged segment would split again.
        if let Some(current_resolution) = resolution {
            if split_reason.is_none()
                && let Some(previous) = self.last_resolution
                && previous != current_resolution
            {
                info!(
                    stream = %self.context.name,
                    from = %previous,
                    to = %current_resolution,
                    "Video resolution changed"
                );
                split_reason = Some(SplitReason::ResolutionChange {
                    from: (previous.width, previous.height),
                    to: (current_resolution.width, current_resolution.height),
                });
            }
            self.last_resolution = Some(current_resolution);
        }
        // The analysis owns metadata only, not the segment bytes. Sharing it
        // avoids cloning every program, stream, language and splice event.
        self.last_ts_analysis = Some(analysis);

        split_reason
    }

    // Reset operator state
    fn reset(&mut self) {
        self.last_init_segment_crc = None;
        self.last_ts_analysis = None;
        self.last_resolution = None;
        self.last_init_segment = None;
        self.resolution_probe_remaining = 50;
    }
}

impl Processor<HlsData> for SegmentSplitOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        let mut split_reason = None;

        // Check if we need to split based on segment type
        match &input {
            HlsData::M4sData(M4sData::InitSegment(init)) => {
                debug!("Init segment received");
                split_reason = self.handle_init_segment(init);
            }
            HlsData::TsData(segment) => {
                split_reason = self.handle_ts_segment(segment);
            }
            HlsData::EndMarker(_) => {
                // Reset state when we see an end marker
                self.reset();
            }
            _ => {}
        }

        // If we need to split, emit an end marker first
        if let Some(reason) = split_reason {
            debug!(
                "{} Emitting end marker for segment split",
                self.context.name
            );
            output(HlsData::end_marker_with_reason(reason))?;

            // If the split was triggered by a non-init segment, we need to re-emit the last init segment.
            if !matches!(&input, HlsData::M4sData(M4sData::InitSegment(_)))
                && let Some(init_segment) = &self.last_init_segment
            {
                output(HlsData::mp4_init(
                    init_segment.segment.clone(),
                    init_segment.data.clone(),
                ))?;
            }
        }

        // Always output the original input
        output(input)?;

        Ok(())
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        self.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "SegmentSplitter"
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::test_support::{INIT, OTHER_INIT, init};

    fn psi_packet(pid: u16, mut section: Vec<u8>) -> Vec<u8> {
        section.extend_from_slice(&ts::mpeg2_crc32(&section).to_be_bytes());
        let mut packet = vec![0xff; 188];
        packet[..5].copy_from_slice(&[0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10, 0]);
        packet[5..5 + section.len()].copy_from_slice(&section);
        packet
    }

    // PSI-only inputs exercise metadata comparison without pretending to carry
    // decodable video. PMT, video and audio use distinct PIDs and valid CRCs.
    fn tables(video: u8, audio: u8, program: u16) -> Vec<u8> {
        let [hi, lo] = program.to_be_bytes();
        [
            psi_packet(0, vec![0, 0xb0, 13, 0, 1, 0xc1, 0, 0, hi, lo, 0xf0, 0]),
            psi_packet(
                0x1000,
                vec![
                    2, 0xb0, 23, hi, lo, 0xc1, 0, 0, 0xe1, 0, 0xf0, 0, video, 0xe1, 0, 0xf0, 0,
                    audio, 0xe1, 1, 0xf0, 0,
                ],
            ),
        ]
        .concat()
    }

    fn ts(data: Vec<u8>) -> HlsData {
        HlsData::TsData(
            hls::TsSegmentData::new(MediaSegment::empty(), data.into())
                .with_crc_validation(true)
                .with_strict_continuity(true),
        )
    }

    fn video(width: u32, random_access: bool) -> HlsData {
        let bytes: &[u8] = match width {
            640 => include_bytes!("../../../hls/tests/fixtures/avc-640x352.ts"),
            1280 => include_bytes!("../../../hls/tests/fixtures/avc-1280x720.ts"),
            _ => panic!("unknown fixture"),
        };
        let mut data = bytes.to_vec();
        if !random_access {
            for packet in data.as_chunks_mut::<188>().0 {
                if packet[3] & 0x20 != 0 && packet[4] > 0 {
                    packet[5] &= !0x40;
                }
            }
        }
        ts(data)
    }

    fn process(items: Vec<HlsData>) -> Vec<HlsData> {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = SegmentSplitOperator::new(context.clone());
        let mut out = Vec::new();
        for item in items {
            operator
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }
        out
    }

    #[test]
    fn codec_changes_report_the_changed_codec_and_preserve_media() {
        for (video, audio) in [(0x24, 0x0f), (0x1b, 0x81)] {
            let before = tables(0x1b, 0x0f, 1);
            let after = tables(video, audio, 1);
            let out = process(vec![ts(before.clone()), ts(after.clone())]);
            assert_eq!(out.len(), 3);
            assert_eq!(out[0].as_ref(), before);
            assert_eq!(out[2].as_ref(), after);
            match &out[1] {
                HlsData::EndMarker(Some(SplitReason::VideoCodecChange { from, to }))
                    if video == 0x24 =>
                {
                    assert_eq!((&*from.codec, &*to.codec), ("H264", "H265"));
                }
                HlsData::EndMarker(Some(SplitReason::AudioCodecChange { from, to }))
                    if audio == 0x81 =>
                {
                    assert_eq!((&*from.codec, &*to.codec), ("AdtsAac", "Ac3"));
                }
                other => panic!("unexpected boundary: {other:?}"),
            }
        }
    }

    #[test]
    fn program_change_splits_once() {
        let out = process(vec![
            ts(tables(0x1b, 0x0f, 1)),
            ts(tables(0x1b, 0x0f, 2)),
            ts(tables(0x1b, 0x0f, 2)),
        ]);
        assert_eq!(out.len(), 4);
        assert!(
            matches!(&out[1], HlsData::EndMarker(Some(SplitReason::StreamStructureChange { description })) if description == "program number changed: 1 -> 2")
        );
        assert!(out[2..].iter().all(HlsData::is_ts));
    }

    #[test]
    fn resolution_change_updates_the_baseline_without_an_extra_split() {
        // Include an intervening segment without RAI: it must not erase the
        // baseline, and repeated 720p segments must not create extra files.
        let inputs = vec![
            video(640, true),
            video(1280, true),
            video(1280, false),
            video(1280, true),
            video(640, true),
        ];
        let expected_media: Vec<Bytes> = inputs
            .iter()
            .map(|item| item.data().unwrap().clone())
            .collect();
        let out = process(inputs);
        let reasons: Vec<_> = out
            .iter()
            .filter_map(|item| match item {
                HlsData::EndMarker(Some(SplitReason::ResolutionChange { from, to })) => {
                    Some((*from, *to))
                }
                _ => None,
            })
            .collect();
        assert_eq!(out.len(), 7);
        assert_eq!(
            reasons,
            [((640, 352), (1280, 720)), ((1280, 720), (640, 352))]
        );
        assert_eq!(
            out.iter()
                .filter_map(HlsData::data)
                .cloned()
                .collect::<Vec<_>>(),
            expected_media
        );
    }

    #[test]
    fn resolution_without_random_access_does_not_advance_the_baseline() {
        let out = process(vec![
            video(640, true),
            video(1280, false),
            video(1280, true),
        ]);
        assert_eq!(out.len(), 4);
        assert!(out[..2].iter().all(HlsData::is_ts));
        assert!(matches!(
            out[2],
            HlsData::EndMarker(Some(SplitReason::ResolutionChange {
                from: (640, 352),
                to: (1280, 720)
            }))
        ));
    }

    #[test]
    fn incomplete_psi_does_not_replace_the_last_program_layout() {
        let full = tables(0x1b, 0x0f, 1);
        let out = process(vec![ts(full.clone()), ts(full[..188].to_vec()), ts(full)]);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(HlsData::is_ts));
    }

    #[test]
    fn init_changes_split_before_the_new_header() {
        let out = process(vec![
            init(INIT),
            init(INIT),
            init(OTHER_INIT),
            init(OTHER_INIT),
        ]);
        assert_eq!(out.len(), 5);
        assert!(
            matches!(&out[2], HlsData::EndMarker(Some(SplitReason::StreamStructureChange { description })) if description == "init segment changed")
        );
        assert_eq!(out[3].as_ref(), OTHER_INIT);
        assert_eq!(out[4].as_ref(), OTHER_INIT);
    }
}
