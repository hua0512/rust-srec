# Performance review: FLV/HLS parsing and fix pipelines

Scope: `crates/flv`, `crates/hls`, `crates/flv-fix`, `crates/hls-fix`, `crates/pipeline-common` (July 2026, ~24k LoC).

**Summary.** The architecture is fundamentally sound: payloads travel as refcounted `Bytes`
handles end-to-end (operator `clone()`s are refcount bumps, not copies), CPU/IO-heavy work runs
on `spawn_blocking` threads, writers use 1 MiB `BufWriter`s with flush only on rotate/close, and
CRC32 delegates to SIMD-capable `zlib_rs`. The big costs are elsewhere: the HLS path re-parses
every TS segment three times, the FLV GOP/split logic degrades to unbounded buffering on
enhanced-RTMP (HEVC/AV1) streams, and FLV file finalization can rewrite an entire multi-GB file.

Findings are ranked by estimated impact. All code references were verified against source.

---

## High impact

### 1. HLS: every TS segment is fully packet-parsed 3×; two passes materialize a `Vec` of every packet

A 4 MB live segment is ~22,000 188-byte TS packets. Per segment:

- **Pass 1** — `hls-fix/src/operators/defragment.rs:101` calls `ts_has_psi_tables()`, which
  parses the whole segment (`hls/src/ts.rs:420-443`) even though PAT/PMT sit in the first few
  packets. `found_psi` is set on the first PAT but the parser keeps stepping through every
  remaining packet — no early exit.
- **Pass 2** — `defragment.rs:108` computes a stream profile via
  `get_stream_profile_with_options(StreamProfileOptions { include_resolution: false })` →
  `parse_stream_and_packets()` (`hls/src/segment.rs:335`), which clones every packet into a
  `Vec<TsPacketRef>` (`hls/src/ts.rs:298`). With `include_resolution: false` that ~1.8 MB Vec
  (plus ~44k atomic refcount ops) is **built and dropped unused** — its only consumer is the
  resolution branch at `segment.rs:374`. The profile itself only feeds a `debug!` log and a
  finish-time completeness heuristic.
- **Pass 3** — `hls-fix/src/operators/segment_split.rs:122` calls `parse_stream_and_packets()`
  again, materializing a second full packet Vec, used mainly for one
  `packets.iter().any(|p| p.has_random_access_indicator())` scan (`:185`).

Cost: ~3× the necessary CPU and ~12 MB memory traffic per 4 MB segment, continuously, per
concurrent recording. This dominates both HLS crates.

**Fix sketch:**
- Parse once per segment and share the result between operators (see refactor plan, Phase 2).
- Make PSI detection early-exit: return a sentinel error from the PAT/PMT callbacks — the parser
  already propagates callback errors and `has_psi_tables` already tolerates `Err` in both arms.
- When `include_resolution` is false, call `parse_stream_info_only()` instead of
  `parse_stream_and_packets()` so no packet Vec is built.
- Add a streaming variant that performs RAI detection and resolution probing inside the
  `on_packet` callback instead of collecting packets
  (`ResolutionDetector::try_pes_reassembly_streaming` is already iterator-based).

Related micro-costs in the same path: `parse_stream_and_packets` zeroes ~144 KB of stack per call
(`[None; 8192]` + two `RefCell<[bool; 8192]>`, `hls/src/ts.rs:230-232`), and
`TsSegmentData::make_parser` (`ts.rs:73`) builds a fresh ~25 KB parser per pass, 3× per segment.
Reuse one parser per stream (it has `reset()`), and replace the PID-indexed arrays with a small
map keyed by the handful of actual stream PIDs.

### 2. FLV: enhanced-RTMP video never matches the keyframe predicate → unbounded buffering, no splits

`FlvTag::is_key_frame_nalu()` (`flv/src/tag.rs:342-358`) returns `false` for every
enhanced-RTMP tag (the `enhanced` branch falls through to `false`) and for legacy codecs other
than AVC/LegacyHevc. Consequences for an HEVC/AV1/enhanced-AVC FLV stream:

- `GopSortOperator` flushes only on `is_key_frame_nalu()` when the stream has video
  (`flv-fix/src/operators/gop_sort.rs:210`); its size-based fallback applies only when
  `has_video == false` (`:212`). `gop_tags` grows at stream bitrate forever
  (≈3.6 GB/hour at 1 MB/s) and nothing is emitted downstream until a header/split/EOS.
- `LimitOperator` uses the same predicate for `can_split_on_tag`
  (`flv-fix/src/operators/limit.rs:296-297`), so size/duration splits never fire either.

The crate parses enhanced tags everywhere else (`hevc.rs`, `av1.rs`), so this is a hot-path
blind spot, not missing codec support. This is a memory-exhaustion bug, not just a perf issue.

**Fix sketch:** teach `is_key_frame_nalu` about enhanced packets (keyframe frame-type +
`EnhancedPacketType::CodedFrames`/`CodedFramesX`), and add a hard cap flush in GopSort as a
safety valve regardless of codec.

### 3. FLV: metadata injection at file close can rewrite the whole file

When the rebuilt `onMetaData` payload differs in size from what's on disk,
`flv-fix/src/script_modifier.rs:170-221` shifts the entire file tail in 64 KB
seek/read/seek/write chunks (`utils/file_utils.rs`) — 2× the segment size in disk I/O
(a 4 GB segment ≈ 8 GB of I/O), competing with the next segment's recording for the disk and
the blocking pool.

With `enable_low_latency` (default) the shift is avoided only if the keyframe-index spacer
written by `ScriptKeyframesFillerOperator` fits. Any segment that outlives the spacer budget
(flat 3.5 h default, `script_filler.rs:59-60`), or any file whose first script tag missed the
filler, falls into the full shift.

Related: the spacer itself is ~117 KB of zero-filled AMF numbers written into **every** segment
(3.5 h ÷ 1.9 s × 2 entries × 9 B), kept forever in the final file.

**Fix sketch:**
- Never shrink: pad the size delta with an ignorable AMF field so the payload size stays
  constant.
- When growth is unavoidable, do one sequential copy pass to a temp file instead of the
  seek ping-pong (or truncate the keyframe index — it is an optimization, not correctness).
- Size the spacer from `LimitConfig::max_duration_ms` instead of a flat 3.5 h; this shrinks both
  the per-file dead weight and the overflow exposure. See refactor plan, Phase 4.

### 4. flv-fix: duplicate filter CRCs each payload 4×

With dedup on (default, `flv-fix/src/pipeline.rs`), every accepted tag gets a full-payload CRC32
in `TagKey::new` and `FingerprintKey::new` inside `track_and_check`
(`operators/duplicate_filter.rs:224,230`), then again for both in `track_tag` (`:156-157`);
replay mode adds more via `TagKey::new_with_timestamp` (`:204,219`). The CRC itself is fast
(zlib-rs SIMD), but this is 4× pure waste plus 4× the cache traffic over each payload — the
largest per-byte compute in the FLV operator chain.

**Fix sketch:** compute the CRC once per tag in `process`, thread it into both key types
(`TagKey::from_parts`, `FingerprintKey::from_parts`), and reuse the keys computed in
`track_and_check` inside `track_tag`. ~10 lines. Bonus: `seen`/`fingerprint_last` SipHash
already-mixed u64 keys — an identity hasher removes another pass per lookup.

---

## Medium impact

### 5. `Pipeline::process_items` does `format!` + indicatif span updates per item

`pipeline-common/src/pipeline.rs:148-153`: one guaranteed heap allocation (`format!`), a TLS
dispatcher lookup (`Span::current()`), and two span-extension lock acquisitions per tag/segment,
even when no progress bar is attached. Negligible per live stream; a top-3 cost when re-muxing
files at 10⁵–10⁶ tags/s. Throttle (every N items / time interval, like flv-fix's own
`should_update_status`) or gate on span activity.

### 6. FLV parsers do redundant per-tag work

- **Sync** `FlvParser::parse_tag` (`flv/src/parser.rs:160-201`): fresh 4 KiB `BytesMut` per tag;
  `resize(total_tag_size, 0)` memsets the whole payload immediately before `read_exact`
  overwrites it; then `FlvTag::demux` re-parses the 11-byte header already parsed at `:172-176`.
  ~250k allocs and a wasted memory pass per GB scanned. `script_modifier`'s onMetaData scan
  (`script_modifier.rs:63-113`) uses this and also reads payloads of tags it discards — a
  header-skim + `seek_relative(data_size + 4)` variant makes that scan near-free.
- **Async** `FlvDecoder::decode` parses each tag header at `flv/src/parser_async.rs:262-266`,
  discards the result, and re-parses it inside `FlvTag::demux` at `:321-326` through
  `Cursor`/`Read` plumbing. The header is already validated: construct the `FlvTag` directly
  from the parsed fields and `split_off(TAG_HEADER_SIZE)`. Also deletes the impossible
  demux-failure branch.
- **`decode_eof`** (`parser_async.rs:356-403`) decodes *all* buffered frames in one call but can
  return only one — the rest are demuxed then discarded with a warning. `FramedRead` calls
  `decode_eof` repeatedly, so returning after the first frame is both faster and fixes silent
  loss of the final tags of every recording. Correctness bug wearing a perf hat.
- `MAX_TAG_DATA_SIZE` check at `parser_async.rs:292` is dead code (`data_size` is a 24-bit
  field); both branches of the `try_resync` result at `:279-289` do the same thing.

### 7. HLS resolution probing can copy the entire video elementary stream

When the per-packet SPS scan fails, `hls/src/resolution.rs:232-266` reassembles PES by
`extend_from_slice`-ing every video payload into a `BytesMut` growing from 4 KiB — a full copy
of most of the segment. For streams with no in-band SPS this repeats for up to 50 segments per
probe window (`segment_split.rs:51,68`; budget reset on every end marker). Size the buffer from
`PES_packet_length` when present, stop after the first complete video PES, and burn the probe
budget faster on no-SPS streams.

### 8. Memory ceilings with multi-MB segments

- Channel capacity is a flat 32 items (`pipeline-common/src/channel_pipeline.rs:14`); input +
  output channels allow ~64 in-flight segments ≈ 256 MB per recording at 4 MB/segment if the
  writer stalls. Fine for FLV tags; wrong unit for HLS segments. Byte-budgeted backpressure
  belongs here (refactor plan, Phase 5).
- `DefragmentOperator::MAX_BUFFER_SIZE = 50` (`hls-fix/src/operators/defragment.rs:57`) can pin
  ~200 MB waiting for an fMP4 init segment, then discards it all and refills. Cap by bytes.
- FLV: cached sequence-header/metadata tags pin the entire decoder buffer they were sliced from
  (64 KiB–4 MiB; `flv-fix/src/operators/split.rs:601-697`, `limit.rs:79-81`). A `detach()` copy
  at those few long-retention sites frees the pinned buffer.

---

## Smaller notes

- `TimingRepairOperator` deep-demuxes every script tag with owned AMF conversion
  (`timing_repair.rs:424-437`) when it only needs 3 numeric fields; chatty CDNs that re-send
  onMetaData pay ~30–80 allocations each time. It also `clone()`s every tag into
  `last_tag`/`last_audio_tag`/`last_video_tag` (`:383-390`) when it only reads timestamp +
  seq-header flag — store `(u32, bool)` per stream instead.
- `gop_sort.rs:132-136` drops Vec capacity every GOP flush (`mem::take` + fresh scratch
  `Vec::new()`s) — keep reusable member buffers.
- AV1 sample validation (`hls-fix/src/analyzer.rs:270-301`) walks every moof/mdat sample per
  segment by default for AV1 fMP4 streams — make opt-in or first-N-segments-per-init.
- Panics on empty payloads: `tag.rs:118` (`is_video_sequence_header`, called per video tag from
  ~6 operators) indexes `bytes[0]` unchecked; `get_video_codec_id`/`get_audio_codec_id`
  (`tag.rs:284-315`) `get_u8()` on possibly-empty data. One-line guards.
- Redundant `BufReader` under `FramedRead` (`flv/src/parser_async.rs:443-447`; mesio's 64 KiB
  BufReader under a 64 KiB FramedRead) adds an extra copy layer — `FramedRead` already reads in
  bulk into its own buffer. Drop the wrapper; raise the default 8 KiB `BUFFER_SIZE`.
- `ts.rs` per-packet accessors (`adaptation_field()`, `payload()`,
  `has_random_access_indicator()`) each allocate a fresh `Bytes` (atomic refcount traffic) to
  read a flag byte — direct index reads remove ~2–6 atomics per packet per pass.

## Verified non-issues

- Payload plumbing is zero-copy `Bytes` throughout both fix pipelines; re-injected cached
  headers/metadata (`split.rs:456-467`, `limit.rs:199-221`) are refcount bumps.
- No O(n²) buffering: gop_sort merges with a linear two-pointer pass; FLV defragment caps at 10
  items; duplicate_filter is hash-based with O(1) eviction.
- Writer I/O: buffered, off the async executor, ~1 syscall per large write, flush only on
  close/rotate; progress callbacks throttled (250 ms / 512 KiB).
- CRC32 implementations are hardware-accelerated (`zlib_rs`); the bytewise MPEG-2 CRC table only
  touches ≤4 KB PSI sections and `validate_crc` defaults to off.
- AMF script data is not re-serialized per tag or per split — once per segment (filler) + once
  per close (modifier).

## Suggested order of attack

1. Finding 2 (enhanced-RTMP keyframe predicate + GopSort cap) — memory-safety-grade.
2. Findings 1 and 3 — the two large sustained CPU/I/O wins.
3. Finding 4 and the `decode_eof` fix from finding 6 — small diffs, immediate payoff.
4. The rest opportunistically, ideally via the structural changes in
   `refactor-plan-media-pipeline.md`.
