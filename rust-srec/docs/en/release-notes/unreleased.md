# Release Notes

## `unreleased`

This release adds multiple accounts per platform, saved proxies, API keys and MCP access, Baidu Netdisk uploads, per-step workflow retries, and configurable danmu statistics. It also fixes recording shutdown, restart recovery, file handling, and credential exposure in logs.

## Before upgrading

- **Cookies move to account profiles:** On first start, cookies, tokens and logins saved in platform, template and streamer settings become account profiles on their platform, named after where they came from with "(migrated)", and each place selects the account it used before. Identical accounts are merged, and a cookie field left empty now inherits instead of turning login off. Twitch OAuth tokens, Douyin TTWID, Douyu device IDs and SOOP logins move into these profiles too. A cookie saved on the Streamlink platform, which covers many different sites, is instead selected by each of its streamers, since Streamlink accounts are chosen per streamer. Older backups are converted the same way when imported. Take a backup before upgrading; going back to an earlier version needs it. See [upgrading from configuration cookies](../concepts/configuration.md#upgrading-from-configuration-cookies).
- **Proxies become saved proxies:** On first start, the proxy set in global, platform, template and streamer settings becomes a saved proxy under **Settings → Proxies**, named after its address, and each of those settings chooses it; identical proxies are merged. A proxy that was turned off becomes **Direct**, and **Use System Proxy** becomes **System proxy**. If the global proxy was turned off and `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY` is set when the new version starts, the global setting becomes **System proxy** instead, so FFmpeg and Streamlink downloads keep going through it; live checks, Mesio downloads and danmu now use it too. Older backups are converted the same way when imported. Take a backup before upgrading; going back to an earlier version needs it. See [upgrading from proxy settings](../concepts/configuration.md#upgrading-proxy-settings).
- **Direct no longer uses environment proxies:** With **Direct**, FFmpeg and Streamlink now ignore `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` too, so a platform, template or streamer whose proxy was turned off no longer sends those downloads through them. Choose **System proxy** where they should still apply. See [choosing how to connect](../concepts/configuration.md#choosing-a-proxy).
- **Accounts in the API:** Platform and template settings no longer take `cookies`; clients add accounts under `/api/credentials` and choose them with `credential_selection`. Requests that still send `cookies` are rejected so an account is never dropped silently. See [credential selection JSON](../reference/configuration-overrides.md#credential-selection-json).
- **Proxy settings in the API:** Clients choose a proxy with `proxy_route` instead of `proxy_config`; requests that still send `proxy_config` are rejected so a proxy is never dropped silently. See [proxy route JSON](../reference/configuration-overrides.md#proxy-route-json).
- **Mesio Loop Protection:** Loop Protection and Offset Consistency Check are now off by default. Engines saved with an earlier version keep their settings, so these options may still be on; turn them off in **FLV Tuning** if you do not need them. See [Loop Protection](../concepts/engines.md#loop-protection).
- **Sign-in sessions:** Existing access tokens need a refresh with a valid refresh token or a new sign-in. Logout, password changes, and successful configuration imports now revoke the affected login sessions. API clients must serialize token refreshes; reusing a consumed refresh token can revoke all sessions for that user. See [session security](../operations/security.md#revocable-login-sessions).
- **Previously exposed credentials:** New logs redact cookies, tokens, and passwords. Existing logs are not rewritten. Rotate credentials that appeared in logs you shared.
- **Filter timezones:** New filters default to UTC. Migration and legacy-backup import preserve existing TimeBased local schedules. Editors now expose UTC, server-local, and IANA timezone choices. See [timezone compatibility](../operations/upgrading.md#filter-timezone-compatibility).
- **Database migrations:** Preset and template timestamps are normalized to Unix milliseconds, and new indexes improve workflow and media-output listing. Invalid historical timestamps stop the migration without changing the data; follow the [repair guidance](../operations/upgrading.md#preset-and-template-timestamps). Index creation scans the affected tables.
- **Execute templates:** Unsupported uses of placeholders now fail before launch. Use the new `program`/`args` mode or a fixed script where shell templates are unsuitable. Windows program mode rejects batch files. See [supported syntax](../reference/processors.md#execute-execute).
- **Source builds:** The frontend and documentation now require Node.js 26. Docker and pre-built installations are unaffected.
- **Removed settings:** `danmu_sampling_config` has been removed from the API and database; it never affected statistics. Older exports still import. Stored `EmailConfig.batch_window_secs` values remain ignored, and the field has been removed from the Rust interface.
- **Rust integrations:** Several unused backend, scheduler, downloader, and metrics interfaces were removed. Use the canonical `domain::StreamerState` and supported subsystem snapshots. See [backend interface changes](../development/architecture.md#backend-rust-interfaces), [scheduler changes](../development/architecture.md#scheduler-state-and-backoff), and [downloader changes](../development/architecture.md#downloader-rust-interfaces).

## New features

- **Multiple accounts per platform:** Save several accounts for a platform and choose, for the platform, a template or a streamer, one fixed account or a pool that rotates between accounts or falls back to the next one when an account's login fails. A recording keeps the same account from start to finish. Accounts are managed on the platform page, including Bilibili QR login; the page shows what uses each account and when it was last used. Each account can also choose its own proxy, used for everything done with it from sign-in to downloads. When no account works, the streamer's card says so instead of showing it offline, the top bar lists accounts that need a new login, and the recording starts again as soon as an account is edited or logged in again. See [account profiles](../concepts/configuration.md#account-profiles-and-selection) and [account proxy settings](../concepts/configuration.md#account-proxies).
- **Saved proxies:** Save proxies once under **Settings → Proxies**, with an optional login, and test that they reach a platform. Global settings, each platform, template, streamer and account then choose whether to connect directly, through the system proxy, or through one of them. The list shows where each proxy is used, and a proxy still in use cannot be deleted. When a platform rate-limits one proxy, only requests through that proxy pause, and an account pool moves on to an account that connects another way. See [Proxies](../concepts/configuration.md#proxies).
- **Douyu QR login:** Douyu accounts can be added by scanning a QR code with the Douyu app, and stay signed in: the session is renewed automatically every few days. A signed-in account is sent with stream requests and can unlock the original quality in rooms that hold anonymous viewers to lower qualities. See [Douyu authentication](../platforms/douyu.md#authentication).
- **API keys and MCP:** Added expiring or non-expiring API keys with `read_only` and `full` access, plus a built-in MCP server at `/api/mcp` for recording queries, danmu analysis, and configuration management. **Settings → API Keys** supports creation, revocation, and MCP client configuration. Keys are shown once and stored as hashes; read-only keys cannot retrieve credentials or configuration. See [API Keys & MCP](../api/api-keys-mcp.md).
- **Baidu Netdisk uploads:** Added the `baidupcs` processor and bundled BaiduPCS-Go in Docker. The preset editor supports login, quota display, and optional automatic re-login. Uploads support destination templates, skip/overwrite policies, per-file results, and retries of unconfirmed files. Remembered credentials are stored server-side; logout removes them. See [Baidu Netdisk](../reference/processors.md#baidu-netdisk-baidupcs).
- **Workflow retries and timeouts:** Steps can set `retry.max_attempts`, `retry.backoff_secs`, and `timeout_secs` in the editor or definition. Scheduled retries survive restarts and keep dependent steps waiting; cancelling the workflow cancels pending retries. See [per-step settings](../reference/workflows.md#per-step-retries-and-timeouts).
- **Execute programs directly:** Added `program` and `args` alongside shell-command mode. Arguments preserve literal spaces, quotes, empty values, and substituted data. Both workflow editors support the new mode and retain output-scanning settings when switching modes.
- **Streamlink FFmpeg selection:** Added `ffmpeg_path` to engine settings and template overrides. Choose a custom executable, inherit the engine setting, or use `FFMPEG_PATH`/`ffmpeg`. Paths with spaces are preserved. See [engine configuration](../concepts/engines.md#streamlink-ffmpeg-executable).
- **Batch pipeline actions:** Added selection and batch cancel, retry, and delete actions to **Pipeline Jobs**. Failed items remain selected for another attempt.
- **Media deletion:** Added single and batch deletion to **Media Outputs**. Entries are removed by default; **Also delete files from disk** removes the recordings too. Session sizes update accordingly.
- **Storage display:** System Health now shows free space and usage for recording disks, including configured overrides. The dashboard shows free space on the fullest disk.
- **Upload status in the header:** The top bar now shows running and queued uploads on every page, with overall progress. Open it to see each upload's streamer, progress, speed, remaining time, and file count, to cancel an upload, or to go to its job. Uploads that fail stay listed with their error until you dismiss them.

## Recording and recovery

- AV1 recordings split by size or duration now start with the headers needed for playback, and repeated stream headers no longer disturb timestamps. E-AC-3 streams are now labelled with the standard codec name in file metadata.
- Loop Protection now removes only packets that exactly match earlier ones. It keeps copies of recent packets, up to 16 MiB per recording.
- Fixed Mesio HLS recordings of fMP4 streams that switch initialization segments, for example around inserted ads. Video after a switch back is no longer written with the wrong initialization data, an ad's initialization data no longer leaks into the main recording, and a recording no longer starts without initialization data after a retried download.
- Fixed Mesio HLS recordings failing on every segment when the playlist redirects to another server. Segment, key, and initialization addresses now resolve against the redirected location.
- Fixed Mesio HLS recordings ending when a CDN briefly served an empty or outdated playlist. Segments that are still waiting to download or retry when they drop out of the live playlist are now kept, up to one playlist's worth, rather than lost.
- Mesio HLS segment and key retry settings now take effect. Key downloads are retried on network and server errors, and the segment retry count and delays control how often a failed segment is retried before it is skipped.
- Fixed Mesio HLS variant selection: the lowest- and closest-bitrate choices no longer pick keyframe-only playlists, and the audio-only and video-only choices now select by the streams a variant actually contains.
- Fixed Twitch ad detection in Mesio logging a warning on every playlist refresh and, on long streams, missing newly announced ads.
- Twitch accounts are now checked: a token that Twitch no longer accepts, for example after signing out or changing the password, marks the account invalid and moves on to the next account instead of failing every check with an unrelated error. A Twitch account can now also use the `auth-token` browser cookie, and tokens copied with the `oauth:` prefix work.
- Live checks that go through Streamlink, for sites without a built-in extractor or when Streamlink is chosen as the extractor, now use your proxy settings instead of connecting directly.
- Danmu now follows your proxy settings, including an account's own, instead of always connecting directly. With **System proxy** it uses the server's proxy environment variables and connects directly to hosts in `NO_PROXY`.
- Improved Mesio FLV recovery from corrupted data: after a damaged section, recording resumes at the next genuine packet instead of merging following packets into one invalid packet.
- Fixed Mesio FLV recordings that stopped slowly when the server stalled before sending data, reported an error page as a clean disconnect, or rejected valid streams whose first bytes arrived in very small pieces. A connection that breaks mid-stream is no longer reported as a completed download.
- Fixed Mesio HLS recordings losing large segments when several downloads were in progress at once, failing single-file streams whose server ignores byte-range requests, and mislabelling MPEG-TS streams that declare an initialization section.
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
- Time-based filters must now include at least one day and different start and end times. Filters saved without these never matched, so the streamer was silently never recorded; edit them to choose days and a real time range.
- Fixed invalid global and platform values being saved. Validation checks types, negative values, and overflow while preserving supported zero/default meanings. Platform edits cannot change the canonical name used for URL lookup.
- Fixed the global settings page warning about unsaved changes right after a successful save. After saving, the page shows the settings as the server stored them.
- Template platform overrides now offer only the settings that take effect there: pipelines and platform options. Other settings entered in a template's platform override were never applied; set them on the template, the platform, or the streamer instead.
- Fixed proxy usernames and passwords containing percent signs, spaces, Unicode, or URL delimiters, with every download engine. Download diagnostics omit proxy URLs, and Mesio logs no longer show proxy credentials. Mesio logs and error messages also mask the tokens in stream addresses.
- Fixed account-email conflicts during backup imports. Email swaps and reuse of released addresses work regardless of input order; conflicts are rejected before configuration changes.
- Fixed concurrent database updates losing results or double-counting media deletion. Failed updates roll back their associated changes.
- Fixed fresh standalone databases to initialize recordings from `OUTPUT_DIR`, or an absolute `./output` fallback. Existing saved folders remain unchanged. Startup output probes now use the same directory grouping as recording attempts. See [output-root probes](../operations/storage.md#output-root-probes).

## Notifications

- Added notifications when a platform starts limiting requests, naming the paused connection (direct, the system proxy or a saved proxy) and the time checks resume, and when a newly logged-in session could not be saved to its account. A platform's request to wait is now limited to 15 minutes, so one response can no longer pause recording for hours.
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
- Fixed notification subscription changes being lost on window focus changes. QR code logins stop checking for a scan once the code expires or the dialog is closed.
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
- The navigation menu on phones and narrow windows now opens and closes instantly with a smooth slide, even on long pages such as the streamer list. Closing it returns keyboard focus to the menu button.
- Collapsing or expanding the sidebar on wider screens is now one smooth motion: icons stay in place instead of jumping sideways, and labels fade with the edge. Hovering a menu entry now enlarges only that entry's icon rather than every icon in the sidebar.
- Fixed text looking blurry on some browsers while hovering a settings card. Settings, health, and pipeline graph cards no longer grow when hovered; they highlight instead, so fields and workflow connections stay in place.
- Fixed the **Pipeline Jobs** summary cards not matching the list below them. The cards, the status filters, and the dashboard now all count pipelines, and a pipeline shows as Pending while it waits for a free worker, so the Pending filter lists those pipelines. Average duration now covers whole pipelines, including time spent waiting. The search box now filters pipelines by name, streamer, session, or ID.

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
- The web container now runs its application server as an ordinary account instead of `root` and sends `X-Content-Type-Options` and `Referrer-Policy` headers on every response; if your reverse proxy already adds these, keep them in one place. Self-built images no longer include a local database file left in the web source folder.
- Fixed the NVIDIA GPU sometimes missing from System Health. The GPU compose file now requests the `compute` and `utility` capabilities that the GPU status check needs; existing installs should download the updated `docker-compose.gpu.yml` again. A GPU that is slow to respond at startup is now shown with its error instead of being left out. See [NVIDIA GPU](../getting-started/docker.md#nvidia-gpu).
- Added opt-in Watchtower updates through `docker compose --profile autoupdate up -d`, using the new unauthenticated `/api/health/idle` check. Mutable image tags are required. A recording started after the idle check can still be interrupted; keep backups for automatic upgrades. See [automatic updates](../operations/upgrading.md#automatic-updates-watchtower).
- The installer now selects English or Chinese from the system locale or `SREC_LANG`, checks downloaded content, and stops if secure secret generation fails.
- Fixed the bundled systemd unit's permissions, state/log directories, environment loading, and shutdown wait. Fresh databases use `/var/lib/rust-srec/output`; existing databases keep their saved folder. Recordings are readable by the service account and group. See [systemd installation](../getting-started/installation.md#systemd-service-linux).
- Desktop quit actions now finalize recordings, including quits during startup and macOS menu/Dock quits, with a one-minute limit. Dock/logout handling may leave the window unresponsive during cleanup.
- Linux and macOS tray minimization now uses window events instead of continuous polling.
- Prevented desktop and standalone instances from using the same recording database concurrently. Fixed SQLite locking on first desktop launch.
- Added a desktop startup recovery screen for locked databases, permission errors, full storage, and corrupt databases, with diagnostic copying and shortcuts to data and log folders.
