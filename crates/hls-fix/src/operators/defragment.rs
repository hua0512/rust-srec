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
        }
    }

    fn reset(&mut self) {
        self.is_gathering = false;
        self.buffer.clear();
        // Don't reset has_init_segment or last_stream_profile as they're properties of the stream
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

        if self.segment_type == Some(SegmentType::Ts) && !self.is_gathering {
            // Check if this segment has PSI tables
            let has_psi_tables = data.ts_has_psi_tables();

            if has_psi_tables {
                debug!(
                    "{} Found PSI tables (PAT/PMT), start gathering",
                    self.context.name
                );
                self.is_gathering = true;

                // Get stream profile for this segment
                if let Some(profile) = data.get_stream_profile() {
                    debug!(
                        "{} Stream profile: {} (complete: {})",
                        self.context.name,
                        profile.summary,
                        profile.is_complete()
                    );
                    self.last_stream_profile = Some(profile);
                }

                self.buffer.push(data);
            } else {
                // For TS segments without PSI tables, only buffer if we're already gathering
                if !self.is_gathering {
                    debug!(
                        "{} Skipping TS data without PSI tables while not gathering",
                        self.context.name
                    );
                    return Ok(());
                }
                // Add to buffer if we're gathering data
                self.buffer.push(data);
            }
        } else if self.segment_type == Some(SegmentType::Ts) {
            // gathering, just output the data
            // consider it as a complete segment stream as we previously checked the stream profile
            // just keep this state forever
            if self.buffer.len() >= Self::MIN_TS_TAGS_NUM {
                for item in self.buffer.drain(..) {
                    output(item)?;
                }
            }
            self.buffer.push(data);
            return Ok(());
        } else {
            // For non-TS segments, add to buffer if we're gathering data
            if self.is_gathering {
                self.buffer.push(data);
            } else {
                // If we're not gathering, pass through the data
                output(data)?;
                return Ok(());
            }
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

        // Enhanced segment validation before flushing
        let is_valid_segment = match self.segment_type {
            Some(SegmentType::Ts) => {
                self.buffer.len() >= min_required && self.validate_ts_segment_completeness()
            }
            Some(SegmentType::M4sInit) | Some(SegmentType::M4sMedia) => {
                self.buffer.len() >= min_required
            }
            _ => false,
        };

        if is_valid_segment {
            let count = self.buffer.len();

            for item in self.buffer.drain(..) {
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
