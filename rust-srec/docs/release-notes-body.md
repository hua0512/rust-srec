## rust-srec v0.8.0

This release shows recorded chat over the video on the session page and live chat on the **Player** page, adds H.265 and AV1 recording for Twitch, a low disk space notification and a way to keep checking streamers without an account when every account in a pool needs attention. It also keeps Bigo checks going through login requests, fixes long or broken FLV recordings from the Mesio engine, closes several ways another site could act as the signed-in user, and fixes a broad range of configuration, backup, notification and web interface problems.

### Highlights
- **Chat over the video** — recordings on the session page play their recorded chat timed to the picture, and live streams on the **Player** page show their chat as it arrives, with a **Danmu settings** panel for message types, opacity, size, speed, coverage, merging and blocked words or users.
- **Twitch H.265 and AV1** — new **Allow H.265 (HEVC)** and **Allow AV1** Twitch settings record the higher qualities some channels stream only in those codecs. Both are off by default.
- **Low disk space notification** — sent when a disk holding recordings or the database reaches 95% used.
- **Check without an account** — an account pool can keep checking and recording streamers without an account while every account in it needs a new login or is disabled.
- **Bigo** — a single login request no longer marks an account as needing a new login, accounts are refreshed hourly, and Mesio recordings keep the stream information Bigo sends scrambled.
- **FLV recordings with Mesio** — exact lengths, correct file details, splitting across stream restarts, seeking in browser players, and fixes for recordings that shrank, drifted out of sync or failed to copy and convert.
- **Security** — the stream proxy, the web interface and recorded files opened from the server refuse requests and content that let another site act as the signed-in user, and sign-in passwords and tokens are kept out of logs.
- **Fixes** — Streamlink recordings honour **Max Part Size**, long multi-byte titles no longer break file names, Twitch, Twitcasting and YouTube checks report the right state, backups keep **Stream lookup** choices and report unreadable parts, and lists and pages that fail to load offer **Retry**.

### Review before upgrading
- **Invalid environment values.** An invalid `API_PORT`, `ACCESS_TOKEN_EXPIRATION_SECS`, `REFRESH_TOKEN_EXPIRATION_SECS`, `REFRESH_TOKEN_REUSE_GRACE_SECS` or `MIN_PASSWORD_LENGTH` now stops startup instead of falling back to the default. Check for values followed by a comment on the same line, such as `ACCESS_TOKEN_EXPIRATION_SECS=3600 # 1 hour`, in files loaded with `docker run --env-file` or systemd's `EnvironmentFile=`.
- **Reverse proxies.** Actions requested from a page on another port or subdomain of the same address are refused. If every action fails with "Request refused" behind a reverse proxy, forward the original address or list it in `CSRF_TRUSTED_ORIGINS`.
- **API error responses.** Malformed bodies, query strings and URL paths now get a JSON error with a `code` instead of plain text; status codes are unchanged.
- **Streamlink part size.** Streamlink recordings now split at **Max Part Size** unless **Max Duration** is also set.
- **`FFMPEG_PATH`.** When set, it is also used for recording by FFmpeg engines whose **Binary Path** is empty.
- **Older backups** contain no **Stream lookup** choices, so importing one resets them to the default. Take a new backup after updating.
- **List page links.** Saved `?page=` links to the media outputs, pipeline jobs, presets or workflows lists open one page earlier, because those lists now count pages from 1.

Full release notes: https://docs.srec.rs/en/release-notes/v0.8.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.8.0
