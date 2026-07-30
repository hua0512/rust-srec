//! Shared cache of segment initialization data (FLV header, onMetaData script
//! tag, audio/video sequence headers) that `SplitOperator` and `LimitOperator`
//! re-emit at the start of each new segment after a split boundary.

use flv::data::FlvData;
use flv::header::FlvHeader;
use flv::tag::FlvTag;
use pipeline_common::PipelineError;
use tracing::debug;

/// Caches the items a downstream writer needs at the start of every segment so
/// they can be re-injected after a split: the FLV header, the onMetaData
/// script tag and the latest audio/video sequence headers.
///
/// Fields are directly accessible because operators also need to inspect
/// (`SplitOperator::split_stream` reads `video_sequence_tag` for codec info)
/// or drain (`SplitOperator::flush_buffered_tags_if_pending` uses `take`) the
/// cached tags outside of a full reinjection.
#[derive(Default)]
pub(crate) struct SegmentInitCache {
    pub(crate) header: Option<FlvHeader>,
    pub(crate) metadata: Option<FlvTag>,
    pub(crate) audio_sequence_tag: Option<FlvTag>,
    pub(crate) video_sequence_tag: Option<FlvTag>,
}

impl SegmentInitCache {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Stores `tag` as the current video sequence header. When `zero_timestamp`
    /// is set, `tag.timestamp_ms` is rebased to 0 so a later `reinject` opens
    /// the new segment with the sequence header at timestamp 0; when unset the
    /// tag keeps the timestamp it carried in the stream.
    pub(crate) fn store_video_sequence_tag(&mut self, mut tag: FlvTag, zero_timestamp: bool) {
        if zero_timestamp {
            tag.timestamp_ms = 0;
        }
        self.video_sequence_tag = Some(tag);
    }

    /// Stores `tag` as the current audio sequence header; see
    /// `store_video_sequence_tag` for the `zero_timestamp` semantics.
    pub(crate) fn store_audio_sequence_tag(&mut self, mut tag: FlvTag, zero_timestamp: bool) {
        if zero_timestamp {
            tag.timestamp_ms = 0;
        }
        self.audio_sequence_tag = Some(tag);
    }

    /// Re-emits the cached items in segment-opening order: header, onMetaData
    /// script tag, video sequence header, audio sequence header.
    ///
    /// Tags are emitted with the timestamps they were stored with; callers that
    /// need rebased timestamps must store with `zero_timestamp` set, otherwise
    /// the segment timeline is not reset and downstream consumers may observe a
    /// timestamp discontinuity at the split point.
    ///
    /// When `debug_name` is provided, each re-emitted item is logged at debug
    /// level with that name as prefix.
    pub(crate) fn reinject(
        &self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
        debug_name: Option<&str>,
    ) -> Result<(), PipelineError> {
        if let Some(header) = &self.header {
            output(FlvData::Header(header.clone()))?;
            if let Some(name) = debug_name {
                debug!("{name} re-emit header after split");
            }
        }
        if let Some(metadata) = &self.metadata {
            output(FlvData::Tag(metadata.clone()))?;
            if let Some(name) = debug_name {
                debug!("{name} re-emit metadata after split");
            }
        }
        if let Some(video_seq) = &self.video_sequence_tag {
            output(FlvData::Tag(video_seq.clone()))?;
            if let Some(name) = debug_name {
                debug!("{name} re-emit video sequence tag after split");
            }
        }
        if let Some(audio_seq) = &self.audio_sequence_tag {
            output(FlvData::Tag(audio_seq.clone()))?;
            if let Some(name) = debug_name {
                debug!("{name} re-emit audio sequence tag after split");
            }
        }
        Ok(())
    }
}
