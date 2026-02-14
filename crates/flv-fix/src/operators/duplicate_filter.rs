//! # DuplicateTagFilterOperator
//!
//! Drops exact duplicate media tags within a rolling window.
//!
//! Some live streaming sources may "loop" the last few seconds of content when
//! a streamer goes offline, effectively replaying a chunk of the stream with
//! identical FLV tags (often with repeated timestamps).
//!
//! This operator performs a conservative deduplication:
//! - Only applies to audio/video *media* tags (script tags and sequence headers
//!   are passed through).
//! - Considers a tag duplicate if `(tag_type, timestamp_ms, crc32(data), len)`
//!   matches one seen recently.
//! - Additionally, if a large timestamp back-jump is detected, it will try to
//!   detect "replay loops" where the same content is re-sent with a constant
//!   timestamp offset and drop those tags as well.
//! - Resets state on `FlvData::Header` so segment boundaries don't cross-talk.
//!
//! This is intentionally conservative to avoid false positives on legitimate
//! repeated content (e.g. identical AAC frames at different timestamps).
use flv::data::FlvData;
use flv::tag::FlvTag;
use pipeline_common::{PipelineError, Processor, StreamerContext};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tracing::{debug, trace};

use crate::crc32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TagKey(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct FingerprintKey(u64);

#[inline]
fn mix64(mut x: u64) -> u64 {
    // SplitMix64
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

impl TagKey {
    fn new(tag: &FlvTag) -> Self {
        Self::new_with_timestamp(tag, tag.timestamp_ms)
    }

    fn new_with_timestamp(tag: &FlvTag, timestamp_ms: u32) -> Self {
        let tag_type: u8 = tag.tag_type.into();
        let ts = timestamp_ms as u64;
        let len = tag.data.len() as u64;
        let crc = crc32::crc32(tag.data.as_ref()) as u64;

        let x = ((tag_type as u64) << 56) ^ (len.rotate_left(17)) ^ ts ^ (crc.rotate_left(1));
        TagKey(mix64(x))
    }
}

impl FingerprintKey {
    fn new(tag: &FlvTag) -> Self {
        let tag_type: u8 = tag.tag_type.into();
        let len = tag.data.len() as u64;
        let crc = crc32::crc32(tag.data.as_ref()) as u64;
        let x = ((tag_type as u64) << 56) ^ (len.rotate_left(17)) ^ (crc.rotate_left(1));
        FingerprintKey(mix64(x))
    }
}

#[derive(Debug, Clone)]
pub struct DuplicateTagFilterConfig {
    /// Maximum number of recently-seen tags to remember for exact duplicate
    /// suppression.
    pub window_capacity_tags: usize,
    /// Minimum timestamp back-jump (ms) to consider the stream as "replaying"
    /// recent content (e.g. streamer went offline and service loops tail).
    pub replay_backjump_threshold_ms: u32,
    /// Enable replay detection using a constant timestamp offset.
    ///
    /// When enabled and a back-jump is detected, the operator will attempt to
    /// find an offset that maps incoming replay timestamps to a previously seen
    /// region and drop tags that match the mapped timestamps.
    pub enable_replay_offset_matching: bool,
}

impl Default for DuplicateTagFilterConfig {
    fn default() -> Self {
        Self {
            window_capacity_tags: 8 * 1024,
            replay_backjump_threshold_ms: 2_000,
            enable_replay_offset_matching: true,
        }
    }
}

pub struct DuplicateTagFilterOperator {
    context: Arc<StreamerContext>,
    config: DuplicateTagFilterConfig,
    order: VecDeque<SeenEntry>,
    seen: HashSet<TagKey>,
    fingerprint_last: HashMap<FingerprintKey, (u32, u64)>,
    seq: u64,
    max_timestamp_seen: u32,
    replay_active: bool,
    replay_offset_ms: Option<i64>,
    dropped_duplicates: u64,
    next_drop_log_at: u64,
}

impl DuplicateTagFilterOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self::with_config(context, DuplicateTagFilterConfig::default())
    }

    pub fn with_config(context: Arc<StreamerContext>, config: DuplicateTagFilterConfig) -> Self {
        let cap = config.window_capacity_tags.max(1);
        Self {
            context,
            config,
            order: VecDeque::with_capacity(cap.min(1024)),
            seen: HashSet::with_capacity(cap.min(1024)),
            fingerprint_last: HashMap::with_capacity(cap.min(1024)),
            seq: 0,
            max_timestamp_seen: 0,
            replay_active: false,
            replay_offset_ms: None,
            dropped_duplicates: 0,
            next_drop_log_at: 1_000,
        }
    }

    pub fn with_capacity(context: Arc<StreamerContext>, capacity: usize) -> Self {
        Self::with_config(
            context,
            DuplicateTagFilterConfig {
                window_capacity_tags: capacity.max(1),
                ..Default::default()
            },
        )
    }

    fn reset(&mut self) {
        self.order.clear();
        self.seen.clear();
        self.fingerprint_last.clear();
        self.seq = 0;
        self.max_timestamp_seen = 0;
        self.replay_active = false;
        self.replay_offset_ms = None;
        self.dropped_duplicates = 0;
        self.next_drop_log_at = 1_000;
    }

    fn track_tag(&mut self, tag: &FlvTag) {
        let fingerprint = FingerprintKey::new(tag);
        let key = TagKey::new(tag);

        self.seq = self.seq.wrapping_add(1);
        let seq = self.seq;

        self.seen.insert(key);
        self.fingerprint_last
            .insert(fingerprint, (tag.timestamp_ms, seq));
        self.order.push_back(SeenEntry {
            key,
            fingerprint,
            seq,
        });

        while self.order.len() > self.config.window_capacity_tags {
            if let Some(old) = self.order.pop_front() {
                self.seen.remove(&old.key);
                if self
                    .fingerprint_last
                    .get(&old.fingerprint)
                    .is_some_and(|&(_, last_seq)| last_seq == old.seq)
                {
                    self.fingerprint_last.remove(&old.fingerprint);
                }
            }
        }
    }

    fn is_exact_duplicate(&self, key: TagKey) -> bool {
        self.seen.contains(&key)
    }

    fn replay_mapped_key(&mut self, tag: &FlvTag, fingerprint: FingerprintKey) -> Option<TagKey> {
        if !self.replay_active || !self.config.enable_replay_offset_matching {
            return None;
        }

        let ts = tag.timestamp_ms;

        // If we don't have an offset yet, try to infer one from the last timestamp
        // of the same fingerprint (if available).
        if self.replay_offset_ms.is_none()
            && let Some(&(prev_ts, _)) = self.fingerprint_last.get(&fingerprint)
            && prev_ts > ts
        {
            let candidate = (prev_ts - ts) as i64;
            let mapped_ts = prev_ts;
            let mapped_key = TagKey::new_with_timestamp(tag, mapped_ts);
            if self.is_exact_duplicate(mapped_key) {
                self.replay_offset_ms = Some(candidate);
                return Some(mapped_key);
            }
        }

        let offset = self.replay_offset_ms?;

        let mapped_ts_i64 = ts as i64 + offset;
        if mapped_ts_i64 < 0 || mapped_ts_i64 > u32::MAX as i64 {
            return None;
        }
        let mapped_ts = mapped_ts_i64 as u32;

        Some(TagKey::new_with_timestamp(tag, mapped_ts))
    }

    fn track_and_check(&mut self, tag: &FlvTag) -> bool {
        // 1) Exact match (type + timestamp + payload).
        let key = TagKey::new(tag);
        if self.seen.contains(&key) {
            return true;
        }

        // 2) Replay-mode match: same payload, but timestamp shifted by a constant offset.
        let fp = FingerprintKey::new(tag);
        if let Some(mapped_key) = self.replay_mapped_key(tag, fp)
            && self.is_exact_duplicate(mapped_key)
        {
            return true;
        }

        false
    }
}

#[derive(Clone, Copy, Debug)]
struct SeenEntry {
    key: TagKey,
    fingerprint: FingerprintKey,
    seq: u64,
}

impl Processor<FlvData> for DuplicateTagFilterOperator {
    fn process(
        &mut self,
        context: &Arc<StreamerContext>,
        input: FlvData,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if context.token.is_cancelled() {
            return Err(PipelineError::Cancelled);
        }

        match input {
            FlvData::Header(_) => {
                self.reset();
                output(input)
            }
            FlvData::Tag(tag) => {
                // Keep metadata and codec config tags as-is. Dedicated operators handle those.
                if tag.is_script_tag()
                    || tag.is_video_sequence_header()
                    || tag.is_audio_sequence_header()
                {
                    return output(FlvData::Tag(tag));
                }

                // Only dedup A/V media tags.
                if !(tag.is_audio_tag() || tag.is_video_tag()) {
                    return output(FlvData::Tag(tag));
                }

                // Update max timestamp seen and detect replay mode on large back-jumps.
                let ts = tag.timestamp_ms;
                let prev_max = self.max_timestamp_seen;
                self.max_timestamp_seen = self.max_timestamp_seen.max(ts);
                // Use the previous max for back-jump detection so the current tag doesn't
                // "mask" a large regression by resetting max_timestamp_seen first.
                if !self.replay_active
                    && prev_max.saturating_sub(ts) > self.config.replay_backjump_threshold_ms
                {
                    self.replay_active = true;
                }

                if self.track_and_check(&tag) {
                    self.dropped_duplicates = self.dropped_duplicates.saturating_add(1);
                    trace!(
                        "{} Dropping duplicate media tag: type={:?} ts={} len={}",
                        self.context.name,
                        tag.tag_type,
                        tag.timestamp_ms,
                        tag.data.len()
                    );
                    if self.dropped_duplicates >= self.next_drop_log_at {
                        debug!(
                            "{} Dropped {} duplicate media tags so far",
                            self.context.name, self.dropped_duplicates
                        );
                        self.next_drop_log_at = self.next_drop_log_at.saturating_add(1_000);
                    }
                    return Ok(());
                }

                self.track_tag(&tag);

                output(FlvData::Tag(tag))
            }
            _ => output(input),
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        _output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        self.reset();
        Ok(())
    }

    fn name(&self) -> &'static str {
        "DuplicateTagFilterOperator"
    }
}

#[cfg(test)]
mod tests {
    use pipeline_common::CancellationToken;

    use super::*;
    use crate::test_utils::{create_audio_tag, create_test_header, create_video_tag};

    #[test]
    fn test_drops_exact_duplicate_media_tags_within_window() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DuplicateTagFilterOperator::with_capacity(context.clone(), 64);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        // Duplicate video and audio tags at the same timestamp.
        let v = create_video_tag(100, true);
        operator
            .process(&context, v.clone(), &mut output_fn)
            .unwrap();
        operator.process(&context, v, &mut output_fn).unwrap();

        let a = create_audio_tag(120);
        operator
            .process(&context, a.clone(), &mut output_fn)
            .unwrap();
        operator.process(&context, a, &mut output_fn).unwrap();

        let video_count = output_items
            .iter()
            .filter_map(|i| match i {
                FlvData::Tag(t) => Some(t),
                _ => None,
            })
            .filter(|t| t.is_video_tag() && !t.is_video_sequence_header())
            .count();
        let audio_count = output_items
            .iter()
            .filter_map(|i| match i {
                FlvData::Tag(t) => Some(t),
                _ => None,
            })
            .filter(|t| t.is_audio_tag() && !t.is_audio_sequence_header())
            .count();

        assert_eq!(video_count, 1);
        assert_eq!(audio_count, 1);
    }

    #[test]
    fn test_allows_same_payload_at_different_timestamps() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DuplicateTagFilterOperator::with_capacity(context.clone(), 64);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        // create_audio_tag uses constant payload; timestamps differ so they should pass.
        operator
            .process(&context, create_audio_tag(100), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_audio_tag(200), &mut output_fn)
            .unwrap();

        let audio_count = output_items
            .iter()
            .filter_map(|i| match i {
                FlvData::Tag(t) => Some(t),
                _ => None,
            })
            .filter(|t| t.is_audio_tag() && !t.is_audio_sequence_header())
            .count();

        assert_eq!(audio_count, 2);
    }

    #[test]
    fn test_resets_on_header() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DuplicateTagFilterOperator::with_capacity(context.clone(), 64);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        // Segment 1.
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        // Segment 2: header resets, so identical tag should pass again.
        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();
        operator
            .process(&context, create_video_tag(100, true), &mut output_fn)
            .unwrap();

        let video_count = output_items
            .iter()
            .filter_map(|i| match i {
                FlvData::Tag(t) => Some(t),
                _ => None,
            })
            .filter(|t| t.is_video_tag() && !t.is_video_sequence_header())
            .count();

        assert_eq!(video_count, 2);
    }

    #[test]
    fn test_drops_replayed_loop_of_last_content() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DuplicateTagFilterOperator::with_capacity(context.clone(), 256);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        // Simulate a short "tail" of content.
        let tail = [
            create_video_tag(900, true),
            create_audio_tag(920),
            create_video_tag(933, false),
            create_audio_tag(940),
        ];

        for item in tail.iter().cloned() {
            operator.process(&context, item, &mut output_fn).unwrap();
        }

        // Upstream loops and replays the same tail with the same timestamps.
        for item in tail.iter().cloned() {
            operator.process(&context, item, &mut output_fn).unwrap();
        }

        let media_tag_count = output_items
            .iter()
            .filter(|i| matches!(i, FlvData::Tag(_)))
            .count();

        // First loop contributes 4 tags, second loop should be fully dropped.
        assert_eq!(media_tag_count, 4);
    }

    #[test]
    fn test_drops_replayed_loop_with_timestamp_offset() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let cfg = DuplicateTagFilterConfig {
            window_capacity_tags: 256,
            // Default is 2000ms; we use a large back-jump anyway.
            replay_backjump_threshold_ms: 2_000,
            ..Default::default()
        };
        let mut operator = DuplicateTagFilterOperator::with_config(context.clone(), cfg);
        let mut output_items = Vec::new();

        let mut output_fn = |item: FlvData| -> Result<(), PipelineError> {
            output_items.push(item);
            Ok(())
        };

        operator
            .process(&context, create_test_header(), &mut output_fn)
            .unwrap();

        // Original tail near the end of stream.
        let tail = [
            create_video_tag(9000, true),
            create_audio_tag(9200),
            create_video_tag(9330, false),
            create_audio_tag(9400),
        ];
        for item in tail.iter().cloned() {
            operator.process(&context, item, &mut output_fn).unwrap();
        }

        // Replay the same tail, but timestamps are offset back to "restart" from ~0.
        let replay = [
            create_video_tag(0, true),
            create_audio_tag(200),
            create_video_tag(330, false),
            create_audio_tag(400),
        ];
        for item in replay.iter().cloned() {
            operator.process(&context, item, &mut output_fn).unwrap();
        }

        let media_tag_count = output_items
            .iter()
            .filter(|i| matches!(i, FlvData::Tag(_)))
            .count();

        // Only the first tail should remain.
        assert_eq!(media_tag_count, 4);
    }
}
