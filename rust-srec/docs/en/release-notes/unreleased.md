# Release Notes

## `unreleased`

This update covers two independent themes: (1) **recording session reliability** — fixing silent recording stops on brief disconnects, accumulated 0-byte session cards on the dashboard, and confusing labels on the session timeline; (2) the **output-root write gate** — fixing a class of failures where rust-srec could not recover from a filesystem issue (disk full, stale Docker bind mount) without a container restart. It also ships the initial scaffolding for backend localization.

## Recording session reliability

Several connected issues that ladder up to "brief disconnects shouldn't stop recording, and empty sessions shouldn't appear on the dashboard."

- **Recording no longer silently stops after a brief disconnect**

  When the upstream CDN rotates, a network blip happens, or the streamer briefly reconnects, the active download ends cleanly. Previously the session would still appear "live" in the system, but the actual download had stopped — meaning the streamer kept broadcasting while rust-srec only captured the segment up to the disconnect. Observed gaps reached over an hour in a single broadcast.

  Now the disconnect enters a short waiting window (default tracks the "offline detection" config, around one minute). If LIVE is detected again within the window, the download automatically restarts and reuses the same session — recording continues seamlessly. If no LIVE is observed by the window's end, the session ends.

- **Transient HTTP 404s no longer count as "streamer offline"**

  When a stream just resumed, platforms like Douyu hand out a freshly-signed URL whose token takes a few seconds to propagate to CDN edge nodes — requests during that gap return 404. HLS streams have similar transient 404 cases (sliding-window eviction, signed-URL expiry, edge desync).

  Previously rust-srec treated any 404 as authoritative "the streamer really went offline" and ended the session immediately. Even though the platform monitor still reported LIVE, the next detection would create another empty session — typically producing 3 zero-byte cards at the start of a single broadcast.

  404 alone no longer drives the offline decision. True offline now flows through two more precise signals: consecutive network failures (count and window come from the "offline detection" config, sharing the same parameters as hysteresis), or HLS's `#EXT-X-ENDLIST` tag (the platform itself signaling the stream has ended).

- **HLS streams that end cleanly close their session immediately**

  When an HLS playlist carries `#EXT-X-ENDLIST` (the platform explicitly marks the stream as ended), the session now ends **immediately**. Previously it had to wait through the full ~90-second hysteresis window — delaying the start of post-processing (remux, upload, etc.).

- **Cleanup of zero-byte "ghost sessions"**

  Recording segments below the `min_segment_size_bytes` threshold are automatically discarded (avoiding meaningless few-second clips), but the corresponding session row used to stay in the database, showing as zero-byte cards on the dashboard. Two cleanup layers added:

  - **API filtering by default** — the sessions list endpoint now hides zero-byte ended sessions by default. Active (still-recording) sessions are always returned. If you need to inspect them for diagnostics, pass `?include_empty=true`, or look up by session ID directly.
  - **Periodic background cleanup** — empty session rows are automatically deleted from the database 5 minutes after end (default scan interval 30 minutes). Related danmu statistics, segments, and lifecycle events are removed alongside via `ON DELETE CASCADE`.

## Frontend

- **Session detail "Timeline" tab counter fixed** — the badge previously counted only title changes, ignoring session lifecycle events. It now sums both, matching the number of entries actually rendered in the tab body.

- **More accurate session timeline translations** in Simplified Chinese:

  - `原因：已完成` → `原因：下载断开` ("download disconnected", more accurate than "completed" for an ambiguous-end case)
  - `通过备份计时器确认。` → `等待恢复超时后确认。` ("confirmed after wait-for-resume timed out", clearer than "via backup timer")
  - New translations for `主播离线` (Streamer Offline), `连续失败` (Consecutive Failures), `弹幕流已关闭` (Danmu Stream Closed), used in session-end cause displays.

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
- `GET /sessions` default behavior changed: zero-byte ended sessions are no longer returned (the "ghost cards" on the dashboard disappear by default). Pass `?include_empty=true` to see all records. `GET /sessions/:id` is unaffected.
- `set_circuit_breaker_blocked` was renamed to `set_infra_blocked(reason)` — external callers of the monitor service (none known) would need to update.
- The `DownloadManagerEvent::DownloadRejected` event now carries a new `kind: DownloadRejectedKind` field. External subscribers of the event stream (via the WebSocket or broadcast API) should expect this field to appear in JSON payloads; ignoring it is safe.

## Notes

- **The stale-mount case is not auto-recoverable from inside the container.** Re-binding a Docker mount requires `CAP_SYS_ADMIN` in the host's mount namespace, which an unprivileged container does not have. The gate detects the failure and tells the user to restart; automatic recovery is a deployment-side concern. The [Docker troubleshooting guide](../getting-started/docker.md#freeing-up-disk-space-when-using-bind-mounts) documents the safe cleanup paths that avoid creating a stale mount in the first place.
