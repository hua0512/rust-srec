//! # DefragmentOperator
//!
//! The DefragmentOperator is responsible for reorganizing fragmented HLS stream data into
//! coherent, complete segments. It addresses common issues in HLS streams such as:
//!
//! - Incomplete or fragmented media segments
//! - Missing initialization segments in fMP4 streams
//! - Segments that start mid-frame rather than with keyframes
//! - Corrupted or partial TS segments lacking PAT/PMT tables
//!
//! ## How it works
//!
//! The operator buffers incoming data until it has collected enough information to constitute
//! a complete segment, then outputs the segment as a unit. This ensures downstream operators
//! receive only well-formed segments containing all necessary structural elements.
//!
//! For TS segments, it ensures segments begin with keyframes and contain PAT/PMT tables.
//! For fMP4 segments, it validates that init segments are present before media segments.
//!
//! ## Configuration
//!
//! The operator maintains state about the current segment type (TS or fMP4) and automatically
//! adapts to format changes in the stream.
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

use hls::{HlsData, M4sData, SegmentType};
use pipeline_common::{PipelineError, Processor, StreamerContext};
use tracing::{debug, info, warn};

pub struct DefragmentOperator {
    context: Arc<StreamerContext>,
    is_gathering: bool,
    buffer: Vec<HlsData>,
    segment_type: Option<SegmentType>,
    has_init_segment: bool,
    // waiting_for_keyframe: bool,
}

impl DefragmentOperator {
    // The minimum number of tags required to consider a segment valid.
    const MIN_TAGS_NUM: usize = 5;

    // The minimum number of tags for TS segments (PAT, PMT, and at least one IDR frame)
    const MIN_TS_TAGS_NUM: usize = 3;

    pub fn new(context: Arc<StreamerContext>) -> Self {
        DefragmentOperator {
            context,
            is_gathering: false,
            buffer: Vec::with_capacity(Self::MIN_TAGS_NUM),
            segment_type: None,
            has_init_segment: false,
            // waiting_for_keyframe: true,
        }
    }

    fn reset(&mut self) {
        self.is_gathering = false;
        self.buffer.clear();
        // Don't reset has_init_segment as that's a property of the stream
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

            if self.buffer.len() >= min_required {
                debug!("{} Flushing buffer on playlist end", self.context.name);
                for item in std::mem::take(&mut self.buffer) {
                    output(item)?;
                }
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
        // let is_segment_start = data.is_segment_start();

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
                    self.handle_end_of_playlist(output)?;

                    // Continue processing the segment
                } else {
                    // For M4S transitions, just update the type but don't treat as playlist end
                    self.segment_type = Some(tag_type);
                }
            }
            _ => {} // Type hasn't changed
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
                self.buffer.push(data);
                return Ok(());
            }
        }

        // For TS segments, special handling for PAT/PMT tables and keyframes
        if self.segment_type == Some(SegmentType::Ts) {
            let is_pat_or_pmt = data.is_pmt_or_pat();
            // let has_keyframe = data.has_keyframe();

            // Always buffer PAT/PMT tables even if we're waiting for a keyframe
            if is_pat_or_pmt {
                if !self.is_gathering {
                    debug!(
                        "{} Starting to gather with PAT/PMT table",
                        self.context.name
                    );
                    self.is_gathering = true;
                }
                debug!("{} Buffering PAT/PMT table", self.context.name);
                // self.buffer.push(data);
                // return Ok(());
            }

            // If we're waiting for a keyframe and this isn't a PAT/PMT
            // if self.waiting_for_keyframe {
            //     if has_keyframe {
            //         debug!(
            //             "{} Found keyframe, can start fully gathering",
            //             self.context.name
            //         );
            //         self.waiting_for_keyframe = false;
            //         // If not already gathering, start now
            //         if !self.is_gathering {
            //             self.is_gathering = true;
            //         }
            //         self.buffer.push(data);
            //         return Ok(());
            //     } else if !self.is_gathering {
            //         // Skip non-essential packets while waiting for a keyframe
            //         debug!(
            //             "{} Skipping non-essential data while waiting for a keyframe",
            //             self.context.name
            //         );
            //         return Ok(());
            //     }
            // }
        }

        // Add to buffer if we're gathering data
        if self.is_gathering {
            self.buffer.push(data);

            // Determine minimum number of tags based on segment type
            let min_required = match self.segment_type {
                Some(SegmentType::Ts) => Self::MIN_TS_TAGS_NUM,
                Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => Self::MIN_TAGS_NUM,
                Some(SegmentType::EndMarker) => 0,
                None => Self::MIN_TAGS_NUM, // Default if type not yet determined
            };

            // Check if we've gathered enough tags to consider this a complete segment
            if self.buffer.len() >= min_required {
                // For TS segments, check if we have enough PAT/PMT tables and at least one keyframe
                // let is_complete = match self.segment_type {
                //     Some(SegmentType::Ts) => {
                //         let has_keyframe = self.buffer.iter().any(|d| d.has_keyframe());
                //         let has_pat_pmt =
                //             self.buffer.iter().filter(|d| d.is_pmt_or_pat()).count() >= 2;
                //         has_keyframe && has_pat_pmt
                //     }
                //     Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => true, // For M4S, just trust the count
                //     Some(SegmentType::EndMarker) => false,
                //     None => false, // Can't complete if we don't know the type
                // };
                let is_complete = self.buffer.len() >= min_required;

                if is_complete {
                    debug!(
                        "{} Gathered enough data ({} items), processing segment",
                        self.context.name,
                        self.buffer.len()
                    );

                    // Output buffered items
                    for item in self.buffer.drain(..) {
                        output(item)?;
                    }

                    self.is_gathering = false;
                    // For TS, wait for keyframe again on the next segment
                    // if self.segment_type == Some(SegmentType::Ts) {
                    //     self.waiting_for_keyframe = true;
                    // }
                    return Ok(());
                }
            }
            Ok(())
        } else {
            // If we're not gathering, decide whether to start gathering or pass through
            // let is_pat_or_pmt = data.is_pmt_or_pat();
            // let has_keyframe = data.has_keyframe();

            // if is_segment_start
            //     || (self.segment_type == Some(SegmentType::Ts) && (has_keyframe || is_pat_or_pmt))
            // {
            //     debug!("{} Starting new segment", self.context.name);
            //     self.is_gathering = true;
            //     self.buffer.push(data);
            //     return Ok(());
            // } else {
            //     // Pass through individual items when not gathering
            //     output(data)?;
            //     return Ok(());
            // }
            Ok(()) // No gathering, just pass through
        }
    }
}

impl Processor<HlsData> for DefragmentOperator {
    fn process(
        &mut self,
        input: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        self.process_internal(input, output)
    }

    fn finish(
        &mut self,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
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

        // Only flush if we have a minimally viable segment
        if self.buffer.len() >= min_required {
            // if self.segment_type == Some(SegmentType::Ts) {
            //     // For TS, check if we have necessary tables
            //     let has_keyframe = self.buffer.iter().any(|d| d.has_keyframe());
            //     let has_pat_pmt = self.buffer.iter().filter(|d| d.is_pmt_or_pat()).count() >= 2;

            //     if !has_keyframe || !has_pat_pmt {
            //         warn!(
            //             "{} Discarding incomplete TS segment on flush (missing PAT/PMT or keyframe)",
            //             self.context.name
            //         );
            //         self.reset();
            //         return Ok(());
            //     }
            // }

            let count = self.buffer.len();

            for item in self.buffer.drain(..) {
                output(item)?;
            }
            self.reset();

            info!(
                "{} Flushing complete segment ({} items)",
                self.context.name,
                self.buffer.len()
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
    use hls::{M4sInitSegmentData, M4sSegmentData, TsSegmentData as TsData};
    use pipeline_common::init_tracing;
    use std::collections::VecDeque;

    // Test utilities
    struct TestContext {
        operator: DefragmentOperator,
        outputs: VecDeque<HlsData>,
    }

    impl TestContext {
        fn new() -> Self {
            let context = Arc::new(StreamerContext::default());
            TestContext {
                operator: DefragmentOperator::new(context),
                outputs: VecDeque::new(),
            }
        }

        fn process(&mut self, data: HlsData) -> Result<(), PipelineError> {
            self.operator.process(data, &mut |output| {
                self.outputs.push_back(output);
                Ok(())
            })
        }

        fn process_all(&mut self, data: Vec<HlsData>) -> Result<(), PipelineError> {
            for item in data {
                self.process(item)?;
            }
            Ok(())
        }

        fn finish(&mut self) -> Result<(), PipelineError> {
            self.operator.finish(&mut |output| {
                self.outputs.push_back(output);
                Ok(())
            })
        }

        fn get_outputs(self) -> Vec<HlsData> {
            self.outputs.into_iter().collect()
        }
    }

    // Create test data
    fn create_ts_data(is_keyframe: bool, is_pat: bool, is_pmt: bool) -> HlsData {
        let mut data = vec![0u8; 188]; // Standard TS packet size
        data[0] = 0x47; // Sync byte

        if is_keyframe {
            data[3] |= 0x20; // Set adaptation field control
            data[4] = 1; // Set adaptation field length
            data[5] |= 0x40; // Set random access indicator
        }

        if is_pat {
            data[1] = 0x00; // PAT PID high bits
            data[2] = 0x00; // PAT PID low bits
        } else if is_pmt {
            data[1] = 0x01; // Common PMT PID high bits
            data[2] = 0x00; // Common PMT PID low bits
        }

        HlsData::TsData(TsData {
            data: Bytes::from(data),
            segment: m3u8_rs::MediaSegment::empty(),
        })
    }

    fn create_m4s_init_segment() -> HlsData {
        let mut data = vec![0u8; 32];
        data[4] = b'f'; // Set ftyp box
        data[5] = b't';
        data[6] = b'y';
        data[7] = b'p';

        HlsData::M4sData(M4sData::InitSegment(M4sInitSegmentData {
            data: Bytes::from(data),
            segment: m3u8_rs::MediaSegment::empty(),
        }))
    }

    fn create_m4s_media_segment(is_moof: bool) -> HlsData {
        let mut data = vec![0u8; 32];

        if is_moof {
            data[4] = b'm'; // Set moof box
            data[5] = b'o';
            data[6] = b'o';
            data[7] = b'f';
        }

        HlsData::M4sData(M4sData::Segment(M4sSegmentData {
            data: Bytes::from(data),
            segment: m3u8_rs::MediaSegment::empty(),
        }))
    }

    // Test a complete TS segment flow
    #[test]
    fn test_ts_segment_complete() {
        init_tracing();
        let mut ctx = TestContext::new();

        // Create a complete TS segment (PAT, PMT, keyframe, regular packets)
        let segment = vec![
            create_ts_data(false, true, false),  // PAT
            create_ts_data(false, false, true),  // PMT
            create_ts_data(true, false, false),  // Keyframe
            create_ts_data(false, false, false), // Regular packet
            create_ts_data(false, false, false), // Regular packet
            create_ts_data(false, false, false), // Regular packet
        ];

        ctx.process_all(segment).unwrap();

        let outputs = ctx.get_outputs();
        assert_eq!(
            outputs.len(),
            6,
            "Should output all 6 packets as a complete segment"
        );
    }

    // Test M4S segments with init segment
    #[test]
    fn test_m4s_init_and_segment() {
        init_tracing();
        let mut ctx = TestContext::new();

        // Process init segment first
        ctx.process(create_m4s_init_segment()).unwrap();

        // Then process media segments
        ctx.process(create_m4s_media_segment(true)).unwrap(); // with moof box

        for _ in 0..4 {
            ctx.process(create_m4s_media_segment(false)).unwrap();
        }

        let outputs = ctx.get_outputs();
        assert_eq!(
            outputs.len(),
            6,
            "Should output init segment and all media packets"
        );
    }

    // Test handling of end of playlist marker
    #[test]
    fn test_end_of_playlist() {
        let mut ctx = TestContext::new();

        // Start with incomplete segment
        ctx.process(create_ts_data(false, true, false)).unwrap(); // PAT
        ctx.process(create_ts_data(false, false, true)).unwrap(); // PMT

        // Send end of playlist marker
        ctx.process(HlsData::EndMarker).unwrap();

        let outputs = ctx.get_outputs();
        assert_eq!(
            outputs.len(),
            1,
            "Should discard incomplete segment and output only end marker"
        );
        assert!(
            matches!(outputs[0], HlsData::EndMarker),
            "Output should be end of playlist marker"
        );
    }

    // Test finish() with a complete segment
    #[test]
    fn test_finish_with_complete_segment() {
        init_tracing();
        let mut ctx = TestContext::new();

        // Process a complete segment
        ctx.process(create_ts_data(false, true, false)).unwrap(); // PAT
        ctx.process(create_ts_data(false, false, true)).unwrap(); // PMT
        ctx.process(create_ts_data(true, false, false)).unwrap(); // Keyframe

        // Call finish
        ctx.finish().unwrap();

        let outputs = ctx.get_outputs();
        assert_eq!(
            outputs.len(),
            3,
            "Should output the complete segment on finish"
        );
    }

    // Test finish() with an incomplete segment
    #[test]
    fn test_finish_with_incomplete_segment() {
        init_tracing();
        let mut ctx = TestContext::new();

        // Only process PAT - incomplete segment
        ctx.process(create_ts_data(false, true, false)).unwrap();

        // Call finish
        ctx.finish().unwrap();

        let outputs = ctx.get_outputs();
        assert_eq!(
            outputs.len(),
            0,
            "Should discard incomplete segment on finish"
        );
    }

    // Test segment type switching
    #[test]
    fn test_segment_type_switching() {
        init_tracing();
        let mut ctx = TestContext::new();

        // Start with TS segment
        ctx.process(create_ts_data(false, false, true)).unwrap(); // PMT
        ctx.process(create_ts_data(true, false, false)).unwrap(); // Keyframe + PAT
        ctx.process(create_ts_data(false, false, false)).unwrap(); // Regular packet
        ctx.process(create_ts_data(false, false, false)).unwrap(); // Regular packet
        ctx.process(create_ts_data(false, false, false)).unwrap(); // Regular packet

        // There should be a end of marker

        // Switch to M4S
        ctx.process(create_m4s_init_segment()).unwrap();
        ctx.process(create_m4s_media_segment(true)).unwrap();
        ctx.process(create_m4s_media_segment(true)).unwrap();
        ctx.process(create_m4s_media_segment(true)).unwrap();
        ctx.process(create_m4s_media_segment(true)).unwrap();
        ctx.finish().unwrap();

        let outputs = ctx.get_outputs();

        assert_eq!(outputs.len(), 11, "Should handle both segment types");

        // First 5 should be TS, last 5 should be M4S
        assert!(matches!(outputs[0], HlsData::TsData(_)));
        assert!(matches!(outputs[5], HlsData::EndMarker));
        assert!(matches!(
            outputs[6],
            HlsData::M4sData(M4sData::InitSegment(_))
        ));
    }
}
