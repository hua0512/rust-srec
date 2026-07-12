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

use hls::{HlsData, M4sData, SegmentType, SplitReason};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::{debug, info, warn};

pub struct DefragmentOperator {
    context: Arc<StreamerContext>,
    is_gathering: bool,
    buffer: Vec<HlsData>,
    buffered_bytes: usize,
    pre_init_buffer_limit: usize,
    pre_init_overflowed: bool,
    segment_type: Option<SegmentType>,
    has_init_segment: bool,
}

impl DefragmentOperator {
    // The minimum number of tags required to consider a segment valid.
    const MIN_TAGS_NUM: usize = 5;

    // The minimum number of tags for TS segments (PAT, PMT, and at least one IDR frame)
    const MIN_TS_TAGS_NUM: usize = 3;

    const DEFAULT_PRE_INIT_BUFFER_LIMIT: usize = 64 * 1024 * 1024;

    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self::with_pre_init_buffer_limit(context, Self::DEFAULT_PRE_INIT_BUFFER_LIMIT)
    }

    fn with_pre_init_buffer_limit(
        context: Arc<StreamerContext>,
        pre_init_buffer_limit: usize,
    ) -> Self {
        DefragmentOperator {
            context,
            is_gathering: false,
            buffer: Vec::with_capacity(Self::MIN_TAGS_NUM),
            buffered_bytes: 0,
            pre_init_buffer_limit: pre_init_buffer_limit.max(1),
            pre_init_overflowed: false,
            segment_type: None,
            has_init_segment: false,
        }
    }

    fn reset(&mut self) {
        self.is_gathering = false;
        self.buffer.clear();
        self.buffered_bytes = 0;
        self.pre_init_overflowed = false;
    }

    fn buffer_item(&mut self, data: HlsData) {
        self.buffered_bytes = self.buffered_bytes.saturating_add(data.size());
        self.buffer.push(data);
    }

    fn flush_buffer(
        &mut self,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let items = std::mem::take(&mut self.buffer);
        self.buffered_bytes = 0;
        for item in items {
            output(item)?;
        }
        Ok(())
    }

    fn process_ts_segment(
        &mut self,
        data: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        output(data)?;
        Ok(())
    }

    // Handle cases for FMP4s init segment
    fn handle_new_header(&mut self, data: HlsData) {
        if !self.buffer.is_empty() {
            warn!(
                "{} Discarded {} items, total size: {}",
                self.context.name,
                self.buffer.len(),
                self.buffered_bytes
            );
            self.reset();
        }
        self.is_gathering = true;
        self.pre_init_overflowed = false;
        self.buffer_item(data);
        self.has_init_segment = true;
        debug!(
            "{} Received init segment, start gathering...",
            self.context.name
        );
    }

    // Handle end of playlist
    fn handle_end_of_playlist(
        &mut self,
        reason: Option<SplitReason>,
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
        output(HlsData::EndMarker(reason))?;
        Ok(())
    }

    fn process_internal(
        &mut self,
        data: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // Handle end of playlist marker
        if let HlsData::EndMarker(reason) = data {
            return self.handle_end_of_playlist(reason, output);
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

                    // Consider it at end of playlist marker
                    self.handle_end_of_playlist(None, output)?;

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
            if self.pre_init_overflowed {
                warn!(
                    stream = %self.context.name,
                    "Delayed fMP4 init arrived after pre-init overflow; rotating output before the init segment"
                );
                output(HlsData::end_marker_with_reason(
                    SplitReason::StreamStructureChange {
                        description: "fMP4 init arrived after pre-init media overflow".to_string(),
                    },
                ))?;
            }
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
                if self.pre_init_overflowed {
                    output(data)?;
                    return Ok(());
                }

                debug!(
                    "{} Buffering M4S segment while waiting for init segment",
                    self.context.name
                );
                // Buffer the segment, don't output yet
                if self.buffer.is_empty() {
                    self.is_gathering = true;
                }

                let next_size = self.buffered_bytes.saturating_add(data.size());
                if next_size > self.pre_init_buffer_limit {
                    warn!(
                        stream = %self.context.name,
                        buffered_bytes = self.buffered_bytes,
                        incoming_bytes = data.size(),
                        limit_bytes = self.pre_init_buffer_limit,
                        "Pre-init buffer limit reached; emitting buffered media without an init segment"
                    );
                    self.pre_init_overflowed = true;
                    self.is_gathering = false;
                    self.flush_buffer(output)?;
                    output(data)?;
                    return Ok(());
                }
                self.buffer_item(data);
                return Ok(());
            }
        }

        // For non-TS segments, add to buffer if we're gathering data
        if self.is_gathering {
            self.buffer_item(data);
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

                    self.flush_buffer(output)?;
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
            Some(SegmentType::Ts) => true,
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

    fn make_m4s_media(size: usize) -> HlsData {
        HlsData::mp4_segment(MediaSegment::empty(), Bytes::from(vec![0; size]))
    }

    fn make_m4s_init(size: usize) -> HlsData {
        HlsData::mp4_init(MediaSegment::empty(), Bytes::from(vec![0; size]))
    }

    #[test]
    fn pre_init_byte_limit_emits_buffer_and_switches_to_passthrough() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::with_pre_init_buffer_limit(Arc::clone(&context), 6);
        let mut out = Vec::new();
        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_m4s_media(4), &mut output)
                .unwrap();
        }
        assert!(out.is_empty());

        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_m4s_media(4), &mut output)
                .unwrap();
        }
        assert_eq!(out.len(), 2);

        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            operator
                .process(&context, make_m4s_media(4), &mut output)
                .unwrap();
        }
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(HlsData::is_mp4_media));
    }

    #[test]
    fn delayed_init_after_pre_init_overflow_starts_a_new_output_file() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::with_pre_init_buffer_limit(Arc::clone(&context), 6);
        let mut out = Vec::new();
        let mut output = |item: HlsData| -> Result<(), PipelineError> {
            out.push(item);
            Ok(())
        };

        operator
            .process(&context, make_m4s_media(4), &mut output)
            .unwrap();
        operator
            .process(&context, make_m4s_media(4), &mut output)
            .unwrap();
        operator
            .process(&context, make_m4s_media(4), &mut output)
            .unwrap();
        operator
            .process(&context, make_m4s_init(4), &mut output)
            .unwrap();
        for _ in 0..4 {
            operator
                .process(&context, make_m4s_media(4), &mut output)
                .unwrap();
        }

        assert!(out[..3].iter().all(HlsData::is_mp4_media));
        assert!(matches!(
            &out[3],
            HlsData::EndMarker(Some(SplitReason::StreamStructureChange { .. }))
        ));
        assert!(out[4].is_mp4_init());
        assert!(out[5..].iter().all(HlsData::is_mp4_media));
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
        assert!(matches!(out[1], HlsData::EndMarker(_)));
    }
}
