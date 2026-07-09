# Refactor plan: FLV/HLS media pipeline

Companion to `perf-review-flv-hls-pipeline.md`. That document lists point findings; this one
answers the structural question: *is a better architecture available?*

## Verdict

**No rewrite is warranted.** The macro-topology is right and should be kept:

```
async downloader → mpsc → [one blocking thread: sync operator chain] → mpsc → [blocking writer thread]
```

- `Processor<T>` (sync, push-based, `&mut dyn FnMut` output) is a good fit for per-item
  transforms and keeps operators trivially testable.
- One processing thread per recording is the correct grain — operators are cheap relative to
  channel hops, so stage-per-thread parallelism would hurt, and the code already avoids it by
  wrapping the whole sync `Pipeline` as a single `ChannelPipeline` stage.
- `Bytes`-everywhere payload ownership and the `FormatStrategy`/`WriterTask` layering are sound.

However, the review's four biggest findings are not independent bugs — they share **three
architectural root causes**. Fixing the root causes fixes the findings structurally and prevents
their recurrence:

| Root cause | Symptoms (findings in the review doc) |
|---|---|
| **A. Items carry only raw bytes; every operator re-derives facts independently** | HLS triple parse (#1); enhanced-RTMP keyframe blind spot (#2); 4× CRC in dup filter (#4); per-tag byte-peeks in ~6 operators |
| **B. Two pipeline runtimes, one of which is used in exactly one shape** | dead flexibility in `ChannelPipeline`; per-item observer cost baked into the core loop (#5); per-item channel hops; item-count (not byte) backpressure (#8) |
| **C. FLV metadata layout is not stable by design** | full-file tail shift at close (#3); 117 KB flat spacer per segment; three components (`script_filler`, writer strategy, `script_modifier`) cooperating implicitly |

The plan below is five bounded phases. Phases are independent — each lands on its own, in
priority order. Phase 0 is tactical hotfixes that should not wait for any refactor.

---

## Phase 0 — Tactical hotfixes (before any refactor)

These are point fixes from the review that are urgent or nearly free:

1. `FlvTag::is_key_frame_nalu`: handle enhanced-RTMP packets (keyframe frame-type +
   `EnhancedPacketType::CodedFrames`/`CodedFramesX`); add a hard cap flush in
   `GopSortOperator::process` regardless of codec. *(memory-exhaustion fix)*
2. `duplicate_filter`: CRC once per tag, reuse keys between `track_and_check` and `track_tag`.
3. `ts.rs::has_psi_tables`: early-exit via sentinel error from the PAT/PMT callbacks;
   `get_ts_stream_profile` with `include_resolution: false` → `parse_stream_info_only()`
   (kills the unused packet Vec).
4. `parser_async.rs::decode_eof`: return after the first decoded frame (also fixes silent loss
   of trailing tags at stream end).
5. Empty-payload guards in `tag.rs` (`is_video_sequence_header`, `get_*_codec_id`).

Each is a small, independently testable diff.

---

## Phase 1 — Classified FLV tags (root cause A, FLV side)

**Problem.** `FlvTag` exposes raw bytes plus ~8 classification methods
(`is_key_frame_nalu`, `is_video_sequence_header`, `is_audio_sequence_header`,
`get_video_codec_id`, …). Six operators call them repeatedly per tag, each re-peeking payload
bytes, and each predicate is a separate place to get codec rules wrong — that is exactly how the
enhanced-RTMP blind spot happened (the rule was updated in the demuxers but not in the
byte-peek predicates).

**Change.** Classify once, at construction:

- Add a small POD to `crates/flv`:

  ```rust
  /// Derived once from the first 1–2 payload bytes when the tag is constructed.
  #[derive(Clone, Copy, Debug, Default)]
  pub struct TagClass {
      pub keyframe: bool,          // enhanced-aware
      pub sequence_header: bool,   // video or audio, per tag_type
      pub end_of_sequence: bool,
      pub enhanced: bool,
      pub codec: Option<CodecKind>, // Avc | Hevc | Av1 | LegacyHevc | Aac | ...
  }
  ```

- Populate it in the three constructors: `FlvTag::demux`, `FlvDecoder::decode`
  (`parser_async.rs`), `FlvParser::parse_tag` (`parser.rs`). All classification logic lives in
  one function (`TagClass::from_payload(tag_type, &data)`).
- Keep the existing predicate methods as one-line field reads for API compatibility; operators
  don't change call sites, they just stop paying per-call parsing and can no longer disagree
  with the demuxers.
- Optional (with finding #4): a `payload_crc: OnceCell<u32>` accessor on `FlvTag` so
  dedup/split/analyzer share one CRC pass per payload.

**Combines naturally with** the decoder cleanup from review finding #6: `FlvDecoder::decode`
already has a fully parsed header at the point it re-demuxes — construct `FlvTag` directly from
the parsed fields + `split_off(TAG_HEADER_SIZE)` and compute `TagClass` right there.

**Touched:** `crates/flv` (tag.rs, parser.rs, parser_async.rs), no operator logic changes.
**Risk:** low; behavior-visible only where the old predicates were wrong.
**Test:** unit tests for `TagClass` across legacy AVC, legacy HEVC, enhanced HEVC/AV1
(sequence header / coded frames / end of sequence); regression test: enhanced-HEVC stream through
`FlvPipeline` must emit GOPs and honor size splits.

## Phase 2 — Analyzed HLS segments (root cause A, HLS side)

**Problem.** `TsSegmentData` exposes ~6 query methods (`has_psi_tables`,
`parse_stream_info_only`, `parse_stream_and_packets`, `get_stream_profile_*`, `has_keyframe`),
each a full O(segment) parse. Operators call them independently → 3 full parses per segment
today, and any new operator adds another.

**Change.** One analysis, computed lazily, cached on the segment:

- Add to `crates/hls`:

  ```rust
  pub struct TsAnalysis {
      pub stream_info: TsStreamInfo,     // PAT/PMT programs, first/last PCR, first PTS per PID
      pub has_psi: bool,
      pub has_random_access: bool,       // any RAI in adaptation fields
      pub resolution: Option<Resolution>,// filled only if probing was requested
  }
  ```

- `TsSegmentData` gains `analysis(&self, opts) -> Result<Arc<TsAnalysis>>` backed by a
  `OnceLock<Arc<TsAnalysis>>` next to `data: Bytes` (`OnceLock<Arc<T>>` is `Clone`, so the enum
  stays `Clone` and buffered clones in defragment share the cache). One pass computes
  everything: PSI, stream info, RAI, and — only when requested — resolution, all inside the
  `on_packet` callback. **No `Vec<TsPacketRef>` is ever materialized.**
- Rewrite the three call sites on it:
  - `DefragmentOperator::process_ts_segment` → `analysis()` (or drop its per-segment profile
    entirely; it only feeds `finish()` — compute there from the last buffered segment).
  - `SegmentSplitOperator::handle_ts_segment` → `analysis()`; RAI comes from the struct;
    resolution probing sets the opts flag per its existing budget logic.
  - `DefragmentOperator::finish` completeness heuristic → `analysis()`.
- Deprecate the per-call query methods (keep as thin wrappers over `analysis()` during
  migration).
- Parser hygiene while here: reuse one `TsParser` per stream via `reset()` instead of
  `make_parser()` per query; replace the `[None; 8192]` / `RefCell<[bool; 8192]>` stack arrays
  with a `SmallVec` keyed by the actual stream PIDs.

**Touched:** `crates/hls` (ts.rs, segment.rs, resolution.rs), `crates/hls-fix` (defragment.rs,
segment_split.rs).
**Risk:** medium — resolution probing semantics must stay budget-driven; keep `SegmentSplit`
deciding *when* to probe, `analysis()` only executing it.
**Test:** instrument the TS parser with a test-only parse counter; assert exactly **one** full
parse per segment through the assembled `HlsPipeline`. Existing split/defragment tests cover
behavior.
**Expected win:** ~⅔ of per-segment CPU in the HLS path, plus the transient ~3.6 MB/segment
allocation traffic.

## Phase 3 — One pipeline runtime, batched bridges, observer out of the core (root cause B)

**Problem.** `pipeline-common` ships two executors. `ChannelPipeline`'s stage-per-thread design
(thread + 2 channel endpoints per processor) is used by nobody: both fix crates wrap the entire
sync `Pipeline` as a single stage. Meanwhile the sync `Pipeline` core loop carries an
unconditional per-item `format!` + indicatif span update, and channel bridges move one item per
hop.

**Change.**

1. **Collapse the runtimes.** Replace `ChannelPipeline` with one explicit bridge that matches
   the only real usage:

   ```rust
   /// Runs `pipeline` on a dedicated blocking thread, bridged by channels.
   pub fn spawn_pipeline<T: Send + 'static>(
       pipeline: Pipeline<T>,
       channel: ChannelSpec,           // items or bytes budget, see Phase 5
   ) -> SpawnedPipeline<T>             // { input_tx, output_rx, handle }
   ```

   `Pipeline` stays the single executor; the `Processor<T>` impl on `Pipeline` (used for
   nesting) can then be deleted along with its wrapped-output plumbing. Callers
   (`flv-fix/src/pipeline.rs`, `hls-fix/src/pipeline.rs`,
   `rust-srec/src/downloader/engine/mesio/*_downloader.rs`, `mesio-cli`) swap
   `ChannelPipeline::new(ctx).add_processor(sync_pipeline).spawn()` for
   `spawn_pipeline(sync_pipeline, spec)`.
2. **Batch the channel hops.** Inside the bridge thread, after a `blocking_recv`, drain whatever
   is immediately available (`try_recv` loop, bounded) and run the batch through the chain
   before the next blocking wait; on the output side, group emissions from one batch into one
   `Vec<T>` channel message to the writer. Amortizes park/unpark and channel contention at high
   tag rates (FLV) without adding latency at low rates (first item is processed immediately).
3. **Move progress out of the core loop.** Replace the inline span update in
   `Pipeline::process_items` (`pipeline.rs:148-153`) with an optional
   `ProgressSink` (`on_items(n)`) called at most every N items / T ms — same pattern as
   flv-fix's `should_update_status`. Default: no sink, zero cost.

**Touched:** `crates/pipeline-common`, thin call-site changes in 4 crates.
**Risk:** low-medium; error-propagation semantics of the current `ChannelPipeline`
(typed error on channel, stage-labeled error on task handle) must be preserved — port its tests.
**Deliberately not doing:** async operators, stage-per-thread parallelism, rayon. The workload
is a linear per-stream chain; parallelism across *recordings* already exists.

## Phase 4 — Layout-stable FLV metadata (root cause C)

**Problem.** The onMetaData rewrite contract is spread across three components:
`ScriptKeyframesFillerOperator` reserves a flat 3.5 h spacer in-stream, the writer records
positions, and `script_modifier` patches at close — falling back to shifting the entire file
tail when the size changed. Nothing *guarantees* the constant-size invariant; it just usually
holds.

**Change.** Make "the script tag payload size never changes after it is first written" an
enforced invariant of the close path:

1. **Size the reservation from configuration**: `FlvPipeline::build_pipeline` already owns both
   `LimitConfig` (max duration/size) and the filler config — derive
   `keyframe_duration_ms` from `max_duration_ms` when set (floor e.g. 1 h ≈ 34 KB instead of
   3.5 h ≈ 117 KB).
2. **Patch in place, always**, in `script_modifier`:
   - new payload smaller → pad the delta with an ignorable AMF field (spacer generalization);
   - new payload larger → truncate the keyframe index to fit (it is a seek optimization, not
     correctness) and log; never grow the tag.
3. **Delete `shift_content_forward`/`shift_content_backward` from the recording path.** Keep
   them only behind an explicit "repair foreign file" entry point (mesio-cli) if that use case
   matters; that path may also want the header-skim scan from review finding #6.
4. Fold the writer-side coordination into `FlvFormatStrategy` so the strategy that wrote the
   file is the one that knows the script-tag position and patches it on `on_file_close` —
   removing the re-open/re-scan in `script_modifier` for files we just wrote.

**Touched:** `crates/flv-fix` (script_filler.rs, script_modifier.rs, writer_task.rs,
amf/builder.rs), `crates/pipeline-common` untouched.
**Risk:** medium — players must tolerate the padding field (they already tolerate today's
zero-filled spacer, same mechanism).
**Test:** write-close-verify: byte-diff the file before/after close and assert only the script
tag region changed; property test across metadata sizes (smaller / equal / would-be-larger).
**Win:** worst case at segment close drops from *2× file size of I/O* to *≤ spacer-size write*;
every segment shrinks by the over-reserved spacer.

## Phase 5 — Byte-budgeted backpressure for segment pipelines (root cause B, memory side)

**Problem.** Backpressure is counted in items (`DEFAULT_CHANNEL_CAPACITY = 32`). For FLV
(tags ≈ KBs) that's fine; for HLS (segments ≈ MBs) a stalled writer can pin ~256 MB per
recording, and `DefragmentOperator` can buffer another ~200 MB waiting for an init segment.

**Change.**

- `ChannelSpec` from Phase 3 gains a bytes mode: a `Semaphore` of budget bytes; the sender
  acquires `min(item_len, budget)` permits before `send`, the consumer releases after the item
  is written/dropped. FLV keeps item mode; HLS gets e.g. a 32–64 MB budget.
- `DefragmentOperator` (hls-fix): cap the pre-init buffer by bytes, not `MAX_BUFFER_SIZE = 50`
  items; on overflow, emit-and-flag rather than clear-and-refill.

**Touched:** `crates/pipeline-common`, `crates/hls-fix`.
**Risk:** low; deadlock-free because a single item larger than the budget must still acquire
the full budget (cap permits at budget size).

---

## What deliberately stays as-is

- `Processor<T>` trait shape and all 13 operator implementations' structure.
- The 3-thread-per-recording topology (downloader task, chain thread, writer thread).
- `Bytes` ownership model and the zero-copy demux path in `crates/flv`.
- `FormatStrategy`/`WriterTask` layering, rotation and progress-callback design.
- Synchronous operators (no async trait); blocking I/O confined to blocking threads.

## Sequencing and effort (rough)

| Phase | Size | Depends on | Primary payoff |
|---|---|---|---|
| 0 hotfixes | XS–S each | — | stops memory blow-up; ~free CPU wins |
| 1 FLV `TagClass` | S | 0.1 | one place for codec rules; removes per-tag re-peeks |
| 2 HLS `TsAnalysis` | M | 0.3 | ~⅔ of HLS per-segment CPU |
| 3 one runtime + batching + observer | M | — | less code, fewer wakeups, clean core loop |
| 4 layout-stable metadata | M | — | kills 2× file-size close I/O; smaller files |
| 5 byte backpressure | S | 3 | bounded memory per recording |

Phases 1+2 eliminate root cause A, phase 3+5 root cause B, phase 4 root cause C. After phase 2,
re-profile before investing in the remaining micro items (per-packet `Bytes` accessor atomics,
PES reassembly copies) — they may no longer matter once the pass count is 1.
