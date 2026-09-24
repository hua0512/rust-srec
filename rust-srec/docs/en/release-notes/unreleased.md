# Release Notes

## `unreleased`

This release adds API keys and MCP access, Baidu Netdisk uploads, per-step workflow retries, and configurable danmu statistics. It also fixes recording shutdown, restart recovery, file handling, and credential exposure in logs.

## Before upgrading

- **Sign-in sessions:** Existing access tokens need a refresh with a valid refresh token or a new sign-in. Logout, password changes, and successful configuration imports now revoke the affected login sessions. API clients must serialize token refreshes; reusing a consumed refresh token can revoke all sessions for that user. See [session security](../operations/security.md#revocable-login-sessions).
- **Previously exposed credentials:** New logs redact cookies, tokens, and passwords. Existing logs are not rewritten. Rotate credentials that appeared in logs you shared.
- **Filter timezones:** New filters default to UTC. Migration and legacy-backup import preserve existing TimeBased local schedules. Editors now expose UTC, server-local, and IANA timezone choices. See [timezone compatibility](../operations/upgrading.md#filter-timezone-compatibility).
- **Database migrations:** Preset and template timestamps are normalized to Unix milliseconds, and new indexes improve workflow and media-output listing. Invalid historical timestamps stop the migration without changing the data; follow the [repair guidance](../operations/upgrading.md#preset-and-template-timestamps). Index creation scans the affected tables.
- **Execute templates:** Unsupported uses of placeholders now fail before launch. Use the new `program`/`args` mode or a fixed script where shell templates are unsuitable. Windows program mode rejects batch files. See [supported syntax](../reference/processors.md#execute-execute).
- **Source builds:** The frontend and documentation now require Node.js 26. Docker and pre-built installations are unaffected.
- **Removed settings:** `danmu_sampling_config` has been removed from the API and database; it never affected statistics. Older exports still import. Stored `EmailConfig.batch_window_secs` values remain ignored, and the field has been removed from the Rust interface.
- **Rust integrations:** Several unused backend, scheduler, downloader, and metrics interfaces were removed. Use the canonical `domain::StreamerState` and supported subsystem snapshots. See [backend interface changes](../development/architecture.md#backend-rust-interfaces), [scheduler changes](../development/architecture.md#scheduler-state-and-backoff), and [downloader changes](../development/architecture.md#downloader-rust-interfaces).

## New features

- **API keys and MCP:** Added expiring or non-expiring API keys with `read_only` and `full` access, plus a built-in MCP server at `/api/mcp` for recording queries, danmu analysis, and configuration management. **Settings → API Keys** supports creation, revocation, and MCP client configuration. Keys are shown once and stored as hashes; read-only keys cannot retrieve credentials or configuration. See [API Keys & MCP](../api/api-keys-mcp.md).
- **Baidu Netdisk uploads:** Added the `baidupcs` processor and bundled BaiduPCS-Go in Docker. The preset editor supports login, quota display, and optional automatic re-login. Uploads support destination templates, skip/overwrite policies, per-file results, and retries of unconfirmed files. Remembered credentials are stored server-side; logout removes them. See [Baidu Netdisk](../reference/processors.md#baidu-netdisk-baidupcs).
- **Workflow retries and timeouts:** Steps can set `retry.max_attempts`, `retry.backoff_secs`, and `timeout_secs` in the editor or definition. Scheduled retries survive restarts and keep dependent steps waiting; cancelling the workflow cancels pending retries. See [per-step settings](../reference/workflows.md#per-step-retries-and-timeouts).
- **Execute programs directly:** Added `program` and `args` alongside shell-command mode. Arguments preserve literal spaces, quotes, empty values, and substituted data. Both workflow editors support the new mode and retain output-scanning settings when switching modes.
- **Streamlink FFmpeg selection:** Added `ffmpeg_path` to engine settings and template overrides. Choose a custom executable, inherit the engine setting, or use `FFMPEG_PATH`/`ffmpeg`. Paths with spaces are preserved. See [engine configuration](../concepts/engines.md#streamlink-ffmpeg-executable).
- **Batch pipeline actions:** Added selection and batch cancel, retry, and delete actions to **Pipeline Jobs**. Failed items remain selected for another attempt.
- **Media deletion:** Added single and batch deletion to **Media Outputs**. Entries are removed by default; **Also delete files from disk** removes the recordings too. Session sizes update accordingly.
- **Storage display:** System Health now shows free space and usage for recording disks, including configured overrides. The dashboard shows free space on the fullest disk.

## Recording and recovery

- Fixed shutdown ordering so recording tools can finalize files, save final segments, and close chat files before exit. Child processes are stopped with the application, including on macOS when the parent process is already exiting.
- Added a standalone shutdown deadline of 30 seconds by default. Configure `RUST_SREC_SHUTDOWN_TIMEOUT_SECS` and keep Docker's stop grace period longer. Forced or incomplete shutdowns are reported at the next startup; recovery warnings clear only after confirmed recovery. See [shutdown settings](../reference/environment.md#shutdown).
- Added cooperative stopping for supported Streamlink 8.5.0 readers so buffered data can reach FFmpeg before finalization. Unsupported readers and forced stops report incomplete draining. See [supported readers and limits](../concepts/engines.md#stopping-streamlink-recordings).
- Fixed final-segment reporting in FFmpeg and Streamlink. Segments are completed only after confirmed process cleanup, and rotated segments start with fresh byte accounting. Stopping Mesio before its first HLS segment no longer reports an engine failure.
- Fixed unfinished sessions after restart: a live broadcast resumes its existing session, while a confirmed offline check closes it and makes final post-processing recoverable.
- Fixed duplicate session completion, overwritten end times, and late events stopping a newer session. Shutdown and disabling no longer publish false offline observations.
- Fixed recording termination after a segment database write failed. Recording continues while the save failure is reported.
- Fixed output-error classification so storage failures are retained even when the stream also disconnects or FFmpeg exits successfully. Disk-full errors trigger storage alerts and retry throttling rather than repeated per-streamer failures.
- Fixed streamer deletion to preserve recording history and wait for all active recordings and post-processing. Deletion continues after a restart. The streamer's URL stays reserved until cleanup finishes; scripts that recreate it must retry later.
- Fixed configuration imports interrupting live recordings or producing duplicate live notifications. New streamers begin monitoring immediately, and rejected imports leave running work unchanged. Replace imports temporarily retain templates and presets needed by finishing recordings and report a warning.
- Fixed monitoring stopping after recoverable internal failures and duplicate or missing monitors after rapid disable/re-enable actions. Automatic recovery stops after ten consecutive crashes; terminal conditions such as streamer removal are not restarted. See [restart limits](../operations/monitoring.md#scheduler-restart-limits).
- Fixed lost start/stop feedback under scheduler load and stale updates overriding current recording state. Offline detection uses resolved global, platform, template, and streamer settings. Recurring checks use ±10% timing variation to spread load, and error backoff remains capped at one hour even for large counts.
- Fixed per-streamer login credentials being lost after edits or bulk actions. Shared platform credential refreshes are serialized, and waiting recordings receive the updated cookies.
- Fixed Unicode output paths in FFmpeg segment recording on Windows. Percent signs in titles, names, and concrete directories remain literal; configured date placeholders still expand.
- Engine tests now use bounded asynchronous version checks and report malformed settings instead of silently applying defaults. Mesio diagnostics show the linked library version. FFmpeg progress tracking remains enabled when custom options request quiet logs.

## Workflows and file processing

- Added save-time validation for missing presets, unknown processors, dependency cycles, and steps that delete or move files while another step still reads them. Option-based source deletion produces a warning. See [workflow validation](../concepts/pipeline.md#automatic-cleanup).
- Fixed workflow failure handling to cancel only dependent steps. Independent branches can finish, and retry restarts only failed or cancelled steps. Jobs belonging to a workflow direct users to retry that workflow.
- Fixed retries leaving workflows stuck, losing successful results, or treating partially processed batches as successful. Failed files are identified, produced files are retained, and retry history preserves logs, timings, sizes, and earlier outputs.
- Fixed restart recovery to resume unfinished processing without repeating completed steps. Recovery also starts processing that was due but never created; it does not run new processing retroactively on recordings completed before this update.
- Fixed cancellation and deletion to stop running work and complete the workflow's cancelled state, including `DELETE /api/pipeline/{pipeline_id}`. Cancelled workflows remain retryable, and withdrawn jobs cannot restart.
- Fixed shutdown to finish accepted pipeline events and stop long-running jobs before the shutdown deadline. Short jobs can finish; interrupted jobs run again after restart. Delayed session cleanup no longer holds shutdown open.
- Changed queue ordering to prefer continuation steps over new workflows at the same priority, then the oldest job within each group. Higher priorities still take precedence. Lower CPU/I/O worker limits apply to new admissions while running jobs finish.
- Added timeout diagnostics with the configured limit, step, processor, current file, and last transfer progress. Restarted jobs clear stale progress. Job logs preserve repeated messages without duplicating already-saved entries.
- Fixed unknown processors leaving sessions stuck in processing. ZIP presets now resolve correctly, and existing jobs with unavailable processors fail at startup. Failed workflow creation releases temporary tracking, and invalid saved preset JSON is rejected before creating jobs.
- Fixed workflow output order to follow the workflow definition. Steps with no input files complete without processing; `execute` still runs its command. Recovery pairs danmu with stored video paths and warns when a unique match is unavailable.
- Replaced generated `_inputs.json` sidecar files with pairing data stored in the pipeline. Existing sidecars are unused and can be deleted. Subtitles stay paired with their own recording segment, including after files move; execute scripts can read `{manifest_json}`.
- Fixed media processors to stage outputs before publication, reject input/output path collisions, and roll back failed batches. Failed or cancelled processing preserves source files and existing destinations. Disabling overwrite also protects against concurrent publication. See [processor behavior](../development/pipeline.md#processor-result-contracts).
- Fixed source removal deleting the newly converted file when input and output resolve to the same path. Subtitle filters now accept apostrophes and filter delimiters in paths.
- Fixed cancelled or timed-out commands and audio probes leaving subprocesses running. Process cleanup includes descendants and bounded output collection.
- Fixed copy, move, and archive operations accepting duplicate destination names. First-attempt moves reject missing inputs; retries can recognize files moved by earlier attempts. Partial rclone and Baidu uploads retain each file's actual result.
- Fixed Baidu uploads retrying permanently rejected files and failing on long file lists. Rclone can upload inputs from unrelated folders. Upload logs omit account details and credential-bearing connection parameters.
- Added ZIP64 support for recordings larger than 4 GiB. ZIP entries with duplicate names are rejected before writing; use `preserve_paths` or rename inputs.
- Fixed thumbnails for recordings shorter than the requested timestamp by using the first frame. Recordings without a usable frame pass through. Audio extraction without re-encoding now uses an extension matching the codec.
- Fixed subtitle burn-in dropping unprocessed videos when passthrough is disabled. Batch jobs report combined file sizes, and FFmpeg progress timestamps consistently use milliseconds.
- Fixed execute output scanning to expand folder placeholders and include only newly created files modified after command start. Unreadable scan directories fail the step. Workflow execute steps do not receive an `{output}` path.
- Fixed workflow editors to preserve dependencies when steps are renamed and reject empty or duplicate IDs. Presets resolve by exact name; missing presets are reported instead of substituting an unrelated preset.
- Fixed workflow steps that use an alternate processor name, such as `transcode` or `upload`, showing no settings or label in the web interface.

## Danmu

- Added configurable statistics at global, platform, template, and streamer levels: ranking lengths, timeline resolution, tracking capacity, and extra stop words. Statistics can be disabled while retaining chat files. See [Danmu Statistics](../reference/settings.md#danmu-statistics).
- Added live statistics snapshots approximately once a minute and recovery of saved counts after restart. Long sessions reduce timeline resolution instead of discarding early activity.
- Added average message rate, unique-chatter estimates, chat/gift totals, gift-sender and gift rankings, and expandable Top Talkers. Gift charts appear only when the platform reports gifts; unique-chatter estimates typically have about 2% error.
- Fixed inflated frequent-word counts and added `≈` markers for estimates. Chinese and Japanese messages now use word segmentation rather than counting whole sentences as words.
- Fixed activity charts reporting ten-second counts as per-minute rates. Peaks and averages use the correct rate, averages include the whole session, and inactive periods display zero.
- Fixed chat recording stopping permanently after connection failures. Reconnection continues during recording, statistics persist, and extended outages appear in System Health.
- Fixed chat failures blocking post-processing. Interrupted chat files are finalized and registered; files for discarded video segments are removed from the session list.
- Fixed queued final messages being dropped at shutdown or assigned to the next segment. Concurrent stop requests and replacement collectors wait for the same completion, preventing overlapping collectors.
- Fixed invalid XML characters and header comments in newly written files. Existing files are not repaired. MCP pagination preserves complete UTF-8 characters and rejects invalid encoding or limits that cannot advance.

## Configuration

- Fixed Bilibili and Douyin forms saving overrides for untouched settings. Inherited settings remain inherited, and platform defaults are displayed correctly.
- Added timezone selection to both time-based filter editors. Matching and wakeups use consistent overnight and daylight-saving boundaries, including repeated hours. Saved timezones survive edits, and backup schema 0.1.8 exports explicit zones.
- Fixed invalid global and platform values being saved. Validation checks types, negative values, and overflow while preserving supported zero/default meanings. Platform edits cannot change the canonical name used for URL lookup.
- Fixed proxy usernames and passwords containing percent signs, spaces, Unicode, or URL delimiters. Download diagnostics omit proxy URLs.
- Fixed account-email conflicts during backup imports. Email swaps and reuse of released addresses work regardless of input order; conflicts are rejected before configuration changes.
- Fixed concurrent database updates losing results, double-counting media deletion, or overwriting template edits during credential refresh. Failed updates roll back their associated changes.
- Fixed fresh standalone databases to initialize recordings from `OUTPUT_DIR`, or an absolute `./output` fallback. Existing saved folders remain unchanged. Startup output probes now use the same directory grouping as recording attempts. See [output-root probes](../operations/storage.md#output-root-probes).

## Notifications

- Fixed missing pipeline started, completed, and failed notifications.
- Fixed event history previews showing only the event name for output-path, GPU, and Baidu Netdisk re-login alerts. They now show the affected path or error. Rejected downloads, invalid credentials, and shutdowns also show their reason.
- Email delivery now reuses SMTP connections. Channel reloads retain the original destination for already-admitted deliveries and preserve failure history for channels that remain loaded.
- Fixed queue capacity handling, cancellation of evicted retries, and retention of failed deliveries from configuration-defined channels. Successful channels are not sent duplicate notifications during retries.
- Fixed Web Push backoff persistence and payload-size checks. Full or unavailable queues drop new events at every priority, count the drops, and do not replay them. Event history and other channels remain independent. See [delivery limits](../concepts/notifications.md#delivery-behavior).
- Added bounded delivery timeouts and retry delays for external channels. Errors report failure categories or HTTP status without credential-bearing URLs or response bodies.
- Fixed Telegram formatting for special characters in names, titles, and errors, including Unicode-safe truncation. Unsupported formatting modes return a configuration error.
- Unified channel enable/priority checks and localized rendering within each delivery attempt. Failed channel reloads preserve the current configuration. See [notification interfaces](../development/notifications.md#backend-notification-interfaces).

## Authentication and API

- Added login limits per account and source address, with HTTP 429 and `Retry-After`. Password verification concurrency is bounded. Behind a reverse proxy, the source limit applies to the proxy's address. See [login throttling](../reference/environment.md#login-throttling).
- Fixed concurrent login requests clearing another attempt's failure count. Failed authentication conceals whether an account exists; disabled status is disclosed only after a correct password.
- Fixed refresh-token rotation to issue at most one replacement and preserve the original token on database failure. Logout, account disablement, and password changes prevent reuse of revoked sessions. Download/log WebSockets and log archive grants revalidate credentials.
- Restricted browser origins and Host headers when `AUTH_DISABLED=true`. Use `API_CORS_ORIGINS` for additional local origins; authenticated deployments retain their existing behavior.
- Resource creation and template/job-preset cloning now return HTTP 201. OpenAPI includes session segments, template cloning, and browser Web Push operations. Unavailable job progress is `null`, not zero.
- Added `download=1` to media content requests for attachment downloads. Media and stream routes prefer the Authorization header; query-token fallback applies only when it is absent. Download/log WebSockets, stream proxy, and logging routes require full API keys.
- Search now treats `%`, `_`, and backslashes literally across jobs, sessions, outputs, notifications, and presets. See [search behavior](../api/index.md#search-filters).
- Parse and session-delete batches now reject more than 100 items before processing. Login device descriptions are limited to 256 Unicode characters, and internal database or credential diagnostics are omitted from API errors.
- Fixed forwarded identifiers and URL values in web requests. Invalid identifiers are rejected, and reserved characters are encoded correctly.
- Fixed credential refresh to save cookies and tokens atomically. Missing or retiring owners reject refresh; failed writes preserve previous credentials.
- Removed credentials and private destinations from application logs, browser diagnostics, and notification debug output. BaiduPCS-Go login now uses protected temporary configuration and stdin instead of process arguments. See [credential handling](../operations/security.md).

## Web interface

- Recording downloads now stream to disk in both same-origin and cross-origin web deployments. The desktop action opens the existing recording in its folder.
- Fixed logout to clear cached account data and prevent the next user seeing the previous session. Expired sessions redirect to sign-in and return to the original page afterward. Temporary server failures during renewal retain the current session and retry.
- Fixed transfer-progress interruptions during token refresh. Dashboard processing counts now update after cancellation, and live recording counters update consistently.
- Reduced repeated page loads and polling, paused updates in background tabs, and limited job-log refreshes to new entries. Live logs render in batches and reconnect after session renewal.
- Reduced unnecessary redraws in configuration, workflow, import-summary, and schedule editors.
- Fixed numeric fields and Douyu CDN selection to stay empty when cleared and show the inherited/default value. Integer-only settings are validated in the form.
- Fixed editable list rows losing focus or values after deletion. Empty custom tags remain separate until named.
- Fixed the raw JSON editor reformatting on every keystroke. Incomplete text remains editable, with validation errors shown separately.
- Fixed notification subscription changes being lost on window focus changes. Bilibili QR login stops polling when the code expires.
- Fixed hidden selections remaining active after searches, filter changes, or pagination. Batch actions apply to the current selection.
- Fixed malformed stored settings preventing entire platform or template lists from loading. Invalid URL search/filter values are ignored individually.
- Added explicit empty/error states for unavailable danmu statistics and missing or inaccessible pipelines. Session timelines identify known events even when their details are unavailable.
- Restored notification/preset card colors and corrected theme swatches after preset or light/dark changes.
- Improved media cards to show file type, name, path, size, and session. Type filters and totals now cover the full library and follow the selected search.
- Added accessible names to icon buttons and keyboard support for schedule controls and backup import choices.
- Completed translations for affected dialogs, player controls, cards, counts, validation errors, dates, times, and durations.
- Added a sidebar account menu for API keys, account settings, password changes, and sign-out.
- Fixed log file download errors appearing untranslated or blank.
- The preset editor now offers **Reset to defaults** for every processor, and asks before replacing a configuration you have edited.
- The sidebar notification dot now animates in when a new critical event arrives and fades out once it has been seen.
- The streamer list now opens with its cards already in place, and the dashboard shows its system and pipeline summaries straight away, instead of placeholders that fill in a few seconds later. The web interface also downloads less when it first loads.
- Pages showing live recordings use much less processing power while downloads are running, so the interface stays responsive and laptops stay cooler with many recordings in progress.
- Fixed a brief flash of the wrong colours when opening the web interface behind Cloudflare with Rocket Loader enabled. Your light/dark mode and custom theme now appear from the first frame.

## Monitoring and maintenance

- Health checks now report degraded status when a component is unknown. Slow filesystem sampling retains stale/degraded values without blocking other probes or accumulating tasks. See [health monitoring](../operations/monitoring.md).
- Added application log limits of 16 MiB per file and 16 files by default, including coordinated rotation in shared directories. Retention runs at startup; oversized entries are marked as truncated. See [log retention](../operations/monitoring.md#logs).
- Log archives now stream as ZIP64 with bounded buffers, up to two concurrent downloads and 10,000 files per archive. Excess requests receive HTTP 429; read or compression failures abort the download.
- Log initialization reports errors instead of panicking. Redirected console logs omit ANSI colors, and live-log formatting is skipped without subscribers. Startup timings retain I/O, engine discovery, and overall measurements.
- SQLite pools now share a suggested 64 MiB private page-cache allowance: 56 MiB for readers and 8 MiB for the writer. This is not a hard process-memory limit. See [SQLite memory budgeting](../operations/monitoring.md#sqlite-memory-budget).
- Vacuum scheduling now checks actual recordings and pauses new starts only while admitted vacuum work runs. Slow filesystem preflight does not hold the recording-start lock.
- Batched related-data queries for job pages and exports, parallelized configuration refreshes with bounded concurrency, and cached parsed filter rules and short-lived configuration reads.
- Consolidated service lifecycle, repository writes, import persistence, and processor result handling. Removed unused dependency features, duplicate models, inactive helpers, and the unconnected Prometheus exporter. JSON health endpoints and Web Push counters remain available.

## Deployment and desktop

- HTTPS reverse proxies now propagate the scheme used to set secure sign-in cookies. `COOKIE_SECURE` still overrides automatic detection; plain-HTTP production deployments log a warning.
- The web container now runs its application server as an ordinary account instead of `root` and sends `X-Content-Type-Options`, `X-Frame-Options`, and `Referrer-Policy` headers on every response; if your reverse proxy already adds these, keep them in one place. Self-built images no longer include a local database file left in the web source folder.
- Added opt-in Watchtower updates through `docker compose --profile autoupdate up -d`, using the new unauthenticated `/api/health/idle` check. Mutable image tags are required. A recording started after the idle check can still be interrupted; keep backups for automatic upgrades. See [automatic updates](../operations/upgrading.md#automatic-updates-watchtower).
- The installer now selects English or Chinese from the system locale or `SREC_LANG`, checks downloaded content, and stops if secure secret generation fails.
- Fixed the bundled systemd unit's permissions, state/log directories, environment loading, and shutdown wait. Fresh databases use `/var/lib/rust-srec/output`; existing databases keep their saved folder. Recordings are readable by the service account and group. See [systemd installation](../getting-started/installation.md#systemd-service-linux).
- Desktop quit actions now finalize recordings, including quits during startup and macOS menu/Dock quits, with a one-minute limit. Dock/logout handling may leave the window unresponsive during cleanup.
- Linux and macOS tray minimization now uses window events instead of continuous polling.
- Prevented desktop and standalone instances from using the same recording database concurrently. Fixed SQLite locking on first desktop launch.
- Added a desktop startup recovery screen for locked databases, permission errors, full storage, and corrupt databases, with diagnostic copying and shortcuts to data and log folders.
