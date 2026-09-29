//! # DefragmentOperator
//!
//! Buffers fMP4 initialization and media items and forwards TS items unchanged.
//! A new initialization section preserves media already paired with the preceding
//! section. Pre-init buffering is bounded; if the limit is exceeded, media is
//! emitted for recovery and a later init starts a new output sequence.
//!
//! This operator preserves item boundaries; it does not validate or repair the
//! encoded media within an item.
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
    // The minimum number of fMP4 items (init + trailing media) buffered before a gathered
    // segment is flushed mid-stream.
    const MIN_TAGS_NUM: usize = 5;

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
        self.buffered_bytes = 0;
        for item in self.buffer.drain(..) {
            output(item)?;
        }
        Ok(())
    }

    // EXT-X-MAP applies forward (RFC 8216 4.3.2.5). Media already gathered
    // with an init still belongs to that init, even if another arrives early.
    fn handle_new_header(
        &mut self,
        data: HlsData,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if !self.buffer.is_empty() {
            if self.has_init_segment {
                self.flush_buffer(output)?;
            } else {
                warn!(
                    stream = %self.context.name,
                    items = self.buffer.len(),
                    bytes = self.buffered_bytes,
                    "Discarding media received before its initialization was known"
                );
            }
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
        Ok(())
    }

    // Handle end of playlist
    fn handle_end_of_playlist(
        &mut self,
        reason: Option<SplitReason>,
        output: &mut dyn FnMut(HlsData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        debug!("{} End of playlist marker received", self.context.name);

        // The buffer only ever holds fMP4 items; process_internal forwards TS directly.
        // Flush everything gathered regardless of item count so a mid-stream discontinuity or
        // recording end never drops a freshly re-gathered init segment or its trailing media.
        if !self.buffer.is_empty() {
            debug!(
                "{} Flushing buffer on playlist end ({} items)",
                self.context.name,
                self.buffer.len()
            );
            self.flush_buffer(output)?;
            self.reset();
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
                    self.has_init_segment = false;

                    // Continue processing the segment
                } else {
                    // For M4S transitions, just update the type but don't treat as playlist end
                    self.segment_type = Some(tag_type);
                }
            }
            _ => {} // Type hasn't changed
        }

        // TS segments are passed through untouched; only fMP4 data is ever buffered.
        if self.segment_type == Some(SegmentType::Ts) {
            output(data)?;
            return Ok(());
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
            return self.handle_new_header(data, output);
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

        // Only fMP4 data reaches here (TS returned above). Gathering can also start
        // pre-init with media-only buffers, so the has_init_segment conjunct below is
        // what guarantees an emitted group opens with its init. Emit the init plus its
        // trailing media once MIN_TAGS_NUM items are buffered.
        if self.is_gathering && self.has_init_segment && self.buffer.len() >= Self::MIN_TAGS_NUM {
            debug!(
                "{} Gathered complete segment ({} items), processing",
                self.context.name,
                self.buffer.len()
            );

            self.flush_buffer(output)?;
            self.is_gathering = false;
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

        // The buffer only ever holds fMP4 items; flush whatever is gathered rather than
        // dropping a short final segment (init + trailing media) at stream end.
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
            "{} Flushed buffered segment ({} items)",
            self.context.name, count
        );

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

    use crate::test_support::{INIT, MEDIA0, MEDIA1, OTHER_INIT, OTHER_MEDIA, init, media};

    #[test]
    fn new_init_preserves_the_short_group_encoded_against_the_previous_init() {
        for (next_init, next_media) in [(INIT, MEDIA1), (OTHER_INIT, OTHER_MEDIA)] {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let mut operator = DefragmentOperator::new(context.clone());
            let mut out = Vec::new();
            for item in [
                init(INIT),
                media(MEDIA0),
                init(next_init),
                media(next_media),
            ] {
                operator
                    .process(&context, item, &mut |item| {
                        out.push(item);
                        Ok(())
                    })
                    .unwrap();
            }
            operator
                .finish(&context, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
            assert_eq!(out.len(), 4);
            assert!(out[0].is_mp4_init() && out[2].is_mp4_init());
            assert!(out[1].is_mp4_media() && out[3].is_mp4_media());
            assert_eq!(
                out.iter().map(AsRef::as_ref).collect::<Vec<&[u8]>>(),
                [INIT, MEDIA0, next_init, next_media]
            );
        }
    }

    #[test]
    fn first_init_does_not_relabel_media_with_an_unknown_configuration() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DefragmentOperator::new(context.clone());
        let mut out = Vec::new();
        for item in [media(MEDIA0), init(OTHER_INIT), media(OTHER_MEDIA)] {
            operator
                .process(&context, item, &mut |item| {
                    out.push(item);
                    Ok(())
                })
                .unwrap();
        }
        operator
            .finish(&context, &mut |item| {
                out.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            out.iter().map(AsRef::as_ref).collect::<Vec<&[u8]>>(),
            [OTHER_INIT, OTHER_MEDIA]
        );
    }

    fn make_ts_segment_without_psi() -> HlsData {
        let mut packet = vec![0xff; 188];
        packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]); // Null PID, payload only.
        HlsData::ts(MediaSegment::empty(), packet.into())
    }

    fn make_ts_segment_with_pat_pmt() -> HlsData {
        HlsData::ts(
            MediaSegment::empty(),
            Bytes::from_static(include_bytes!("../../../hls/tests/fixtures/avc-640x352.ts")),
        )
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
    fn forwards_end_marker_after_ts_without_psi() {
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

    #[test]
    fn flushes_short_fmp4_buffer_on_playlist_end() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::new(context.clone());

        let mut out = Vec::new();
        {
            let mut output = |item: HlsData| -> Result<(), PipelineError> {
                out.push(item);
                Ok(())
            };
            // Init + two media items: fewer than MIN_TAGS_NUM, so still buffered.
            operator
                .process(&context, make_m4s_init(4), &mut output)
                .unwrap();
            operator
                .process(&context, make_m4s_media(4), &mut output)
                .unwrap();
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
                .process(&context, HlsData::end_marker(), &mut output)
                .unwrap();
        }

        assert_eq!(out.len(), 4);
        assert!(out[0].is_mp4_init());
        assert!(out[1..3].iter().all(HlsData::is_mp4_media));
        assert!(matches!(out[3], HlsData::EndMarker(_)));
    }

    #[test]
    fn flushes_short_fmp4_buffer_on_finish() {
        let token = CancellationToken::new();
        let context = StreamerContext::arc_new(token);
        let mut operator = DefragmentOperator::new(context.clone());

        let mut out = Vec::new();
        let mut output = |item: HlsData| -> Result<(), PipelineError> {
            out.push(item);
            Ok(())
        };

        // Init + two media items: fewer than MIN_TAGS_NUM, so still buffered.
        operator
            .process(&context, make_m4s_init(4), &mut output)
            .unwrap();
        operator
            .process(&context, make_m4s_media(4), &mut output)
            .unwrap();
        operator
            .process(&context, make_m4s_media(4), &mut output)
            .unwrap();

        operator.finish(&context, &mut output).unwrap();

        assert_eq!(out.len(), 3);
        assert!(out[0].is_mp4_init());
        assert!(out[1..].iter().all(HlsData::is_mp4_media));
    }
}
