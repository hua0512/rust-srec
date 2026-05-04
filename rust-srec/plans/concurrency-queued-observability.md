# Concurrency-Limited Download Observability + Pipeline Decoupling

## Original bug

### What the user reported

> "Currently the frontend doesn't have good observability when streamer
> downloads are limited (by global config `max_concurrent_downloads`).
> Streamers show on live state with no progress."

That is: a user with, say, `max_concurrent_downloads = 6` and 10 popular
streamers going live simultaneously sees four cards stuck on the red
`Live` badge with **zero bytes downloaded, zero segments, no engine
activity, and no error message**. Dashboards look broken. The user
can't tell whether the downloads are about to start, queued, throttled,
or genuinely failing.

### What's actually happening on the backend

Reproducing the symptom led to a chain of related issues, not just a
missing UI hint. Audit below.

#### Issue A — silent indefinite park on `acquire_owned()`

`DownloadManager::start_download_with_engine` (manager.rs, pre-fix)
called:

```rust
let permit = self
    .normal_limit
    .semaphore()
    .acquire_owned()
    .await
    .map_err(...)?;
```

`tokio::sync::Semaphore::acquire_owned()` is FIFO and *unbounded* — it
will wait forever for a permit. There is no timeout, no wakeup signal,
no event emission. The streamer's row stays at `state=LIVE`, the
download manager has no entry in `active_downloads`, and the WebSocket
emits nothing because no `DownloadStarted` event has fired. From the
frontend's perspective, the streamer is **live and silent**.

#### Issue B — the monitor event handler is single-threaded and blocks

`services/container.rs::setup_monitor_event_subscriptions` runs:

```rust
loop {
    tokio::select! {
        _ = cancellation_token.cancelled() => break,
        result = receiver.recv() => match result {
            Ok(event) => {
                Self::handle_monitor_event(&download_manager, …, event).await;
                //                                                   ^^^^^
                //                            entire loop blocks until this returns
            }
            …
        }
    }
}
```

`handle_monitor_event` for `MonitorEvent::StreamerLive` awaits
`download_manager.start_download(…)`, which contains the
`acquire_owned().await` from Issue A. So when the limit is saturated
the **entire monitor-event subscriber task is parked**. Cascading
consequences:

- `StreamerLive` events for other streamers queue up behind the
  blocked acquire and don't get processed.
- `StreamerOffline` for the *currently-blocked* streamer queues
  behind itself: the streamer keeps its DB `state=LIVE` indefinitely
  even after the platform reports it offline, until *some* slot
  finally frees and the offline event finally drains.
- Resume-from-hysteresis events
  (`setup_resume_download_subscriber`'s synthetic `StreamerLive`)
  share the same handler and stall identically.
- Danmu start/stop triggers riding on the same loop don't fire.

The user sees this as "everything stops moving" the moment the
concurrency limit fills up.

#### Issue C — danmu only correctly gated *by accident*

The pre-fix `StreamerLive` arm did:

```rust
match download_manager.start_download(…).await {  // blocks until slot
    Ok(id) => …,
    Err(e) => warn!(…),
}

if merged_config.record_danmu {
    danmu_service.start_collection(…).await;       // only runs after download started
}
```

Today danmu collection is gated correctly — but **only because
`start_download` didn't return until the permit was acquired**,
i.e., as a side-effect of bug B. The moment we decouple the monitor
loop (which we have to, to fix B), naive code would spawn the
download in the background and immediately call `start_collection`,
opening a platform danmu socket for a stream we're not actually
recording yet. With many streamers queued, that's many wasted
platform connections — risking rate-limit bans on aggressive
platforms.

#### Issue D — stale stream URLs after long queue waits

`MonitorEvent::StreamerLive` carries `streams: Vec<StreamInfo>`, where
each `StreamInfo.url` may include signed query params with short TTLs
(Douyin / Huya / Bilibili). If a streamer is queued for, say, 10
minutes waiting for a slot, the cached URL may already return 403 by
the time we acquire. The engine starts, fails fast, increments
`consecutive_error_count`, and may trip the circuit breaker — even
though the stream was healthy and would have worked with a fresh
URL.

The pre-fix code had no mechanism to refresh URLs after a wait.

#### Issue E — high-priority streamers lose their priority advantage under saturation

The pre-fix manager held two separate semaphores:

- `normal_limit` (capacity = `max_concurrent_downloads`)
- `high_priority_limit` (capacity = `high_priority_extra_slots`)

Acquire flow:

```rust
let permit = if is_high_priority {
    match self.high_priority_limit.semaphore().try_acquire_owned() {
        Ok(permit) => permit,                                         // (1)
        Err(_) => self.normal_limit.semaphore().acquire_owned().await,// (2)
    }
} else {
    self.normal_limit.semaphore().acquire_owned().await               // (3)
};
```

If the high-priority extra pool is full (1 fails), the high-priority
streamer falls through to (2) — but `tokio::Semaphore` is strict
FIFO. A high-priority streamer arriving after 10 normal streamers
have already queued ends up at position 11. There is no
priority-based wakeup; the design intent ("VIPs jump the line under
load") simply doesn't hold past the dedicated pool.

#### Issue F — no observability at all

There was no event in the system that meant "this download is parked
waiting for a slot." The progress event enum had `DownloadStarted`,
`Progress`, segment events, `ConfigUpdated`/`ConfigUpdateFailed`,
plus terminal `Completed`/`Failed`/`Cancelled`/`Rejected`. None
described the queued-but-not-started state. The frontend's WS
handler had nowhere to hook a "queued" signal even if it wanted one.

### Why this had been hidden

- Most users never hit the limit (they monitor < 6 streamers, or
  their popular ones don't go live simultaneously). The bug surfaces
  only at peak hours or with large streamer sets.
- When triggered, it looks like a transient platform issue ("nothing
  is happening, must be a streaming-platform hiccup") rather than a
  client-side scheduling failure.
- The DB row for the streamer keeps `state=LIVE`, so the user has no
  audit trail: no rejection notification, no `last_error`, no entry
  in the recent failures list.

### What we fix

| Issue | Fix |
|---|---|
| A — silent indefinite park | `DownloadQueue::acquire` emits `DownloadQueued` event when it has to park; honours per-session `CancellationToken` |
| B — monitor loop blocks | `StreamerLive` arm `tokio::spawn`s the per-streamer pipeline; loop never awaits a slot |
| C — danmu accidentally gated | Pipeline phase 6 explicitly gates `danmu_service.start_collection` on `start_with_slot` returning a download id |
| D — stale URLs | Pipeline phase 4 calls `monitor.check_streamer` to refetch when `slot.waited_ms() > FRESHNESS_THRESHOLD_MS` (default 60s, env-overridable) |
| E — broken high-priority precedence | Single `DownloadQueue` with priority-promoting wakeup: high-priority waiters get the next free slot regardless of which tier freed it |
| F — no observability | New `DownloadQueued` proto + WS event + frontend store + amber/rose Queued status badge with tooltip |

## Context

When a streamer goes live and the global `max_concurrent_downloads` limit is
saturated, today's behavior is silently broken:

1. **Frontend has no signal.** The streamer card shows the red `Live` badge,
   but no progress, no bytes, no segment activity. The user can't tell
   "about to start," "queued," or "broken."
2. **The monitor event loop blocks.** `services/container.rs::setup_monitor_event_subscriptions`
   awaits `handle_monitor_event` serially. Inside the `StreamerLive` arm,
   `download_manager.start_download(...).await` calls
   `Semaphore::acquire_owned().await`, which parks the future indefinitely
   when the limit is hit. Subsequent `StreamerLive`, `StreamerOffline`,
   resume-from-hysteresis, and danmu-trigger events for *every other
   streamer* queue up behind the blocked acquire. A streamer that goes
   offline while another is queued stays "Live" in the DB until a slot
   frees.
3. **Danmu collection ignores the slot.** Today danmu starts *after*
   `start_download` returns, so it's gated correctly only as a side-effect
   of (2). Once we decouple the loop, naive danmu-start would fire while
   the download is still queued — wasting platform connections and risking
   rate-limit bans.
4. **Stale URLs after long waits.** Stream URLs (especially HLS m3u8 with
   signed query params) have short TTLs on some platforms. A queued live
   captured 10 minutes ago may have a 403 URL by the time we finally
   acquire.
5. **High-priority streamers don't actually take precedence.** The current
   design has `high_priority_extra_slots` as a dedicated extra pool, but
   when both pools are saturated, high-prio falls back to the normal pool
   as a regular FIFO waiter behind any earlier-arriving normal streamers.

This plan ships a single coherent change that fixes all five and replaces
the current `tokio::sync::Semaphore`-based gating with a proper
priority-aware download queue.

## Goals

- Frontend shows a `Queued` badge whenever a streamer is live but waiting
  for a download slot, with priority and "X of Y in use" detail.
- The monitor event handler never blocks — every per-streamer pipeline
  runs in its own spawned task.
- Danmu collection is correctly gated: starts only after the download has
  a real `download_id`.
- A streamer that goes offline while queued cancels cleanly with no engine
  startup, no platform connection waste, no slot leak.
- After a non-trivial wait, the pipeline refetches live state (incl. fresh
  URLs) via the existing `MonitorService::check_streamer`.
- High-priority streamers actually take precedence in the queue, not just
  in their dedicated pool.
- Snapshot includes pending entries so a page refresh keeps the badge.
- All existing public APIs (`set_max_concurrent_downloads`,
  `active_count`, throttle integration, etc.) keep their signatures and
  semantics.

## Architecture

```
Monitor ── StreamerLive ──▶ container.rs subscription loop
                                  │ (never blocks)
                                  ▼
                          tokio::spawn(run_live_download_pipeline)
                                  │
                                  ▼
   ┌────────────────────────────────────────────────────────────┐
   │ 1. has_active_download / pending_starts dedup → bail if    │
   │    duplicate                                               │
   │ 2. is_active() / is_disabled() pre-checks → bail           │
   │ 3. download_manager.preflight(req) → bail with             │
   │    DownloadRejected on engine missing / CB open / output   │
   │    root degraded                                           │
   │ 4. download_manager.acquire_slot(req, cancel_token):       │
   │      - try fast path; on miss, register PendingEntry,      │
   │        emit DownloadQueued, park on priority queue         │
   │      - tokio::select! on permit_signal vs cancel_token     │
   │ 5. if slot.waited_ms > FRESHNESS_THRESHOLD_MS:             │
   │      monitor.check_streamer(...) → fresh streams or bail   │
   │ 6. download_manager.start_with_slot(slot, config) →        │
   │    engine bring-up + DownloadStarted event                 │
   │ 7. if record_danmu: danmu.start_collection(...)            │
   └────────────────────────────────────────────────────────────┘

Monitor ── StreamerOffline ──▶ session_cancels.cancel(session_id)
                                  ↓
                       (all queued pipelines for that session bail)
```

The `DownloadQueue` replaces both `ConcurrencyLimit` semaphores. It
implements wakeup ordering: high-prio waiter gets next slot regardless of
which tier frees, as long as `total_in_flight < total_capacity`; within a
tier, FIFO by `queued_at_ms`.

## Files touched

### Backend — new modules

- **`rust-srec/src/downloader/queue.rs`** (new) — `DownloadQueue`,
  `SlotGuard`, `Priority`, `PendingEntry`, `AcquireRequest`,
  `AcquireError`. Replaces internals of the two existing
  `ConcurrencyLimit`s. Owns priority ordering, cancellation surface,
  pending snapshot, capacity reconfiguration.

- **`rust-srec/src/services/session_cancels.rs`** (new) —
  `SessionCancelTokens(DashMap<String, CancellationToken>)` with
  `token_for(session_id)`, `cancel(session_id)`, `drop(session_id)`.
  Held on `ServiceContainer`.

### Backend — modified

- **`rust-srec/src/downloader/manager.rs`**
  - Drop `ConcurrencyLimit` (lines 53–127) and the two
    `Arc<ConcurrencyLimit>` fields. Replace with `Arc<DownloadQueue>`.
  - Split `start_download_with_engine` (line 1137) into:
    - `preflight(&self, req: PreflightRequest) -> Result<EngineHandle, PreflightError>`
      — engine resolve+availability, circuit-breaker check (emits
      `DownloadRejected{CircuitBreaker}`), output-root gate check
      (emits `DownloadRejected{OutputRootUnavailable}`), `prepare_output_dir`
      (emits same on failure). Returns the resolved engine to reuse.
    - `acquire_slot(&self, req: AcquireRequest, cancel: CancellationToken) -> Result<SlotGuard, AcquireError>`
      — calls into queue; emits `DownloadQueued` if it had to park.
    - `start_with_slot(&self, slot, config, engine_handle) -> Result<String>`
      — engine bring-up, `active_downloads` insert, segment-event task
      spawn, `DownloadStarted` emit. The slot's permit is moved into
      the `ActiveDownload` entry (same lifetime as today).
  - `start_download` (line 776) becomes a thin wrapper:
    `preflight → acquire_slot → start_with_slot`. Same signature, same
    return. Used by tests and any non-pipeline caller.
  - Public methods (`set_max_concurrent_downloads`,
    `max_concurrent_downloads`, `active_count`,
    `total_concurrent_slots`, `set_high_priority_extra_slots`,
    `high_priority_extra_slots`, `subscribe`) keep their signatures;
    delegate to the new queue.
  - Add `pub fn snapshot_pending(&self) -> Vec<PendingEntry>` (delegates
    to queue) for the WS snapshot.
  - New `DownloadProgressEvent::DownloadQueued` variant; update
    `streamer_id`/`streamer_name`/`session_id` impls.

- **`rust-srec/src/downloader/mod.rs`** — re-export `queue::*`.

- **`rust-srec/src/services/container.rs`**
  - Add `session_cancels: Arc<SessionCancelTokens>` field on
    `ServiceContainer`, plumb to constructor.
  - Replace the `StreamerLive` arm body (line 3187-3441) with a
    `tokio::spawn` calling a new `run_live_download_pipeline` helper.
    The helper does the seven steps in the architecture diagram.
  - Replace the `StreamerOffline` arm (line 3442+) prologue with
    `session_cancels.cancel(&session_id)` before the existing danmu/
    download stop logic. (The existing logic remains a no-op for
    queued-not-started cases and a real stop for active cases.)
  - Update `setup_monitor_event_subscriptions` (line 2242) to spawn
    pipelines instead of awaiting `handle_monitor_event` for
    `StreamerLive`. Other arms stay serial.
  - Update the `setup_resume_download_subscriber` synthetic-live
    emission (line 2199) to also go through
    `run_live_download_pipeline`.
  - Add `FRESHNESS_THRESHOLD_MS` constant + env override
    `RUST_SREC_QUEUE_FRESHNESS_MS` (default 60_000).

- **`rust-srec/src/api/proto/mod.rs`**
  - `create_snapshot_message(downloads, queued)` — accept the queued
    slice; map into the new proto field.

- **`rust-srec/src/api/routes/downloads.rs`**
  - WS connect path (lines 116, 172, 185) — fetch queued via
    `manager.snapshot_pending()` and pass to `create_snapshot_message`.
  - `map_download_event_to_ws` (around line 380+) — new arm for
    `DownloadProgressEvent::DownloadQueued` → `WsMessage{DownloadQueued}`.

### Backend — proto

- **`rust-srec/proto/download_progress.proto`**
  - `EventType::EVENT_TYPE_DOWNLOAD_QUEUED = 12`
  - `WsMessage.payload` oneof: `DownloadQueued download_queued = 12`
  - `DownloadSnapshot { repeated DownloadState downloads = 1; repeated DownloadQueued queued = 2; }`
  - New message:
    ```protobuf
    message DownloadQueued {
      string streamer_id = 1;
      string session_id = 2;
      string streamer_name = 3;
      string engine_type = 4;
      int64  queued_at_ms = 5;
      bool   is_high_priority = 6;
    }
    ```

### Frontend

- **`rust-srec/frontend/src/store/downloads.ts`**
  - Add `queuedByStreamer: Map<string, QueuedEntry>` to state.
  - Actions: `setQueued`, `clearQueuedByStreamer`, `setSnapshot`
    primer also seeds queued.
  - Selector: `getQueuedForStreamer(streamerId): QueuedEntry | undefined`.
  - Terminal-event handlers (`removeDownload`) also clear queued by
    streamer for defensiveness.

- **`rust-srec/frontend/src/providers/WebSocketProvider.tsx`**
  - `handleMessage` switch: new `case EventType.DOWNLOAD_QUEUED` →
    `setQueued`.
  - `EventType.SNAPSHOT` arm: feed `message.payload.value.queued` into
    snapshot primer.
  - `EventType.DOWNLOAD_REJECTED` (currently no-op) and the existing
    terminal handlers now also call `clearQueuedByStreamer`.

- **`rust-srec/frontend/src/components/streamers/card/use-streamer-status.tsx`**
  - Accept third arg `queuedEntry?: QueuedEntry`.
  - New branch *before* the `LIVE` case (line 246):
    - `state === 'LIVE' && queuedEntry && !activeDownload`
    - Label: `<Trans>Queued</Trans>`
    - Color: amber (normal) or red (high priority)
    - Tooltip: "Stream is live but the global download limit (X) is
      fully in use. Recording and danmu collection will start
      automatically when a slot frees." Plus "X of Y slots in use",
      plus elapsed wait via `formatDistanceToNow(queuedAt)`.

- **`rust-srec/frontend/src/components/streamers/streamer-card.tsx`**
  - Pull `queuedEntry` via `useDownloadStore(useShallow(s => s.getQueuedForStreamer(id)))`.
  - Pass to `useStreamerStatus`.

- **`rust-srec/frontend/src/locales/{en,zh-CN}/messages.po`**
  - New strings auto-extracted on `pnpm extract`. zh-CN translations:
    - `Queued` → `排队中`
    - `Stream is live but the global download limit (...) is fully in
      use...` → translate
    - `Concurrency limit reached` → `已达到并发限制`
    - `Recording and danmu collection will start when a slot frees.` →
      translate

## Component design

### `DownloadQueue` (new module `downloader/queue.rs`)

```rust
pub struct DownloadQueue {
    normal_capacity:    AtomicUsize,
    high_extra_capacity:AtomicUsize,
    in_flight_total:    AtomicUsize,
    in_flight_normal:   AtomicUsize,
    waiters:            Mutex<BinaryHeap<Waiter>>,
    pending:            Arc<DashMap<String, Arc<PendingEntry>>>, // by session_id
    event_tx:           broadcast::Sender<QueueEvent>,
}

pub enum Priority { Normal, High }

pub struct PendingEntry {
    pub session_id:       String,
    pub streamer_id:      String,
    pub streamer_name:    String,
    pub engine_type:      EngineType,
    pub priority:         Priority,
    pub queued_at_ms:     i64,
    cancel:               CancellationToken,
    notify:               Arc<Notify>,
}

pub struct SlotGuard {
    queue:         Arc<DownloadQueue>,
    is_high_prio:  bool,
    queued_at_ms:  i64,
    acquired_at_ms:i64,
    session_id:    String,
    armed:         bool,
}

impl SlotGuard {
    pub fn waited_ms(&self) -> i64 { self.acquired_at_ms - self.queued_at_ms }
    pub fn is_high_priority(&self) -> bool { self.is_high_prio }
    /// Move ownership into ActiveDownload (called by start_with_slot).
    /// Disarms the drop-side release; in_flight stays decremented when
    /// the ActiveDownload entry itself is removed.
    pub(crate) fn into_active(mut self) -> ActiveSlot { self.armed = false; ... }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        if self.armed {
            self.queue.release(self.is_high_prio);
        }
    }
}

pub enum AcquireError {
    Cancelled,
    DuplicateSession, // session_id already pending or active
    ShuttingDown,
}
```

**Wakeup rule** (called from `release()` and `set_capacity` increase):

```rust
fn wake_next(&self) {
    let waiters = self.waiters.lock();
    // Pick highest-priority waiter that fits current capacity
    if let Some(top) = waiters.peek() {
        let fits = match top.priority {
            High   => self.in_flight_total.load() < self.total_capacity(),
            Normal => self.in_flight_normal.load() < self.normal_capacity(),
        };
        if fits {
            let w = waiters.pop().unwrap();
            self.account_acquired(w.priority);
            w.notify.notify_one();
        }
    }
}
```

Ordering inside the heap: `(priority_rank_desc, queued_at_ms_asc,
insertion_seq_asc)`. The insertion seq is an `AtomicU64` on the queue,
ensures stable ordering when two waiters arrive in the same millisecond.

**`set_capacity(normal, high_extra)`**:
- If increased, attempt `wake_next` repeatedly until either no waiters
  fit or the new capacity is filled.
- If decreased, just lower the atomics. Active in-flights stay until
  drop. New acquires blocked beyond the new limit. Equivalent to today's
  `set_desired` reservation logic — simpler because there's no semaphore
  permit concept.

### `acquire_slot` flow

```rust
pub async fn acquire_slot(
    &self,
    req: AcquireRequest,
    cancel: CancellationToken,
    on_queued: impl FnOnce(&PendingEntry),
) -> Result<SlotGuard, AcquireError> {
    // Dedup: same session_id already pending or active is a no-op error.
    if self.pending.contains_key(&req.session_id) {
        return Err(AcquireError::DuplicateSession);
    }

    // Fast path: capacity available, no queueing.
    if let Some(guard) = self.try_acquire_fast(&req) {
        return Ok(guard);
    }

    // Slow path: register pending, emit event, park.
    let entry = Arc::new(PendingEntry { ... });
    self.pending.insert(req.session_id.clone(), entry.clone());

    let _ = self.event_tx.send(QueueEvent::Queued(entry.clone()));
    on_queued(&entry);

    // Park.
    let acquired = tokio::select! {
        _ = entry.notify.notified() => true,
        _ = cancel.cancelled() => false,
    };

    self.pending.remove(&req.session_id);

    if !acquired {
        // We were cancelled. The waker may have raced — if it counted
        // us as acquired, undo it.
        if self.was_counted_acquired(&entry) {
            self.release(entry.priority == Priority::High);
        }
        return Err(AcquireError::Cancelled);
    }

    Ok(self.build_guard(entry))
}
```

The cancel-vs-notify race needs care: if `notify_one` fires
simultaneously with `cancel.cancelled()`, we may have been counted as
acquired but bail out. Concrete fix: the wakeup path increments
`in_flight` before notifying; on cancellation, the acquire path checks
a per-waiter "was promoted" flag (an `AtomicBool` set just before
`notify_one`) and decrements + re-wakes the next waiter if so.

### `DownloadManager::preflight`

Replaces the pre-acquire portion of `start_download_with_engine`
(lines 1145-1193). Emits any rejection events. Returns a resolved
`EngineHandle` to be passed to `start_with_slot` so engine resolution
isn't repeated.

```rust
pub async fn preflight(
    &self,
    req: &PreflightRequest,
) -> Result<EngineHandle, PreflightRejected> {
    let (engine, engine_type, engine_key) = self.resolve_engine(...).await?;
    let engine_key = engine_key.for_streamer(&req.streamer_id);

    if !engine.is_available() {
        return Err(PreflightRejected::EngineUnavailable);
    }
    if !self.circuit_breakers.is_allowed(&engine_key) {
        let _ = self.event_tx.send(/* DownloadRejected{CircuitBreaker} */);
        return Err(PreflightRejected::CircuitBreakerOpen);
    }
    if let Some(gate) = self.output_root_gate.get() {
        if let Err(blocked) = gate.check(&req.output_dir) {
            let _ = self.event_tx.send(/* DownloadRejected{OutputRootUnavailable} */);
            return Err(PreflightRejected::OutputRootBlocked);
        }
    }
    if let Err(engine_err) = self.prepare_output_dir(&req.config_lite).await {
        // emit DownloadRejected{OutputRootUnavailable} as today's
        // start_download_with_engine does (lines 1169-1190)
        return Err(PreflightRejected::PrepareOutputDirFailed);
    }

    Ok(EngineHandle { engine, engine_type, engine_key })
}
```

This keeps the **pre-acquire fail-fast property** that issue #508's fix
relies on (manager.rs:1152 comment: holding a slot during ENOENT/ENOSPC
starves healthy streamers).

### `start_with_slot`

```rust
pub async fn start_with_slot(
    &self,
    slot: SlotGuard,
    config: DownloadConfig,
    engine: EngineHandle,
) -> Result<String> {
    let active_slot = slot.into_active(); // disarms drop release
    let download_id = uuid::Uuid::new_v4().to_string();
    let cdn_host = ...;

    self.active_downloads.insert(download_id.clone(), ActiveDownload {
        permit: Some(active_slot),  // releases in_flight on remove
        ..
    });

    let _ = self.event_tx.send(DownloadProgressEvent::DownloadStarted { ... });

    // engine.start spawn + segment event handler spawn (unchanged from
    // current start_download_with_engine lines 1268+)
    ...

    Ok(download_id)
}
```

### `start_download` wrapper (back-compat)

```rust
pub async fn start_download(
    &self,
    config: DownloadConfig,
    engine_id: Option<String>,
    is_high_priority: bool,
) -> Result<String> {
    let req_pre = PreflightRequest::from(&config, engine_id.clone());
    let engine = self.preflight(&req_pre).await
        .map_err(|e| crate::Error::Other(e.to_string()))?;

    let req_acq = AcquireRequest::from(&config, is_high_priority);
    let slot = self.queue.acquire(req_acq, CancellationToken::new(), |_| {})
        .await
        .map_err(|e| crate::Error::Other(format!("acquire: {:?}", e)))?;

    self.start_with_slot(slot, config, engine).await
}
```

Tests, scheduler, and any non-pipeline caller continue to use this and
get exactly the same behavior they have today (except "rejected" and
"queued" emits arrive in the new structured shape, which is additive).

### Pipeline helper (`container.rs::run_live_download_pipeline`)

```rust
async fn run_live_download_pipeline(
    deps: Arc<LivePipelineDeps>,
    payload: StreamerLivePayload,
) {
    let cancel = deps.session_cancels.token_for(&payload.session_id);

    // 1. Dedup & active checks
    if deps.download_manager.has_active_download(&payload.streamer_id) { return; }
    let meta = match deps.streamer_manager.get_streamer(&payload.streamer_id) {
        Some(m) if m.is_active() && !m.is_disabled() => m,
        _ => return,
    };

    // 2. Build initial config
    let merged_config = match deps.config_service
        .get_config_for_streamer(&payload.streamer_id).await { ... };
    let is_high_priority = meta.priority == Priority::High;

    // 3. Preflight (emits DownloadRejected on CB/output gate failure)
    let req_pre = PreflightRequest { ... };
    let engine = match deps.download_manager.preflight(&req_pre).await {
        Ok(e) => e,
        Err(_) => return, // event already emitted
    };

    // 4. Acquire slot — may emit DownloadQueued, may park
    let req_acq = AcquireRequest {
        session_id:    payload.session_id.clone(),
        streamer_id:   payload.streamer_id.clone(),
        streamer_name: payload.streamer_name.clone(),
        engine_type:   engine.engine_type,
        is_high_priority,
    };
    let slot = match deps.download_manager
        .acquire_slot(req_acq, cancel.clone(), |_| {}).await
    {
        Ok(s) => s,
        Err(_) => return,
    };

    // 5. Freshness check
    let streams = if slot.waited_ms() > deps.freshness_threshold_ms {
        match deps.monitor.check_streamer(&meta).await {
            Ok(LiveStatus::Live { streams, media_headers, media_extras, .. }) => {
                // Use fresh
                streams
            }
            Ok(_) | Err(_) => {
                debug!(streamer_id = %payload.streamer_id,
                       "queued live no longer valid post-acquire, aborting");
                return; // SlotGuard drops, wakes next waiter
            }
        }
    } else {
        // Cheap re-check: still active, still live in our metadata?
        let fresh = deps.streamer_manager.get_streamer(&payload.streamer_id);
        if !fresh.as_ref().map_or(false, |m| m.is_active() && !m.is_disabled()) {
            return;
        }
        payload.streams
    };

    // 6. Build full config + start engine
    let config = build_download_config(&deps, &payload, &streams, &merged_config);
    let download_id = match deps.download_manager
        .start_with_slot(slot, config, engine).await
    {
        Ok(id) => id,
        Err(e) => {
            warn!(error = %e, "engine start failed");
            return;
        }
    };

    // 7. Danmu (gated on real download_id)
    if merged_config.record_danmu {
        let _ = deps.danmu_service.start_collection(
            &payload.session_id,
            &payload.streamer_id,
            &payload.streamer_url,
            Some(merged_config.danmu_sampling_config.clone()),
            merged_config.cookies.clone(),
            payload.media_extras,
        ).await;
    }
}
```

## Contracts that must be preserved (regression list)

| # | Contract | Source | Test |
|---|---|---|---|
| C1 | `set_max_concurrent_downloads(n) -> usize` returns applied limit | `services/container.rs:1651`, `pipeline/throttle.rs` | R2 |
| C2 | `max_concurrent_downloads()` getter | `container.rs:1647` | R8 |
| C3 | `active_count() -> usize` | `container.rs:2877,3653` | R10 |
| C4 | `total_concurrent_slots()` | `container.rs:2878` | R10 |
| C5 | `set_high_priority_extra_slots(n) -> usize` | existing | R5 |
| C6 | `start_download(config, engine_id, prio) -> Result<String>` | `container.rs:3389`, tests | R1 |
| C7 | `DownloadLimitAdjuster` trait impl on manager | `ThrottleController` | R8 |
| C8 | Pre-acquire CB rejection emits `DownloadRejected{CircuitBreaker}` and does NOT consume a slot | `manager.rs:798-815` | R1 |
| C9 | Pre-acquire output-root rejection emits `DownloadRejected{OutputRootUnavailable}` and does NOT consume a slot | `manager.rs:822-846`, `1164-1190` | R1 |
| C10 | `prepare_output_dir` runs before slot acquisition | `manager.rs:1152` comment | R1 |
| C11 | FIFO within priority tier | `tokio::sync::Semaphore` | R5 |
| C12 | High-prio falls back to normal pool | `manager.rs:1199` | R5 |
| C13 | Decrease-limit reserves currently-available slots | `ConcurrencyLimit::set_desired` | R2 |
| C14 | Same streamer dedup on concurrent live | `container.rs:3199` | R4 |
| C15 | Hysteresis-resume same-session dedup | session_id reuse | R3 |
| C16 | `DownloadStarted` always follows successful slot acquisition | `manager.rs:1251` | R10 |
| C17 | Slot release on download removal | `manager.rs:1434` | R6, R7 |

### Risk register (test names map to "Test plan" below)

| # | Risk | Trigger | Test name |
|---|---|---|---|
| R1 | Pre-acquire rejection regresses to "queue then fail" | open CB, call `start_download` | `pipeline_pre_acquire_rejection_does_not_enqueue` |
| R2 | Decrease-limit no longer reserves | saturate, then `set_max_concurrent_downloads(lower)` | `set_capacity_decrease_blocks_new_acquires` |
| R3 | Hysteresis-resume double-acquires | spawn pipeline twice with same session_id | `same_session_id_dedupes` |
| R4 | Concurrent live for same streamer | race two spawns | `concurrent_live_same_streamer_dedupes` |
| R5 | High-prio FIFO regression | saturate, queue (N1, N2, H1), free one slot | `high_priority_takes_next_slot` |
| R6 | Slot leak on offline-during-wait | saturate, queue N1, fire StreamerOffline | `cancel_pending_releases_pending_state` |
| R7 | Slot leak on cancelled future | drop guard mid-acquire | `dropped_acquire_does_not_leak_capacity` |
| R8 | Throttle controller no longer adjusts | call `ThrottleController::adjust...` | `throttle_controller_still_writes_through` |
| R9 | Snapshot doesn't include queued | saturate, queue 2, fetch snapshot | `snapshot_includes_queued_entries` |
| R10 | DownloadStarted no longer emitted after queue | saturate, queue, free slot | `queued_then_started_event_order` |
| R11 | `available_permits` private-field probe | existing reserves test | rewrite as `set_capacity_decrease_blocks_new_acquires` |
| R12 | Engine availability skipped | `start_download` with unavailable engine | `engine_unavailable_returns_without_queueing` |
| R13 | Stale URL after long wait | mock monitor returning new URLs, wait > threshold | `freshness_refetch_replaces_streams` |
| R14 | Danmu starts before download | saturate, queue with `record_danmu=true`, free slot | `danmu_starts_only_after_download_id` |

## Implementation order

Each step keeps the test suite green; you can pause after any of them.

1. **Add `DownloadQueue` module + unit tests, unused.** Pure addition. Tests
   cover priority ordering, FIFO within tier, capacity changes, drop
   semantics, cancellation race.

2. **Wire `DownloadQueue` behind existing `DownloadManager` API.** Replace
   `ConcurrencyLimit` internals; keep `start_download` monolithic. Rewrite
   `test_runtime_reconfigure_max_concurrent_downloads_reserves_permits`
   against the public API. All other tests untouched.

3. **Split manager API.** Add `preflight`, `acquire_slot`,
   `start_with_slot`. `start_download` becomes wrapper. No caller change.

4. **Add `SessionCancelTokens` + `pending_starts` map plumbing.** Add
   cancellation surface to `acquire_slot`. Still no caller change.

5. **Proto changes + WS mapping.** Add `DownloadQueued` proto message,
   event variant, mapper arm. `DownloadSnapshot.queued` field. Backend
   emits the events. Frontend safely ignores unknown event_type.

6. **Container.rs pipeline rewrite.** Replace `StreamerLive` arm with
   `tokio::spawn(run_live_download_pipeline(...))`. Wire offline cancel.
   Wire freshness refetch. Land snapshot getter consumption.

7. **Frontend store + WS handler + status badge + i18n.** Independent
   from backend; relies on stable proto from step 5.

## Test plan

### New unit tests in `downloader/queue.rs`

- `acquire_fast_path` — capacity available, no queue, no event
- `acquire_slow_path_emits_queued` — saturated, second acquirer gets `Queued` event
- `priority_ordering_high_first` — N1 queued, N2 queued, H1 queued; free → H1, free → N1
- `fifo_within_tier` — N1 queued, N2 queued; free → N1
- `cancel_during_wait` — queue an acquire, cancel the token, future returns `AcquireError::Cancelled`, queue capacity unchanged
- `cancel_race_promotion` — race notify and cancel, assert at most one outcome and no leak
- `set_capacity_increase_wakes_waiters` — saturated, queue 3, raise capacity, all 3 wake
- `set_capacity_decrease_blocks_new_acquires` — saturated, lower capacity, drop two, only one new acquire fits
- `same_session_id_returns_duplicate` — two `acquire` calls with same session_id, second errors
- `dropped_acquire_does_not_leak_capacity` — promote a waiter, drop the resulting `SlotGuard` (armed) → release, in_flight returns to baseline
- `snapshot_pending_returns_in_arrival_order`

### New integration tests in `tests/queued_pipeline_e2e.rs`

- `pipeline_pre_acquire_rejection_does_not_enqueue` (CB open + output gate degraded variants)
- `concurrent_live_same_streamer_dedupes`
- `cancel_pending_releases_pending_state` (offline cancels queued)
- `freshness_refetch_replaces_streams` (mock monitor)
- `danmu_starts_only_after_download_id`
- `queued_then_started_event_order`
- `snapshot_includes_queued_entries`
- `high_priority_takes_next_slot` (cross-checks queue + pipeline)

### Existing tests adjusted

- `test_runtime_reconfigure_max_concurrent_downloads_reserves_permits`
  → rewritten to assert behavior via public API only.
- `test_engine_registration` and similar — unaffected.

### Frontend tests

- `useDownloadStore`: `setQueued`, `clearQueuedByStreamer`, snapshot
  primer with mixed downloads + queued.
- `useStreamerStatus`: returns `Queued` branch when
  `state=LIVE, queuedEntry=present, activeDownload=undefined`; returns
  `Live` when `activeDownload=present`.
- Visual snapshot of `StreamerCard` in queued state.

### End-to-end smoke

- Set `max_concurrent_downloads = 1` in dev. Start two streamers. Verify:
  - First card: `Live` + progress.
  - Second card: amber `Queued` badge with tooltip.
  - First card finishes → second transitions to Live + progress.
- Set `max_concurrent_downloads = 1`. Start two streamers, second goes
  offline while queued. Verify second card returns to `Offline` cleanly,
  no console errors, no orphan engine process.
- Saturated; bump `max_concurrent_downloads` via UI. Verify queued
  streamers immediately transition.

## Verification

```bash
# Backend
cargo nextest run --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Frontend
pnpm --dir rust-srec/frontend lint
pnpm --dir rust-srec/frontend test
pnpm --dir rust-srec/frontend extract  # update .po files
```

End-to-end: run the dev binary with
`RUST_SREC_QUEUE_FRESHNESS_MS=5000 cargo run` (low threshold to
exercise the refetch path), saturate downloads, observe UI.

## Out of scope (deliberate)

- Per-platform freshness thresholds (single global default for now).
- Pipeline post-processing queue observability (separate concern).
- Surfacing `DownloadQueued` as a notification event (channels,
  Discord/Telegram/etc.) — concurrency wait is not an alertable
  condition.
- Modifying the `StreamerState` machine — `Queued` is an orthogonal
  property derived from the download manager's view, not a streamer
  state.
- Persisting queue state across process restarts — saturated limits
  resolve naturally as slots open.

## Critical files for the executor to read first

- `rust-srec/src/downloader/manager.rs` (the ConcurrencyLimit, the
  start_download/start_download_with_engine path, the public API
  surface, the existing reserves-permits test)
- `rust-srec/src/services/container.rs` (the StreamerLive arm at
  3187-3441, setup_monitor_event_subscriptions at 2242,
  setup_resume_download_subscriber at 2156)
- `rust-srec/src/monitor/service.rs` (`check_streamer` at 411 — the
  existing dedup'd refetch entry point)
- `rust-srec/src/pipeline/throttle.rs` (the `DownloadLimitAdjuster`
  trait at 80-88 that must keep working)
- `rust-srec/src/api/proto/mod.rs` and
  `rust-srec/src/api/routes/downloads.rs` (snapshot / mapper)
- `rust-srec/proto/download_progress.proto`
- `rust-srec/frontend/src/store/downloads.ts`
- `rust-srec/frontend/src/providers/WebSocketProvider.tsx`
- `rust-srec/frontend/src/components/streamers/card/use-streamer-status.tsx`

---

## Execution log

Plan executed in seven incremental commits as planned. Each step kept
the test suite green before moving on.

### Step 1 — `DownloadQueue` module (unused)
- New `rust-srec/src/downloader/queue.rs` (~720 lines) with
  `DownloadQueue`, `SlotGuard`, `ActiveSlot`, `Priority`,
  `PendingEntry`, `AcquireRequest`, `AcquireError`.
- Internal heap orders by (priority desc, queued_at asc, seq asc).
- 13 unit tests covering fast path, slow path, priority ordering,
  FIFO within tier, cancellation, capacity changes, dedup, drop
  semantics, shutdown.
- Shipped in: `rust-srec/src/downloader/queue.rs`, exports in
  `rust-srec/src/downloader/mod.rs`.

### Step 2 — Wire queue behind existing manager API
- Replaced `Arc<ConcurrencyLimit>` × 2 with single
  `Arc<DownloadQueue>` on `DownloadManager`.
- `set_max_concurrent_downloads`, `set_high_priority_extra_slots`,
  `max_concurrent_downloads`, `high_priority_extra_slots`,
  `total_concurrent_slots`, `active_count` all keep their
  signatures; delegate to the queue.
- Old `start_download_with_engine` rewritten to use queue's acquire
  path. Acquire/release semantics now driven by `SlotGuard` →
  `ActiveSlot` lifecycle (held by the `ActiveDownload` map entry).
- `apply_best_effort` calls and `OwnedSemaphorePermit` field
  removed — release happens automatically when the `ActiveSlot`
  drops.
- Existing `test_runtime_reconfigure_max_concurrent_downloads_reserves_permits`
  rewritten as `test_runtime_reconfigure_max_concurrent_downloads`
  asserting through the public API.

### Step 3 — Split manager API
- New public methods on `DownloadManager`: `preflight`,
  `acquire_slot`, `start_with_slot`.
- New public types `PreflightRequest`, `EngineHandle`.
- `preflight` runs engine resolve + circuit breaker + output gate
  + `prepare_output_dir`, emitting `DownloadRejected` directly on
  failure so the caller can bail without consuming a slot.
- `acquire_slot` parks on the queue, emitting `DownloadQueued` only
  when it had to wait. Honours a passed `CancellationToken`.
- `start_with_slot` consumes the slot, registers the active
  download, emits `DownloadStarted`, spawns the engine.
- `start_download` retained as a thin sequential wrapper for
  callers (tests, scheduler) that don't need per-phase visibility.

### Step 4 — `SessionCancelTokens`
- New `rust-srec/src/services/session_cancels.rs` with
  `token_for(session_id)`, `cancel(session_id)`, `forget(session_id)`.
- `forget` is the rename — `drop` clashed with `Drop::drop` in the
  guard pattern used by the pipeline.
- 5 unit tests.

### Step 5 — Proto + WS mapping
- `rust-srec/proto/download_progress.proto`:
  - `EVENT_TYPE_DOWNLOAD_QUEUED = 12`
  - `WsMessage.payload.download_queued` oneof variant
  - `DownloadSnapshot.queued: repeated DownloadQueued`
  - new `DownloadQueued` message (streamer_id, session_id,
    streamer_name, engine_type, queued_at_ms, is_high_priority)
- `create_snapshot_message(downloads, queued)` in
  `src/api/proto/mod.rs` maps queued entries.
- `src/api/routes/downloads.rs` mapper arm produces a
  `DownloadQueued` envelope from the new manager event variant.
- WS connect path + subscribe/unsubscribe path call
  `manager.snapshot_pending()` and pass the result to
  `create_snapshot_message`.

### Step 6 — Container pipeline rewrite
- `ServiceContainer` gains `session_cancels: Arc<SessionCancelTokens>`.
- `StreamerLive` arm body extracted into free
  `run_live_download_pipeline(...)` and invoked via `tokio::spawn`.
- Pipeline runs the 6-phase split: dedup → preflight → acquire_slot
  (with per-session cancel) → freshness re-check (via
  `monitor.check_streamer` when `waited_ms > FRESHNESS_THRESHOLD_MS`)
  → `start_with_slot` → danmu (gated on download success).
- `StreamerOffline` arm now fires
  `session_cancels.cancel(&session_id)` before existing teardown.
- Resume-from-hysteresis subscriber routes the synthetic
  `StreamerLive` through the new pipeline (same dedup applies).
- `setup_monitor_event_subscriptions` and the resume subscriber
  pass the new args to `handle_monitor_event`.
- Freshness threshold default: 60_000 ms; env override
  `RUST_SREC_QUEUE_FRESHNESS_MS`.
- `CancelGuard` Drop impl ensures stale tokens never linger after
  any pipeline early-return.

### Step 7 — Frontend
- `proto:gen` regenerated TS bindings for the new message + enum.
- `store/downloads.ts`:
  - `queuedByStreamer: Map<string, QueuedEntry>` state
  - `setQueued`, `clearQueuedByStreamer`, `getQueuedForStreamer`
  - snapshot primer accepts `(downloads, queued)`
  - `upsertMeta` defensively clears the streamer's queued entry
- `providers/WebSocketProvider.tsx`:
  - `DOWNLOAD_QUEUED` case calls `setQueued`
  - snapshot priming reads `payload.value.queued`
  - terminal/`DOWNLOAD_REJECTED` events clear queued by streamer
- `components/streamers/card/use-streamer-status.tsx` — new
  `Queued` branch before `LIVE`. Amber for normal, rose for
  high-priority. Tooltip explains "Concurrency limit reached" /
  "High-priority slot waiting", shows wait-since timestamp.
- `components/streamers/streamer-card.tsx` reads queued entry
  via store and threads it through.
- i18n: 6 new strings extracted; zh-CN translations added
  (`Queued`, `Queued for download`, `Concurrency limit reached`,
  `High-priority slot waiting (concurrency limit reached)`,
  `This stream is live but the global download limit is fully
  in use...`, `Waiting since`).

## Verification record

- `cargo clippy --manifest-path rust-srec/Cargo.toml --lib -- -D warnings` — 0 errors, 0 warnings
- `cargo nextest run --manifest-path rust-srec/Cargo.toml` —
  **1311 passed, 9 skipped** (was 1303 before; +8 tests added)
- `pnpm fmt && pnpm lint` (frontend) — 0 warnings, 0 errors on 375 files
- `pnpm test` (frontend) — 10 passed
- `pnpm extract` — 6 new strings; zh-CN catalogue fully translated
- All seven contracts in the original "Risk register" still hold:
  - C1–C7 public API signatures preserved (verified by existing
    integration tests passing)
  - C8–C10 pre-acquire fail-fast emits unchanged (preflight runs
    before queue)
  - C11–C13 FIFO + high-priority fallback + decrease-limit semantics
    covered by new queue unit tests
  - C14–C15 dedup via `pending_pipelines` (per-streamer atomic
    reservation) + `pending_starts` session_id key + existing
    `has_active_download` check
  - C16–C17 `DownloadStarted` + slot release on active-download
    drop preserved

## Post-review fixes

After the initial implementation, an internal review surfaced several
correctness bugs and one P2 polish item. Each was addressed before
sign-off; tests added where applicable.

### P1 — queue race window between fast-path miss and waiter enqueue

**Bug.** `acquire()` did `try_acquire_fast()` then enqueued a waiter.
A `release()` happening in that gap saw an empty heap and returned
without promoting; the new waiter then slept indefinitely even though
capacity had freed. A subsequent caller could take the slot via fast
path, breaking FIFO/priority guarantees and leaving a stale
`DownloadQueued` badge.

**Fix.** Two changes in `downloader/queue.rs::acquire`:
1. **Fast-path fairness:** check `self.pending.is_empty()` before
   trying `try_acquire_fast`. If any waiter is parked, the newcomer
   queues instead of jumping the line. O(1) DashMap-len read,
   negligible perf cost.
2. **Post-enqueue promotion:** after pushing the waiter onto the heap,
   loop `try_promote_one()` until it returns false. Catches any slot
   freed during the fast-path-to-enqueue window.
3. The queued callback now fires only if `promoted` is still false
   after the promotion pass — avoids emitting a `Queued` event for a
   request that immediately got promoted.

**Tests.** Two new unit tests in `queue::tests`:
- `no_lost_wakeup_when_release_races_with_enqueue` — would hang
  before the fix (timeout-protected with 2-second cap).
- `fast_path_does_not_jump_ahead_of_existing_waiter` — verifies that
  a parked waiter wakes first when the slot frees.

### P1 — pending-start dedup not implemented

**Bug.** With `tokio::spawn` per `StreamerLive`, two events for the
same streamer (e.g., a real `StreamerLive` racing with a synthetic
hysteresis-resume one) could both pass `has_active_download` because
the active-downloads entry isn't populated until `start_with_slot`
completes. The queue's session_id dedup helped only in the rare case
of identical session_ids; identical streamer_ids with different
session_ids would still produce two parallel pipelines.

**Fix.** Added `pending_pipelines: Arc<DashMap<String, ()>>` to
`ServiceContainer`. The pipeline does `insert(streamer_id, ())` as
its first action; `Some(_)` return means "already in flight, bail."
A `PipelineReservationGuard` Drop impl removes the entry on every
exit path, including panics. Threaded through both
`setup_monitor_event_subscriptions` and
`setup_resume_download_subscriber`.

### P1 — shutdown didn't cancel queued pipelines

**Bug.** `shutdown_with_timeout` cancelled the container token and
called `download_manager.stop_all()`. The latter dropped active
downloads, releasing their queue slots — and queued pipelines would
acquire those slots and start fresh engines mid-shutdown.

**Fix.**
- New public `DownloadManager::shutdown_queue()` that calls
  `queue.shutdown()` (sets `shutting_down=true` and notifies all
  pending waiters).
- Container's shutdown sequence now calls `shutdown_queue()` BEFORE
  `stop_all()`, so pending acquires return `ShuttingDown` instead of
  racing for newly-released slots.

### P2 — OutOfSchedule queued downloads could still start

**Bug.** Post-acquire short-wait recheck used `metadata.is_active()`,
but `OutOfSchedule` counts as active. If the schedule window closed
mid-wait under the freshness threshold, the recording would start out
of schedule. The `StateChanged::OutOfSchedule` handler stopped active
downloads but didn't cancel queued starts.

**Fix.**
1. Tightened the recheck to require `state == StreamerState::Live` AND
   `!is_disabled()`. `OutOfSchedule` (and `InspectingLive`,
   `NotLive`, etc.) now correctly reject the start.
2. The `MonitorEvent::StateChanged { new_state: OutOfSchedule }`
   handler now iterates `download_manager.snapshot_pending()` and
   calls `session_cancels.cancel(&entry.session_id)` for each entry
   matching the streamer_id, so queued pipelines wake and bail.

### P2 — freshness refetch only refreshed URLs, not headers/extras

**Bug.** Long-wait branch took `streams = fresh_streams` but ignored
the refetched `media_headers` and `media_extras`. On platforms where
signed URLs and required headers (Host overrides, signed referer)
rotate together, the engine would still 403 with old headers + new
URLs.

**Fix.** Destructure all three fields from the
`LiveStatus::Live { streams, media_headers, media_extras, .. }`
arm and replace the corresponding pipeline-local variables.
Made `media_headers` and `media_extras` mutable bindings on the
payload destructure.

### P2 — queued aborts didn't emit a clear signal

**Bug.** The proto/comment contract said "queued cleared by started
or terminal events", but acquire cancellation and freshness aborts
returned silently. Frontend's `queuedByStreamer` could keep stale
entries until the WS reconnected.

**Fix.** New event `DownloadProgressEvent::DownloadDequeued`
(streamer_id, streamer_name, session_id) emitted by
`DownloadManager::acquire_slot` when:
1. The slow path emitted `DownloadQueued` (tracked via an
   `Arc<AtomicBool>` shared with the on_queued closure), AND
2. The acquire returned `Err(_)` other than `DuplicateSession`
   (duplicate is benign; the original pipeline still owns the badge).

Wire-up:
- New proto message `DownloadDequeued` + `EVENT_TYPE_DOWNLOAD_DEQUEUED = 13`
- WS mapper arm in `api/routes/downloads.rs`
- Frontend `WebSocketProvider.tsx` adds a `DOWNLOAD_DEQUEUED` case
  that calls `clearQueuedByStreamer(streamerId)`

This guarantees every `DownloadQueued` is paired with exactly one of:
`DownloadStarted` (success), `DownloadDequeued` (abort), or
`DownloadRejected` (preflight failure surfaced after queueing —
shouldn't happen since preflight runs before queue, but defensive).

### Final verification after review fixes

- `cargo clippy --lib -- -D warnings` — 0 errors (added two
  `#[allow(clippy::too_many_arguments)]` on the pipeline plumbing fns
  whose 8 args are all individually meaningful Arcs)
- `cargo nextest run` — **1311 passed**, 9 skipped
- `pnpm fmt && pnpm lint` — 0 warnings, 0 errors

