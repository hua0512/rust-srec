# Release Notes

## `unreleased`

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

- **Live danmu statistics while recording**

  Danmu statistics no longer wait for the stream to end: while a recording is running, a snapshot is saved about once a minute, so the session page's danmu panel (totals, activity timeline, top talkers, frequent words) fills in while the stream is still live. If the app crashes or the host reboots mid-recording, at most the last minute of statistics is lost instead of the whole session's.

- **Activity timeline covers the whole stream on long sessions**

  The danmu activity chart used to keep only the most recent six hours at full detail and silently dropped the oldest points on longer sessions. Once the limit is reached it now halves the chart's resolution instead, so a 12-hour recording still charts from the first minute to the last — just at a coarser granularity.

- **Expandable Top Talkers**

  The Top Talkers card on the session page shows the six most active chatters by default and can now be expanded to the full ranking (up to 32 users are tracked per session).

- **Better word splitting for Chinese chat**

  The frequent-words statistic now treats full-width punctuation (`,` `。` `!` `?` ...), symbols and emoji as word separators. Previously only spaces and ASCII punctuation split words, so a Chinese message without spaces was counted as one giant "word".

- **Unique chatters metric**

  The session danmu panel now shows how many distinct users chatted during the stream (a memory-bounded estimate, accurate to about 1–2%), alongside the total message count. Sessions recorded before this release show a dash.

- **Gift rankings**

  For platforms that report gifts in chat (Bilibili, Douyu, Bigo, SOOP, ...), the session page now shows two extra charts: the top gift senders and the most-sent gifts, both weighted by the number of gift items rather than messages. The charts only appear when the stream actually received gifts.

- **Removed the `danmu_sampling_config` setting**

  This template/streamer setting never had any effect — statistics have always counted every message. The field has been removed from the REST API (`/api/templates`) and the database; existing configurations are cleaned up automatically, and older exports that still contain the field import fine.

## Web interface

- **Sidebar user menu**

  User account controls have moved to a dedicated user menu popup at the bottom of the sidebar. You can now access API key management, account settings, password changes, and sign out from a single place anywhere in the interface.

## Installation

- **Locale-aware installation script**

  The `install.sh` bootstrap script now automatically detects the system locale (or respects `SREC_LANG`) and redirects to the English or Chinese interactive installer accordingly. The script verifies downloaded contents before execution to avoid running captive-portal error pages, and secret generation fails closed if secure random generation fails.

