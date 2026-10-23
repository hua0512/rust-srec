## rust-srec v0.7.0

This release adds MPEG-DASH recording with Mesio, TikTok DASH streams and HTTP/2 downloads for HLS and DASH, and shows live progress, speed and time remaining for workflow steps such as remuxing, copies, compression and uploads. It also redesigns the engine settings, lists partial files left by interrupted jobs, and fixes missing or repeated HLS segments, webhook notifications, stuck copies and a broad range of recording, workflow and web interface problems.

### Highlights
- **MPEG-DASH recording** — Mesio records live and finished `.mpd` streams, merging separate video and audio into one MP4, with a new **DASH** tab for quality, audio language, start position and download limits. Encrypted (DRM) streams are not supported.
- **TikTok DASH streams** — TikTok rooms now also list their DASH streams; FLV stays the default unless `dash` is put first in **Preferred Formats**.
- **Faster, sturdier HLS** — with **HTTP Version** on **Auto**, Mesio uses HTTP/2 for HLS and DASH on direct connections, backs off when a server answers HTTP 429 instead of ending the recording, and no longer leaves out slow, retried or repeated segments of finished streams.
- **Live workflow progress** — remuxing, transcoding, copies, compression and rclone/Baidu Netdisk uploads show percentage, speed, time remaining and the current file on the job page, the workflow page and the top bar, including automatic retries.
- **Interrupted jobs** — partial files left by a crash or forced exit are listed for review and cleanup, and copies stuck on an unresponsive disk or share are shown instead of running on unnoticed.
- **Engine settings** — Mesio, FFmpeg and Streamlink settings are regrouped into per-format tabs and sections with an **Advanced** area, and template overrides can inherit, reset or override every setting.
- **New workflow** — **Full Processing (Single Pass)** writes recording metadata while remuxing instead of rewriting the file in a separate step.
- **Fixes** — webhook channels from the web interface now send notifications, rclone **Max Retries** takes effect, Bigo login walls and Douyin mobile-API rooms are no longer shown as offline, and pages no longer overflow at laptop and desktop sizes.

### Review before upgrading
- **System certificates.** Mesio downloads and most desktop-app connections now trust the operating system's certificates. If connections fail with a certificate error, update the system's certificates, such as `ca-certificates` on Linux.
- **Native-resolution thumbnails** that were silently saved at 320 pixels wide are not repaired: turn on **Native resolution** and save them again.
- **HTTP/2 only** now really uses HTTP/2, so it fails against servers without HTTP/2.
- **Removed settings** that had no effect: **Rclone Executable** (use `RCLONE_PATH`), the thumbnail **Output Pattern**, Mesio's **Live Reorder Window** and the **Settings updated** notification event.

Full release notes: https://docs.srec.rs/en/release-notes/v0.7.0 · 中文版：https://docs.srec.rs/zh/release-notes/v0.7.0
