# Downloader Engine Architecture Plan

This document captures the target Mesio downloader architecture across HLS and
FLV. It intentionally allows breaking API changes: the goal is a performant,
robust, coherent, and maintainable downloader layer with first-class progress
reporting.

The current branch has implemented the right core direction: a per-download
`DownloadSession<T>` with a guaranteed media `items` stream and a best-effort
protocol-neutral `DownloadEvent` stream. I do not see a better fundamental
architecture than this split. The major refactor is now through the public
boundary, orchestrator, progress delivery, and stale-config cleanup; remaining
work is incremental hardening rather than another architecture change.

## Current Assessment

The HLS lifecycle refactor moved the HLS internals in the right direction. The
reactor/store/assembler design is correct and should be preserved as-is:

- `PlaylistWatcher` owns playlist polling (it is the *only* playlist fetcher,
  binds `playlist_url` once at `watcher.rs` construction, and has no rebind
  API).
- The scheduler reactor (`reactor.rs`) owns segment lifecycle state,
  deduplication, the lifecycle retry decision (via `store::apply_outcome`),
  bounded task spawning (a `JoinSet` capped at `download_concurrency`), and
  cancellation (token-cancel aborts the `JoinSet`).
- `fetch`-and-process tasks (`fetch.rs`) own network I/O, byte reservations
  (`budget.rs`), and crypto processing (`crypto.rs`).
- `SequenceAssembler` (`assembler.rs`) owns ordered media emission, fMP4 init
  gating, gap policy, and terminal stream events, delivered downstream through a
  backpressured `event_tx.send().await` (never `try_send`).

This split is good and the plan does not change it. What changes is everything
*around* the engines: the public boundary, progress delivery, source/cache
ownership, config hygiene, and the migration order.

### Current Status

Completed on this branch:

- HLS progress callbacks and `HlsConfig::progress_reporter` are removed. HLS now
  emits protocol-neutral `DownloadEvent` telemetry.
- `mesio-cli` no longer mutates protocol internals or installs HLS progress
  callbacks; it renders progress from `DownloadEvent`.
- `crates/mesio` no longer touches `tracing_indicatif`; CLI rendering owns span
  progress.
- HLS playlist refresh, segment timeout, gap skip, segment/key/playlist resource,
  and retry-scheduled telemetry are surfaced through the event stream.
- FLV emits `Started`, `ResourceStarted`, coalesced `Progress`, and
  `ResourceFinished` events and maps session item errors to `DownloadError`.
- `rust-srec` uses `MesioDownloader::{start_hls,start_flv,detect_protocol}` and
  keeps its writer-driven `SegmentEvent` progress path.
- The signed-URL source failure bug is fixed: HTTP 401/403 startup failures try
  the next source instead of permanently deactivating the source.
- `async-trait`, `indicatif`, and `tracing-indicatif` are removed from
  `crates/mesio`.
- `MesioDownloader` owns source selection for requests with
  `DownloadRequest::sources`, emits `SourceSelected`, and cold-restarts HLS on
  terminal attempt failure with a synthesized item-stream discontinuity.
- `DownloadHandle::join()` exposes HLS lifecycle completion through
  `DownloadTerminal` instead of detaching the watcher/reactor/assembler
  supervisor task.
- Dead HLS config fields were removed from `crates/mesio`, coordinated through
  `rust-srec` model/mapping/test changes, and stripped from persisted Mesio
  engine JSON by a DB migration.
- `output_config.metrics_enabled` is now the single honored HLS metrics gate;
  disabling it disables `PerformanceMetrics` allocation/recording and makes
  `DownloadHandle::metrics()` return `None`.

Residual architecture risks:

- **FLV reconnect policy is explicit but not implemented beyond startup source
  selection.** The default remains terminal failure. Non-default
  `FlvReconnect` modes are rejected with `DownloadError::Configuration` until
  they are implemented, because silent no-op reconnect settings would make the
  public API misleading. Implementing `ReconnectSameSourceWithDiscontinuity` or
  `SwitchSourceWithDiscontinuity` still requires reinjecting a split/header
  boundary.
- **HLS failover is cold restart only.** This is the intended v1 contract, but it
  means cross-source continuity is a visible discontinuity, not a seamless media
  sequence join.

Treated residuals:

- Mid-stream FLV body errors are mapped to `DownloadError::StreamNetwork`, while
  decoder failures still map to `FlvDecode`.
- `MesioDownloader` now routes single-source HLS/FLV startup through the
  `MediaEngine` trait implementations.
- HLS source failover clears the active metrics slot between source attempts.
- FLV CLI processing no longer holds span-enter guards across awaits.

### Findings the original plan missed

- **`rust-srec` is a heavier consumer than the CLI.** It derives
  per-segment/byte telemetry from `hls-fix`/`flv-fix` writer callbacks
  (`SegmentEvent`), not from mesio progress. The event stream must remain
  optional to consume and safe to drop.
- **The "stale" config fields are persisted in `rust-srec`.** Each field slated
  for deletion is a stored override in `rust-srec`'s DB model
  (`database/models/engine.rs`) and is copied into `HlsConfig` by
  `rust-srec/src/downloader/engine/mesio/config.rs`, which has live unit tests
  asserting the mapped values. Deleting them is a cross-crate breaking change.
- **The old retry surface was dead code and has been deleted.** The live
  source-retry classifier is the source-level failure disposition.

## Target Shape

The core public concept is a per-download session, not a protocol object plus
mutable setup.

### Public types

```rust
pub struct DownloadRequest {
    pub url: url::Url,
    pub protocol: ProtocolSelection,
    pub sources: Vec<ContentSource>,
    pub cache: Option<Arc<CacheManager>>,
    /// Parent token; the session mints a child of this. Defaults to the
    /// downloader-level token when None.
    pub cancel: Option<CancellationToken>,
    pub options: DownloadOptions,
}

/// Protocol selection carries protocol-specific per-request options so that
/// per-download choices (e.g. HLS variant selection) stop living in static
/// config. `Auto` defers to `detect_protocol`.
pub enum ProtocolSelection {
    Auto,
    Hls(HlsRequestOptions),
    Flv(FlvRequestOptions),
}

#[derive(Default)]
pub struct DownloadOptions {
    pub hls: HlsRequestOptions,
    pub flv: FlvRequestOptions,
    // future: pub range: Option<(u64, Option<u64>)>,
}

pub struct DownloadSession<T> {
    /// Guaranteed/authoritative: media items AND terminal/boundary markers.
    pub items: BoxMediaStream<T, DownloadError>,
    /// Best-effort telemetry only. Optional to consume; safe to drop.
    pub events: DownloadEventStream,
    pub handle: DownloadHandle,
}

/// Returned only for `ProtocolSelection::Auto`, where the item type cannot be
/// known statically until detection resolves.
pub enum DownloaderSession {
    Flv(DownloadSession<flv::data::FlvData>),
    Hls(DownloadSession<hls::HlsData>),
}

impl DownloaderSession {
    pub fn into_hls(self) -> Result<DownloadSession<hls::HlsData>, DownloadError>;
    pub fn into_flv(self) -> Result<DownloadSession<flv::data::FlvData>, DownloadError>;
}
```

### Engine trait

```rust
/// Static-dispatch trait. Intentionally NOT object-safe: the associated
/// `Item`/`Stream` types and the `async fn` (RPITIT) both prevent
/// `Box<dyn MediaEngine>`. Runtime protocol selection is the `DownloaderSession`
/// enum, exactly as `DownloaderInstance` is today. Do NOT add `#[async_trait]`:
/// it is unnecessary (the enum gives runtime selection), insufficient (the
/// associated type keeps the trait non-dyn), and costs a heap allocation per
/// call. Keep the explicit `+ Send` bound — edition-2024 AFIT does not add it
/// and engine futures are spawned across threads.
pub trait MediaEngine: Send + Sync + 'static {
    type Item: Send + 'static;

    fn start(
        &self,
        request: DownloadRequest,
    ) -> impl Future<Output = Result<DownloadSession<Self::Item>, DownloadError>> + Send;
}
```

> Naming note: the trait must **not** be called `DownloadEngine`. `rust-srec`
> already has an object-safe `#[async_trait] DownloadEngine`
> (`rust-srec/src/downloader/engine/traits.rs`) dispatching across
> `MesioEngine`/`FfmpegEngine`/`StreamlinkEngine`. A different crate, so no
> compiler clash, but reusing the name is a maintenance trap. Use `MediaEngine`
> (or `ProtocolEngine`).

### Entry point

```rust
pub struct MesioDownloader {
    config: MesioConfig,
    default_cache: Option<Arc<CacheManager>>,
    // future: SourceManager, once cold-restart failover moves into the orchestrator
}

impl MesioDownloader {
    /// Only for `ProtocolSelection::Auto`.
    pub async fn start(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloaderSession, DownloadError>;

    /// Dispatch-free paths for callers that know the protocol.
    pub async fn start_hls(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<hls::HlsData>, DownloadError>;

    pub async fn start_flv(
        &self,
        request: DownloadRequest,
    ) -> Result<DownloadSession<flv::data::FlvData>, DownloadError>;
}
```

Keep `DownloadSession<T>` as the single session type; `DownloaderSession` is just
the auto-detect wrapper, and `into_hls()`/`into_flv()` are the only
runtime-fallible accessors. Do **not** collapse into a single
`DownloadSession<MediaItem>`: `FlvData` and `HlsData` have disjoint variants, so a
shared item enum would force every consumer to match per-item on the hot path.

### `DownloadContext` (future crate-internal cleanup)

The branch currently passes `DownloadRequest` directly into concrete protocol
downloaders. A future cleanup can introduce a `pub(crate) DownloadContext`,
constructed inside `start`/`start_hls`/`start_flv`, that only engine impls see.
It would carry shared runtime dependencies and the per-session sink/token. Field
ownership is explicit so the per-download `CacheManager` and `ClientPool` waste
is not reintroduced:

```rust
pub(crate) struct DownloadContext {
    // SHARED (cheap Arc::clone per session):
    clients: Arc<ClientPool>,                 // ClientPool already derives Clone
    cache: Option<Arc<CacheManager>>,         // default None, allocate only on request
    metrics: Option<Arc<PerformanceMetrics>>, // Option's None is a real disable path
    // PER-SESSION (constructed fresh at start):
    events: EventSink,                        // Sender for THIS session's stream
    cancel: CancellationToken,                // child of request.cancel
}
```

Cache precedence is one explicit rule resolved in `MesioDownloader`:
`let cache = request.cache.clone().or_else(|| self.default_cache.clone());`.
Cancellation: `let engine_token = request.cancel.unwrap_or_else(|| self.token.clone()).child_token();`
(reproducing `hls_downloader.rs:115`). Default `default_cache` to `None` — do not
mirror `DownloadManagerConfig::default()`'s `Some(CacheConfig::default())`
(`downloader.rs:351`), which forces a per-download `CacheManager::new` the live
path never consumes.

### `ClientPool` ownership (corrected)

The original plan's single global `Arc<ClientPool>` is **unworkable** for
`rust-srec`'s multi-stream host. Connection-affecting config (proxy, user-agent,
headers, `danger_accept_invalid_certs`, `force_ipv4/6`, timeouts, http version,
system-proxy) is baked into each `reqwest::Client` at build time
(`downloader.rs:255-323`) and `rust-srec` varies it per recording. A single
shared pool cannot honor those differences.

Decision (resolved): build the `Arc<ClientPool>` **per session** from the
request's connection config and place it in `DownloadContext`. This preserves
today's per-instance correctness with zero new key/eviction surface and
eliminates the real waste (the factory rebuilds a whole downloader per
`create_for_url`). `MesioConfig` owns the base connection config and stays
**out** of `DownloadRequest`. Keep `client_for_url`/`client_for_host` per-host
TLS routing and the `RUST_SREC_NATIVE_TLS_HOSTS` override intact
(`downloader.rs:210-238`) — they are load-bearing for the Douyu `edgesrv.com`
native-tls fallback. Add an acceptance criterion that per-recording
proxy/header/TLS isolation is preserved — over-sharing is a silent correctness
regression, not just a perf concern.

A config-keyed pool *cache* is a deferred optimization, added only if profiling
shows the 2-client (rustls + native-tls) build is hot for same-config bursts
(e.g. one streamer reconnecting in a tight loop). When/if added, the key is a
**dedicated `ClientKey` struct over the connection-affecting subset** — **not**
`impl Eq/Hash for DownloaderConfig`. Two reasons: (1) `DownloaderConfig.headers`
is a `HeaderMap`, which is neither `Eq` nor `Hash`, so the derive is impossible
anyway; (2) `DownloaderConfig` mixes connection-affecting fields with
non-connection ones — `cache_config` is irrelevant to the client, and `params`
are query params applied per-request (`client.get(url).query(&base.params)`),
not baked into the client, so including either would fragment the pool wrongly.
`ClientKey` includes only the client-build inputs (timeouts, `user_agent`,
sorted `(HeaderName, HeaderValue)` pairs, `proxy`, `use_system_proxy`,
`danger_accept_invalid_certs`, `force_ipv4/6`, `http_version`,
`http2_keep_alive_interval`, pool sizing), derives `Eq + Hash`, and needs
bounded eviction.

## Unified Event Model

Progress is a protocol-neutral, best-effort stream of telemetry events. It is
**not** a synchronous callback and **not** tied to tracing spans. The single most
important correction to the original plan is the **two-tier delivery split**:

### Tier 1 — Item stream (`items`): guaranteed, authoritative

Terminal and boundary signals stay on the item stream exactly as today, because
the assembler already delivers them via backpressured `send().await`
(`assembler.rs:489/500`) and both consumers' pipe strategies close on them:

- normal end → `HlsData::EndMarker(SplitReason::EndOfStream)`
  (`hls_downloader.rs:167-171`);
- discontinuity boundary → `HlsData::EndMarker(SplitReason::Discontinuity)`
  (`hls_downloader.rs:157-162`);
- fatal → terminal `Err(DownloadError)` (`hls_downloader.rs:175`);
- FLV: clean channel close (EOF), second-`Header`/`EndOfSequence` boundary
  (produced downstream by `FlvPipeline`), and `Err` for fatal.

`PipeHlsStrategy::should_close_pipe` (`pipe_hls_strategy.rs:85-91`) and
`PipeFlvStrategy::should_close_pipe` (`pipe_flv_strategy.rs:94-100`) derive pipe
closure solely from these item-stream values. Moving them to a lossy sink would
silently break pipe-mode framing. The per-segment `MediaSegment.discontinuity`
flag (`HlsData::is_discontinuity`) is intrinsic to `Data` items and stays
regardless.

### Tier 2 — Event stream (`events`): best-effort telemetry

```rust
pub type DownloadEventStream =
    Pin<Box<dyn Stream<Item = DownloadEvent> + Send + 'static>>;

#[derive(Debug)]
pub enum DownloadEvent {
    Started { protocol: ProtocolType, url: Arc<str> },
    SourceSelected { url: Arc<str>, priority: u8, attempt: u32 },
    ResourceStarted { resource: ResourceId, display_url: Arc<str>, content_length: Option<u64> },
    Progress { resource: ResourceId, bytes_delta: u64, bytes_total: u64 },
    ResourceFinished { resource: ResourceId, bytes: u64, from_cache: bool },
    RetryScheduled { resource: Option<ResourceId>, attempt: u32, delay: Duration, reason: Arc<str> },
    GapSkipped { from_sequence: u64, to_sequence: u64, reason: GapSkipReason },
    SegmentTimeout { sequence_number: u64, waited: Duration },
    PlaylistRefreshed { media_sequence_base: u64, target_duration: f64 },
    // An aggregate `StreamProgress { active, completed, bytes_total, bytes_per_sec }`
    // is deferred (see Resolved Decisions); in v1 a renderer aggregates the
    // per-resource events itself.
    /// Best-effort count of events dropped on a full channel.
    Lagged { dropped: u64 },
}
```

`Completed`, `Failed`, and `Discontinuity` are **removed** from the event enum —
they are authoritative terminals and live on the item stream. If a renderer wants
a convenience mirror, it derives "done" from the item stream ending, not from a
droppable event. `GapSkipped`/`SegmentTimeout`/`PlaylistRefreshed` are currently
dropped entirely (`hls_downloader.rs:173`), so routing them to best-effort
telemetry is a strict improvement. `GapSkipReason` (`hls/events.rs:54-70`) is kept
as a typed payload rather than flattened to a `String`, to preserve
machine-readable diagnostics.

### The sink

```rust
#[derive(Clone)]
pub struct EventSink {
    tx: tokio::sync::mpsc::Sender<DownloadEvent>,
    dropped: Arc<AtomicU64>,
}

impl EventSink {
    /// Non-blocking; never `.await`. Drops (and counts) on a full channel so it
    /// can never throttle the fetch/reactor hot paths.
    pub fn emit(&self, event: DownloadEvent) {
        if self.tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn dropped(&self) -> u64 { self.dropped.load(Ordering::Relaxed) }
}
```

Use a **bounded** channel sized to `download_concurrency * k` (small, e.g. 256).
The `dropped` counter makes the "backpressure does not stall downloads" test
meaningful (otherwise dropping everything trivially passes). Forbid
`unbounded()`: an undrained unbounded stream (pipe mode, `rust-srec`) grows
without bound. The `Lagged { dropped }` summary lets a renderer note
"(N progress samples dropped)".

### `ResourceId` (corrected: identity, not just `msn`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResourceId {
    HlsPlaylist { url: Arc<str> },
    /// Carries the engine's own dedup key, NOT msn alone and NOT the fetch url.
    HlsSegment { key: SegmentKey },   // SegmentKey { kind, uri: Arc<str>, byte_range }
    HlsKey { uri: Arc<str> },
    FlvStream { url: Arc<str> },
}
```

Two corrections to the original `ResourceId { url: String }`:

1. **Use `Arc<str>`, never `String`.** Today `Progress` is emitted per network
   chunk with a fresh `url.to_string()` malloc (`fetch.rs:394`), multiplied by
   `download_concurrency`. `SegmentKey.uri` is already `Arc<str>`
   (`identity.rs:37`) and in scope in the fetch task, so the per-chunk emit
   becomes a refcount bump. `Arc<str>` composes with the derived `Hash`/`Eq`.
2. **HLS segment identity must be the canonical `SegmentKey`, not `msn`.** An
   fMP4 init segment shares its `msn` with the first media segment it covers
   (`planner.rs:466`), and BYTERANGE segments share a URI and differ only by
   `byte_range` (`identity.rs:164-184`) — so `msn` alone collides. Use the
   query-stripped canonical `SegmentKey.uri` (the value the store/cache dedup on),
   not the volatile `parsed_url`, so a renderer's per-resource state stays stable
   across token-rotation retries. Carry the human-readable `parsed_url` only as a
   non-identity `display_url` on `ResourceStarted`. This fixes both the allocation
   and a real correlation bug (events today stringify `parsed_url`, which forks on
   signed-URL refresh).

### Progress emission granularity

Coalesce in the fetch body loop: accumulate `bytes_since_last` and emit `Progress`
only when it crosses a threshold (default ~256 KiB) **or** a min interval (~100 ms)
elapses. The post-loop `ResourceFinished` (`fetch.rs:435-443`) carries the exact
final count, so no precision is lost. Make the threshold a const/config knob so a
test can force per-chunk emission with `MIN_DELTA = 0`. Gate event *construction*
on sink presence so pipe mode pays nothing. (Reqwest chunk sizes are not
MTU-bound — expect tens-to-hundreds of `Progress` per segment, not a precise
count.)

### Where events are emitted (a 3-source problem)

"Add `EventSink` to `FetchContext`" does not reach all resource kinds:

- **Segment events:** re-target the four existing `report_progress` sites
  (`fetch.rs:98/326/390/435`) at the `FetchContext`-borne sink.
- **Playlist events:** thread a *separate* `EventSink` clone into
  `PlaylistWatcher` — it is the sole playlist fetcher and never receives
  `FetchContext`.
- **Key events:** net-new. `fetch_key` (`fetch.rs:648`) is `FetchContext`-reachable
  and now emits `ResourceStarted`/`ResourceFinished` for keys.
- **`RetryScheduled`:** the retry decision lives in `store::apply_outcome`
  (`store.rs:544-594`), which has **no** sink and must not gain one (the store is
  the single owner of control-plane state). Today the retry path returns
  `Vec::new()` (`store.rs:594`), so retries are invisible at the reactor boundary.
  Enrich the return to `(Vec<AssemblerInput>, Option<RetryNotice>)` where
  `RetryNotice { msn, key, attempt, delay, reason: Arc<str> }` is filled verbatim
  from existing fields. The reactor (`reactor.rs:184-199`, already holds
  `Arc<FetchContext>` and the sink) maps the notice to `DownloadEvent::RetryScheduled`.
  Keep `reason: Arc<str>` (the store already holds `Arc<str>`; do not re-stringify).
  Attempt-level (in-fetch) retries, if surfaced, get a *distinct* event — they are
  a different retry layer.

### Events vs Metrics (do not merge into one pipeline)

The reviewer's idea to make `DownloadEvent` the single emission primitive and
rebuild `PerformanceMetrics` as an event consumer was **rejected**: today's
counters are lock-free `AtomicU64` `fetch_add` (`metrics.rs:106-111`) that never
drop, whereas `EventSink::emit` is best-effort and drops under backpressure.
Feeding metrics off the lossy progress channel would silently undercount — a
regression. Keep `PerformanceMetrics` as the cumulative source of truth, derive
`DownloadEvent` at the same call sites, and never aggregate bytes from two
independent paths.

Decision (resolved): `PerformanceMetrics` is **surfaced** — expose it on
`DownloadHandle` as an optional accessor (e.g. `handle.metrics() ->
Option<MetricsSnapshot>`) so the lossless throughput/ETA counters are usable
instead of being `..`-destructured and discarded (`hls_downloader.rs:131-136`).
A single honored `metrics_enabled` gate (in `DownloadContext`/`MesioConfig`)
turns the record sites and the accessor on/off together; today the flag is dead
and metrics are unconditionally on.

### Aggregate progress (deferred to a future pass)

A renderer must aggregate across N concurrent segments with no total-byte target
for live streams. In v1 the renderer does this itself from the per-resource
events plus its own timer — per-resource events are the source of truth and the
engine emits no aggregate.

If engine-side aggregation is later wanted, add a `StreamProgress` event emitted
from the reactor on a coarse timer (250–500 ms, `try_send`): `active =
inflight.len()` (`reactor.rs:114`), `completed` from a new counter bumped in
`apply_outcome`'s `Completed` arm (`store.rs:526`), `bytes_total` from
`metrics.download_bytes_total`. Compute `bytes_per_sec` from a **rolling delta**
of `download_bytes_total` over the timer interval — do **not** reuse
`metrics.average_throughput()` (`metrics.rs:214`), which averages over summed
overlapping per-segment latencies and badly understates wall-clock rate.

## Public API Cleanup

The capability traits, free functions, manager methods, and factory have
collapsed into `MesioDownloader` + `DownloadRequest` + `DownloadSession`.
Deleted on this branch:

- Capability traits `Resumable`, `MultiSource`, `Cacheable`, `RawDownload`,
  `RawResumable` and their FLV/HLS impls.
- Free helper fns `download_with_resume`, `download_with_sources`,
  `download_with_sources_and_cache`, `download_raw_with_resume`.
- `DownloadManager`, `DownloadManagerConfig`, `protocol_mut`,
  `MesioDownloaderFactory`, `DownloaderInstance`, `DownloadStream`,
  `process_stream!`, the vestigial `Protocol` enum, and the old
  `media_protocol.rs` module.
- `DownloadError::is_retryable` and `hls/retry.rs`.

Still live by design: `ProtocolType`, protocol builders, `create_client`, and the
shared `BoxMediaStream` alias.

**Coordinate (NOT dead):**

- `FlvDownloadError::AllSourcesFailed` is matched by `rust-srec`'s production
  `classify_flv_error` (`rust-srec/src/downloader/engine/mesio/mod.rs:82-88`) and a
  test. Keep it, or remove it together with a coordinated edit to
  `classify_flv_error` (and the test) in the same change.

Re-add byte-range/resume later as a typed `DownloadOptions.range` field if a
consumer appears — not as a trait, and do not ship a raw-HTTP `ResourceId` or the
word "resume" until a raw engine actually exists.

### Variant selection moves to the request

`variant_selection_policy` moves **out** of static `HlsPlaylistConfig`
(`config.rs:199`) into per-request `HlsRequestOptions`, as a per-call override that
falls back to a config default when `None` (do not delete the config default —
`rust-srec` maps it per-target). This is low-risk: selection is consumed once
before any task spawns (`engine/mod.rs:114-123`) and is never re-evaluated by the
watcher, so it is a localized change to `engine::start`'s prelude. Leave
`live_refresh_interval`/`adaptive_refresh_*` in static config — those are
connection-scoped, not per-download.

## Source, Cache, and Failover Boundaries

`SourceManager` lives in the orchestrator (`MesioDownloader`), not inside the
engines. Engines receive a single selected source URL. Preserve the existing
`SourceManager` circuit-breaker/EMA machinery (`source.rs:344-422`) as the
failover intelligence — do not reimplement it.

### Failover is explicit, per protocol

**HLS v1 — cold restart, orchestrated above the engine.** There is no in-engine
source swap: the watcher's `playlist_url` is immutable and the `SourceManager`
never reaches the live reactor. On any live-engine terminal failure (watcher
retry exhaustion → `TerminalCause::Failed`, a media-sequence reset surfaced as
`PipelineError` at `planner.rs:134-150` → `reactor.rs:266-273`, or a stream-level
`Err`), the orchestrator calls `source_manager.record_failure(url, err)`, tears
down the session (drop cancels watcher/reactor/assembler), and starts a **new**
`DownloadSession` against the next selected source. The orchestrator must
**synthesize** a discontinuity marker between sessions (the existing
`HlsData::EndMarker(SplitReason::Discontinuity)` is the natural vehicle). This is
net-new orchestrator wiring; `record_failure`-on-terminal does not exist today.

> Correction to the original plan: mid-stream HLS failover is **not** "realistic
> because the lifecycle can continue from the latest source." A second source has
> an unrelated `media_sequence` range; a forward MSN jump is treated as a window
> slide and gap-skipped (`planner.rs:125-133`), and a backward jump trips the
> deliberate reset-to-fatal path (`planner.rs:134-150`). True hot rebind (v2,
> out of scope) would require a watcher rebind API, a discontinuity barrier, and
> anchoring the new source's MSNs *above* the assembler emit cursor — net-new
> machinery, not a free consequence.

**FLV reconnect — typed enum, default conservative.**

```rust
pub enum FlvReconnect {
    FailTerminal,                    // default
    ReconnectSameSourceWithDiscontinuity,
    SwitchSourceWithDiscontinuity,
}
```

Any reconnect emits `Discontinuity` on the **item** stream and re-injects a
synthetic `Split`/`Header`, never a transparent stitch — the decoder
(`parser_async.rs`) cannot resume cleanly mid-stream, so every reconnect is
resync-with-loss. This makes the original plan's conservative intent the typed
contract.

### Error taxonomy (scoped down)

Do **not** add a four-variant `ErrorKind` table — the failover loops switch source
on *any* `Err`, so `SwitchSource`/`RetrySameSource` is meaningless to them, and
The dead `is_retryable` + `retry.rs` path has been deleted. Keep only the
defensible core:

1. **Fix the confirmed bug:** a signed-URL 401/403 at startup permanently
   deactivates the source (`source.rs:339-352`). Introduce a small source-level
   `source_disposition() -> { DeactivateSource, TryNextSource }` that treats
   401/403 as transient (a playlist refresh re-signs the URL), and have
   `is_non_recoverable_source_error` defer to it.
2. **Done:** delete the dead `is_retryable` and `retry.rs` rather than folding
   them in.
3. **Keep** the store's generation-aware `FailureClass` (`store.rs:544-560`)
   internal and untouched — its signed-URL/auth and `OverBudget` rules are not
   expressible at the `DownloadError` level and must not be reconciled away.

The FLV network-vs-decoder seam is now typed enough for the public error
boundary: mid-stream body failures are marked by `BytesStreamReader` as
connection-aborted I/O and converted to `DownloadError::StreamNetwork`, while
real decoder failures continue to surface as `FlvDecode`. A future
protocol-neutral `Failed { kind }` event can map from those public categories
without adding a third parallel enum.

### Unified item error

The public `items` stream errors as `DownloadError` (HLS already does via the
`HlsDownloaderError = DownloadError` alias). FLV adds a thin terminal-only
`.map_err(DownloadError::from)` at the session boundary (a stream errors at most
once, so it is cheap). Do not rely on the error *type* to carry network-vs-decoder
— carry that as the structured `Failed { kind }` event above. Keep
`FlvDownloadError` engine-internal.

### Cache

Cache is a `DownloadContext` dependency, not a capability trait. FLV live caching
is already effectively disabled (the write path is commented out with a TODO at
`flv_downloader.rs:517-524`); the real correctness risk is HLS playlist/segment
caching with rotating signed URLs — the canonical `SegmentKey.uri` (query-stripped
per `IdentityPolicy`) is the right cache key and is already used (`fetch.rs:190-196`).

## Cancellation and Shutdown

One child token per session (child of `request.cancel`), fired by **both**
`DownloadHandle::cancel()` **and** drop of the `items` stream. They are
complementary; neither replaces the other (`CancellationToken::cancel()` is
idempotent, so multiple triggers coexist safely).

- The drop guard **must** live on the `items` stream type (keep the
  `CancelOnDropStream` wrapper, `hls_downloader.rs:24-47`), **not** on the
  `DownloadSession` struct: the CLI destructures the session by move, which Rust
  forbids for a type with a `Drop` impl, and a struct-level guard would fire while
  `items`/`handle` are still in use. `tokio_util` tokens do not cancel on drop, so
  this stays an explicit `Drop`.
- Apply the same pattern to FLV — FLV has no `CancelOnDropStream` today; its
  cancel comes from acquisition-time token races plus `tx.send` failure when the
  reader drops (`flv_downloader.rs:306-332`). Give FLV's `items` a genuine drop
  guard so both protocols share one contract.
- The session owns the three engine `JoinHandle`s (`EngineHandles`,
  `engine/mod.rs:58-63`) instead of the current detached `tokio::spawn`
  (`hls_downloader.rs:130-150`), which orphans them and swallows the reactor's
  `Terminal` outcome into a `debug!` log. Add an optional
  `DownloadHandle::join() -> Terminal` that surfaces the terminal reason
  (`AuthoritativeEnd`/`Cancelled`/`DownstreamClosed`/`PipelineError`) and lets a
  long-running recorder await full teardown. Keep `cancel()`/`is_cancelled()` as
  the primary contract; `join()` is additive, for observability and clean
  shutdown — **not** for drain ordering (drain is a property of reading `items`
  to its `EndMarker`, enforced by the channel protocol, not the supervisor).
- Preserve the invariant that `JoinSet`-drop aborts in-flight fetches
  (`reactor.rs:225-229`); if spawning moves off the `JoinSet`, replicate
  abort-on-cancel or dropped sessions leak running downloads.
- Define the join contract per-engine: FLV's single forwarder has no multi-task
  flush.

## CLI Integration

The CLI becomes an event consumer and pipeline driver:

```rust
let session = downloader.start_hls(request).await?;
let DownloadSession { items, events, handle } = session;

let progress_task = tokio::spawn(render_download_events(events, download_span));
let stats = run_pipeline(items, writer_span, token).await; // terminals ride here
handle.cancel();
let _ = progress_task.await;
```

The CLI keeps using `tracing_indicatif` span-attached progress bars
(`mesio-cli/src/utils/spans.rs`) — that is fine; the rule is that **`crates/mesio`
never touches tracing spans**. One shared renderer translates `DownloadEvent` into
bar updates for both protocols. Pipe mode ignores `events` entirely and closes on
the item-stream `EndMarker`s — which is exactly why terminals must stay on `items`.

The CLI has stopped mutating protocol internals, installing HLS progress
callbacks, and depending on engine-side span mutation. FLV content length now
reaches the progress bar through the CLI's `ResourceStarted` handler.

## Config Cleanup

Breaking changes were coordinated across `crates/mesio` and `rust-srec`:
runtime fields, persisted override structs, JSON mapping tests, and DB migration
landed together. Old persisted Mesio engine configs are cleaned by
`rust-srec/migrations/20260613000000_remove_dead_mesio_hls_config_keys.sql`.

Removed dead/duplicated fields:

- `HlsConfig::progress_reporter` (`config.rs:181`) — replaced by `EventSink`.
- `HlsPerformanceConfig::batch_scheduler` (`config.rs:31`) — no longer consulted
  by the reactor scheduler.
- `HlsPerformanceConfig::zero_copy_enabled` (`config.rs:33`) — defined, never read.
- `HlsFetcherConfig::streaming_threshold_bytes` (`config.rs:273`) — defined,
  never read.
- `HlsFetcherConfig::segment_raw_cache_ttl` (`config.rs:270`) — only a builder
  setter writes it; no engine reader.
- The duplicate `HlsPerformanceConfig::metrics_enabled`; the remaining
  `HlsOutputConfig::metrics_enabled` is the single honored gate.
- `DownloadManagerConfig::{max_retry_count, enforce_certificate_validation}` and
  the `#[allow(dead_code)]` `DownloadManager.config` field — deleted with the
  manager surface; TLS is canonically `DownloaderConfig::danger_accept_invalid_certs`.

`metrics_enabled` now controls `PerformanceMetrics` allocation/recording and the
`DownloadHandle::metrics()` accessor. It does not gate `DownloadEvent`
telemetry; progress events remain best-effort renderer input.

Verified **alive** (keep): `download_concurrency` (`engine/mod.rs:178`),
`processed_segment_buffer_multiplier` (`engine/mod.rs:180`), `max_segment_retries`
(`fetch.rs:230`), `offload_decryption_to_cpu_pool` (`engine/mod.rs:146`), and the
whole `HlsEngineConfig`.

Target hierarchy, cleanly separated: HTTP/client config (`MesioConfig` → pool);
protocol config (static, e.g. `HlsEngineConfig`, refresh intervals); session
policy (`DownloadRequest`/`DownloadOptions`, incl. variant selection); and
observability/event delivery (`EventSink`, `PerformanceMetrics`).

## Downstream Consumers and Migration Order

Two in-tree consumers; both must compile after every phase.

- **`mesio-cli`** — now uses `MesioDownloader` and consumes the event stream for
  rendering. It no longer installs HLS callbacks or mutates protocol internals.
- **`rust-srec` (production multi-stream recorder)** — the heavier consumer.
  It now starts typed `MesioDownloader` sessions, keeps using builders/config
  enums, and gets progress from `hls-fix`/`flv-fix` writer callbacks (not mesio
  progress). It ignores the `events` stream, so events must be droppable without
  deadlock (bounded `try_send`), exactly as for pipe mode.

Gate every phase on `cargo build --workspace` + `cargo nextest run --locked
--workspace` (including `crates/mesio/tests/hls_engine.rs`). Concentrate all
symbol/field deletions into the final phase, after both consumers compile against
the new API, so the breaking change lands atomically.

## Implementation Phases (re-sequenced)

### Phase 1 — Add the neutral session types beside the current API

Status: complete.

- Add `DownloadRequest`, `DownloadOptions`, `ProtocolSelection`,
  `DownloadSession<T>`, `DownloaderSession`, `DownloadEvent`, `ResourceId`,
  `EventSink` (with `dropped` counter), `DownloadHandle`, and the `MediaEngine`
  trait, in a protocol-neutral module independent of the `hls`/`flv` engine
  modules and feature gates.
- Do not remove anything yet.
- Unit tests: `EventSink` drops-and-counts on a full channel; child-token cancel
  fires through `handle.cancel()`.
- Added HLS drop-cancel coverage; FLV shares the same drop guard contract.

### Phase 2 — Port HLS onto the session contract

Status: complete.

- Thread `EventSink` through the **three** sources: re-target the four
  `report_progress` sites at the `FetchContext` sink; thread a sink clone into
  `PlaylistWatcher`; add key `ResourceStarted`/`Finished`; surface `RetryNotice`
  from `apply_outcome` and emit `RetryScheduled` from the reactor.
- Coalesce `Progress`; use `Arc<str>`/`SegmentKey` identity.
- Move `variant_selection_policy` into `HlsRequestOptions`.
- Return `DownloadSession<HlsData>`; keep terminals on `items`.
- `HlsConfig::progress_reporter`, `HlsProgressReporter`, and
  `set_progress_reporter` are removed.
- `DownloadHandle::join()` exposes `DownloadTerminal` for lifecycle observation.

### Phase 3 — Port FLV onto the session contract

Status: complete for session/progress; structured reconnect/failure telemetry is
deferred.

- Return `DownloadSession<FlvData>`; emit coalesced `Progress` from the body
  forwarder; map the FLV error to `DownloadError` only at the session boundary.
- Remove `tracing_indicatif`/span mutation from the engine
  (`flv_downloader.rs:179-184`); the CLI renders `content_length` from
  `ResourceStarted`.
- Mid-stream network failures map to `DownloadError::StreamNetwork`; decoder
  failures still map to `DownloadError::FlvDecode`.
- Added the `FlvReconnect` enum (default `FailTerminal`); reconnect behaviors
  beyond the default remain future work.

### Phase 4 — Add the `MesioDownloader` surface + orchestrator

Status: complete for HLS cold-restart failover and FLV startup source selection.

- `MesioDownloader::{start,start_hls,start_flv}`, `MesioConfig`, and
  protocol-neutral session types exist.
- Protocol detection is owned by `MesioDownloader`.
- `MesioDownloader` uses `SourceManager`, emits `SourceSelected`, and implements
  HLS cold-restart failover with synthesized cross-session discontinuity.

### Phase 5 — Port `mesio-cli`

Status: complete.

- Consume `items` + `events`; one shared `DownloadEvent` renderer for FLV and HLS.
- Remove `protocol_mut` and `HlsProgressReporter` usage. Pipe mode drops `events`.

### Phase 6 — Port `rust-srec`

Status: complete.

- Migrate `rust-srec/src/downloader/engine/mesio/{engine,hls_downloader,flv_downloader,config}.rs`
  to `MesioDownloader::{start_hls,start_flv}`. `events` may be ignored.
- Stale HLS config mapping/model fields were removed with a DB migration.

### Phase 7 — Final cleanup and verify

Status: complete for the refactor scope in this document.

- Done: deleted the compat shim (factory/`DownloaderInstance`/`DownloadStream`),
  the dead capability traits/free fns/manager methods, `Protocol` enum,
  `process_stream!`, the dead `is_retryable`/`retry.rs`, and the unused
  `async-trait` dep.
- Run:
  - `cargo fmt --all`
  - `cargo clippy --locked --all-targets --all-features -- -D warnings`
  - `cargo check --locked --workspace`
  - `cargo test --locked -p mesio-engine`
  - targeted `rust-srec` config/model/migration tests

## Testing Plan

Contract tests, added as the relevant phase lands:

- HLS progress events are emitted for network segment downloads and cache hits.
- Playlist and key `ResourceStarted`/`Finished` events are emitted.
- `RetryScheduled` is emitted from the reactor when `apply_outcome` reschedules.
- FLV `Progress` is emitted for streamed chunks; `MIN_DELTA=0` forces per-chunk.
- Event backpressure does not stall downloads, and the `EventSink` drop counter
  increments under a full channel.
- Pipe-mode close boundaries (`EndMarker(EndOfStream)`, `EndMarker(Discontinuity)`,
  per-segment discontinuity flag) survive event-stream backpressure because they
  ride the item stream.
- Dropping the `items` stream and calling `handle.cancel()` fire the **same**
  child token; assert prompt teardown (in-flight segment fetches aborted within a
  tight bound, not after the next watcher refresh interval).
- `ResourceId` equality is stable across a token-rotation retry (canonical
  `SegmentKey` identity), so a renderer does not double-count.
- A signed-URL 401/403 at startup is treated as try-next/transient, not permanent
  source deactivation.
- If a structured failure event is later added, a mid-stream FLV network drop
  surfaces as network failure, not a decoder error.
- A terminal HLS engine failure triggers `record_failure` + cold-restart against
  the next source with a synthesized discontinuity, or exhausts sources with a
  typed error — never a silent stall.
- CLI file mode renders progress from events; pipe mode emits no progress to
  stdout and still closes correctly.

## Resolved Decisions

All six prior open decisions are resolved:

1. **`PerformanceMetrics` fate → surface it + single gate.** Expose it on
   `DownloadHandle` as an optional accessor (the lossless throughput/ETA source),
   and wire one honored `metrics_enabled` gate to it. See *Events vs Metrics* and
   *Config Cleanup*.
2. **`StreamProgress` aggregate event → deferred.** Ship per-resource events in
   v1; the renderer aggregates across concurrent segments itself. The aggregate
   event is sketched as a future addition in *Aggregate progress*.
3. **FLV reconnect default → `FailTerminal`.** Conservative; never silently stitch
   unrelated streams. `rust-srec` already retries/circuit-breaks above mesio, so
   the engine stays dumb and predictable.
4. **Persisted-config removal → DB migration.** Ship a migration that strips the
   removed keys from persisted engine-config rows, sequenced with the field
   removal in the final cleanup phase, plus a release note. See *Config Cleanup*.
5. **`ClientPool` sharing → per-session pool.** Build the pool per session from
   the request's connection config; no `Eq`/`Hash` on `DownloaderConfig`. A
   config-keyed `ClientKey` cache is a deferred, profile-gated optimization. See
   *ClientPool ownership*.
6. **Hot mid-stream HLS rebind → out of scope (v2).** Cold restart is the correct
   v1; a seamless join across independent CDNs is not achievable. See *Failover*.

## Acceptance Criteria

- Engines expose one coherent session API
  (`MesioDownloader::{start,start_hls,start_flv}`);
  no `protocol_mut`, no factory/`DownloaderInstance` in the final surface.
- Progress is reported for HLS and FLV via one protocol-neutral `DownloadEvent`
  stream, with no protocol-specific CLI callbacks and no engine access to tracing
  spans.
- Event delivery is non-blocking (`try_send`), cannot throttle media reads, and
  drops are counted; the `events` stream is safe to ignore (pipe mode, `rust-srec`).
- Terminal/boundary signals stay on the guaranteed item stream; pipe-mode framing
  is preserved.
- HLS keeps its reactor/store/assembler correctness properties; `hls_engine.rs`
  passes throughout.
- FLV and HLS share one source/cache/cancel/progress contract; failover policy is
  explicit per protocol.
- Per-recording proxy/header/TLS isolation is preserved across concurrent sessions.
- Public config contains only fields that affect behavior, and both in-tree
  consumers compile after every phase.
- CLI progress rendering lives entirely outside `crates/mesio`.
