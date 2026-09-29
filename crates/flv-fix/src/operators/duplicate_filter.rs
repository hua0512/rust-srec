//! Bounded, byte-verified duplicate suppression for recognized media packets.
//!
//! CRC32 is only a lookup accelerator; matching payload bytes are required before
//! dropping a tag. Payloads are copied into bounded storage so small tags cannot
//! pin large decoder buffers. Opaque, control, and multitrack tags pass through.
//! With offset matching disabled, exact-match history survives timestamp jumps
//! until eviction or a source barrier. A sequence header is a barrier only when
//! its bytes differ from the previous one on that track, because some sources
//! resend unchanged configuration every GOP or immediately before a loop.
//!
//! Optional replay detection buffers a short candidate run after a clock reset.
//! It requires a unique historical run with three distinct payloads and matching
//! timestamp deltas. Ambiguous or incomplete candidates are emitted unchanged.
//! This remains a heuristic and is disabled by default, as is pipeline filtering.
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use bytes::Bytes;
use flv::{FlvData, FlvTag, SplitReason};
use pipeline_common::{PipelineError, Processor, StreamerContext, crc32};
use tracing::{debug, trace};

const REPLAY_CONFIRMATION_PACKETS: usize = 3;
const MAX_PENDING_TAGS: usize = 8;
const MAX_PENDING_BYTES: usize = 1024 * 1024;
const MAX_REPLAY_CANDIDATES: usize = 16;
// Upstream GOP sorting delivers media in bursts. Only stream time determines
// continuity; wall-clock pauses between deliveries must not end a replay.
const REPLAY_GAP_MS: u32 = 500;

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
    /// Minimum per-stream backwards jump that starts offset replay matching.
    /// Ignored when offset matching is disabled; exact-match history is retained.
    pub replay_backjump_threshold_ms: u32,
    /// Opt in to heuristic replay removal after a clock reset. Confirmation
    /// needs three distinct packets in order, at one consistent offset. Pending
    /// packets are released after 500 ms of stream time, after eight packets,
    /// or on finish. Delivery pauses do not expire replay state.
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
    first_timestamp: u32,
}

enum ReplayState {
    Normal,
    Candidate(Candidate),
    Confirmed {
        cursor: ReplayCursor,
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
    // Last sequence header per track (audio, video). Kept across resets so an
    // unchanged repeat after a source barrier is still recognized.
    sequence_headers: [Option<Bytes>; 2],
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
            sequence_headers: [None, None],
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

    /// Resets history unless the header repeats the track's current configuration.
    fn observe_sequence_header(&mut self, tag: &FlvTag) {
        let track = usize::from(tag.is_video_tag());
        if self.sequence_headers[track].as_ref() != Some(tag.data()) {
            self.reset();
            self.sequence_headers[track] = Some(Bytes::copy_from_slice(tag.data()));
        }
    }

    fn begin_replay(&mut self, tag: &SeenTag) {
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
        output: &mut dyn FnMut(FlvData) -> Result<(), PipelineError>,
    ) -> Result<(), PipelineError> {
        let seen = SeenTag::new(&tag);
        let stream = usize::from(tag.is_video_tag());
        if self.config.enable_replay_offset_matching
            && self.last_timestamps[stream].is_some_and(|last| {
                last.saturating_sub(tag.timestamp_ms) > self.config.replay_backjump_threshold_ms
            })
        {
            self.flush_pending(output)?;
            self.last_timestamps = [None; 2];
            self.begin_replay(&seen);
        }
        self.last_timestamps[stream] = Some(tag.timestamp_ms);

        if matches!(&self.replay,
            ReplayState::Confirmed { cursor, last_timestamp }
                if cursor.next == self.history.order.len()
                    && tag.timestamp_ms.abs_diff(*last_timestamp) <= self.config.replay_backjump_threshold_ms.max(REPLAY_GAP_MS)
        ) {
            // A complete replay may loop again with a different timestamp base.
            // Require fresh confirmation rather than reusing its previous offset.
            self.begin_replay(&seen);
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
                let expired = tag.timestamp_ms.abs_diff(candidate.first_timestamp) > REPLAY_GAP_MS;
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
                last_timestamp,
            } => {
                if tag.timestamp_ms.abs_diff(last_timestamp) <= REPLAY_GAP_MS
                    && cursor.advance(&self.history, &seen)
                {
                    self.replay = ReplayState::Confirmed {
                        cursor,
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
            FlvData::Tag(tag) if tag.classification().media => self.process_media(tag, output),
            item => {
                self.flush_pending(output)?;
                if let FlvData::Tag(tag) = &item
                    && tag.sequence_header().is_some()
                {
                    self.observe_sequence_header(tag);
                    return output(item);
                }
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
        self.sequence_headers = [None, None];
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
    use crate::test_utils::fixtures::{
        AAC_LC_CONFIG, AAC_LC_SILENCE, AAC_LC_SILENCE_ENHANCED, AV1_CODEC_CONFIG, AV1_KEYFRAME,
    };

    const PCM_INTERVAL_MS: u32 = 23;

    fn tag(kind: FlvTagType, timestamp: u32, data: &[u8]) -> FlvData {
        FlvData::Tag(FlvTag::new(
            timestamp,
            0,
            kind,
            false,
            Bytes::copy_from_slice(data),
        ))
    }

    // One 16-bit stereo PCM sample; identities differ through actual sample data.
    fn pcm(timestamp: u32, sample: u16) -> FlvData {
        let [lo, hi] = sample.to_le_bytes();
        tag(FlvTagType::Audio, timestamp, &[0x3f, lo, hi, lo, hi])
    }

    /// Four distinct PCM packets, one interval apart.
    fn sequence(start: u32) -> Vec<FlvData> {
        (0..4)
            .map(|index| pcm(start + index * PCM_INTERVAL_MS, index as u16))
            .collect()
    }

    fn shift(items: &mut [FlvData], ms: u32) {
        for item in items {
            if let FlvData::Tag(tag) = item {
                tag.timestamp_ms += ms;
            }
        }
    }

    fn replay_config() -> DuplicateTagFilterConfig {
        DuplicateTagFilterConfig {
            enable_replay_offset_matching: true,
            ..Default::default()
        }
    }

    fn operator(
        config: DuplicateTagFilterConfig,
    ) -> (Arc<StreamerContext>, DuplicateTagFilterOperator) {
        let context = StreamerContext::arc_new(CancellationToken::new());
        let operator = DuplicateTagFilterOperator::with_config(context.clone(), config);
        (context, operator)
    }

    /// Output after processing every item and finishing.
    fn run(config: DuplicateTagFilterConfig, input: Vec<FlvData>) -> Vec<FlvData> {
        let (context, mut operator) = operator(config);
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

    /// Output length after each processed item, without finishing. Shows when
    /// packets are held back and when they are released.
    fn output_len_after_each(
        config: DuplicateTagFilterConfig,
        input: &[FlvData],
    ) -> (Vec<usize>, Vec<FlvData>) {
        let (context, mut operator) = operator(config);
        let mut output = Vec::new();
        let mut lengths = Vec::new();
        for item in input {
            operator
                .process(&context, item.clone(), &mut |item| {
                    output.push(item);
                    Ok(())
                })
                .unwrap();
            lengths.push(output.len());
        }
        (lengths, output)
    }

    #[test]
    fn exact_duplicates_are_removed_but_new_timestamps_survive() {
        for config in [DuplicateTagFilterConfig::default(), replay_config()] {
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
        // AVC/HEVC keyframes: `ffmpeg -f lavfi -i color=c=black:s=16x16:r=25
        // -frames:v 1 -c:v libx264 (or libx265) -f flv`, encoder SEI removed,
        // in legacy and Enhanced-RTMP framing.
        let cases: &[(&str, FlvTagType, &[u8])] = &[
            ("legacy AVC", FlvTagType::Video, b"\x17\x01\0\0\0\0\0\0\x0f\x65\x88\x84\0\x2b\xff\xfe\xf6\x73\x7c\x0a\x6b\x6d\xb1\x81"),
            ("enhanced AVC", FlvTagType::Video, b"\x91avc1\0\0\0\0\0\0\x0f\x65\x88\x84\0\x2b\xff\xfe\xf6\x73\x7c\x0a\x6b\x6d\xb1\x81"),
            ("enhanced HEVC CodedFramesX", FlvTagType::Video, b"\x93hvc1\0\0\0\x0b\x28\x01\xaf\x1d\x80\xee\x23\x8f\xff\x5e\x8f"),
            ("enhanced HEVC", FlvTagType::Video, b"\x91hvc1\0\0\0\0\0\0\x0b\x28\x01\xaf\x1d\x80\xee\x23\x8f\xff\x5e\x8f"),
            ("legacy AAC", FlvTagType::Audio, AAC_LC_SILENCE),
            ("enhanced AAC", FlvTagType::Audio, AAC_LC_SILENCE_ENHANCED),
            ("enhanced AV1", FlvTagType::Video, AV1_KEYFRAME),
        ];
        for &(label, kind, data) in cases {
            let packet = tag(kind, 100, data);
            assert_eq!(
                run(
                    DuplicateTagFilterConfig::default(),
                    vec![packet.clone(), packet.clone()]
                ),
                vec![packet],
                "{label}"
            );
        }
    }

    #[test]
    fn crc_collisions_with_distinct_bytes_are_kept() {
        // Opaque AAC bodies whose CRC32 values collide (asserted below).
        // The filter compares container identity, so AAC is never decoded.
        let first: &[u8] = b"\xaf\x01\x4b\x15\x7c\x8a\xaa\x45\x53\x1c";
        let second: &[u8] = b"\xaf\x01\x91\x5f\x5a\x95\x90\x9a\x61\x66";
        assert_eq!(crc32::crc32(first), crc32::crc32(second));
        let aac = |timestamp, data| tag(FlvTagType::Audio, timestamp, data);

        // Each packet is repeated, so a filter that drops nothing fails too.
        assert_eq!(
            run(
                DuplicateTagFilterConfig::default(),
                vec![
                    aac(100, first),
                    aac(100, first),
                    aac(100, second),
                    aac(100, second)
                ]
            ),
            vec![aac(100, first), aac(100, second)]
        );

        // A colliding packet cannot anchor a replay; the identical one can.
        let original = vec![aac(10_000, first), pcm(10_023, 1), pcm(10_046, 2)];
        for (anchor, replay_removed) in [(second, false), (first, true)] {
            let replay = vec![aac(0, anchor), pcm(23, 1), pcm(46, 2)];
            let input = [original.clone(), replay].concat();
            let expected = if replay_removed {
                original.clone()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected);
        }
    }

    #[test]
    fn retention_is_bounded_by_both_bytes_and_packet_count() {
        // Each PCM packet is 5 bytes. Either limit alone evicts the first packet.
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
        // No retention, or packets larger than the byte budget, keep everything.
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
    fn ineligible_packets_are_kept_and_reset_media_history() {
        use FlvTagType::{Audio, Video};
        let cases: &[(&str, FlvTagType, bool, &[u8])] = &[
            ("filtered PCM", Audio, true, b"\x3f\x01\0\x01\0"),
            ("enhanced audio sequence end", Audio, false, b"\x92Opus"),
            (
                "enhanced audio multichannel config",
                Audio,
                false,
                b"\x94Opus\0",
            ),
            (
                "enhanced audio multitrack",
                Audio,
                false,
                b"\x95\x01Opus\0\x01",
            ),
            ("device-specific sound format", Audio, false, b"\xff\x01"),
            ("empty AAC frame", Audio, false, b"\xaf\x01"),
            ("AVC command frame", Video, false, b"\x57\x01"),
            ("enhanced video metadata", Video, false, b"\x94av01\0"),
            ("enhanced video sequence end", Video, false, b"\x92av01"),
            (
                "enhanced video multitrack",
                Video,
                false,
                b"\x96\x01av01\0\x12\0",
            ),
            (
                "unknown enhanced video FourCC",
                Video,
                false,
                b"\x91????\x01",
            ),
            (
                "AV1 CodedFramesX, defined only for AVC/HEVC",
                Video,
                false,
                b"\x93av01\x12\0",
            ),
            ("empty AVC frame", Video, false, b"\x27\x01\0\0\0"),
        ];
        for &(label, kind, filtered, bytes) in cases {
            let barrier = FlvData::Tag(FlvTag::new(
                0,
                0,
                kind,
                filtered,
                Bytes::copy_from_slice(bytes),
            ));
            let input = vec![pcm(0, 1), barrier.clone(), barrier, pcm(0, 1)];
            assert_eq!(run(replay_config(), input.clone()), input, "{label}");
        }
    }

    fn lc_44k_config() -> FlvData {
        tag(FlvTagType::Audio, 0, AAC_LC_CONFIG)
    }

    fn with_config(config: &FlvData, media: &[FlvData]) -> Vec<FlvData> {
        [std::slice::from_ref(config), media].concat()
    }

    #[test]
    fn unchanged_sequence_headers_keep_history() {
        // Sources may resend unchanged configuration every GOP or before a loop.
        let config = lc_44k_config();
        let original = sequence(10_000);
        for (label, replay, filter) in [
            (
                "exact loop",
                sequence(10_000),
                DuplicateTagFilterConfig::default(),
            ),
            ("offset loop", sequence(0), replay_config()),
        ] {
            let input = [
                with_config(&config, &original),
                with_config(&config, &replay),
            ]
            .concat();
            let expected = [with_config(&config, &original), vec![config.clone()]].concat();
            assert_eq!(run(filter, input), expected, "{label}");
        }
    }

    #[test]
    fn the_current_configuration_is_remembered_across_reconnects() {
        // The header resets history, so pcm(0, 2) is new; the unchanged
        // configuration after it must not reset again.
        let config = lc_44k_config();
        let header = FlvData::Header(FlvHeader::new(true, false));
        let input = vec![
            config.clone(),
            pcm(0, 1),
            header.clone(),
            pcm(0, 2),
            config.clone(),
            pcm(0, 2),
        ];
        assert_eq!(
            run(DuplicateTagFilterConfig::default(), input),
            vec![config.clone(), pcm(0, 1), header, pcm(0, 2), config]
        );
    }

    #[test]
    fn configuration_changes_on_either_track_reset_history() {
        let original = sequence(10_000);
        let av1_config = [&b"\x90av01"[..], AV1_CODEC_CONFIG].concat();
        for (label, second) in [
            ("same track", tag(FlvTagType::Audio, 0, b"\xaf\0\x11\x90")),
            ("other track", tag(FlvTagType::Video, 0, &av1_config)),
        ] {
            let input = [
                with_config(&lc_44k_config(), &original),
                with_config(&second, &original),
            ]
            .concat();
            for filter in [DuplicateTagFilterConfig::default(), replay_config()] {
                assert_eq!(run(filter, input.clone()), input, "{label}");
            }
        }
    }

    #[test]
    fn unparsed_sequence_headers_always_reset_history() {
        // A multitrack sequence start has no single configuration to compare.
        let multitrack = tag(FlvTagType::Video, 0, b"\x96\x00av01\0\x81\0\x0c\0");
        let FlvData::Tag(inner) = &multitrack else {
            unreachable!()
        };
        assert!(inner.classification().sequence_header && inner.sequence_header().is_none());
        let input = vec![multitrack.clone(), pcm(0, 1), multitrack, pcm(0, 1)];
        assert_eq!(
            run(DuplicateTagFilterConfig::default(), input.clone()),
            input
        );
    }

    #[test]
    fn identical_timestamp_loops_are_removed_without_requiring_offset_matching() {
        // The loop jumps back further than the offset threshold, so offset
        // mode starts replay matching while exact mode keeps its history.
        let threshold = DuplicateTagFilterConfig::default().replay_backjump_threshold_ms;
        let count = threshold / PCM_INTERVAL_MS + 2;
        let original: Vec<_> = (0..count)
            .map(|i| pcm(10_000 + i * PCM_INTERVAL_MS, i as u16))
            .collect();
        let new_content = pcm(10_000 + count * PCM_INTERVAL_MS, count as u16);
        let input = [
            original.clone(),
            original.clone(),
            vec![new_content.clone()],
        ]
        .concat();
        let expected = [original, vec![new_content]].concat();
        for config in [DuplicateTagFilterConfig::default(), replay_config()] {
            assert_eq!(run(config, input.clone()), expected);
        }
    }

    #[test]
    fn offset_replays_need_three_distinct_matching_packets() {
        let original = sequence(10_000);
        for (matched, replay_removed) in [(2, false), (3, true)] {
            let replay = &sequence(0)[..matched];
            let new_content = pcm(matched as u32 * PCM_INTERVAL_MS, 99);
            let input = [original.clone(), replay.to_vec(), vec![new_content.clone()]].concat();
            let expected = if replay_removed {
                [original.clone(), vec![new_content]].concat()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected, "{matched} matched");
        }
    }

    #[test]
    fn replay_is_opt_in_and_requires_distinct_ordered_evidence() {
        let original = sequence(10_000);
        let replay = sequence(0);
        let input = [original.clone(), replay].concat();
        assert_eq!(
            run(DuplicateTagFilterConfig::default(), input.clone()),
            input
        );
        assert_eq!(run(replay_config(), input), original);

        // Valid repeated silence has no distinct evidence that it is a replay.
        let input: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| {
                (0..10).map(move |i| {
                    tag(
                        FlvTagType::Audio,
                        start + i * PCM_INTERVAL_MS,
                        AAC_LC_SILENCE,
                    )
                })
            })
            .collect();
        assert_eq!(run(replay_config(), input.clone()), input);
        // Two alternating payloads also fail the distinct-evidence requirement.
        let input: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..6).map(move |i| pcm(start + i * PCM_INTERVAL_MS, (i % 2) as u16)))
            .collect();
        assert_eq!(run(replay_config(), input.clone()), input);
    }

    #[test]
    fn repeated_payload_anchors_and_new_offsets_require_fresh_confirmation() {
        // The first payload repeats inside the run, giving two anchors.
        let run_at = |start| {
            [1, 2, 1, 3]
                .into_iter()
                .zip(0..)
                .map(|(sample, i)| pcm(start + i * PCM_INTERVAL_MS, sample))
                .collect::<Vec<_>>()
        };
        let original = run_at(10_000);
        let input = [original.clone(), run_at(0), run_at(1000), run_at(0)].concat();
        assert_eq!(run(replay_config(), input), original);
    }

    #[test]
    fn mismatches_release_candidates_and_end_confirmed_replays() {
        let original = sequence(10_000);
        // The third packet differs before confirmation: everything is kept.
        let mut unconfirmed = sequence(0);
        unconfirmed[2] = pcm(2 * PCM_INTERVAL_MS, 99);
        let input = [original.clone(), unconfirmed].concat();
        assert_eq!(run(replay_config(), input.clone()), input);
        // After three matches confirm the replay, the first new packet ends it,
        // and packets that follow are no longer treated as replay.
        let confirmed = sequence(0)[..3].to_vec();
        let new_content = [vec![pcm(3 * PCM_INTERVAL_MS, 99)], confirmed.clone()].concat();
        let input = [original.clone(), confirmed, new_content.clone()].concat();
        assert_eq!(
            run(replay_config(), input),
            [original, new_content].concat()
        );
    }

    #[test]
    fn packet_order_and_timestamp_deltas_must_match_the_source_run() {
        let original = sequence(10_000);
        for (label, replay, replay_removed) in [
            ("matching run", sequence(0), true),
            (
                "reordered",
                vec![pcm(0, 0), pcm(23, 2), pcm(46, 1), pcm(69, 3)],
                false,
            ),
            (
                "different delta",
                vec![pcm(0, 0), pcm(24, 1), pcm(46, 2), pcm(69, 3)],
                false,
            ),
        ] {
            let input = [original.clone(), replay].concat();
            let expected = if replay_removed {
                original.clone()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected, "{label}");
        }
    }

    #[test]
    fn audio_and_video_must_agree_on_the_replay_offset() {
        let video = |ts| tag(FlvTagType::Video, ts, AV1_KEYFRAME);
        let original = vec![pcm(10_000, 1), video(10_000), pcm(10_023, 2), video(10_033)];
        for (video_offset, replay_removed) in [(0, true), (1, false)] {
            let replay = vec![
                pcm(0, 1),
                video(video_offset),
                pcm(23, 2),
                video(33 + video_offset),
            ];
            let input = [original.clone(), replay].concat();
            let expected = if replay_removed {
                original.clone()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected);
        }
    }

    #[test]
    fn candidate_limits_release_held_packets_before_finish() {
        // Repeated payloads cannot confirm a replay. The replay's first seven
        // packets are held, and the eighth releases all of them.
        let repeated: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..8).map(move |i| pcm(start + i * PCM_INTERVAL_MS, 1)))
            .collect();
        let (lengths, output) = output_len_after_each(replay_config(), &repeated);
        assert_eq!(lengths, [1, 2, 3, 4, 5, 6, 7, 8, 8, 8, 8, 8, 8, 8, 8, 16]);
        assert_eq!(output, repeated);

        // Two packets just over half the lookahead budget cannot be held
        // together, so the second releases the first.
        let large = |timestamp, fill: u8| {
            let mut data = vec![fill; MAX_PENDING_BYTES / 2 + 1];
            data[0] = 0x3f;
            tag(FlvTagType::Audio, timestamp, &data)
        };
        let large: Vec<_> = [10_000, 0]
            .into_iter()
            .flat_map(|start| (0..3).map(move |i| large(start + i * PCM_INTERVAL_MS, i as u8)))
            .collect();
        let (lengths, output) = output_len_after_each(replay_config(), &large);
        assert_eq!(lengths, [1, 2, 3, 3, 5, 6]);
        assert_eq!(output, large);
    }

    #[test]
    fn stream_timestamp_gaps_expire_candidates_and_confirmed_offsets() {
        for (matched, replay_removed) in [(1, false), (3, true)] {
            // Payloads and relative timestamps still match history, but a gap
            // over the 500 ms replay gap ends suppression after `matched` packets.
            let mut original = sequence(10_000);
            let mut replay = sequence(0);
            shift(&mut original[matched..], REPLAY_GAP_MS + 1);
            shift(&mut replay[matched..], REPLAY_GAP_MS + 1);
            let input = [original.clone(), replay.clone()].concat();
            let expected = if replay_removed {
                [original, replay[matched..].to_vec()].concat()
            } else {
                input.clone()
            };
            assert_eq!(run(replay_config(), input), expected, "{matched} matched");
        }
    }

    #[test]
    fn non_media_items_release_held_packets_in_order() {
        for marker in [
            FlvData::Header(FlvHeader::new(true, false)),
            FlvData::EndOfSequence(Bytes::new()),
            FlvData::Split(SplitReason::Discontinuity),
            FlvData::Split(SplitReason::DurationLimit),
            FlvData::Split(SplitReason::SizeLimit),
            tag(FlvTagType::Audio, 0, AAC_LC_CONFIG),
        ] {
            let input = [
                sequence(10_000),
                vec![pcm(0, 0), pcm(23, 1), marker.clone()],
            ]
            .concat();
            let (lengths, output) = output_len_after_each(replay_config(), &input);
            // Both replay packets are held until the marker arrives.
            assert_eq!(lengths[4..], [4, 4, 7], "{marker:?}");
            assert_eq!(output, input, "{marker:?}");
        }
    }

    #[test]
    fn only_source_boundaries_reset_history() {
        for (marker, resets) in [
            (FlvData::Header(FlvHeader::new(true, false)), true),
            (FlvData::EndOfSequence(Bytes::new()), true),
            (FlvData::Split(SplitReason::Discontinuity), true),
            (FlvData::Split(SplitReason::SizeLimit), false),
            (FlvData::Split(SplitReason::DurationLimit), false),
            (FlvData::Split(SplitReason::Manual { request_id: 1 }), false),
        ] {
            let input = vec![pcm(0, 1), marker.clone(), pcm(0, 1)];
            let expected = if resets {
                input.clone()
            } else {
                vec![pcm(0, 1), marker.clone()]
            };
            assert_eq!(
                run(DuplicateTagFilterConfig::default(), input),
                expected,
                "{marker:?}"
            );
        }
    }

    #[test]
    fn flushing_a_candidate_propagates_output_failure_without_reemitting_it() {
        let (context, mut operator) = operator(replay_config());
        for item in [sequence(10_000), vec![pcm(0, 0), pcm(23, 1)]].concat() {
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
        assert!(matches!(
            result,
            Err(PipelineError::Io(error)) if error.kind() == std::io::ErrorKind::BrokenPipe
        ));
        operator
            .finish(&context, &mut |item| {
                output.push(item);
                Ok(())
            })
            .unwrap();
        assert_eq!(output, vec![pcm(0, 0)]);
    }
}
