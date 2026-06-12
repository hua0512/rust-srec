# HLS Engine Architecture

This document describes the target architecture for the Mesio HLS engine. The goal is a production-grade downloader with explicit lifecycle ownership, deterministic scheduling, bounded memory behavior, and minimal media-payload copying.

## Goals

- Keep segment lifecycle state in one authoritative component.
- Prevent duplicate segment downloads across playlist refreshes, retries, and scheduler backpressure.
- Preserve output correctness for fMP4 init segments, discontinuities, byte ranges, and live gaps.
- Keep media bytes zero-copy where possible by moving `bytes::Bytes` handles through the pipeline.
- Make retries, terminal failures, and skipped gaps observable and testable.
- Keep implementation modular enough to support future LL-HLS, alternate cache backends, and direct-to-sink output.

## Non-Goals

- Do not maximize zero-copy for metadata at the cost of unclear ownership.
- Do not reintroduce speculative scheduler-side prefetch as core logic.
- Do not make playlist parsing responsible for download execution policy.
- Do not hand-roll protocol parsing when the existing parser crates can represent the data safely.

## Pipeline Overview

```text
PlaylistWatcher
    -> ManifestPlanner
    -> SegmentStateStore
    -> DownloadExecutor
    -> PayloadProcessor
    -> SequenceAssembler
    -> OutputSink
```

Each stage has a narrow contract. Control-plane state and data-plane payload movement are intentionally separate.

## Component Responsibilities

### PlaylistWatcher

Fetches and parses master/media playlists.

Responsibilities:

- Load the initial playlist.
- Select the media playlist variant.
- Refresh live playlists with adaptive interval logic.
- Emit `PlaylistSnapshot` values.
- Preserve raw playlist bytes for unchanged-playlist fast path.

It must not:

- Decide which segments are eligible to download.
- Track in-flight downloads.
- Apply retry policy to individual segments.

### ManifestPlanner

Diffs playlist snapshots into normalized segment descriptors.

Responsibilities:

- Resolve segment, init map, and key URLs once per snapshot.
- Apply inherited playlist query parameters.
- Infer BYTERANGE offsets.
- Convert Twitch prefetch tags into typed descriptors.
- Preserve discontinuity and encryption metadata.
- Emit `SegmentDescriptor` values.

This stage owns all playlist-specific normalization. Later stages should not need to inspect raw `MediaSegment` fields to decide identity.

### SegmentStateStore

Owns the authoritative lifecycle state for every known segment.

Responsibilities:

- Deduplicate discovered, queued, pending, and in-flight work.
- Track retry eligibility and terminal failures.
- Produce ready jobs for the executor.
- Apply executor outcomes.
- Expose skipped/completed state to the sequence assembler.
- Prune old state safely for long-running live streams.

The scheduler/executor should not own lifecycle truth. It asks for ready work and reports outcomes.

### DownloadExecutor

Runs bounded concurrent segment downloads.

Responsibilities:

- Pull ready work from `SegmentStateStore`.
- Prioritize init segments before media, and media before playlist-provided prefetch.
- Enforce download concurrency.
- Apply HTTP and network retry policy for a single executor attempt.
- Emit `SegmentOutcome` with payload or failure.

It must not:

- Dedupe by MSN or URI independently.
- Decide whether a failed segment should be globally retried later.
- Predict future segments.

### PayloadProcessor

Transforms raw segment bytes into HLS payloads.

Responsibilities:

- Decrypt encrypted segments through a bounded crypto executor.
- Fetch and cache decryption keys without blocking async I/O.
- Derive the effective IV from the playlist key tag or MSN.
- Preserve `Bytes` when no transform is needed.
- Convert mutable transforms through `BytesMut` only when required.
- Return typed payloads for init/media/TS data.

This component is where unavoidable byte copies should happen. The rest of the pipeline should move handles.

### SequenceAssembler

Produces ordered stream events from completed payloads and state updates.

Responsibilities:

- Emit fMP4 init segments before applicable media.
- Reorder media by MSN.
- Apply gap policies.
- Emit discontinuity and gap events.
- Reject stale completed segments.
- Continue draining executor output even when reorder buffers are under pressure, because the next completed segment may unblock the buffer.

### OutputSink

Owns the final consumer-facing stream boundary.

Responsibilities:

- Convert assembled payloads into `HlsStreamEvent`.
- Propagate terminal stream events.
- Keep downstream send errors visible.

## Core Data Model

### SegmentKey

`SegmentKey` is the canonical identity for lifecycle and scheduling.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SegmentKey {
    pub kind: SegmentKind,
    pub uri: Arc<str>,
    pub byte_range: Option<ByteRangeKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    Init,
    Media,
    PlaylistPrefetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ByteRangeKey {
    pub length: u64,
    pub offset: u64,
}
```

Rules:

- Do not use MSN alone as identity.
- Do not use formatted strings as the primary identity once this model exists.
- Include BYTERANGE in identity.
- Init segments are keyed independently from media segments.

### SegmentDescriptor

`SegmentDescriptor` is the normalized input to lifecycle scheduling.

```rust
pub struct SegmentDescriptor {
    pub key: SegmentKey,
    pub msn: u64,
    pub uri: Arc<str>,
    pub parsed_url: Arc<Url>,
    pub base_url: Arc<str>,
    pub byte_range: Option<m3u8_rs::ByteRange>,
    pub discontinuity: bool,
    pub encryption: Option<EncryptionDescriptor>,
    pub media_segment: Arc<m3u8_rs::MediaSegment>,
}
```

The descriptor is allowed to carry parser-native structures for compatibility, but identity and scheduling should use typed fields.

### EncryptionDescriptor

`EncryptionDescriptor` is the normalized encryption metadata needed by the payload processor. It should be created by the manifest planner so the processor does not need to reinterpret raw playlist key tags.

```rust
pub struct EncryptionDescriptor {
    pub method: EncryptionMethod,
    pub key_uri: Arc<str>,
    pub iv: EffectiveIv,
    pub key_format: KeyFormat,
}

pub enum EncryptionMethod {
    Aes128Cbc,
    SampleAes,
}

pub enum EffectiveIv {
    Explicit([u8; 16]),
    MediaSequenceDerived(u64),
}

pub enum KeyFormat {
    Identity,
    Unsupported(Arc<str>),
}
```

Rules:

- AES-128 CBC with `KEYFORMAT=identity` is the first supported target.
- If an AES-128 key tag omits IV, derive the IV from the segment MSN before decryption.
- Treat unsupported methods or key formats as terminal segment failures.
- Cache fetched keys by normalized key URI and key format.
- Do not store raw key bytes in logs or tracing fields.

### SegmentState

```rust
pub enum SegmentState {
    Discovered,
    Queued,
    InFlight {
        attempt: u32,
        started_at: Instant,
    },
    Completed {
        completed_at: Instant,
    },
    RetryAt {
        attempt: u32,
        retry_at: Instant,
        reason: Arc<str>,
    },
    TerminalFailed {
        reason: Arc<str>,
    },
    Skipped {
        reason: SkipReason,
    },
}
```

Rules:

- A segment can be scheduled only from `Discovered` or due `RetryAt`.
- `Completed`, `TerminalFailed`, and `Skipped` are not schedulable.
- A retry budget belongs to the state store, not the executor queue.
- State transitions should be unit-tested directly.

### SegmentOutcome

```rust
pub enum SegmentOutcome {
    Completed {
        key: SegmentKey,
        msn: u64,
        payload: SegmentPayload,
    },
    Failed {
        key: SegmentKey,
        msn: u64,
        retryable: bool,
        reason: Arc<str>,
    },
}
```

### SegmentPayload

```rust
pub enum SegmentPayload {
    Ts {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
    Mp4Init {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
    Mp4Media {
        data: Bytes,
        descriptor: Arc<SegmentDescriptor>,
    },
}
```

`SegmentPayload` should move through channels by value. Cloning should clone `Bytes` handles, not byte buffers.

## Zero-Copy Strategy

Use zero-copy where it has a measurable payoff: media payload bytes.

Rules:

- HTTP response bodies should become `Bytes`.
- Cache get/put should use `Bytes` handles.
- Scheduler, processor, assembler, and output channels should move payload structs containing `Bytes`.
- Reorder buffers should store payload handles, not copied buffers.
- Metadata should use simple owned or shared types (`Arc<str>`, `Arc<Url>`, typed keys).
- Do not over-optimize small strings or parser metadata if it makes ownership harder to reason about.

Mutable transforms are the exception:

```text
Bytes -> BytesMut -> decrypt/repair/transmux -> Bytes
```

This allows one intentional copy only when mutation is genuinely required. AES-CBC decryption, transmuxing, or repair stages may need new output bytes.

Future extension:

```rust
pub trait PayloadBuffer {
    fn as_bytes(&self) -> &[u8];
    fn into_bytes(self) -> Bytes;
}
```

This can later support memory cache payloads, file-backed payloads, or direct sink writes without changing scheduling state.

## Encrypted Streams

Encryption and decryption are CPU-bound once the key bytes and encrypted payload are available. The async runtime must not perform AES work on core I/O worker threads.

Recommended encrypted-stream flow:

```text
DownloadExecutor
    -> raw encrypted Bytes
    -> PayloadProcessor
    -> key cache / async key fetch
    -> CryptoExecutor
    -> decrypted Bytes
    -> SequenceAssembler
```

### Key and IV Handling

- Fetch keys asynchronously with retry/backoff.
- Cache successful key fetches by normalized key URI and key format.
- Validate AES-128 keys are exactly 16 bytes.
- Parse explicit IV values once during planning or processing.
- Derive missing AES-128 CBC IVs from the segment MSN.
- Avoid logging raw keys, IVs, cookies, or signed URLs.

### CryptoExecutor

Use a crypto executor abstraction so the engine can change execution strategy without changing the rest of the pipeline.

```rust
pub enum CryptoBackend {
    TokioBlocking,
    DedicatedThreadPool,
    Rayon,
}

pub trait CryptoExecutor {
    async fn decrypt_aes128_cbc(
        &self,
        data: Bytes,
        key: [u8; 16],
        iv: [u8; 16],
    ) -> Result<Bytes, HlsDownloaderError>;
}
```

Default backend:

- `TokioBlocking`.

Reasoning:

- It integrates with the existing Tokio pipeline.
- It avoids blocking async I/O workers.
- Segment-level parallelism already exists through concurrent segment processing.
- AES-128 CBC is chained within a segment, so useful parallelism is mostly across segments, not within one segment.
- It avoids adding a second CPU runtime until profiling proves it is needed.

Optional backend:

- `Rayon`, using a dedicated pool, not the global pool.

Use Rayon only when profiling shows encrypted streams are CPU-bound and Tokio blocking work is contending with other blocking tasks. If enabled, configure a bounded pool:

```rust
let pool = rayon::ThreadPoolBuilder::new()
    .num_threads(crypto_threads)
    .build()?;
```

Do not call the Rayon global pool from random pipeline code. Keep it behind `CryptoExecutor` so CPU budgets are explicit.

### Copy Behavior

AES-CBC decryption requires mutable output. The expected copy path is:

```text
encrypted Bytes -> mutable buffer -> decrypt in place -> decrypted Bytes
```

This is an intentional exception to the zero-copy payload rule. The no-op clear segment path should remain zero-copy; encrypted segments should perform one controlled copy or allocation for decrypted output.

## Scheduling Model

The executor should pull work from the state store:

```rust
let jobs = state_store.next_ready_jobs(available_slots, now);
```

Priority order:

1. Init segments.
2. Media segments.
3. Playlist-provided prefetch segments.

Within the same priority:

1. Lower MSN first.
2. Earlier retry deadline first.
3. Stable descriptor insertion order as tie-breaker.

The ready queue can be implemented with:

- `VecDeque<SegmentKey>` for normal ordered work.
- `BinaryHeap<Reverse<RetryEntry>>` for retry deadlines.
- `HashMap<SegmentKey, SegmentRecord>` for authoritative state.

This avoids repeated batch sorting on hot paths and keeps the scheduler simple.

## Retry Model

There are two retry scopes:

### Attempt Retry

Handled inside `DownloadExecutor` for a single job attempt.

Examples:

- TCP reset.
- timeout while reading body.
- HTTP 429.
- HTTP 5xx.

### Lifecycle Retry

Handled by `SegmentStateStore` after an executor attempt fails.

Examples:

- CDN returns a transient 404 for a segment that may appear shortly.
- network retries were exhausted, but the segment is still relevant.

Recommended classification:

- Retryable: 404, 429, 5xx, network, timeout, body decode/read errors.
- Terminal: 401, 403, malformed URL, unsupported encryption/key state, invalid segment format.

The retry budget should be expressed as lifecycle reschedules, not just HTTP attempts.

## Backpressure and Memory

Backpressure should be based on bytes in the pipeline, not only segment count.

Recommended budgets:

- `max_inflight_download_bytes`
- `max_reorder_buffer_bytes`
- `max_pending_payload_bytes`
- `max_state_entries`

Important rule:

The sequence assembler must continue draining completed payloads even when its reorder buffer is near a configured limit. Otherwise, an older segment queued behind newer segments can never arrive at the assembler, which can create a false gap or a deadlock.

When memory is above budget:

- Stop scheduling new downloads.
- Keep receiving completed outcomes.
- Prefer resolving gaps or emitting ordered payloads.
- If policy allows skipping, mark skipped state explicitly and emit a gap event.

## Observability

Metrics should be attached to state transitions and payload movement.

Recommended counters:

- playlist refresh success/failure
- descriptors discovered
- jobs queued
- jobs deduplicated
- jobs started
- jobs completed
- attempt retries
- lifecycle retries
- terminal failures
- gap skips
- stale completions rejected
- bytes downloaded
- bytes emitted
- key fetch success/failure
- key cache hits/misses
- decryption operations
- decryption bytes
- decryption latency
- crypto executor queue depth
- cache hits/misses
- reorder buffer depth/bytes

Recommended spans:

- playlist URL and refresh generation
- segment key, MSN, kind
- retry attempt and reason
- key URI fingerprint, never raw key bytes
- crypto backend and decrypt duration
- output gap from/to sequence

## Implementation Plan

### Phase 1: Introduce Typed Identity

- Add `SegmentKey`, `ByteRangeKey`, and `SegmentDescriptor`.
- Convert playlist segment discovery to produce descriptors.
- Keep existing scheduler APIs temporarily by adapting descriptors into current jobs.
- Add tests for:
  - same MSN init/media identity separation
  - byte range identity separation
  - query-param inherited identity
  - Twitch prefetch descriptor kind

### Phase 2: Add SegmentStateStore

- Move lifecycle registry responsibilities into `SegmentStateStore`.
- Replace formatted string identities with `SegmentKey`.
- Add explicit state transition tests.
- Keep scheduler active/pending sets only as temporary assertions, then remove them once store ownership is complete.

### Phase 3: Replace Batch Scheduler with Ready Queue

- Implement priority ready queue.
- Pull `next_ready_jobs()` from the state store.
- Remove batch sorting as the primary scheduling primitive.
- Keep a small dispatch coalescing window only if profiling shows it helps.

### Phase 4: Zero-Copy Payload Pipeline

- Ensure fetcher returns `Bytes`.
- Ensure cache stores and returns `Bytes`.
- Make processor return typed `SegmentPayload`.
- Keep no-op transforms as handle moves.
- Convert decrypt/repair paths through `BytesMut` only when required.
- Add `EncryptionDescriptor` and normalize key/IV metadata before processing.
- Introduce `CryptoExecutor` with `TokioBlocking` as the default backend.
- Keep a `Rayon` backend optional and disabled by default until benchmarks justify it.
- Add metrics for decryption latency, bytes, queue depth, and key cache behavior.

### Phase 5: Sequence Assembler Integration

- Feed completed payload outcomes and skipped/failed state into the assembler.
- Make gap decisions using explicit segment state.
- Keep draining completed outcomes under reorder-buffer pressure.
- Add tests for:
  - out-of-order completion under full buffer
  - retryable missing segment followed by late success
  - terminal segment failure followed by configured skip
  - fMP4 init rotation across discontinuities

### Phase 6: Configuration Cleanup

- Remove any remaining scheduler-side prefetch configuration.
- Expose lifecycle retry and byte-budget settings.
- Keep backward-compatible deserialization where persisted config may contain old fields.

## Migration Notes

- Existing `ScheduledSegmentJob` can be replaced by `SegmentDescriptor` plus executor-specific metadata.
- Existing `SegmentLifecycleRegistry` can evolve into `SegmentStateStore` rather than being deleted immediately.
- Existing `OutputManager` can become `SequenceAssembler` with the same external event contract.
- Existing Twitch prefetch handling should remain playlist-provided and lower priority.
- Old prefetch config should remain ignored on deserialization until persisted configs have naturally rolled forward.

## Acceptance Criteria

- A segment cannot be downloaded twice while it is queued or in flight.
- Retryable transient failures are rescheduled only after their retry deadline.
- Terminal failures are never rescheduled.
- Same-MSN init and media segments cannot collide.
- BYTERANGE segments with the same URI but different ranges cannot collide.
- Output does not stall when a later segment fills the reorder buffer before an earlier segment completes.
- No media payload copy happens on the no-op path from fetcher to output.
- Clippy and tests pass with `-D warnings`.
