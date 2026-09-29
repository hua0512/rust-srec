//! Bounded, byte-verified duplicate suppression for recognized media packets.
//!
//! CRC32 is only a lookup accelerator; matching payload bytes are required before
//! dropping a tag. Payloads are copied into bounded storage so small tags cannot
//! pin large decoder buffers. Opaque, control, and multitrack tags pass through.
//!
//! Optional replay detection buffers a short candidate run after a clock reset.
//! It requires a unique historical run with three distinct payloads and matching
//! timestamp deltas. Ambiguous or incomplete candidates are emitted unchanged.
//! This remains a heuristic and is disabled by default, as is pipeline filtering.
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use flv::{FlvData, FlvTag, SplitReason};
use pipeline_common::{PipelineError, Processor, StreamerContext, crc32};
use tracing::{debug, trace};

const REPLAY_CONFIRMATION_PACKETS: usize = 3;
const MAX_PENDING_TAGS: usize = 8;
const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MAX_REPLAY_CANDIDATES: usize = 16;
const REPLAY_GAP: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct TagKey {
    tag_type: u8,
    stream_id: u32,
    timestamp_ms: u32,
    len: usize,
    crc: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SeenTag {
    key: TagKey,
    data: Bytes,
}

impl Hash for SeenTag {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash collisions still undergo the derived, byte-for-byte Eq check.
        self.key.hash(state);
    }
}

impl SeenTag {
    fn new(tag: &FlvTag) -> Self {
        Self {
            key: TagKey {
                tag_type: tag.tag_type().into(),
                stream_id: tag.stream_id,
                timestamp_ms: tag.timestamp_ms,
                len: tag.data().len(),
                crc: crc32::crc32(tag.data()),
            },
            data: tag.data().clone(),
        }
    }

    fn same_payload(&self, other: &Self) -> bool {
        self.key.tag_type == other.key.tag_type
            && self.key.stream_id == other.key.stream_id
            && self.key.len == other.key.len
            && self.key.crc == other.key.crc
            && self.data == other.data
    }
}

#[derive(Default)]
struct History {
    order: VecDeque<SeenTag>,
    seen: HashSet<SeenTag>,
    bytes: usize,
}

impl History {
    fn clear(&mut self) {
        self.order.clear();
        self.seen.clear();
        self.bytes = 0;
    }

    /// Returns false only for a retained, byte-identical duplicate. Packets that
    /// cannot be retained still return true so their caller can forward them.
    fn remember(&mut self, mut tag: SeenTag, config: &DuplicateTagFilterConfig) -> bool {
        if config.window_capacity_tags == 0 || tag.data.len() > config.window_capacity_bytes {
            // Missing packets must not join two unrelated historical replay runs.
            self.clear();
            return true;
        }
        if self.seen.contains(&tag) {
            return false;
        }
        while self.order.len() >= config.window_capacity_tags
            || self.bytes > config.window_capacity_bytes - tag.data.len()
        {
            if let Some(old) = self.order.pop_front() {
                self.bytes -= old.data.len();
                self.seen.remove(&old);
            }
        }
        tag.data = Bytes::copy_from_slice(&tag.data);
        self.bytes += tag.data.len();
        self.seen.insert(tag.clone());
        self.order.push_back(tag);
        true
    }
}

#[derive(Debug, Clone)]
pub struct DuplicateTagFilterConfig {
    /// Maximum number of media packets retained. Zero disables retention.
    pub window_capacity_tags: usize,
    /// Maximum retained payload bytes (16 MiB by default). Oversized packets
    /// pass through. Replay lookahead additionally retains at most 1 MiB.
    pub window_capacity_bytes: usize,
    /// Minimum per-stream backwards timestamp jump that starts a new timeline.
    pub replay_backjump_threshold_ms: u32,
    /// Opt in to heuristic replay removal after a clock reset. Confirmation
    /// needs three distinct packets in order, at one consistent offset. Pending
    /// packets are released on the next input after 500 ms, after eight packets,
    /// or on finish; this synchronous processor does not run an idle timer.
    pub enable_replay_offset_matching: bool,
}

impl Default for DuplicateTagFilterConfig {
    fn default() -> Self {
        Self {
            window_capacity_tags: 8 * 1024,
            window_capacity_bytes: 16 * 1024 * 1024,
            replay_backjump_threshold_ms: 2_000,
            enable_replay_offset_matching: false,
        }
    }
}

#[derive(Clone, Copy)]
struct ReplayCursor {
    next: usize,
    offset_ms: i64,
}

impl ReplayCursor {
    fn advance(&mut self, history: &History, tag: &SeenTag) -> bool {
        if history.order.get(self.next).is_some_and(|old| {
            old.same_payload(tag)
                && old.key.timestamp_ms as i64 - tag.key.timestamp_ms as i64 == self.offset_ms
        }) {
            self.next += 1;
            true
        } else {
            false
        }
    }
}

struct Candidate {
    cursors: Vec<ReplayCursor>,
    pending: Vec<FlvTag>,
    bytes: usize,
    started: Instant,
    first_timestamp: u32,
}

enum ReplayState {
    Normal,
    Candidate(Candidate),
    Confirmed {
        cursor: ReplayCursor,
        last_seen: Instant,
        last_timestamp: u32,
    },
}

pub struct DuplicateTagFilterOperator {
    context: Arc<StreamerContext>,
    config: DuplicateTagFilterConfig,
    history: History,
    replay: ReplayState,
    // Audio and video can have different clocks. A jump starts a fresh epoch
    // for both so the second stream's first packet cannot restart confirmation.
    last_timestamps: [Option<u32>; 2],
}

impl DuplicateTagFilterOperator {
    pub fn new(context: Arc<StreamerContext>) -> Self {
        Self::with_config(context, DuplicateTagFilterConfig::default())
    }

    pub fn with_config(context: Arc<StreamerContext>, config: DuplicateTagFilterConfig) -> Self {
        Self {
            context,
            config,
            history: History::default(),
            replay: ReplayState::Normal,
            last_timestamps: [None; 2],
        }
    }

    pub fn with_capacity(context: Arc<StreamerContext>, capacity: usize) -> Self {
        Self::with_config(
            context,
            DuplicateTagFilterConfig {
                window_capacity_tags: capacity,
                ..Default::default()
            },
        )
    }

    fn emit_new(
        &mut self,
        tag: FlvTag,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        self.history.remember(SeenTag::new(&tag), &self.config);
        output(FlvData::Tag(tag))
    }

    fn release_candidate(
        &mut self,
        candidate: Candidate,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        // The reset was not a proven replay. Its packets belong to a new epoch,
        // even if an old epoch contained identical bytes at identical timestamps.
        self.history.clear();
        for tag in candidate.pending {
            self.emit_new(tag, output)?;
        }
        Ok(())
    }

    fn flush_pending(
        &mut self,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        if matches!(self.replay, ReplayState::Candidate(_))
            && let ReplayState::Candidate(candidate) =
                std::mem::replace(&mut self.replay, ReplayState::Normal)
        {
            self.release_candidate(candidate, output)?;
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.replay = ReplayState::Normal;
        self.history.clear();
        self.last_timestamps = [None; 2];
    }

    fn begin_replay(&mut self, tag: &SeenTag, now: Instant) {
        self.replay = ReplayState::Normal;
        if self.config.enable_replay_offset_matching {
            let mut cursors = Vec::new();
            for (index, old) in self.history.order.iter().enumerate() {
                if old.same_payload(tag) {
                    if cursors.len() == MAX_REPLAY_CANDIDATES {
                        // Repetitive content cannot identify a unique source run.
                        self.history.clear();
                        return;
                    }
                    cursors.push(ReplayCursor {
                        next: index,
                        offset_ms: old.key.timestamp_ms as i64 - tag.key.timestamp_ms as i64,
                    });
                }
            }
            if !cursors.is_empty() {
                self.replay = ReplayState::Candidate(Candidate {
                    cursors,
                    pending: Vec::new(),
                    bytes: 0,
                    started: now,
                    first_timestamp: tag.key.timestamp_ms,
                });
                return;
            }
        }
        self.history.clear();
    }

    fn process_media(
        &mut self,
        mut tag: FlvTag,
        now: Instant,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let seen = SeenTag::new(&tag);
        let stream = usize::from(tag.is_video_tag());
        if self.last_timestamps[stream].is_some_and(|last| {
            last.saturating_sub(tag.timestamp_ms) > self.config.replay_backjump_threshold_ms
        }) {
            self.flush_pending(output)?;
            self.last_timestamps = [None; 2];
            self.begin_replay(&seen, now);
        }
        self.last_timestamps[stream] = Some(tag.timestamp_ms);

        if matches!(&self.replay,
            ReplayState::Confirmed { cursor, last_seen, last_timestamp }
                if cursor.next == self.history.order.len()
                    && now.duration_since(*last_seen) <= REPLAY_GAP
                    && tag.timestamp_ms.abs_diff(*last_timestamp) <= self.config.replay_backjump_threshold_ms.max(REPLAY_GAP.as_millis() as u32)
        ) {
            // A complete replay may loop again with a different timestamp base.
            // Require fresh confirmation rather than reusing its previous offset.
            self.begin_replay(&seen, now);
        }

        match std::mem::replace(&mut self.replay, ReplayState::Normal) {
            ReplayState::Normal => {
                if !self.history.remember(seen, &self.config) {
                    trace!(streamer = %self.context.name, timestamp_ms = tag.timestamp_ms, "Dropping byte-identical media tag");
                    return Ok(());
                }
                output(FlvData::Tag(tag))
            }
            ReplayState::Candidate(mut candidate) => {
                let byte_limit = self.config.window_capacity_bytes.min(MAX_PENDING_BYTES);
                let expired = now.duration_since(candidate.started) > REPLAY_GAP
                    || tag.timestamp_ms.abs_diff(candidate.first_timestamp)
                        > REPLAY_GAP.as_millis() as u32;
                if expired || tag.data().len() > byte_limit.saturating_sub(candidate.bytes) {
                    self.release_candidate(candidate, output)?;
                    return self.emit_new(tag, output);
                }
                candidate
                    .cursors
                    .retain_mut(|cursor| cursor.advance(&self.history, &seen));
                if candidate.cursors.is_empty() {
                    self.release_candidate(candidate, output)?;
                    return self.emit_new(tag, output);
                }
                candidate.bytes += tag.data().len();
                // Detach from possibly much larger decoder buffers before buffering.
                tag.set_data(Bytes::copy_from_slice(tag.data()));
                candidate.pending.push(tag);
                let distinct = candidate
                    .pending
                    .iter()
                    .enumerate()
                    .filter(|(index, tag)| {
                        !candidate.pending[..*index].iter().any(|earlier| {
                            earlier.tag_type() == tag.tag_type()
                                && earlier.stream_id == tag.stream_id
                                && earlier.data() == tag.data()
                        })
                    })
                    .count();
                if candidate.cursors.len() == 1
                    && distinct >= REPLAY_CONFIRMATION_PACKETS
                    && seen.key.timestamp_ms > candidate.first_timestamp
                {
                    debug!(streamer = %self.context.name, packets = candidate.pending.len(), offset_ms = candidate.cursors[0].offset_ms, "Confirmed media replay");
                    self.replay = ReplayState::Confirmed {
                        cursor: candidate.cursors[0],
                        last_seen: now,
                        last_timestamp: seen.key.timestamp_ms,
                    };
                } else if candidate.pending.len() >= MAX_PENDING_TAGS {
                    self.release_candidate(candidate, output)?;
                } else {
                    self.replay = ReplayState::Candidate(candidate);
                }
                Ok(())
            }
            ReplayState::Confirmed {
                mut cursor,
                last_seen,
                last_timestamp,
            } => {
                if now.duration_since(last_seen) <= REPLAY_GAP
                    && tag.timestamp_ms.abs_diff(last_timestamp) <= REPLAY_GAP.as_millis() as u32
                    && cursor.advance(&self.history, &seen)
                {
                    self.replay = ReplayState::Confirmed {
                        cursor,
                        last_seen: now,
                        last_timestamp: tag.timestamp_ms,
                    };
                    Ok(())
                } else {
                    self.history.clear();
                    self.emit_new(tag, output)
                }
            }
        }
    }
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
            FlvData::Tag(tag) if tag.classification().media => {
                self.process_media(tag, Instant::now(), output)
            }
            item => {
                self.flush_pending(output)?;
                let resets_timeline = match &item {
                    FlvData::Header(_) | FlvData::EndOfSequence(_) => true,
                    FlvData::Split(reason) => !matches!(
                        reason,
                        SplitReason::SizeLimit
                            | SplitReason::DurationLimit
                            | SplitReason::Manual { .. }
                    ),
                    // Control/config/opaque A/V packets are barriers: don't infer
                    // replay across unexamined tracks or decoder state changes.
                    FlvData::Tag(tag) => tag.is_audio_tag() || tag.is_video_tag(),
                };
                if resets_timeline {
                    self.reset();
                }
                output(item)
            }
        }
    }

    fn finish(
        &mut self,
        _context: &Arc<StreamerContext>,
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let result = self.flush_pending(output);
        self.reset();
        result
    }

    fn name(&self) -> &'static str {
        "DuplicateTagFilterOperator"
    }
}

#[cfg(test)]
mod tests {
    use flv::{FlvHeader, FlvTagType};
    use pipeline_common::CancellationToken;

    use super::*;

    // AV1 keyframe from the FFmpeg fixture documented in pipeline.rs.
    const AV1_FRAME: &[u8] = b"\x91av01\x12\0\x0a\x0a\0\0\0\x01\x9f\xf9\xb5\xf2\0\x80\x32\x0e\x10\0\xd0\0\0\x02\x80\0\0\0\xa9\x8e\x5e\xd0";

    // One 16-bit stereo PCM sample; identities differ through actual sample data.
    fn pcm(timestamp: u32, sample: u16) -> FlvData {
        let [lo, hi] = sample.to_le_bytes();
        FlvData::Tag(FlvTag::new(
            timestamp,
            0,
            FlvTagType::Audio,
            false,
            Bytes::from(vec![0x3f, lo, hi, lo, hi]),
        ))
    }

    fn replay_config() -> DuplicateTagFilterConfig {
        DuplicateTagFilterConfig {
            enable_replay_offset_matching: true,
            ..Default::default()
        }
    }

    fn run(config: DuplicateTagFilterConfig, input: Vec<FlvData>) -> Vec<FlvData> {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator = DuplicateTagFilterOperator::with_config(context.clone(), config);
        let mut output = Vec::new();
        let mut emit = |item| {
            output.push(item);
            Ok(())
        };
        for item in input {
            operator.process(&context, item, &mut emit).unwrap();
        }
        operator.finish(&context, &mut emit).unwrap();
        output
    }

    fn sequence(start: u32) -> Vec<FlvData> {
        (0..4)
            .map(|index| pcm(start + index * 23, index as u16))
            .collect()
    }

    #[test]
    fn exact_duplicates_are_removed_but_new_timestamps_survive() {
        for replay in [false, true] {
            let config = DuplicateTagFilterConfig {
                enable_replay_offset_matching: replay,
                ..Default::default()
            };
            assert_eq!(
                run(
                    config,
                    vec![pcm(100, 1), pcm(100, 1), pcm(123, 1), pcm(123, 2)]
                ),
                vec![pcm(100, 1), pcm(123, 1), pcm(123, 2)]
            );
        }
    }

    #[test]
    fn recognized_legacy_and_enhanced_media_are_deduplicated() {
        // Real AAC silence and the AV1 keyframe used by the pipeline fixture.
        // AVC/HEVC: FFmpeg -f lavfi -i color=c=black:s=16x16:r=25 -frames:v 1
        // -c:v libx264 (or libx265) -f flv; encoder SEI removed from the packet.
        let cases: &[(FlvTagType, &[u8])] = &[
            (FlvTagType::Video, b"\x17\x01\0\0\0\0\0\0\x0f\x65\x88\x84\0\x2b\xff\xfe\xf6\x73\x7c\x0a\x6b\x6d\xb1\x81"),
            (FlvTagType::Video, b"\x91avc1\0\0\0\0\0\0\x0f\x65\x88\x84\0\x2b\xff\xfe\xf6\x73\x7c\x0a\x6b\x6d\xb1\x81"),
            (FlvTagType::Video, b"\x93hvc1\0\0\0\x0b\x28\x01\xaf\x1d\x80\xee\x23\x8f\xff\x5e\x8f"),
            (FlvTagType::Video, b"\x91hvc1\0\0\0\0\0\0\x0b\x28\x01\xaf\x1d\x80\xee\x23\x8f\xff\x5e\x8f"),
            (FlvTagType::Audio, b"\xaf\x01\x21\x10\x04\x60\x8c\x1c"),
            (FlvTagType::Audio, b"\x91mp4a\x21\x10\x04\x60\x8c\x1c"),
            (FlvTagType::Video, AV1_FRAME),
        ];
        for &(kind, data) in cases {
            let tag = FlvData::Tag(FlvTag::new(
                100,
                0,
                kind,
                false,
                Bytes::copy_from_slice(data),
            ));
            assert_eq!(
                run(
                    DuplicateTagFilterConfig::default(),
                    vec![tag.clone(), tag.clone()]
                ),
                vec![tag]
            );
        }
    }

    #[test]
    fn crc_collisions_and_old_combined_key_collisions_preserve_distinct_payloads() {
        // These opaque AAC bodies test container identity, without decoding AAC.
        let payloads: &[(u32, &[u8])] = &[
            (100, b"\xaf\x01\x4b\x15\x7c\x8a\xaa\x45\x53\x1c"),
            (100, b"\xaf\x01\x91\x5f\x5a\x95\x90\x9a\x61\x66"),
            (26_196_655, b"\xaf\x01\xbf\xc3\xd0\x2a\xcc\x13\x3b\x6f"),
            (26_231_339, b"\xaf\x01\xe1\xe4\x60\x67\x6e\xf9\xe5\x44"),
        ];
        assert_eq!(crc32::crc32(payloads[0].1), crc32::crc32(payloads[1].1));
        let tags: Vec<_> = payloads
            .iter()
            .map(|(ts, bytes)| {
                FlvData::Tag(FlvTag::new(
                    *ts,
                    0,
                    FlvTagType::Audio,
                    false,
                    Bytes::copy_from_slice(bytes),
                ))
            })
            .collect();
        // Repeat each immediately: bypassing the filter cannot satisfy this test.
        let input = tags
            .iter()
            .flat_map(|tag| [tag.clone(), tag.clone()])
            .collect();
        assert_eq!(run(DuplicateTagFilterConfig::default(), input), tags);

        let colliding = |ts, index: usize| {
            FlvData::Tag(FlvTag::new(
                ts,
                0,
                FlvTagType::Audio,
                false,
                Bytes::copy_from_slice(payloads[index].1),
            ))
        };
        let input = vec![
            colliding(10_000, 0),
            pcm(10_023, 1),
            pcm(10_046, 2),
            colliding(0, 1),
            pcm(23, 1),
            pcm(46, 2),
        ];
        assert_eq!(run(replay_config(), input.clone()), input);
    }

    #[test]
    fn retention_is_bounded_by_both_bytes_and_packet_count() {
        for (tags, bytes) in [(2, 1000), (1000, 10)] {
            let config = DuplicateTagFilterConfig {
                window_capacity_tags: tags,
                window_capacity_bytes: bytes,
                ..Default::default()
            };
            assert_eq!(
                run(
                    config,
                    vec![pcm(0, 1), pcm(23, 2), pcm(46, 3), pcm(0, 1), pcm(0, 1)]
                ),
                vec![pcm(0, 1), pcm(23, 2), pcm(46, 3), pcm(0, 1)]
            );
        }
        for (tags, bytes) in [(0, 1000), (1000, 0), (1000, 4)] {
            let config = DuplicateTagFilterConfig {
                window_capacity_tags: tags,
                window_capacity_bytes: bytes,
                ..Default::default()
            };
            let input = vec![pcm(0, 1), pcm(0, 1), pcm(23, 2), pcm(23, 2)];
            assert_eq!(run(config, input.clone()), input);
        }
    }

    #[test]
    fn ineligible_packets_are_preserved_and_separate_media_history() {
        let cases: &[(FlvTagType, bool, &[u8])] = &[
            (FlvTagType::Audio, true, b"\x3f\x01\0\x01\0"),
            (FlvTagType::Audio, false, b"\xaf\0\x12\x10"),
            (FlvTagType::Audio, false, b"\x92Opus"),
            (FlvTagType::Audio, false, b"\x94Opus\0"),
            (FlvTagType::Audio, false, b"\x95\x01Opus\0\x01"),
            (FlvTagType::Audio, false, b"\xff\x01"),
            (FlvTagType::Audio, false, b"\xaf\x01"),
            (FlvTagType::Video, false, b"\x57\x01"),
            (FlvTagType::Video, false, b"\x94av01\0"),
            (FlvTagType::Video, false, b"\x92av01"),
            (FlvTagType::Video, false, b"\x96\x01av01\0\x12\0"),
            (FlvTagType::Video, false, b"\x91????\x01"),
            (FlvTagType::Video, false, b"\x93av01\x12\0"),
            (FlvTagType::Video, false, b"\x27\x01\0\0\0"),
        ];
        for &(kind, filtered, bytes) in cases {
            let barrier = FlvData::Tag(FlvTag::new(
                0,
                0,
                kind,
                filtered,
                Bytes::copy_from_slice(bytes),
            ));
            let input = vec![pcm(0, 1), barrier.clone(), barrier, pcm(0, 1)];
            assert_eq!(
                run(replay_config(), input.clone()),
                input,
                "payload {bytes:02x?}"
            );
        }
    }

    #[test]
    fn timestamp_reset_starts_a_new_epoch_even_when_exact_bytes_match() {
        let input = vec![pcm(0, 1), pcm(3000, 2), pcm(0, 1)];
        for config in [DuplicateTagFilterConfig::default(), replay_config()] {
            assert_eq!(run(config, input.clone()), input);
        }
    }

    #[test]
    fn replay_is_opt_in_and_requires_distinct_ordered_evidence() {
        let original = sequence(10_000);
        let replay = sequence(0);
        let input: Vec<_> = original.iter().chain(&replay).cloned().collect();
        assert_eq!(
            run(DuplicateTagFilterConfig::default(), input.clone()),
            input
        );
        assert_eq!(run(replay_config(), input), original);

        // Valid repeated silence has no distinct evidence that it is a replay.
        let silence = |ts| {
            FlvData::Tag(FlvTag::new(
                ts,
                0,
                FlvTagType::Audio,
                false,
                Bytes::from_static(b"\xaf\x01\x21\x10\x04\x60\x8c\x1c"),
            ))
        };
        let input: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..10).map(move |i| silence(start + i * 23)))
            .collect();
        assert_eq!(run(replay_config(), input.clone()), input);
        // Two alternating payloads also fail the distinct-evidence requirement.
        let input: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..6).map(move |i| pcm(start + i * 23, (i % 2) as u16)))
            .collect();
        assert_eq!(run(replay_config(), input.clone()), input);
    }

    #[test]
    fn repeated_payload_anchors_and_new_offsets_require_fresh_confirmation() {
        let original = vec![
            pcm(10_000, 1),
            pcm(10_023, 2),
            pcm(10_046, 1),
            pcm(10_069, 3),
        ];
        let mut input = original.clone();
        for start in [0, 1000, 0] {
            input.extend([
                pcm(start, 1),
                pcm(start + 23, 2),
                pcm(start + 46, 1),
                pcm(start + 69, 3),
            ]);
        }
        assert_eq!(run(replay_config(), input), original);
    }

    #[test]
    fn mismatches_release_candidates_and_end_confirmed_replays() {
        let original = sequence(10_000);
        let unconfirmed = vec![pcm(0, 0), pcm(23, 1), pcm(46, 99), pcm(69, 3)];
        let input: Vec<_> = original.iter().chain(&unconfirmed).cloned().collect();
        assert_eq!(run(replay_config(), input.clone()), input);
        let new_content = vec![pcm(69, 99), pcm(0, 0), pcm(23, 1), pcm(46, 2)];
        let input: Vec<_> = original
            .iter()
            .chain(&sequence(0)[..3])
            .chain(&new_content)
            .cloned()
            .collect();
        let expected: Vec<_> = original.iter().chain(&new_content).cloned().collect();
        assert_eq!(run(replay_config(), input), expected);
    }

    #[test]
    fn packet_order_and_timestamp_deltas_must_match_the_source_run() {
        for replay in [
            vec![pcm(0, 0), pcm(23, 2), pcm(46, 1), pcm(69, 3)],
            vec![pcm(0, 0), pcm(24, 1), pcm(46, 2), pcm(69, 3)],
        ] {
            let input: Vec<_> = sequence(10_000).into_iter().chain(replay).collect();
            assert_eq!(run(replay_config(), input.clone()), input);
        }
    }

    #[test]
    fn audio_and_video_must_agree_on_the_replay_offset() {
        let video = |ts| {
            FlvData::Tag(FlvTag::new(
                ts,
                0,
                FlvTagType::Video,
                false,
                Bytes::from_static(AV1_FRAME),
            ))
        };
        let original = vec![pcm(10_000, 1), video(10_000), pcm(10_023, 2), video(10_033)];
        for video_offset in [0, 1] {
            let replay = vec![
                pcm(0, 1),
                video(video_offset),
                pcm(23, 2),
                video(33 + video_offset),
            ];
            let input: Vec<_> = original.iter().chain(&replay).cloned().collect();
            let expected = if video_offset == 0 {
                original.clone()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected);
        }
    }

    #[test]
    fn candidate_limits_release_packets_before_finish() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        // Repeated payloads cannot confirm replay, so eight pending packets
        // must be released even when no subsequent input arrives.
        let repeated: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..8).map(move |i| pcm(start + i * 23, 1)))
            .collect();
        // Three large but valid PCM packets exceed the lookahead byte budget
        // before confirmation. The history budget is large enough for all three.
        let large: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| {
                (0..3).map(move |i| {
                    let mut data = vec![i as u8; 600 * 1024 + 1];
                    data[0] = 0x3f;
                    FlvData::Tag(FlvTag::new(
                        start + i * 23,
                        0,
                        FlvTagType::Audio,
                        false,
                        Bytes::from(data),
                    ))
                })
            })
            .collect();
        for input in [repeated, large] {
            let mut operator =
                DuplicateTagFilterOperator::with_config(context.clone(), replay_config());
            let mut output = Vec::new();
            for item in &input {
                operator
                    .process(&context, item.clone(), &mut |item| {
                        output.push(item);
                        Ok(())
                    })
                    .unwrap();
            }
            assert_eq!(output, input);
        }
    }

    #[test]
    fn inactivity_expires_candidates_and_confirmed_offsets() {
        for confirmed_packets in [1, 3] {
            let context = StreamerContext::arc_new(CancellationToken::new());
            let mut operator =
                DuplicateTagFilterOperator::with_config(context.clone(), replay_config());
            let start = Instant::now();
            let mut output = Vec::new();
            let mut input = sequence(10_000);
            input.extend(sequence(0)[..confirmed_packets].iter().cloned());
            input.push(pcm(confirmed_packets as u32 * 23, confirmed_packets as u16));
            if confirmed_packets == 1 {
                input.push(pcm(46, 2));
            }
            for (index, item) in input.iter().enumerate() {
                let FlvData::Tag(tag) = item else {
                    unreachable!()
                };
                // Inject time at the processing boundary; no sleeps or wall-clock races.
                let now = if index >= 4 + confirmed_packets {
                    start + Duration::from_millis(501)
                } else {
                    start
                };
                operator
                    .process_media(tag.clone(), now, &mut |item| {
                        output.push(item);
                        Ok(())
                    })
                    .unwrap();
            }
            operator
                .finish(&context, &mut |item| {
                    output.push(item);
                    Ok(())
                })
                .unwrap();
            let expected = if confirmed_packets == 1 {
                input
            } else {
                sequence(10_000).into_iter().chain([pcm(69, 3)]).collect()
            };
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn boundaries_flush_candidates_and_only_source_boundaries_reset_history() {
        for marker in [
            FlvData::Header(FlvHeader::new(true, false)),
            FlvData::EndOfSequence(Bytes::new()),
            FlvData::Split(SplitReason::Discontinuity),
            FlvData::Split(SplitReason::DurationLimit),
        ] {
            let mut input = sequence(10_000);
            input.extend([pcm(0, 0), pcm(23, 1), marker]);
            assert_eq!(run(replay_config(), input.clone()), input);
        }
        for marker in [
            FlvData::Split(SplitReason::SizeLimit),
            FlvData::Split(SplitReason::DurationLimit),
            FlvData::Split(SplitReason::Manual { request_id: 1 }),
        ] {
            assert_eq!(
                run(
                    DuplicateTagFilterConfig::default(),
                    vec![pcm(0, 1), marker.clone(), pcm(0, 1)]
                ),
                vec![pcm(0, 1), marker]
            );
        }
        for marker in [
            FlvData::Header(FlvHeader::new(true, false)),
            FlvData::EndOfSequence(Bytes::new()),
            FlvData::Split(SplitReason::Discontinuity),
        ] {
            let input = vec![pcm(0, 1), marker, pcm(0, 1)];
            assert_eq!(
                run(DuplicateTagFilterConfig::default(), input.clone()),
                input
            );
        }
    }

    #[test]
    fn flushing_a_candidate_propagates_output_failure_without_reemitting_it() {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let mut operator =
            DuplicateTagFilterOperator::with_config(context.clone(), replay_config());
        for item in sequence(10_000).into_iter().chain([pcm(0, 0), pcm(23, 1)]) {
            operator.process(&context, item, &mut |_| Ok(())).unwrap();
        }
        let mut output = Vec::new();
        let result = operator.finish(&context, &mut |item| {
            if !output.is_empty() {
                return Err(PipelineError::Io(std::io::ErrorKind::BrokenPipe.into()));
            }
            output.push(item);
            Ok(())
        });
        assert!(
            matches!(result,Err(PipelineError::Io(error)) if error.kind()==std::io::ErrorKind::BrokenPipe)
        );
        operator
            .finish(&context, &mut |item| {
                output.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(output, vec![pcm(0, 0)]);
    }
}
