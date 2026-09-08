# Release Notes

## `unreleased`

## Service Lifecycle Organization

- **Startup timings focus on initialization work**

  Container shutdown deadlines, output-root helpers, event decisions and existing lifecycle tests are separated by responsibility. Startup logs omit tiny synchronous construction timings while retaining I/O, engine discovery and overall measurements. Initialization order, the shared output-root snapshot and shutdown behavior are preserved; see [container responsibilities](../concepts/architecture.md#service-container-responsibilities).

## Build Dependencies

- **Compile only the system inventory and logging features in use**

  System inventory now enables CPU, memory and disk support without unused temperature-component, network-interface or user-account inventory features. Unused JSON log formatter features and direct protobuf well-known-type dependencies were removed; protobuf generation still retains its required dependencies. Backend dependency declarations share workspace versions without upgrading the locked packages.

## Backend Model Cleanup

- **One canonical streamer state and fewer unused Rust interfaces**

  Database models now use `domain::StreamerState`. Removed unused database batching, configuration coalescing, duplicate session entities, and inactive convenience methods. Rust integrations should follow the [backend interface notes](../concepts/architecture.md#backend-rust-interfaces). REST payloads, recording behavior, state-transition validation and runtime retirement are unchanged.

## Internal Metrics

- **Monitoring code reflects the available interfaces**

  Removed the unused Prometheus exporter and unwired download, pipeline, streamer and system counters, including their latent underflow and label-escaping paths. Internal web-push delivery counters and JSON health endpoints remain available. Rust callers of the removed metrics API must use the supported subsystem snapshots.

## Scheduler Recovery

- **Large error counts preserve the one-hour backoff cap**

  Error backoff bounds the exponent before arithmetic can overflow. Dormant actor state-file APIs and the unused streamer error writer are removed; runtime recovery, batch detection and terminal actor decisions are preserved. Rust integrations should follow the [scheduler interface notes](../concepts/architecture.md#scheduler-state-and-backoff).

- **Actor timing uses resolved overrides and recurring checks are spread out**

  Actor creation and metadata/template/platform/global updates now resolve offline-confirmation settings directly from the four-layer configuration instead of racing a separate metadata refresh. Recurring checks receive bounded ±10% jitter. Unrelated updates preserve pending deadlines; actual cadence changes can shorten them while keeping admission, cooldown, smart-wake and immediate-check constraints. See [scheduler timing](../concepts/architecture.md).

- **Terminal actor decisions no longer trigger crash restarts**

  A removed streamer or another non-recoverable message error now follows the same graceful-stop path as a timer error. Recoverable crashes still restart, but ten consecutive crashes stop automatic recovery even when backoff has aged earlier failures out of its window. Explicit removal/reset restores the budget; see [restart limits](../operations/monitoring.md#scheduler-restart-limits).

- **Shutdown and disable feedback do not invent an offline event**

  Scheduler feedback from shutdown or streamer-disable cleanup now parks local polling without publishing an authoritative Offline observation. This preserves shutdown session recovery and avoids racing the disable workflow's own session closure. Unknown internal stops wait for a real status check; actual streamer-offline feedback still reaches the monitor.

## Danmu Text

- **Danmu XML and MCP pages preserve valid text**

  Newly written danmu XML filters characters forbidden by XML 1.0 and sanitizes header comments. MCP byte pages preserve complete UTF-8 characters and continuation offsets; invalid encoding or nonprogressing limits return explicit errors. Existing files are not repaired; see [MCP paging](../api/api-keys-mcp.md).

## Service Ownership

- **Session cleanup is joined during shutdown**

  Shutdown cancels delayed ended-session eviction without waiting for its retention interval. API assembly reuses the container's repositories and configuration import service while keeping archive caches local. See [service ownership](../concepts/architecture.md#service-ownership).

## Health Monitoring

- **Unknown components make overall health degraded**

  A completed refresh no longer reports healthy when a disk or another component has unknown status. Unhealthy components still take precedence; degraded instances remain ready.

- **Slow disk sampling keeps health checks responsive**

  System and disk sampling runs on one dedicated thread with bounded waits. A stalled filesystem leaves earlier values marked stale or degraded while other probes continue, without accumulating replacement tasks. Health-checker shutdown can finish even if the operating-system call remains blocked; see [slow filesystem sampling](../operations/monitoring.md#slow-filesystem-sampling).

## Downloader Interfaces

- **Mesio diagnostics report the linked library version**

  Engine checks now report Mesio's compiled package version instead of a hardcoded historical value. Download manager ownership, event delivery and tests are organized into focused modules, and unused update/process/configuration wrappers are removed. Existing download events and runtime shutdown behavior are preserved; see the [Rust interface notes](../concepts/architecture.md#downloader-rust-interfaces).

## Process Cleanup

- **Confirmed startup recovery clears earlier runtime debt**

  Recovery now reports partial hydration, pipeline and coordinator failures explicitly and pages through all session segments. Only confirmed recovery clears earlier generation debt, while current ownership remains dirty until clean exit. Cross-process marker transactions prevent stale acknowledgements from overwriting a replacement generation.

- **macOS process cleanup waits for exit confirmation within its deadline**

  Forced cleanup now handles a leader that is exiting but not yet waitable when process-group termination returns EPERM. It uses the remaining cleanup budget to confirm exit without reaping, then retries guarded group termination. Unconfirmed cleanup remains an error; Streamlink buffer-draining limits are unchanged.

## Logging

- **Logging startup and idle behavior are predictable**

  Log-file initialization returns errors instead of panicking, retention runs immediately at cleanup-service startup, redirected console output omits ANSI colors, and live-log formatting is skipped without subscribers. Daily files still have no byte-size cap.

- **Log archives stream within resource limits**

  Downloads stream ZIP64 archives with bounded buffers instead of keeping the complete ZIP in memory. Two downloads can run at once; additional requests receive HTTP 429 with a retry delay. Each archive can contain up to 10,000 matching log files. Active logs are read up to their scanned size, interrupted downloads release capacity, and read or compression failures abort the download instead of completing a partial archive. No temporary archive file is created.

## Database Maintenance

- **Default database pages avoid full-table sorting**

  A new startup migration adds creation-time ordering indexes for unfiltered DAG and media-output pages. It removes four unused job timestamp indexes while preserving the indexes used by retention cleanup and duration statistics. Existing records and page ordering are unchanged; creating the new indexes scans those tables during the upgrade.

- **SQLite pools share a bounded cache allowance**

  The standard read and write pools now share a 64 MiB suggested private page-cache budget, with 56 MiB divided among read connections and 8 MiB reserved for the writer. The adaptive pool size and 256 MiB memory-mapping setting are unchanged. This reduces the cache allowance on larger pools; it is not a hard process-memory limit. See [SQLite memory budgeting](../operations/monitoring.md#sqlite-memory-budget).

- **Concurrent database mutations preserve their own results**

  Competing media-output deletions adjust session size once, and a failed size update rolls back deletion. Error increments return their own count. Template credential refresh reads and writes under one reserved transaction, preserving configuration edits committed before it.

- **Vacuum admission follows actual recording activity**

  Scheduled vacuum checks the download manager's active recordings instead of counting nonexistent download jobs. It defers when recording activity exceeds the configured limit or admission is busy, and holds new starts until admitted vacuum work finishes. Filesystem preflight runs before that gate, so a slow disk-space check does not hold up recording starts. Lightweight retention remains independent.

## Configuration

- **Large configuration refreshes use bounded parallel lookups**

  Startup and global/platform/template updates refresh up to 16 independent streamers together while preserving event order and per-streamer best-effort behavior. Startup output-root discovery is reused for both health registration and write probes. Configuration precedence, retirement handling and persistent recovery acknowledgements are unchanged. See [configuration refresh](../concepts/configuration.md#hot-reload-cache-and-update-events).

- **Backup imports validate the final account email assignments**

  Imports reject emails already assigned to retained accounts before changing configuration. Swaps between updated users and reuse of released emails work in either input order, while later failures roll back all changes. Email uniqueness follows stored values exactly, including case, whitespace, empty strings and absent emails. Every successful import still revokes all refresh tokens, even when a Merge import omits users; see [backup and restore](../operations/backup-restore.md#configuration-export).

- **Filters reuse parsed rules and handle timezone boundaries consistently**

  Cron and regex definitions use bounded caches. Time-based filters accept explicit IANA timezones and share overnight/DST interval boundaries for matching and wakeups, including overlapping repeated-hour windows. Existing omitted timezone defaults remain server-local for time-based rules and UTC for cron; frontend timezone controls are not added.

- **Proxy credentials preserve literal URL characters**

  Separate proxy usernames and passwords are percent-encoded before insertion, including literal percent signs, spaces, Unicode and authority delimiters. They replace embedded credentials while preserving the proxy host and port. Download-start diagnostics omit proxy URLs.

- **Startup output probes use recording-compatible gate keys**

  Probe discovery uses actual streamer/platform values and concrete directories, so a startup failure can block and later recover through the same key used by recording attempts. Writable child directories no longer require write access to ancestor keys. Explicit root boundaries retain precedence; ambiguous templates are skipped and probe work is bounded. Discovery follows saved settings, while historical gate entries and startup disk-probe topology remain unchanged; see [output-root probes](../operations/storage.md#output-root-probes).

- **Invalid configuration patches are rejected before saving**

  Global settings reject incorrect JSON types, invalid negative values, and overflowing counts before changing any saved field. Existing zero values for automatic concurrency, disabled recording limits, and retention remain supported, as do the existing timeout clamps. Platform settings retain their canonical name so edits cannot break URL-based platform lookup; numeric overrides are checked before storage.

- **Preset and template dates use consistent millisecond storage**

  Built-in presets and templates updated during credential refresh now display correct dates. Preset, template, and configuration-import writes consistently store integer milliseconds while the API keeps its existing date-string format. A new migration converts historical date strings without rounding fractional milliseconds or altering existing integers. Invalid historical values stop the migration without changing the data; see [timestamp repair guidance](../operations/upgrading.md#preset-and-template-timestamps).

- **Fresh standalone installations use the configured recording directory**

  A new standalone database initializes its output folder from `OUTPUT_DIR`, falling back to `./output` resolved against the startup working directory. The bundled systemd unit and Docker Compose configuration now supply the initial recording location. Existing databases retain their saved folder, including an explicitly saved `/app/output`.

- **Bilibili quality overrides preserve inheritance**

  Opening an inherited platform-options form no longer writes a quality override. Template and streamer forms show an inherited choice; selecting it clears the override. Platform defaults and quality codes now match the Bilibili extractor. Template options saved by the UI are applied by the resolver, while existing flat configurations remain supported.

## Post-processing

- Media and transfer processors share compatible error selection, result accumulation and source-file accounting. Existing output order, skip metadata, staging/rollback and source-deletion policies remain intact; naming and path-identity rules stay processor-specific.

- **Processor publication, paths and retries preserve their contracts**

  Subtitle filters accept apostrophes and filter delimiters in paths. No-overwrite publication uses native no-replace operations on supported platforms, retry waits cap at 30 seconds without overflow, and FFmpeg progress reports milliseconds consistently. File checks and abandoned temporary-output cleanup no longer block async workers.

- **Pipeline coordination drains accepted events during shutdown**

  Pending coordination events and queries finish in order before later requests use the same state directly. Closing or aborting the coordinator no longer discards its accepted queue, and a lost reply never causes an accepted event to run twice. Callers still own executing returned work; cancelling a caller does not guarantee those external actions complete.

- **Failed workflow publication releases its pending context**

  Failed database publication removes temporary segment and paired-workflow tracking. Successfully published workflows keep their tracking so workers can complete them. Malformed stored preset JSON is rejected before jobs are created, and validation errors omit configuration values.

- **Rclone moves reject missing inputs on the first attempt**

  A move validates every input before starting a transfer, so a missing file cannot be reported as a completed upload on its first attempt. Retried jobs retain the existing recovery behavior for sources consumed by earlier moves, including partial transfers within one execution.

- **Restart recovery pairs danmu with stored video segments**

  Recovery uses stored video paths to associate XML files with their original segment indices, avoiding duplicate processing caused by title digits or media-output IDs. XML files without a unique matching segment are skipped with a warning; their historical associations are not guessed.

- **Worker logs retain repeated messages and withdrawn jobs stay stopped**

  Successive log batches retain every entry even when timestamps and messages match. Completion snapshots avoid repeating entries already streamed to storage, while the existing log limit and API format stay intact. Workers check queue ownership before starting execution, so a removed job cannot run with a replacement cancellation token.

- **Execute jobs can run programs without a shell**

  The execute processor accepts `program` and an optional `args` array. Each argument expands placeholders once and retains spaces, quotes, empty values, and shell metacharacters as literal data. Existing `command` configurations remain supported. Both modes use the same timeout, process cleanup, and output scanning; ambiguous configurations and Windows batch files in program mode are rejected.

- **Lower worker limits take effect while jobs are running**

  Reducing CPU or I/O concurrency now restricts subsequent job admission immediately. Already-admitted jobs finish normally, and released capacity follows the latest limit without requiring another settings change. Rapid limit changes and worker cancellation do not leave stale permit reservations.

- **Workflow edits keep dependencies and preset warnings accurate**

  Renaming a step updates its dependents in both workflow editors, and empty or duplicate IDs are rejected. Labels and delete-after-transform checks resolve referenced presets by exact name regardless of the preset count. Inline steps display their own processor. Loading, failed, or missing preset lookups are shown explicitly, and saving a potentially destructive step with unresolved presets requires confirmation.

- **Incomplete outputs no longer replace completed files**

  Thumbnails, extracted audio, metadata copies, and subtitle burn-ins now use temporary files and verify that output exists and is nonempty before publishing it. A failed or cancelled batch cleans up its staged files and preserves source files and existing destinations. Publication failures roll back earlier outputs in the batch. Output paths that alias inputs are rejected, and disabling overwrite remains safe when jobs publish to the same destination concurrently.

- **Cancelled and timed-out commands stop their subprocesses**

  Post-processing commands now stop their entire process tree when a job is cancelled or times out. They receive closed standard input and drain their output within bounded log limits, including when a parent exits while a descendant still holds a pipe. Audio probes use the same cleanup behavior.

## Email Delivery

- Email channels now retain an SMTP connection pool and render localized content once per message. Configuration replacements keep separate pools while previously admitted deliveries retain their original channel. The unused `EmailConfig.batch_window_secs` field was removed; existing JSON values remain harmless and ignored. Email delivery is still immediate.

## Notifications

- **Bounded queues and consistent Web Push persistence**

  Ordinary channel admission now enforces capacity atomically and cancels evicted retries. Breaker cooldowns preserve attempt accounting. First-attempt Web Push success clears stored backoff, failed stale-subscription deletion is not reported as successful, and abbreviated payloads also enforce their byte cap. See [delivery contracts](../concepts/notifications.md#queue-and-web-push-delivery).

- **Web-push overload stays bounded**

  Full or unavailable push queues drop new events without spawning fallback tasks. FIFO admission applies to every priority; rejected pushes are counted in notification statistics and are not automatically replayed. Event history and ordinary channel delivery remain independent; see [delivery behavior](../concepts/notifications.md#delivery-behavior).

- **Configuration channels retain failed deliveries**

  Notifications that exhaust their retries now appear in the in-memory dead-letter list even when their channel comes from configuration. Database persistence remains available for database channels; persistence failures preserve the in-memory record. Retention cleanup still applies, and successful channels are not sent the same notification again during retries.

- **Notification reloads preserve breaker state and delivery targets**

  Channel and subscription lookups see one complete registry during reload. Still-loaded database IDs retain breaker history, failed reads preserve the registry, and dynamic additions survive discovery. Admitted notifications keep their selected channel instance through delivery and retries; removal followed by re-addition gets an isolated breaker generation.

- **Failed notification deliveries finish within a time limit and protect credentials**

  Discord and Telegram requests now time out after 30 seconds. Gotify and webhook timeouts are capped at five minutes; zero uses the 30-second default. Rate-limit retries wait at most 30 seconds each and stop after three attempts. Invalid retry delays no longer cause a panic. Delivery errors retain the failure category or HTTP status without exposing token-bearing URLs or server response bodies.

- **Telegram formatting preserves literal content**

  HTML, Markdown, and MarkdownV2 settings now use explicit formatting entities so special characters in streamer names, titles, and errors cannot break message parsing. Unicode-safe truncation keeps formatting spans valid. Empty mode sends plain text; unknown modes return a local configuration error.

## Recording Engines

- **Streamlink can use an explicit FFmpeg executable**

  Set the optional backend engine field `ffmpeg_path` to choose the FFmpeg process used for Streamlink remuxing. Omitted or null values retain `FFMPEG_PATH` then `ffmpeg` fallback. See [engine configuration](../concepts/engines.md#streamlink-ffmpeg-executable).

## Recording

- **Recording filenames and event tracking survive custom input**

  Percent signs in streamer names, titles and concrete output directories remain literal while configured date tokens still expand. Startup probes follow the same rules. Required FFmpeg info logs and statistics override quiet options, long stderr records are scanned incrementally, and download snapshots copy only their public fields.

- **Concurrent danmu stops share collector completion**

  Stop callers and replacement sessions wait for the same collector outcome without consuming another caller's completion signal. Waiting for startup serialization and the previous collector shares a ten-second handoff budget. A cancelled caller or timeout does not discard completion or allow an overlapping replacement; connection setup retains its separate behavior.

- **Streamlink stop keeps output flowing while containing subprocesses**

  Stdout forwarding and stderr readers stay alive while Streamlink stops, allowing emitted tail data to reach FFmpeg before EOF. Unix requests SIGTERM; Windows uses a bounded natural-exit window before forced tree termination. Both subprocess trees are contained, including descendants left after a parent exits. Internal Streamlink ring-buffer data and forced-stop tails remain subject to loss; see [stopping Streamlink recordings](../concepts/engines.md#stopping-streamlink-recordings).

- **Engine resolution uses bounded version probes and reports invalid settings**

  Custom engine resolution and engine tests probe executables asynchronously with a three-second limit and bounded process cleanup. Probe output is drained with bounded memory use. Repository failures, malformed base settings, and invalid overrides are reported instead of silently selecting defaults; missing named configurations retain the existing fallback behavior. Synchronous startup constructors use the same bounded probe.

- **Recording stops retain confirmed final segments**

  FFmpeg and Streamlink receive a separate bounded cleanup period after their graceful-stop deadline expires. Confirmed final segments are published once before the recording ends; unconfirmed cleanup does not advertise completion. Unused FFmpeg standard output cannot fill an unread pipe, and stopping Mesio before its first HLS segment no longer counts as an engine failure.

- **Sessions reconcile correctly after restart**

  The first confirmed offline check closes an unfinished session left by the previous process and makes its final post-processing recoverable. Failed or suppressed checks remain retryable. A broadcast that is still live continues its existing session, and normal offline grace periods remain in effect.

- **Concurrent session events preserve one completion and its cause**

  Live checks, offline checks, download completion, timers, and user stops now serialize their session changes for each streamer. Late or duplicate events cannot overwrite a completed session's end time or repeat its completion, and an old session's offline signal cannot stop its live successor. Disabling a streamer preserves authoritative offline causes and still closes an active session when its timer handle is missing.

- **Output write failures pause recording retries reliably**

  A failed buffered write during shutdown is now preserved even when the stream also disconnected. Disk-full, read-only, permission, missing-path, and output timeout failures reach the output-root gate before the recording failure is published. FFmpeg and Streamlink retain recognized output failures even after a zero exit code. Input network errors do not mark storage unavailable, and retries remain throttled if no output-root gate is attached.

- **Streamer removal waits for every recording's post-processing**

  An older recording could still be processing when a newer recording finished, allowing streamer removal to leave the older work without the information needed to resume after a restart. Removal now waits for all unfinished recordings. Final post-processing that could not be started remains pending and can be retried after its missing preset is restored.

- **Replace imports retain configurations needed by finishing recordings**

  Replacing a backup that omitted both a streamer and its template could fail and roll back the import. Omitted templates and presets may now remain visible temporarily, with an import warning, until the departing recordings finish their post-processing. Editing a retained definition, reimporting it, or assigning a retained template to an active streamer keeps it.

- **Shared platform logins no longer refresh concurrently**

  Several recordings sharing a login could refresh it at the same time and incorrectly report that signing in again was required. Refreshes now wait for one another, and waiting recordings receive the updated cookies.

- **Recordings are finished properly when the app is stopped**

  Stopping rust-srec — Ctrl+C, `docker compose down`, a container restart, or a system shutdown — could cut a recording short. The recording tool was sometimes killed before it finished writing, leaving the last part truncated, missing from the session's file list, or without its chat file. Shutdown now stops taking on new work first, lets the recordings that are already running finish and be saved, and only then closes down. Recording tools are now tied to the app itself as well, so none of them are left running after it exits.

- **Shutdown always finishes within a time limit**

  A recording that could not finish used to be able to hold shutdown open indefinitely. There is now a deadline — 30 seconds by default — after which anything still running is stopped and the app exits regardless. If your recordings routinely need longer to close, raise `RUST_SREC_SHUTDOWN_TIMEOUT_SECS`; the Docker Compose file has a matching `stop_grace_period` so Docker waits for the app rather than killing it first. A shutdown that takes longer than its grace period but still saves everything is treated as a normal exit. See [Configuration](../getting-started/configuration.md#shutdown).

- **An interrupted shutdown is reported at the next startup**

  If the app was killed outright, or could not finish closing in time, that is now recorded and reported the next time it starts, so an interrupted recording is visible instead of silently incomplete.

- **A failed save no longer ends a running recording**

  If one part of a recording could not be written to the database, the whole recording stopped. It now keeps going and the failure is reported instead.

- **Fixed Unicode filenames in FFmpeg segment recording**

  FFmpeg recording and post-processing no longer force the entire child process into the `C` locale. That override could prevent Unicode output paths from opening on Windows, particularly for segment-mode filenames expanded with `strftime`. Message and numeric formatting remain stable for progress parsing, while character and time handling retain the parent UTF-8 locale.

- **Streamers stay monitored after an unexpected failure**

  A streamer whose monitoring hit an unexpected internal error was quietly left unwatched until rust-srec was restarted, so nothing was recorded in the meantime. Monitoring is now restarted automatically. Quickly disabling and re-enabling a streamer no longer leaves it monitored twice or not at all either.

- **A sign-in saved for a single streamer is no longer lost**

  Scanning the QR code to sign in to Bilibili for one streamer kept the account only until that streamer was next touched: renaming it, enabling or disabling it, changing its priority, or including it in a bulk action signed it out again, and its recordings carried on without the account. Sign-ins kept fresh for one streamer went the same way — whether renewed automatically, refreshed by hand, or handed over by the platform while checking whether the streamer was live. All of them now stay put.
- **A full disk is reported instead of quietly breaking every recording**

  When the drive filled up part-way through a recording, the built-in recorder reported it as a broken recording and held it against that streamer, while every other streamer kept starting and failing the same way. Running out of space is now recognised for what it is: you get the "Output path inaccessible" alert, recordings pause, and they resume on their own within 30 seconds of space becoming available.

- **Deleting a streamer keeps its recording history**

  Removing a streamer also erased every session it had ever recorded, along with the file list, segments and chat statistics attached to them — the recordings were still on disk, but nothing in the app could find them. Past sessions now stay in the list after the streamer is gone, under the name it was recorded with, and any session that was still recording is closed out rather than left showing as live.

- **Deleting a streamer stops its recording cleanly first**

  Deleting a streamer used to remove it immediately while its recording was still running, so the last part could end with an error, the session's timeline was left incomplete and post-processing could fail on a streamer that no longer existed. Deleting now takes the streamer off the list right away and finishes its recording properly first: the recording is closed out and saved, its post-processing is allowed to finish, and only then is the streamer removed for good. If post-processing is still running, the streamer disappears from the list immediately and is cleaned up in the background as soon as it finishes — including after a restart. This applies to every way of deleting a streamer: the delete button, bulk delete, the API, and importing a configuration that no longer contains it.

  One thing to know if you script this: for the short while a deleted streamer is still being cleaned up, its address stays reserved. Adding a streamer with the same address again — or importing a configuration that still contains it — is refused with "still being removed; try again shortly" until the clean-up finishes. Deleting a template a still-departing streamer used is refused the same way.

- **Importing a configuration no longer disturbs your running recordings**

  Importing a backup used to briefly flip every live streamer to offline, which produced a duplicate "went live" notification and made the streamer bounce between states on the dashboard. Streamers that were live stay live, and their recordings keep running untouched. New streamers in the imported file also start being monitored right away instead of waiting for the next restart, and an import that is rejected no longer stops anything at all.

## Authentication

- **Logout and password changes invalidate issued access tokens**

  Login and refresh tokens now share a durable session. Single logout closes only that session; logout-all and configured refresh-token reuse detection revoke every session for the user. Password changes revoke existing sessions atomically, disabled accounts cannot restore old sessions, and successful configuration imports revoke login sessions too. Download/log WebSockets revalidate every five seconds with a three-second lookup deadline; one-shot log archive grants recheck their issuing identity. After upgrading, existing unbound access tokens require a refresh with a still-valid refresh token or a new sign-in. See [session security](../operations/security.md#revocable-login-sessions).

- **Failed login responses conceal account existence and disabled state**

  Missing users receive bounded dummy Argon2 verification and the same credential error as an incorrect password. Disabled-account status is disclosed only after a correct password. Login throttling still runs before password work.

- **Refresh-token rotation is atomic and rejects replay**

  A refresh token issues at most one replacement, and failed database writes preserve the original token. Reusing a consumed token from an open session now revokes all of the user's access and refresh sessions by default; replaying a logged-out session leaves other devices signed in. The optional grace window suppresses that revocation but no longer issues tokens for a replay. Clients must serialize refreshes; see [refresh-token rotation](../operations/security.md#refresh-token-rotation) for configuration and concurrency behavior.

## API and integrations

- **Creation responses and OpenAPI match the running API**

  Resource creation and template/job-preset cloning now return the documented 201 status with the same JSON bodies. OpenAPI includes session segments, template cloning and all four browser Web Push operations. Job summaries report unavailable progress as `null` instead of a fabricated zero; the dedicated progress endpoint still returns actual snapshots when available. See [response contracts](../api/index.md#creation-responses-and-job-progress).

- **Job pages and configuration exports batch related lookups**

  Streamer display names, exported filters and notification subscriptions now use deduplicated batches of up to 500 owners instead of one query per owner. Response/export ordering, missing-owner behavior and best-effort handling of related-data failures are preserved; failed batches retry their owners individually. See [configuration exports](../operations/backup-restore.md#configuration-export).

- **Search treats percent signs and underscores literally**

  Searches for jobs, sessions, media outputs, notification events and both kinds of presets no longer interpret `%` and `_` as wildcards. Backslashes also match literally, so `audio_extract` only finds that text rather than names such as `audioXextract`. Existing case matching, other filters, pagination totals and media summaries remain consistent; see [search filters](../api/index.md#search-filters).

- **API errors and request batches have explicit boundaries**

  Internal diagnostics no longer enter ad-hoc API errors. Parse and session-delete batches reject more than 100 items before work begins. Device descriptions are bounded to 256 Unicode characters for new logins, refreshed legacy sessions and diagnostics; credential refresh keeps its relogin indication.

- **Configuration reads are cached coherently and missing entities use typed errors**

  Stream proxy and parsing requests reuse a five-second global snapshot, immediately invalidated by application writes and imports. Administrative reads remain authoritative and expired cache entries never hide refresh failures. Platform, template and engine handlers distinguish missing entities from database errors without matching message text.

- **API keys for programmatic access**

  You can now create long-lived API keys as an alternative to short-lived JWT session tokens. Keys belong to the user who created them, carry an optional expiration timestamp, and can be scoped to either `read_only` (access to non-sensitive queries such as sessions, danmu, aggregate statistics, notification events, and system health) or `full` access (all requests including configuration changes and mutations). Keys are stored as SHA-256 hashes and displayed only once at creation. Revoking a key invalidates it immediately across the server and clears any authorization cache. API keys cannot manage other keys or change passwords. Read-only keys can read authenticated health details and recorded media; download/log WebSockets, stream proxy and logging routes require full keys. These manual routes prefer Authorization headers, with query-token fallback only when the header is absent. See [API Keys & MCP](../api/api-keys-mcp.md).

- **Built-in Model Context Protocol (MCP) server**

  The backend now exposes a built-in MCP server using the streamable HTTP transport at `/api/mcp`. AI assistants such as Claude Code, Claude Desktop, and Cursor can connect directly using an API key to inspect recording sessions, analyze danmu activity and word frequency, read raw chat XML with byte pagination, observe pipeline jobs, manage streamers, and update configuration. Tools execute in-process against existing application services, sharing the same validations and dynamic updates. Read-only keys are restricted to safe inspection tools and cannot access configuration or credentials. See [API Keys & MCP](../api/api-keys-mcp.md).

- **Dedicated API key management in the Web UI**

  A new **Settings → API Keys** page lets you create, inspect, and revoke API keys with custom names and expiry dates. The page also generates ready-to-copy MCP configuration snippets for Claude Code, Cursor, and standard MCP clients.

- **Repeated failed sign-ins are now slowed down**

  Guessing a password can no longer be retried without limit. After five failed sign-ins an account is refused for a cooling-off period, and the reply says how many seconds to wait. Signing in correctly clears the count immediately, and the message never reveals whether the username exists. A second, far more generous limit caps how much password-checking work one source can ask for; note that behind a reverse proxy — including the frontend that ships with rust-srec — every sign-in looks like it comes from the proxy, so that limit protects the server rather than telling users apart. Password checks are also queued rather than all run at once, so a flood of sign-in attempts no longer slows the rest of the app down. Both limits can be tuned with `API_LOGIN_MAX_FAILURES`, `API_LOGIN_IP_MAX_FAILURES`, and `API_LOGIN_WINDOW_SECS`. See [Configuration](../getting-started/configuration.md#login-throttling).

- **Stricter browser access while sign-in is turned off**

  In the local-development mode that runs without sign-in, the backend used to accept browser requests from any website you happened to have open. It now turns away requests that come from an unknown page, or that arrive under a web address other than your own machine's, so another tab cannot quietly issue commands to your recorder. Requests from the local web interface, the desktop app, and tools such as `curl` are unaffected. If you open the interface from a different address, list it in the new `API_CORS_ORIGINS` setting. Nothing changes when sign-in is on — and with sign-in on, a password or API key is what protects the API. See [Configuration](../getting-started/configuration.md#security-auth).

- **Cancelling a pipeline with `DELETE /api/pipeline/{pipeline_id}` now ends it**

  This request stopped the pipeline's steps but left the pipeline itself showing as still processing, for good: it could not be retried, it stayed that way after a restart, and the recording it belonged to never finished post-processing. It now ends the pipeline too, so it can be retried and the recording moves on. Only scripts and integrations calling this request directly were affected — the **Cancel** button in the web interface uses a different one and always worked.

## Pipeline and uploads

- **Shared processor drivers preserve publication policies**

  Output planning, four media drivers, nine single-file skip results and path-resolution mechanisms now share implementations with explicit naming, mapping, publication and identity policies. Staged rollback and incremental remux cleanup remain distinct; see [processor contracts](../concepts/pipeline.md#processor-result-contracts).

- **Pipeline completion and recovery share artifact handling with fewer reads**

  Pipeline construction, input manifests, leaf-output collection and source-artifact reservations now use shared implementations. DAG publication reads a streamer once for its name and platform, completion reuses transaction-owned snapshots, and recovery pages all statuses for a session in one scan. Manifest schemas, output order, optional metadata, failed-write reporting and duplicate-completion protection remain unchanged; see [pipeline error handling](../concepts/pipeline.md#error-handling).

- **Retries retain execution history and previously published files**

  Starting another attempt no longer resets step timings, log counters, file-size metadata or produced-artifact history. Earlier logs are not duplicated, and additional stored execution-metadata fields survive completion and failure updates. The processor runs only after its attempt marker is saved; invalid metadata or a failed write stops processing and reports a failure without replacing the original metadata. Failed retries do not delete files published by earlier attempts; processors retain responsibility for their staged temporary outputs.

- **Retry workflows without leaving cancelled branches stuck**

  Retrying just one job in a failed workflow could leave its other branches stuck indefinitely. Workflow jobs now direct you to retry the whole workflow, which restarts its failed and cancelled branches together.

- **Upload recordings to Baidu Netdisk**

  A new `baidupcs` pipeline processor uploads recordings to Baidu Netdisk through the BaiduPCS-Go command-line tool, which is now bundled in the Docker image. Add it to a pipeline like any other upload step: the destination folder supports the usual streamer/title/date placeholders, same-name files can be skipped or overwritten, and uploads appear in the same live progress, per-file records and streamer-card indicators as rclone transfers. Log in from the preset editor — paste your netdisk cookies (or BDUSS and STOKEN) once and the account card shows who is signed in and how much space is left. Tick **Remember for automatic re-login** and upload jobs log in again by themselves when the session expires, so a recording made at night still lands in the netdisk without anyone clicking Login; leave it unticked and the credentials are handed to BaiduPCS-Go without the app keeping them. If the remembered credentials themselves stop working, a notification tells you to log in again and further attempts pause for an hour instead of hammering Baidu. Logging out forgets the remembered credentials. Because BaiduPCS-Go's exit code does not reflect upload results, rust-srec reads the tool's per-file output instead, and a retried job re-sends only the files that did not make it. See [DAG Pipeline](../concepts/pipeline.md#baidu-netdisk-baidupcs).

- **Post-processing no longer stalls or repeats work after a crash**

  If the app stopped unexpectedly — a crash, a host reboot, a container killed mid-job — a recording's post-processing could be left half-finished. The remaining steps were never started and the session stayed stuck as still-processing with no way to retry it, while a step that had just completed could run a second time on the next start, re-uploading files that had already been uploaded and re-running any move or delete steps that followed it. Post-processing now resumes from where it stopped, and steps that already finished are not run again.

- **Pipeline steps that could never run no longer hang the whole recording**

  A step naming a processor that does not exist was accepted, queued, and then simply never picked up — the pipeline sat at "processing" forever, the recording it belonged to never finished post-processing, and the stuck job kept counting towards the queue depth, eventually making the app throttle its own recordings. This affected the built-in **Create ZIP archive** preset and any compression preset made in the preset editor, both of which named a processor the workers did not recognise. Those presets now run. A pipeline that still names an unknown processor is rejected when you save it, with the list of processors you can use, and jobs already stuck from before this release are failed at startup so the recording waiting on them can move on.

- **Pipeline notifications for started, finished and failed jobs now arrive**

  Subscribing to **Pipeline started**, **Pipeline completed** or **Pipeline failed** produced nothing: a transcode or upload could fail and no notification was ever sent. These now fire as the jobs run.

- **Deleting a pipeline execution stops the work it was doing**

  Deleting a pipeline that was still running removed it from the list but left its job running in the background — the transcode or upload carried on, and the recording it belonged to kept waiting for a pipeline that no longer existed. Deleting now cancels the work first.

- **Queued jobs run oldest-first**

  With more work queued than the workers could keep up with, the most recently added job was always picked next, so an older job could be passed over indefinitely while newer recordings kept jumping ahead of it. Jobs of equal priority now run in the order they were queued; a higher priority still goes first.

- **Transcoding no longer deletes the file it just produced**

  When "remove input on success" was enabled and the output turned out to be the same file as the input — reachable through a symlinked folder, or differing only in upper/lower case on macOS and Windows — the step overwrote the recording with the transcoded version and then deleted it, losing both. The step now detects that the input and the output are one file and keeps it.

- **A move step no longer reports success for a file it did not move**

  When a move step found its source file missing, any file with the same name in the destination folder was accepted as proof the move had already happened, and that unrelated file was passed on to the following steps. This resume now only applies where it was meant to — a retried job, or one picked up again after a crash.

- **Workflow outputs reach the next step in a consistent order**

  A step placed after a workflow received that workflow's outputs in a different order on every run, which could change the result of steps that combine their inputs, such as concatenation. The order now follows the workflow definition.

- **Stopping the app no longer waits on a long post-processing job**

  A transcode or upload running at shutdown held the app open until the shutdown deadline expired and everything was cut off, which was then reported as an unclean exit. Short jobs are still given time to finish and record their result; a longer one is now asked to stop early so the app can close normally, and it runs again from the start next time.

- **Post-processing that never got started is now picked up**

  If the app stopped in the moment between a recording finishing and its post-processing being set up — or if setting it up failed — that work was lost for good: the uploads and transcodes configured to run after a recording simply never ran, and nothing said so. Startup now notices post-processing that was due but never began and starts it, however long the app was down, both for a whole session and for an individual part of a recording. Recordings that had already finished before this update are left alone, so updating — or turning post-processing on for the first time — does not retroactively run it across everything you have recorded so far.

- **Select several pipelines and act on them at once**

  Tidying up finished pipelines meant opening each card's menu and confirming one at a time. **Pipeline Jobs** now has a **Select** button: tick the pipelines you want and cancel, retry or delete all of them together. Retry and cancel only apply to the pipelines they can — the buttons show how many of your selection they will touch — while delete works on any of them, stopping anything still running first. If part of the batch does not go through, only those pipelines stay selected so you can try again on just them.

- **Delete recorded files from the Media Outputs page**

  Media outputs could only be browsed, so clearing space meant finding the files on disk yourself and then having no way to tidy up the leftover entries. Each output now has a **Delete** option, and a **Select** button lets you clear many at once. By default this only removes the entry and leaves the file untouched; tick **Also delete files from disk** in the confirmation to remove the recording itself. Deleting an entry whose file you already removed by hand works as expected, and the owning recording's total size is corrected either way.

- **Custom command steps no longer trip over unusual file names**

  A recording named after the stream title can end up with characters a shell reads as instructions — an apostrophe, a quote, `$`, a backtick, a semicolon. A pipeline step running a custom command would then fail on that file, or run part of the title as a command of its own. File paths, titles and streamer names are now quoted automatically before the command runs, so they are passed on as text instead of being run — as a bare argument, inside quotes, inside `$(...)` or backticks, or in a here-document body. Pipes, `&&` and redirects in your own command still work, and placeholders you already wrapped in quotes yourself keep working — do not add extra escaping around them. See [DAG Pipeline](../concepts/pipeline.md#built-in-processors) for the two remaining limits.
- **A job picked up again after a restart no longer shows stale progress**

  If the app stopped while a transcode or upload was running, that job was put back in the queue on the next start but kept showing the percentage it had reached before — a job that was only waiting its turn could sit there reading "42%" until it actually started running again. The old figure is now cleared the moment the job returns to the queue.

- **Large recordings can now be put in a ZIP archive**

  A ZIP step gave up on any recording larger than 4 GB: it wrote most of the archive, then failed and left nothing behind. Recordings are now archived whatever their size.

## Danmu

- **Chat recording now survives network interruptions**

  If the connection to a platform's chat server dropped and could not be re-established within a few minutes, chat recording used to stop for the rest of the stream — the video kept recording, but every later part had no chat file, and nothing said so. Chat now keeps reconnecting for as long as the recording lasts, and picks up again by itself when the connection comes back. Each part of the recording still gets its own chat file even if the connection is down while that part is recorded, and the statistics carry on from where they left off instead of restarting. A chat connection that stays down is reported on the system health page, so an outage is visible rather than silent.

- **Post-processing no longer stalls when chat recording fails**

  When chat recording ended unexpectedly, the session's post-processing steps — uploads, transcodes, anything configured to run after a recording finishes — were never started for that session. They now run as normal.

- **The last messages of a stream are no longer dropped**

  When a recording stopped, chat messages the platform had already delivered but that were still queued were discarded — up to a hundred of the stream's final messages, missing from both the statistics and the last chat file. They are now collected before the recording closes.

- **Chat no longer spills into the next part of a recording**

  When a recording rolled over to a new part, chat messages the platform had already delivered but that were still queued ended up in the new part's chat file, showing at its very first second instead of at the end of the part they were actually sent in. They now stay with the part they belong to.

- **Chat files are no longer left incomplete**

  A chat recording that ended because of a connection failure left its file unterminated and unregistered, so it did not appear among the session's files and could not be used by the danmaku conversion step. The file is now closed properly and recorded like any other, and chat belonging to a recording part that gets discarded for being too small is now removed from the session's file list along with it.

- **Danmu statistics are configurable per streamer**

  How chat activity is summarised is no longer fixed. A new **Danmu Statistics** section in Global Config — and an override on every platform, template and streamer — sets how many chatters and words are ranked, how fine the activity timeline is, how many distinct chatters are tracked before counts become estimates, and extra words to ignore in the frequent-words chart. You can also turn the summary off entirely while still recording the chat files, which skips storing viewer names. See [Configuration](../getting-started/configuration.md#danmu-statistics).

- **Frequent-word counts are no longer inflated**

  The frequent-words chart could report counts far above the truth on busy streams — a word sent a handful of times could appear with a count in the thousands, and the lower half of the chart filled with unrelated words all showing near-identical figures. Counts are now accurate, and any entry that is still an estimate is marked with `≈`.

- **Activity chart rates were six times too high**

  The timeline, its peak and its average were labelled per minute but counted per ten-second bucket, so all three read about six times the real rate — more on long streams, where the chart's resolution is reduced automatically. They now show true per-minute rates. The average is also taken across the whole stream rather than only the moments with chat, and quiet stretches are drawn as gaps at zero instead of a straight line at the surrounding rate.

- **Statistics survive a restart mid-recording**

  If the app restarted while a stream was being recorded, its chat statistics started over from zero and the lower numbers replaced what was already saved. Counting now resumes where it left off.

- **More detail on the session page**

  The danmu panel now shows the average messages per minute and, for streams that received gifts, how the total splits between chat and gifts.

- **Live danmu statistics while recording**

  Danmu statistics no longer wait for the stream to end: while a recording is running, a snapshot is saved about once a minute, so the session page's danmu panel (totals, activity timeline, top talkers, frequent words) fills in while the stream is still live. If the app crashes or the host reboots mid-recording, at most the last minute of statistics is lost instead of the whole session's.

- **Activity timeline covers the whole stream on long sessions**

  The danmu activity chart used to keep only the most recent six hours at full detail and silently dropped the oldest points on longer sessions. Once the limit is reached it now halves the chart's resolution instead, so a 12-hour recording still charts from the first minute to the last — just at a coarser granularity.

- **Expandable Top Talkers**

  The Top Talkers card on the session page shows the six most active chatters by default and can be expanded to the full ranking, which is scrollable. How many chatters are ranked is configurable and defaults to 100.

- **Chinese and Japanese chat is now split into real words**

  The frequent-words statistic used to split only on punctuation, symbols and emoji. Chinese and Japanese are written without spaces, so a message with no punctuation still ended up counted as one long "word" and the chart filled with whole sentences instead of words. Chat in those languages now goes through proper word segmentation, including common livestream vocabulary that general-purpose dictionaries miss, so `主播今天好厉害啊` counts `主播`, `今天` and `厉害`. Other languages are unaffected.

- **Unique chatters metric**

  The session danmu panel now shows how many distinct users chatted during the stream (a memory-bounded estimate, typically within about 2%), alongside the total message count. Sessions recorded before this release show a dash.

- **Gift rankings**

  For platforms that report gifts in chat (Bilibili, Douyu, Bigo, SOOP, ...), the session page now shows two extra charts: the top gift senders and the most-sent gifts, both weighted by the number of gift items rather than messages. The charts only appear when the stream actually received gifts.

- **Removed the `danmu_sampling_config` setting**

  This template/streamer setting never had any effect — statistics have always counted every message. The field has been removed from the REST API (`/api/templates`) and the database; existing configurations are cleaned up automatically, and older exports that still contain the field import fine.

## Web interface

- **Screen readers and keyboards can use every control**

  Buttons that show only an icon — the menu and sidebar buttons, back arrows, remove and copy buttons, the player's settings and remove buttons, and the pipeline graph's zoom controls — were announced as an unnamed button, so a screen reader could not say what they do. They now all have a spoken name. The day and hour pickers in a time filter and the merge-or-replace choice when restoring a backup could only be operated with a mouse; they can now be reached with Tab and operated with the keyboard — the space bar for the day and hour buttons, the arrow keys for the merge-or-replace choice — and they announce which options are selected.

- **Emptied settings boxes stay empty**

  Clearing a number — a timeout, a retry count, an image width — left something behind that could not be saved, and saving reported a value that is not a number. Douyu's preferred CDN could not be cleared at all: deleting it immediately put the previous value back. Clearing a box now returns that setting to its default, the box stays empty, and the value that will be used instead is shown in its place. Settings that only accept whole numbers now say so in the form, instead of letting a fractional value through to fail later.

- **Editing a list of settings no longer jumps to another row**

  Removing a custom webhook header, a metadata tag or a stream parameter shifted the remaining rows onto their neighbours and took the cursor with them. Two blank custom tags also merged into one as soon as they were added. Each row now keeps what you typed into it, blank rows stay separate, and a tag is saved once you give it a name.

- **The raw JSON editor no longer rewrites what you type**

  In the platform options JSON view every keystroke reformatted the whole document and sent the cursor to the end, so compact JSON was impossible to type. The text now stays as you typed it, an incomplete snippet is reported without being thrown away, and loading a configuration or cancelling still refreshes the editor.

- **Notification subscriptions survive switching away and back**

  Ticking events for a notification channel, switching to another window and coming back discarded everything you had ticked and restored the previously saved list. Your selection is now kept until you save or close the dialog, and clicking a checkbox toggles it instead of only clicking the row around it.

- **Bilibili QR sign-in stops once the code expires**

  The sign-in dialog kept asking the server about an expired QR code every two seconds, and typing anywhere else on the settings page sent an extra request each time. It now stops as soon as the code expires and waits for you to ask for a new one.

- **Selected items no longer follow you to another page**

  On the recordings and streamers lists, rows you had ticked stayed selected after you searched, changed a filter or moved to another page, even though they were no longer on screen — and a delete or batch action then applied to them too. Changing what the list shows now clears the selection, so an action only ever affects rows you can see.

- **Downloading a recording starts right away**

  Saving a recording from a session pulled the entire file into the browser first and only then offered to save it: nothing appeared to happen for a long time, and a large recording could take the tab down with it. When the web interface is served from the same address as the recorder, which is the usual setup, the download now goes straight to disk and starts as soon as you click. Where the two are on separate addresses, and in the desktop app, the file is still prepared in the browser first, with a progress toast while it is. The player window's title is translated as well.

- **One unreadable setting no longer blanks the platforms and templates pages**

  A platform or template whose stored settings could not be read — written by an older version, or edited by hand — stopped the whole list from loading. Only the setting that cannot be read is now skipped, and every other setting and entry loads normally.

- **A recording without chat statistics says so**

  Opening a recording that has no danmaku statistics, because chat capture was off or the recording had only just begun, reported "Failed to load danmu statistics." with a retry button. It now simply says the statistics are not available for that recording.

- **A pipeline that cannot be opened reports the reason**

  Opening a processing pipeline that no longer exists, or opening one while the server was unreachable, left the page showing loading placeholders indefinitely. The page now explains what went wrong and offers a way back.

- **Signing out and back in behaves like a fresh start**

  After signing out, the next person to sign in on the same tab could still see the previous account's streamers, recordings and transfers for a moment. Following a link into the app while signed out also dropped you at the dashboard afterwards, losing the page you actually wanted. Signing out now clears everything from the previous session, and signing in takes you to the page you originally opened, with its search and filters intact.

- **The dashboard's processing tile keeps up with cancellations**

  Cancelling a pipeline updated the counts on the processing jobs page but left the dashboard's own processing tile showing the old numbers until its next automatic refresh. Both now update together.

- **Notification and preset cards show their colors again**

  The rounded tile behind the icon at the top of every notification-channel and processing-preset card had lost its colored tint and sat on a plain background. Each tile is tinted again to match the kind of channel or processing step it stands for — indigo for Discord, blue for e-mail, green for uploads, and so on.

- **Configuration editors respond faster while typing**

  Editing a template, a platform or a streamer redrew the whole form on every keystroke, which felt sluggish on the longer forms and worst of all with the pipeline editor open. A change now refreshes only the part of the page that shows it. The import summary under **Backup & Restore** and the 24-hour bar in the time filter no longer rebuild themselves for unrelated changes either, and the live figures on a running recording count up smoothly instead of stopping partway.

- **The browser console no longer prints workflow settings or the log stream address**

  Saving a pipeline workflow wrote its complete step configuration to the browser's developer console, including any upload credentials it carried, and opening the log viewer printed the address it connects to, which contains your access token. Neither is printed any more.

- **The last untranslated corners of the interface now follow your language**

  Some parts of the interface stayed in English whatever language you chose: the theme import dialog, the danmu viewer's filter button and footer, the video player's title and error message, the cookie fields on the player form, the badges on a recording card, the version line at the bottom of the sidebar, the comparison table shown when a recording was split, and a few button labels for screen readers. Counts such as "12 messages", "3 Segments (7 Files)" and "5 selected" were also assembled from separate words, so they could never be phrased naturally in Chinese. All of these now read in your language, and so do the error messages on the form for a new processing job.

- **Dates, times and durations follow your language**

  "5 minutes ago" on notifications, streamer cards and the system health page, and the date ranges on the recordings and log pages, were always written in English and in an English date format. They now match the language you have selected, and a session's duration is shown with units in your language. A session that has not finished yet is now labelled "In progress" instead of the mistyped "In active".

- **A bad link no longer replaces a list page with an error**

  Recordings, streamers, platforms, templates, processing jobs, workflows, presets, media files, the notification events feed and the player all remember your search, filters and page number in the address bar. If any one of those values was not something the page could read — a hand-edited address, a truncated link, or one saved before an update changed what a filter accepts — the whole page was replaced by "Something went wrong!" and a block of technical detail. Values that cannot be read are now simply ignored, and the rest of the address still applies: a link carrying both a search term and an unusable page number now runs the search and starts from the first page.

- **Media files page is readable again**

  Every card on the **Media files** page was squeezed into the same shape regardless of what it held, and long recording names were cut down to two or three characters. Each file now leads with a coloured icon for what it actually is — a recording, audio, a thumbnail or a chat file — followed by its full name on its own line and the folder it sits in. The size and the recording session sit along the bottom. Names that are still too long to fit end in an ellipsis and show in full on hover.

- **Media file types are translated and filter the whole library**

  The type buttons above the list used to show the internal names `VIDEO`, `THUMBNAIL` and `DANMU_XML`, untranslated in every language, and they were built only from the page you happened to be looking at — so the buttons changed as you paged, and picking one filtered just that page. They now read as **Video**, **Audio**, **Thumbnail** and **Danmaku** in your language, each with the number of files of that type across your whole library, and picking one filters everything, not only the current page. The file count and total size next to the search box follow the type and search you have chosen, instead of only adding up what was on screen.

- **See how much disk space is left**

  The system health page said no more than "Healthy" about storage, so there was no way to tell whether a clean-up was due until space had already run low. A new **Storage** section now shows, for every disk your recordings are written to, how much space is free, how much of the disk is in use, and a bar that turns amber and then red as it fills. Disks are found from your output folders, including per-streamer, template and platform overrides, so a second drive is measured too; folders on the same disk are shown together. The dashboard's **Disk** card now leads with the free space on the fullest disk instead of just a status word.

- **Session timeline no longer hides events it can read**

  Lifecycle entries whose stored details were missing or unreadable were listed as an unrecognised event. A known entry — a session starting or ending — now shows with its proper label and a note that further details are unavailable.

- **Sidebar user menu**

  User account controls have moved to a dedicated user menu popup at the bottom of the sidebar. You can now access API key management, account settings, password changes, and sign out from a single place anywhere in the interface.

- **A workflow step no longer shows the wrong preset**

  Opening a step that uses a preset could show a different preset's settings, and **Detach & Edit** then replaced the step with the preset that was shown — an upload or conversion step could quietly turn into a delete step. A step now always shows the preset it names, and a step whose preset has been renamed or deleted says so instead of falling back to an unrelated one.

- **A brief server hiccup no longer signs you out**

  If the server was restarting, unreachable for a moment, or answering with an error while the browser was renewing your sign-in, you were thrown back to the login page and had to type your password again. This happened to everyone who had a tab open during a restart or update. A renewal that fails for any reason other than your sign-in genuinely having expired now leaves you signed in, and is simply tried again a moment later.

## Security

- **Baidu Netdisk login credentials no longer appear in process arguments**

  Manual and automatic BaiduPCS-Go logins now use private stdin/config staging. Command history is disabled only in the temporary directory; existing history, unrelated settings and other accounts are preserved. Successful account updates are committed atomically, failures preserve the previous config, and cancellation/timeout cleans up the contained child before releasing account access. See [Baidu Netdisk login](../concepts/pipeline.md#baidu-netdisk-baidupcs) for compatible binaries and storage boundaries.

- **Notification diagnostics redact credentials consistently**

  Channel configuration Debug output hides tokens, passwords, credential-bearing URLs, authentication headers, and private destinations, including nested service configuration. Web-push diagnostics also hide private VAPID keys, cached tokens, and subscription credentials. Serialization and delivery continue to use the configured values.

- **Successful logins preserve other failed-login attempts**

  Simultaneous login requests could cause one successful request to erase another request's failure from the login limit. Releasing a successful attempt now preserves other attempts from the same address.

- **Cookies, tokens and passwords are no longer written to the logs**

  Starting a recording, saving a platform, template or streamer, and the background check that keeps platform cookies fresh, all used to write the credentials themselves into the log files, the live log view and the browser console. Anyone able to read a log file, a log export or a container's logs could pick out the cookies, refresh tokens and account passwords for your platform accounts. Logs now record only which settings were touched, never their values. Old log files are not cleaned up, so if you have shared logs with anyone, change the cookies and passwords that appeared in them.

## Deployment

- **The sign-in cookie is protected automatically behind an HTTPS reverse proxy**

  When the web interface is published through a reverse proxy that terminates HTTPS, the cookie that keeps you signed in was only marked HTTPS-only if you set `COOKIE_SECURE=true` by hand; otherwise the browser was willing to send it over plain HTTP as well. The scheme reported by the proxy now reaches the app intact, so the cookie is marked HTTPS-only on its own. Plain-HTTP installs on a home or office network keep working as before, and `COOKIE_SECURE` still overrides the automatic choice in either direction. If a production install is still reached over plain HTTP, a warning is logged once to point this out.

- **Optional automatic container updates**

  The Docker Compose file now ships an opt-in `watchtower` service (`docker compose --profile autoupdate up -d`) that pulls new images and restarts the containers on its own — but only while the system is idle. A new unauthenticated `GET /api/health/idle` endpoint reports whether anything is recording, queued to record, or being processed by a pipeline job (upload, remux, danmaku conversion, ...); while it reports busy, the update is postponed to the next check, so a restart never cuts a recording or an upload short. Automatic updates require a mutable image tag (`VERSION=latest`). See [Upgrade and Rollback](../operations/upgrading.md#automatic-updates-watchtower).

## Installation

- **Locale-aware installation script**

  The `install.sh` bootstrap script now automatically detects the system locale (or respects `SREC_LANG`) and redirects to the English or Chinese interactive installer accordingly. The script verifies downloaded contents before execution to avoid running captive-portal error pages, and secret generation fails closed if secure random generation fails.

- **Building from source now requires Node.js 26**

  The web interface and the documentation site are now built with Node.js 26. If you build rust-srec from source, update Node before running the frontend — the repository now ships an `.nvmrc`, so version managers pick the right one for you. Docker and pre-built binary installations are unaffected.

- **The bundled systemd service file starts the recorder**

  Installing rust-srec as a system service with the `rust-srec.service` file from the repository did not work: the service stopped immediately with a permission error, before it ever reached its database, and the directories it needed had to be created by hand first. The file now writes logs to `/var/log/rust-srec`, has systemd create and own `/var/lib/rust-srec` and `/var/log/rust-srec` on every start, and picks up `JWT_SECRET` and any other secrets from `/etc/rust-srec/rust-srec.env`. Stopping the service now waits long enough for recordings in progress to be saved, the fixed memory and CPU caps are opt-in so they cannot cut a recording short on a machine they do not suit, and the file's header lists the commands needed to install it. After the first start, set the recording folder in the web interface: the built-in default points inside the Docker image, so a system service records nothing until you change it. Recordings made by the service are readable only by the `rust-srec` account and its group, so add any other account that needs them — a file server, a media scanner — to that group. Docker installations are unaffected.

## Desktop

- **Closing the app finishes the recording first**

  Quitting the desktop app while a stream was recording could leave the recording tool running in the background or cut the file short. The app now waits for the recording to be saved before it exits, up to a one-minute limit.

- **Only one instance can use a recording database**

  The desktop app and a separately-run rust-srec server could both open the same database at the same time, each unaware of the other, and record the same streamers twice. Whichever starts second now stops instead.

- **Fixed SQLite lock on first launch**

  The desktop application now establishes SQLite WAL mode through a dedicated bootstrap connection before opening the read and write connection pools. Previously, concurrent initialization of both pools caused transient `SQLITE_BUSY` errors when opening fresh database files on first launch, because SQLite requires an exclusive lock when switching journal modes that cannot wait for a busy timeout. Reusable connection pool options also no longer repeat the journal-mode pragma during pool growth.

- **Actionable boot failure and recovery screen**

  When the desktop application encounters an unrecoverable startup error (such as a locked database, permission denial, full storage, or a corrupted database image), it now displays a dedicated safe-mode recovery screen instead of silently crashing or failing to launch. The interface highlights the exact failure stage and error kind, provides actionable troubleshooting guidance, lets you open the data and log folders directly, and allows one-click copying of full diagnostic details.
