## rust-srec v0.6.0

This release adds multiple accounts per platform, saved proxies, API keys and MCP access, Baidu Netdisk uploads, lossless cutting of live recordings, automatic output cleanup, per-step workflow retries, a reworked live player, and configurable danmu statistics. It also fixes recording shutdown, restart recovery, file handling, and credential exposure in logs.

### Highlights
- **Multiple accounts per platform** — save several accounts for a platform and choose one fixed account or a pool that rotates or falls back when a login fails. Accounts are managed on the platform page, with QR login for Bilibili and Douyu, and each account can use its own proxy.
- **Saved proxies** — save proxies once under **Settings → Proxies**, test them, and choose direct, system proxy, or a saved proxy for global settings, each platform, template, streamer, and account. Rate limiting on one proxy pauses only the requests that go through it.
- **API keys and MCP** — expiring or non-expiring `read_only` and `full` API keys, plus a built-in MCP server at `/api/mcp` for recording queries, danmu analysis, and configuration management.
- **Baidu Netdisk uploads** — a new `baidupcs` processor with BaiduPCS-Go bundled in Docker, login from the preset editor, destination templates, and per-file results.
- **Lossless cutting** — **Split file now** finishes the current file and continues recording in a new one without reconnecting or re-encoding. FFmpeg and Streamlink need the experimental **Enable lossless cutting** engine option.
- **Output retention** — optionally remove outputs from ended sessions after a chosen number of days, as records only or with their files.
- **Workflow retries and timeouts** — per-step retry counts, backoff, and timeouts that survive restarts, plus save-time workflow validation and batch actions for pipeline jobs and media outputs.
- **Platforms and player** — TikTok danmu, Douyu Android App extraction, restored RedBook extraction, and a live player with connection controls, playback recovery, stream pickers, and latency presets.
- **Danmu statistics** — configurable rankings, live snapshots that survive restarts, gift and chatter statistics, and word segmentation for Chinese and Japanese.
- **Reliability** — recordings, chat files, and pipeline work are finalized at shutdown and resumed after restart without repeating completed steps. Credentials no longer appear in logs.

### Review before upgrading
- **Take a backup first.** On first start, cookies and logins in platform, template, and streamer settings become account profiles, and proxy settings become saved proxies. Going back to an earlier version needs the backup.
- **Direct means direct.** With **Direct**, FFmpeg and Streamlink now ignore `HTTP_PROXY`, `HTTPS_PROXY`, and `ALL_PROXY`; choose **System proxy** where they should still apply.
- **API changes.** Requests that still send `cookies` or `proxy_config` are rejected; use `/api/credentials` with `credential_selection`, and `proxy_route`. API clients must serialize token refreshes, since reusing a consumed refresh token can revoke all of that user's sessions.
- **Douyu defaults to the App method.** Without a signed-in account, rooms with several qualities usually stay at 超清; add a signed-in account for the original quality.
- **Mesio Loop Protection** and Offset Consistency Check are now off by default for new engines; engines saved earlier keep their settings.
- **Update the web container and the backend together**, and rotate any credentials that appeared in logs you shared before.

Full release notes: https://docs.srec.rs/en/release-notes/v0.6.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.6.0
