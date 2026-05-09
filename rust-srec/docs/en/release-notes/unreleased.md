# Release Notes

## `unreleased`

This update covers four independent themes: (1) **recording session reliability** — adding a quiet-period for brief network blips, cleaning up empty session cards, and improving the timeline display; (2) the **output-root write gate** — fixing a class of failures where rust-srec could not recover from a filesystem issue (disk full, stale Docker bind mount) without a container restart; (3) a **new check-history strip on the streamer details page** that gives you an at-a-glance view of every recent monitor poll, with a tooltip showing exactly which stream quality was picked; (4) **clearer behavior when the concurrent-download limit is hit** — a Queued badge on streamer cards, smarter ordering for high-priority streamers, and no more "everything froze" stalls when the limit saturates. It also ships the initial scaffolding for backend localization, adds first-class **bandwidth & throughput controls** to the rclone pipeline step so you can throttle uploads from the UI, and gives the Mesio CLI a cleaner way to run without creating a log file.

## Streamer check-history strip

- **At-a-glance view of recent monitor checks**

  The streamer details page now shows the last 60 monitor polls as a row of small colored bars — green when the streamer was live, gray for offline, amber when a filter (like a schedule rule) skipped the streamer, and red when something went wrong. You can tell at a glance whether the monitor has been running normally and where any recent hiccups happened.

- **Hover any bar to see exactly what happened**

  The tooltip shows the time of the check, how long it took, and the full **list of stream qualities** the platform offered, with the one rust-srec actually picked for recording highlighted with a check mark. Useful when troubleshooting "why didn't it pick the higher quality?" or confirming a particular bitrate / codec was selected. For polls that didn't go live (filtered, errored), the tooltip explains why.

- **Updates live as new checks happen**

  When the dashboard is open, new bars stream in as each poll completes — no need to refresh. The header shows a green pulsing **LIVE** indicator while the connection is active, and falls back to "Last check ⟨X⟩ seconds ago" when offline so you can tell whether the data on screen is fresh.

## Recording session reliability

- **Quiet period for brief disconnects: short blips no longer end the session**

  When the upstream CDN rotates, a network blip happens, or the streamer briefly reconnects, the active download ends cleanly. Previously every disconnect immediately ended the session — and the next LIVE detection would create a fresh session card, so a few back-to-back blips would stack multiple zero-byte cards on the dashboard.

  Now the disconnect enters a short waiting window (default tracks the "offline detection" config, around one minute). If LIVE is detected again within the window, **recording continues seamlessly** under the same session — no new card appears. If no LIVE is observed by the window's end, the session ends.

- **HTTP 404 is no longer treated as authoritative "streamer offline"**

  When a stream just resumed, platforms like Douyu hand out a freshly-signed URL whose token takes a few seconds to propagate to CDN edge nodes — requests during that gap return 404. HLS streams have similar transient 404 cases (sliding-window eviction, signed-URL expiry, edge desync).

  404 alone no longer drives the offline decision. True offline now flows through two more precise signals: consecutive network failures (count and window come from the "offline detection" config, sharing the same parameters as the quiet period), or HLS's `#EXT-X-ENDLIST` tag (the platform itself signaling the stream has ended).

- **HLS streams that end cleanly close their session immediately**

  When an HLS playlist carries `#EXT-X-ENDLIST` (the platform explicitly marks the stream as ended), the session now ends **immediately** — no quiet-period wait. Post-processing (remux, upload, etc.) starts sooner as a result.

- **Cleanup of zero-byte "ghost sessions"**

  Recording segments below the `min_segment_size_bytes` threshold are automatically discarded (avoiding meaningless few-second clips). Previously the corresponding session row stayed in the database, showing as zero-byte cards on the dashboard. Two cleanup layers added:

  - **API filtering by default** — the sessions list endpoint now hides zero-byte ended sessions by default. Active (still-recording) sessions are always returned. Pass `?include_empty=true` to inspect for diagnostics, or look up by session ID directly.
  - **Periodic background cleanup** — empty session rows are automatically deleted from the database 5 minutes after end (default scan interval 30 minutes). Related danmu statistics, segments, and lifecycle events are removed alongside.

## Concurrent downloads: visibility and smarter scheduling

When more streamers go live than your `max_concurrent_downloads` setting allows, rust-srec now tells you exactly which ones are waiting for a slot — and uses smarter rules to decide who gets the next free slot.

- **New "Queued" badge on the streamer card**

  When a streamer is live but parked waiting for a free download slot, the card now shows an amber **Queued** badge (or a deeper rose tone for high-priority streamers) instead of staying on the red "Live" badge with no progress. Hover for a tooltip with "Concurrency limit reached" and a "waiting since X" timer, so you can tell at a glance which streamers are actually recording vs. queued.

  Refreshing the dashboard while streamers are queued keeps the badges visible — the queue state is part of the snapshot the dashboard receives on connect.

- **High-priority streamers actually take priority**

  Previously, when both the dedicated high-priority pool and the normal pool were full, a high-priority streamer would line up in plain first-come-first-served order behind whoever called earlier. Now when a slot frees, it goes to the highest-priority waiter — regardless of which pool freed it.

- **No more "everything froze" when the limit saturates**

  Before, hitting the concurrency limit could block the monitor loop until a slot freed. That meant a streamer going **offline** during a saturated period would stay stuck on "Live" until the limit cleared. Live, offline, and resume events for every streamer now keep flowing in real time, even when downloads are queued.

- **Danmu won't connect for streams that aren't recording yet**

  When a recording is parked in the queue, danmu (chat) collection waits too — it no longer opens a platform connection prematurely. This avoids burning through platform connection limits on streams that aren't actually being recorded.

- **Stale URLs after long waits are refreshed automatically**

  Stream URLs on some platforms (Douyin, Huya, etc.) include signed tokens that can expire within minutes. When a queued recording waits longer than the **Queued Refresh Threshold** (default 60 seconds, tunable from **Concurrency & Performance** in the global config — no restart needed), rust-srec automatically re-checks the streamer to get fresh URLs and headers before starting the engine. Set the threshold to 0 to refresh on every queue wait; set it higher to reduce platform requests when streams are typically stable.

- **Schedule windows are honored even after a wait**

  If a streamer's recording schedule closes while it was queued, the queued recording is cancelled cleanly instead of starting out-of-schedule the moment a slot opens.

- **Graceful shutdown won't start new recordings mid-shutdown**

  Previously, queued recordings could grab a slot the instant a finishing recording released one — meaning a fresh recording could spin up while the rest of the system was tearing down. Queued recordings are now rejected during shutdown so the system winds down cleanly.

- **Duplicate live events no longer spawn duplicate recordings**

  If the same streamer's live event is delivered twice in quick succession (e.g., a real live event racing with a hysteresis-resume re-emit), only one recording pipeline runs — the duplicate is recognized and skipped.

- **Health check tells you when the concurrency limit is the bottleneck**

  The `/health` endpoint's `download_manager` component now reports **Degraded** specifically when all slots are full *and* one or more streamers are queued waiting — i.e. your `max_concurrent_downloads` setting is actively holding things back. Saturated-but-no-waiters stays Healthy (full utilisation is normal operation, not a failure), so this signal won't fire on every prime-time peak. Useful for monitoring dashboards and alerting on under-provisioning.

## Rclone bandwidth & throughput controls

The rclone pipeline step now exposes rclone's bandwidth and concurrency knobs as dedicated form fields, so you no longer need to know rclone's CLI flag syntax to throttle an upload.

- **Cap upload bandwidth without leaving the UI**

  In the rclone step's **Advanced → Throughput** card, the new **Bandwidth Limit** field accepts simple values like `10M` (cap both directions at 10 MiB/s), asymmetric values like `10M:100k` (10 MiB/s up, 100 KiB/s down), or even a full timetable like `08:00,512k 23:00,off` for time-of-day shaping. Examples sit right under the input — no need to look up rclone's docs.

- **Tune concurrency and remote API rate limits**

  The same card adds dedicated inputs for **Transfers** (concurrent files), **Checkers** (concurrent integrity checks), **TPS Limit** / **TPS Burst** (transactions per second to the remote API — useful when a provider rate-limits you), **Multi-Thread Streams**, and **Multi-Thread Cutoff** (file size at which multi-thread copy kicks in). Empty means "use rclone's default", so you only set what you actually want to change.

- **Existing presets still work; "Extra Arguments" still wins**

  Older saved presets load unchanged. If you'd already added something like `--bwlimit 5M` to the **Extra Arguments** list, that keeps working — and continues to take precedence over the dedicated Throughput fields, so nothing you've configured silently changes behavior.

## Mesio CLI

- **Run Mesio without creating a log file**

  Mesio now supports `--disable-log-file` for one-off runs, scripts, and temporary folders where you only want messages on the console. When the option is used, Mesio does not create `mesio.log`.

- **Cleaner log output when redirecting commands**

  Mesio now avoids adding console colors when the output is being redirected, so saved logs stay easy to read in plain text.

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

- **Rclone processor switched to a typed `RcloneConfig` struct.** The processor previously parsed its config by poking at a generic `serde_json::Value`; it now uses a `#[derive(Deserialize)]` struct like every other processor in the crate. No behavior change — just removes a soup of `.get(…).and_then(…)` calls and gives the new throughput fields a self-documenting home.

## Compatibility

- Two new database migrations run automatically on startup (the check-history strip and the new queue-refresh threshold field on `global_config`). Nothing for you to do.
- `GET /sessions` default behavior changed: zero-byte ended sessions are no longer returned (the "ghost cards" on the dashboard disappear by default). Pass `?include_empty=true` to see all records. `GET /sessions/:id` is unaffected.
- `set_circuit_breaker_blocked` was renamed to `set_infra_blocked(reason)` — external callers of the monitor service (none known) would need to update.
- The `DownloadManagerEvent::DownloadRejected` event now carries a new `kind: DownloadRejectedKind` field. External subscribers of the event stream (via the WebSocket or broadcast API) should expect this field to appear in JSON payloads; ignoring it is safe.
- The download WebSocket stream emits two new event types: `DOWNLOAD_QUEUED` (a streamer is parked waiting for a slot) and `DOWNLOAD_DEQUEUED` (a queued attempt was cancelled before starting). The initial snapshot also includes a new `queued` array. External subscribers should expect these new variants; ignoring them is safe.

## Notes

- **The stale-mount case is not auto-recoverable from inside the container.** Re-binding a Docker mount requires `CAP_SYS_ADMIN` in the host's mount namespace, which an unprivileged container does not have. The gate detects the failure and tells the user to restart; automatic recovery is a deployment-side concern. The [Docker troubleshooting guide](../getting-started/docker.md#freeing-up-disk-space-when-using-bind-mounts) documents the safe cleanup paths that avoid creating a stale mount in the first place.
