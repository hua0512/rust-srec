//! # DefragmentOperator
//!
//! The DefragmentOperator is responsible for reorganizing fragmented HLS stream data into
//! coherent, complete segments. It addresses common issues in HLS streams such as:
//!
//! - Incomplete or fragmented media segments
//! - Missing initialization segments in fMP4 streams
//! - Corrupted or partial TS segments lacking PAT/PMT tables
//!
//! ## How it works
//!
//! The operator buffers incoming data until it has collected enough information to constitute
//! a complete segment, then outputs the segment as a unit. This ensures downstream operators
//! receive only well-formed segments containing all necessary structural elements.
//!
//! For TS segments, it uses optimized zero-copy parsing to validate PSI tables and stream
//! completeness. For fMP4 segments, it validates that init segments are present before media segments.
//!
//! ## Configuration
//!
//! The operator maintains state about the current segment type (TS or fMP4) and automatically
//! adapts to format changes in the stream. It leverages stream profiling for intelligent
//! segment validation.
//!
//! ## License
//!
//! MIT License
//!
//! ## Authors
//!
//! - hua0512
//!
use std::sync::Arc;

use hls::{HlsData, M4sData, SegmentType, StreamProfile};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::{debug, info, warn};

pub struct DefragmentOperator {
    context: Arc<StreamerContext>,
    is_gathering: bool,
    buffer: Vec<HlsData>,
    segment_type: Option<SegmentType>,
    has_init_segment: bool,
    last_stream_profile: Option<StreamProfile>,
    ts_psi_seen: bool,
}

impl DefragmentOperator {
    // The minimum number of tags required to consider a segment valid.
    const MIN_TAGS_NUM: usize = 5;

    // The minimum number of tags for TS segments (PAT, PMT, and at least one IDR frame)
    const MIN_TS_TAGS_NUM: usize = 3;

    // Maximum buffer size to prevent indefinite growth
    const MAX_BUFFER_SIZE: usize = 50;

    pub fn new(context: Arc<StreamerContext>) -> Self {
        DefragmentOperator {
            context,
            is_gathering: false,
            buffer: Vec::with_capacity(Self::MIN_TAGS_NUM),
            segment_type: None,
            has_init_segment: false,
            last_stream_profile: None,
            ts_psi_seen: false,
        }
    }

    fn reset(&mut self) {
        self.is_gathering = false;
        self.buffer.clear();
        // Don't reset has_init_segment or last_stream_profile as they're properties of the stream
    }

    fn flush_buffer(
        &mut self,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        for item in self.buffer.drain(..) {
            output(item)?;
        }
        Ok(())
    }

    fn process_ts_segment(
        &mut self,
        data: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // NOTE: Some live streams may emit TS segments that don't contain PAT/PMT (PSI) tables,
        // especially around join points or due to how segment boundaries are cut.
        //
        // Correctness policy: never drop those segments just because PSI is missing.
        // We also deliberately do not emit `HlsData::EndMarker` purely because PSI is missing:
        // absence of PSI is not a reliable boundary signal on live streams.
        //
        // Future option (not implemented): optionally split at the first PSI segment or inject PSI
        // so that each output file starts with PAT/PMT for maximal standalone playability.
        let has_psi_tables = data.ts_has_psi_tables();
        if has_psi_tables {
            debug!(
                "{} Found PSI tables (PAT/PMT) in TS stream",
                self.context.name
            );

            if let Some(profile) = data.get_stream_profile() {
                debug!(
                    "{} Stream profile: {} (complete: {})",
                    self.context.name,
                    profile.summary,
                    profile.is_complete()
                );
                self.last_stream_profile = Some(profile);
            }

            self.ts_psi_seen = true;
        }

        output(data)?;
        Ok(())
    }

    /// Validates if the buffered TS segment is complete using zero-copy stream analysis
    fn validate_ts_segment_completeness(&self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }

        // Check if we have any segments with PSI tables
        let has_psi_segments = self.buffer.iter().any(|data| data.ts_has_psi_tables());

        if !has_psi_segments {
            debug!(
                "{} TS segment lacks PSI tables, incomplete",
                self.context.name
            );
            return false;
        }

        // For advanced validation, check if we have a complete stream profile
        if let Some(ref profile) = self.last_stream_profile {
            debug!(
                "{} TS segment validation: {} - has_video: {}, has_audio: {}",
                self.context.name, profile.summary, profile.has_video, profile.has_audio
            );

            // Consider segment complete if we have either video OR audio streams (more lenient)
            // OR if we have enough buffer items regardless of stream completeness
            return profile.has_video
                || profile.has_audio
                || self.buffer.len() >= Self::MIN_TAGS_NUM;
        }

        // Fallback: if we have PSI tables and minimum packets, consider complete
        debug!("{} TS segment basic validation passed", self.context.name);
        true
    }

    // Handle cases for FMP4s init segment
    fn handle_new_header(&mut self, data: HlsData) {
        if !self.buffer.is_empty() {
            warn!(
                "{} Discarded {} items, total size: {}",
                self.context.name,
                self.buffer.len(),
                self.buffer.iter().map(|d| d.size()).sum::<usize>()
            );
            self.reset();
        }
        self.is_gathering = true;
        self.buffer.push(data);
        self.has_init_segment = true;
        debug!(
            "{} Received init segment, start gathering...",
            self.context.name
        );
    }

    // Handle end of playlist
    fn handle_end_of_playlist(
        &mut self,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} End of playlist marker received", self.context.name);

        // Flush any buffered data
        if !self.buffer.is_empty() {
            let min_required = match self.segment_type {
                Some(SegmentType::Ts) => Self::MIN_TS_TAGS_NUM,
                Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => Self::MIN_TAGS_NUM,
                Some(SegmentType::EndMarker) => 0,
                None => Self::MIN_TAGS_NUM,
            };

            if matches!(self.segment_type, Some(SegmentType::Ts)) {
                if self.buffer.len() < min_required {
                    warn!(
                        "{} Flushing short TS buffer on playlist end ({} < {} items) to preserve data",
                        self.context.name,
                        self.buffer.len(),
                        min_required
                    );
                } else {
                    debug!("{} Flushing TS buffer on playlist end", self.context.name);
                }
                self.flush_buffer(output)?;
                self.reset();
            } else if self.buffer.len() >= min_required {
                debug!("{} Flushing buffer on playlist end", self.context.name);
                self.flush_buffer(output)?;
                self.reset();
            } else {
                warn!(
                    "{} Discarding incomplete segment on playlist end ({} items)",
                    self.context.name,
                    self.buffer.len()
                );
                self.reset();
            }
        }

        // Output the end of playlist marker
        output(HlsData::EndMarker)?;
        Ok(())
    }

    fn process_internal(
        &mut self,
        data: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Handle end of playlist marker
        if matches!(data, HlsData::EndMarker) {
            return self.handle_end_of_playlist(output);
        }

        // Determine segment type
        let tag_type = data.segment_type();

        match self.segment_type {
            None => {
                // First segment we've seen, just set the type
                info!(
                    "{} Stream segment type detected as {:?}",
                    self.context.name, tag_type
                );
                self.segment_type = Some(tag_type);
                if tag_type == SegmentType::Ts {
                    self.ts_psi_seen = false;
                }
            }
            Some(current_type) if current_type != tag_type => {
                // Special case: don't consider M4sInit to M4sMedia (or vice versa) as changing segment type
                let is_m4s_transition = (current_type == SegmentType::M4sInit
                    && tag_type == SegmentType::M4sMedia)
                    || (current_type == SegmentType::M4sMedia && tag_type == SegmentType::M4sInit);

                if !is_m4s_transition {
                    info!(
                        "{} Stream segment type changed from {:?} to {:?}",
                        self.context.name, current_type, tag_type
                    );
                    self.segment_type = Some(tag_type);
                    if tag_type == SegmentType::Ts {
                        self.ts_psi_seen = false;
                    }

                    // Consider it at end of playlist marker
                    self.handle_end_of_playlist(output)?;

                    // Continue processing the segment
                } else {
                    // For M4S transitions, just update the type but don't treat as playlist end
                    self.segment_type = Some(tag_type);
                }
            }
            _ => {} // Type hasn't changed
        }

        if self.segment_type == Some(SegmentType::Ts) {
            return self.process_ts_segment(data, output);
        }

        // Special handling for M4S initialization segments
        if data.is_init_segment() {
            self.handle_new_header(data);
            return Ok(());
        }

        // For M4S segments, wait for init segment if we haven't seen one
        if (self.segment_type == Some(SegmentType::M4sInit)
            || self.segment_type == Some(SegmentType::M4sMedia))
            && !self.has_init_segment
        {
            // If this is an M4S segment but we haven't seen an init segment yet
            if let HlsData::M4sData(M4sData::Segment(_)) = &data {
                debug!(
                    "{} Buffering M4S segment while waiting for init segment",
                    self.context.name
                );
                // Buffer the segment, don't output yet
                if self.buffer.is_empty() {
                    self.is_gathering = true;
                }

                // Clean up the buffer if it's too large
                if self.buffer.len() >= Self::MAX_BUFFER_SIZE {
                    warn!(
                        "{} Buffer too large, discarding incomplete segment while waiting for init segment",
                        self.context.name
                    );
                    self.buffer.clear();
                }
                self.buffer.push(data);
                return Ok(());
            }
        }

        // For non-TS segments, add to buffer if we're gathering data
        if self.is_gathering {
            self.buffer.push(data);
        } else {
            // If we're not gathering, pass through the data
            output(data)?;
            return Ok(());
        }

        // // Check buffer size and force emission if too large
        // if self.buffer.len() >= Self::MAX_BUFFER_SIZE {
        //     warn!(
        //         "{} Buffer size limit reached ({}), force emitting",
        //         self.context.name, Self::MAX_BUFFER_SIZE
        //     );

        //     // Force emit all buffered items
        //     for item in self.buffer.drain(..) {
        //         output(item)?;
        //     }
        //     self.is_gathering = false;
        //     return Ok(());
        // }

        // Check if we've gathered enough data and if gathering is active
        if self.is_gathering && !self.buffer.is_empty() {
            // Determine minimum number of tags based on segment type
            let min_required = match self.segment_type {
                Some(SegmentType::Ts) => Self::MIN_TS_TAGS_NUM,
                Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => Self::MIN_TAGS_NUM,
                Some(SegmentType::EndMarker) => 0,
                None => Self::MIN_TAGS_NUM, // Default if type not yet determined
            };

            // Check if we've gathered enough tags to consider this a complete segment
            if self.buffer.len() >= min_required {
                // Enhanced completion check using stream profiling
                let is_complete = match self.segment_type {
                    Some(SegmentType::Ts) => {
                        // Use advanced stream analysis for TS segments
                        // self.validate_ts_segment_completeness()
                        true
                    }
                    Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => {
                        // For M4S, check if we have init segment for media segments
                        if !self.has_init_segment
                            && matches!(self.segment_type, Some(SegmentType::M4sMedia))
                        {
                            false
                        } else {
                            self.buffer.len() >= min_required
                        }
                    }
                    Some(SegmentType::EndMarker) => false,
                    None => false, // Can't complete if we don't know the type
                };

                if is_complete {
                    debug!(
                        "{} Gathered complete segment ({} items), processing",
                        self.context.name,
                        self.buffer.len()
                    );

                    // Output buffered items
                    for item in self.buffer.drain(..) {
                        output(item)?;
                    }

                    self.is_gathering = false;
                }
            }
        }

        Ok(())
    }
}

impl Processor<HlsData> for DefragmentOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }
        self.process_internal(input, output)
    }

    fn finish(
        &mut self,
        context: &Arc<StreamerContext>,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            debug!("Cancellation requested during finish, attempting to flush buffer.");
        }

        if self.buffer.is_empty() {
            return Ok(());
        }

        debug!(
            "{} Flushing buffered data ({} items)",
            self.context.name,
            self.buffer.len()
        );

        // Determine minimum requirements based on segment type
        let min_required = match self.segment_type {
            Some(SegmentType::Ts) => Self::MIN_TS_TAGS_NUM,
            Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => Self::MIN_TAGS_NUM,
            Some(SegmentType::EndMarker) => 0,
            None => Self::MIN_TAGS_NUM, // Default if type not yet determined
        };

        // Enhanced segment validation before flushing
        let is_valid_segment = match self.segment_type {
            Some(SegmentType::Ts) => {
                if !self.ts_psi_seen {
                    warn!(
                        "{} Finishing TS stream without PSI; flushing {} buffered segment(s) to preserve data",
                        self.context.name,
                        self.buffer.len()
                    );
                    true
                } else {
                    // For TS segments in gathering mode, we've already validated the stream
                    // and established it's valid, so we can be more lenient with the final flush
                    if self.is_gathering {
                        // Still validate completeness, but allow smaller buffers if we have PSI tables
                        self.validate_ts_segment_completeness()
                    } else {
                        // Not yet gathering, need full validation
                        self.buffer.len() >= min_required && self.validate_ts_segment_completeness()
                    }
                }
            }
            Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => {
                self.buffer.len() >= min_required
            }
            _ => false,
        };

        if is_valid_segment {
            let count = self.buffer.len();

            for item in self.buffer.drain(..) {
                if context.token.is_cancelled() {
                    warn!(
                        "{} Cancellation occurred during flush, some data might be lost.",
                        self.context.name
                    );
                    return Err(PipelineError::Cancelled);
                }
                output(item)?;
            }
            self.reset();

            info!(
                "{} Flushed complete segment ({} items)",
                self.context.name, count
            );
        } else {
            warn!(
                "{} Discarding incomplete segment on flush ({} items)",
                self.context.name,
                self.buffer.len()
            );
            self.reset();
        }

        Ok(())
    }

    fn name(&self) -> &'static str {
        "Defragment"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use m3u8_rs::MediaSegment;
    use pipeline_common::StreamerContext;
    use tokio_util::sync::CancellationToken;

    fn make_ts_segment_without_psi() -> HlsData {
        let mut data = vec![0u8; 188 * 2];
        data[0] = 0x47;
        data[188] = 0x47;
        HlsData::ts(MediaSegment::empty(), Bytes::from(data))
    }

    fn make_ts_segment_with_pat_pmt() -> HlsData {
        fn make_pat_packet(pmt_pid: u16) -> [u8; 188] {
            let mut packet = [0xFFu8; 188];
            packet[0] = 0x47;
            packet[1] = 0x40; // PUSI=1, PID=0
            packet[2] = 0x00;
            packet[3] = 0x10; // payload only

            let section_length: u16 = 13;
            let mut i = 4;
            packet[i] = 0x00; // pointer_field
            i += 1;
            packet[i] = 0x00; // table_id
            i += 1;
            packet[i] = 0xB0 | ((section_length >> 8) as u8 & 0x0F);
            i += 1;
            packet[i] = (section_length & 0xFF) as u8;
            i += 1;
            packet[i] = 0x00;
            packet[i + 1] = 0x01; // transport_stream_id
            i += 2;
            packet[i] = 0xC1; // version=0, current_next=1
            i += 1;
            packet[i] = 0x00; // section_number
            i += 1;
            packet[i] = 0x00; // last_section_number
            i += 1;
            packet[i] = 0x00;
            packet[i + 1] = 0x01; // program_number=1
            i += 2;
            packet[i] = 0xE0 | ((pmt_pid >> 8) as u8 & 0x1F);
            packet[i + 1] = (pmt_pid & 0xFF) as u8;
            i += 2;
            packet[i..i + 4].copy_from_slice(&[0, 0, 0, 0]); // CRC32 placeholder
            // Compute real MPEG-2 CRC-32 over the section (table_id through before CRC)
            let section_start = 5; // after pointer_field
            let crc = ts::mpeg2_crc32(&packet[section_start..i]);
            packet[i..i + 4].copy_from_slice(&crc.to_be_bytes());
            packet
        }

        fn make_pmt_packet(program_number: u16, pmt_pid: u16, video_pid: u16) -> [u8; 188] {
            let mut packet = [0xFFu8; 188];
            packet[0] = 0x47;
            packet[1] = 0x40 | ((pmt_pid >> 8) as u8 & 0x1F); // PUSI=1
            packet[2] = (pmt_pid & 0xFF) as u8;
            packet[3] = 0x10; // payload only

            let section_length: u16 = 18;
            let mut i = 4;
            packet[i] = 0x00; // pointer_field
            i += 1;
            packet[i] = 0x02; // table_id
            i += 1;
            packet[i] = 0xB0 | ((section_length >> 8) as u8 & 0x0F);
            i += 1;
            packet[i] = (section_length & 0xFF) as u8;
            i += 1;
            packet[i] = (program_number >> 8) as u8;
            packet[i + 1] = (program_number & 0xFF) as u8;
            i += 2;
            packet[i] = 0xC1; // version=0, current_next=1
            i += 1;
            packet[i] = 0x00; // section_number
            i += 1;
            packet[i] = 0x00; // last_section_number
            i += 1;
            packet[i] = 0xE0 | ((video_pid >> 8) as u8 & 0x1F); // PCR PID = video_pid
            packet[i + 1] = (video_pid & 0xFF) as u8;
            i += 2;
            packet[i] = 0xF0; // program_info_length = 0
            packet[i + 1] = 0x00;
            i += 2;
            packet[i] = 0x1B; // stream_type H.264
            i += 1;
            packet[i] = 0xE0 | ((video_pid >> 8) as u8 & 0x1F);
            packet[i + 1] = (video_pid & 0xFF) as u8;
            i += 2;
            packet[i] = 0xF0; // ES_info_length = 0
            packet[i + 1] = 0x00;
            i += 2;
            packet[i..i + 4].copy_from_slice(&[0, 0, 0, 0]); // CRC32 placeholder
            // Compute real MPEG-2 CRC-32 over the section (table_id through before CRC)
            let section_start = 5; // after pointer_field
            let crc = ts::mpeg2_crc32(&packet[section_start..i]);
            packet[i..i + 4].copy_from_slice(&crc.to_be_bytes());
            packet
        }

        let pat = make_pat_packet(0x0100);
        let pmt = make_pmt_packet(1, 0x0100, 0x0101);
        let mut data = Vec::with_capacity(188 * 2);
        data.extend_from_slice(&pat);
        data.extend_from_slice(&pmt);
        HlsData::ts(MediaSegment::empty(), Bytes::from(data))
    }

    #[test]
    fn passes_through_ts_without_psi_and_no_split_at_first_psi() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::new(context.clone());

        let mut out = Vec::new();
        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_ts_segment_without_psi(), &mut output)
                .unwrap();
            operator
                .process(&context, make_ts_segment_without_psi(), &mut output)
                .unwrap();
        }
        assert_eq!(out.len(), 2);

        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_ts_segment_with_pat_pmt(), &mut output)
                .unwrap();
        }

        assert_eq!(out.len(), 3);
        assert!(matches!(out[0], HlsData::TsData(_)));
        assert!(matches!(out[1], HlsData::TsData(_)));
        assert!(matches!(out[2], HlsData::TsData(_)));
    }

    #[test]
    fn flushes_ts_without_psi_on_playlist_end() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::new(context.clone());

        let mut out = Vec::new();
        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_ts_segment_without_psi(), &mut output)
                .unwrap();
        }
        assert_eq!(out.len(), 1);

        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, HlsData::end_marker(), &mut output)
                .unwrap();
        }

        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], HlsData::TsData(_)));
        assert!(matches!(out[1], HlsData::EndMarker));
    }
}
