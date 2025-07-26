use crc32fast::Hasher;
use hls::{HlsData, M4sData, M4sInitSegmentData, Resolution, StreamProfile, TsStreamInfo};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::sync::Arc;
use tracing::{debug, info, warn};

/// An operator that splits HLS segments when parameters change.
///
/// The SegmentSplitOperator performs deep inspection of stream metadata:
///
/// - MP4 initialization segment changes (different codecs, resolutions, etc.)
/// - TS segment stream changes (codec changes, program changes, stream layout changes)
/// - Stream parameter changes detected through stream profiles
/// - Video resolution changes detected through SPS parsing
///
/// When meaningful changes are detected, the operator inserts an end marker
/// to properly split the HLS stream.
pub struct SegmentSplitOperator {
    context: Arc<StreamerContext>,
    last_init_segment_crc: Option<u32>,
    last_stream_profile: Option<StreamProfile>,
    last_ts_stream_info: Option<TsStreamInfo>,
    last_resolution: Option<Resolution>,
    last_init_segment: Option<M4sInitSegmentData>,
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
            last_stream_profile: None,
            last_ts_stream_info: None,
            last_resolution: None,
            last_init_segment: None,
        }
    }

    // Calculate CRC32 for byte content
    fn calculate_crc(data: &[u8]) -> u32 {
        let mut hasher = Hasher::new();
        hasher.update(data);
        hasher.finalize()
    }

    // Handle MP4 init segment - returns true if a split is needed
    fn handle_init_segment(&mut self, input: &HlsData) -> Result<bool, PipelineError> {
        // Get data from HlsData
        let data = match input {
            HlsData::M4sData(M4sData::InitSegment(init)) => init,
            _ => {
                return Err(PipelineError::Processing(
                    "Expected MP4 init segment".to_string(),
                ));
            }
        };

        let crc = Self::calculate_crc(&data.data);
        let mut needs_split = false;

        if let Some(previous_crc) = self.last_init_segment_crc {
            if previous_crc != crc {
                info!(
                    "{} Detected different init segment, splitting the stream",
                    self.context.name
                );
                needs_split = true;
            }
        } else {
            // First init segment encountered
            info!("{} First init segment encountered", self.context.name);
        }

        // Always update to the latest init segment, since this is the only place we see them.
        self.last_init_segment = Some(data.clone());
        self.last_init_segment_crc = Some(crc);

        // Check for resolution changes in MP4 init segments
        // if !needs_split {
        //     if let Some(current_resolution) = self.extract_resolution(input) {
        //         if let Some(previous_resolution) = &self.last_resolution {
        //             if previous_resolution != &current_resolution {
        //                 info!(
        //                     "{} MP4 video resolution changed: {} -> {}",
        //                     self.context.name, previous_resolution, current_resolution
        //                 );
        //                 needs_split = true;
        //             }
        //         } else {
        //             // First time we detect resolution in MP4
        //             info!(
        //                 "{} Detected MP4 video resolution: {}",
        //                 self.context.name, current_resolution
        //             );
        //         }
        //         self.last_resolution = Some(current_resolution);
        //     }
        // }

        Ok(needs_split)
    }

    // Handle TS segment
    // Returns true if a split is needed
    fn handle_ts_segment(&mut self, input: &HlsData) -> Result<bool, PipelineError> {
        // Quick check if segment has PSI tables
        if !input.ts_has_psi_tables() {
            debug!(
                "{} TS segment has no PSI tables, skipping analysis",
                self.context.name
            );
            return Ok(false);
        }

        // Parse stream information
        let current_stream_info = match input.parse_ts_psi_tables_zero_copy() {
            Some(Ok(info)) => info,
            Some(Err(e)) => {
                warn!("{} Failed to parse TS PSI tables: {}", self.context.name, e);
                return Ok(false);
            }
            None => {
                debug!("{} Not a TS segment", self.context.name);
                return Ok(false);
            }
        };

        // Get current stream profile for comparison
        let current_profile = input.get_stream_profile();

        let mut needs_split = false;

        // Compare with previous stream information
        if let Some(previous_info) = &self.last_ts_stream_info {
            // Check for program changes
            if previous_info.program_count != current_stream_info.program_count {
                info!(
                    "{} Program count changed: {} -> {}",
                    self.context.name,
                    previous_info.program_count,
                    current_stream_info.program_count
                );
                needs_split = true;
            }

            // Check for transport stream ID changes
            if previous_info.transport_stream_id != current_stream_info.transport_stream_id {
                info!(
                    "{} Transport Stream ID changed: {} -> {}",
                    self.context.name,
                    previous_info.transport_stream_id,
                    current_stream_info.transport_stream_id
                );
                needs_split = true;
            }

            // Compare stream layouts within programs
            if !needs_split && previous_info.programs.len() != current_stream_info.programs.len() {
                info!(
                    "{} Number of programs changed: {} -> {}",
                    self.context.name,
                    previous_info.programs.len(),
                    current_stream_info.programs.len()
                );
                needs_split = true;
            }

            // Check individual program changes
            if !needs_split {
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
                        needs_split = true;
                        break;
                    }

                    if prev_prog.pcr_pid != curr_prog.pcr_pid {
                        info!(
                            "{} PCR PID changed for program {}: 0x{:04X} -> 0x{:04X}",
                            self.context.name,
                            curr_prog.program_number,
                            prev_prog.pcr_pid,
                            curr_prog.pcr_pid
                        );
                        needs_split = true;
                        break;
                    }

                    // Check stream count changes
                    let prev_stream_count = prev_prog.video_streams.len()
                        + prev_prog.audio_streams.len()
                        + prev_prog.other_streams.len();
                    let curr_stream_count = curr_prog.video_streams.len()
                        + curr_prog.audio_streams.len()
                        + curr_prog.other_streams.len();

                    if prev_stream_count != curr_stream_count {
                        info!(
                            "{} Stream count changed for program {}: {} -> {}",
                            self.context.name,
                            curr_prog.program_number,
                            prev_stream_count,
                            curr_stream_count
                        );
                        needs_split = true;
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
                            needs_split = true;
                            break;
                        }
                    }

                    // Check for codec changes in audio streams
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
                            needs_split = true;
                            break;
                        }
                    }
                }
            }
        }

        // Compare stream profiles for high-level changes
        if let (Some(current_profile), Some(previous_profile)) =
            (&current_profile, &self.last_stream_profile)
        {
            if !needs_split {
                // Check for codec changes
                if current_profile.has_h264 != previous_profile.has_h264
                    || current_profile.has_h265 != previous_profile.has_h265
                    || current_profile.has_aac != previous_profile.has_aac
                    || current_profile.has_ac3 != previous_profile.has_ac3
                {
                    info!("{} Stream codec availability changed", self.context.name);
                    needs_split = true;
                }

                // Check for stream type changes
                if current_profile.has_video != previous_profile.has_video
                    || current_profile.has_audio != previous_profile.has_audio
                {
                    info!(
                        "{} Stream type availability changed (video: {} -> {}, audio: {} -> {})",
                        self.context.name,
                        previous_profile.has_video,
                        current_profile.has_video,
                        previous_profile.has_audio,
                        current_profile.has_audio
                    );
                    needs_split = true;
                }

                // Check for resolution changes using StreamProfile
                if let (Some(current_res), Some(previous_res)) =
                    (&current_profile.resolution, &previous_profile.resolution)
                {
                    if current_res != previous_res {
                        info!(
                            "{} Video resolution changed via profile: {} -> {}",
                            self.context.name, previous_res, current_res
                        );
                        needs_split = true;
                    }
                }
            }
        }

        // Additional resolution change check (if video streams are present)
        if !needs_split
            && (current_stream_info
                .programs
                .iter()
                .any(|p| !p.video_streams.is_empty()))
        {
            if let Some(current_resolution) = current_profile.as_ref().and_then(|p| p.resolution) {
                if let Some(previous_resolution) = &self.last_resolution {
                    if previous_resolution != &current_resolution {
                        info!(
                            "{} Video resolution changed: {} -> {}",
                            self.context.name, previous_resolution, current_resolution
                        );
                        needs_split = true;
                    }
                } else {
                    // First time we detect resolution
                    info!(
                        "{} Detected video resolution: {}",
                        self.context.name, current_resolution
                    );
                }
                self.last_resolution = Some(current_resolution);
            }
        }

        // Update stored information
        self.last_ts_stream_info = Some(current_stream_info);
        if let Some(profile) = current_profile {
            self.last_stream_profile = Some(profile);
        }

        Ok(needs_split)
    }

    // Reset operator state
    fn reset(&mut self) {
        self.last_init_segment_crc = None;
        self.last_stream_profile = None;
        self.last_ts_stream_info = None;
        self.last_resolution = None;
        self.last_init_segment = None;
    }
}

impl Processor<HlsData> for SegmentSplitOperator {
    fn process(
        &mut self,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let mut need_split = false;

        // Check if we need to split based on segment type
        match &input {
            HlsData::M4sData(M4sData::InitSegment(_)) => {
                debug!("Init segment received");
                need_split = self.handle_init_segment(&input)?;
            }
            HlsData::TsData(_) => {
                need_split = self.handle_ts_segment(&input)?;
            }
            HlsData::EndMarker => {
                // Reset state when we see an end marker
                self.reset();
            }
            _ => {}
        }

        // If we need to split, emit an end marker first
        if need_split {
            debug!(
                "{} Emitting end marker for segment split",
                self.context.name
            );
            output(HlsData::end_marker())?;

            // If the split was triggered by a non-init segment, we need to re-emit the last init segment.
            if !matches!(&input, HlsData::M4sData(M4sData::InitSegment(_))) {
                if let Some(init_segment) = &self.last_init_segment {
                    output(HlsData::mp4_init(
                        init_segment.segment.clone(),
                        init_segment.data.clone(),
                    ))?;
                }
            }
        }

        // Always output the original input
        output(input)?;

        Ok(())
    }

    fn finish(
        &mut self,
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
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::init_test_tracing;

    // Helper function to create a working TS data with specific codec combinations
    fn create_ts_data_with_codecs(video_codec: u8, audio_codec: u8, program_num: u16) -> Vec<u8> {
        let mut ts_data = Vec::new();

        // PAT packet (188 bytes)
        let mut pat_packet = vec![0u8; 188];
        pat_packet[0] = 0x47; // Sync byte
        pat_packet[1] = 0x40; // PUSI set, PID = 0 (PAT)
        pat_packet[2] = 0x00;
        pat_packet[3] = 0x10; // No scrambling, payload only

        // Simple PAT payload
        pat_packet[4] = 0x00; // Pointer field
        pat_packet[5] = 0x00; // Table ID (PAT)
        pat_packet[6] = 0x80; // Section syntax indicator
        pat_packet[7] = 0x0D; // Section length (13 bytes)
        pat_packet[8] = 0x00;
        pat_packet[9] = 0x01; // Transport stream ID
        pat_packet[10] = 0x01; // Version 0 + current/next = 1
        pat_packet[11] = 0x00;
        pat_packet[12] = 0x00; // Section numbers
        // Program entry
        pat_packet[13] = (program_num >> 8) as u8;
        pat_packet[14] = (program_num & 0xFF) as u8;
        pat_packet[15] = 0xE1;
        pat_packet[16] = 0x00; // PMT PID 0x100

        // PMT packet (188 bytes)
        let mut pmt_packet = vec![0u8; 188];
        pmt_packet[0] = 0x47; // Sync byte
        pmt_packet[1] = 0x41; // PUSI set, PID = 0x100
        pmt_packet[2] = 0x00;
        pmt_packet[3] = 0x10; // No scrambling, payload only

        pmt_packet[4] = 0x00; // Pointer field
        pmt_packet[5] = 0x02; // Table ID (PMT)
        pmt_packet[6] = 0x80; // Section syntax indicator
        pmt_packet[7] = 0x17; // Section length (23 bytes for 2 streams)
        pmt_packet[8] = (program_num >> 8) as u8;
        pmt_packet[9] = (program_num & 0xFF) as u8;
        pmt_packet[10] = 0x01; // Version 0 + current/next = 1
        pmt_packet[11] = 0x00;
        pmt_packet[12] = 0x00; // Section numbers
        pmt_packet[13] = 0xE1;
        pmt_packet[14] = 0x00; // PCR PID 0x100
        pmt_packet[15] = 0x00;
        pmt_packet[16] = 0x00; // Program info length
        // Video stream
        pmt_packet[17] = video_codec;
        pmt_packet[18] = 0xE1;
        pmt_packet[19] = 0x00; // Elementary PID 0x100
        pmt_packet[20] = 0x00;
        pmt_packet[21] = 0x00; // ES info length
        // Audio stream
        pmt_packet[22] = audio_codec;
        pmt_packet[23] = 0xE1;
        pmt_packet[24] = 0x01; // Elementary PID 0x101
        pmt_packet[25] = 0x00;
        pmt_packet[26] = 0x00; // ES info length

        ts_data.extend_from_slice(&pat_packet);
        ts_data.extend_from_slice(&pmt_packet);
        ts_data
    }

    #[test]
    fn test_stream_change_detection() {
        let context = StreamerContext::arc_new();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Create initial TS segment with H.264 + AAC
        let ts_data1 = create_ts_data_with_codecs(0x1B, 0x0F, 1); // H.264 + AAC
        let ts_segment1 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data1),
        });

        // Process the initial segment
        operator.process(ts_segment1, &mut output_fn).unwrap();

        // Create second TS segment with H.265 + AC-3 (different codecs)
        let ts_data2 = create_ts_data_with_codecs(0x24, 0x81, 1); // H.265 + AC-3
        let ts_segment2 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data2),
        });

        // Process the modified segment
        operator.process(ts_segment2, &mut output_fn).unwrap();

        // Should have split the stream (segment1 + end marker + segment2)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker"),
        }
    }

    #[test]
    fn test_program_change_detection() {
        let context = StreamerContext::arc_new();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Create initial TS segment with program 1
        let ts_data1 = create_ts_data_with_codecs(0x1B, 0x0F, 1); // H.264 + AAC, program 1
        let ts_segment1 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data1),
        });

        // Process the initial segment
        operator.process(ts_segment1, &mut output_fn).unwrap();

        // Create second TS segment with program 2 (different program number)
        let ts_data2 = create_ts_data_with_codecs(0x1B, 0x0F, 2); // H.264 + AAC, program 2
        let ts_segment2 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data2),
        });

        // Process the segment with different program
        operator.process(ts_segment2, &mut output_fn).unwrap();

        // Should have split the stream (segment1 + end marker + segment2)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker"),
        }
    }

    #[test]
    fn test_resolution_change_detection() {
        init_test_tracing!();
        let context = StreamerContext::arc_new();
        let mut operator = SegmentSplitOperator::new(context);
        let mut output_items = Vec::new();

        // Create a mutable output function
        let mut output_fn = |item: HlsData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Create initial TS segment with H.264 (which typically defaults to 1920x1080)
        let ts_data1 = create_ts_data_with_codecs(0x1B, 0x0F, 1); // H.264 + AAC
        let ts_segment1 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data1),
        });

        // Process the initial segment
        operator.process(ts_segment1, &mut output_fn).unwrap();

        // Create second TS segment with H.265 (which typically defaults to 3840x2160)
        let ts_data2 = create_ts_data_with_codecs(0x24, 0x0F, 1); // H.265 + AAC
        let ts_segment2 = HlsData::TsData(hls::TsSegmentData {
            segment: MediaSegment::empty(),
            data: Bytes::from(ts_data2),
        });

        // Process the segment with different codec (and implied resolution)
        operator.process(ts_segment2, &mut output_fn).unwrap();

        // Should have split the stream due to both codec and resolution change
        // (segment1 + end marker + segment2)
        assert_eq!(output_items.len(), 3);
        match &output_items[1] {
            HlsData::EndMarker => {}
            _ => panic!("Expected EndMarker after resolution change"),
        }
    }
}
