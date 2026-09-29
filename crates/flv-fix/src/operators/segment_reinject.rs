//! Shared cache of segment initialization data (FLV header, onMetaData script
//! tag, audio/video sequence headers) that `SplitOperator` and `LimitOperator`
//! re-emit at the start of each new segment after a split boundary.

use bytes::Bytes;
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
/// Store payloads through the methods below so these long-lived entries cannot
/// retain a large decoder buffer through a small `Bytes` slice.
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

    fn detach_payload(mut tag: FlvTag) -> FlvTag {
        tag.set_data(Bytes::copy_from_slice(tag.data()));
        tag
    }

    pub(crate) fn store_metadata(&mut self, tag: FlvTag) {
        self.metadata = Some(Self::detach_payload(tag));
    }

    /// Stores `tag` as the current video sequence header. When `zero_timestamp`
    /// is set, `tag.timestamp_ms` is rebased to 0 so a later `reinject` opens
    /// the new segment with the sequence header at timestamp 0; when unset the
    /// tag keeps the timestamp it carried in the stream.
    pub(crate) fn store_video_sequence_tag(&mut self, mut tag: FlvTag, zero_timestamp: bool) {
        if zero_timestamp {
            tag.timestamp_ms = 0;
        }
        self.video_sequence_tag = Some(Self::detach_payload(tag));
    }

    /// Stores `tag` as the current audio sequence header; see
    /// `store_video_sequence_tag` for the `zero_timestamp` semantics.
    pub(crate) fn store_audio_sequence_tag(&mut self, mut tag: FlvTag, zero_timestamp: bool) {
        if zero_timestamp {
            tag.timestamp_ms = 0;
        }
        self.audio_sequence_tag = Some(Self::detach_payload(tag));
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use flv::FlvTagType;

    use super::*;
    use crate::test_utils::fixtures::{AAC_LC_CONFIG, AV1_CODEC_CONFIG};

    struct DecoderBuffer {
        data: Vec<u8>,
        drops: Arc<AtomicUsize>,
    }

    impl AsRef<[u8]> for DecoderBuffer {
        fn as_ref(&self) -> &[u8] {
            &self.data
        }
    }

    impl Drop for DecoderBuffer {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn cached_tags_release_decoder_storage_and_reinject_exact_content() {
        for zero_timestamp in [false, true] {
            let drops = Arc::new(AtomicUsize::new(0));
            let mut data = vec![0; 1024 * 1024];
            let av1_sequence_start = [&b"\x90av01"[..], AV1_CODEC_CONFIG].concat();
            let payloads: [&[u8]; 3] = [
                b"\x02\0\x0aonMetaData\x03\0\0\x09",
                &av1_sequence_start,
                AAC_LC_CONFIG,
            ];
            for (index, payload) in payloads.iter().enumerate() {
                data[index * 128..index * 128 + payload.len()].copy_from_slice(payload);
            }
            let buffer = Bytes::from_owner(DecoderBuffer {
                data,
                drops: drops.clone(),
            });
            let kinds = [FlvTagType::ScriptData, FlvTagType::Video, FlvTagType::Audio];
            let tags: Vec<_> = payloads
                .iter()
                .enumerate()
                .map(|(index, payload)| {
                    FlvTag::new(
                        123,
                        0,
                        kinds[index],
                        false,
                        buffer.slice(index * 128..index * 128 + payload.len()),
                    )
                })
                .collect();
            let header = FlvHeader::new(true, true);
            let mut cache = SegmentInitCache::new();
            cache.header = Some(header.clone());
            cache.store_metadata(tags[0].clone());
            cache.store_video_sequence_tag(tags[1].clone(), zero_timestamp);
            cache.store_audio_sequence_tag(tags[2].clone(), zero_timestamp);
            drop(tags);
            drop(buffer);
            assert_eq!(
                drops.load(Ordering::SeqCst),
                1,
                "cached slices must release the decoder's allocation"
            );

            let mut output = Vec::new();
            cache
                .reinject(
                    &mut |item| {
                        output.push(item);
                        Ok(())
                    },
                    None,
                )
                .unwrap();
            let mut expected = vec![FlvData::Header(header)];
            expected.extend(payloads.iter().enumerate().map(|(index, payload)| {
                let timestamp = if zero_timestamp && index > 0 { 0 } else { 123 };
                FlvData::Tag(FlvTag::new(
                    timestamp,
                    0,
                    kinds[index],
                    false,
                    Bytes::copy_from_slice(payload),
                ))
            }));
            assert_eq!(output, expected);
        }
    }
}
