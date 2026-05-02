# Release Notes

## `unreleased`

This update covers two independent themes: (1) **session-lifecycle Hysteresis FSM fixes** — closing a data-loss bug where a brief disconnect would silently stop recording for the rest of the broadcast, and adding atomicity guarantees to the FSM's critical concurrent paths; (2) the **output-root write gate** — fixing a class of failures where rust-srec could not recover from a filesystem issue (disk full, stale Docker bind mount) without a container restart. It also ships the initial scaffolding for backend localization.

## Session lifecycle / Hysteresis FSM fixes

> Background: the `refactor/session-hysteresis` branch introduced a `Recording → Hysteresis → (Recording | Ended)` state machine for recording sessions, designed to absorb "ambiguous" disconnects (FLV TCP close, transient engine errors) so brief network jitter doesn't immediately end recording. Production logs surfaced multiple boundary conditions where the FSM was bypassed or raced against downstream — this update fixes them layer by layer.

- **Fixed silent recording stop after FLV clean disconnect**

  (kinetic / 2026-05-02: 1.5-hour gap from 02:28 to 03:51)

  When the engine reported `Completed` + `CleanDisconnect`, `SessionLifecycle` correctly parked the session in Hysteresis (80-second quiet-period). But `resume_from_hysteresis`, on observing LIVE within the window, short-circuited *before* `start_or_resume`, bypassing the atomic transaction that enqueues the `MonitorEvent::StreamerLive` outbox event. The container's `handle_monitor_event::StreamerLive` is the **only** production path that calls `download_manager.start_download(...)` — so the resumed session showed "Live" in memory but the download was never actually restarted.

  Fix: `SessionTransition::Started` carries an optional `DownloadStartPayload` sidecar (streamer_url + streams + media_headers + media_extras). The container subscribes to `Started{from_hysteresis: true}`, synthesizes a `MonitorEvent::StreamerLive` from the payload, and dispatches through the existing handler — same code path as a fresh start. The `has_active_download` idempotency guard plus a new `is_session_active` defense make the subscriber safe against races.

- **Atomic CAS for `Hysteresis → (Recording | Ended)` transitions**

  `enter_ended_state` (called by hysteresis-timer fire, `on_offline_detected`, authoritative `on_download_terminal`) and `resume_from_hysteresis` (called by `on_live_detected`) could previously mutate `self.sessions` and `self.hysteresis` concurrently for the same session_id. Without atomicity, possible inconsistent outcomes:
  - Memory says Recording but DB says Ended (Ended wins remove, Resumed overwrites in-memory state but DB end_time was already committed).
  - Three transitions broadcast for one session_id within milliseconds (`Resumed + Started{from_hysteresis: true}` from resume, then `Ended` from a losing path that didn't realize it lost).

  Fix: `self.hysteresis.remove(session_id)` is now the single CAS point (DashMap removes are per-key atomic). The path that successfully removes the handle wins; the loser detects a snapshot mismatch (`was_in_hysteresis=true && claim=None`) and bails — no DB write, no in-memory update, no broadcast. `resume_from_hysteresis` returns `None` on CAS loss; `on_live_detected` falls through to `start_or_resume`, naturally producing a fresh `Created` session_id (the prior session is now Ended).

- **Stop classifying transient HTTP 404s as DefinitiveOffline**

  (Minana呀 / 2026-04-29: three consecutive 0-byte ghost sessions)

  The `OfflineClassifier` previously promoted any mesio 404 to `DefinitiveOffline { PlaylistGone(404) }`, bypassing hysteresis and ending the session immediately. Production logs proved this overfires in two cases:
  - **FLV initial-request 404**: Douyu and similar CDNs occasionally return 404 on a freshly-issued stream URL while the new token propagates to the edge — the platform monitor still reports LIVE, but the FSM ended the session.
  - **HLS segment / playlist mid-stream 404**: sliding-window eviction races, signed-URL token expiry on platforms that 404 instead of 403, CDN edge desync — any of these mark a still-live stream as dead.

  Drop the classification rule. True offline now flows through two more precise channels:
  - Consecutive `Network` failures (count = `offline_check_count`, window = `count × interval_ms`, all sourced from scheduler config — single source of truth shared with `HysteresisConfig`).
  - HLS `#EXT-X-ENDLIST` (mesio detected it internally; this PR plumbs the signal end-to-end as `EngineEndSignal::HlsEndlist`, so the session ends authoritatively without waiting ~90 seconds for hysteresis to expire).

- **Fixed actor-side spurious `StreamerOffline` emit on clean engine disconnect**

  (沈心 / 2026-05-01: multiple empty session cards within one broadcast)

  `StreamerActor::handle_download_ended` called `process_status(LiveStatus::Offline)` on the `DownloadEndPolicy::StreamerOffline | Stopped(_)` path — making the monitor emit a `MonitorEvent::StreamerOffline` authoritative event even when the engine had only TCP-closed cleanly. That bypassed the hysteresis quiet-period the FSM had just entered.

  New `DownloadEndPolicy::Completed` variant carries the "engine clean end, platform status ambiguous" semantic. `scheduler::service` routes `DownloadTerminalEvent::Completed` to it; the actor's new arm only updates local scheduling state (resume short polling, increment `offline_observed`) without pushing to the monitor. Mirrors the existing `DanmuStreamClosed` arm precedent — the FSM owns authority decisions.

- **Three-layer defense against 0-byte ghost sessions**

  Even after the fixes above, residual edge cases (initial-request errors before first byte, etc.) can still produce empty `live_sessions` rows. Three complementary defenses:

  1. **API filter** (`SessionFilters::include_empty`, default `false`) — `GET /sessions` excludes `total_size_bytes=0` ended sessions by default; active sessions (`end_time IS NULL`) are always kept. Diagnostic access via `?include_empty=true` and `GET /sessions/:id`.
  2. **Background janitor (`SessionJanitor`)** — periodic `DELETE FROM live_sessions WHERE total_size_bytes = 0 AND end_time IS NOT NULL AND end_time < ?`. Defaults: 5-minute retention, 30-minute interval. All four FK references (`media_outputs` / `danmu_statistics` / `session_segments` / `session_events`) have `ON DELETE CASCADE` configured, so child rows clean up automatically. Idempotent and crash-safe (the SELECT predicate is the source of truth).
  3. **Small-segment guard (existing)** — `services::container`'s `min_segment_size_bytes` threshold deletes the on-disk file but previously left the row. The first two layers fill that gap.

- **Classifier window/threshold derived from scheduler config**

  Previously hardcoded constants `60s window / threshold 2`. Now `OfflineClassifier::from_scheduler(count, interval_ms)`: window = `count × interval_ms`, threshold = `max(count, 2)` (floor of 2 preserves Bilibili-style mid-stream RST safety). Same source as `HysteresisConfig::from_scheduler` — operators tune one place for "how long until I believe the stream is offline."

- **HLS `#EXT-X-ENDLIST` plumbed end-to-end**

  Mesio's HLS coordinator already detected ENDLIST internally, but the signal was discarded at two `// TODO(hysteresis)` sites in the rust-srec wrapper. This PR threads `HlsStreamEvent::EndlistEncountered` from the playlist engine through a new channel, translates it to `HlsData::EndMarker(Some(SplitReason::EndOfStream))` in the mesio HLS filter, and observes it via a new `inspect` closure on `consume_stream`, finally promoting to `EngineEndSignal::HlsEndlist`. All four `hls-fix` operators are verified to preserve `EndMarker` reasons.

## Frontend

- **Session-detail Timeline tab counter fixed** — the badge previously counted only `session.titles`, ignoring the new `session.events`. Now sums both, correctly reflecting the entries rendered in the tab body.
- **`terminal-cause` translation disambiguation** — the timeline's `Completed/Failed/Cancelled/Rejected/...` labels previously shared lingui keys with the pipeline-jobs status list, so Simplified Chinese rendered `原因：已完成` ("Reason: Done") under a `Pending Confirmation` badge — semantically nonsensical. Wrapped in `<Trans context="terminal-cause">` for distinct keys with more accurate Chinese: `Completed → 下载断开`, `Failed → 下载失败`, `Streamer Offline → 主播离线`, `Consecutive Failures → 连续失败`, etc.
- **`Confirmed via backstop timer` translation fixed** — Simplified Chinese changed from "通过备份计时器确认" (reads as "via backup/redundant timer") to "等待恢复超时后确认" (reflects the actual semantic: "confirmed after wait-for-resume timed out").

## Highlights

- Added **output-root write gate** for recording filesystem failure resilience ([#508](https://github.com/hua0512/rust-srec/issues/508))

  When the recording disk fills or the target mount becomes unwritable, rust-srec now pauses recordings at the filesystem boundary, exposes the situation in `/health`, emits one critical notification with actionable recovery text, and auto-recovers when the filesystem becomes writable again — without restart for the common out-of-space case. For the specific case where a Docker bind mount has been broken by host-side cleanup (e.g., BaoTa panel's move-to-trash on a mounted directory), the gate cannot auto-recover (it's a Linux VFS limitation unrelated to rust-srec), but it now detects the situation within one monitor tick, stops the cascading retry storm that was burying the logs, and tells the user to restart the container with clear recovery instructions.

  **Replaces the 40+ cascading failure storm** that was the user-visible symptom of #508 with a single clean `Degraded` status and one notification. See the new [Docker troubleshooting guide](../getting-started/docker.md#freeing-up-disk-space-when-using-bind-mounts) for the safe cleanup paths that avoid the stale-mount trap.

- Added **runtime ENOSPC detection** in the ffmpeg and streamlink engines

  The engine stderr readers now watch for `"No space left on device"` / errno `-28` / exit code 228 and emit a `SegmentEvent::DiskFull` to the download manager, which routes it into the write gate. This is critical for the common case where the disk fills mid-recording while today's date directory already exists, so the pre-start `ensure_output_dir` hook can't catch it.

- Added **`StreamerState::OutOfSpace` runtime wiring**

  The state existed in the domain model but was never set at runtime. It now lights up when the write gate blocks a streamer, and clears automatically when the gate recovers. Visible in the streamer list as a stop-state badge.

- Added **backend notification localization** via `rust-i18n`

  New `rust-srec/locales/{en,zh-CN}.yml` files, new `RUST_SREC_LOCALE` environment variable. **Every notification event** is localized in both English and Simplified Chinese — stream online/offline, download lifecycle, segments, pipeline jobs, system alerts, and credential events. Channels that deliver to external receivers (Telegram, Gotify, Discord, webhook, email, web push) honor the locale automatically.

- Added **`output_path_inaccessible` notification event** and frontend subscription

  Distinct from the existing `out_of_space` proactive disk warning: this fires when the gate has *actually blocked* recordings. Priority is Critical. Emitted exactly once per `Healthy → Degraded` transition (not per failed attempt). Delivered through every enabled notification channel. Visible in the subscription manager with a distinct red shade.

- Added **one-shot startup probe** for configured output roots

  On container boot, after streamer hydration and before the scheduler starts, the gate runs a bounded 5-second probe per configured root (via `spawn_blocking` + timeout) to surface broken mounts from second zero instead of waiting for the first monitor tick to try starting a download.

## New environment variables

| Variable | Purpose |
|---|---|
| `RUST_SREC_OUTPUT_ROOTS` | Comma-separated list of absolute paths to treat as output-root boundaries for the write gate. If unset, the gate derives one root from `OUTPUT_DIR` with a 2-component heuristic. |
| `RUST_SREC_LOCALE` | Backend locale for notification strings. Affects every notification event (stream, download, segment, pipeline, system, credential). Supported: `en`, `zh-CN`. Defaults to `en`. |

See the [configuration doc](../getting-started/configuration.md#backend-service) for details.

## Notable refactors

The gate work included several supporting refactors that improve the downloader subsystem beyond just #508:

- **`ensure_output_dir` hoisted out of engines.** Previously each engine (`ffmpeg`, `streamlink`) called `ensure_output_dir` inside its own `start()`, with duplicate error-wrapping logic. The call now lives in a single `DownloadManager::prepare_output_dir` pre-start hook, which is also where the write gate is consulted. Mesio and future engines get this for free.

- **Fixed pre-existing `EngineStartError::from(crate::Error)` bug.** The old impl classified every I/O failure as `DownloadFailureKind::Other`, losing the `std::io::ErrorKind`. The new impl walks the error source chain, locates the first `std::io::Error`, and classifies based on its kind — so retry decisions and the circuit breaker now see the correct failure category for all I/O paths.

- **Renamed `set_circuit_breaker_blocked` → `set_infra_blocked(reason)`** in `monitor/service.rs`. The new signature takes an `InfraBlockReason` enum with variants for both circuit-breaker blocks (existing behavior) and output-root gate blocks (new). Both go through the same persistence path so the audit trail stays in one place. This is a public API rename; no deprecated alias is kept.

- **Extended `reset_errors`** (doc clarification only — the actual reset path was already correct via `StreamerManager::clear_error_state`).

- **`DownloadManager.output_root_gate` field uses `OnceLock`** for lock-free reads after a one-shot late-bind write at container init time. Necessary because the services container constructs `NotificationService` after `DownloadManager` in one of its two builders.

## Compatibility

- No database migrations.
- No breaking frontend API changes. `GET /sessions` default behavior changed: ended sessions with `total_size_bytes=0` are no longer returned; pass `?include_empty=true` to restore the previous "return all" behavior. `GET /sessions/:id` is unaffected.
- `set_circuit_breaker_blocked` was renamed to `set_infra_blocked(reason)` — external callers of the monitor service (none known) would need to update.
- The `DownloadManagerEvent::DownloadRejected` event now carries a new `kind: DownloadRejectedKind` field. External subscribers of the event stream (via the WebSocket or broadcast API) should expect this field to appear in JSON payloads; ignoring it is safe.
- `DownloadEndPolicy` gains a `Completed` variant (engine clean end, platform status ambiguous). The original `StreamerOffline | Stopped(_)` arm is preserved and continues handling authoritative offline. Non-exhaustive `match` against `handle_download_ended` callers is unaffected; exhaustive matches need a new arm.
- `SessionTransition::Started` gains a `download_start: Option<Box<DownloadStartPayload>>` field. Existing matchers using the rest pattern (`Started { .. }`) need no change; exhaustive struct literals need `download_start: None`. `SessionTransition` no longer derives `PartialEq`/`Eq` — `StreamInfo` doesn't implement `Eq`, but in-tree usage is `matches!`-based.
- `SessionFilters` gains an `include_empty: Option<bool>` field, defaulting to `None` (empty sessions hidden). All internal call sites updated.

## Notable refactors (session lifecycle)

- `SessionLifecycle::on_live_detected` / `resume_from_hysteresis` / `enter_ended_state` concurrent protocol uses `self.hysteresis.remove(session_id)` as a single atomic CAS point. `resume_from_hysteresis` now returns `Option<StartSessionOutcome>`; `None` indicates a lost CAS and the caller falls through to `start_or_resume`. `enter_ended_state` bails out on `was_in_hysteresis=true && claim=None` snapshot inconsistency — no DB write, no in-memory update, no broadcast. Together they guarantee at most one of {`Resumed + Started{from_hysteresis: true}`, `Ended`} broadcasts fires for any single Hysteresis exit.

- `OfflineClassifier`'s window and threshold moved from module-private `const`s to a `from_scheduler(count, interval_ms)` constructor, sharing the source of truth with `HysteresisConfig::from_scheduler`. `OfflineClassifier::new()` survives (legacy `60s / threshold 2` defaults) for test fixtures only; production sites in `services::container` migrated to `from_scheduler`.

- `OfflineSignal::PlaylistGone(u16)` variant deleted — the `session_events.payload` audit log had no historical rows using it (verified clean slate before merge). Frontend `OfflineSignalSchema` doc updated.

- `crates/pipeline-common::SplitReason` gains an `EndOfStream` variant, emitted by the mesio HLS playlist engine on `#EXT-X-ENDLIST` observation, propagated transparently through the hls-fix pipeline (`segment_split` / `segment_limiter` / `defragment` / `analyzer` all verified to preserve the reason), and observed by rust-srec's `consume_stream` to drive `EngineEndSignal::HlsEndlist`.

- New `services::session_janitor` — background periodic GC for `live_sessions` rows where `total_size_bytes=0 AND end_time<retention_cutoff`. Spawn site is `ServiceContainer::start()`, alongside the lifecycle subscribers. Defaults: `retention=5min`, `interval=30min`, `MIN_RETENTION=60s` (production floor).

## Notes

- **The stale-mount case is not auto-recoverable from inside the container.** Re-binding a Docker mount requires `CAP_SYS_ADMIN` in the host's mount namespace, which an unprivileged container does not have. The gate detects the failure and tells the user to restart; automatic recovery is a deployment-side concern. The [Docker troubleshooting guide](../getting-started/docker.md#freeing-up-disk-space-when-using-bind-mounts) documents the safe cleanup paths that avoid creating a stale mount in the first place.
