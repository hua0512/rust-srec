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

The stages below are a logical data flow, not a one-task-per-stage deployment:

```text
PlaylistWatcher
    -> ManifestPlanner        (deterministic snapshot diff)
    -> SegmentStateStore      (identity, lifecycle, scheduling)
    -> fetch-and-process      (download + decrypt -> SegmentPayload)
    -> SequenceAssembler
    -> OutputSink
```

The runtime collapses these into **two owned loops plus an off-thread crypto
pool**, not a chain of tasks passing work to each other:

```text
Task A  PlaylistWatcher        async playlist polling; emits PlaylistSnapshot
                                   |
                                   v  (snapshot channel)
Task B  Scheduler Reactor      owns SegmentStateStore; runs ManifestPlanner on
                               each snapshot; drives bounded fetch-and-process
                               futures; applies their outcomes; forwards finished
                               payloads downstream
           |  spawns per-segment tasks (JoinSet, bounded by max_concurrency)
           |  each task: fetch -> CryptoExecutor (off-thread) -> SegmentPayload
                                   |
                                   v  (AssemblerInput channel)
Task C  SequenceAssembler /     reorder by MSN, gap policy, init-before-media,
        OutputSink             terminal events

        CryptoExecutor pool    CPU-bound AES, off the reactor thread
```

Control-plane state lives in exactly one place — the reactor (Task B). It owns the
`SegmentStateStore`, so identity, dedup, retry budget, and scheduling priority are
all decided in one loop with no shared lock and no cross-task work-handoff
protocol. Data-plane payloads move by value: out of the spawned futures and across
a single downstream channel to the assembler.

The reactor loop never blocks. Network I/O happens inside the spawned
fetch-and-process futures; AES happens in the crypto pool; the loop itself only
mutates state and decides what to spawn next. Concurrency comes from the spawned
futures, bounded by the dispatch gate's `inflight.len() < max_concurrency` check —
the reactor is the sole spawner, so no semaphore is needed — not from a second
scheduling task.

### Why a reactor, not a stage-per-task pipeline

A task-per-stage design has to hand work between the store and an executor over
channels, which forces lifecycle truth to be split (or duplicated) across tasks
and needs an unbounded feedback channel for outcomes. Folding the store owner and
the download driver into one loop removes both: outcomes return through a
`JoinSet`, which is intrinsically bounded by the dispatch gate's concurrency
limit, and the single source of truth is never copied across a task boundary. Backpressure
becomes one gate (slots ∧ inflight-bytes ∧ pending-bytes) checked in one place,
and shutdown becomes one `select!` arm. See Scheduling Model for the loop itself.

The reactor also makes the LL-HLS goal cheap rather than a new pipeline shape.
Each LL-HLS feature maps onto machinery the loop already has:

- Blocking playlist reload (`_HLS_msn` / `_HLS_part` long-poll) is just the
  `PlaylistWatcher`'s request; the reactor sees the resulting snapshot like any
  other.
- Partial segments (parts) reuse the scheduling and byte-budget machinery, but they
  require a part dimension in identity that does not exist yet: the current
  `SegmentKey` has no part index. Adding LL-HLS means first defining that — a
  `PartKey`/part index on `SegmentKey` (extend `ByteRangeKey` or add a part field) —
  and only then do parts dedup like segments.
- Preload hints (`EXT-X-PRELOAD-HINT`) would be descriptors with a low-priority
  `SegmentSource`, the same way Twitch prefetch is. Whether a preload hint collapses
  into the published part is **not** automatic: it needs an explicit
  preload→published-part promotion rule (the hint URL and the eventual part must map
  to one identity, with the same fetch-URL-refresh-on-rediscovery handling). That
  rule must be specified before claiming preload dedup is safe.

Given those two additions, a higher part rate just tops up more slots per loop pass
and the byte budgets bound memory unchanged — no new stage, no new task. The point
is that LL-HLS fits the reactor's *shape*; it still needs the part identity model
and promotion rule defined, not assumed.

## Component Responsibilities

### PlaylistWatcher

Fetches and parses master/media playlists.

Responsibilities:

- Load the initial playlist.
- Select the media playlist variant.
- Refresh live playlists with adaptive interval logic.
- Apply source-specific textual preprocessing before parsing (e.g. rewriting
  `#EXT-X-TWITCH-PREFETCH` tags into standard segment entries), so the parser only
  ever sees standard playlist syntax. Classifying those segments — prefetch
  priority, ad filtering — is the planner's job, not the watcher's.
- Emit `PlaylistSnapshot` values.
- Preserve raw playlist bytes for unchanged-playlist fast path.

It must not:

- Decide which segments are eligible to download.
- Track in-flight downloads.
- Apply retry policy to individual segments.
- Send consumer-facing events (`EndlistEncountered`, `PlaylistRefreshed`)
  directly. The reactor derives those from the snapshot it plans (see
  `AssemblerInput::Notice`), so the client event channel has exactly one
  producer.

#### Snapshot channel

The watcher publishes snapshots through a **coalescing, single-slot, latest-wins**
channel (`tokio::sync::watch`), not an unbounded queue. Coalescing keeps a busy
reactor from drowning in stale snapshots, but it can drop intermediate generations.
That is safe — but only because of explicit gap handling, not because the latest
snapshot is a superset:

- **A live media playlist is a sliding window, not a true superset.** Segments enter
  and leave the window over time, so "the latest snapshot still contains everything"
  is false. The reactor must not assume contiguity across coalesced generations.
- **Discovery still happens against the store.** The reactor re-runs `plan()` on the
  latest snapshot it reads and diffs against the store, so every segment still in the
  window is discovered regardless of how many generations coalesced before it.
- **Window-slides are marked, never silent.** Each snapshot carries a monotonic
  **generation** and `media_sequence_base`. `plan()` compares the snapshot's first
  MSN against the last contiguous planned MSN; any hole — segments that left the
  window during a coalesced gap, the only genuinely unrecoverable case — is returned
  as an explicit missing range and forwarded as `AssemblerInput::Skipped`. A lost
  segment becomes a visible gap, never a silent stall.
- **Coalescing rarely drops anything in practice.** The reactor loop is non-blocking
  (see The Reactor Loop), so it iterates faster than snapshots arrive; coalescing
  only drops generations under extreme overload, where falling behind the live edge —
  and marking the skipped range — is the correct, honest behavior. An unbounded mpsc
  would instead pile up stale snapshots; a bounded mpsc would stall polling.
- **Snapshots are cheap to clone.** The discovery arm clones the borrowed value on
  every read (`borrow_and_update().clone()`), so `PlaylistSnapshot` keeps its parsed
  segment list behind `Arc` — a clone copies handles, never the parsed playlist.
- **Consumer drop stops the watcher.** If all snapshot receivers are dropped, the
  watcher exits even while a live playlist is byte-identical and nothing is being
  published. Receiver closure is a lifecycle signal, not a terminal stream cause.
- **The terminal cause is carried, never inferred from a sender drop.** A dropped
  watch sender is ambiguous: it happens on a clean finish *and* when the watcher task
  dies on a fetch/parse error. So the snapshot carries the cause explicitly:

  ```rust
  pub enum TerminalCause { Endlist, Failed(Arc<str>) }
  // PlaylistSnapshot { ..., terminal: Option<TerminalCause> }
  ```

  On `EXT-X-ENDLIST` the watcher publishes a snapshot with
  `terminal = Some(Endlist)` (retained as the latest value), then drops the sender.
  On a fetch/parse failure it publishes `terminal = Some(Failed(reason))` if it can,
  then drops. The reactor reads the retained value before the close, so:
  - `Some(Endlist)` → begin the authoritative-end drain (→ `StreamEnded`).
  - `Some(Failed(_))` → pipeline error (→ `Err`).
  - `changed()` → `Err` with no terminal value seen → the watcher died before
    signalling: pipeline error (`Terminal::WatcherFailed`), **not** a clean end.

  A watcher failure can therefore never masquerade as a clean ENDLIST drain.

### ManifestPlanner

Diffs playlist snapshots into normalized segment descriptors. This is a
deterministic function the reactor calls on each `PlaylistSnapshot` —
`plan(&snapshot, &store, &mut ctx) -> Planned { descriptors, missing }` — not a
task. `descriptors` are the new segments to schedule; `missing` are explicit MSN
ranges the snapshot proves were dropped from the window before being seen (see
Snapshot channel).

`ctx` is a `PlannerContext` owned by the reactor and threaded through every call,
because two pieces of normalization are inherently cross-snapshot and cannot be
derived from a single snapshot:

- **The BYTERANGE inference chain.** A snapshot's first BYTERANGE segment may
  continue a chain started in the previous snapshot, so the last inferred range
  end (`last_byterange_uri` / `last_byterange_end`) must survive across refreshes.
  Resetting it at each refresh would mis-skip every chain-continuing segment at a
  refresh boundary — the property the existing
  `process_segments_preserves_byterange_context_across_refreshes` test pins down.
- **The last contiguous planned MSN**, the baseline for window-slide gap
  detection.

`plan()` is pure over `(snapshot, store, ctx)` — no I/O, no clock reads — so
normalization stays unit-testable in isolation: tests construct a context, feed a
sequence of snapshots, and assert descriptors, missing ranges, and how the context
evolves. The reactor loop stays thin either way.

Responsibilities:

- Resolve segment, init map, and key URLs once per snapshot.
- Apply inherited playlist query parameters.
- Apply the `IdentityPolicy` to derive each `SegmentKey.uri` (see Identity Normalization), so rotated auth params do not fork identity.
- Track `EXT-X-MAP` scope: a snapshot-level map seeds the scope before the first
  segment on each `plan()` call, segment-scoped maps override as they are
  encountered, and a refresh with no map declaration keeps the carried scope from
  the previous snapshot.
- Detect MSN-base gaps: compare the snapshot's first MSN against the last contiguous planned MSN (tracked in `PlannerContext`) and return any hole as a `missing` range rather than skipping silently.
- Distinguish a window that moved *backwards*: a small regression is a stale CDN
  edge (plan nothing, wait for a fresh window — never re-anchor the BYTERANGE
  chain or regress fetch URLs); a regression beyond a few window-lengths is a
  media-sequence reset, returned as `Planned::reset`. The reactor surfaces a
  reset as a terminal pipeline error, because the assembler's emit cursor never
  regresses — re-based segments would be stale-rejected forever, a silent
  download-and-discard loop. A reset means the stream restarts as a new session.
- Infer BYTERANGE offsets, resolving each to an absolute `ByteRangeKey.offset` (skip when neither explicit nor inferable); the inference chain lives in `PlannerContext` so it survives refresh boundaries. The chain anchor is keyed by **MSN** (a segment at MSN *m* with a matching URI and no explicit offset starts where MSN *m−1* ended): keying on MSN — not just URI — keeps offsets deterministic when an overlapping window is re-scanned, so a new tail segment infers from the true end of its immediate predecessor rather than a stale anchor left over from a prior generation. Already-decided segments are re-emitted on every refresh (the store uses them to refresh volatile fetch metadata), so the chain advances through the whole window each pass.
- Convert Twitch prefetch tags into descriptors with `source = PlaylistPrefetch`, keying them by the same `SegmentKey` they will carry once they appear as normal media.
- Drop ad segments (e.g. Twitch ad-marked segments): no descriptor is emitted, so
  they never enter lifecycle state and never reach the assembler.
- Preserve discontinuity and encryption metadata.
- Emit `SegmentDescriptor` values.

The planner owns all playlist-specific normalization. Later stages should not need to inspect raw `MediaSegment` fields to decide identity.

### Scheduler Reactor

A single task that owns the control plane. It is the only place lifecycle truth
lives, and it is also the thing that drives downloads — so there is no separate
executor task and no work-handoff channel between "the store" and "the executor".

The reactor is a `select!` loop (see Scheduling Model for the body) that:

- Ingests `PlaylistSnapshot` values, runs `ManifestPlanner`, and registers new
  descriptors in the `SegmentStateStore`.
- Spawns bounded fetch-and-process futures for ready work, gated by the
  `inflight.len() < max_concurrency` dispatch bound and the byte budgets.
- Applies each finished future's `SegmentOutcome` back into the store.
- Forwards `AssemblerInput` items (payloads, skips, terminal failures, end) to the assembler.
- Wakes on the earliest retry deadline to reschedule due work.
- Tears down deterministically on cancellation or authoritative end.

It must not block. All network I/O is inside the spawned futures; all AES is in
the crypto pool. The loop only mutates state and decides what to spawn.

#### SegmentStateStore

The owned state inside the reactor. Not an `Arc<Mutex<..>>`; it never leaves the
loop.

Responsibilities:

- Deduplicate discovered, queued, pending, and in-flight work by `SegmentKey`.
- On re-discovery of a known `SegmentKey`, refresh volatile fetch metadata
  (`parsed_url`, key fetch URL, byte range, `source`) while preserving lifecycle
  state (see Re-discovery must refresh fetch metadata). Never retry a stale URL.
- Track retry eligibility and terminal failures, owning the lifecycle retry budget.
- Produce ready jobs in priority order via `next_ready_job(now, &budget)` /
  `next_ready_jobs(slots, now, &budget)`, reserving the estimated download bytes
  against the budget as part of admission (see Byte budget ownership). The returned
  job owns the RAII reservation, so admission and byte-charging are one step.
- Report `has_unfinished_work()` — true while any segment is `Discovered`, `Queued`,
  `InFlight`, or `RetryAt` (any deadline, including future ones). This, not
  "nothing schedulable right now", is the authoritative-end drain predicate: a
  segment waiting on a future retry deadline is unfinished work and must hold the
  stream open until it completes, terminalizes, or is skipped.
- Apply outcomes, mapping `FailureClass` to retryable-vs-terminal. `apply_outcome`
  returns the `AssemblerInput` items the outcome implies (a `Payload` on success, a
  `TerminalFailed`/`Skipped` when it terminalizes or skips), so the assembler is told
  about segments that will never complete.
- Prune old state for long-running live streams under one invariant: **never
  evict an entry whose `SegmentKey` can still appear in the playlist window**.
  Evicting a `Completed` entry still inside the window makes the next refresh
  re-discover and re-download it, breaking the no-duplicate guarantee — so
  pruning removes only entries below the current window's first MSN, keeping
  init entries and in-flight work (as `SegmentLifecycleRegistry::prune_before_msn`
  does today). The init-retention cap (`max_retained_inits`) is itself subject to
  the invariant: it may evict only init records *below* the window start, never an
  in-window init (which the next refresh would otherwise re-discover and
  re-download). `max_state_entries` is a backstop applied *within* that rule, not a
  substitute for it: when nothing can be evicted safely, state temporarily
  exceeds the cap rather than violating the invariant.

#### fetch-and-process future

The per-segment data-plane unit the reactor spawns. It collapses what would
otherwise be a `DownloadExecutor` and a `PayloadProcessor` into one future, so the
payload is finished by the time the reactor observes the outcome.

Responsibilities:

- Download `descriptor.parsed_url` into `Bytes`, applying attempt-level HTTP and
  network retry for this single attempt.
- Charge and release the shared `Arc<ByteBudget>` (download bytes reserved before
  reading the body and reconciled to actual; the `processing` permit reserved as an
  upper bound before decrypt and reconciled after) — this is why the budget is shared
  with the task and not owned by the store (see Byte budget ownership).
- Decrypt through the `CryptoExecutor` (off-thread) when the descriptor carries
  encryption; otherwise move the `Bytes` handle unchanged.
- Return a `SegmentOutcome` carrying either a typed `SegmentPayload` or a
  `FailureClass` — it reports the class, it does not decide retry policy.

It must not:

- Dedupe by MSN or URI, or touch the store directly.
- Decide whether a failed segment is globally retried later, or its priority.
- Predict future segments.

### PayloadProcessor

Transforms raw segment bytes into HLS payloads. This is the processing half of the
fetch-and-process future, not a separate task — it runs in the same spawned future
that downloaded the bytes, so no channel hop sits between fetch and decrypt.

Responsibilities:

- Decrypt encrypted segments through a bounded crypto executor.
- Fetch and cache decryption keys without blocking async I/O.
- Derive the effective IV from the playlist key tag or MSN.
- Preserve `Bytes` when no transform is needed.
- Convert mutable transforms through `BytesMut` only when required.
- Return typed payloads for init/media/TS data.

This component is where unavoidable byte copies should happen. The rest of the pipeline should move handles.

### SequenceAssembler

Produces ordered stream events from a single typed input stream. The reactor→assembler
channel carries `AssemblerInput`, not bare payloads, so that completions, skips,
terminal failures, and end-of-stream all cross the boundary in order:

```rust
pub enum AssemblerInput {
    Payload(SegmentPayload),
    /// The store gave up on these MSNs (gap-skip policy or terminal failure of a
    /// segment the assembler is waiting on). The assembler must stop waiting and
    /// advance past them.
    Skipped { from_msn: u64, to_msn: u64 },
    /// A specific segment will never arrive (terminal failure).
    TerminalFailed { key: SegmentKey, msn: u64 },
    /// Playlist-level notice derived by the reactor from a planned snapshot.
    /// Forwarded to the sink immediately on drain, never reordered; counts
    /// toward max_pending_items, never toward payload bytes.
    Notice(PlaylistNotice),
    /// Authoritative end: drain the reorder buffer in order, then emit StreamEnded.
    End,
}

pub enum PlaylistNotice {
    PlaylistRefreshed { media_sequence_base: u64, target_duration: f32 },
    EndlistEncountered,
    // extend as the external HlsStreamEvent contract requires
}
```

Without `Skipped`/`TerminalFailed`/`End` on this channel, a segment the store has
already marked `TerminalFailed` or `Skipped` would leave the assembler blocked on an
MSN that can never complete. The reactor derives these items from
`store.apply_outcome` and from planner-detected missing ranges.

`Notice` exists so the consumer-facing channel has exactly one producer. Today the
playlist engine sends `EndlistEncountered`/`PlaylistRefreshed` straight to the
client channel while the output task sends data events, so playlist events can race
ahead of in-flight segments. Here the watcher never touches the client channel: the
reactor derives notices from the snapshot it just planned (`PlaylistRefreshed` from
a new generation, `EndlistEncountered` from `TerminalCause::Endlist`) and pushes
them through `pending` like any other item. The assembler forwards a `Notice`
immediately on drain — notices carry no MSN and are never reordered — preserving
today's at-refresh-time semantics with a single ordered producer. `SegmentTimeout`
and `GapSkipped` remain assembler-generated.

`Skipped` carries an MSN **range**, and the reactor coalesces on enqueue
(`push_skipped`): pushing `Skipped { from, to }` adjacent to or overlapping the tail
of `pending` extends that item instead of appending a new one. This bounds control
items independently of how wide a window-slide is — a 10 000-segment gap is one
`Skipped`, not 10 000 — and complements the `max_pending_items` cap.

Responsibilities:

- Emit fMP4 init segments before applicable media — gated per init *key*, not
  just "any init seen". Each media descriptor carries the `SegmentKey` of its
  governing `EXT-X-MAP` (`SegmentDescriptor::init_key`); the assembler holds a
  media segment until that specific init key has arrived, so a rotated or
  slow-to-fetch init cannot lose the race against the first media it covers. An
  init that terminally fails marks its key failed, and dependent media is then
  skipped as a visible gap rather than gating the stream forever (this also
  covers a first init failing before any media payload arrives). The arrived and
  failed init-key sets are bounded; if very high init rotation evicts an old key
  while dependent media is still buffered, the media is treated as unresolved:
  live streams wait until the reorder buffer reaches its configured limit and
  then force a visible `GapSkipped`, while ENDLIST/VOD flush emits a visible
  `GapSkipped` instead of silently dropping the media.
- Reorder media by MSN.
- Advance past `Skipped`/`TerminalFailed` MSNs instead of waiting on them.
- Apply gap policies.
- Emit discontinuity and gap events.
- Forward `Notice` items to the sink immediately, without reordering.
- Reject stale completed segments (stale = MSN below the emit cursor: already
  emitted or already skipped).
- Continue draining `AssemblerInput` even when reorder buffers are under pressure, because the next item may unblock the buffer.

When the reorder buffer is at `max_reorder_buffer_bytes` and blocked on a missing
MSN, the assembler does not wait out the gap policy's count/duration threshold:
buffer-over-budget forces the skip decision immediately — the blocking gap is
skipped, the gap event is emitted, and emission resumes. A late completion for a
skipped MSN is then rejected as stale. This is the defined enforcement action for
`max_reorder_buffer_bytes`; waiting at the cap would recreate exactly the deadlock
the keep-draining rule exists to prevent. The same fail-safe applies when the
cursor media is gated on an unresolved fMP4 init key: below the buffer limit the
assembler still waits for a slow init, but at the limit it skips that media as a
visible `GapSkipped` and advances.

### OutputSink

Owns the final consumer-facing stream boundary.

Responsibilities:

- Convert assembled payloads into `HlsStreamEvent`.
- Convert `PlaylistNotice` items into their `HlsStreamEvent` counterparts
  (`PlaylistRefreshed`, `EndlistEncountered`).
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
- Include BYTERANGE in identity. `ByteRangeKey.offset` is the resolved absolute
  offset, not an `Option`: the manifest planner must infer it from the prior
  segment's end (the `last_byterange_end` inference path) before building the
  key. A BYTERANGE that has no explicit offset and no inferable predecessor is a
  skip, not an `offset == 0` guess.
- Init and media at the same URI are distinct resources, so `SegmentKind`
  separates them in identity. Prefetch is **not** a kind: a Twitch
  `PREFETCH_SEGMENT` URL is the same resource that reappears as a normal media
  segment on the next refresh, so keying it by a `PlaylistPrefetch` kind would
  split one resource into two keys and download its bytes twice — exactly the
  cross-refresh duplication this model exists to prevent. Prefetch-ness lives on
  `SegmentDescriptor::source` (scheduling priority only), never on the key.

### Identity Normalization

`SegmentKey.uri` is a typed handle, but typing alone does not make identity
stable — the string it holds still has to be stable across playlist refreshes for
dedup to work. This is the policy that actually decides whether the
"no duplicate downloads across refreshes" goal is met, so it is specified here
rather than left to URL construction.

The problem: token-bearing CDNs (Twitch, signed-URL providers) can rotate auth
query parameters on every refresh while the underlying segment is unchanged. If
the rotated query is part of identity, the same segment looks new on each refresh
and is downloaded repeatedly. If too much of the URL is stripped, distinct
segments collide.

Rules:

- Identity is the normalized path plus an explicit set of **significant** query
  keys. Insignificant keys (rotating tokens, signatures, expiries) are excluded
  from the normalized URI used to build `SegmentKey.uri`.
- Which query keys are significant is **per-source policy**, because sources
  differ (Twitch already has bespoke handling). Expose it as a small hook:

  ```rust
  pub trait IdentityPolicy {
      /// Produce the canonical identity URI for a resolved segment URL.
      fn canonical_uri(&self, resolved: &Url) -> Arc<str>;
  }
  ```

- The default policy keeps the full resolved URL (current behavior), so sources
  without a known token scheme are never under-deduplicated. The consequence is
  that rotated-auth-param dedup is **not** a global guarantee: it holds only for a
  source that has a configured token-aware policy. The matching acceptance
  criterion is scoped the same way — stripping arbitrary query params by default
  would risk merging genuinely distinct segments, which is the worse failure.
- Normalization happens once, in the manifest planner, before the key exists.
  Later stages compare keys, never URLs.
- MSN is not a substitute for URI identity: it resets across discontinuities and
  playlist reloads, so it cannot anchor identity (see the MSN-alone rule above).

### SegmentDescriptor

`SegmentDescriptor` is the normalized input to lifecycle scheduling.

```rust
pub struct SegmentDescriptor {
    pub key: SegmentKey,
    pub msn: u64,
    pub source: SegmentSource,
    pub parsed_url: Arc<Url>,
    pub discontinuity: bool,
    pub encryption: Option<EncryptionDescriptor>,
    /// For media governed by an EXT-X-MAP: the key of that init resource. The
    /// assembler gates this media's emission on the init's arrival, so a
    /// rotated init cannot lose the race against the first media it covers.
    pub init_key: Option<SegmentKey>,
    pub media_segment: Arc<m3u8_rs::MediaSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentSource {
    Playlist,
    PlaylistPrefetch,
}
```

The descriptor is allowed to carry parser-native structures for compatibility, but identity and scheduling should use typed fields.

`source` carries prefetch-ness for scheduling priority only; it is deliberately
absent from `SegmentKey` (see `SegmentKind`). When a prefetch URI later appears
as a normal media segment, it resolves to the same `SegmentKey`, so the state
store recognizes it as already known and the `source` simply upgrades from
`PlaylistPrefetch` to `Playlist`.

`key.uri` and `parsed_url` are not redundant: `key.uri` is the normalized
identity string (auth params stripped per `IdentityPolicy`), while `parsed_url`
is the full URL actually fetched, retaining the rotating auth params the CDN
requires. Identity dedup uses `key.uri`; the fetch-and-process future downloads `parsed_url`.

#### Re-discovery must refresh fetch metadata

Because identity is stable but the fetch URL is not, re-discovering an existing
`SegmentKey` is **not a no-op** — it must refresh the volatile fetch metadata, or
dedup will pin a stale (possibly expired) signed URL and later fetch or retry the
wrong request. When `store.ingest` sees a descriptor whose `SegmentKey` already
exists, it merges by lifecycle state:

- `Discovered`, `Queued`, `RetryAt`: refresh `parsed_url`, the encryption fetch URL
  (see EncryptionDescriptor), and volatile descriptor fields (byte range,
  discontinuity, `source` upgrade) from the newest descriptor. Preserve lifecycle
  state — attempt count, retry deadline, insertion order. The next fetch or retry
  then uses the fresh URL.
- `InFlight`: leave the in-flight attempt's URL alone (it is already downloading),
  but record the refreshed `parsed_url` so that if the attempt fails and reschedules
  to `RetryAt`, the retry uses the fresh URL, not the one that was in flight.
- `Completed`, `TerminalFailed`: no refresh — these are never fetched again.

This is the same identity-stable / fetch-volatile split applied over time rather
than across the prefetch→media transition.

For `SegmentKind::Init`, `msn` is the media sequence number of the first segment
the init map covers (its `EXT-X-MAP` position), used by the sequence assembler to
decide which media an init applies to. It is ordering metadata only and never
participates in identity, so a rotated init across a discontinuity is a new
`SegmentKey` (new URI) carrying the MSN at which it takes effect.

### EncryptionDescriptor

`EncryptionDescriptor` is the normalized encryption metadata needed by the payload processor. It should be created by the manifest planner so the processor does not need to reinterpret raw playlist key tags.

```rust
pub struct EncryptionDescriptor {
    pub method: EncryptionMethod,
    /// Normalized cache identity for the key (auth params stripped per the
    /// source's IdentityPolicy). Stable across refreshes.
    pub key_identity_uri: Arc<str>,
    /// Full URL actually fetched, retaining rotating auth params. Refreshed on
    /// re-discovery, exactly like SegmentDescriptor::parsed_url.
    pub key_fetch_url: Arc<Url>,
    pub iv: EffectiveIv,
    pub key_format: KeyFormat,
}

pub enum EncryptionMethod {
    Aes128Cbc,
    /// Any method the processor cannot decrypt yet (SAMPLE-AES, AES-256, ...).
    /// Carries the raw method token for diagnostics. Always maps to a terminal
    /// segment failure; do not add per-method variants until the decrypt path
    /// for that method actually exists.
    Unsupported(Arc<str>),
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
- If an AES-128 key tag omits IV for media, derive the IV from the segment MSN
  before decryption.
- If an AES-128 key tag applies to an `EXT-X-MAP`, it must include an explicit
  IV. A missing init-map IV is normalized to `EncryptionMethod::Unsupported` so
  the init segment fails terminally instead of being decrypted with a media MSN.
- `EncryptionMethod::Unsupported` and `KeyFormat::Unsupported` map to terminal
  segment failures. SAMPLE-AES is intentionally not a first-class method: it
  needs NAL/container-aware partial decryption, not a cipher swap, so it stays in
  `Unsupported` until that path is implemented.
- Cache fetched keys by `key_identity_uri` and key format — never by the full
  `key_fetch_url`, whose rotating auth params would defeat every cache hit.
  Resolve `key_identity_uri` with the same source-specific `IdentityPolicy` used
  for segment identity.
- Honor a key cache TTL; on expiry, re-fetch using the latest `key_fetch_url`
  (refreshed on re-discovery), not the URL the key was first fetched with.
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
        class: FailureClass,
        reason: Arc<str>,
    },
    TerminalFailed {
        class: FailureClass,
        reason: Arc<str>,
    },
    Skipped {
        reason: SkipReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Http(u16),
    Network,
    Timeout,
    Decode,
    UnsupportedCrypto,
    InvalidFormat,
}
```

`FailureClass` is the machine-readable classification; `reason: Arc<str>` is the
human-readable detail. Retry policy (see Retry Model) and the per-class
observability counters key off `FailureClass`, so neither parses the string.

Rules:

- A segment can be scheduled only from `Discovered` or due `RetryAt`.
- `Completed`, `TerminalFailed`, and `Skipped` are not schedulable.
- A retry budget belongs to the state store, not the per-attempt fetch retry inside the future.
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
        class: FailureClass,
        reason: Arc<str>,
    },
}
```

The fetch-and-process future reports the observed `FailureClass`; the state store
decides retryable-vs-terminal from that class and the remaining lifecycle budget,
rather than the future pre-deciding a `retryable` bool.

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

Recommended encrypted-stream flow, all inside one fetch-and-process future except
the final hand-off:

```text
fetch-and-process future:
    download             -> raw encrypted Bytes
    key cache / key fetch
    CryptoExecutor       -> decrypted Bytes (off-thread)
    wrap                 -> SegmentPayload
                              |
                   (reactor) -> SequenceAssembler
```

### Key and IV Handling

- Fetch keys asynchronously with retry/backoff, using `key_fetch_url` (full URL).
- Cache successful key fetches by `key_identity_uri` and key format, with a TTL;
  signed key URLs rotate, so the identity URI is the cache key and the fetch URL is
  refreshed on re-discovery.
- Coalesce concurrent fetches of the same key (single-flight by
  `key_identity_uri`): at a key rotation, up to `max_concurrency` fetch-and-process
  futures hit the same uncached key at once, and without coalescing each issues its
  own request. One fetch is in flight per identity; the rest await its result.
- Validate AES-128 keys are exactly 16 bytes.
- Parse explicit IV values once during planning or processing.
- Derive missing AES-128 CBC IVs from the segment MSN.
- Avoid logging raw keys, IVs, cookies, or signed URLs.

### CryptoExecutor

Use a crypto executor abstraction so the engine can change execution strategy without changing the rest of the pipeline.

Dispatch on a backend enum rather than a `dyn` trait. A trait with
`async fn decrypt_aes128_cbc` is not dyn-compatible, so `Box<dyn CryptoExecutor>`
would not compile — which defeats the goal of swapping backends at runtime. Since
the backend set is closed and already enumerated, an enum that matches internally
gives runtime selection with no object-safety problem and no `async_trait`
boxing:

```rust
pub enum CryptoBackend {
    TokioBlocking,
    DedicatedThreadPool,
    Rayon,
}

pub struct CryptoExecutor {
    backend: CryptoBackend,
    // e.g. an owned rayon::ThreadPool when backend == Rayon
}

impl CryptoExecutor {
    pub async fn decrypt_aes128_cbc(
        &self,
        data: Bytes,
        key: [u8; 16],
        iv: [u8; 16],
    ) -> Result<Bytes, HlsDownloaderError> {
        match self.backend {
            CryptoBackend::TokioBlocking => { /* spawn_blocking */ }
            CryptoBackend::DedicatedThreadPool => { /* pool + oneshot */ }
            CryptoBackend::Rayon => { /* pool.spawn + tokio oneshot bridge */ }
        }
    }
}
```

The `Rayon` and `DedicatedThreadPool` arms bridge a synchronous pool back into
async with a `oneshot` per operation; account for that bridge when comparing
against `TokioBlocking`. If a generic seam is ever wanted, make it a
static-dispatch trait bound (`fn new<E: CryptoExecutor>`), not a trait object.

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

### The Reactor Loop

The `SegmentStateStore` is owned by the reactor task and never leaves it — not an
`Arc<Mutex<..>>`, and not a separate task the executor messages for work. The same
loop that owns the state also drives the downloads, so the only "channel" between
scheduling and execution is the spawned task itself. In-flight work lives in a
`JoinSet` (chosen deliberately — see below); concurrency is bounded by the
dispatch gate's `inflight.len() < max_concurrency` check (the reactor is the sole
spawner, so the JoinSet's own length is the limit), and outcomes return through
`JoinSet::join_next`, which is intrinsically bounded by that same limit (there is
no separate, sizeable outcome channel to overflow under a failure storm).

The loop must never `.await` a blocking operation inside a `select!` arm — doing so
suspends every other arm (discovery, retry, cancellation, other completions). The
two places this matters are completion forwarding and snapshot intake, handled by a
permit-driven forward and a coalescing watch channel respectively:

```rust
// `pending`: reactor-local VecDeque<AssemblerInput>. Carries payloads AND control
// items (Skipped / TerminalFailed / Notice / End). Bounded two ways: payload bytes
// against max_pending_payload_bytes, total items against max_pending_items; adjacent
// Skipped ranges are coalesced on push so control items cannot grow unbounded (see
// SequenceAssembler).
// `pending_payload_bytes`: running counter of payload bytes in `pending`, maintained
// on push/pop — never recomputed by walking the buffer.
// `planner_ctx`: PlannerContext — cross-snapshot BYTERANGE inference chain and last
// contiguous planned MSN (see ManifestPlanner).
// `budget`: Arc<ByteBudget> shared with the spawned tasks (see Byte Budget Ownership).
// The reactor reserves download bytes at admission (via next_ready_job) and hands the
// RAII reservation to the task; the task reconciles it to the real size and reserves
// processing bytes itself.
let mut ending = false;     // a terminal cause was seen: drain known work, then finish
let mut end_queued = false; // AssemblerInput::End enqueued exactly once
loop {
    let terminal = tokio::select! {
        // Discovery. `if !ending` disables this arm once ending starts, so a closed
        // watch (changed() returns Err immediately, forever) cannot hot-loop.
        // Also gated on pending capacity: while `pending` is at its item cap, suspend
        // snapshot intake so missing-range pushes can't grow it unbounded. The watch
        // retains the latest value (incl. a terminal one), so nothing is lost — the
        // arm re-enables and re-reads once `pending` drains.
        changed = playlist_rx.changed(),
            if !ending && pending.len() < max_pending_items => {
            match changed {
                Ok(()) => {
                    let snapshot = playlist_rx.borrow_and_update().clone();
                    // The terminal cause is explicit on the snapshot, never inferred
                    // from a sender drop. A `Failed` snapshot is the only one not
                    // planned; an ENDLIST snapshot still carries the final window, so
                    // it is planned/ingested like any other *before* `ending` is set.
                    match snapshot.terminal {
                        Some(TerminalCause::Failed(reason)) =>
                            Some(Terminal::PipelineError(reason)),
                        terminal => {
                            let planned = plan(&snapshot, &store, &mut planner_ctx);
                            store.ingest(planned.descriptors, Instant::now());
                            for r in planned.missing { push_skipped(&mut pending, r); }
                            // Snapshot-derived notices (PlaylistRefreshed /
                            // EndlistEncountered) cross the same ordered boundary
                            // as payloads — see AssemblerInput::Notice.
                            push_notices(&mut pending, &snapshot);
                            if matches!(terminal, Some(TerminalCause::Endlist)) {
                                ending = true;
                            }
                            None
                        }
                    }
                }
                // Sender dropped WITHOUT a terminal snapshot first: the watcher task
                // died (fetch/parse failure), not a clean end. Pipeline error, not End.
                Err(_) => Some(Terminal::WatcherFailed),
            }
        }
        // Completion: apply_outcome returns the assembler events this outcome implies
        // — Payload on success, TerminalFailed/Skipped when the store terminalizes or
        // skips. All cross the same boundary, in order.
        Some(joined) = inflight.join_next() => {
            match joined {
                Ok(outcome) => {
                    for ev in store.apply_outcome(&outcome, Instant::now()) {
                        push_input(&mut pending, ev);    // coalesces adjacent Skipped
                    }
                    None
                }
                Err(e) if e.is_cancelled() => None,      // aborted on shutdown
                Err(e) => Some(Terminal::TaskPanic(e)),  // surfaced, not swallowed
            }
        }
        // Forward: only when an item is buffered AND downstream has a permit.
        permit = assembler_tx.reserve(), if !pending.is_empty() => {
            match permit {
                Ok(permit) => { permit.send(pending.pop_front().unwrap()); None } // sync send
                Err(_) => Some(Terminal::DownstreamClosed), // sink gone: terminal, visible
            }
        }
        // Due retries are promoted out of the heap every loop pass (not only
        // at admission), so a deadline that elapses while the dispatch gate
        // is closed becomes Queued work and stops driving this arm — a past
        // deadline must never leave the arm permanently ready (busy-spin).
        // next_retry_deadline() returns a far-future Instant when no retry is
        // pending, keeping this arm inert rather than spinning.
        _ = sleep_until(store.next_retry_deadline()) => None,
        _ = cancel.cancelled() => Some(Terminal::Cancelled),
    };
    if let Some(cause) = terminal { break cause; }

    // Authoritative end: enqueue End only once *all lifecycle work is finished*, not
    // merely once nothing is schedulable right now. `has_unfinished_work()` covers
    // Discovered/Queued/InFlight AND every RetryAt (including future deadlines), so a
    // final-window segment waiting on a retry deadline holds the stream open — the
    // sleep_until(next_retry_deadline) arm wakes the loop to run it before End.
    if ending && !store.has_unfinished_work() {
        if !end_queued {
            pending.push_back(AssemblerInput::End);   // last item; drains via forward arm
            end_queued = true;
        } else if pending.is_empty() {                // End has been forwarded
            break Terminal::AuthoritativeEnd;
        }
    }

    // Top-up gate. The download-byte budget is enforced *at admission*: next_ready_job
    // reserves the estimated download bytes against `budget` and hands back a job that
    // owns the RAII reservation, returning None if there is no ready work OR the budget
    // cannot fit the estimate. Reserving before the spawn (not inside the task) closes
    // the check-then-spawn-then-reserve race where many tasks pass an advisory read
    // before any of them has charged. Pending payload bytes / item count stay reactor-
    // local gates. This runs while `ending` too — the final ENDLIST/VOD window is ready
    // work that must download before the drain condition above can be satisfied.
    let now = Instant::now();
    while inflight.len() < max_concurrency
        && pending_payload_bytes < max_pending_payload_bytes
        && pending.len() < max_pending_items
    {
        let Some(job) = store.next_ready_job(now, &budget) else { break };  // reserves or None
        inflight.spawn(fetch_and_process(job, processor.clone(), crypto.clone(), budget.clone()));
    }
}
```

Why this shape:

- **`reserve()`-then-`send` forwarding, not `send().await`.** A bare
  `assembler_tx.send(payload).await` inside the completion arm would stall the whole
  reactor whenever the assembler is backpressured — no snapshots polled, no retries
  fired, no cancellation observed. Buffering into `pending` and forwarding only when
  `assembler_tx.reserve()` yields a permit keeps every arm live. A closed downstream
  (`reserve()` → `Err`) is **terminal and surfaced**, never `.ok()`-swallowed,
  honoring the OutputSink "send errors visible" contract.
- **`JoinSet`, not `FuturesUnordered`.** Cancellation semantics differ:
  `JoinSet::drop`/`abort_all` aborts its tasks, whereas a `FuturesUnordered` of
  detached `JoinHandle`s would drop the handles and leave the spawned tasks running.
  The shutdown contract depends on abort-on-drop, so the in-flight set is a `JoinSet`
  (`spawn`/`join_next`/`abort_all`).
- **`pending` is bounded in two dimensions, from both producers.** Payload items
  count against `max_pending_payload_bytes`; *all* items (including near-zero-byte
  `Skipped`/`TerminalFailed`) count against `max_pending_items`. Both producers
  respect the item cap: the dispatch gate stops spawning downloads (so completion
  pushes stay bounded), and the discovery arm is guarded by
  `pending.len() < max_pending_items` (so snapshot-derived missing-range pushes are
  suspended, not unbounded). The only overshoot is the in-flight set's completions
  (≤ `max_concurrency`), which cannot be refused once the work is done. Adjacent
  `Skipped` ranges are also coalesced on push (`push_skipped`/`push_input`), so a
  wide window-slide is one item, not many. `push_*` never drop items — the bound is
  enforced by suspending the producer, never by discarding an event.
- **One typed boundary to the assembler.** `pending` holds `AssemblerInput`, not
  bare payloads, so terminal failures, skips, and end-of-stream cross the same
  ordered channel as payloads. Without this, the assembler would wait forever on a
  segment the store has already marked `TerminalFailed` or `Skipped` (see
  SequenceAssembler).
- **Explicit terminal cause, not inferred from a sender drop.** The snapshot carries
  `terminal: Option<TerminalCause>` (`Endlist` or `Failed`). Only `Endlist` begins the
  authoritative-end drain; `Failed`, and a bare `changed()` → `Err` (the watcher task
  died before signalling anything), both become a pipeline-error terminal. A watcher
  fetch/parse failure can never masquerade as a clean ENDLIST.
- **`ending` separates "no more snapshots" from "stop working".** It is disabled on
  the discovery arm via `if !ending`, so the now-closed watch cannot hot-loop the
  reactor. The loop keeps draining already-known work, enqueues `AssemblerInput::End`
  as the final item, forwards it, and only then breaks `Terminal::AuthoritativeEnd`.
  Cancellation, by contrast, breaks immediately.

`store.next_ready_job(now, &budget)` returns the highest-priority schedulable
segment, reserves its estimated download bytes against `budget`, and marks it
in-flight — all in one call, so dedup, scheduling, and byte admission share one
decision point with no lock, no work-handoff protocol, and no check-then-reserve
race. It returns `None` when there is no ready work *or* the budget cannot fit the
estimate. `next_ready_jobs(slots, now, &budget)` is just the batched form for a
single top-up pass:

```rust
let jobs = store.next_ready_jobs(available_slots, now, &budget);
```

The reactor body must stay cheap: `ingest`, `apply_outcome`, `next_ready_job`, and
the permit-send are in-memory operations. All network I/O lives in the spawned
tasks; all AES lives in the crypto pool. If the loop ever does real work, it stops
being able to dispatch.

### Priority

Priority order:

1. `SegmentKind::Init`.
2. `SegmentKind::Media` with `source == Playlist`.
3. `SegmentKind::Media` with `source == PlaylistPrefetch`.

Prefetch ranks last by reading `SegmentDescriptor::source`, not by a distinct
`SegmentKind`, so a prefetch URI and its later media incarnation still resolve to
one `SegmentKey` (see Core Data Model).

Within the same priority:

1. Lower MSN first.
2. Earlier retry deadline first.
3. Stable descriptor insertion order as tie-breaker.

Lower-MSN-first is a deliberate ordering choice, not a neutral default: after a
stall it drains the oldest pending media before the live edge, favoring ordered
catch-up over latency. That is correct for archival/VOD output. A future
low-latency-live mode may want bounded catch-up or newest-first; keep the
comparator pluggable rather than hard-coding the order across the scheduler.

### Ready Queue Structures

The ready queue can be implemented with:

- `VecDeque<SegmentKey>` for normal ordered work.
- `BinaryHeap<Reverse<RetryEntry>>` for retry deadlines.
- `HashMap<SegmentKey, SegmentRecord>` for authoritative state.

This avoids repeated batch sorting on hot paths and keeps the scheduler simple.

The `HashMap` is the single source of truth; the `VecDeque` and `BinaryHeap` are
**advisory** indexes. Pruning for long-running live streams (`max_state_entries`)
can evict a `HashMap` entry whose `SegmentKey` still sits in the queue or heap, so
a popped key may be stale or absent. Therefore `next_ready_jobs` re-validates each
popped key against the `HashMap` and silently drops keys that are gone or no
longer schedulable (lazy deletion / tombstones). Never treat a queue/heap entry as
authoritative on its own.

## Retry Model

There are two retry scopes:

### Attempt Retry

Handled inside the fetch-and-process future for a single job attempt.

Examples:

- TCP reset.
- timeout while reading body.
- HTTP 429.
- HTTP 5xx.

An attempt holds a concurrency slot and its download byte reservation for its
whole duration, and it can only use the URL captured at spawn — re-discovery
refreshes the store, never a running future. So attempt retry must be tight: a
small fixed attempt count with short backoff, bounded well below a segment
duration. Anything longer-lived returns to the store as a lifecycle retry, which
frees the slot and the reservation and re-engages the refreshed-URL path.

### Lifecycle Retry

Handled by `SegmentStateStore` (in the reactor) after a fetch-and-process attempt fails.

Examples:

- CDN returns a transient 404 for a segment that may appear shortly.
- network retries were exhausted, but the segment is still relevant.

Recommended classification, expressed over `FailureClass`:

- Retryable: `Http(404 | 429 | 500..=599)`, `Network`, `Timeout`, `Decode`.
- Conditionally retryable: `Http(401 | 403)` — see auth-failure rule below.
- Terminal: `UnsupportedCrypto`, `InvalidFormat`, and malformed URLs (which never
  produce a job).

The state store owns this mapping: the fetch-and-process future reports a
`FailureClass`, and the store decides retryable-vs-terminal from the class and the
remaining lifecycle budget. The retry budget should be expressed as lifecycle
reschedules, not just HTTP attempts.

#### Auth failures and stale signed URLs

`Http(401 | 403)` is **not unconditionally terminal**, because a signed URL can
expire mid-flight while a newer playlist has already refreshed it (see Re-discovery
must refresh fetch metadata). The store records the descriptor generation each
in-flight attempt used. On a 401/403:

- If the store now holds a **fresher** `parsed_url`/`key_fetch_url` for that
  `SegmentKey` than the failing attempt used (re-discovery advanced the generation),
  treat it as retryable and reschedule against the fresh URL, consuming one
  lifecycle reschedule. The denial was against a URL that is already stale.
- If the attempt already used the freshest URL (no newer generation), terminalize —
  the auth denial is real, not an expiry.

This is bounded: each refreshed-URL retry spends lifecycle budget, so a CDN that
401/403s even the freshest URL terminalizes once the budget is exhausted, with no
retry loop.

## Backpressure and Memory

Backpressure should be based on bytes in the pipeline, not only segment count.

Recommended budgets:

- `max_inflight_download_bytes` — raw response bodies, from download start until
  consumed (wrapped on the clear path, or fed to decrypt on the encrypted path).
- `max_processing_bytes` — decrypted/transformed **output** resident in the
  decrypt/transform stage. The encrypted input stays under download bytes, so input
  and output are both counted while they coexist (see release points below).
- `max_reorder_buffer_bytes` — payloads buffered in the assembler awaiting order.
- `max_pending_payload_bytes` — completed payloads buffered in the reactor between
  completion and the downstream permit-send.
- `max_pending_items` — total `AssemblerInput` items buffered in the reactor,
  including near-zero-byte control items (`Skipped`/`TerminalFailed`/`Notice`/`End`)
  that `max_pending_payload_bytes` does not bound. Without it, control items pile up
  unbounded under a slow downstream.
- `max_state_entries` — control-plane records, independent of bytes.

A segment's size is not known at admission — no HTTP request has been made yet, so
there is no `Content-Length` to consult. The admission estimate for
`max_inflight_download_bytes` is therefore `ByteRangeKey.length` when the segment
has a byte range (the one size the playlist states exactly), and otherwise a
running estimate of recent actual segment sizes, falling back to a configured
per-segment default before any segment has completed (without that fallback the
budget cannot gate at all). The reservation is then reconciled twice: against
`Content-Length` when the response headers arrive, and against the actual size as
the body streams. A body that exceeds its
reservation does not silently overrun: the task acquires additional capacity before
appending each over-budget chunk and aborts the segment if the budget cannot be
extended (see Byte budget ownership).

### Byte lifecycle and release points

Each budget must have a defined charge and release point, with **no uncounted
window in between**, or encrypted streams undercount and the crypto stage becomes
an unbounded sink. The raw body therefore stays charged until decrypt actually
consumes it — it is never released "on body completion" into a gap while a task
waits for a crypto slot:

- `max_inflight_download_bytes` covers the raw response body from download start
  until the body is no longer resident: on the clear path, until the payload is
  wrapped and its handle moves to `pending`; on the encrypted path, until
  decryption has consumed the encrypted input. The body is continuously charged
  while a task is parked waiting for a crypto slot — there is no release-then-recharge
  hole.
- `max_processing_bytes` covers the **decrypted/transformed output** (and any
  transmux/repair output). Because the output size is unknown until padding is
  removed, it is reserved at an upper bound (the encrypted input length) before
  decrypt dispatches, reconciled to the actual output size after decrypt, and
  released when the payload is wrapped. At peak, an encrypted segment holds its input
  under download bytes and its (reserved) output under processing bytes
  simultaneously, so both are counted. The processing reservation is held by the
  fetch-and-process future *through the post-decrypt cache write* and dropped
  only after wrap — releasing at decrypt return would leave the decrypted bytes
  resident but uncharged across a potentially slow disk-cache write. On the clear
  path the download reservation plays the same role, held until wrap.
- `max_pending_payload_bytes` covers the wrapped payload from when it enters the
  reactor's `pending` buffer until the permit-send downstream.
- `max_reorder_buffer_bytes` covers a payload from when the assembler buffers it
  until emit or skip.

Crypto admission is gated by **bytes, not just queue depth**. The
`crypto executor queue depth` metric counts operations, but a handful of large
segments can exceed memory while depth stays low. Because decrypt runs inside the
fetch-and-process task, enforce this with a shared byte-counting gate (a
`max_processing_bytes` semaphore). The decrypted output size is not known until
after padding removal, so the task **reserves an upper bound before dispatching
crypto** — the encrypted input length, since AES-CBC output is never larger than its
input — and reconciles the reservation to the actual output size once decrypt
returns, releasing fully on wrap. A task parked on that gate still holds its
encrypted input's download reservation, so that capacity stays unavailable and the
reactor's admission step (`next_ready_job`) cannot reserve for new downloads — a
decrypt backlog throttles the front of the pipeline instead of piling up uncounted
bytes.

Processing has the same oversize-terminal rule as download: an encrypted input
larger than `max_processing_bytes` can never acquire the gate, so it terminalizes
as oversize instead of parking forever. The gate's wait is FIFO (queued acquire),
so one large reservation cannot be starved indefinitely by a stream of smaller
ones.

### Byte budget ownership

The byte budgets are **not** part of `SegmentStateStore`. The store is the reactor's
single-owner control-plane state and is never touched by the spawned tasks (see
fetch-and-process future), but those tasks are exactly who discovers real body sizes
and waits on the crypto byte gate. Putting byte accounting on the store would require
tasks to reach into it, breaking the single-owner property.

Instead, byte accounting lives in a separate `Arc<ByteBudget>` owned by the runtime
and shared by the reactor and every task. Both budgets are **counting reservation
primitives**, not bare counters you read then charge — a reservation is acquired
up front and held by an RAII guard that releases on drop:

```rust
pub struct ByteBudget {
    download: ByteSemaphore,    // max_inflight_download_bytes
    processing: ByteSemaphore,  // max_processing_bytes
}

/// RAII byte reservation. Releases its held bytes on drop. `grow` tries to acquire
/// more capacity so a reservation can track a body whose real size exceeds the
/// initial estimate; it is non-blocking and returns `Err` rather than waiting, so a
/// caller that cannot grow aborts instead of risking a mutual-wait deadlock.
pub struct ByteReservation { /* held: u64, source: ... */ }
impl ByteSemaphore {
    fn try_reserve(&self, bytes: u64) -> Option<ByteReservation>;
}
impl ByteReservation {
    fn grow(&mut self, extra: u64) -> Result<(), BudgetExceeded>;  // non-blocking
    fn reconcile(&mut self, actual: u64);   // shrink to the true size
}
```

- **Download bytes are reserved at admission, not inside the task.** `next_ready_job`
  calls `download.try_reserve(estimate)` (the resolved `ByteRangeKey.length` when
  present, else the running size estimate — see Backpressure and Memory) and
  returns the job owning the `ByteReservation`;
  if the reservation fails, it returns `None` and nothing spawns. Reserving *before*
  the spawn — rather than reading an atomic in the gate and charging later inside the
  task — closes the race where many tasks pass an advisory read before any of them
  has charged. The task receives the reservation, streams the body, and reconciles it
  to the actual size; it releases when decrypt consumes the body (or on wrap, clear
  path).
- **A body larger than its reservation is enforced at chunk granularity.** Chunked or
  under-reported responses can exceed the estimate. Before appending a chunk that
  would push the running size past the current reservation, the task calls
  `reservation.grow(delta)`. If capacity is available it proceeds. If not, it does
  **not** block indefinitely (many tasks all waiting to grow would deadlock): it
  aborts the download and returns a retryable `FailureClass`, so the lifecycle retry
  re-attempts later when the budget is freer — guaranteeing forward progress. A
  segment whose size would exceed a configured per-segment maximum (or
  `max_inflight_download_bytes` entirely, which it can never fit) aborts as an
  oversize terminal. Either way a misbehaving server cannot blow the budget one chunk
  at a time.
- **Oversize-at-admission requires a *known* size.** Only a `ByteRangeKey.length`
  proves a segment can never fit; a fallback estimate is a guess, so an estimate
  above capacity clamps the reservation to capacity instead of terminalizing —
  the Content-Length and chunk-granularity checks then decide from real sizes.
  Admission-time terminalizations are also capped per top-up pass (the rest of
  the ready index waits for later passes), so they cannot flood `pending` past
  its item bound in one call.
- **Processing permits follow the same reserve-then-reconcile shape.** Reserve an
  upper bound (encrypted input length) before decrypt, reconcile to the actual output
  size after, release on wrap.
- `pending` bytes remain reactor-local because only the reactor owns the `pending`
  buffer.

**Bounded overshoot of `pending`.** The top-up gate checks
`pending_payload_bytes < max_pending_payload_bytes` *before spawning*, but tasks
already in flight still push their payloads to `pending` on completion. So `pending`
can exceed its budget by at most the in-flight set's worth of payloads (≤
`max_concurrency` segments). This bounded overshoot is acceptable and documented; if
a hard cap is required, reserve the estimated payload bytes against the pending
budget at spawn time and reconcile to the real size on completion, so the gate
accounts for not-yet-arrived payloads.

Important rule:

The sequence assembler must continue draining completed payloads even when its reorder buffer is near a configured limit. Otherwise, an older segment queued behind newer segments can never arrive at the assembler, which can create a false gap or a deadlock.

When memory is above budget:

- Stop scheduling new downloads.
- Keep receiving completed outcomes.
- Prefer resolving gaps or emitting ordered payloads.
- If policy allows skipping, mark skipped state explicitly and emit a gap event.

## Lifecycle and Shutdown

Bounded memory and production-grade behavior require a defined teardown, not just
a defined steady state. Shutdown is reached on an authoritative end
(`TerminalCause::Endlist`), on cancellation, or on a terminal pipeline error — where
a pipeline error includes a watcher failure (`TerminalCause::Failed` or a watch
sender dropped before any terminal value was seen).

The reactor's `select!` makes this mostly fall out: each terminal cause is the
value the loop `break`s with, and the in-flight `JoinSet` is already in hand.

The three causes have **different** terminal behavior — they are not interchangeable,
and "always emit `StreamEnded`" is wrong for two of them:

| Cause | In-flight tasks | Reorder buffer | Terminal event |
| --- | --- | --- | --- |
| Authoritative end (`TerminalCause::Endlist`) | awaited to completion; payloads still flow | drained in MSN order after `AssemblerInput::End`, then closed | `EndlistEncountered` (forwarded as an `AssemblerInput::Notice` when the ENDLIST snapshot was planned) then `StreamEnded` |
| Cancellation (caller drop / cancel token) | aborted via `JoinSet` drop/`abort_all` | dropped without emission | none — the caller initiated the stop; the stream just ends |
| Pipeline error (downstream closed, task panic, terminal-failure policy, **watcher failure**) | aborted | dropped | the error is propagated to the consumer as the stream's terminal `Err`; no `StreamEnded` |

The assembler distinguishes the authoritative end from the abnormal closes by the
explicit `AssemblerInput::End` item: it arrives only on the authoritative path, so a
channel close *without* a preceding `End` is a cancel/error and the buffer is
dropped, not drained.

Common to all three: the playlist watcher stops publishing snapshots first (it
closes the watch sender), so no new descriptors enter the store. What happens to
already-known work then **differs by cause**:

- **Authoritative end** keeps scheduling. The final ENDLIST/VOD window is already in
  the store as ready work; the reactor continues topping up and draining it (`ending`
  stops new snapshots, not the dispatch loop) until the store and in-flight set are
  empty. It then enqueues `AssemblerInput::End` as the final item, forwards it, and
  only then breaks `Terminal::AuthoritativeEnd`. In-flight tasks are awaited, not
  aborted; payloads keep flowing.
- **Cancellation and pipeline error** stop scheduling immediately and abort. Because
  in-flight work is a `JoinSet`, dropping it (or `abort_all`) aborts the tasks; this
  is why the set is a `JoinSet` and not a `FuturesUnordered` of detached
  `JoinHandle`s, which would leave the spawned tasks running.
- In all cases, `CryptoExecutor` work already dispatched cannot be cancelled:
  `spawn_blocking` tasks run to completion, and rayon/pool jobs finish their current
  operation. Bound the crypto queue (`crypto executor queue depth` and
  `max_processing_bytes`) so teardown cannot block behind an unbounded backlog of
  pending decrypts.

So `OutputSink` emits `StreamEnded` on the authoritative-end path **only**; on
cancellation it emits nothing, and on pipeline error it emits an `Err`. Send errors
during teardown are surfaced, not swallowed.

The ordering requirement is: stop snapshots → for an authoritative end, keep
draining known work then drain the assembler and emit `StreamEnded`; for
cancellation/error, stop scheduling, abort the in-flight set, quiesce the crypto
queue, drop the assembler, and emit nothing / the error. Reversing any step risks
either a lost tail segment or a teardown that hangs on in-flight work.

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
- inflight download bytes (gauge)
- processing bytes — crypto input + output resident (gauge)
- pending payload bytes — reactor buffer awaiting downstream permit (gauge)
- cache hits/misses
- reorder buffer depth/bytes

Recommended spans:

- playlist URL and refresh generation
- segment key, MSN, kind
- retry attempt and reason
- key URI fingerprint, never raw key bytes
- crypto backend and decrypt duration
- output gap from/to sequence

Per-segment fields (MSN, segment key, URI) belong on spans, where each is a
distinct trace, not on metric labels. A live stream produces unbounded MSNs, so
promoting them to counter labels creates unbounded metric cardinality. Counters
above are aggregated and label only on bounded dimensions (`FailureClass`,
`SegmentKind`, `source`, crypto backend).

## Implementation Plan

### Phase 1: Introduce Typed Identity

- Add `SegmentKey`, `ByteRangeKey`, and `SegmentDescriptor`.
- Convert playlist segment discovery to produce descriptors.
- Keep existing scheduler APIs temporarily by adapting descriptors into current jobs.
- Add tests for:
  - same MSN init/media identity separation
  - byte range identity separation
  - query-param inherited identity
  - rotated auth query parameter resolves to the same `SegmentKey`
  - Twitch prefetch carries `SegmentSource::PlaylistPrefetch` but the same
    `SegmentKey` as its later media incarnation (no second key)

### Phase 2: Build the Scheduler Reactor

This phase replaces the current split — `SegmentLifecycleRegistry` on the
playlist-monitor task plus the scheduler's own `pending_identities` and
`active_job_identities` sets, three dedup structures across two tasks — with one
reactor task that owns a single `SegmentStateStore` and drives downloads directly.
Folding the state owner and the download driver together is the point: it removes
the work-handoff channel and the unbounded outcome feedback channel
(`coordinator.rs:101`) in one move.

It is also the riskiest phase, so it lands as three independently shippable
steps. The watcher extraction comes first because everything else is built
against it: `playlist.rs` (~1,500 lines) currently mixes refresh logic, job
production, lifecycle, Twitch preprocessing, and direct client-event emission,
and pulling snapshot production out of it is real work in its own right.

#### Phase 2a: Extract the PlaylistWatcher

- Split snapshot production out of `playlist.rs` into its own task: fetch,
  textual preprocessing (Twitch prefetch rewrite), parse, publish
  `PlaylistSnapshot` over the coalescing watch channel — nothing else.
- Add `PlaylistSnapshot::terminal` and have the watcher set it before dropping
  the sender, on both the `Endlist` and `Failed` paths.
- Remove the watcher's direct client-event sends (`EndlistEncountered`,
  `PlaylistRefreshed`); these become reactor-derived `AssemblerInput::Notice`
  items in 2c, leaving the client channel with a single producer.
- Drive the existing job production from the snapshots temporarily, so 2a ships
  without behavior change while the old scheduler still runs.

#### Phase 2b: SegmentStateStore and ManifestPlanner (no I/O)

- Move all three dedup structures into one `SegmentStateStore` keyed by `SegmentKey`.
- Replace formatted string identities with `SegmentKey`.
- Implement the priority ready queue (`VecDeque` + retry `BinaryHeap` + authoritative
  `HashMap`) with the tombstone invariant; expose `next_ready_job(now, &budget)` /
  `next_ready_jobs(..)` (reserving download bytes at admission) and
  `has_unfinished_work()` (Discovered/Queued/InFlight/RetryAt — the drain predicate).
- Enforce the prune invariant — never evict an entry whose `SegmentKey` can still
  appear in the playlist window — with direct tests proving pruning cannot cause a
  re-download.
- Implement `plan(&snapshot, &store, &mut PlannerContext)`: the cross-snapshot
  BYTERANGE inference chain and last-contiguous-MSN tracking live in the context;
  MSN-gap detection returns explicit `missing` ranges; ad segments are dropped
  here.
- Remove batch sorting as the primary scheduling primitive.
- Add explicit state-transition, priority/tombstone, and planner-context tests
  against the store and planner directly (no I/O).

#### Phase 2c: The Reactor Loop and Byte Budgets

- Build the `select!` loop: ingest snapshots from the coalescing watch channel,
  reading `snapshot.terminal` for the cause (`Endlist` vs `Failed`) and treating a
  bare sender-drop as `WatcherFailed`; plan/ingest the ENDLIST snapshot before setting
  `ending` so its final window is not dropped; guard the discovery arm with
  `if !ending && pending.len() < max_pending_items` so a closed watch cannot hot-loop
  and snapshot intake cannot overflow `pending`. Drive a `JoinSet` of fetch-and-process
  tasks bounded by the `inflight.len() < max_concurrency` dispatch gate, apply
  outcomes, forward `AssemblerInput` via permit-reserve into a bounded `pending`
  buffer (never `send().await` in an arm), wake on the earliest retry deadline, and
  handle the terminal causes plus the drain → enqueue-`End` → finish
  authoritative-end path. Gate the drain on `!store.has_unfinished_work()` (which
  includes future-deadline `RetryAt`), not on "nothing schedulable now".
- Define `AssemblerInput` (with `push_skipped` range coalescing and the `Notice`
  variant carrying playlist-level events) and have `apply_outcome` return the items
  to forward, so terminal/skip state reaches the assembler. Bound `pending` by
  `max_pending_payload_bytes` AND `max_pending_items`, enforced by suspending the
  producers (dispatch gate, snapshot intake) — never by dropping events.
- Introduce the shared `Arc<ByteBudget>` of counting reservation primitives (not
  bare counters). Download bytes are reserved at admission inside `next_ready_job`
  (RAII `ByteReservation` moved into the task), grown at chunk granularity if the body
  exceeds the estimate and aborted if it can't fit; processing bytes are reserved at
  the encrypted-input upper bound before decrypt and reconciled after. The reactor's
  dispatch gate is slots ∧ pending-bytes ∧ pending-items; download bytes gate via the
  admission reservation, not a separate read.
- Keep the old scheduler's active/pending sets only as temporary assertions, then
  delete them once the reactor owns scheduling.
- Keep a small dispatch coalescing window only if profiling shows it helps.

### Phase 3: Zero-Copy Payload Pipeline

Parts of this phase are verify-and-formalize, not build: the fetcher already
returns `Bytes` (`fetcher.rs`, via `response.bytes()` and streamed
`BytesMut::freeze`), and decryption already offloads to `spawn_blocking` with a
TTL key cache (`decryption.rs`). Confirm those, then close the typed gaps.

- Confirm the fetcher returns `Bytes` (already true) and that the streamed path
  freezes without an extra copy.
- Ensure cache stores and returns `Bytes`.
- Make processor return typed `SegmentPayload`.
- Keep no-op transforms as handle moves.
- Convert decrypt/repair paths through `BytesMut` only when required.
- Add `EncryptionDescriptor` and normalize key/IV metadata before processing.
- Wrap the existing `spawn_blocking` decrypt path in `CryptoExecutor` with
  `TokioBlocking` as the default backend (mostly a formalization of current
  behavior).
- Keep a `Rayon` backend optional and disabled by default until benchmarks justify it.
- Add metrics for decryption latency, bytes, queue depth, and key cache behavior.

### Phase 4: Sequence Assembler Integration

`OutputManager` already implements the assembler's core: MSN-keyed reorder
(`BTreeMap`), a separate `pending_init_segments` map with fMP4 init gating,
discontinuity pre-flush, configurable gap-skip strategies, live pruning via
`split_off`, and keep-draining-at-limit. This phase renames it to
`SequenceAssembler` and feeds it explicit segment state — it does not
reimplement reorder/init/gap logic.

- Feed the assembler the single `AssemblerInput` stream (`Payload` / `Skipped` /
  `TerminalFailed` / `End`), not bare payloads.
- Make gap decisions using explicit segment state.
- Keep draining `AssemblerInput` under reorder-buffer pressure.
- Add tests for:
  - out-of-order completion under full buffer
  - retryable missing segment followed by late success
  - terminal segment failure unblocks the assembler instead of stalling it
  - planner-detected window-slide surfaces as `AssemblerInput::Skipped`
  - fMP4 init rotation across discontinuities

### Phase 5: Configuration Cleanup

- Remove any remaining scheduler-side prefetch configuration.
- Expose lifecycle retry and byte-budget settings.
- Expose per-source `IdentityPolicy` wiring: how a source (e.g. Twitch) supplies
  its token-aware policy, and that the default full-URL policy applies everywhere
  else (see Identity Normalization).
- Keep backward-compatible deserialization where persisted config may contain old fields.

### Integration Test Harness

Lands with Phase 2 and grows with each later phase. Most acceptance criteria are
loop-shape properties that store unit tests cannot catch — admission-time
reservation, ENDLIST drain ordering, a 403 retried against a refreshed URL,
coalesced window-slide skips — and are only verifiable against a scripted origin.
The harness is therefore a deliverable, not an afterthought:

- A local mock CDN (axum/hyper) that replays scripted playlist generations and
  serves segment bodies, with per-request fault injection: 404/403/429/5xx,
  timeouts, truncated bodies, missing or lying `Content-Length`, rotating auth
  tokens on segment and key URLs, ENDLIST, and window slides.
- Scenario tests asserting the acceptance criteria end-to-end against the public
  `HlsStreamEvent` stream.
- Replaces the `#[ignore]`d coordinator test that hits a real CDN with a
  hardcoded signed URL (`coordinator.rs`); delete that test when the harness
  lands.

## Migration Notes

- The current playlist-engine task (which owns `SegmentLifecycleRegistry`) and the
  separate `BatchScheduler` task collapse into the single Scheduler Reactor. The
  `PlaylistWatcher` stays its own task and feeds the reactor snapshots. Net task
  count drops from three (playlist+scheduler+output) to two loops plus the crypto
  pool, and the unbounded outcome channel is removed.
- Existing `ScheduledSegmentJob` can be replaced by `SegmentDescriptor` plus the
  fetch-and-process future's local metadata.
- Existing `SegmentLifecycleRegistry` can evolve into `SegmentStateStore` rather than being deleted immediately.
- Existing `OutputManager` can become `SequenceAssembler` with the same external event contract.
- Existing Twitch prefetch handling stays playlist-provided and lowest priority,
  but moves from the dedup identity to `SegmentDescriptor::source`. The current
  identity (URI-only, `playlist.rs`) already collapses a prefetch URL and its
  later media incarnation into one download; preserve that property — do not let
  the typed key reintroduce a second download by keying on prefetch-ness.
- Old prefetch config should remain ignored on deserialization until persisted configs have naturally rolled forward.

## Acceptance Criteria

- A segment cannot be downloaded twice while it is queued or in flight.
- A playlist-prefetch segment that later appears as a normal media segment is downloaded at most once.
- Under a source's configured token-aware identity policy, a segment whose only change across refreshes is a rotated auth query parameter is downloaded at most once. (The default full-URL policy does not make this claim.)
- Retryable transient failures are rescheduled only after their retry deadline.
- Terminal failures are never rescheduled.
- Same-MSN init and media segments cannot collide.
- BYTERANGE segments with the same URI but different ranges cannot collide.
- Output does not stall when a later segment fills the reorder buffer before an earlier segment completes.
- A segment that terminally fails or is skipped never leaves the assembler waiting on it: the assembler is told via `AssemblerInput::TerminalFailed`/`Skipped` and advances.
- A segment that rolls out of the live window before it is planned (window-slide, including across coalesced snapshots) surfaces as an explicit `AssemblerInput::Skipped` range, never a silent stall.
- An `Http(403)` against a signed URL that has since been refreshed is retried against the fresh URL; a `403` against the freshest URL terminalizes within the lifecycle budget.
- `TerminalCause::Endlist` drives a drain → `AssemblerInput::End` → `StreamEnded` finish, with the final window's segments downloaded first.
- `AssemblerInput::End` is not enqueued while any segment is still `Discovered`, `Queued`, `InFlight`, or `RetryAt` (including a not-yet-due retry deadline).
- A watcher failure — `TerminalCause::Failed`, or a watch sender dropped before any terminal value — terminates with an error, never a clean `StreamEnded`.
- A closed watch never hot-loops the reactor after `ending` begins.
- `pending` cannot grow unbounded under a slow downstream: it is bounded by `max_pending_payload_bytes` and `max_pending_items`, and adjacent `Skipped` ranges coalesce.
- Download bytes are reserved at admission (before spawning), so the concurrent in-flight download bytes never exceed `max_inflight_download_bytes` regardless of how many tasks pass the gate together.
- A response body that exceeds its reservation is enforced at chunk granularity: the task acquires more capacity before each over-budget chunk and aborts the segment if the budget cannot be extended; it never overruns silently.
- Store pruning never causes a re-download: an entry whose `SegmentKey` is still inside the current playlist window is never evicted — including init records under the `max_retained_inits` cap.
- A BYTERANGE stream whose explicit-offset anchor stays in-window while a new offset-less tail segment appears resolves the new segment's offset from its true predecessor end, not a stale anchor, on every refresh.
- An fMP4 init that terminally fails (including before any media payload) never causes a permanent gated stall: dependent media is skipped as a visible gap.
- A rotated init segment is emitted before the media that depends on it, even when that media completes before the rotated init arrives.
- A media-sequence reset (window regressing far below the planned watermark) terminates the stream with an error, never a silent download-and-discard loop and never a clean `StreamEnded`. A small backward step (stale CDN edge) plans nothing and waits, without manufacturing a `missing` range over already-decided MSNs.
- The decrypted payload of an encrypted segment stays charged to a byte budget continuously until the payload is wrapped, with no uncounted window across the post-decrypt cache write.
- A reorder buffer at `max_reorder_buffer_bytes` forces the gap-skip decision immediately instead of waiting at the cap; a late completion for a skipped MSN is rejected as stale.
- The client event channel has a single producer: playlist-level events (`PlaylistRefreshed`, `EndlistEncountered`) reach the consumer via `AssemblerInput::Notice` through the assembler, never directly from the watcher.
- Concurrent fetch-and-process futures needing the same decryption key issue one key fetch (single-flight by `key_identity_uri`).
- No media payload copy happens on the no-op path from fetcher to output.
- The ready queue and retry heap never schedule a `SegmentKey` absent from the authoritative state map after pruning.
- Clippy and tests pass with `-D warnings`.
