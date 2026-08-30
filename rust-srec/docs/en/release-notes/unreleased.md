# Release Notes

## `unreleased`

## Recording

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

## API and integrations

- **API keys for programmatic access**

  You can now create long-lived API keys as an alternative to short-lived JWT session tokens. Keys belong to the user who created them, carry an optional expiration timestamp, and can be scoped to either `read_only` (access to non-sensitive queries such as sessions, danmu, aggregate statistics, notification events, and system health) or `full` access (all requests including configuration changes and mutations). Keys are stored as SHA-256 hashes and displayed only once at creation. Revoking a key invalidates it immediately across the server and clears any authorization cache. API keys cannot manage other keys or change passwords, and WebSocket media/download streams continue to require JWT tokens to prevent keys from leaking into URLs or access logs. See [API Keys & MCP](../api/api-keys-mcp.md).

- **Built-in Model Context Protocol (MCP) server**

  The backend now exposes a built-in MCP server using the streamable HTTP transport at `/api/mcp`. AI assistants such as Claude Code, Claude Desktop, and Cursor can connect directly using an API key to inspect recording sessions, analyze danmu activity and word frequency, read raw chat XML with byte pagination, observe pipeline jobs, manage streamers, and update configuration. Tools execute in-process against existing application services, sharing the same validations and dynamic updates. Read-only keys are restricted to safe inspection tools and cannot access configuration or credentials. See [API Keys & MCP](../api/api-keys-mcp.md).

- **Dedicated API key management in the Web UI**

  A new **Settings → API Keys** page lets you create, inspect, and revoke API keys with custom names and expiry dates. The page also generates ready-to-copy MCP configuration snippets for Claude Code, Cursor, and standard MCP clients.

## Pipeline and uploads

- **Upload recordings to Baidu Netdisk**

  A new `baidupcs` pipeline processor uploads recordings to Baidu Netdisk through the BaiduPCS-Go command-line tool, which is now bundled in the Docker image. Add it to a pipeline like any other upload step: the destination folder supports the usual streamer/title/date placeholders, same-name files can be skipped or overwritten, and uploads appear in the same live progress, per-file records and streamer-card indicators as rclone transfers. Log in from the preset editor — paste your netdisk cookies (or BDUSS and STOKEN) once and the account card shows who is signed in and how much space is left. Tick **Remember for automatic re-login** and upload jobs log in again by themselves when the session expires, so a recording made at night still lands in the netdisk without anyone clicking Login; leave it unticked and the credentials are handed to BaiduPCS-Go without the app keeping them. If the remembered credentials themselves stop working, a notification tells you to log in again and further attempts pause for an hour instead of hammering Baidu. Logging out forgets the remembered credentials. Because BaiduPCS-Go's exit code does not reflect upload results, rust-srec reads the tool's per-file output instead, and a retried job re-sends only the files that did not make it. See [DAG Pipeline](../concepts/pipeline.md#baidu-netdisk-baidupcs).

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

- **Session timeline no longer hides events it can read**

  Lifecycle entries whose stored details were missing or unreadable were listed as an unrecognised event. A known entry — a session starting or ending — now shows with its proper label and a note that further details are unavailable.

- **Sidebar user menu**

  User account controls have moved to a dedicated user menu popup at the bottom of the sidebar. You can now access API key management, account settings, password changes, and sign out from a single place anywhere in the interface.

## Deployment

- **Optional automatic container updates**

  The Docker Compose file now ships an opt-in `watchtower` service (`docker compose --profile autoupdate up -d`) that pulls new images and restarts the containers on its own — but only while the system is idle. A new unauthenticated `GET /api/health/idle` endpoint reports whether anything is recording, queued to record, or being processed by a pipeline job (upload, remux, danmaku conversion, ...); while it reports busy, the update is postponed to the next check, so a restart never cuts a recording or an upload short. Automatic updates require a mutable image tag (`VERSION=latest`). See [Upgrade and Rollback](../operations/upgrading.md#automatic-updates-watchtower).

## Installation

- **Locale-aware installation script**

  The `install.sh` bootstrap script now automatically detects the system locale (or respects `SREC_LANG`) and redirects to the English or Chinese interactive installer accordingly. The script verifies downloaded contents before execution to avoid running captive-portal error pages, and secret generation fails closed if secure random generation fails.

- **Building from source now requires Node.js 26**

  The web interface and the documentation site are now built with Node.js 26. If you build rust-srec from source, update Node before running the frontend — the repository now ships an `.nvmrc`, so version managers pick the right one for you. Docker and pre-built binary installations are unaffected.

## Desktop

- **Closing the app finishes the recording first**

  Quitting the desktop app while a stream was recording could leave the recording tool running in the background or cut the file short. The app now waits for the recording to be saved before it exits, up to a one-minute limit.

- **Only one instance can use a recording database**

  The desktop app and a separately-run rust-srec server could both open the same database at the same time, each unaware of the other, and record the same streamers twice. Whichever starts second now stops instead.

- **Fixed SQLite lock on first launch**

  The desktop application now establishes SQLite WAL mode through a dedicated bootstrap connection before opening the read and write connection pools. Previously, concurrent initialization of both pools caused transient `SQLITE_BUSY` errors when opening fresh database files on first launch, because SQLite requires an exclusive lock when switching journal modes that cannot wait for a busy timeout. Reusable connection pool options also no longer repeat the journal-mode pragma during pool growth.

- **Actionable boot failure and recovery screen**

  When the desktop application encounters an unrecoverable startup error (such as a locked database, permission denial, full storage, or a corrupted database image), it now displays a dedicated safe-mode recovery screen instead of silently crashing or failing to launch. The interface highlights the exact failure stage and error kind, provides actionable troubleshooting guidance, lets you open the data and log folders directly, and allows one-click copying of full diagnostic details.


